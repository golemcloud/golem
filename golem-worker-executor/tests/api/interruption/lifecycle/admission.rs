use super::*;

mod initialization {
    use super::*;
    use anyhow::Context as _;
    use golem_common::model::oplog::{AgentError, OplogEntry, OplogErrorKind};
    use golem_service_base::error::worker_executor::InterruptKind;
    use golem_worker_executor::services::{HasOplog, UsesAllDeps};
    use golem_worker_executor::worker::Worker;
    use pretty_assertions::assert_eq;
    use test_r::test;

    macro_rules! preparation_test {
        ($name:ident, $durable:expr) => {
            #[test]
            #[timeout("2m")]
            async fn $name(
                last_unique_id: &LastUniqueId,
                deps: &WorkerExecutorTestDependencies,
                #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
                _tracing: &Tracing,
            ) -> anyhow::Result<()> {
                quota_stop_during_core_initialization(
                    last_unique_id,
                    deps,
                    agent_counters,
                    $durable,
                )
                .await
            }
        };
    }

    preparation_test!(durable_quota_stop_during_real_core_initialization, true);
    preparation_test!(ephemeral_quota_stop_during_real_core_initialization, false);

    async fn quota_stop_during_core_initialization(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        agent_counters: &PrecompiledComponent,
        durable: bool,
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
        let seed_id = executor
            .start_agent(&component.id, agent_id!("Counter", "preparation-seed"))
            .await?;
        let seed = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
            .await
            .unwrap()
            .primary();
        wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while seed.eviction_class().await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("seed idle")?;
        let name = if durable {
            agent_id!("Counter", "monthly-preparation")
        } else {
            agent_id!("EphemeralCounter", "monthly-preparation")
        };
        let physical_name = if durable {
            name.clone()
        } else {
            name.clone()
                .with_ephemeral_invocation_phantom(&IdempotencyKey::fresh())
                .unwrap()
        };
        let id = AgentId::from_agent_id(component.id, &physical_name).unwrap();
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
        let mut initializer = worker
            .owner_execution()
            .test_gate_next_monotonic_clock_start();
        Worker::start_if_needed(worker.clone()).await?;
        tokio::time::timeout(Duration::from_secs(10), initializer.entered())
            .await
            .context("core initializer gate")?;
        let before = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert!(before.iter().any(|e| matches!(&e.entry, PublicOplogEntry::Start(start) if start.function_name == "monotonic_clock::now")));
        assert!(
            !before
                .iter()
                .any(|e| matches!(e.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        );
        let generation = worker.resident_generation_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let mut attempts =
            crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
        let mut raw = worker.raw_interrupt_for_test();
        let helper = worker.await_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
            .await?
            .context("monthly proposal during core initialization")?;
        assert_eq!(attempt.proposal.resident_generation, generation);
        assert!(attempt.accepted);
        let quota_stop = worker
            .monthly_stop_for_test()
            .expect("monthly accepted during core initializer");
        assert!(matches!(quota_stop, InterruptKind::Suspend(_)));
        assert!(worker.concurrent_agent_permit_is_held().await);
        let mut receipt = published_interrupt_receipt(worker.as_ref()).await?;
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), raw.recv())
                .await
                .context("preparation raw interrupt")??,
            quota_stop
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), helper).await?,
            quota_stop
        );
        initializer.release();
        tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await
        .context("preparation stop joins")??;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("preparation physical unload")?;
        worker.retained_cleanup_for_test().await?;
        refresh.await?;
        assert_eq!(worker.resident_generation_for_test(), generation);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        assert_eq!(
            limits
                .initialize_account(context.account_id)
                .await?
                .monthly_observer_count_for_test(),
            0
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
                OplogEntry::Error { error, kind, .. } => Some((error, kind)),
                _ => None,
            })
            .collect();
        let suspends = entries
            .values()
            .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
            .count();
        let received = receipt.try_recv();
        assert_eq!(
            received,
            Ok(()),
            "accepted preparation stop must acknowledge after physical unload"
        );
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed)
        ));
        assert!(
            !entries.values().any(|e| matches!(
                e,
                OplogEntry::AgentInvocationStarted { .. }
                    | OplogEntry::AgentInvocationFinished { .. }
            )),
            "core initialization cannot fabricate a guest invocation"
        );
        if durable {
            assert!(
                errors.is_empty(),
                "monthly preparation wrote recovery failure: {errors:?}"
            );
            assert_eq!(suspends, 1);
            registry.set_policy(policy);
            limits.run_batch_for_test().await;
            executor.resume(&id, false).await?;
            assert_eq!(
                executor
                    .invoke_and_await_agent(&component, &name, "increment", data_value!())
                    .await?
                    .into_typed::<u32>()?,
                1
            );
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            assert_eq!(
                recovered
                    .iter()
                    .filter(|e| matches!(e.entry, PublicOplogEntry::AgentInvocationStarted(_)))
                    .count(),
                2
            );
            assert_eq!(
                recovered
                    .iter()
                    .filter(|e| matches!(e.entry, PublicOplogEntry::AgentInvocationFinished(_)))
                    .count(),
                2
            );
        } else {
            assert!(
                matches!(
                    errors.as_slice(),
                    [(
                        AgentError::EphemeralCannotSuspend(_),
                        OplogErrorKind::Recovery
                    )]
                ),
                "{errors:?}"
            );
            assert_eq!(suspends, 0);
        }
        shutdown.cancel();
        Ok(())
    }
}

