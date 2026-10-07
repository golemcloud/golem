use super::*;
use anyhow::Context as _;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("60s")]
async fn queued_quota_proposal_is_rejected_after_worker_window_replacement(
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
    worker.set_interrupting(InterruptKind::Restart).await?;
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
    worker.set_interrupting(user_suspend).await?;
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
    let quota_stop = worker
        .monthly_stop_for_test()
        .expect("accepted current proposal");
    assert!(matches!(quota_stop, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(quota_stop));
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
        quota_stop
    );
    assert_eq!(worker.frozen_stop_for_test(), Some(quota_stop));
    assert_eq!(worker.owner_stop_for_test().await, Some(quota_stop));
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

mod activation {
    use super::*;
    use golem_common::model::Timestamp;
    use golem_common::model::oplog::OplogEntry;
    use golem_service_base::error::worker_executor::InterruptKind;
    use golem_worker_executor::services::HasOplog;
    use pretty_assertions::assert_eq;
    use test_r::test;

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
            Arc::new(MutableResourceLimitsRegistry::new(available_quota_policy())),
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
    async fn frozen_interrupt_wins_over_quota_invocation_admission(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let policy = available_quota_policy();
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
            while !entry.monthly_capacity_is_exhausted_for_test(
                golem_common::model::agent::AgentMode::Durable,
            ) {
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
}
