use super::*;
use pretty_assertions::assert_eq;
use test_r::test;

async fn resource_metering_configuration_controls_startup_and_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_counters: &PrecompiledComponent,
    metering: ResourceUsageMeteringConfig,
) -> anyhow::Result<()> {
    let available_policy = MonthlyResourcePolicy {
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
    let registry = Arc::new(MutableResourceLimitsRegistry::new(available_policy.clone()));
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
    let mut dimensions = Vec::new();
    if metering.compute {
        dimensions.push(IntegrationExhaustion::Compute);
    }
    if metering.memory {
        dimensions.push(IntegrationExhaustion::Memory);
    }
    if metering.filesystem {
        dimensions.push(IntegrationExhaustion::EphemeralStorage);
        dimensions.push(IntegrationExhaustion::DurableStorage);
    }

    if dimensions.is_empty() {
        let ephemeral_agent = agent_id!("EphemeralCounter", "metering-disabled-ephemeral");
        executor
            .invoke_and_await_agent(&component, &ephemeral_agent, "increment", data_value!())
            .await?;
        let durable_agent = agent_id!("Counter", "metering-disabled-durable");
        executor
            .start_agent(&component.id, durable_agent.clone())
            .await?;
        executor
            .invoke_and_await_agent(&component, &durable_agent, "increment", data_value!())
            .await?;
    }

    for exhausted in dimensions {
        assert_eq!(registry.monthly_usage_mode_revision(), 0);
        let (target_component, agent_type, must_be_resident) = match exhausted {
            IntegrationExhaustion::Compute | IntegrationExhaustion::Memory => {
                (&component, "Counter", true)
            }
            IntegrationExhaustion::DurableStorage => (&component, "Counter", false),
            IntegrationExhaustion::EphemeralStorage => (&component, "EphemeralCounter", false),
        };
        let loaded_agent = agent_id!(agent_type, format!("loaded-{exhausted:?}"));
        let loaded_worker_id = executor
            .start_agent(&target_component.id, loaded_agent.clone())
            .await?;
        if !matches!(exhausted, IntegrationExhaustion::EphemeralStorage) {
            let initial = executor
                .invoke_and_await_agent(target_component, &loaded_agent, "increment", data_value!())
                .await?;
            assert_eq!(initial.into_typed::<u32>()?, 1);
        }
        let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &loaded_worker_id);
        if must_be_resident {
            executor
                .wait_for_status(
                    &loaded_worker_id,
                    AgentStatus::Idle,
                    Duration::from_secs(10),
                )
                .await?;
            assert!(
                executor.worker_is_loaded(&owned_agent_id).await,
                "the durable target must be loaded and idle before {exhausted:?} exhaustion"
            );
        }

        let mut exhausted_policy = available_policy.clone();
        match exhausted {
            IntegrationExhaustion::Compute => {
                exhausted_policy.available_fuel = 0;
            }
            IntegrationExhaustion::Memory => {
                exhausted_policy.available_memory_gb_seconds = 0;
            }
            IntegrationExhaustion::DurableStorage => {
                exhausted_policy.available_durable_storage_byte_seconds = 0;
            }
            IntegrationExhaustion::EphemeralStorage => {
                exhausted_policy.available_ephemeral_storage_byte_seconds = 0;
            }
        }

        let delayed_policy =
            must_be_resident.then(|| registry.delay_next_policy(exhausted_policy.clone()));
        if let Some((started, _)) = &delayed_policy {
            tokio::time::timeout(Duration::from_secs(10), started.acquire())
                .await
                .expect("the exhausted policy refresh must begin")
                .unwrap()
                .forget();
        }

        if let Some((_, release)) = delayed_policy {
            release.add_permits(1);
        } else {
            registry.set_policy(exhausted_policy);
        }
        wait_for_policy_rejection(&executor, &component, exhausted).await?;
        assert_eq!(registry.monthly_usage_mode_revision(), 0);

        if must_be_resident {
            executor
                .wait_for_status(
                    &loaded_worker_id,
                    AgentStatus::Idle,
                    Duration::from_secs(10),
                )
                .await?;
            assert!(
                executor.worker_is_loaded(&owned_agent_id).await,
                "the durable target must remain loaded and idle after {exhausted:?} exhaustion is installed"
            );
        }

        let invocation_key = IdempotencyKey::fresh();
        let before_blocked_invocation = executor.oplog_max_index(&loaded_worker_id).await?;

        if matches!(exhausted, IntegrationExhaustion::EphemeralStorage) {
            let error = executor
                .invoke_and_await_agent(target_component, &loaded_agent, "increment", data_value!())
                .await
                .expect_err("the ephemeral target must reject the exhausted policy");
            assert!(
                error.to_string().contains(exhaustion_error(exhausted)),
                "metering={metering:?}, exhausted={exhausted:?}, error={error}"
            );
        } else {
            executor
                .invoke_agent_with_key(
                    target_component,
                    &loaded_agent,
                    &invocation_key,
                    "increment",
                    data_value!(),
                )
                .await?;
            executor
                .wait_for_status(
                    &loaded_worker_id,
                    AgentStatus::Suspended,
                    Duration::from_secs(10),
                )
                .await?;
            let blocked_oplog = executor
                .get_oplog(&loaded_worker_id, OplogIndex::INITIAL)
                .await?;
            assert_eq!(
                count_agent_invocation_pair_since(&blocked_oplog, before_blocked_invocation),
                (0, 0),
                "the target counter must not advance guest work while {exhausted:?} is exhausted"
            );

            registry.set_policy(available_policy.clone());
            wait_for_policy_admission(&executor, &component, exhausted).await?;
            executor.resume(&loaded_worker_id, false).await?;
            wait_for_invocation_pair(&executor, &loaded_worker_id, before_blocked_invocation)
                .await?;
            let recovered_oplog = executor
                .get_oplog(&loaded_worker_id, OplogIndex::INITIAL)
                .await?;
            assert_eq!(
                count_agent_invocation_pair_since(&recovered_oplog, before_blocked_invocation),
                (1, 1),
                "the pending {exhausted:?} invocation must execute exactly once after recovery"
            );
            let next = executor
                .invoke_and_await_agent(target_component, &loaded_agent, "increment", data_value!())
                .await?;
            assert_eq!(next.into_typed::<u32>()?, 3);
            continue;
        }

        let blocked_oplog = executor
            .get_oplog(&loaded_worker_id, OplogIndex::INITIAL)
            .await?;
        assert_eq!(
            count_agent_invocation_pair_since(&blocked_oplog, before_blocked_invocation),
            (0, 0),
            "the ephemeral target must not start guest work while storage is exhausted"
        );
        registry.set_policy(available_policy.clone());
        wait_for_policy_admission(&executor, &component, exhausted).await?;
        assert_eq!(registry.monthly_usage_mode_revision(), 0);
        let recovered = executor
            .invoke_and_await_agent(target_component, &loaded_agent, "increment", data_value!())
            .await?;
        assert_eq!(recovered.into_typed::<u32>()?, 1);
    }

    shutdown.cancel();

    Ok(())
}

