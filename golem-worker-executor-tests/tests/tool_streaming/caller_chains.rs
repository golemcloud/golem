// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The caller that agents and tools give the agents and tools they call: the original caller
//! stays, and the calling agent or tool is the immediate caller. The built-in shell never
//! appears as a caller.

use super::*;
use async_trait::async_trait;
use golem_common::model::caller::Principal;
use golem_common::model::card::ScopeCard;
use golem_common::model::component::ComponentDto;
use golem_common::model::{AgentFingerprint, AgentId};
use golem_service_base::model::auth::AuthCtx;
use golem_worker_executor::services::rpc::{Rpc, RpcDemand, RpcError};
use golem_worker_executor::worker::target::ResolvedTarget;
use golem_worker_executor_test_utils::callers;
use std::sync::Mutex;
use test_r::{test, timeout};

/// The `ToolStreamingCaller` agents of the caller component get the `streaming` provider tools
/// and the built-in `bash` tool, with filesystem access.
async fn deploy_streaming_and_bash(
    executor: &TestWorkerExecutor,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    environment: &TestEnvironmentStateService,
    caller_component: &golem_common::model::component::ComponentDto,
    provider_component: &golem_common::model::component::ComponentDto,
    streaming: &PrecompiledComponent,
    bash: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let mut deployment: Option<ToolDeploymentState> = None;
    // The streaming tools run in the provider component that the tests read: a second stored
    // copy would also hold the provider's agent types, and calls could reach either copy.
    for (component, package, stored) in [
        (
            streaming,
            "golem-it:tool-streaming-rust-provider",
            Some(provider_component),
        ),
        (bash, "golem:bash", None),
    ] {
        let provider = match stored {
            Some(stored) => stored.clone(),
            None => {
                executor
                    .component_dep(&context.default_environment_id, component)
                    .store()
                    .await?
            }
        };
        let metadata = extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", component.wasm_name)),
            false,
            true,
        )
        .await?;
        let next = deployment_state(
            context.account_id,
            provider.id,
            provider.revision,
            package,
            "ToolStreamingCaller",
            metadata.tools,
        );
        match deployment.as_mut() {
            Some(current) => {
                current.registered_tools.extend(next.registered_tools);
                for (owner, bindings) in next.tool_bindings {
                    current
                        .tool_bindings
                        .entry(owner)
                        .or_default()
                        .extend(bindings);
                }
            }
            None => deployment = Some(next),
        }
    }
    let mut deployment = deployment.unwrap();
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    Ok(())
}

struct ChainHarness {
    executor: TestWorkerExecutor,
    context: TestContext,
    caller_component: golem_common::model::component::ComponentDto,
    provider_component: golem_common::model::component::ComponentDto,
    environment: Arc<TestEnvironmentStateService>,
}

