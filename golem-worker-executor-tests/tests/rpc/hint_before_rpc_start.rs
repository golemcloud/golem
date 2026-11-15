use super::*;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentTarget, InvocationFreshnessDisposition,
};
use golem_common::model::card::ScopeCard;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_worker_executor::services::rpc::{Rpc, RpcDemand};
use pretty_assertions::assert_eq;
use std::sync::atomic::AtomicBool;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);

/// Records the idempotency key of every `inc_by` dispatch. While armed, the first `inc_by`
/// reaches the target and then never returns, so the caller's `Start` stays without `End`
/// after the target has run.
struct DispatchThenHangRpc {
    inner: Arc<dyn Rpc>,
    hang_after_dispatch: Arc<AtomicBool>,
    dispatched: Arc<tokio::sync::Notify>,
    attempts: Arc<Mutex<Vec<Option<IdempotencyKey>>>>,
}

impl DispatchThenHangRpc {
    fn record(&self, method_name: &str, idempotency_key: &Option<IdempotencyKey>) -> bool {
        if method_name != "inc_by" {
            return false;
        }
        self.attempts.lock().unwrap().push(idempotency_key.clone());
        self.hang_after_dispatch.swap(false, Ordering::SeqCst)
    }

    async fn hang(&self) {
        self.dispatched.notify_one();
        std::future::pending::<()>().await
    }
}

#[async_trait]
impl Rpc for DispatchThenHangRpc {
    async fn cancel_scheduled_invocation(
        &self,
        target: &OwnedAgentId,
        key: golem_common::model::InvocationId,
        receipt: golem_common::model::ScheduleId,
        owner: &AgentId,
        auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        self.inner
            .cancel_scheduled_invocation(target, key, receipt, owner, auth)
            .await
    }

    async fn schedule_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: golem_common::model::AgentFingerprint,
        request_key: golem_common::model::InvocationRequestKey,
        invocation: golem_common::model::AgentInvocation,
        scheduled_at: chrono::DateTime<chrono::Utc>,
        auth_ctx: &AuthCtx,
    ) -> Result<golem_common::model::ScheduleId, RpcError> {
        self.inner
            .schedule_invocation(
                owned_agent_id,
                expected_fingerprint,
                request_key,
                invocation,
                scheduled_at,
                auth_ctx,
            )
            .await
    }

    async fn create_demand(
        &self,
        agent: AgentTarget,
        environment_id: EnvironmentId,
        method: &str,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        self.inner
            .create_demand(
                agent,
                environment_id,
                method,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
                key,
                freshness,
            )
            .await
    }

    async fn invoke_and_await(
        &self,
        agent: AgentTarget,
        environment_id: EnvironmentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<golem_common::model::AgentFingerprint>,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        let hang = self.record(&method, &key);
        let result = self
            .inner
            .invoke_and_await(
                agent,
                environment_id,
                key,
                freshness,
                expected_callee_fingerprint,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
                card,
            )
            .await;
        if hang {
            self.hang().await;
        }
        result
    }

    async fn invoke(
        &self,
        agent: AgentTarget,
        environment_id: EnvironmentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<golem_common::model::AgentFingerprint>,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
    ) -> Result<(), RpcError> {
        let hang = self.record(&method, &key);
        let result = self
            .inner
            .invoke(
                agent,
                environment_id,
                key,
                freshness,
                expected_callee_fingerprint,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
            )
            .await;
        if hang {
            self.hang().await;
        }
        result
    }
}

#[test]
#[timeout("3m")]
async fn sync_rpc_repair_after_stdout_hint_executes_target_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    repair_after_stdout_hint_executes_target_once(
        last_unique_id,
        deps,
        fixture,
        "await",
        "golem::rpc::wasm-rpc::invoke_and_await",
    )
    .await
}

#[test]
#[timeout("3m")]
async fn fire_and_forget_rpc_repair_after_stdout_hint_executes_target_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    repair_after_stdout_hint_executes_target_once(
        last_unique_id,
        deps,
        fixture,
        "invoke",
        "golem::rpc::wasm-rpc::invoke",
    )
    .await
}

