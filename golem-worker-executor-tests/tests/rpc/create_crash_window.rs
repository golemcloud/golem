use super::*;
use golem_common::model::ScanCursor;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentTypeName, InvocationFreshnessDisposition,
};
use golem_common::model::card::ScopeCard;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{CreateRequestKind, OplogEntry};
use golem_worker_executor::services::rpc::{Rpc, RpcDemand};
use golem_worker_executor::worker::target::ResolvedTarget;
use pretty_assertions::assert_eq;
use std::sync::atomic::AtomicBool;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

/// While armed, the first `create_and_initialize` creates the agent and then never returns, so
/// the caller's `create` has a `Start` and no `End` after the agent exists.
struct CreateThenHangRpc {
    inner: Arc<dyn Rpc>,
    hang_after_create: Arc<AtomicBool>,
    created: Arc<tokio::sync::Notify>,
    created_ids: Arc<Mutex<Vec<AgentId>>>,
}

#[async_trait]
impl Rpc for CreateThenHangRpc {
    async fn create_and_initialize(
        &self,
        target: ResolvedTarget,
        exclusive: bool,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        self_caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        idempotency_key: IdempotencyKey,
    ) -> Result<Box<dyn RpcDemand>, golem_service_base::error::worker_executor::WorkerExecutorError>
    {
        let agent_id = target.owned_agent_id.agent_id;
        let result = self
            .inner
            .create_and_initialize(
                target,
                exclusive,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                self_caller,
                config,
                auth_ctx,
                idempotency_key,
            )
            .await;
        self.created_ids.lock().unwrap().push(agent_id);
        if self.hang_after_create.swap(false, Ordering::SeqCst) {
            self.created.notify_one();
            std::future::pending::<()>().await;
        }
        result
    }

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
        expected_fingerprint: AgentFingerprint,
        request_key: InvocationRequestKey,
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
        agent: ResolvedTarget,
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
                agent,
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
        agent: ResolvedTarget,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<AgentFingerprint>,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        callee_caller: &Caller,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        self.inner
            .invoke_and_await(
                agent,
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
        agent: ResolvedTarget,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<AgentFingerprint>,
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
                agent,
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

async fn host_api_agent_ids(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
) -> anyhow::Result<BTreeSet<AgentId>> {
    let (cursor, agents) = executor
        .get_workers_metadata(
            &component.environment_id,
            &AgentTypeName("GolemHostApi".to_string()),
            None,
            ScanCursor::default(),
            100,
            true,
        )
        .await?;
    assert!(cursor.is_none());
    Ok(agents.into_iter().map(|agent| agent.agent_id).collect())
}

/// A create without a key inside an agent allocates the new agent's ID on the host. The executor
/// stops after the agent was created and before the caller's `End`. Recovery of the same call
/// must reach the agent it created, not allocate a second one, and a later call allocates a new ID.
#[test]
#[timeout("2m")]
async fn guest_create_without_a_key_reaches_the_same_agent_after_a_crash_before_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let hang_after_create = Arc::new(AtomicBool::new(true));
    let created = Arc::new(tokio::sync::Notify::new());
    let created_ids = Arc::new(Mutex::new(Vec::new()));
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let hang_after_create = hang_after_create.clone();
            let created = created.clone();
            let created_ids = created_ids.clone();
            move |inner| {
                Arc::new(CreateThenHangRpc {
                    inner,
                    hang_after_create: hang_after_create.clone(),
                    created: created.clone(),
                    created_ids: created_ids.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let caller_key = agent_key!("GolemHostApi", "create-crash-caller");
    let caller = executor
        .start_agent(&component.id, caller_key.clone())
        .await?;
    let key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_key = caller_key.clone();
        let key = key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &caller_key,
                    &key,
                    "outbound_agent_rpc_create_unkeyed_result",
                    data_value!("GolemHostApi", "unkeyed-child"),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), created.notified()).await?;
    let child = created_ids.lock().unwrap()[0];
    assert_ne!(child, caller.agent_id);
    assert_eq!(
        host_api_agent_ids(&executor, &component).await?,
        BTreeSet::from([caller.agent_id, child])
    );
    executor.shutdown_and_wait_for_invocation_loops().await?;
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    let recovered = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller_key,
            &key,
            "outbound_agent_rpc_create_unkeyed_result",
            data_value!("GolemHostApi", "unkeyed-child"),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(recovered, Ok(child.as_uuid().to_string()));
    assert_eq!(
        host_api_agent_ids(&executor, &component).await?,
        BTreeSet::from([caller.agent_id, child]),
        "recovery must not create a second agent"
    );
    let creates = executor
        .stored_oplog(&child)
        .await
        .into_iter()
        .filter_map(|entry| match entry {
            OplogEntry::Create { parameters, .. } => Some(parameters.create_request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0].kind, CreateRequestKind::Unkeyed);
    assert_eq!(
        creates[0].caller,
        callers::agent_in(
            caller.agent_id,
            caller_key.agent_type.clone(),
            executor.get_worker_metadata(&caller).await?.fingerprint,
            caller.environment_id
        )
    );

    let fresh = executor
        .invoke_and_await_agent(
            &component,
            &caller_key,
            "outbound_agent_rpc_create_unkeyed_result",
            data_value!("GolemHostApi", "unkeyed-child"),
        )
        .await?
        .into_typed::<Result<String, String>>()?
        .map_err(|error| anyhow::anyhow!(error))?;
    assert_ne!(fresh, child.as_uuid().to_string());
    let after = host_api_agent_ids(&executor, &component).await?;
    assert_eq!(after.len(), 3, "a new call creates a new agent: {after:?}");
    assert!(after.iter().any(|id| id.as_uuid().to_string() == fresh));
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}