impl ChainHarness {
    async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        streaming: &PrecompiledComponent,
        caller: &PrecompiledComponent,
        bash: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        Self::start_with(
            last_unique_id,
            deps,
            streaming,
            caller,
            bash,
            TestExecutorOverrides::default(),
        )
        .await
    }

    async fn start_with(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        streaming: &PrecompiledComponent,
        caller: &PrecompiledComponent,
        bash: &PrecompiledComponent,
        overrides: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let environment = Arc::new(TestEnvironmentStateService::default());
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                environment_state_service: Some(environment.clone()),
                ..overrides
            },
        )
        .await?;
        let caller_component = executor
            .component_dep(&context.default_environment_id, caller)
            .store()
            .await?;
        let provider_component = executor
            .component_dep(&context.default_environment_id, streaming)
            .store()
            .await?;
        deploy_streaming_and_bash(
            &executor,
            deps,
            &context,
            &environment,
            &caller_component,
            &provider_component,
            streaming,
            bash,
        )
        .await?;
        Ok(Self {
            executor,
            context,
            caller_component,
            provider_component,
            environment,
        })
    }

    /// Drops the executor and starts another one on the same stores, as after a crash: every
    /// agent is rebuilt from its oplog.
    async fn restart(
        mut self,
        deps: &WorkerExecutorTestDependencies,
        overrides: TestExecutorOverrides,
    ) -> anyhow::Result<Self> {
        drop(self.executor);
        self.executor = start_with_overrides(
            deps,
            &self.context,
            TestExecutorOverrides {
                environment_state_service: Some(self.environment.clone()),
                ..overrides
            },
        )
        .await?;
        Ok(self)
    }

    /// The agent `ToolStreamingCaller(name)` of the caller component.
    fn agent(&self, name: &str) -> OwnedAgentId {
        self.executor
            .keyed_agent_id(
                self.context.default_environment_id,
                &agent_key!("ToolStreamingCaller", name),
            )
            .expect("a valid agent key")
    }

    /// The agent `CallerReport(name)` of the provider component.
    fn report_agent(&self, name: &str) -> OwnedAgentId {
        self.executor
            .keyed_agent_id(
                self.context.default_environment_id,
                &agent_key!("CallerReport", name),
            )
            .expect("a valid agent key")
    }

    /// The agent `ToolStreamingCaller(..)` as the executor records it when it makes a call.
    async fn agent_principal(&self, agent: &OwnedAgentId) -> anyhow::Result<Principal> {
        let fingerprint: AgentFingerprint =
            self.executor.get_worker_metadata(agent).await?.fingerprint;
        Ok(callers::agent_in(
            agent.agent_id,
            golem_common::model::agent::AgentTypeName("ToolStreamingCaller".to_string()),
            fingerprint,
            self.context.default_environment_id,
        )
        .immediate)
    }

    fn streaming_tool(&self) -> Principal {
        callers::tool_principal(self.context.default_environment_id, "streaming")
    }

    /// Runs `streaming run <mode>` from the agent `name` as `caller` and returns what the tool
    /// wrote to stdout.
    async fn run_mode(&self, name: &str, caller: &Caller, mode: &str) -> anyhow::Result<String> {
        let evidence: StreamEvidence = self
            .executor
            .invoke_and_await_agent_as_caller(
                &self.caller_component,
                &agent_key!("ToolStreamingCaller", name),
                caller.clone(),
                "collect",
                data_value!(mode, Vec::<u8>::new(), 1u32),
            )
            .await?
            .into_typed()?;
        assert_eq!(evidence.completion, "ok", "{mode}: {evidence:?}");
        Ok(String::from_utf8(evidence.output)?)
    }
}

/// One `report_and_bump` dispatch: its idempotency key and the caller that it gives the target.
type Dispatch = (Option<IdempotencyKey>, Caller);

