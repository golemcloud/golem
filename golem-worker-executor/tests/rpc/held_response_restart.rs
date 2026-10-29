use super::*;
use golem_common::model::agent::{AgentConfigEntryDto, InvocationFreshnessDisposition};
use golem_common::model::card::ScopeCard;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_worker_executor::services::rpc::{Rpc, RpcDemand};
use pretty_assertions::assert_eq;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);

/// Records the idempotency key of every `inc_by` dispatch. The dispatch at position `held` reaches
/// the target, which runs its effect and returns, but the caller never receives the response: the
/// call stays pending until the executor is dropped.
struct HoldResponseRpc {
    inner: Arc<dyn Rpc>,
    held: usize,
    attempts: Arc<Mutex<Vec<Option<IdempotencyKey>>>>,
    effect_applied: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Rpc for HoldResponseRpc {
    async fn create_demand(
        &self,
        agent: &OwnedAgentId,
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
                agent, method, created_by, caller, env, stack, config, auth, key, freshness,
            )
            .await
    }

    async fn invoke_and_await(
        &self,
        agent: &OwnedAgentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
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
        let hold = method == "inc_by" && {
            let mut attempts = self.attempts.lock().unwrap();
            attempts.push(key.clone());
            attempts.len() == self.held + 1
        };
        let result = self
            .inner
            .invoke_and_await(
                agent, key, freshness, method, input, created_by, caller, env, stack, config, auth,
                card,
            )
            .await;
        if hold {
            self.effect_applied.notify_one();
            std::future::pending::<()>().await;
        }
        result
    }

    async fn invoke(
        &self,
        agent: &OwnedAgentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
    ) -> Result<(), RpcError> {
        self.inner
            .invoke(
                agent, key, freshness, method, input, created_by, caller, env, stack, config, auth,
            )
            .await
    }
}

/// `grow_memory_before_rpc_activation` grows memory between creating the durable RPC target and
/// calling it. The target activation is a durable `WriteRemote` call, so its committed `End`, not
/// the `GrowMemory` hint, is the oplog tip when the invocation `Start` is appended and committed.
#[test]
#[timeout("3m")]
async fn durable_rpc_after_activation_redispatches_with_the_same_key_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    held_response_restart_case(
        last_unique_id,
        deps,
        fixture,
        agent_id!("CancelTester", "held-response-caller"),
        "grow_memory_before_rpc_activation",
        data_value!("held-response-target"),
        0,
        1,
    )
    .await
}

/// Control: the same durable RPC with no hint between target creation and the call.
#[test]
#[timeout("3m")]
async fn durable_rpc_without_hint_redispatches_with_the_same_key_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    held_response_restart_case(
        last_unique_id,
        deps,
        fixture,
        agent_id!("RpcAuthTester", "held-response-caller"),
        "try_call_counter",
        data_value!("held-response-target"),
        0,
        1,
    )
    .await
}

/// `durable_operation_after_rpc_result("async-rpc")` calls `inc_by(1)`, grows memory, then calls
/// `inc_by(7)` on the same activated target. The uncommitted `GrowMemory` hint is the oplog tip when
/// the second RPC begins; its Start barrier must preserve both the hint and the call before dispatch.
#[test]
#[timeout("3m")]
async fn durable_rpc_after_uncommitted_hint_redispatches_with_the_same_key_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    held_response_restart_case(
        last_unique_id,
        deps,
        fixture,
        agent_id!("CancelTester", "held-response-caller"),
        "durable_operation_after_rpc_result",
        data_value!("held-response-target", "async-rpc"),
        1,
        8,
    )
    .await
}

async fn held_response_restart_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    caller_id: ParsedAgentId,
    method: &str,
    input: TypedSchemaValue,
    held: usize,
    expected_count: u64,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let effect_applied = Arc::new(tokio::sync::Notify::new());
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let attempts = attempts.clone();
            let effect_applied = effect_applied.clone();
            move |rpc| {
                Arc::new(HoldResponseRpc {
                    inner: rpc,
                    held,
                    attempts: attempts.clone(),
                    effect_applied: effect_applied.clone(),
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
    let target_id = agent_id!("RpcCounter", "held-response-target");
    let caller = executor
        .start_agent(&component.id, caller_id.clone())
        .await?;
    let key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        let key = key.clone();
        let method = method.to_string();
        let input = input.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(&component, &caller_id, &key, &method, input)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), effect_applied.notified()).await?;
    let stored_before_crash = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    assert_eq!(
        executor
            .stored_oplog(&caller)
            .await
            .iter()
            .filter(|entry| matches!(entry,
                golem_common::model::oplog::OplogEntry::Start { function_name, .. }
                    if function_name.to_string() == "golem::rpc::wasm-rpc::invoke_and_await"))
            .count(),
        held + 1,
        "every dispatched RPC must already have a committed Start before its response arrives"
    );
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(&component, &caller_id, &key, method, input),
    )
    .await??;

    let recovered = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    let attempts = attempts.lock().unwrap().clone();
    let count = executor
        .invoke_and_await_agent(&component, &target_id, "get_value", data_value!())
        .await?
        .into_typed::<u64>()?;
    let rpc_starts = |oplog: &[PublicOplogEntryWithIndex]| {
        oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == "golem::rpc::wasm-rpc::invoke_and_await")
            })
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>()
    };
    let context = format!(
        "caller {caller_id}.{method}; target count: {count}; dispatch keys: {attempts:?}; \
         RPC Starts stored before the crash: {:?}; RPC Starts after recovery: {:?}",
        rpc_starts(&stored_before_crash),
        rpc_starts(&recovered),
    );

    assert_eq!(attempts.len(), held + 2, "{context}");
    assert!(attempts[held].is_some(), "{context}");
    assert_eq!(
        attempts[held + 1],
        attempts[held],
        "the redispatch must reuse the key of the dispatch whose response was lost; {context}"
    );
    assert_eq!(
        count, expected_count,
        "the target must apply each effect once; {context}"
    );
    Ok(())
}
