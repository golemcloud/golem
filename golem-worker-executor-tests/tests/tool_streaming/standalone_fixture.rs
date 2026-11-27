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

use super::*;
use golem_common::model::component::{ComponentDto, ComponentId};
use golem_common::schema::{IntoSchema, IntoTypedSchemaValue};
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor_test_utils::standalone_tool::StandaloneToolCall;
use tokio_util::sync::CancellationToken;

pub(super) const CHECKPOINT_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const REJECTED_OPERATION_CUT: OplogIndex = OplogIndex::from_u64(2);
pub(super) const ARCHIVE_LAST_INDEX: OplogIndex = OplogIndex::from_u64(100);
pub(super) const INITIAL_FAILURE_RETENTION: Duration = Duration::from_secs(1);
pub(super) const EXTENDED_FAILURE_RETENTION: Duration = Duration::from_secs(10);
pub(super) const FAILURE_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(IntoSchema)]
pub(super) struct ToolValueInput {
    pub value: String,
}

#[derive(IntoSchema)]
pub(super) struct InvocationIdInput {
    #[schema(rename = "rpc-name")]
    pub rpc_name: Option<String>,
}

#[derive(IntoSchema)]
pub(super) struct ToolModeInput {
    pub mode: String,
}

pub(super) struct BaselineToolDeployment {
    pub provider: ComponentDto,
    pub component: ComponentDto,
    pub deployment: ToolDeploymentState,
}

impl BaselineToolDeployment {
    pub fn replaced_by_host(&self, tool_name: &ToolName) -> ToolDeploymentState {
        let mut deployment = self.deployment.clone();
        let source = ToolSource::Host {
            host_tool_id: "mcp-import".to_string().try_into().unwrap(),
            implementation_version: "1".into(),
        };
        deployment
            .registered_tools
            .get_mut(tool_name)
            .unwrap()
            .registration
            .source = source.clone();
        for bindings in deployment.tool_bindings.values_mut() {
            if let Some(binding) = bindings.get_mut(tool_name) {
                binding.source = source.clone();
            }
        }
        deployment
    }

    pub async fn store(
        executor: &TestWorkerExecutor,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        provider: &PrecompiledComponent,
        caller: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let provider_component = executor
            .component_dep(&context.default_environment_id, provider)
            .store()
            .await?;
        let component = executor
            .component_dep(&context.default_environment_id, caller)
            .name("virtual-native-policy")
            .store()
            .await?;
        let metadata = extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", provider.wasm_name)),
            false,
            true,
        )
        .await?;
        let mut deployment =
            rust_provider_deployment(context.account_id, &provider_component, metadata.tools)
                .bound_to("ToolStreamingCaller")
                .build();
        rebind_to_baseline(&mut deployment, component.id);
        Ok(Self {
            provider: provider_component,
            component,
            deployment,
        })
    }
}

pub(super) fn rebind_to_baseline(deployment: &mut ToolDeploymentState, component_id: ComponentId) {
    let agent = ToolBindingOwner::AgentType {
        agent_type_name: AgentTypeName("ToolStreamingCaller".into()),
    };
    let baseline = ToolBindingOwner::ComponentBaseline { component_id };
    let mut bindings = deployment
        .tool_bindings
        .remove(&agent)
        .expect("agent tool bindings");
    for binding in bindings.values_mut() {
        binding.owner = baseline.clone();
    }
    deployment.tool_bindings.insert(baseline.clone(), bindings);
    if let Some(mut chains) = deployment.tool_middleware_chains.remove(&agent) {
        for chain in chains.values_mut() {
            chain.owner = baseline.clone();
        }
        deployment.tool_middleware_chains.insert(baseline, chains);
    }
}

pub(super) struct StandaloneFixture {
    pub context: TestContext,
    pub environment: Arc<TestEnvironmentStateService>,
    pub effect_gate: Arc<NativeOrderEffectGate>,
    pub effects: Arc<AtomicUsize>,
    pub checkpoints: CrashCheckpointServer,
    pub promises: PromiseCheckpointServer,
    pub release_wakes: CancellationToken,
    pub storage: Arc<std::sync::Mutex<Option<Arc<dyn SchedulerStorage + Send + Sync>>>>,
    pub retention_insert: Arc<retention_storage::RetentionInsertGate>,
    pub overrides: TestExecutorOverrides,
    pub tools: BaselineToolDeployment,
    _http: tokio::task::JoinHandle<()>,
}

