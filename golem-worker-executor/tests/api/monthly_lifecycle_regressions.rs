use super::*;
use golem_common::model::Timestamp;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

fn available_policy() -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

#[test]
#[timeout("60s")]
async fn consumed_suspend_teardown_does_not_survive_reactivation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shutdown = CancellationToken::new();
    let metering = ResourceUsageMeteringConfig {
        compute: true,
        memory: false,
        filesystem: false,
    };
    let limits = ResourceLimitsGrpc::new(
        Arc::new(MutableResourceLimitsRegistry::new(available_policy())),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits,
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let name = agent_id!("Counter", "consumed-suspend");
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
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(worker.is_loaded().await);
    let generation = worker.resident_generation_for_test();
    let fingerprint = worker.get_initial_worker_metadata().fingerprint;
    let key = IdempotencyKey::fresh();
    let (outcome, release_outcome) = worker.pause_next_outcome_for_test(false);
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
                    "increment",
                    data_value!(),
                )
                .await
        }
    });
    outcome.await?;
    assert!(worker.concurrent_agent_permit_is_held().await);
    let acquisitions = worker.permit_acquisitions_for_test();
    let (published, release_driver) = worker.pause_next_stop_publication_for_test();
    let (teardown, release_teardown) = worker.pause_next_teardown_fence_for_test();
    let accepted = InterruptKind::Suspend(Timestamp::now_utc());
    worker.set_interrupting(accepted).await?;
    let mut receipt = worker.accepted_stop_receipt_for_test();
    published.await?;
    release_outcome.send(false).unwrap();
    let snapshot = teardown.await?;
    eprintln!("[lifecycle-regression] consumed stop before teardown: {snapshot:?}");
    assert_eq!(snapshot.generation, generation);
    assert_eq!(
        snapshot.pending, None,
        "the invocation loop consumed the stop"
    );
    assert_eq!(snapshot.frozen, Some(accepted));
    assert_eq!(worker.owner_stop_for_test().await, Some(accepted));
    assert!(!worker.has_unload_cleanup_for_test());
    release_teardown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !worker.has_unload_cleanup_for_test() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    eprintln!(
        "[lifecycle-regression] unload entered with publisher paused: pending={:?}",
        worker.pending_stop_for_test().await
    );
    // Permit-held unload must join the paused publisher before releasing the window.
    assert!(worker.concurrent_agent_permit_is_held().await);
    release_driver.send(false).unwrap();
    worker.join_accepted_stops_for_test().await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    worker.retained_cleanup_for_test().await?;
    assert!(!worker.concurrent_agent_permit_is_held().await);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(worker.resident_generation_for_test(), generation);
    eprintln!(
        "[lifecycle-regression] after unload and joined publisher: pending={:?}",
        worker.pending_stop_for_test().await
    );
    receipt.recv().await?;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    let before_resume = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    let suspends_before = before_resume
        .values()
        .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
        .count();
    assert_eq!(suspends_before, 1);
    assert!(!invocation.is_finished());
    executor.resume(&id, false).await?;
    let result = tokio::time::timeout(Duration::from_secs(10), invocation).await;
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    let suspends_after = entries
        .values()
        .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
        .count();
    eprintln!(
        "[lifecycle-regression] next demand: suspend markers {suspends_before} -> {suspends_after}, result={result:?}"
    );
    assert_eq!(
        suspends_after, 1,
        "one accepted stop must produce only one Suspend across reconstruction"
    );
    assert_eq!(result???.into_typed::<u32>()?, 2);
    assert_eq!(entries.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    assert_eq!(
        entries
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        3
    );
    assert!(
        !entries
            .values()
            .any(|e| matches!(e, OplogEntry::Interrupted { .. } | OplogEntry::Error { .. }))
    );
    assert_eq!(
        worker.get_initial_worker_metadata().fingerprint,
        fingerprint
    );
    assert!(worker.resident_generation_for_test() > generation);
    assert_eq!(worker.pending_stop_for_test().await, None);
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

#[test]
#[timeout("60s")]
async fn frozen_interrupt_wins_over_monthly_invocation_admission(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let policy = available_policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let metering = ResourceUsageMeteringConfig {
        compute: true,
        memory: false,
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
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let name = agent_id!("Counter", "frozen-interrupt-admission");
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
    let generation = worker.resident_generation_for_test();
    let fingerprint = worker.get_initial_worker_metadata().fingerprint;
    let (admission, release_admission) = worker.pause_next_invocation_admission_for_test();
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(&component, &name, &key, "increment", data_value!())
        .await?;
    admission.await?;
    assert!(worker.concurrent_agent_permit_is_held().await);
    let acquisitions = worker.permit_acquisitions_for_test();
    let (frozen, release_driver) = worker.pause_next_stop_freeze_for_test();
    let accepted = InterruptKind::Interrupt(Timestamp::now_utc());
    worker.set_interrupting(accepted).await?;
    let mut receipt = worker.accepted_stop_receipt_for_test();
    frozen.await?;
    assert_eq!(worker.frozen_stop_for_test(), Some(accepted));
    assert_eq!(worker.pending_stop_for_test().await, Some(accepted));
    assert_eq!(worker.owner_stop_for_test().await, None);
    let (mut selected, release_selected) = worker.pause_next_outcome_for_test(true);
    let waiting_for_publication = worker.observe_next_publication_wait_for_test();
    let (teardown, release_teardown) = worker.pause_next_teardown_fence_for_test();
    let mut exhausted = policy.clone();
    exhausted.available_fuel = 0;
    registry.set_policy(exhausted);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    let entry = limits.initialize_account(context.account_id).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !entry
            .monthly_capacity_is_exhausted_for_test(golem_common::model::agent::AgentMode::Durable)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    release_admission.send(false).unwrap();
    // Either selection wrongly overtakes the frozen stop, or it waits for publication.
    // Observing both boundaries avoids a scheduling-dependent negative assertion.
    let selected_before_publication = tokio::select! {
        result = &mut selected => { result?; true }
        result = waiting_for_publication => { result?; false }
    };
    let snapshot;
    if selected_before_publication {
        release_selected.send(false).unwrap();
        snapshot = teardown.await?;
        release_driver.send(false).unwrap();
    } else {
        release_driver.send(false).unwrap();
        selected.await?;
        release_selected.send(false).unwrap();
        snapshot = teardown.await?;
    }
    eprintln!(
        "[lifecycle-regression] admission selected_before_publication={selected_before_publication}, teardown={snapshot:?}"
    );
    worker.join_accepted_stops_for_test().await?;
    assert_eq!(worker.owner_stop_for_test().await, Some(accepted));
    release_teardown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    worker.retained_cleanup_for_test().await?;
    refresh.await?;
    receipt.recv().await?;
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
    let interrupted = entries
        .values()
        .filter(|e| matches!(e, OplogEntry::Interrupted { .. }))
        .count();
    let suspended = entries
        .values()
        .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
        .count();
    eprintln!(
        "[lifecycle-regression] accepted={accepted:?}, owner={:?}, Interrupted={interrupted}, Suspend={suspended}",
        worker.owner_stop_for_test().await
    );
    assert_eq!(
        (interrupted, suspended),
        (1, 0),
        "the frozen explicit Interrupt owns the invocation terminal"
    );
    assert!(!selected_before_publication);
    assert_eq!(entries.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 0);
    assert_eq!(
        entries
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        2
    );
    assert!(
        !entries
            .values()
            .any(|e| matches!(e, OplogEntry::Error { .. }))
    );
    registry.set_policy(policy);
    limits.run_batch_for_test().await;
    let before_resume = executor.oplog_max_index(&id).await?;
    executor.resume(&id, false).await?;
    wait_for_invocation_pair(&executor, &id, before_resume).await?;
    let recovered = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(recovered.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    assert_eq!(
        recovered
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        3
    );
    assert_eq!(
        recovered
            .values()
            .filter(|e| matches!(e, OplogEntry::Interrupted { .. }))
            .count(),
        1
    );
    assert!(
        !recovered
            .values()
            .any(|e| matches!(e, OplogEntry::Suspend { .. } | OplogEntry::Error { .. }))
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &name, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3
    );
    assert_eq!(
        worker.get_initial_worker_metadata().fingerprint,
        fingerprint
    );
    assert!(worker.resident_generation_for_test() > generation);
    shutdown.cancel();
    Ok(())
}
