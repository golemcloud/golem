use super::*;
use anyhow::Context as _;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::{HasOplog, HasOplogService, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

mod nonmatching;

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
async fn durable_pending_managed_xfs_storage_suspends_and_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_managed_xfs_storage(last_unique_id, deps, host_api_tests, AgentMode::Durable).await
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
async fn ephemeral_pending_managed_xfs_storage_fails_with_matching_resource_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_managed_xfs_storage(last_unique_id, deps, host_api_tests, AgentMode::Ephemeral).await
}

async fn pending_managed_xfs_storage(
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
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
            config.filesystem_storage.mode = filesystem.clone().unwrap();
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("Networking", "managed-xfs-storage-seed"),
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
        agent_id!("Networking", "managed-xfs-storage")
    } else {
        agent_id!("EphemeralNetworking", "managed-xfs-storage")
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
    let (flushed, mut flushes) = tokio::sync::mpsc::unbounded_channel();
    let usage_source: Arc<dyn ResourceUsageFlusher> = Arc::new(ObservedUsageFlusher {
        owner: Arc::downgrade(&worker),
        send: flushed,
    });
    worker
        .owner_runtime_resources()
        .register_resource_usage_flusher(Arc::downgrade(&usage_source));
    let mut capacity_updates = limits
        .subscribe_capacity_updates_for_test(context.account_id)
        .await?;
    capacity_updates.borrow_and_update();
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
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
    let initial = worker.current_monthly_proposal_for_test();
    let mut next_flush = async || {
        loop {
            let (window, _) = flushes.recv().await.context("owner usage flusher closed")?;
            if window == initial.diagnostic_window_id {
                return Ok::<_, anyhow::Error>(());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), next_flush())
        .await
        .context("initial monitor pre-wait flush")??;
    let oplog = worker.all().oplog_service();
    let read_stored = async || {
        let last = oplog.get_last_index(&owned, mode).await?;
        oplog
            .read_exact(&owned, mode, OplogIndex::INITIAL, last.as_u64())
            .await
    };
    let prefix = read_stored().await?;
    let prefix_last = *prefix
        .last_key_value()
        .context("committed invocation prefix is empty")?
        .0;
    let mut raw = worker.raw_interrupt_for_test();
    let acquisitions = worker.permit_acquisitions_for_test();
    assert_eq!(initial.exhaustion, None);
    assert!(worker.concurrent_agent_permit_is_held().await);
    let account = limits.initialize_account(context.account_id).await?;
    assert!(!account.monthly_capacity_is_exhausted_for_test(mode));
    assert!(!invocation.is_finished());
    allocate_storage(&owned).await?;
    assert!(
        !invocation.is_finished(),
        "no Store checkpoint runs while TCP is pending"
    );
    // No policy/rate update triggers this check. The unchanged outer bound includes
    // waiting for the real timer; the original proposal bound begins after its flush.
    next_flush().await?;
    assert!(!capacity_updates.has_changed()?);
    drop(capacity_updates);
    let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await
        .context("storage exhaustion proposal")?
        .unwrap();
    assert_eq!(attempt.proposal.policy_revision, initial.policy_revision);
    assert_eq!(
        attempt.proposal.diagnostic_window_id,
        initial.diagnostic_window_id
    );
    assert_eq!(
        attempt.proposal.exhaustion,
        Some(if durable {
            "monthly durable storage exhausted"
        } else {
            "monthly ephemeral storage exhausted"
        })
    );
    assert!(attempt.accepted);
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
    assert!(!worker.monthly_window_active_for_test());
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert!(worker.unload_succeeded_for_test());
    limits.run_batch_for_test().await;
    let settled = registry.applied_updates();
    // System-clock allocation time includes measured overshoot beyond the one-byte-second cap.
    assert!(
        settled
            .iter()
            .any(|u| u.monthly_policy_revision == initial.policy_revision
                && u.period == initial.period
                && if durable {
                    u.durable_storage_byte_seconds_delta > 1
                } else {
                    u.ephemeral_storage_byte_seconds_delta > 1
                }),
        "captured storage updates: {settled:?}",
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
    assert_storage_deleted(&owned)?;
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
    let stopped_raw = read_stored().await?;
    assert_eq!(
        stopped_raw.range(..=prefix_last).collect::<Vec<_>>(),
        prefix.iter().collect::<Vec<_>>()
    );
    assert!(
        !stopped_raw
            .values()
            .any(|entry| matches!(entry, OplogEntry::Jump { .. }))
    );
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
        applied_refresh(&limits, &registry, context.account_id)
            .await?
            .await?;
        executor.resume(&id, false).await?;
        let result = tokio::time::timeout(Duration::from_secs(30), invocation)
            .await???
            .into_typed::<Result<String, String>>()?;
        assert_eq!(result, Err("ErrorCode::ConnectionBroken".to_string()));
        let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let recovered_raw = read_stored().await?;
        let stopped_last = *stopped_raw.last_key_value().unwrap().0;
        assert_eq!(
            recovered_raw.range(..=stopped_last).collect::<Vec<_>>(),
            stopped_raw.iter().collect::<Vec<_>>()
        );
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

#[test]
#[tracing::instrument]
#[timeout("2m")]
#[test_r::tag(group5)]
async fn durable_storage_month_rollover_reconstructs_and_runs_pending_invocation_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: false,
        filesystem: true,
    };
    let current_policy = MonthlyResourcePolicy {
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
    let registry = Arc::new(MutableResourceLimitsRegistry::new(current_policy.clone()));
    let shutdown = CancellationToken::new();
    let resource_limits: Arc<dyn ResourceLimits> = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_millis(10),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        resource_limits,
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("Counter", "durable-storage-month-rollover");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let first = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(first.into_typed::<u32>()?, 1);

    let mut exhausted_policy = current_policy.clone();
    exhausted_policy.available_durable_storage_byte_seconds = 0;
    registry.set_policy(exhausted_policy);
    wait_for_policy_rejection(&executor, &component, IntegrationExhaustion::DurableStorage).await?;
    assert_eq!(registry.monthly_usage_mode_revision(), 0);
    let before_pending_invocation = executor.oplog_max_index(&worker_id).await?;
    let pending_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &pending_key,
            "increment",
            data_value!(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    let blocked_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&blocked_oplog, before_pending_invocation),
        (0, 0),
        "the queued increment must not execute before shutdown"
    );

    drop(executor);
    shutdown.cancel();

    let mut next_policy = current_policy;
    next_policy.period = if next_policy.period.month == 12 {
        AccountUsagePeriod {
            year: next_policy.period.year + 1,
            month: 1,
        }
    } else {
        AccountUsagePeriod {
            year: next_policy.period.year,
            month: next_policy.period.month + 1,
        }
    };
    registry.set_policy(next_policy);
    assert_eq!(registry.monthly_usage_mode_revision(), 0);

    let shutdown = CancellationToken::new();
    let resource_limits: Arc<dyn ResourceLimits> = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_millis(10),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        resource_limits,
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    assert_eq!(registry.monthly_usage_mode_revision(), 0);
    wait_for_invocation_pair(&executor, &worker_id, before_pending_invocation).await?;
    let recovered_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&recovered_oplog, before_pending_invocation),
        (1, 1),
        "reconstruction must execute the pending increment exactly once"
    );
    let next = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(next.into_typed::<u32>()?, 3);
    shutdown.cancel();

    Ok(())
}
