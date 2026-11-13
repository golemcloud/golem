// A kept RPC handle is called while its child's generation is cached but retired by a deletion.
// Gated before `CacheRemoved`, the deletion has already completed `DurableStateRemoved`, so the
// pinned call fails as not found. Gated before `DurableStateRemoved`, a running deletion answers
// the refusal an unpinned caller gets. A deletion that fails after stopping the child's state
// actor leaves the generation cached; the call is refused the same way and does not crash the
// caller. A call that pinned the child before the deletion stopped its state actor also fails as
// not found, without a panic.

use super::*;
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::component::ComponentId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::services::component::ComponentService;
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

// The executor is built with `panic = "abort"` (`[profile.release]` and `[profile.dev]` in the
// root `Cargo.toml`), so a panic that the unwinding test harness contains in a spawned task
// aborts a production executor. Panics that name one of the watched agents, and every state
// actor panic, are recorded.
static WATCHED_PANICS: std::sync::OnceLock<Mutex<(Vec<String>, Vec<String>)>> =
    std::sync::OnceLock::new();

// Matched as well as the agent name, so a change in how the agent is named in the panic message
// cannot make the check pass without looking.
const STATE_ACTOR_PANIC: &str = "Worker state actor";

fn watch_panics_naming(name: &str) {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    let watched = WATCHED_PANICS.get_or_init(|| Mutex::new((Vec::new(), Vec::new())));
    watched.lock().unwrap().0.push(name.to_string());
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info.to_string();
            if let Some(watched) = WATCHED_PANICS.get()
                && let Ok(mut watched) = watched.lock()
                && (message.contains(STATE_ACTOR_PANIC)
                    || watched.0.iter().any(|name| message.contains(name.as_str())))
            {
                watched.1.push(message);
            }
            previous(info);
        }));
    });
}