/// Forwards every call and, once the first `report_and_bump` call has run on its target, never
/// returns its answer to the caller: the call stays pending until the executor is dropped.
struct HoldReportAnswerRpc {
    inner: Arc<dyn Rpc>,
    /// Every `report_and_bump` dispatch, in order.
    dispatches: Arc<Mutex<Vec<Dispatch>>>,
    answer_held: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Rpc for HoldReportAnswerRpc {
    async fn cancel_scheduled_invocation(
        &self,
        target: &OwnedAgentId,
        key: golem_common::model::InvocationId,
        receipt: golem_common::model::ScheduleId,
        canceller: &Principal,
        auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        self.inner
            .cancel_scheduled_invocation(target, key, receipt, canceller, auth)
            .await
    }

    async fn schedule_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: AgentFingerprint,
        request_key: golem_common::model::InvocationRequestKey,
        invocation: golem_common::model::AgentInvocation,
        scheduled_at: jiff::Timestamp,
        self_agent_id: &AgentId,
        auth_ctx: &AuthCtx,
    ) -> Result<golem_common::model::ScheduleId, RpcError> {
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

    async fn create_and_initialize(
        &self,
        target: ResolvedTarget,
        exclusive: bool,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        caller: &Caller,
        config: Vec<golem_common::model::agent::AgentConfigEntryDto>,
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

    async fn create_demand(
        &self,
        target: ResolvedTarget,
        method_name: &str,
        invocation: Option<golem_api_grpc::proto::golem::worker::InvocationStart>,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        caller: &Caller,
        config: Vec<golem_common::model::agent::AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        self.inner
            .create_demand(
                target,
                method_name,
                invocation,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                caller,
                config,
                auth_ctx,
                idempotency_key,
                freshness_disposition,
            )
            .await
    }

    async fn invoke_and_await(
        &self,
        target: ResolvedTarget,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<AgentFingerprint>,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        caller: &Caller,
        config: Vec<golem_common::model::agent::AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        let hold = method_name == "report_and_bump" && {
            let mut dispatches = self.dispatches.lock().unwrap();
            dispatches.push((idempotency_key.clone(), caller.clone()));
            dispatches.len() == 1
        };
        let result = self
            .inner
            .invoke_and_await(
                target,
                idempotency_key,
                freshness_disposition,
                expected_callee_fingerprint,
                method_name,
                method_parameters,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                caller,
                config,
                auth_ctx,
                scope_card,
            )
            .await;
        if hold {
            self.answer_held.notify_one();
            std::future::pending::<()>().await;
        }
        result
    }

    async fn invoke(
        &self,
        target: ResolvedTarget,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        expected_callee_fingerprint: Option<AgentFingerprint>,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        caller: &Caller,
        config: Vec<golem_common::model::agent::AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RpcError> {
        self.inner
            .invoke(
                target,
                idempotency_key,
                freshness_disposition,
                expected_callee_fingerprint,
                method_name,
                method_parameters,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                caller,
                config,
                auth_ctx,
            )
            .await
    }
}

fn with_immediate(caller: &Caller, immediate: Principal) -> Caller {
    Caller {
        immediate,
        ..caller.clone()
    }
}

/// U calls agent A, A calls tool T through its generated client, and T calls agent B: T sees U
/// and A, and B sees U and T. The records say the same.
#[test]
#[timeout("3m")]
async fn a_chain_keeps_the_original_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let harness = ChainHarness::start(last_unique_id, deps, streaming, caller, bash).await?;
    let user = callers::account(AccountId::new());
    let a = harness.agent("chain-a");

    let tool_view = harness.run_mode("chain-a", &user, "caller").await?;
    let a_principal = harness.agent_principal(&a).await?;
    let tool_caller = with_immediate(&user, a_principal);
    assert_eq!(tool_view, callers::caller_text(&tool_caller));

    let agent_view = harness
        .run_mode("chain-a", &user, "call-agent:chain-b")
        .await?;
    let agent_caller = with_immediate(&user, harness.streaming_tool());
    assert_eq!(agent_view, callers::caller_text(&agent_caller));

    // The calling agent recorded the tool's caller in the tool invocation's `Start`, and the
    // agent that the tool called recorded its own caller.
    let tool_starts = harness
        .executor
        .recorded_callers(&a.agent_id)
        .await
        .into_iter()
        .filter(|(name, _)| name == "entity streaming")
        .map(|(_, caller)| caller)
        .collect::<Vec<_>>();
    assert_eq!(tool_starts, [tool_caller.clone(), tool_caller]);
    let b = harness.report_agent("chain-b");
    let recorded = harness.executor.recorded_callers(&b.agent_id).await;
    assert!(
        recorded.iter().any(|(name, _)| name == "report"),
        "{recorded:?}"
    );
    for (name, recorded) in recorded {
        assert_eq!(recorded, agent_caller, "{name}");
    }
    Ok(())
}

/// Agent A runs `bash`, and `bash` runs tool T: T sees A as the immediate caller, never the
/// shell.
#[test]
#[timeout("3m")]
async fn a_command_of_an_agents_shell_is_a_call_of_the_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let harness = ChainHarness::start(last_unique_id, deps, streaming, caller, bash).await?;
    let user = callers::account(AccountId::new());
    let a = harness.agent("shell-a");

    let output: String = harness
        .executor
        .invoke_and_await_agent_as_caller(
            &harness.caller_component,
            &agent_key!("ToolStreamingCaller", "shell-a"),
            user.clone(),
            "run_bash",
            data_value!("streaming run caller"),
        )
        .await?
        .into_typed()?;
    let a_principal = harness.agent_principal(&a).await?;
    assert_eq!(
        output,
        callers::caller_text(&with_immediate(&user, a_principal))
    );
    Ok(())
}

/// Tool T runs `bash`, and `bash` runs tool T again: the inner T sees the outer T as the
/// immediate caller, not the agent that hosts both.
#[test]
#[timeout("3m")]
async fn a_command_of_a_tools_shell_is_a_call_of_the_tool(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let harness = ChainHarness::start(last_unique_id, deps, streaming, caller, bash).await?;
    let user = callers::account(AccountId::new());

    let output = harness
        .run_mode("shell-t", &user, "bash:streaming run caller")
        .await?;
    assert_eq!(
        output,
        callers::caller_text(&with_immediate(&user, harness.streaming_tool()))
    );
    Ok(())
}

/// A user starts `bash` on agent A from outside, as `golem ssh` does, and `bash` runs tool T: T
/// sees the user as the original caller and A as the immediate one.
#[test]
#[timeout("3m")]
async fn a_command_of_a_shell_started_on_an_agent_is_a_call_of_the_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let harness = ChainHarness::start(last_unique_id, deps, streaming, caller, bash).await?;
    let user = callers::account(AccountId::new());
    let a = harness
        .executor
        .start_agent(
            &harness.caller_component.id,
            agent_key!("ToolStreamingCaller", "ssh-a"),
        )
        .await?;
    let fingerprint = harness.executor.get_worker_metadata(&a).await?.fingerprint;
    let input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(
            // The canonical input of `bash run`: the positional script, then the options.
            [
                ("script", SchemaType::string()),
                ("cwd", SchemaType::string()),
                ("timeout", SchemaType::u32()),
            ]
            .into_iter()
            .map(|(name, body)| golem_common::schema::NamedFieldType {
                name: name.to_string(),
                body,
                metadata: Default::default(),
            })
            .collect(),
        )),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String("streaming run caller".to_string()),
                SchemaValue::String(String::new()),
                SchemaValue::U32(60),
            ],
        },
    );
    let output = harness
        .executor
        .invoke_external_tool(
            &a.agent_id,
            fingerprint,
            IdempotencyKey::fresh(),
            ToolName::try_from("bash").unwrap(),
            vec!["run".to_string()],
            input,
            InvocationContextStack::fresh(),
            user.clone(),
            None,
        )
        .await?;
    let AgentInvocationResult::ExternalTool { result } = output.result else {
        anyhow::bail!("expected an external tool result, got {output:?}");
    };
    let result = result
        .map_err(|error| anyhow::anyhow!("bash failed: {error:?}"))?
        .result
        .expect("bash returns its output");
    let SchemaValue::Record { fields } = result.into_parts().1 else {
        anyhow::bail!("bash returns a record");
    };
    let [
        SchemaValue::String(stdout),
        SchemaValue::String(stderr),
        SchemaValue::U8(0),
        _,
    ] = fields.as_slice()
    else {
        anyhow::bail!("bash failed: {fields:?}");
    };
    assert!(stderr.is_empty(), "{stderr}");
    let a_principal = harness.agent_principal(&a).await?;
    assert_eq!(
        stdout,
        &callers::caller_text(&with_immediate(&user, a_principal))
    );
    Ok(())
}

