use super::*;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_worker_executor::services::UsesAllDeps;
use golem_worker_executor::worker::Worker;
use golem_worker_executor_test_utils::callers;
use test_r::test;

#[test]
#[timeout("2m")]
async fn deletion_cancels_initial_wait_without_acquiring_a_permit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_concurrent_agent_limit(deps, &context, 1).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_key!("Counter", "delete-wait-seed"))
        .await?;
    let seed = executor
        .production_active_agent(&seed_id)
        .await
        .unwrap()
        .primary();
    let held = executor
        .acquire_account_concurrent_agent_permit(seed_id.agent_id)
        .await;
    let name = agent_key!("Counter", "delete-initial-wait");
    let id = executor.keyed_agent_id(context.default_environment_id, &name)?;
    let worker = Worker::get_or_create_suspended_with_freshness(
        seed.all(),
        &id,
        None,
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        callers::anonymous(),
        golem_common::model::agent::InvocationFreshnessDisposition::MayExist,
        golem_worker_executor::worker::CreateIntent::Invocation {
            owner: golem_worker_executor::worker::NewOwner::keyed(component.id, name.clone()),
            idempotency_key: None,
        },
    )
    .await?;
    Worker::start_if_needed(worker.clone()).await?;
    assert!(worker.is_loading());
    executor.delete_worker(&id).await?;
    assert!(!worker.is_loading());
    assert!(!worker.is_loaded().await);
    drop(held);
    let permit = executor
        .acquire_account_concurrent_agent_permit(id.agent_id)
        .await;
    drop(permit);
    Ok(())
}
