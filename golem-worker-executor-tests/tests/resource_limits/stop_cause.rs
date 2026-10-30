use super::*;
use golem_common::model::agent::Principal;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_worker_executor::services::UsesAllDeps;
use golem_worker_executor::worker::Worker;
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
        .start_agent(&component.id, agent_id!("Counter", "delete-wait-seed"))
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    let held = executor
        .acquire_account_concurrent_agent_permit(seed_id)
        .await;
    let name = agent_id!("Counter", "delete-initial-wait");
    let id = golem_common::model::AgentId {
        component_id: component.id,
        agent_id: name.to_string(),
    };
    let worker = Worker::get_or_create_suspended(
        seed.all(),
        &OwnedAgentId::new(context.default_environment_id, &id),
        None,
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        Principal::anonymous(),
    )
    .await?;
    Worker::start_if_needed(worker.clone()).await?;
    assert!(worker.is_loading());
    executor.delete_worker(&id).await?;
    assert!(!worker.is_loading());
    assert!(!worker.is_loaded().await);
    drop(held);
    let permit = executor.acquire_account_concurrent_agent_permit(id).await;
    drop(permit);
    Ok(())
}
