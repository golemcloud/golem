use super::*;
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::InterruptKind;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("2m")]
async fn monthly_before_selection_and_user_stop_after_selection_keep_one_result(
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
        let name = agent_id!("Counter", format!("outcome-cutoff-{after_selection}"));
        let id = executor.start_agent(&component.id, name.clone()).await?;
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        let worker = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
            .unwrap()
            .primary();
        let (selected, release) = worker.pause_next_outcome_for_test(after_selection);
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
                        "increment",
                        data_value!(),
                    )
                    .await
            }
        });
        selected.await?;
        let mut raw = worker.raw_interrupt_for_test();
        let helper = worker.await_interrupt_for_test();
        let receipt;
        let expected;
        let mut refresh = None;
        if after_selection {
            expected = InterruptKind::Interrupt(Timestamp::now_utc());
            receipt = worker.set_interrupting(expected).await;
            tokio::time::timeout(Duration::from_secs(5), async {
                while worker.frozen_stop_for_test().is_none() {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            assert!(
                matches!(
                    raw.try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                ),
                "late stop must wait for the selected result writer"
            );
        } else {
            let mut exhausted = policy.clone();
            exhausted.available_memory_gb_seconds = 0;
            registry.set_policy(exhausted);
            refresh = Some(tokio::spawn({
                let limits = limits.clone();
                async move { limits.run_batch_for_test().await }
            }));
            expected = tokio::time::timeout(Duration::from_secs(5), helper).await?;
            assert_eq!(Some(expected), worker.monthly_stop_for_test());
            receipt = None;
        }
        release.send(false).unwrap();
        assert_eq!(raw.recv().await?, expected);
        if let Some(mut receipt) = receipt {
            receipt.recv().await?;
        }
        if !after_selection {
            executor
                .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
                .await?;
            assert!(!invocation.is_finished());
            refresh.take().unwrap().await?;
            let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            assert_eq!(
                count_agent_invocation_pair_since(&oplog, OplogIndex::INITIAL),
                (2, 1)
            );
            registry.set_policy(policy);
            limits.run_batch_for_test().await;
            executor.resume(&id, false).await?;
        }
        let result = invocation.await??;
        assert_eq!(
            result.into_return_value(),
            Some(golem_common::schema::SchemaValue::U32(1))
        );
        worker.join_accepted_stops_for_test().await?;
        let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&oplog, OplogIndex::INITIAL),
            (2, 2)
        );
        let same_result = executor
            .invoke_and_await_agent_with_key(&component, &name, &key, "increment", data_value!())
            .await?;
        assert_eq!(
            same_result.into_return_value(),
            Some(golem_common::schema::SchemaValue::U32(1))
        );
        let again = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&again, OplogIndex::INITIAL),
            (2, 2)
        );
        shutdown.cancel();
    }
    Ok(())
}
