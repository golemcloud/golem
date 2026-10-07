use super::*;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("60s")]
async fn teardown_first(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    loaded_compute_election(last_unique_id, deps, agent_counters, true).await
}

#[test]
#[timeout("60s")]
async fn driver_first(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    loaded_compute_election(last_unique_id, deps, agent_counters, false).await
}

async fn loaded_compute_election(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_counters: &PrecompiledComponent,
    teardown_first: bool,
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
    let metering = ResourceUsageMeteringConfig {
        compute: true,
        memory: true,
        filesystem: true,
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
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let name = agent_id!("Counter", "loaded-Compute-election");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &name, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        1
    );
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let active = executor.production_active_agent(&owned).await.unwrap();
    let worker = active.primary();
    let entry = limits.initialize_account(context.account_id).await?;
    assert!(
        worker
            .get_last_known_status()
            .await
            .pending_invocations
            .is_empty()
    );
    worker.drain_lifecycle_for_test().await?;
    assert!(worker.is_loaded().await);
    let generation = worker.resident_generation_for_test();
    let fingerprint = worker.get_initial_worker_metadata().fingerprint;
    let mut exhausted = policy.clone();
    exhausted.available_fuel = 0;
    registry.set_policy(exhausted.clone());
    tokio::time::timeout(Duration::from_secs(10), limits.run_batch_for_test()).await?;
    worker.drain_lifecycle_for_test().await?;
    assert!(worker.is_loaded().await);
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert!(!worker.concurrent_agent_permit_is_held().await);
    assert_eq!(entry.monthly_observer_count_for_test(), 0);
    assert_eq!(worker.monthly_stop_for_test(), None);
    assert_eq!(worker.pending_stop_for_test().await, None);
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.selected_owner_failure_for_test().await, None);
    let key = IdempotencyKey::fresh();
    let before = executor.oplog_max_index(&id).await?;
    let (teardown, release_teardown) = worker.pause_next_teardown_fence_for_test();
    let (_, driver, release_driver) = worker.pause_next_stop_driver_for_test();
    executor
        .invoke_agent_with_key(&component, &name, &key, "increment", data_value!())
        .await?;
    let snapshot = tokio::time::timeout(Duration::from_secs(10), teardown).await??;
    eprintln!("[owner-election] teardown_first={teardown_first} before_fence={snapshot:?}");
    assert_eq!(snapshot.generation, generation);
    assert_eq!(snapshot.final_interrupt, None);
    assert_eq!(snapshot.pending, None);
    assert_eq!(snapshot.frozen, None);
    assert_eq!(snapshot.selected_owner_failure, None);
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(entry.monthly_observer_count_for_test(), 1);
    let acquisitions = worker.permit_acquisitions_for_test();

    // A real monitor proposal targets the permit-held window paused before its owner fence.
    registry.set_policy(exhausted);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), driver).await??;
    let accepted = worker
        .monthly_stop_for_test()
        .expect("monitor accepted a stop");
    assert!(matches!(accepted, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(accepted));
    assert_eq!(worker.frozen_stop_for_test(), None);
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    eprintln!(
        "[owner-election] accepted={accepted:?} pending={:?} frozen={:?} selected={:?} generation={generation}",
        worker.pending_stop_for_test().await,
        worker.frozen_stop_for_test(),
        worker.selected_owner_failure_for_test().await
    );
    let stop_result;
    if teardown_first {
        release_teardown.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.selected_owner_failure_for_test().await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        eprintln!(
            "[owner-election] teardown_winner={:?}",
            worker.selected_owner_failure_for_test().await
        );
        release_driver.send(false).unwrap();
        stop_result = worker.join_accepted_stops_for_test().await;
    } else {
        release_driver.send(false).unwrap();
        stop_result = worker.join_accepted_stops_for_test().await;
        eprintln!(
            "[owner-election] driver_winner={:?}",
            worker.selected_owner_failure_for_test().await
        );
        release_teardown.send(()).unwrap();
    }
    eprintln!(
        "[owner-election] stop_result={stop_result:?} pending={:?} frozen={:?} selected={:?} generation={}",
        worker.pending_stop_for_test().await,
        worker.frozen_stop_for_test(),
        worker.selected_owner_failure_for_test().await,
        worker.resident_generation_for_test()
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let cleanup = worker.retained_cleanup_for_test().await;
    refresh.await?;
    let receipt_result = receipt.try_recv();
    eprintln!("[owner-election] cleanup={cleanup:?} receipt={receipt_result:?}");
    stop_result?;
    cleanup?;
    assert_eq!(worker.frozen_stop_for_test(), Some(accepted));
    assert_eq!(worker.owner_stop_for_test().await, Some(accepted));
    assert_eq!(receipt_result, Ok(()));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(entry.monthly_observer_count_for_test(), 0);
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    use golem_common::model::oplog::OplogEntry;
    assert_eq!(entries.values().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 0);
    assert!(!entries.values().any(|entry| matches!(
        entry,
        OplogEntry::Interrupted { .. } | OplogEntry::Error { .. }
    )));
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        2
    );
    registry.set_policy(policy);
    limits.run_batch_for_test().await;
    executor.resume(&id, false).await?;
    wait_for_invocation_pair(&executor, &id, before).await?;
    let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    assert_eq!(count_agent_invocation_pair_since(&oplog, before), (1, 1));
    assert_eq!(
        worker.get_initial_worker_metadata().fingerprint,
        fingerprint
    );
    assert!(worker.resident_generation_for_test() > generation);
    let recovered = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(recovered.values().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &name, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3
    );
    shutdown.cancel();
    Ok(())
}