/// Tool T schedules a call on agent B and cancels it: the call never runs. A call that T
/// schedules and does not cancel runs.
#[test]
#[timeout("3m")]
async fn a_tool_cancels_the_call_that_it_scheduled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let harness = ChainHarness::start(last_unique_id, deps, streaming, caller, bash).await?;
    let user = callers::account(AccountId::new());
    let b = agent_key!("CallerReport", "scheduled-b");
    let count = || async {
        harness
            .executor
            .invoke_and_await_agent(&harness.provider_component, &b, "count", data_value!())
            .await?
            .into_typed::<u32>()
    };

    assert_eq!(
        harness
            .run_mode("scheduler-a", &user, "schedule-and-cancel-bump:scheduled-b")
            .await?,
        "cancelled"
    );
    // The cancelled call was due two seconds after it was scheduled.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(count().await?, 0, "the cancelled call ran");

    assert_eq!(
        harness
            .run_mode("scheduler-a", &user, "schedule-bump:scheduled-b")
            .await?,
        "scheduled"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while count().await? == 0 {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the scheduled call did not run"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(count().await?, 1);
    Ok(())
}

/// U calls agent A, A calls tool T, and T calls agent B, but B's answer never reaches T: the
/// executor is rebuilt while it is held. A and T are rebuilt from the oplog, T calls B again with
/// the key of the lost call, and every call keeps the caller it had: T sees U and A, B sees U and
/// T, the records are unchanged, and B's effect runs once.
#[test]
#[timeout("5m")]
async fn a_restart_inside_the_tool_chain_keeps_every_recorded_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let dispatches = Arc::new(Mutex::new(Vec::new()));
    let answer_held = Arc::new(tokio::sync::Notify::new());
    let overrides = TestExecutorOverrides {
        wrap_rpc: Some(Arc::new({
            let dispatches = dispatches.clone();
            let answer_held = answer_held.clone();
            move |rpc| {
                Arc::new(HoldReportAnswerRpc {
                    inner: rpc,
                    dispatches: dispatches.clone(),
                    answer_held: answer_held.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let harness = ChainHarness::start_with(
        last_unique_id,
        deps,
        streaming,
        caller,
        bash,
        overrides.clone(),
    )
    .await?;
    let user = callers::account(AccountId::new());
    let a = harness.agent("chain-a");
    let b = harness.report_agent("chain-b");
    let key = IdempotencyKey::fresh();
    let mode = "call-agent-bump:chain-b";
    let invoke = |executor: TestWorkerExecutor, component: ComponentDto| {
        let user = user.clone();
        let key = key.clone();
        async move {
            executor
                .invoke_and_await_agent_impl(
                    &component,
                    &agent_key!("ToolStreamingCaller", "chain-a"),
                    Some(&key),
                    Some(user),
                    "collect",
                    data_value!(mode, Vec::<u8>::new(), 1u32),
                )
                .await
        }
    };
    let call = tokio::spawn(invoke(
        harness.executor.clone(),
        harness.caller_component.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(60), answer_held.notified()).await?;

    // B has run its effect and recorded its caller, and A has recorded the tool's caller in the
    // tool invocation's `Start`, before the answer is lost.
    let a_principal = harness.agent_principal(&a).await?;
    let tool_caller = with_immediate(&user, a_principal);
    let agent_caller = with_immediate(&user, harness.streaming_tool());
    let tool_starts = |recorded: Vec<(String, Caller)>| {
        recorded
            .into_iter()
            .filter(|(name, _)| name == "entity streaming")
            .map(|(_, caller)| caller)
            .collect::<Vec<_>>()
    };
    let tool_starts_before = tool_starts(harness.executor.recorded_callers(&a.agent_id).await);
    let b_before = harness.executor.recorded_callers(&b.agent_id).await;
    assert!(!tool_starts_before.is_empty());
    assert!(
        tool_starts_before
            .iter()
            .all(|caller| *caller == tool_caller),
        "{tool_starts_before:?}"
    );
    assert!(
        b_before.iter().any(|(name, _)| name == "report_and_bump"),
        "{b_before:?}"
    );

    call.abort();
    let _ = call.await;
    let harness = harness.restart(deps, overrides).await?;

    let result: StreamEvidence = tokio::time::timeout(
        Duration::from_secs(120),
        invoke(harness.executor.clone(), harness.caller_component.clone()),
    )
    .await??
    .into_typed()?;
    assert_eq!(result.completion, "ok", "{result:?}");
    let output = String::from_utf8(result.output)?;
    let (tool_view, agent_view) = output.split_once('\n').expect("two lines");
    assert_eq!(tool_view, callers::caller_text(&tool_caller));
    assert_eq!(agent_view, callers::caller_text(&agent_caller));

    // The lost call was sent again with its key and the caller it had.
    let dispatches = dispatches.lock().unwrap().clone();
    assert_eq!(dispatches.len(), 2, "{dispatches:?}");
    assert!(dispatches[0].0.is_some());
    assert_eq!(dispatches[1].0, dispatches[0].0);
    for (_, callee_caller) in &dispatches {
        assert_eq!(
            callers::caller_text(callee_caller),
            callers::caller_text(&agent_caller)
        );
    }

    // The records did not change.
    let tool_starts_after = tool_starts(harness.executor.recorded_callers(&a.agent_id).await);
    assert_eq!(tool_starts_after, tool_starts_before);
    let b_after = harness.executor.recorded_callers(&b.agent_id).await;
    assert_eq!(b_after, b_before);
    for (name, recorded) in &b_after {
        assert_eq!(*recorded, agent_caller, "{name}");
    }

    // B applied the bump once.
    let count: u32 = harness
        .executor
        .invoke_and_await_agent(
            &harness.provider_component,
            &agent_key!("CallerReport", "chain-b"),
            "count",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(count, 1);
    Ok(())
}