mod loaded_compute_admission {
    use super::*;
    use anyhow::Context as _;
    use golem_worker_executor::services::HasOplog;
    use pretty_assertions::assert_eq;
    use test_r::test;

    #[test]
    #[timeout("60s")]
    async fn exhausted_compute_after_idle_preserves_queued_invocation_and_recovers(
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
        let name = agent_id!("Counter", "loaded-compute-admission");
        let id = executor.start_agent(&component.id, name.clone()).await?;
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let active = executor.production_active_agent(&owned).await.unwrap();
        let worker = active.primary();
        let setup_key = IdempotencyKey::fresh();
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &setup_key,
                    "increment",
                    data_value!()
                )
                .await?
                .into_typed::<u32>()?,
            1
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(!worker.monthly_window_active_for_test());
        assert!(!worker.concurrent_agent_permit_is_held().await);
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
        assert_eq!(worker.selected_owner_failure_for_test().await, None);
        let key = IdempotencyKey::fresh();
        let before = executor.oplog_max_index(&id).await?;
        let acquisitions = worker.permit_acquisitions_for_test();
        executor
            .invoke_agent_with_key(&component, &name, &key, "increment", data_value!())
            .await?;
        executor
            .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
            .await?;
        worker.join_accepted_stops_for_test().await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("compute admission must physically stop without entering the queued invocation")?;
        worker.retained_cleanup_for_test().await?;
        assert_eq!(worker.resident_generation_for_test(), generation);
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions + 1);
        assert_eq!(entry.monthly_observer_count_for_test(), 0);
        let stopped_prefix = executor.stored_oplog(&id).await;
        let entries = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await;
        use golem_common::model::oplog::OplogEntry;
        let final_suspends: Vec<_> = entries
            .iter()
            .filter_map(|(index, entry)| match entry {
                OplogEntry::Suspend { timestamp } => Some((*index, *timestamp)),
                _ => None,
            })
            .collect();
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
            1,
            "one admission Suspend must remain authoritative: {final_suspends:?}"
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
        let recovered_raw = executor.stored_oplog(&id).await;
        assert!(recovered_raw.len() >= stopped_prefix.len());
        assert_eq!(
            &recovered_raw[..stopped_prefix.len()],
            stopped_prefix.as_slice(),
            "ordinary reconstruction must retain the complete committed admission prefix"
        );
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
}
