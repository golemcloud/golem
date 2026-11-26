// A kept RPC handle is called while its child's generation is cached but retired by a deletion.
// Gated before `CacheRemoved`, the deletion has already completed `DurableStateRemoved`, so the
// pinned call fails as not found. Gated before `DurableStateRemoved`, a running deletion answers
// the refusal an unpinned caller gets. A deletion that fails after stopping the child's state
// actor leaves the generation cached; the call is refused the same way and does not crash the
// caller. A call that pinned the child before the deletion stopped its state actor also fails as
// not found, without a panic.

use super::*;
use crate::panic_watch::{panics_naming, watch_panics_naming};
use golem_common::model::agent::{
    AgentConfigEntryDto, InvocationFreshnessDisposition, ParsedAgentKey,
};
use golem_common::model::component::ComponentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{AgentInvocation, ScheduleId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::rpc::{Rpc, RpcDemand};
use golem_worker_executor::storage::keyvalue::KeyValueStorageError;
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor::worker::target::ResolvedTarget;
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

struct RaceGate {
    target: Mutex<Option<ComponentId>>,
    target_agent: Mutex<Option<OwnedAgentId>>,
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

/// Holds the first schedule of the gate's target agent where the caller's schedule enters the
/// executor's RPC, before the executor acquires the target and reads its status.
struct GatedSchedule {
    inner: Arc<dyn Rpc>,
    gate: Arc<RaceGate>,
}

#[async_trait]
impl Rpc for GatedSchedule {
    async fn create_and_initialize(
        &self,
        target: ResolvedTarget,
        exclusive: bool,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        idempotency_key: IdempotencyKey,
    ) -> Result<Box<dyn RpcDemand>, golem_service_base::error::worker_executor::WorkerExecutorError>
    {
        self.inner
            .create_and_initialize(
                target,
                exclusive,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                caller,
                config,
                auth_ctx,
                idempotency_key,
            )
            .await
    }

    async fn cancel_scheduled_invocation(
        &self,
        target: &OwnedAgentId,
        key: golem_common::model::InvocationId,
        receipt: ScheduleId,
        canceller: &golem_common::model::caller::Principal,
        auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        self.inner
            .cancel_scheduled_invocation(target, key, receipt, canceller, auth)
            .await
    }

    async fn schedule_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: golem_common::model::AgentFingerprint,
        request_key: golem_common::model::InvocationRequestKey,
        invocation: AgentInvocation,
        scheduled_at: jiff::Timestamp,
        self_agent_id: &golem_common::model::AgentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ScheduleId, RpcError> {
        let is_target = self.gate.target_agent.lock().unwrap().as_ref() == Some(owned_agent_id);
        if is_target && self.gate.armed.swap(false, Ordering::AcqRel) {
            self.gate.entered.add_permits(1);
            self.gate.release.acquire().await.unwrap().forget();
        }
        self.inner
            .schedule_invocation(
                owned_agent_id,
                expected_fingerprint,
                request_key,
                invocation,
                scheduled_at,
                self_agent_id,
                auth_ctx,
            )
            .await
    }

    async fn create_demand(
        &self,
        target: ResolvedTarget,
        method: &str,
        invocation: Option<golem_api_grpc::proto::golem::worker::InvocationStart>,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        callee_caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        self.inner
            .create_demand(
                target,
                method,
                invocation,
                created_by,
                caller,
                env,
                stack,
                callee_caller,
                config,
                auth,
                key,
                freshness,
            )
            .await
    }

    async fn invoke_and_await(
        &self,
        target: ResolvedTarget,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<golem_common::model::AgentFingerprint>,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        callee_caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        card: Option<golem_common::model::card::ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        self.inner
            .invoke_and_await(
                target,
                key,
                freshness,
                expected_callee_fingerprint,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                callee_caller,
                config,
                auth,
                card,
            )
            .await
    }

    async fn invoke(
        &self,
        target: ResolvedTarget,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<golem_common::model::AgentFingerprint>,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        callee_caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
    ) -> Result<(), RpcError> {
        self.inner
            .invoke(
                target,
                key,
                freshness,
                expected_callee_fingerprint,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                callee_caller,
                config,
                auth,
            )
            .await
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
) -> anyhow::Result<GatedDeletionAnswers> {
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
    watch_panics_naming(&format!("\"{child_name}\""));
    let caller: ParsedAgentKey = agent_key!("GolemHostApi", caller_name.as_str());
    let counter: ParsedAgentKey = agent_key!("Counter", child_name.as_str());
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

    let counter_worker = crate::agent::keyed_agent_id(counter_component.environment_id, &counter);
    let owned_counter = counter_worker.clone();
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
    // The pinned schedule path reads the child's status after it resolves the cached
    // generation, whose state actor the deletion may have stopped.
    let schedule = match tokio::time::timeout(
        Duration::from_secs(20),
        executor.invoke_and_await_agent(
            &caller_component,
            &caller,
            "kept_agent_rpc_schedule_result",
            data_value!("increment"),
        ),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "kept_agent_rpc_schedule_result did not finish while the deletion was gated"
        )
    })? {
        Err(error) => Err(format!("{error:#}")),
        Ok(result) => Ok(result.into_typed::<Result<(), String>>()?),
    };

    if !fail_removal {
        hook.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(30), deleting.take().unwrap()).await???;
    }
    Ok(GatedDeletionAnswers {
        calls: results,
        schedule,
        panics: panics_naming(&format!("\"{child_name}\"")),
    })
}

