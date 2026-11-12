// A kept RPC handle is called while its child's generation is cached but retired by a deletion.
// Gated before `CacheRemoved`, the deletion has already completed `DurableStateRemoved`, so the
// pinned call fails as not found. Gated before `DurableStateRemoved`, a running deletion answers
// the refusal an unpinned caller gets. A deletion that fails after stopping the child's state
// actor leaves the generation cached; the call is refused the same way and does not crash the
// caller.

use super::*;
use golem_common::model::agent::ParsedAgentId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::storage::keyvalue::KeyValueStorageError;
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor::worker::{WorkerDeletionHook, WorkerDeletionStage};
use pretty_assertions::assert_eq;
use std::sync::atomic::AtomicBool;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

struct GateBeforeStage {
    target: OwnedAgentId,
    stage: WorkerDeletionStage,
    gated: AtomicBool,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[async_trait]
impl WorkerDeletionHook for GateBeforeStage {
    async fn before_stage(
        &self,
        owned_agent_id: &OwnedAgentId,
        stage: WorkerDeletionStage,
    ) -> Result<(), WorkerExecutorError> {
        if owned_agent_id == &self.target
            && stage == self.stage
            && !self.gated.swap(true, Ordering::AcqRel)
        {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        Ok(())
    }
}

async fn kept_call_during_gated_deletion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    agent_counters: &PrecompiledComponent,
    stage: WorkerDeletionStage,
    fail_removal: bool,
    suffix: &str,
) -> anyhow::Result<Vec<(String, Result<(), String>)>> {
    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_key_value_storage: Some(Arc::new({
                let faults = faults.clone();
                move |inner| Arc::new(FaultInjectingKeyValueStorage::new(inner, faults.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let counter_component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let caller_name = format!("kept-deleting-caller-{suffix}");
    let child_name = format!("kept-deleting-child-{suffix}");
    let caller: ParsedAgentId = agent_id!("GolemHostApi", caller_name.as_str());
    let counter: ParsedAgentId = agent_id!("Counter", child_name.as_str());
    executor
        .start_agent(&caller_component.id, caller.clone())
        .await?;

    let kept = executor
        .invoke_and_await_agent(
            &caller_component,
            &caller,
            "keep_agent_rpc",
            data_value!("Counter", child_name.as_str(), "increment"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(kept, Ok(()));

    let counter_worker =
        AgentId::from_agent_id(counter_component.id, &counter).map_err(anyhow::Error::msg)?;
    let owned_counter = OwnedAgentId::new(context.default_environment_id, &counter_worker);
    let hook = Arc::new(GateBeforeStage {
        target: owned_counter.clone(),
        stage,
        gated: AtomicBool::new(false),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    executor.set_worker_deletion_hook(hook.clone());
    let mut deleting = Some(tokio::spawn({
        let executor = executor.clone();
        let counter_worker = counter_worker.clone();
        async move { executor.delete_worker(&counter_worker).await }.in_current_span()
    }));
    tokio::time::timeout(Duration::from_secs(10), hook.entered.acquire())
        .await
        .map_err(|_| anyhow::anyhow!("the deletion never reached the gated stage"))??
        .forget();
    if fail_removal {
        // The deletion stops the child's state actor, then fails to remove its durable state.
        faults.fail(
            "remove",
            1,
            KeyValueStorageError::Other("injected: removal fails".to_string()),
        );
        hook.release.add_permits(1);
        let deleted =
            tokio::time::timeout(Duration::from_secs(30), deleting.take().unwrap()).await??;
        assert!(
            deleted.is_err(),
            "the injected removal failure must fail the deletion"
        );
    }
    assert!(
        executor.worker_is_cached(&owned_counter).await,
        "the deleting generation must still be cached while the deletion is gated"
    );

    let mut results = Vec::new();
    for method in KEPT_RPC_CALLS {
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            executor.invoke_and_await_agent(
                &caller_component,
                &caller,
                method,
                data_value!("increment"),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("{method} did not finish while the deletion was gated"))??
        .into_typed::<Result<(), String>>()?;
        results.push((method.to_string(), result));
    }

    if !fail_removal {
        hook.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(30), deleting.take().unwrap()).await???;
    }
    Ok(results)
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn kept_handle_to_a_child_whose_durable_state_is_removed_is_not_found(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let results = kept_call_during_gated_deletion(
        last_unique_id,
        deps,
        host_api_tests,
        agent_counters,
        WorkerDeletionStage::CacheRemoved,
        false,
        "removed",
    )
    .await?;
    for (method, result) in results {
        assert!(
            matches!(&result, Err(error) if error.starts_with("RpcError::NotFound(")),
            "{method} through a handle to a child whose durable state is removed must fail as \
             not found: {result:?}"
        );
    }
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn kept_handle_to_a_child_being_deleted_is_refused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let results = kept_call_during_gated_deletion(
        last_unique_id,
        deps,
        host_api_tests,
        agent_counters,
        WorkerDeletionStage::DurableStateRemoved,
        false,
        "running",
    )
    .await?;
    assert_refused_as_being_deleted(results);
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn kept_handle_to_a_child_whose_deletion_failed_is_refused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let results = kept_call_during_gated_deletion(
        last_unique_id,
        deps,
        host_api_tests,
        agent_counters,
        WorkerDeletionStage::DurableStateRemoved,
        true,
        "failed",
    )
    .await?;
    assert_refused_as_being_deleted(results);
    Ok(())
}

fn assert_refused_as_being_deleted(results: Vec<(String, Result<(), String>)>) {
    for (method, result) in results {
        assert!(
            matches!(
                &result,
                Err(error) if error.starts_with("RpcError::RemoteAgentError(")
                    && error.contains("being deleted")
            ),
            "{method} through a handle to a child being deleted must get the deletion refusal: \
             {result:?}"
        );
    }
}
