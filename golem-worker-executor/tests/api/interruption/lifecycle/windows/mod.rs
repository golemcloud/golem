use super::*;
use anyhow::Context as _;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

mod retained_terminal;

#[test]
#[timeout("60s")]
async fn same_cached_worker_replaces_monthly_window_and_accepts_current_quota(
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
        Arc::new(move |config| {
            config.resource_usage_metering = metering;
            config.oplog.max_payload_size = 1;
        }),
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
    let original_raw = executor.stored_oplog(&id).await;
    let (_, _, payload) = completed_invocation_payload(&original, &key)?;
    let (old_replay, release_old_replay) =
        executor.gate_next_oplog_payload_read(&id, payload.clone());
    worker.set_interrupting(InterruptKind::Restart).await?;
    tokio::time::timeout(Duration::from_secs(10), old_replay)
        .await
        .context("old completed invocation payload acquisition")??;
    let account = limits.initialize_account(context.account_id).await?;
    let old_acquisitions = worker.permit_acquisitions_for_test();
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);

    let initial = worker.current_monthly_proposal_for_test();
    let user_suspend = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
    let mut raw = worker.raw_interrupt_for_test();
    worker.set_interrupting(user_suspend).await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        user_suspend
    );
    let mut updates = limits
        .subscribe_capacity_updates_for_test(context.account_id)
        .await?;
    drop(updates.borrow_and_update());
    assert_eq!(account.monthly_observer_count_for_test(), 2);
    let mut exhausted = policy.clone();
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted.clone());
    let old_refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), updates.changed())
        .await
        .context("old exhausted capacity applied before recorded invocation payload delivery")??;
    let old = worker.current_monthly_proposal_for_test();
    assert_eq!(old.exhaustion, Some("monthly memory exhausted"));
    assert_eq!(old.policy_revision, initial.policy_revision + 1);
    assert_eq!(old.period, initial.period);
    assert_eq!(old.fuel_generation, initial.fuel_generation);
    assert_eq!(old.fingerprint, initial.fingerprint);
    assert_eq!(old.diagnostic_window_id, initial.diagnostic_window_id);
    assert_eq!(old.start_attempt, initial.start_attempt);
    assert_eq!(old.resident_generation, initial.resident_generation);
    assert_eq!(old.diagnostic_window_id, initial.diagnostic_window_id);
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(worker.permit_acquisitions_for_test(), old_acquisitions);
    assert_eq!(worker.monthly_stop_for_test(), None);

    // A same-revision credit allows ordinary reconstruction without invalidating the proposal's revision.
    assert!(!updates.has_changed()?);
    drop(updates.borrow_and_update());
    registry.limits.lock().unwrap().monthly_policy = policy;
    let restored_refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), updates.changed())
        .await
        .context("same-revision credit applied")??;
    let credited = worker.current_monthly_proposal_for_test();
    assert_eq!(credited.exhaustion, None);
    assert_eq!(credited.policy_revision, old.policy_revision);
    assert_eq!(credited.period, old.period);
    assert_eq!(credited.fuel_generation, old.fuel_generation);
    assert_eq!(credited.fingerprint, old.fingerprint);
    assert_eq!(credited.diagnostic_window_id, old.diagnostic_window_id);
    assert_eq!(credited.start_attempt, old.start_attempt);
    assert_eq!(credited.resident_generation, old.resident_generation);
    assert_eq!(credited.diagnostic_window_id, old.diagnostic_window_id);
    assert_eq!(worker.monthly_stop_for_test(), None);
    drop(updates);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    release_old_replay.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        worker.join_accepted_stops_for_test().await?;
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
        worker.retained_cleanup_for_test().await
    })
    .await
    .context("old window physical cleanup")??;
    let old_raw = executor.stored_oplog(&id).await;
    assert!(old_raw.len() >= original_raw.len());
    assert_eq!(&old_raw[..original_raw.len()], original_raw.as_slice());
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    assert_eq!(worker.permit_acquisitions_for_test(), old_acquisitions);
    assert_eq!(worker.monthly_stop_for_test(), None);
    tokio::time::timeout(Duration::from_secs(10), old_refresh).await??;
    tokio::time::timeout(Duration::from_secs(10), restored_refresh).await??;
    worker.drain_lifecycle_for_test().await?;
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);

    let (new_replay, release_new_replay) = executor.gate_next_oplog_payload_read(&id, payload);
    let resume = tokio::spawn({
        let executor = executor.clone();
        let id = id.clone();
        async move { executor.resume(&id, false).await }
    });
    tokio::time::timeout(Duration::from_secs(10), new_replay)
        .await
        .context("replacement completed invocation payload acquisition")??;
    let current = worker.current_monthly_proposal_for_test();
    assert!(Arc::ptr_eq(
        &worker,
        &executor
            .production_active_agent(&owned)
            .await
            .unwrap()
            .primary()
    ));
    assert_eq!(current.fingerprint, old.fingerprint);
    assert_ne!(current.start_attempt, old.start_attempt);
    assert!(current.resident_generation > old.resident_generation);
    assert_ne!(current.diagnostic_window_id, old.diagnostic_window_id);
    assert_eq!(current.policy_revision, old.policy_revision);
    assert_eq!(current.period, old.period);
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert_eq!(worker.permit_acquisitions_for_test(), old_acquisitions + 1);

    let replacement = current;
    let mut updates = limits
        .subscribe_capacity_updates_for_test(context.account_id)
        .await?;
    drop(updates.borrow_and_update());
    assert_eq!(account.monthly_observer_count_for_test(), 2);
    let mut raw = worker.raw_interrupt_for_test();
    registry.limits.lock().unwrap().monthly_policy = exhausted;
    let new_refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), updates.changed())
        .await
        .context("replacement exhausted at original revision")??;
    let current = worker.current_monthly_proposal_for_test();
    assert_eq!(current.fingerprint, replacement.fingerprint);
    assert_eq!(
        current.diagnostic_window_id,
        replacement.diagnostic_window_id
    );
    assert_eq!(current.start_attempt, replacement.start_attempt);
    assert_eq!(current.resident_generation, replacement.resident_generation);
    assert_eq!(
        current.diagnostic_window_id,
        replacement.diagnostic_window_id
    );
    assert_eq!(current.policy_revision, old.policy_revision);
    assert_eq!(current.period, old.period);
    assert_eq!(current.fuel_generation, old.fuel_generation);
    assert_eq!(current.exhaustion, old.exhaustion);
    drop(updates);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    eprintln!("[stale-target] replacement otherwise eligible: {current:?}");
    let fresh = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("current actual monitor proposal")?;
    assert_eq!(fresh.proposal, current);
    assert!(fresh.accepted);
    eprintln!(
        "[stale-target] accepted current attempt: {:?}",
        fresh.proposal
    );
    let quota_stop = worker
        .monthly_stop_for_test()
        .expect("accepted current proposal");
    assert!(matches!(quota_stop, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(quota_stop));
    let mut receipt = published_interrupt_receipt(worker.as_ref()).await?;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        quota_stop
    );
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
    let stopped_raw = executor.stored_oplog(&id).await;
    assert!(stopped_raw.len() >= old_raw.len());
    assert_eq!(&stopped_raw[..old_raw.len()], old_raw.as_slice());
    assert!(
        !stopped
            .values()
            .any(|entry| matches!(entry, OplogEntry::Jump { .. }))
    );
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
        "[stale-target] real old window replaced, current target accepted, one current receipt, joined cleanup, completed history unchanged"
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
        let mut outcome = executor.gate_next_invocation_started(&owned).await?;
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
        outcome.entered().await;
        assert!(worker.concurrent_agent_permit_is_held().await);
        let acquisitions = worker.permit_acquisitions_for_test();
        assert!(matches!(
            &*worker
                .owner_runtime_resources()
                .execution_status()
                .read()
                .unwrap(),
            golem_worker_executor::model::ExecutionStatus::Suspended { .. }
        ));
        let accepted = InterruptKind::Suspend(Timestamp::now_utc());
        assert!(
            worker.set_interrupting(accepted).await?.is_none(),
            "Started persistence precedes Running and promises no public stop receipt"
        );
        worker.join_accepted_stops_for_test().await?;
        assert_eq!(worker.owner_stop_for_test().await, Some(accepted));
        outcome.release();
        assert_eq!(worker.owner_stop_for_test().await, Some(accepted));
        worker.join_accepted_stops_for_test().await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        worker.retained_cleanup_for_test().await?;
        assert!(!worker.concurrent_agent_permit_is_held().await);
        assert_eq!(worker.pending_stop_for_test().await, None);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        assert_eq!(worker.resident_generation_for_test(), generation);
        eprintln!(
            "[lifecycle-regression] after unload and joined publisher: pending={:?}",
            worker.pending_stop_for_test().await
        );
        // The exact internal no-Store acknowledgement is proved by the publication body
        // matrix. Do not subscribe to a private receipt after this completed driver.
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
    async fn explicit_interrupt_preserves_queued_invocation_behind_selected_writer_and_quota(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        use anyhow::Context as _;
        use golem_worker_executor::model::LookupResult;
        use golem_worker_executor::services::HasEvents;
        use golem_worker_executor::services::events::Event;

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
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let active = executor.production_active_agent(&owned).await.unwrap();
        let worker = active.primary();
        let mut events = worker.events().subscribe();
        let initial = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await;
        let constructor_key = initial
            .values()
            .find_map(|entry| match entry {
                OplogEntry::PendingAgentInvocation {
                    idempotency_key, ..
                } => Some(idempotency_key.clone()),
                _ => None,
            })
            .context("reserved constructor invocation")?;
        match worker.lookup_invocation_result(&constructor_key).await {
            LookupResult::Complete(result) => {
                result?;
            }
            LookupResult::Pending => {
                events
                    .wait_for(|event| match event {
                        Event::InvocationCompleted {
                            agent_id,
                            idempotency_key,
                            result,
                        } if agent_id == &id && idempotency_key == &constructor_key => {
                            Some(*result.clone())
                        }
                        _ => None,
                    })
                    .await??;
            }
            result => anyhow::bail!("constructor has no successful completion: {result:?}"),
        }
        let generation = worker.resident_generation_for_test();
        let fingerprint = worker.get_initial_worker_metadata().fingerprint;
        let (selected, release_selected) = executor.gate_next_invocation_outcome_append(&id).await;
        let completed_key = IdempotencyKey::fresh();
        let first = tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let name = name.clone();
            let completed_key = completed_key.clone();
            async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &name,
                        &completed_key,
                        "increment",
                        data_value!(),
                    )
                    .await
            }
        });
        selected.await?;
        // The selected result's terminal append keeps later work queued.
        let key = IdempotencyKey::fresh();
        executor
            .invoke_agent_with_key(&component, &name, &key, "increment", data_value!())
            .await?;
        assert!(worker.concurrent_agent_permit_is_held().await);
        let acquisitions = worker.permit_acquisitions_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let accepted = InterruptKind::Interrupt(Timestamp::now_utc());
        let mut receipt = worker
            .set_interrupting(accepted)
            .await?
            .context("accepted user stop has no receipt")?;
        assert_eq!(worker.pending_stop_for_test().await, Some(accepted));
        assert_eq!(worker.owner_stop_for_test().await, Some(accepted));
        assert_eq!(worker.await_interrupt_for_test().await, accepted);
        let entry = limits.initialize_account(context.account_id).await?;
        assert!(!entry.monthly_capacity_is_exhausted_for_test(
            golem_common::model::agent::AgentMode::Durable
        ));
        let mut capacity_updates = limits
            .subscribe_capacity_updates_for_test(context.account_id)
            .await?;
        let mut exhausted = policy.clone();
        exhausted.available_fuel = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        capacity_updates
            .wait_for(|_| {
                entry.monthly_capacity_is_exhausted_for_test(
                    golem_common::model::agent::AgentMode::Durable,
                )
            })
            .await?;
        drop(capacity_updates);
        assert!(entry.monthly_capacity_is_exhausted_for_test(
            golem_common::model::agent::AgentMode::Durable
        ));
        let blocked = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await;
        assert_eq!(blocked.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &completed_key)).count(), 1);
        assert_eq!(
            blocked
                .values()
                .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
                .count(),
            1,
            "only the constructor has finished before the selected writer append"
        );
        assert_eq!(blocked.values().filter(|e| matches!(e, OplogEntry::PendingAgentInvocation { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
        assert_eq!(blocked.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 0);
        assert!(!blocked.values().any(|e| matches!(
            e,
            OplogEntry::Interrupted { .. } | OplogEntry::Suspend { .. } | OplogEntry::Error { .. }
        )));
        assert!(worker.concurrent_agent_permit_is_held().await);
        release_selected.send(false).unwrap();
        assert_eq!(first.await??.into_typed::<u32>()?, 1);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
            accepted
        );
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        worker.join_accepted_stops_for_test().await?;
        assert_eq!(worker.owner_stop_for_test().await, Some(accepted));
        tokio::time::timeout(
            Duration::from_secs(10),
            worker.await_ready_to_process_commands(),
        )
        .await??;
        worker.retained_cleanup_for_test().await?;
        assert!(!worker.is_loaded().await);
        assert!(!worker.concurrent_agent_permit_is_held().await);
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
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed)
        ));
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
            "the explicit Interrupt owns the invocation terminal"
        );
        assert_eq!(entries.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 0);
        assert_eq!(entries.values().filter(|e| matches!(e, OplogEntry::PendingAgentInvocation { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
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
        executor.resume(&id, false).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "increment",
                    data_value!()
                )
                .await?
                .into_typed::<u32>()?,
            2
        );
        let recovered = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await;
        assert_eq!(recovered.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
        assert_eq!(recovered.values().filter(|e| matches!(e, OplogEntry::PendingAgentInvocation { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
        assert_eq!(recovered.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &completed_key)).count(), 1);
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
