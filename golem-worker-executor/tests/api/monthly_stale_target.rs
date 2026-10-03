use super::*;
use anyhow::Context as _;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("60s")]
async fn queued_monthly_proposal_is_rejected_after_worker_window_replacement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
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
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let name = agent_id!("Counter", "monthly-stale-target");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let key = IdempotencyKey::fresh();
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &name, &key, "increment", data_value!())
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
    .await
    .context("initial idle permit release")?;
    let original_tip = worker.oplog().current_oplog_index().await;
    let original = worker
        .oplog()
        .read_exact(OplogIndex::INITIAL, original_tip.as_u64())
        .await;
    let (old_replay, release_old_replay) = worker.pause_completed_replay_for_test(key.clone());
    worker.set_interrupting(InterruptKind::Restart).await;
    tokio::time::timeout(Duration::from_secs(10), old_replay)
        .await
        .context("old completed replay")??;
    let account = limits.initialize_account(context.account_id).await?;
    let old_acquisitions = worker.permit_acquisitions_for_test();
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);

    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let (queued, release_old_proposal) = worker.pause_next_monthly_acceptance_for_test();
    let mut exhausted = policy.clone();
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted.clone());
    let old_refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), queued)
        .await
        .context("old monitor proposal enqueued")??;
    let mut old = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("old proposal observation")?;
    assert_eq!(old.proposal.exhaustion, Some("monthly memory exhausted"));
    assert_eq!(old.proposal, worker.current_monthly_proposal_for_test());
    assert_eq!(worker.monthly_stop_for_test(), None);
    eprintln!(
        "[stale-target] queued old actual monitor proposal: {:?}",
        old.proposal
    );

    // A same-revision credit allows ordinary reconstruction without invalidating the proposal's revision.
    registry.limits.lock().unwrap().monthly_policy = policy;
    let restored_refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker
            .current_monthly_proposal_for_test()
            .exhaustion
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("same-revision credit applied")?;
    let user_suspend = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
    let mut raw = worker.raw_interrupt_for_test();
    worker.set_interrupting(user_suspend).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        user_suspend
    );
    release_old_replay.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        worker.join_accepted_stops_for_test().await?;
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
        worker.retained_cleanup_for_test().await
    })
    .await
    .context("old unload while actor proposal remains held")??;
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    assert_eq!(worker.permit_acquisitions_for_test(), old_acquisitions);
    assert_eq!(worker.monthly_stop_for_test(), None);
    tokio::time::timeout(Duration::from_secs(10), old_refresh).await??;
    tokio::time::timeout(Duration::from_secs(10), restored_refresh).await??;

    let (new_replay, release_new_replay) = worker.pause_completed_replay_for_test(key.clone());
    let resume = tokio::spawn({
        let executor = executor.clone();
        let id = id.clone();
        async move { executor.resume(&id, false).await }
    });
    tokio::time::timeout(Duration::from_secs(10), new_replay)
        .await
        .context("replacement replay while old actor proposal remains held")??;
    let current = worker.current_monthly_proposal_for_test();
    assert!(Arc::ptr_eq(
        &worker,
        &executor
            .production_active_agent(&owned)
            .await
            .unwrap()
            .primary()
    ));
    assert!(matches!(
        old.completed.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(current.fingerprint, old.proposal.fingerprint);
    assert_ne!(current.start_attempt, old.proposal.start_attempt);
    assert!(current.resident_generation > old.proposal.resident_generation);
    assert_ne!(current.window_identity, old.proposal.window_identity);
    assert_eq!(current.policy_revision, old.proposal.policy_revision);
    assert_eq!(current.period, old.proposal.period);
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert_eq!(worker.permit_acquisitions_for_test(), old_acquisitions + 1);

    let (_, accepted, release_driver) = worker.pause_next_stop_driver_for_test();
    registry.limits.lock().unwrap().monthly_policy = exhausted;
    let new_refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker
            .current_monthly_proposal_for_test()
            .exhaustion
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("replacement exhausted at original revision")?;
    let current = worker.current_monthly_proposal_for_test();
    assert_eq!(current.policy_revision, old.proposal.policy_revision);
    assert_eq!(current.period, old.proposal.period);
    assert_eq!(current.fuel_generation, old.proposal.fuel_generation);
    assert_eq!(current.exhaustion, old.proposal.exhaustion);
    eprintln!("[stale-target] replacement otherwise eligible: {current:?}");
    release_old_proposal.send(()).unwrap();
    assert!(
        !tokio::time::timeout(Duration::from_secs(10), old.completed)
            .await
            .context("old target rejection acknowledgment")??
    );
    eprintln!("[stale-target] rejected old attempt: {:?}", old.proposal);
    let fresh = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("current actual monitor proposal")?;
    assert_eq!(fresh.proposal, current);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), fresh.completed)
            .await
            .context("current target acceptance acknowledgment")??
    );
    eprintln!(
        "[stale-target] accepted current attempt: {:?}",
        fresh.proposal
    );
    tokio::time::timeout(Duration::from_secs(10), accepted).await??;
    let monthly = worker
        .monthly_stop_for_test()
        .expect("accepted current proposal");
    assert!(matches!(monthly, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(monthly));
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.owner_stop_for_test().await, None);
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let mut raw = worker.raw_interrupt_for_test();
    release_driver.send(false).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        monthly
    );
    assert_eq!(worker.frozen_stop_for_test(), Some(monthly));
    assert_eq!(worker.owner_stop_for_test().await, Some(monthly));
    release_new_replay.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        worker.join_accepted_stops_for_test().await?;
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
        worker.retained_cleanup_for_test().await
    })
    .await
    .context("current target physical cleanup")??;
    tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    tokio::time::timeout(Duration::from_secs(10), new_refresh).await??;
    tokio::time::timeout(Duration::from_secs(10), resume).await???;
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    assert_eq!(
        worker.resident_generation_for_test(),
        current.resident_generation
    );
    assert_eq!(worker.permit_acquisitions_for_test(), old_acquisitions + 1);
    assert_eq!(
        worker
            .oplog()
            .read_exact(OplogIndex::INITIAL, original_tip.as_u64())
            .await,
        original
    );
    let stopped = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        2
    );
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
            .count(),
        2
    );
    assert!(
        !stopped
            .values()
            .any(|e| matches!(e, OplogEntry::Error { .. } | OplogEntry::Interrupted { .. }))
    );
    eprintln!(
        "[stale-target] old target rejected, current target accepted, one current receipt, joined cleanup, completed history unchanged"
    );
    shutdown.cancel();
    Ok(())
}