macro_rules! resource_metering_configuration_test {
    ($name:ident, $compute:literal, $memory:literal, $filesystem:literal) => {
        #[test]
        #[test_r::tag(group5)]
        #[tracing::instrument]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            _tracing: &Tracing,
            #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
        ) -> anyhow::Result<()> {
            resource_metering_configuration_controls_startup_and_invocation(
                last_unique_id,
                deps,
                agent_counters,
                ResourceUsageMeteringConfig {
                    compute: $compute,
                    memory: $memory,
                    filesystem: $filesystem,
                },
            )
            .await
        }
    };
}

resource_metering_configuration_test!(resource_metering_000, false, false, false);
resource_metering_configuration_test!(resource_metering_001, false, false, true);
resource_metering_configuration_test!(resource_metering_010, false, true, false);
resource_metering_configuration_test!(resource_metering_011, false, true, true);
resource_metering_configuration_test!(resource_metering_100, true, false, false);
resource_metering_configuration_test!(resource_metering_101, true, false, true);
resource_metering_configuration_test!(resource_metering_110, true, true, false);
resource_metering_configuration_test!(resource_metering_111, true, true, true);

#[test]
#[tracing::instrument]
#[timeout("2m")]
#[test_r::tag(group5)]
async fn all_disabled_account_metering_preserves_per_agent_memory_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("large_dynamic_memory")] large_dynamic_memory: &PrecompiledComponent,
) -> anyhow::Result<()> {
    const MAX_MEMORY_BYTES: u64 = 8 * 1024 * 1024;
    const GROWTH_MIB: u64 = 30;

    let metering = ResourceUsageMeteringConfig::default();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: 0,
        available_memory_gb_seconds: 0,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: 0,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: 0,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }));
    registry.set_max_memory_per_worker(MAX_MEMORY_BYTES);
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
        .component_dep(&context.default_environment_id, large_dynamic_memory)
        .store()
        .await?;
    let agent_id = agent_id!(
        "LargeDynamicMemoryAgent",
        "unmetered-per-agent-memory-limit"
    );
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let error = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "run_with_memory_and_work",
            data_value!(GROWTH_MIB, 0u64),
        )
        .await
        .expect_err("dynamic growth must retain the per-agent memory limit");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("memory limit"),
        "unexpected per-agent memory limit error: {error}"
    );
    shutdown.cancel();

    Ok(())
}