struct GatedDeletionAnswers {
    calls: Vec<(String, Result<(), String>)>,
    // The guest's answer to the schedule through the kept handle, or the rendered failure of the
    // caller's invocation.
    schedule: Result<Result<(), String>, String>,
    // Panics naming the child, and every state actor panic.
    panics: Vec<String>,
}

// A failed schedule through the kept handle traps the caller, so the caller's invocation fails
// with `refusal`. Resolving the cached generation does not panic on its stopped state actor,
// which aborts an executor built with panic = "abort".
fn assert_schedule_refused_without_a_panic(answers: &GatedDeletionAnswers, refusal: &str) {
    assert!(
        answers.panics.is_empty(),
        "the kept schedule must not panic on the stopped state actor: {:#?}",
        answers.panics
    );
    assert!(
        matches!(&answers.schedule, Err(error) if error.contains(refusal)),
        "the kept schedule must fail the caller's invocation with {refusal:?}: {:?}",
        answers.schedule
    );
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
    assert_schedule_refused_without_a_panic(&results, "Not found: Worker");
    for (method, result) in results.calls {
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
    assert_schedule_refused_without_a_panic(
        &results,
        "Remote agent error: Invalid input: Worker is being deleted",
    );
    assert_refused_as_being_deleted(results.calls);
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
    assert_schedule_refused_without_a_panic(
        &results,
        "Remote agent error: Invalid input: Worker is being deleted",
    );
    assert_refused_as_being_deleted(results.calls);
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
) -> anyhow::Result<PinnedRaceAnswers> {
    let context = TestContext::new(last_unique_id);
    let gate = Arc::new(RaceGate {
        target: Mutex::new(None),
        target_agent: Mutex::new(None),
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
            wrap_rpc: Some(Arc::new({
                let gate = gate.clone();
                move |inner| {
                    Arc::new(GatedSchedule {
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
    let caller: ParsedAgentKey = agent_key!("GolemHostApi", caller_name.as_str());
    let counter: ParsedAgentKey = agent_key!("Counter", child_name.as_str());
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

    let counter_worker = crate::agent::keyed_agent_id(counter_component.environment_id, &counter);
    let owned_counter = counter_worker.clone();

    // A call is held at the forced-revision metadata lookup of the child that follows the pin in
    // `rpc_target_worker`. A schedule makes no component lookup and is validated when it runs, so
    // it is held where it enters the executor's RPC. After the release it acquires the retired
    // generation that the cache still holds while the deletion waits before `CacheRemoved`, and
    // its status read reaches the stopped state actor.
    if method.contains("schedule") {
        *gate.target_agent.lock().unwrap() = Some(owned_counter.clone());
    } else {
        *gate.target.lock().unwrap() = Some(counter_component.id);
    }
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
        .map_err(|_| anyhow::anyhow!("the pinned call never reached its gate"))??
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

    let result = match result.map_err(|error| anyhow::anyhow!("the caller task failed: {error}"))? {
        Ok(result) => Ok(result.into_typed::<Result<(), String>>()?),
        Err(error) => Err(format!("{error:#}")),
    };
    let caller_worker = crate::agent::keyed_agent_id(caller_component.environment_id, &caller);
    let caller_errors = executor
        .get_oplog(
            &caller_worker,
            golem_common::model::oplog::OplogIndex::INITIAL,
        )
        .await?
        .into_iter()
        .filter_map(|entry| match entry.entry {
            golem_common::model::oplog::PublicOplogEntry::Error(error) => Some(error.error),
            _ => None,
        })
        .collect();
    Ok(PinnedRaceAnswers {
        result,
        caller_errors,
        panics: panics_naming(&format!("\"{child_name}\"")),
    })
}

struct PinnedRaceAnswers {
    // The guest's answer, or the rendered failure of the caller's invocation.
    result: Result<Result<(), String>, String>,
    // The errors of the caller's failed attempts, from its oplog.
    caller_errors: Vec<String>,
    // Panics naming the child, and every state actor panic.
    panics: Vec<String>,
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
            Ok(PinnedRaceAnswers {
                result: Ok(Err(error)),
                panics,
                ..
            }) if error.starts_with("RpcError::NotFound(") && panics.is_empty() => {}
            Ok(PinnedRaceAnswers { result, panics, .. }) => failures.push(format!(
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

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn kept_handle_schedule_overtaken_by_the_deletion_is_not_found_without_a_panic(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    // The deletion removes the child's durable state and stops its state actor while the schedule
    // is held before it acquires the child, which the cache still holds. The schedule then reads
    // the status of the stopped state actor. A failed schedule traps the caller, so every attempt the caller makes must fail
    // as not found, as the pinned calls do above, and none with an internal error from the
    // stopped state actor.
    let answers = pinned_call_racing_deletion(
        last_unique_id,
        deps,
        host_api_tests,
        agent_counters,
        "kept_agent_rpc_schedule_result",
        "schedule",
    )
    .await?;
    assert!(answers.panics.is_empty(), "{:#?}", answers.panics);
    assert!(
        matches!(&answers.result, Err(error) if error.contains("Not found: Worker")),
        "the caller's invocation must fail as not found: {:?}",
        answers.result
    );
    assert!(
        !answers.caller_errors.is_empty()
            && answers
                .caller_errors
                .iter()
                .all(|error| error.contains("Not found: Worker")),
        "every attempt of the pinned schedule overtaken by the deletion must fail as not found: \
         {:#?}",
        answers.caller_errors
    );
    Ok(())
}
