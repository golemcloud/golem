use super::*;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("2m")]
async fn durable_monthly_memory_tick_interrupts_silent_tcp_and_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    monthly_memory_exhaustion_interrupts_silent_tcp(
        last_unique_id,
        deps,
        host_api_tests,
        "Networking",
        true,
        true,
    )
    .await
}

#[test]
#[timeout("2m")]
async fn ephemeral_monthly_memory_tick_interrupts_silent_tcp_with_terminal_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    monthly_memory_exhaustion_interrupts_silent_tcp(
        last_unique_id,
        deps,
        host_api_tests,
        "EphemeralNetworking",
        false,
        true,
    )
    .await
}

async fn silent_tcp_monitor_controls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    metering: ResourceUsageMeteringConfig,
    stale: bool,
) -> anyhow::Result<()> {
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("Networking", "monitor-controls");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, mut closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = connected_tx.send(());
        let mut byte = [0_u8];
        let closed = matches!(socket.read(&mut byte).await, Ok(0) | Err(_));
        let _ = closed_tx.send(closed);
    });
    let invocation = {
        let executor = executor.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(&component, &agent, "tcp_collect_p3", data_value!(port))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), connected_rx).await??;
    let entry = limits.initialize_account(context.account_id).await?;
    let enabled = metering.compute || metering.memory || metering.filesystem;
    assert_eq!(
        entry.monthly_observer_count_for_test(),
        usize::from(enabled)
    );
    assert!(executor.concurrent_agent_permit_is_held(&owned).await);

    let mut refreshes = Vec::new();
    if stale {
        let active = executor.production_active_agent(&owned).await.unwrap();
        let (entered, release) = active.primary().pause_next_monthly_acceptance_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        refreshes.push(applied_refresh(&limits, &registry, context.account_id).await?);
        tokio::time::timeout(Duration::from_secs(30), entered).await??;
        // Capacity can recover without a policy revision (for example, an account credit).
        registry.limits.lock().unwrap().monthly_policy = policy.clone();
        refreshes.push(applied_refresh(&limits, &registry, context.account_id).await?);
        tokio::time::timeout(Duration::from_secs(30), async {
            while entry.monthly_capacity_is_exhausted_for_test(
                golem_common::model::agent::AgentMode::Durable,
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        release.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut closed_rx)
                .await
                .is_err()
        );
        assert!(
            !invocation.is_finished(),
            "a stale exhaustion proposal must not stop the held window"
        );
    }

    let mut disabled_exhaustion = policy.clone();
    if !metering.compute {
        disabled_exhaustion.available_fuel = 0;
    }
    if !metering.memory {
        disabled_exhaustion.available_memory_gb_seconds = 0;
    }
    if !metering.filesystem {
        disabled_exhaustion.available_durable_storage_byte_seconds = 0;
        disabled_exhaustion.available_ephemeral_storage_byte_seconds = 0;
    }
    registry.set_policy(disabled_exhaustion);
    let first_refresh = applied_refresh(&limits, &registry, context.account_id).await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut closed_rx)
            .await
            .is_err()
    );
    assert!(!invocation.is_finished());
    assert_eq!(
        entry.monthly_observer_count_for_test(),
        usize::from(enabled)
    );

    let mut overage = policy;
    overage.mode = MonthlyUsageMode::AllowOverage;
    overage.available_fuel = 0;
    overage.available_memory_gb_seconds = 0;
    overage.available_durable_storage_byte_seconds = 0;
    overage.available_ephemeral_storage_byte_seconds = 0;
    {
        let mut current = registry.limits.lock().unwrap();
        current.monthly_policy = overage;
        current.monthly_usage_mode_revision += 1;
        current.monthly_policy_revision += 1;
    }
    let second_refresh = applied_refresh(&limits, &registry, context.account_id).await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut closed_rx)
            .await
            .is_err()
    );
    assert!(!invocation.is_finished());
    assert!(executor.concurrent_agent_permit_is_held(&owned).await);

    executor.interrupt(&worker).await?;
    assert!(tokio::time::timeout(Duration::from_secs(30), closed_rx).await??);
    let error = tokio::time::timeout(Duration::from_secs(30), invocation)
        .await??
        .expect_err("user interrupt must remain effective under every metering configuration");
    assert!(!error.to_string().contains("monthly"));
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.concurrent_agent_permit_is_held(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(entry.monthly_observer_count_for_test(), 0);
    refreshes.extend([first_refresh, second_refresh]);
    for refresh in refreshes {
        tokio::time::timeout(Duration::from_secs(30), refresh).await??;
    }
    limits.run_batch_for_test().await;
    for update in registry.applied_updates() {
        if !metering.compute {
            assert_eq!(update.fuel_delta, 0);
        }
        if !metering.memory {
            assert_eq!(update.memory_gb_seconds_delta, 0);
            assert_eq!(update.memory_byte_nanoseconds_remainder, 0);
        }
        if !metering.filesystem {
            assert_eq!(update.durable_storage_byte_seconds_delta, 0);
            assert_eq!(update.ephemeral_storage_byte_seconds_delta, 0);
        }
    }
    server.await?;
    shutdown.cancel();
    Ok(())
}

pub(super) async fn applied_refresh(
    limits: &Arc<ResourceLimitsGrpc>,
    registry: &MutableResourceLimitsRegistry,
    account: AccountId,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let revision = registry.current_limits().monthly_policy_revision;
    let refresh = {
        let limits = limits.clone();
        tokio::spawn(async move { limits.run_batch_for_test().await })
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        while limits.initialized_policy_revision_for_test(account).await != Some(revision) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(refresh)
}

macro_rules! monitor_control {
    ($name:ident, $compute:expr, $memory:expr, $filesystem:expr) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            silent_tcp_monitor_controls(
                last_unique_id,
                deps,
                host_api_tests,
                ResourceUsageMeteringConfig {
                    compute: $compute,
                    memory: $memory,
                    filesystem: $filesystem,
                },
                false,
            )
            .await
        }
    };
}

#[test]
#[timeout("2m")]
async fn silent_tcp_stale_same_revision_proposal_is_rejected(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    silent_tcp_monitor_controls(
        last_unique_id,
        deps,
        host_api_tests,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        },
        true,
    )
    .await
}

monitor_control!(silent_tcp_monitor_000, false, false, false);
monitor_control!(silent_tcp_monitor_001, false, false, true);
monitor_control!(silent_tcp_monitor_010, false, true, false);
monitor_control!(silent_tcp_monitor_011, false, true, true);
monitor_control!(silent_tcp_monitor_100, true, false, false);
monitor_control!(silent_tcp_monitor_101, true, false, true);
monitor_control!(silent_tcp_monitor_110, true, true, false);
monitor_control!(silent_tcp_monitor_111, true, true, true);
