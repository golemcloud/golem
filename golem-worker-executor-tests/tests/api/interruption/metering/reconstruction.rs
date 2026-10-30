use super::*;
use golem_worker_executor::worker::EvictionClass;
use pretty_assertions::assert_eq;
use test_r::test;

inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

#[test]
#[timeout("2m")]
async fn identical_live_work_has_equal_compute_charge_after_real_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut charges = Vec::new();
    let context = TestContext::new(last_unique_id);
    let metering = ResourceUsageMeteringConfig {
        compute: true,
        memory: false,
        filesystem: false,
    };
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::AllowOverage,
        available_fuel: 0,
        available_memory_gb_seconds: 0,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: 0,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: 0,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy));
    let shutdown = CancellationToken::new();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, counters)
        .store()
        .await?;
    let name = agent_id!("Counter", "reconstruction-charge");
    // Compare the identical increment work after setup. Initialization remains charged, but
    // its SDK maps and random inputs can change instruction counts across fresh incarnations.
    // Replaying that initialization is still inside the measured reconstruction interval.
    for reconstruct in [false, true] {
        let setup = registry.applied_updates().len();
        let id = executor.start_agent(&component.id, name.clone()).await?;
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        limits.run_batch_for_test().await;
        assert!(
            registry.applied_updates()[setup..]
                .iter()
                .map(|update| i128::from(update.fuel_delta))
                .sum::<i128>()
                > 0,
            "live initialization must remain charged"
        );
        let baseline = registry.applied_updates().len();
        for expected in [1u32, 2] {
            if reconstruct && expected == 2 {
                let worker = executor
                    .production_active_agent(&OwnedAgentId::new(
                        context.default_environment_id,
                        &id,
                    ))
                    .await
                    .unwrap()
                    .primary();
                tokio::time::timeout(Duration::from_secs(10), async {
                    while worker.eviction_class().await != Some(EvictionClass::LoadedIdle) {
                        tokio::task::yield_now().await;
                    }
                })
                .await?;
                assert!(worker.stop_if_evictable(EvictionClass::LoadedIdle).await);
                assert!(!worker.is_loaded().await);
            }
            let result = executor
                .invoke_and_await_agent(&component, &name, "increment", data_value!())
                .await?;
            assert_eq!(result.into_typed::<u32>()?, expected);
        }
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL),
            (3, 3)
        );
        executor.delete_worker(&id).await?;
        limits.run_batch_for_test().await;
        let updates = registry.applied_updates();
        assert!(
            updates[baseline..]
                .iter()
                .all(|update| update.monthly_policy_revision == 0)
        );
        charges.push(
            updates[baseline..]
                .iter()
                .map(|update| i128::from(update.fuel_delta))
                .sum::<i128>(),
        );
    }
    executor.shutdown_and_wait_for_invocation_loops().await?;
    shutdown.cancel();
    assert!(charges[0] > 0, "live instructions must remain billable");
    assert_eq!(
        charges[1], charges[0],
        "reconstruction must not add compute charges"
    );
    Ok(())
}