fn panics_naming(name: &str) -> Vec<String> {
    WATCHED_PANICS
        .get()
        .map(|watched| {
            watched
                .lock()
                .unwrap()
                .1
                .iter()
                .filter(|message| message.contains(name) || message.contains(STATE_ACTOR_PANIC))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

struct RaceGate {
    target: Mutex<Option<ComponentId>>,
    armed: AtomicBool,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

struct GatedMetadata {
    inner: Arc<dyn ComponentService>,
    gate: Arc<RaceGate>,
}

#[async_trait]
impl ComponentService for GatedMetadata {
    async fn get(
        &self,
        engine: &wasmtime::Engine,
        component_id: ComponentId,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<
        (
            wasmtime::component::Component,
            golem_service_base::model::component::Component,
        ),
        WorkerExecutorError,
    > {
        self.inner
            .get(engine, component_id, component_revision)
            .await
    }

    async fn get_metadata(
        &self,
        component_id: ComponentId,
        forced_revision: Option<golem_common::model::component::ComponentRevision>,
    ) -> Result<golem_service_base::model::component::Component, WorkerExecutorError> {
        let is_target = *self.gate.target.lock().unwrap() == Some(component_id);
        if is_target && forced_revision.is_some() && self.gate.armed.swap(false, Ordering::AcqRel) {
            self.gate.entered.add_permits(1);
            self.gate.release.acquire().await.unwrap().forget();
        }
        self.inner.get_metadata(component_id, forced_revision).await
    }

    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_environment: golem_common::model::environment::EnvironmentId,
        resolving_application: golem_common::model::application::ApplicationId,
        resolving_account: golem_common::model::account::AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        self.inner
            .resolve_component(
                component_reference,
                resolving_environment,
                resolving_application,
                resolving_account,
            )
            .await
    }

    async fn all_cached_metadata(&self) -> Vec<golem_service_base::model::component::Component> {
        self.inner.all_cached_metadata().await
    }

    async fn invalidate_current_deployed_metadata(&self) {
        self.inner.invalidate_current_deployed_metadata().await;
    }

    async fn invalidate_current_deployed_metadata_for_environment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_current_deployed_metadata_for_environment(environment_id)
            .await;
    }

    async fn invalidate_all_metadata_for_environment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_all_metadata_for_environment(environment_id)
            .await;
    }

    async fn invalidate_all(&self) {
        self.inner.invalidate_all().await;
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
    let caller: ParsedAgentKey = agent_id!("GolemHostApi", caller_name.as_str());
    let counter: ParsedAgentKey = agent_id!("Counter", child_name.as_str());
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

async fn pinned_call_racing_deletion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    agent_counters: &PrecompiledComponent,
    method: &str,
    suffix: &str,
) -> anyhow::Result<(Result<(), String>, Vec<String>)> {
    let context = TestContext::new(last_unique_id);
    let gate = Arc::new(RaceGate {
        target: Mutex::new(None),
        armed: AtomicBool::new(false),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new({
                let gate = gate.clone();
                move |inner| {
                    Arc::new(GatedMetadata {
                        inner,
                        gate: gate.clone(),
                    })
                }
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
    let caller_name = format!("race-deleting-caller-{suffix}");
    let child_name = format!("race-deleting-child-{suffix}");
    watch_panics_naming(&format!("\"{child_name}\""));
    let caller: ParsedAgentKey = agent_id!("GolemHostApi", caller_name.as_str());
    let counter: ParsedAgentKey = agent_id!("Counter", child_name.as_str());
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

    // The only forced-revision metadata lookup of the child on this path is the one in
    // `rpc_target_worker`, after the pin, so the call is held there with the child pinned.
    *gate.target.lock().unwrap() = Some(counter_component.id);
    gate.armed.store(true, Ordering::Release);
    let call = tokio::spawn({
        let executor = executor.clone();
        let caller_component = caller_component.clone();
        let caller = caller.clone();
        let method = method.to_string();
        async move {
            executor
                .invoke_and_await_agent(
                    &caller_component,
                    &caller,
                    &method,
                    data_value!("increment"),
                )
                .await
        }
        .in_current_span()
    });
    tokio::time::timeout(Duration::from_secs(20), gate.entered.acquire())
        .await
        .map_err(|_| anyhow::anyhow!("the pinned call never reached the metadata lookup"))??
        .forget();
    let hook = Arc::new(GateBeforeStage {
        target: owned_counter.clone(),
        stage: WorkerDeletionStage::CacheRemoved,
        gated: AtomicBool::new(false),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    executor.set_worker_deletion_hook(hook.clone());
    let deleting = tokio::spawn({
        let executor = executor.clone();
        let counter_worker = counter_worker.clone();
        async move { executor.delete_worker(&counter_worker).await }.in_current_span()
    });
    tokio::time::timeout(Duration::from_secs(20), hook.entered.acquire())
        .await
        .map_err(|_| anyhow::anyhow!("the deletion never reached the gated stage"))??
        .forget();

    gate.release.add_permits(1);
    let result = tokio::time::timeout(Duration::from_secs(20), call)
        .await
        .map_err(|_| anyhow::anyhow!("{method} did not finish while the deletion was gated"))?;

    hook.release.add_permits(1);
    let _ = tokio::time::timeout(Duration::from_secs(30), deleting).await;

    let result = result
        .map_err(|error| anyhow::anyhow!("the caller task failed: {error}"))??
        .into_typed::<Result<(), String>>()?;
    Ok((result, panics_naming(&format!("\"{child_name}\""))))
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn kept_handle_call_overtaken_by_the_deletion_is_not_found_without_a_panic(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    for (index, method) in KEPT_RPC_CALLS.iter().enumerate() {
        let result = pinned_call_racing_deletion(
            last_unique_id,
            deps,
            host_api_tests,
            agent_counters,
            method,
            &format!("removed-{index}"),
        )
        .await;
        match result {
            Ok((Err(error), panics))
                if error.starts_with("RpcError::NotFound(") && panics.is_empty() => {}
            Ok((result, panics)) => failures.push(format!(
                "{method}: result {result:?}, executor state-actor panics or panics naming the child: {panics:#?}"
            )),
            Err(error) => failures.push(format!("{method}: {error:?}")),
        }
    }
    assert!(
        failures.is_empty(),
        "pinned calls enqueued after the deletion removed the durable state must fail as not \
         found without a panic, which aborts an executor built with panic = \"abort\": \
         {failures:#?}"
    );
    Ok(())
}