/// The caller writes stdout (a hint entry) directly before an RPC `Start`. The executor stops
/// after the target ran `inc_by` but before the caller's `End`; recovery repairs the call.
async fn repair_after_stdout_hint_executes_target_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    mode: &str,
    function_name: &str,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let hang_after_dispatch = Arc::new(AtomicBool::new(true));
    let dispatched = Arc::new(tokio::sync::Notify::new());
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let hang_after_dispatch = hang_after_dispatch.clone();
            let dispatched = dispatched.clone();
            let attempts = attempts.clone();
            move |inner| {
                Arc::new(DispatchThenHangRpc {
                    inner,
                    hang_after_dispatch: hang_after_dispatch.clone(),
                    dispatched: dispatched.clone(),
                    attempts: attempts.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let target_name = format!("hint-before-{mode}-target");
    let caller_id = agent_id!("CancelTester", format!("hint-before-{mode}-caller"));
    let target_id = agent_id!("RpcCounter", target_name.clone());
    let caller = executor
        .start_agent(&component.id, caller_id.clone())
        .await?;
    let target = executor
        .start_agent(&component.id, target_id.clone())
        .await?;
    let key = IdempotencyKey::fresh();
    // A durable target is dispatched without committing the caller's `Start` first; the gate
    // commits the hint and the `Start` so that recovery finds the call incomplete.
    let mut gate = executor
        .gate_next_rpc_commit(&caller, RpcCheckpoint::Start)
        .await;
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        let key = key.clone();
        let target_name = target_name.clone();
        let mode = mode.to_string();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &caller_id,
                    &key,
                    "inc_after_stdout",
                    data_value!(target_name, mode),
                )
                .await
        })
    };
    // The first gated `Start` is the activating `get_value`. The caller stays paused in it
    // while the gate is re-armed, so the next gated `Start` is the increment.
    tokio::time::timeout(Duration::from_secs(30), gate.committed()).await?;
    let mut increment_gate = executor
        .gate_next_rpc_commit(&caller, RpcCheckpoint::Start)
        .await;
    drop(gate);
    tokio::time::timeout(Duration::from_secs(30), increment_gate.committed()).await?;
    drop(increment_gate);
    tokio::time::timeout(Duration::from_secs(30), dispatched.notified()).await?;

    let prefix = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    let start = prefix
        .iter()
        .rfind(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == function_name))
        .expect("the increment must have a committed Start")
        .oplog_index;
    let hint = start.previous();
    assert!(
        prefix
            .iter()
            .any(|entry| entry.oplog_index == hint
                && matches!(&entry.entry, PublicOplogEntry::Log(_))),
        "the stdout hint must directly precede the increment Start; caller oplog: {prefix:#?}"
    );
    assert!(
        !prefix.iter().any(
            |entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)
        ),
        "the crash window is after the target ran and before the caller's End"
    );
    let live_key = attempts.lock().unwrap()[0].clone();

    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    let recovered = tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(
            &component,
            &caller_id,
            &key,
            "inc_after_stdout",
            data_value!(target_name, mode),
        ),
    )
    .await?;
    let count = executor
        .invoke_and_await_agent(&component, &target_id, "get_value", data_value!())
        .await?
        .into_typed::<u64>()?;

    let caller_oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    let ends: Vec<_> = caller_oplog
        .iter()
        .filter(
            |entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start),
        )
        .map(|entry| entry.oplog_index)
        .collect();
    let target_oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    let increments: Vec<_> = target_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(started) => match &started.invocation {
                PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.method_name == "inc_by" =>
                {
                    Some((entry.oplog_index, method.invocation_id))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    let attempts = attempts.lock().unwrap().clone();
    let caller_key = executor
        .external_invocation_key(&caller, &key, "inc_after_stdout")
        .await?;
    let target_live_key = match &live_key {
        Some(live_key) => Some(
            executor
                .finalized_invocation_key(
                    &target,
                    &InvocationRequestKey::Internal(live_key.clone()),
                    "inc_by",
                )
                .await?,
        ),
        None => None,
    };
    eprintln!(
        "mode {mode}: hint at {hint}, Start at {start}, End at {ends:?}; attempt keys {attempts:?}; \
         keys derived from the hint index {:?} and from the index before it {:?}; \
         target inc_by AgentInvocationStarted (index, key) {increments:?}; counter {count}; \
         recovered caller result {recovered:?}",
        IdempotencyKey::derived(&IdempotencyKey::new(caller_key.to_string()), hint),
        IdempotencyKey::derived(
            &IdempotencyKey::new(caller_key.to_string()),
            hint.previous()
        ),
    );

    recovered?;
    let increments_for_attempt_key = increments
        .iter()
        .filter(|(_, key)| Some(key) == target_live_key.as_ref())
        .count();
    assert_eq!(
        (increments_for_attempt_key, increments.len(), count),
        (1, 1, 1),
        "(target inc_by AgentInvocationStarted for the attempt's key, all target inc_by \
         AgentInvocationStarted, provider-side counter); attempt keys {attempts:?}"
    );
    assert!(
        attempts.iter().all(|attempt| attempt == &live_key),
        "every attempt must carry the key of the original dispatch: {attempts:?}"
    );
    assert_eq!(ends.len(), 1, "the repaired Start must have one End");

    executor
        .invoke_and_await_agent(&component, &target_id, "inc_by", data_value!(1u64))
        .await?;
    let next = executor
        .invoke_and_await_agent(&component, &target_id, "get_value", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(next, 2, "the next fresh invocation must continue from 1");
    Ok(())
}
