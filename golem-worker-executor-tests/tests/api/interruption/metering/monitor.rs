use super::*;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

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
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
        }),
    )
    .await?;
    if stale {
        // No Worker exists yet, so a full refresh cannot wait for its filesystem reply.
        let entry = limits.initialize_account(context.account_id).await?;
        let initial_revision = registry.current_limits().monthly_policy_revision;
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        tokio::time::timeout(Duration::from_secs(30), limits.run_batch_for_test()).await?;
        let exhausted_revision = registry.current_limits().monthly_policy_revision;
        assert_eq!(exhausted_revision, initial_revision + 1);
        assert_eq!(
            limits
                .initialized_policy_revision_for_test(context.account_id)
                .await,
            Some(exhausted_revision)
        );
        assert!(entry.monthly_capacity_is_exhausted_for_test(
            golem_common::model::agent::AgentMode::Durable
        ));
        registry.limits.lock().unwrap().monthly_policy = policy.clone();
        tokio::time::timeout(Duration::from_secs(30), limits.run_batch_for_test()).await?;
        assert_eq!(
            limits
                .initialized_policy_revision_for_test(context.account_id)
                .await,
            Some(exhausted_revision)
        );
        assert_eq!(
            registry.current_limits().monthly_policy.period,
            policy.period
        );
        assert!(!entry.monthly_capacity_is_exhausted_for_test(
            golem_common::model::agent::AgentMode::Durable
        ));
    }
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
    limits.initialize_account(context.account_id).await?;
    let active_worker = executor
        .production_active_agent(&owned)
        .await
        .unwrap()
        .primary();

    let mut refreshes = Vec::new();
    if stale {
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut closed_rx)
                .await
                .is_err()
        );
        assert!(
            !invocation.is_finished(),
            "same-revision credit must allow the new silent window to remain live"
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

    executor.interrupt(&worker).await?;
    assert!(tokio::time::timeout(Duration::from_secs(30), closed_rx).await??);
    let error = tokio::time::timeout(Duration::from_secs(30), invocation)
        .await??
        .expect_err("user interrupt must remain effective under every metering configuration");
    assert!(!error.to_string().contains("monthly"));
    active_worker.join_accepted_stops_for_test().await?;
    active_worker.retained_cleanup_for_test().await?;
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
async fn silent_tcp_after_same_revision_capacity_credit_stays_live(
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

mod retries {
    use super::*;
    use anyhow::Context as _;
    use golem_common::model::oplog::{PublicAgentInvocation, PublicOplogEntry};
    use golem_service_base::error::worker_executor::InterruptKind;
    use pretty_assertions::assert_eq;
    use test_r::test;
    use tokio::io::AsyncReadExt;

    #[test]
    #[timeout("60s")]
    async fn quota_exhaustion_after_capacity_updates_settles_and_recovers(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let available = MonthlyResourcePolicy {
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
        let registry = Arc::new(MutableResourceLimitsRegistry::new(available.clone()));
        let shutdown = CancellationToken::new();
        let metering = ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        };
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
            Arc::new(move |config| {
                config.agent_sockets.enabled = true;
                config.resource_usage_metering = metering;
            }),
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let name = agent_id!("Networking", "monthly-controlled-tick");
        let id = executor.start_agent(&component.id, name.clone()).await?;
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let active = executor.production_active_agent(&owned).await.unwrap();
        let worker = active.primary();
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.eviction_class().await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("initial idle release")?;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
        let (closed_tx, mut closed_rx) = tokio::sync::oneshot::channel();
        let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = connected_tx.send(());
            let mut byte = [0];
            let _ = closed_tx.send(matches!(socket.read(&mut byte).await, Ok(0)));
        }));
        let key = IdempotencyKey::fresh();
        let invocation = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let name = name.clone();
            let key = key.clone();
            async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &name,
                        &key,
                        "tcp_collect_p3",
                        data_value!(port),
                    )
                    .await
            }
        }));
        tokio::time::timeout(Duration::from_secs(10), connected_rx).await??;
        // Connection acceptance precedes the guest's receive call. Require its open durable chunk.
        let chunk = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if let Some(entry) = entries.iter().find(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::Start(start)
                            if start.function_name == "sockets::types::tcp-socket::receive-chunk"
                    )
                }) {
                    return Ok::<_, anyhow::Error>(entry.oplog_index);
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        let original_raw = executor.stored_oplog(&id).await;
        let mut raw = worker.raw_interrupt_for_test();

        registry.set_policy(available.clone());
        let first_refresh = applied_refresh(&limits, &registry, context.account_id).await?;
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            closed_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        registry.set_policy(available.clone());
        let second_refresh = applied_refresh(&limits, &registry, context.account_id).await?;
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            closed_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        let (selected, release_outcome) = executor.gate_next_invocation_outcome_append(&id).await;
        let mut exhausted = available.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let final_refresh = applied_refresh(&limits, &registry, context.account_id).await?;
        tokio::time::timeout(Duration::from_secs(10), selected).await??;
        let quota_stop = worker.await_interrupt_for_test().await;
        assert!(matches!(quota_stop, InterruptKind::Suspend(_)));
        assert!(!invocation.is_finished());
        assert!(matches!(
            closed_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        let mut receipt = published_interrupt_receipt(worker.as_ref()).await?;
        release_outcome.send(false).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
            quota_stop
        );
        assert!(tokio::time::timeout(Duration::from_secs(10), closed_rx).await??);
        tokio::time::timeout(Duration::from_secs(10), async {
            worker.join_accepted_stops_for_test().await?;
            while worker.is_loaded().await {
                tokio::task::yield_now().await;
            }
            worker.retained_cleanup_for_test().await
        })
        .await
        .context("joined physical cleanup")??;
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed)
        ));
        // Capacity application precedes the worker's filesystem reply and physical cleanup.
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::try_join!(first_refresh, second_refresh, final_refresh)
        })
        .await??;
        limits.run_batch_for_test().await;
        let billed = registry.applied_memory_byte_nanoseconds();
        assert!(billed > 0, "pre-exhaustion memory remains billable");
        limits.run_batch_for_test().await;
        assert_eq!(registry.applied_memory_byte_nanoseconds(), billed);

        executor
            .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
            .await?;
        let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| matches!(&entry.entry,
            PublicOplogEntry::AgentInvocationStarted(start)
                if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.idempotency_key == key)))
                .count(),
            1
        );
        assert_eq!(
            count_agent_invocation_pair_since(&oplog, OplogIndex::INITIAL),
            (2, 1)
        );
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Suspend(_)))
                .count(),
            1
        );
        assert!(oplog.iter().all(|entry| !matches!(
            &entry.entry,
            PublicOplogEntry::Interrupted(_) | PublicOplogEntry::Error(_)
        )));
        assert!(oplog.iter().all(
            |entry| !matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == chunk)
        ));
        assert!(oplog.iter().all(|entry| !matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == chunk)));
        let stopped_raw = executor.stored_oplog(&id).await;
        assert!(stopped_raw.len() >= original_raw.len());
        assert_eq!(&stopped_raw[..original_raw.len()], original_raw.as_slice());
        server.await?;

        registry.set_policy(available);
        limits.run_batch_for_test().await;
        executor.resume(&id, false).await?;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), invocation)
                .await???
                .into_typed::<Result<String, String>>()?,
            Err("ErrorCode::ConnectionBroken".to_string())
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.eviction_class().await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let recovered_raw = executor.stored_oplog(&id).await;
        assert!(recovered_raw.len() >= stopped_raw.len());
        assert_eq!(&recovered_raw[..stopped_raw.len()], stopped_raw.as_slice());
        let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL),
            (2, 2)
        );
        assert_eq!(
            recovered
                .iter()
                .filter(|entry| matches!(&entry.entry,
            PublicOplogEntry::AgentInvocationStarted(start)
                if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.idempotency_key == key)))
                .count(),
            1
        );
        shutdown.cancel();
        Ok(())
    }
}