pub(super) struct StandaloneSetup {
    pub fixture: StandaloneFixture,
    pub executor: TestWorkerExecutor,
}

impl StandaloneFixture {
    pub async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        provider: &PrecompiledComponent,
        caller: &PrecompiledComponent,
    ) -> anyhow::Result<StandaloneSetup> {
        let context = TestContext::new(last_unique_id);
        let environment = Arc::new(TestEnvironmentStateService::default());
        let effect_gate = Arc::new(NativeOrderEffectGate::default());
        let NativeOrderHttpServer {
            port: http_port,
            server: http,
            effects,
        } = start_native_order_http_server(Some(effect_gate.clone())).await;
        let checkpoints = CrashCheckpointServer::start().await?;
        let promises = PromiseCheckpointServer::start().await?;
        let checkpoint_env: BTreeMap<String, String> = checkpoints
            .agent_env("")
            .into_iter()
            .chain(promises.agent_env("PROVIDER"))
            .collect();
        let release_wakes = CancellationToken::new();
        let storage = Arc::new(std::sync::Mutex::new(None));
        let retention_insert = Arc::new(retention_storage::RetentionInsertGate::default());
        let overrides = TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            wrap_scheduler_storage: Some(Arc::new({
                let release = release_wakes.clone();
                let storage = storage.clone();
                let gate = retention_insert.clone();
                move |inner| {
                    *storage.lock().unwrap() = Some(inner.clone());
                    Arc::new(crate::scheduled_ephemeral::PausedScheduler {
                        inner: Arc::new(retention_storage::HoldRetentionInsert {
                            inner,
                            gate: gate.clone(),
                        }),
                        release: release.clone(),
                        invocation_wake: Default::default(),
                        acknowledged: Default::default(),
                    })
                }
            })),
            wrap_component_service: Some(Arc::new(move |inner| {
                Arc::new(BaselineOwnerComponentService {
                    inner,
                    owner_loads: Default::default(),
                    http_port,
                    extra_env: checkpoint_env.clone(),
                })
            })),
            ..Default::default()
        }
        .with_agent_sockets();
        let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let mut tools =
            BaselineToolDeployment::store(&executor, deps, &context, provider, caller).await?;
        let baseline = ToolBindingOwner::ComponentBaseline {
            component_id: tools.component.id,
        };
        tools
            .deployment
            .tool_bindings
            .get_mut(&baseline)
            .unwrap()
            .get_mut(&ToolName::try_from("streaming").unwrap())
            .unwrap()
            .filesystem_access = ToolFilesystemAccess::Allowed;
        environment.set_tool_deployment(
            context.default_environment_id,
            tools.component.id,
            tools.component.revision,
            Some(tools.deployment.clone()),
        );
        let fixture = Self {
            context,
            environment,
            effect_gate,
            effects,
            checkpoints,
            promises,
            release_wakes,
            storage,
            retention_insert,
            overrides,
            tools,
            _http: http,
        };
        Ok(StandaloneSetup { fixture, executor })
    }

    pub fn call(
        &self,
        executor: &TestWorkerExecutor,
        key: &str,
        value: &str,
    ) -> anyhow::Result<StandaloneToolCall> {
        let tool = ToolName::try_from("streaming").unwrap();
        executor.standalone_tool(
            &self.tools.component,
            self.tools.deployment.registered_tools[&tool].type_id,
            "streaming",
            "no-stream",
            &IdempotencyKey::new(key.into()),
            ToolValueInput {
                value: value.into(),
            }
            .into_typed_schema_value()?,
        )
    }
}

pub(super) async fn wait_for_retention_deletion(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(CHECKPOINT_TIMEOUT, async {
        while executor.get_worker_metadata(owner).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
}
