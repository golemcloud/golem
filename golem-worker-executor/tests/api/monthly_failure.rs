use super::*;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("2m")]
async fn monthly_stop_preserves_independent_guest_failure_on_both_sides_of_selection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for after_selection in [false, true] {
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
        let name = agent_id!(
            "FailingCounter",
            format!("monthly-failure-{after_selection}")
        );
        let id = executor.start_agent(&component.id, name.clone()).await?;
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        let worker = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
            .unwrap()
            .primary();
        let (at_outcome, release_outcome) = worker.pause_next_outcome_for_test(after_selection);
        let (_, driver_entered, release_driver) = worker.pause_next_stop_driver_for_test();
        let key = IdempotencyKey::fresh();
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
                        "add",
                        data_value!(11u64),
                    )
                    .await
            }
        });
        at_outcome.await?;
        let acquisitions = worker.permit_acquisitions_for_test();
        let generation = worker.resident_generation_for_test();
        let mut exhausted = policy;
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        driver_entered.await?;
        assert!(worker.monthly_stop_for_test().is_some());
        let mut receipt = worker.accepted_stop_receipt_for_test();
        release_driver.send(false).unwrap();
        release_outcome.send(false).unwrap();
        let error = invocation
            .await?
            .expect_err("genuine guest trap must survive quota stop");
        assert!(!error.to_string().contains("monthly"), "{error}");
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        worker.join_accepted_stops_for_test().await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        worker.retained_cleanup_for_test().await?;
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        assert_eq!(worker.resident_generation_for_test(), generation);
        refresh.await?;
        let entries = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await;
        let errors: Vec<_> = entries
            .values()
            .filter_map(|entry| match entry {
                OplogEntry::Error { error, .. } => Some(error),
                _ => None,
            })
            .collect();
        assert!(
            matches!(errors.as_slice(), [AgentError::DeterministicTrap(_)]),
            "{errors:?}"
        );
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
                .count(),
            1
        );
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
                .count(),
            0
        );
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed)
        ));
        assert_eq!(
            limits
                .initialize_account(context.account_id)
                .await?
                .monthly_observer_count_for_test(),
            0
        );
        shutdown.cancel();
    }
    Ok(())
}
