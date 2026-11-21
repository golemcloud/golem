use super::*;

use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[tracing::instrument]
#[timeout("2m")]
#[test_r::tag(group5)]
async fn durable_storage_month_rollover_reconstructs_and_runs_pending_invocation_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: false,
        filesystem: true,
    };
    let current_policy = MonthlyResourcePolicy {
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
    let registry = Arc::new(MutableResourceLimitsRegistry::new(current_policy.clone()));
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
    let agent_id = agent_key!("Counter", "durable-storage-month-rollover");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let first = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(first.into_typed::<u32>()?, 1);

    let mut exhausted_policy = current_policy.clone();
    exhausted_policy.available_durable_storage_byte_seconds = 0;
    registry.set_policy(exhausted_policy);
    wait_for_policy_rejection(&executor, &component, IntegrationExhaustion::DurableStorage).await?;
    assert_eq!(registry.monthly_usage_mode_revision(), 0);
    let before_pending_invocation = executor.oplog_max_index(&worker_id).await?;
    let pending_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &pending_key,
            "increment",
            data_value!(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    let blocked_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&blocked_oplog, before_pending_invocation),
        (0, 0),
        "the queued increment must not execute before shutdown"
    );

    drop(executor);
    shutdown.cancel();

    let mut next_policy = current_policy;
    next_policy.period = if next_policy.period.month == 12 {
        AccountUsagePeriod {
            year: next_policy.period.year + 1,
            month: 1,
        }
    } else {
        AccountUsagePeriod {
            year: next_policy.period.year,
            month: next_policy.period.month + 1,
        }
    };
    registry.set_policy(next_policy);
    assert_eq!(registry.monthly_usage_mode_revision(), 0);

    let shutdown = CancellationToken::new();
    let resource_limits: Arc<dyn ResourceLimits> = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_millis(10),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        resource_limits,
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    assert_eq!(registry.monthly_usage_mode_revision(), 0);
    wait_for_invocation_pair(&executor, &worker_id.agent_id, before_pending_invocation).await?;
    let recovered_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&recovered_oplog, before_pending_invocation),
        (1, 1),
        "reconstruction must execute the pending increment exactly once"
    );
    let next = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(next.into_typed::<u32>()?, 3);
    shutdown.cancel();

    Ok(())
}

#[cfg(target_os = "linux")]
mod linux;
