use super::*;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentTarget, InvocationFreshnessDisposition,
};
use golem_common::model::card::ScopeCard;
use golem_common::model::environment::EnvironmentId;
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
/// call stays pending until the executor is dropped. With `hold_first_cancel`, the first scheduled
/// invocation cancellation is held the same way after the target commits it.
struct HoldResponseRpc {
    inner: Arc<dyn Rpc>,
    held: usize,
    attempts: Arc<Mutex<Vec<Option<IdempotencyKey>>>>,
    effect_applied: Arc<tokio::sync::Notify>,
    hold_first_cancel: bool,
    cancel_results: Arc<Mutex<Vec<bool>>>,
}

#[async_trait]
impl Rpc for HoldResponseRpc {
    async fn cancel_scheduled_invocation(
        &self,
        target: &OwnedAgentId,
        key: IdempotencyKey,
        receipt: golem_common::model::ScheduleId,
        owner: &AgentId,
        auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        let result = self
            .inner
            .cancel_scheduled_invocation(target, key, receipt, owner, auth)
            .await;
        let hold = {
            let mut results = self.cancel_results.lock().unwrap();
            if let Ok(cancelled) = &result {
                results.push(*cancelled);
            }
            self.hold_first_cancel && results.len() == 1
        };
        if hold {
            self.effect_applied.notify_one();
            std::future::pending::<()>().await;
        }
        result
    }

    async fn schedule_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: golem_common::model::AgentFingerprint,
        invocation: golem_common::model::AgentInvocation,
        scheduled_at: chrono::DateTime<chrono::Utc>,
        auth_ctx: &AuthCtx,
    ) -> Result<golem_common::model::ScheduleId, RpcError> {
        self.inner
            .schedule_invocation(
                owned_agent_id,
                expected_fingerprint,
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
                agent,
                environment_id,
                key,
                freshness,
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
        if hold {
            self.effect_applied.notify_one();
            std::future::pending::<()>().await;
        }
        result
    }

    async fn invoke(
        &self,
        agent: AgentTarget,
        environment_id: EnvironmentId,
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
                agent,
                environment_id,
                key,
                freshness,
                method,
                input,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
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
                    hold_first_cancel: false,
                    cancel_results: Arc::default(),
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

/// The target removes the pending entry of a scheduled invocation, but the caller crashes before
/// the token cancellation records its `End`. Recovery repairs the incomplete cancellation in
/// place: the target reports nothing left to cancel and the scheduled invocation never runs.
#[test]
#[timeout("3m")]
async fn committed_scheduled_cancellation_completes_after_caller_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let effect_applied = Arc::new(tokio::sync::Notify::new());
    let cancel_results = Arc::new(Mutex::new(Vec::new()));
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let effect_applied = effect_applied.clone();
            let cancel_results = cancel_results.clone();
            move |rpc| {
                Arc::new(HoldResponseRpc {
                    inner: rpc,
                    held: 0,
                    attempts: Arc::default(),
                    effect_applied: effect_applied.clone(),
                    hold_first_cancel: true,
                    cancel_results: cancel_results.clone(),
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
    let server_name = "held-cancellation-target";
    let server = agent_id!("ScheduledInvocationServer", server_name);
    let caller_id = agent_id!("ScheduledInvocationClient", "held-cancellation-owner");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let caller = executor
        .start_agent(&component.id, caller_id.clone())
        .await?;
    let due = chrono::Utc::now() + chrono::Duration::hours(1);
    executor
        .invoke_and_await_agent(
            &component,
            &caller_id,
            "schedule_at",
            data_value!(server_name.to_string(), due.timestamp() as u64, 0u32),
        )
        .await?;
    let scheduled_key = executor
        .attached_agent_status(&target)
        .await?
        .pending_invocations
        .iter()
        .find(|pending| pending.not_before.is_some())
        .and_then(|pending| pending.idempotency_key().cloned())
        .expect("the schedule must commit a future pending invocation on the target");

    let key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &caller_id,
                    &key,
                    "cancel_scheduled",
                    data_value!(),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), effect_applied.notified()).await?;
    assert_eq!(*cancel_results.lock().unwrap(), vec![true]);
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(
            &component,
            &caller_id,
            &key,
            "cancel_scheduled",
            data_value!(),
        ),
    )
    .await??;

    assert_eq!(
        *cancel_results.lock().unwrap(),
        vec![true, false],
        "recovery must repeat the committed cancellation once and find nothing left to cancel"
    );
    let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    let cancel_starts = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start)
            if start.function_name == "golem::rpc::cancellation-token::cancel")
        })
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    assert_eq!(
        cancel_starts.len(),
        1,
        "the repair must complete the original Start"
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end)
                if end.start_index == cancel_starts[0]))
            .count(),
        1,
        "the cancellation Start must have exactly one End"
    );
    assert!(
        executor
            .attached_agent_status(&target)
            .await?
            .pending_invocations
            .iter()
            .all(|pending| !pending.has_idempotency_key(&scheduled_key))
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        0
    );
    Ok(())
}
