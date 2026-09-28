use super::*;
use anyhow::Context as _;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::{MonthlyClockForTest, Worker};
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

mod nonmatching;

#[test]
#[timeout("2m")]
async fn durable_pending_scripted_storage_suspends_and_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_scripted_storage(last_unique_id, deps, host_api_tests, AgentMode::Durable).await
}

#[test]
#[timeout("2m")]
async fn ephemeral_pending_scripted_storage_fails_with_matching_resource_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_scripted_storage(last_unique_id, deps, host_api_tests, AgentMode::Ephemeral).await
}

async fn pending_scripted_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    mode: AgentMode,
) -> anyhow::Result<()> {
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: 0,
        available_memory_gb_seconds: 0,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: 1,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: 1,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: false,
        filesystem: true,
    };
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
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("Networking", "scripted-storage-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("seed permit release")?;
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "scripted-storage")
    } else {
        agent_id!("EphemeralNetworking", "scripted-storage")
    };
    let key = IdempotencyKey::fresh();
    let physical = if durable {
        name.clone()
    } else {
        name.clone()
            .with_ephemeral_invocation_phantom(&key)
            .unwrap()
    };
    let id = AgentId::from_agent_id(component.id, &physical).unwrap();
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = Worker::get_or_create_suspended(
        seed.all(),
        &owned,
        None,
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        Principal::anonymous(),
    )
    .await?;
    // Only the allocation observation is scripted. Worker startup, the permit, the meter,
    // filesystem generation deletion and cooperative TCP teardown remain real.
    let source = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    worker
        .owner_runtime_resources()
        .set_scripted_filesystem_usage_for_test(source.clone());
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let invocation = tokio::spawn({
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
    });
    let (mut socket, _) =
        tokio::time::timeout(Duration::from_secs(30), listener.accept()).await??;
    let chunk = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            executor.commit_oplog(&id).await?;
            let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            if let Some(entry) = entries.iter().find(|e| matches!(&e.entry, PublicOplogEntry::Start(start) if start.function_name == "sockets::types::tcp-socket::receive-chunk")) {
                return Ok::<_, anyhow::Error>(entry.oplog_index);
            }
            tokio::task::yield_now().await;
        }
    }).await.context("open receive chunk")??;
    tokio::time::timeout(Duration::from_secs(10), polls.recv())
        .await
        .context("monthly timer armed")?
        .unwrap();
    let initial = worker.current_monthly_proposal_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let acquisitions = worker.permit_acquisitions_for_test();
    assert_eq!(initial.exhaustion, None);
    assert!(worker.concurrent_agent_permit_is_held().await);
    let account = limits.initialize_account(context.account_id).await?;
    assert!(!account.monthly_capacity_is_exhausted_for_test(mode));
    assert!(!invocation.is_finished());
    let observations_before_allocation = source.observations();
    source.set(FilesystemUsage::Authoritative {
        allocated_bytes: 101,
        filesystem_objects: 1,
    });
    // The system-clock sampler must accept the nonzero scripted observation and accrue usage.
    // Registry still grants one byte-second, not zero.
    tokio::time::timeout(Duration::from_secs(10), async {
        while source.observations() == observations_before_allocation {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("positive scripted observation")?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !invocation.is_finished(),
        "no Store checkpoint runs while TCP is pending"
    );
    clock.advance(Duration::from_secs(30));
    let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await
        .context("storage exhaustion proposal")?
        .unwrap();
    assert_eq!(attempt.proposal.policy_revision, initial.policy_revision);
    assert_eq!(attempt.proposal.window_identity, initial.window_identity);
    assert_eq!(
        attempt.proposal.exhaustion,
        Some(if durable {
            "monthly durable storage exhausted"
        } else {
            "monthly ephemeral storage exhausted"
        })
    );
    assert!(attempt.completed.await?);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        InterruptKind::Suspend(_)
    ));
    let other = if durable {
        AgentMode::Ephemeral
    } else {
        AgentMode::Durable
    };
    assert!(!account.monthly_capacity_is_exhausted_for_test(other));
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), socket.read(&mut byte))
            .await
            .context("socket close")??,
        0
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("physical window release")?;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    assert_eq!(clock.active_sleeps(), 0);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert!(worker.unload_succeeded_for_test());
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    let observations = source.observations();
    limits.run_batch_for_test().await;
    let settled = registry.applied_updates();
    assert!(
        settled
            .iter()
            .any(|u| u.monthly_policy_revision == initial.policy_revision
                && u.period == initial.period
                && if durable {
                    u.durable_storage_byte_seconds_delta == 1
                } else {
                    u.ephemeral_storage_byte_seconds_delta == 1
                })
    );
    for update in &settled {
        assert_eq!(update.fuel_delta, 0);
        assert_eq!(update.memory_gb_seconds_delta, 0);
        assert_eq!(update.memory_byte_nanoseconds_remainder, 0);
        assert_eq!(
            if durable {
                update.ephemeral_storage_byte_seconds_delta
            } else {
                update.durable_storage_byte_seconds_delta
            },
            0
        );
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    limits.run_batch_for_test().await;
    assert_eq!(
        source.observations(),
        observations,
        "closed generations must stop sampling"
    );
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    let errors: Vec<_> = entries
        .values()
        .filter_map(|e| match e {
            OplogEntry::Error { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    if durable {
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            entries
                .values()
                .any(|entry| matches!(entry, OplogEntry::Suspend { .. }))
        );
        let suspended = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&suspended, OplogIndex::INITIAL),
            (2, 1)
        );
        assert!(
            suspended.iter().all(
                |e| !matches!(&e.entry, PublicOplogEntry::End(end) if end.start_index == chunk)
            )
        );
        assert!(suspended.iter().all(
            |e| !matches!(&e.entry, PublicOplogEntry::Cancelled(end) if end.start_index == chunk)
        ));
        let mut grant = policy;
        grant.available_durable_storage_byte_seconds = u64::MAX;
        registry.set_policy(grant);
        monthly::applied_refresh(&limits, &registry, context.account_id)
            .await?
            .await?;
        executor.resume(&id, false).await?;
        let result = tokio::time::timeout(Duration::from_secs(30), invocation)
            .await???
            .into_typed::<Result<String, String>>()?;
        assert_eq!(result, Err("ErrorCode::InvalidState".to_string()));
        let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL),
            (2, 2)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    } else {
        assert!(
            !entries
                .values()
                .any(|entry| matches!(entry, OplogEntry::Suspend { .. }))
        );
        assert!(
            matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == "monthly ephemeral storage exhausted"),
            "{errors:?}"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(10), invocation)
                .await??
                .is_err()
        );
    }
    shutdown.cancel();
    Ok(())
}
