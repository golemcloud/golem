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

use crate::Tracing;
use anyhow::Context;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::post;
use futures::StreamExt;
use golem_common::agent_id;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::extraction::extract_component_metadata;
use golem_common::model::agent::{AgentTypeName, GolemUserPrincipal, Principal};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::oplog::payload::types::{
    SerializableEntityBodyExecution, SerializableToolOperationTerminal, SerializableToolRpcError,
};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolDeploymentState, ToolFilesystemAccess,
    ToolName, ToolProvisionConfig, ToolSource,
};
use golem_common::schema::{
    BinaryRestrictions, BinaryValuePayload, FromSchema, SchemaGraph, SchemaType, SchemaValue,
    TypedSchemaValue, build_input_record,
};
use golem_common::{
    data_value,
    model::{AgentStatus, OwnedAgentId, RetryConfig},
};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::tool::{
    ToolAttachmentModeMetadata, ToolAttachmentTerminalMetadata, ToolBodyAdmissionMetadata,
    ToolOperationLaneMetadata, ToolOperationMetadata, ToolOperationWinnerMetadata,
    ToolOwnerFailureMetadata,
};
use golem_worker_executor::services::environment_state::{
    EnvironmentStateService, ToolActivationSnapshot, ToolDiscoveryError,
};
use golem_worker_executor::worker::owner_lane::OwnerInvocationId;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::{inherit_test_dep, test, timeout};
use tokio_stream::wrappers::ReceiverStream;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_ts_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_ts_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_scala")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_moonbit")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("large_dynamic_memory")]
    PrecompiledComponent
);

#[derive(Debug, FromSchema)]
struct StreamEvidence {
    output: Vec<u8>,
    chunks_read: u32,
    bytes_read: u64,
    output_closed: bool,
    completion: String,
}

#[derive(Debug, FromSchema)]
struct TsStreamEvidence {
    output: Vec<u8>,
    bytes_read: u64,
}

#[derive(Debug, FromSchema)]
struct ScalaStreamEvidence {
    output: String,
    bytes_read: i64,
}

#[derive(Debug, FromSchema)]
struct ScalaCleanupEvidence {
    error: String,
    stdin_cancelled: bool,
    stdout_terminal: String,
}

fn deployment_state(
    owner_account_id: AccountId,
    provider_component_id: golem_common::model::component::ComponentId,
    provider_revision: ComponentRevision,
    provider_component_name: &str,
    caller_agent_type: &str,
    definitions: Vec<golem_common::schema::tool::Tool>,
) -> ToolDeploymentState {
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let account_email = AccountEmail::new("test@golem");
    let mut registered_tools = BTreeMap::new();

    for definition in definitions {
        let root_name = definition
            .commands
            .nodes
            .first()
            .expect("tool definition has a root command")
            .name
            .clone();
        let name = ToolName::try_from(root_name).expect("valid discovered tool name");
        registered_tools.insert(
            name,
            RegisteredTool {
                deployment_revision,
                definition,
                provision: ToolProvisionConfig::default(),
                source: ToolSource::Component {
                    component_id: provider_component_id,
                    component_revision: provider_revision,
                    component_name: ComponentName(provider_component_name.to_string()),
                },
                owner_account_id,
                owner_account_email: account_email.clone(),
                metadata_version: "0.1.0".to_string(),
            },
        );
    }

    let agent_type = AgentTypeName(caller_agent_type.to_string());
    let bindings = registered_tools
        .iter()
        .map(|(name, tool)| {
            let filesystem_access = if name.as_str() == "capable-streaming" {
                ToolFilesystemAccess::Allowed
            } else {
                ToolFilesystemAccess::Unset
            };
            (
                name.clone(),
                CompiledToolBinding {
                    deployment_revision,
                    agent_type_name: agent_type.clone(),
                    tool_name: name.clone(),
                    version: tool.definition.version.clone(),
                    metadata_version: tool.metadata_version.clone(),
                    account_id: owner_account_id,
                    account_email: account_email.clone(),
                    parameters: NormalizedJsonValue::new(serde_json::json!({})),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access,
                    source: tool.source.clone(),
                },
            )
        })
        .collect();

    ToolDeploymentState {
        deployment_revision,
        registered_tools,
        agent_tool_bindings: BTreeMap::from([(agent_type, bindings)]),
    }
}

struct ReorderedToolActivationService {
    inner: TestEnvironmentStateService,
    activation_calls: AtomicUsize,
    first_call_blocked: tokio::sync::Notify,
    release_first_call: tokio::sync::Notify,
}

impl Default for ReorderedToolActivationService {
    fn default() -> Self {
        Self {
            inner: TestEnvironmentStateService::default(),
            activation_calls: AtomicUsize::new(0),
            first_call_blocked: tokio::sync::Notify::new(),
            release_first_call: tokio::sync::Notify::new(),
        }
    }
}

impl ReorderedToolActivationService {
    fn set_tool_deployment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        component_id: golem_common::model::component::ComponentId,
        component_revision: ComponentRevision,
        deployment: Option<ToolDeploymentState>,
    ) {
        self.inner.set_tool_deployment(
            environment_id,
            component_id,
            component_revision,
            deployment,
        );
    }

    fn activation_calls(&self) -> usize {
        self.activation_calls.load(Ordering::SeqCst)
    }

    async fn wait_for_first_call_to_block(&self) {
        self.first_call_blocked.notified().await;
    }

    fn release_first_call(&self) {
        self.release_first_call.notify_one();
    }
}

#[async_trait::async_trait]
impl EnvironmentStateService for ReorderedToolActivationService {
    async fn get_agent_deployment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        agent_type: &AgentTypeName,
    ) -> Result<
        Option<golem_service_base::model::AgentDeploymentDetails>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner
            .get_agent_deployment(environment_id, agent_type)
            .await
    }

    async fn get_agent_secrets(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) -> Result<
        HashMap<
            golem_common::model::agent_secret::CanonicalAgentSecretPath,
            golem_service_base::model::agent_secret::AgentSecret,
        >,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner.get_agent_secrets(environment_id).await
    }

    async fn get_agent_secret_revision(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        agent_secret_id: golem_common::model::agent_secret::AgentSecretId,
        path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
        revision: golem_common::model::agent_secret::AgentSecretRevision,
    ) -> Result<
        Option<golem_service_base::model::agent_secret::AgentSecret>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner
            .get_agent_secret_revision(environment_id, agent_secret_id, path, revision)
            .await
    }

    async fn get_retry_policies(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) -> Result<
        Vec<golem_common::model::retry_policy::NamedRetryPolicy>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner.get_retry_policies(environment_id).await
    }

    async fn get_tool_activation(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        agent_type: &AgentTypeName,
        tool_name: &ToolName,
    ) -> Result<Option<ToolActivationSnapshot>, ToolDiscoveryError> {
        match self.activation_calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.first_call_blocked.notify_one();
                self.release_first_call.notified().await;
                Ok(None)
            }
            1 => {
                self.inner
                    .get_tool_activation(environment_id, agent_type, tool_name)
                    .await
            }
            ordinal => panic!(
                "replay unexpectedly performed tool activation lookup number {}",
                ordinal + 1
            ),
        }
    }
}

fn assert_evidence(evidence: &StreamEvidence, output: &[u8], chunks_read: u32, bytes_read: u64) {
    assert_eq!(evidence.output, output);
    assert_eq!(evidence.chunks_read, chunks_read);
    assert_eq!(evidence.bytes_read, bytes_read);
    assert!(!evidence.output_closed);
    assert_eq!(evidence.completion, "ok");
}

type HttpGateState = (
    Arc<tokio::sync::Barrier>,
    tokio::sync::mpsc::UnboundedSender<(String, Vec<u8>)>,
    tokio::sync::mpsc::UnboundedSender<(String, Vec<u8>)>,
);

async fn gated_http_stream(State(state): State<HttpGateState>, request: Request) -> Response<Body> {
    let tag = request.uri().path().trim_start_matches('/').to_string();
    let mut request_body = request.into_body().into_data_stream();
    let first = request_body
        .next()
        .await
        .expect("gated HTTP request has an initial body chunk")
        .expect("read gated HTTP request body");
    state
        .1
        .send((tag.clone(), first.to_vec()))
        .expect("record initial gated HTTP upload");
    state.0.wait().await;

    let (response_tx, response_rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        let mut received = first.to_vec();
        response_tx
            .send(Ok::<_, Infallible>(Bytes::from(format!("http-{tag}:"))))
            .await
            .expect("send gated HTTP response marker");
        response_tx
            .send(Ok::<_, Infallible>(first))
            .await
            .expect("echo initial gated HTTP upload");
        while let Some(chunk) = request_body.next().await {
            let chunk = chunk.expect("read remaining gated HTTP request body");
            received.extend_from_slice(&chunk);
            response_tx
                .send(Ok::<_, Infallible>(chunk))
                .await
                .expect("echo remaining gated HTTP upload");
        }
        state
            .2
            .send((tag, received))
            .expect("record completed gated HTTP upload");
    });

    Response::builder()
        .status(200)
        .body(Body::from_stream(ReceiverStream::new(response_rx)))
        .expect("build gated HTTP response")
}

async fn start_gated_http_server() -> (
    u16,
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)>,
    tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind gated HTTP server");
    let port = listener.local_addr().expect("gated HTTP address").port();
    let (first_tx, first_rx) = tokio::sync::mpsc::unbounded_channel();
    let (complete_tx, complete_rx) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new()
        .route("/{tag}", post(gated_http_stream))
        .with_state((
            Arc::new(tokio::sync::Barrier::new(2)),
            first_tx,
            complete_tx,
        ));
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve gated HTTP requests");
    });
    (port, task, first_rx, complete_rx)
}

async fn start_trap_attempt_server() -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind trap-attempt server");
    let port = listener.local_addr().expect("trap-attempt address").port();
    let task = tokio::spawn(async move {
        let mut attempt = 0_u64;
        loop {
            let (mut connection, _) = listener
                .accept()
                .await
                .expect("accept trap-attempt connection");
            connection
                .write_all(&[u8::from(attempt != 0)])
                .await
                .expect("write trap-attempt response");
            attempt += 1;
        }
    });
    (port, task)
}

struct CrashCheckpointArrival {
    name: String,
    release: tokio::sync::oneshot::Sender<()>,
}

async fn announce_crash_checkpoint(
    Path(name): Path<String>,
    State(current_name): State<Arc<tokio::sync::RwLock<Option<String>>>>,
) -> axum::http::StatusCode {
    *current_name.write().await = Some(name);
    axum::http::StatusCode::NO_CONTENT
}

async fn start_crash_checkpoint_server() -> (
    u16,
    u16,
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<CrashCheckpointArrival>,
) {
    use tokio::io::AsyncWriteExt;

    let announcement_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind crash checkpoint server");
    let announcement_port = announcement_listener
        .local_addr()
        .expect("crash checkpoint address")
        .port();
    let gate_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind crash checkpoint gate");
    let gate_port = gate_listener
        .local_addr()
        .expect("crash checkpoint gate address")
        .port();
    let (arrivals, received) = tokio::sync::mpsc::unbounded_channel();
    let current_name = Arc::new(tokio::sync::RwLock::new(None));
    let app = Router::new()
        .route("/{name}", post(announce_crash_checkpoint))
        .with_state(current_name.clone());
    let task = tokio::spawn(async move {
        let announcements = axum::serve(announcement_listener, app);
        let gates = async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = gate_listener.accept() => {
                        let (mut connection, _) = accepted.expect("accept crash checkpoint gate");
                        let name = current_name
                            .read()
                            .await
                            .clone()
                            .expect("checkpoint name announced before gate connection");
                        let arrivals = arrivals.clone();
                        connections.spawn(async move {
                            eprintln!("crash checkpoint server received `{name}`");
                            let (release, wait) = tokio::sync::oneshot::channel();
                            arrivals
                                .send(CrashCheckpointArrival { name, release })
                                .expect("record crash checkpoint arrival");
                            if wait.await.is_ok() {
                                connection
                                    .write_all(&[1])
                                    .await
                                    .expect("release crash checkpoint gate");
                            }
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => {
                        completed
                            .expect("crash checkpoint gate task exists")
                            .expect("serve crash checkpoint gate");
                    }
                }
            }
        };
        tokio::select! {
            result = announcements => result.expect("serve crash checkpoint announcements"),
            () = gates => unreachable!("crash checkpoint gate listener does not stop"),
        }
    });
    (announcement_port, gate_port, task, received)
}

async fn wait_for_active_tool_operations(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    expected: usize,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor
                .active_entity_metadata(agent_id)
                .await
                .is_some_and(|active| active.tool_operations.operations.len() == expected)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {expected} active tool operations"))?;
    Ok(())
}

async fn wait_for_tool_stdin_state(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    accepted_bytes: u64,
    delivered_bytes: u64,
    buffered_bytes: usize,
    capacity_bytes: usize,
    backpressured: bool,
    terminal: Option<ToolAttachmentTerminalMetadata>,
    producer_operation_active: bool,
    producer_active: bool,
    consumer_active: bool,
) -> anyhow::Result<()> {
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(active) = executor.active_entity_metadata(agent_id).await
                && let Some(stdin) = active
                    .tool_operations
                    .operations
                    .first()
                    .and_then(|operation| operation.stdin.as_ref())
                && stdin.accepted_bytes == accepted_bytes
                && stdin.delivered_bytes == delivered_bytes
                && stdin.buffered_bytes == buffered_bytes
                && stdin.capacity_bytes == capacity_bytes
                && stdin.backpressured == backpressured
                && stdin.terminal == terminal
                && stdin.producer_operation_active == producer_operation_active
                && stdin.producer_active == producer_active
                && stdin.consumer_active == consumer_active
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if result.is_err() {
        let active = executor.active_entity_metadata(agent_id).await;
        anyhow::bail!(
            "timed out waiting for tool stdin state accepted={accepted_bytes}, delivered={delivered_bytes}, buffered={buffered_bytes}, capacity={capacity_bytes}, backpressured={backpressured}, terminal={terminal:?}, producer-operation-active={producer_operation_active}, producer-active={producer_active}, consumer-active={consumer_active}; active metadata: {active:#?}"
        );
    }
    Ok(())
}

async fn wait_for_owner_replay_settling(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor.owner_replay_is_settling(agent_id).await? {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for owner replay to enter settlement"))??;
    Ok(())
}

async fn wait_for_completed_entity_terminal(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
) -> anyhow::Result<OplogIndex> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
            if let Some(start_index) = oplog.iter().find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke"
                        && oplog.iter().any(|candidate| {
                            matches!(
                                &candidate.entry,
                                PublicOplogEntry::End(params)
                                    if params.start_index == entry.oplog_index
                            )
                        }) =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            }) {
                return Ok::<_, anyhow::Error>(start_index);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for completed entity terminal"))?
}

async fn next_crash_checkpoint(
    arrivals: &mut tokio::sync::mpsc::UnboundedReceiver<CrashCheckpointArrival>,
    expected: &str,
) -> anyhow::Result<CrashCheckpointArrival> {
    let arrival = tokio::time::timeout(std::time::Duration::from_secs(30), arrivals.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for `{expected}` checkpoint"))?
        .ok_or_else(|| anyhow::anyhow!("checkpoint server stopped before `{expected}`"))?;
    assert_eq!(arrival.name, expected);
    Ok(arrival)
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn concurrent_tool_attempt_identity_survives_reordered_admission_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(ReorderedToolActivationService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "attempt-identity-replay");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let call = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "concurrent_attempt_identity_replay",
        data_value!(),
    );
    let crash_and_replay = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            environment_state.wait_for_first_call_to_block(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("first activation lookup did not block"))?;
        let original_provider = next_crash_checkpoint(
            &mut provider_checkpoint_arrivals,
            "attempt-identity-accepted",
        )
        .await?;
        original_provider
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original accepted tool checkpoint was dropped"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        environment_state.release_first_call();

        let original = next_crash_checkpoint(
            &mut caller_checkpoint_arrivals,
            "concurrent-attempt-identities",
        )
        .await?;
        assert_eq!(environment_state.activation_calls(), 2);

        executor.simulated_crash(&worker_id).await?;
        drop(original.release);

        assert_eq!(
            environment_state.activation_calls(),
            2,
            "replay must claim both attempts without repeating admission"
        );
        let replayed = next_crash_checkpoint(
            &mut caller_checkpoint_arrivals,
            "concurrent-attempt-identities",
        )
        .await?;
        replayed
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed attempt-identity checkpoint was dropped"))?;
        Ok::<_, anyhow::Error>(())
    };
    let (result, ()) = tokio::try_join!(call, crash_and_replay)?;
    let outcomes: Vec<String> = result.into_typed()?;
    assert_eq!(outcomes, ["rejected", "accepted"]);
    assert_eq!(environment_state.activation_calls(), 2);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let accepted_start = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("second attempt retained its accepted Start");
    let rejected_start = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::tool::internal::invocation-rejected" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("first attempt retained its rejected Start");
    assert!(
        accepted_start < rejected_start,
        "the later accepted attempt must be durably ordered before the earlier rejected attempt"
    );
    assert!(
        executor
            .active_entity_metadata(&owned_agent_id)
            .await
            .is_none_or(|active| active.tool_operations.operations.is_empty())
    );

    executor.delete_worker(&worker_id).await?;
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn rust_generated_client_streams_live_and_handles_edges(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (http_port, http_server, mut first_http_uploads, mut complete_http_uploads) =
        start_gated_http_server().await;
    let (trap_once_port, trap_once_server) = start_trap_attempt_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "rust-live");
    let mut env = HashMap::new();
    env.insert("HTTP_GATE_PORT".to_string(), http_port.to_string());
    env.insert("TRAP_ONCE_PORT".to_string(), trap_once_port.to_string());
    let worker_id = executor
        .start_agent_with(&caller_component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let principal_context: Vec<String> = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            Principal::GolemUser(GolemUserPrincipal {
                account_id: AccountId::new(),
            }),
            "principal_context",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        principal_context,
        ["golem-user", "golem-user", "golem-user"]
    );

    let marker_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "marker_before_eof",
            data_value!(b"first".to_vec(), b"second".to_vec()),
        ),
    )
    .await;
    let marker: StreamEvidence = match marker_result {
        Ok(result) => result?.into_typed()?,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!("marker-before-EOF call timed out; active metadata: {active:#?}")
        }
    };
    assert_evidence(&marker, b"marker:firstsecond", 2, 11);

    let alternating_chunks = (0..128_u16)
        .map(|index| vec![(index % 251) as u8; 4096])
        .collect::<Vec<_>>();
    let alternating_output = alternating_chunks.concat();
    let alternating: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "alternating_echo",
            data_value!(128_u32, 4096_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(
        &alternating,
        &alternating_output,
        128,
        alternating_output.len() as u64,
    );

    let binary_input = vec![0, 1, 255, 128, 0, 13, 10];
    let echo: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("echo", binary_input.clone(), 2_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&echo, &binary_input, 4, binary_input.len() as u64);

    let empty: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("empty", Vec::<u8>::new(), 1_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&empty, b"", 0, 0);

    let binary: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("binary", Vec::<u8>::new(), 1_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&binary, &[0, 255, 1, 128, 0, 13, 10, 254], 0, 0);

    let fragmented: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("fragmented", Vec::<u8>::new(), 1_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&fragmented, &[b'f', b'r', b'a', b'g', 0, 255], 0, 0);

    let partial_error: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "result_before_stdout",
            data_value!("declared-error"),
        )
        .await?
        .into_typed()?;
    assert_eq!(partial_error.output, b"marker:");
    assert!(partial_error.completion.contains("Declared"));

    let concurrent: Vec<Vec<u8>> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "two_live_calls",
            data_value!(b"left".to_vec(), b"right".to_vec()),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        concurrent,
        [b"marker:left".to_vec(), b"marker:right".to_vec()]
    );

    let left_first = b"left-first\0".to_vec();
    let left_rest = vec![255, b'l', b'e', b'f', b't'];
    let right_first = b"right-first\0".to_vec();
    let right_rest = vec![128, b'r', b'i', b'g', b'h', b't'];
    let http: Vec<StreamEvidence> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "two_http_calls",
            data_value!(
                left_first.clone(),
                left_rest.clone(),
                right_first.clone(),
                right_rest.clone()
            ),
        )
        .await?
        .into_typed()?;
    let left_body = [left_first.clone(), left_rest.clone()].concat();
    let right_body = [right_first.clone(), right_rest.clone()].concat();
    let left_output = [b"http-left:".as_slice(), left_body.as_slice()].concat();
    let right_output = [b"http-right:".as_slice(), right_body.as_slice()].concat();
    assert_evidence(&http[0], &left_output, 2, left_body.len() as u64);
    assert_evidence(&http[1], &right_output, 2, right_body.len() as u64);

    let mut initial_uploads = BTreeMap::new();
    let mut complete_uploads = BTreeMap::new();
    for _ in 0..2 {
        let (tag, bytes) =
            tokio::time::timeout(std::time::Duration::from_secs(5), first_http_uploads.recv())
                .await
                .expect("initial gated HTTP upload checkpoint timed out")
                .expect("initial gated HTTP upload channel closed");
        initial_uploads.insert(tag, bytes);

        let (tag, bytes) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            complete_http_uploads.recv(),
        )
        .await
        .expect("completed gated HTTP upload checkpoint timed out")
        .expect("completed gated HTTP upload channel closed");
        complete_uploads.insert(tag, bytes);
    }
    assert_eq!(
        initial_uploads,
        BTreeMap::from([
            ("left".to_string(), left_first),
            ("right".to_string(), right_first),
        ])
    );
    assert_eq!(
        complete_uploads,
        BTreeMap::from([
            ("left".to_string(), left_body),
            ("right".to_string(), right_body),
        ])
    );

    let capable_input = b"staged-capable-input".to_vec();
    let capable: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/capable.bin", capable_input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&capable, &capable_input, 1, capable_input.len() as u64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable.bin")
            .await?,
        capable_input
    );

    let stdout_only_capable_input = b"stdout-only-capable".to_vec();
    let started_contracts: Vec<Vec<u8>> = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "started_invocation_contracts",
            data_value!(
                "/stdout-only-capable.bin",
                stdout_only_capable_input.clone()
            ),
        ),
    )
    .await
    .expect("started invocation contract checks timed out")?
    .into_typed()?;
    assert_eq!(
        started_contracts,
        [
            Vec::new(),
            vec![b'f', b'r', b'a', b'g', 0, 255],
            b"cached-result".to_vec(),
            stdout_only_capable_input.clone(),
        ]
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/stdout-only-capable.bin")
            .await?,
        stdout_only_capable_input
    );

    let raw_agent_id = agent_id!("ToolStreamingCaller", "rust-raw-modes");
    let raw_worker_id = executor
        .start_agent(&caller_component.id, raw_agent_id.clone())
        .await?;
    let raw_modes: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &raw_agent_id,
            "raw_modes_and_handles",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        raw_modes,
        ["invoke-and-await", "invoke", "async-invoke-and-await",]
    );

    let raw_handles: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &raw_agent_id,
            "raw_handle_lifecycles",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        raw_handles,
        [
            "out-of-order-get",
            "explicit-cancel",
            "result-detach",
            "stdout-detach",
            "stdout-operation-resume",
        ]
    );

    let observer_detach: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &raw_agent_id,
            "raw_observer_detach_and_fire_open",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        observer_detach,
        ["invoke-open-stdin", "invoke-and-await-observer-detach"]
    );
    executor.delete_worker(&raw_worker_id).await?;

    let stdout_drop: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "stdout_drop_preserves_sibling",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(stdout_drop, ["blocked-writer-woke", "sibling-completed"]);

    let edge_lifecycles: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "edge_lifecycles",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        edge_lifecycles,
        [
            "large-collect",
            "early-stdin-close",
            "early-stdout-close",
            "source-failure",
            "no-stream",
            "optional-streams",
            "pre-dispatch-rejection",
            "unused-stdout",
        ]
    );

    let nested: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("nested", b"nested-input".to_vec(), 2_u32),
        )
        .await?
        .into_typed()?;
    assert_eq!(nested.output, b"marker:nested-input");
    assert!(nested.chunks_read > 0);
    assert_eq!(nested.bytes_read, 19);
    assert!(!nested.output_closed);
    assert_eq!(nested.completion, "ok");

    let nested_capable_input = b"nested-capable-input".to_vec();
    let nested_capable_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!(
                "nested-capable-parent-end",
                nested_capable_input.clone(),
                2_u32
            ),
        ),
    )
    .await;
    let nested_capable: StreamEvidence = match nested_capable_result {
        Ok(result) => result?.into_typed()?,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!("nested capable parent-end call timed out; active metadata: {active:#?}")
        }
    };
    assert_evidence(&nested_capable, b"nested-capable-started", 0, 0);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/nested-capable-parent-end.bin")
            .await?,
        nested_capable_input
    );
    let nested_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entity_starts = nested_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some((entry.oplog_index, params.parent_start_index))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        entity_starts.iter().any(|(_, parent)| {
            parent.is_some_and(|parent| entity_starts.iter().any(|(start, _)| *start == parent))
        }),
        "a nested tool Start must retain another entity Start as its oplog parent"
    );

    let capable_modes: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "capable_modes_and_cohorts",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        capable_modes,
        [
            "synchronous",
            "reverse-synchronous",
            "reverse-result-cohort",
            "parent-end-async",
            "parent-end-no-body",
            "parent-end-fire-and-forget",
        ]
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-order.log")
            .await?,
        b"R1R2S1S2P1P2".as_slice()
    );
    for (path, expected) in [
        ("/capable-sync.bin", b"sync".as_slice()),
        ("/capable-r1.bin", b"reverse-first".as_slice()),
        ("/capable-r2.bin", b"reverse-second".as_slice()),
        ("/capable-s1.bin", b"first".as_slice()),
        ("/capable-s2.bin", b"second".as_slice()),
        ("/capable-parent-async.bin", b"parent-async".as_slice()),
        ("/capable-parent-fire.bin", b"parent-fire".as_slice()),
    ] {
        assert_eq!(
            executor.get_file_contents(&worker_id, path).await?,
            expected
        );
    }
    assert!(
        executor
            .get_file_contents(&worker_id, "/capable-parent-no-body.bin")
            .await
            .is_err(),
        "cancelled no-body parent cohort member must never execute"
    );

    let nested_lane_input = b"nested-lane-inheritance".to_vec();
    let nested_lane: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!(
                "nested-capable:/capable-nested-outer.bin",
                nested_lane_input.clone()
            ),
        )
        .await?
        .into_typed()?;
    assert_evidence(
        &nested_lane,
        &nested_lane_input,
        1,
        nested_lane_input.len() as u64,
    );
    for path in ["/capable-nested-inner.bin", "/capable-nested-outer.bin"] {
        assert_eq!(
            executor.get_file_contents(&worker_id, path).await?,
            nested_lane_input
        );
    }
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-order.log")
            .await?,
        b"R1R2S1S2P1P2NO".as_slice()
    );

    let trap_once_oplog_start = executor.oplog_max_index(&worker_id).await?;
    let trap_once_input = b"durable-capable-effect".to_vec();
    let trap_once: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("trap-once:/capable-trap-once.bin", trap_once_input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(
        &trap_once,
        &trap_once_input,
        1,
        trap_once_input.len() as u64,
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-trap-once.bin")
            .await?,
        trap_once_input
    );
    let trap_once_oplog = executor
        .get_oplog(&worker_id, trap_once_oplog_start)
        .await?;
    let trap_once_entity_starts = trap_once_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        trap_once_entity_starts.len(),
        1,
        "trap recovery must retain the original entity Start"
    );
    let durable_effects = trap_once_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::api::generate_idempotency-key"
                        && params.parent_start_index == Some(trap_once_entity_starts[0])
            )
        })
        .count();
    assert_eq!(
        durable_effects, 1,
        "trap recovery must consume the recorded nested durable effect exactly once"
    );

    for (name, method, calls) in [
        ("interrupt-stdin", "hold_open_stdin", 3_u32),
        ("interrupt-stdout", "hold_unread_stdout", 1_u32),
    ] {
        let held_agent_id = agent_id!("ToolStreamingCaller", name);
        let held_worker_id = executor
            .start_agent(&caller_component.id, held_agent_id.clone())
            .await?;
        let held_owned_agent_id =
            OwnedAgentId::new(context.default_environment_id, &held_worker_id);
        executor
            .invoke_agent(
                &caller_component,
                &held_agent_id,
                method,
                data_value!(calls),
            )
            .await?;
        wait_for_active_tool_operations(&executor, &held_owned_agent_id, calls as usize).await?;
        if method == "hold_unread_stdout" {
            let stdout = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    if let Some(stdout) = executor
                        .active_entity_metadata(&held_owned_agent_id)
                        .await
                        .and_then(|active| active.tool_operations.operations.into_iter().next())
                        .and_then(|operation| operation.stdout)
                        && stdout.buffered_bytes == 16 * 1024 * 1024
                    {
                        break stdout;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| {
                anyhow::anyhow!("await-result-before-read never reached bounded backpressure")
            })?;
            assert_eq!(stdout.accepted_bytes, 16 * 1024 * 1024);
            assert_eq!(stdout.delivered_bytes, 0);
            assert!(!stdout.terminal_selected);
        }
        executor.interrupt(&held_worker_id).await?;
        if let Some(active) = executor.active_entity_metadata(&held_owned_agent_id).await {
            assert!(active.tool_operations.operations.is_empty());
            assert!(active.lane.holder.is_none());
            assert_eq!(active.lane.active_invocation_count, 0);
            assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
        }
        executor.delete_worker(&held_worker_id).await?;
    }

    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }
    http_server.abort();
    trap_once_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn rust_tool_trap_retries_owner_without_replay_overflow(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 2,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "rust-trap");
    let result = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("trap", Vec::<u8>::new(), 1_u32),
        )
        .await;
    let error = result.expect_err("the tool trap must fail the owner invocation");
    assert!(
        error
            .to_string()
            .contains("deterministic streaming tool trap"),
        "the original tool trap must survive owner recovery: {error:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn guest_trap_fences_a_blocked_sibling_and_drains_the_owner_group(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 1,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "trap-with-blocked-sibling");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "trap_with_blocked_sibling",
            data_value!(),
        ),
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!(
                "guest trap did not drain its blocked sibling; active metadata: {active:#?}"
            );
        }
    };
    let error = result.expect_err("the exact guest trap must fail the owner invocation");
    assert!(
        error
            .to_string()
            .contains("deterministic streaming tool trap"),
        "the original guest-trap provenance must survive owner-group fencing: {error:?}"
    );

    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let invocation_start = oplog
        .iter()
        .rev()
        .find_map(|entry| {
            matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))
                .then_some(entry.oplog_index)
        })
        .expect("failed invocation has an AgentInvocationStarted entry");
    let entity_starts = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.parent_start_index == Some(invocation_start)
                        && params.function_name == "golem::entity::invoke"
            )
        })
        .count();
    assert_eq!(
        entity_starts, 2,
        "the trapped operation and its already-running blocked sibling must both be durable"
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                entry.oplog_index > invocation_start
                    && matches!(entry.entry, PublicOplogEntry::Error(_))
            })
            .count(),
        1,
        "the owner must classify the original trap exactly once"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn detached_and_fire_and_forget_traps_fail_the_owner_without_entity_terminals(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 1,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    for (name, method) in [
        ("trap-with-waiter", "collect"),
        ("trap-after-result-drop", "drop_trapping_result"),
        ("trap-fire-and-forget", "fire_and_forget_trap"),
    ] {
        let agent_id = agent_id!("ToolStreamingCaller", name);
        let worker_id = executor
            .start_agent(&caller_component.id, agent_id.clone())
            .await?;
        let result = if method == "collect" {
            executor
                .invoke_and_await_agent(
                    &caller_component,
                    &agent_id,
                    method,
                    data_value!("trap", Vec::<u8>::new(), 1_u32),
                )
                .await
        } else {
            executor
                .invoke_and_await_agent(&caller_component, &agent_id, method, data_value!())
                .await
        };
        let error = result.expect_err("a tool trap must fail its launching owner invocation");
        assert!(
            error
                .to_string()
                .contains("deterministic streaming tool trap"),
            "the original classified trap must reach the owner boundary for {method}: {error:?}"
        );

        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let invocation_start = oplog
            .iter()
            .rev()
            .find_map(|entry| {
                matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))
                    .then_some(entry.oplog_index)
            })
            .expect("failed invocation has an AgentInvocationStarted entry");
        assert!(
            oplog.iter().all(|entry| {
                entry.oplog_index <= invocation_start
                    || !matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
            }),
            "{method} must not commit AgentInvocationFinished after a detached tool trap"
        );
        let entity_starts = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.parent_start_index == Some(invocation_start)
                        && params.function_name == "golem::entity::invoke" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            entity_starts.len(),
            1,
            "{method} must retain exactly one accepted entity Start"
        );
        assert!(
            oplog.iter().all(|entry| {
                !matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params)
                        if entity_starts.contains(&params.start_index)
                )
            }),
            "{method} must not append an ordinary entity terminal after a tool trap"
        );
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| {
                    entry.oplog_index > invocation_start
                        && matches!(entry.entry, PublicOplogEntry::Error(_))
                })
                .count(),
            1,
            "{method} must classify and record the original owner trap exactly once"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn capable_streams_enforce_completion_limits_without_leaks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 64;
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "capable-limits");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let exact_input = vec![b'i'; 64];
    let exact: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("stdout-exact:/capable-exact.bin", exact_input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&exact, &[b'x'; 64], 1, 64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-exact.bin")
            .await?,
        exact_input
    );

    let stdout_over: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("stdout-over:/capable-stdout-over.bin", vec![b'o']),
        )
        .await?
        .into_typed()?;
    assert!(
        stdout_over.completion.contains("ResourceExhausted"),
        "stdout overflow must stay operation-fatal even when the provider catches the write error: {stdout_over:#?}"
    );

    let stdin_over: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/capable-stdin-over.bin", vec![b'i'; 65]),
        )
        .await?
        .into_typed()?;
    assert!(
        stdin_over.completion.contains("ResourceExhausted"),
        "stdin overflow must settle without launching a sidecar: {stdin_over:#?}"
    );

    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let overflow_agent = agent_id!("ToolStreamingCaller", "capable-overflow-replay");
    let overflow_worker = executor
        .start_agent_with(
            &caller_component.id,
            overflow_agent.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let overflow_owner = OwnedAgentId::new(context.default_environment_id, &overflow_worker);
    executor
        .invoke_agent(
            &caller_component,
            &overflow_agent,
            "hold_capable_overflow_terminal",
            data_value!(vec![b'i'; 65]),
        )
        .await?;
    let original_gate = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("original capable overflow terminal gate timed out")
    .expect("checkpoint server stopped");
    assert_eq!(original_gate.name, "capable-stdin-overflow-terminal");
    let original_start = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(active) = executor.active_entity_metadata(&overflow_owner).await
                && active.reached_oplog_marker.is_some()
                && active.tool_operations.operations.is_empty()
                && matches!(active.lane.holder, Some(OwnerInvocationId::Agent(_)))
                && active.lane.active_invocation_count == 1
                && active.slots.iter().all(|slot| slot.invocations.is_empty())
            {
                let oplog = executor
                    .get_oplog(&overflow_worker, OplogIndex::INITIAL)
                    .await
                    .expect("read capable overflow oplog");
                if let Some(start) = oplog.iter().find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke" =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                }) {
                    break start;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capable overflow did not settle without a sidecar");

    executor.simulated_crash(&overflow_worker).await?;
    drop(original_gate.release);
    let replayed_gate = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("replayed capable overflow terminal gate timed out")
    .expect("checkpoint server stopped");
    assert_eq!(replayed_gate.name, "capable-stdin-overflow-terminal");
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(active) = executor.active_entity_metadata(&overflow_owner).await
                && active.reached_oplog_marker.is_some()
                && active.tool_operations.operations.is_empty()
                && matches!(active.lane.holder, Some(OwnerInvocationId::Agent(_)))
                && active.lane.active_invocation_count == 1
                && active.slots.iter().all(|slot| slot.invocations.is_empty())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replayed capable overflow created a body or lane registration");
    let overflow_oplog = executor
        .get_oplog(&overflow_worker, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        overflow_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke"
                )
            })
            .count(),
        1,
        "capable overflow replay must reuse its one no-body Start"
    );
    assert_eq!(
        overflow_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == original_start
                )
            })
            .count(),
        1,
        "capable overflow replay must retain exactly one no-body terminal"
    );
    replayed_gate
        .release
        .send(())
        .expect("release replayed capable overflow gate");
    assert!(
        executor
            .get_file_contents(&overflow_worker, "/must-not-run-after-overflow.bin")
            .await
            .is_err(),
        "replayed capable stdin overflow must not run a body"
    );
    checkpoint_server.abort();

    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn completed_tool_replay_bypasses_current_attachment_memory_pressure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("large_dynamic_memory")] large_dynamic_memory: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const SYSTEM_MEMORY_BYTES: u64 = 128 * 1024 * 1024;
    const ATTACHMENT_BYTES: usize = 2 * 1024 * 1024;
    const DESIRED_REPLAY_HEADROOM_BYTES: u64 = 1024 * 1024;

    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 2 * ATTACHMENT_BYTES;
                config.memory.system_memory_override = Some(SYSTEM_MEMORY_BYTES);
                config.memory.worker_memory_ratio = 1.0;
                config.memory.component_size_coefficient = 0.0;
                config.memory.acquire_retry_delay = std::time::Duration::from_millis(25);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let memory_component = executor
        .component_dep(&context.default_environment_id, large_dynamic_memory)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "completed-replay-memory-pressure");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let input = vec![b'i'; ATTACHMENT_BYTES];
    executor
        .invoke_agent(
            &caller_component,
            &agent_id,
            "hold_completed_attachment_reconstruction_under_pressure",
            data_value!("/completed-under-pressure.bin", ATTACHMENT_BYTES as u64),
        )
        .await?;
    let original_checkpoint = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("original completed-attachment checkpoint timed out")
    .expect("checkpoint server stopped");
    assert_eq!(original_checkpoint.name, "completed-attachment-pressure");
    let target_memory = executor.worker_memory_requirement(&owned_agent_id).await?;

    let pressure_agent = agent_id!("LargeDynamicMemoryAgent", "attachment-replay-pressure");
    let pressure_worker = executor
        .start_agent(&memory_component.id, pressure_agent.clone())
        .await?;
    let pressure_owned = OwnedAgentId::new(context.default_environment_id, &pressure_worker);
    let initial_pressure_memory = executor.worker_memory_requirement(&pressure_owned).await?;
    let growth_budget = SYSTEM_MEMORY_BYTES
        .checked_sub(target_memory + initial_pressure_memory + DESIRED_REPLAY_HEADROOM_BYTES)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "initial workers leave no room for calibrated pressure: target={target_memory}, pressure={initial_pressure_memory}"
            )
        })?;
    let growth_mib = growth_budget / (1024 * 1024);
    anyhow::ensure!(growth_mib > 0, "calibrated pressure growth is empty");
    executor
        .invoke_agent(
            &memory_component,
            &pressure_agent,
            "run_with_memory_and_work",
            data_value!(growth_mib, 120_000_u64),
        )
        .await?;
    let pressure_memory = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let memory = executor
                .worker_memory_requirement(&pressure_owned)
                .await
                .expect("read pressure worker memory");
            if memory + target_memory + ATTACHMENT_BYTES as u64 > SYSTEM_MEMORY_BYTES {
                break memory;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pressure worker did not consume attachment headroom");
    assert!(
        pressure_memory + target_memory <= SYSTEM_MEMORY_BYTES,
        "pressure must still leave room to restart the owner: pressure={pressure_memory}, target={target_memory}, pool={SYSTEM_MEMORY_BYTES}"
    );

    let mut reconstruction = executor.gate_next_completed_entity_reconstruction(&worker_id);
    executor.simulated_crash(&worker_id).await?;
    drop(original_checkpoint.release);
    tokio::time::timeout(std::time::Duration::from_secs(30), reconstruction.entered())
        .await
        .expect("completed tool body did not reexecute under attachment memory pressure");
    assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
    reconstruction.release();

    let replayed_checkpoint = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("replayed completed-attachment checkpoint timed out")
    .expect("checkpoint server stopped");
    assert_eq!(replayed_checkpoint.name, "completed-attachment-pressure");
    replayed_checkpoint
        .release
        .send(())
        .expect("release replayed completed-attachment checkpoint");
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(&worker_id).await?;
            if metadata.status == AgentStatus::Idle && metadata.pending_invocation_count == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("replayed owner invocation did not settle"))??;
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/completed-under-pressure.bin")
            .await?,
        input,
        "completed replay must preserve the body-reconstructed filesystem state"
    );
    checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn incomplete_tool_replay_persists_attachment_upgrade_rejection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("large_dynamic_memory")] large_dynamic_memory: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const SYSTEM_MEMORY_BYTES: u64 = 128 * 1024 * 1024;
    const ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;
    const DESIRED_REPLAY_HEADROOM_BYTES: u64 = 4 * 1024 * 1024;

    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 2 * ATTACHMENT_BYTES as usize;
                config.memory.system_memory_override = Some(SYSTEM_MEMORY_BYTES);
                config.memory.worker_memory_ratio = 1.0;
                config.memory.component_size_coefficient = 0.0;
                config.memory.acquire_retry_delay = std::time::Duration::from_millis(25);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let memory_component = executor
        .component_dep(&context.default_environment_id, large_dynamic_memory)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "incomplete-upgrade-rejection");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "reject_incomplete_attachment_upgrade_under_pressure",
        data_value!(),
    );
    let prepare_replay = async {
        let original_checkpoint = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("original incomplete-attachment checkpoint timed out")
        .expect("checkpoint server stopped");
        assert_eq!(original_checkpoint.name, "after-large-eof-before-terminal");
        let operation = executor
            .active_entity_metadata(&owned_agent_id)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("incomplete tool operation is active at the crash checkpoint");
        let original_start = operation.start_index.expect("durable tool Start index");
        let stdout = operation.stdout.expect("incomplete tool stdout metadata");
        assert_eq!(stdout.buffered_bytes as u64, ATTACHMENT_BYTES);
        assert_eq!(stdout.delivered_bytes, 0);
        let reconstructed_entity_memory = executor
            .active_entity_metadata(&owned_agent_id)
            .await
            .expect("incomplete tool entity remains active at the crash checkpoint")
            .slots
            .into_iter()
            .flat_map(|slot| slot.invocations)
            .map(|invocation| invocation.linear_memory_bytes)
            .sum::<u64>();
        anyhow::ensure!(
            reconstructed_entity_memory > 0,
            "incomplete tool entity has no reconstructed linear-memory charge"
        );

        let mut reconstructed_body = executor.gate_next_entity_body_start(&worker_id);
        executor.simulated_crash(&worker_id).await?;
        drop(original_checkpoint.release);
        executor.resume(&worker_id, true).await?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstructed_body.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("reconstructed entity body start gate was not reached"))?;

        let target_memory = executor.worker_memory_requirement(&owned_agent_id).await?;
        let restart_memory = target_memory
            .checked_add(reconstructed_entity_memory)
            .ok_or_else(|| anyhow::anyhow!("reconstructed memory requirement overflow"))?;
        let pressure_agent = agent_id!("LargeDynamicMemoryAgent", "incomplete-attachment-pressure");
        let pressure_worker = executor
            .start_agent(&memory_component.id, pressure_agent.clone())
            .await?;
        let pressure_owned = OwnedAgentId::new(context.default_environment_id, &pressure_worker);
        let initial_pressure_memory = executor.worker_memory_requirement(&pressure_owned).await?;
        let growth_budget = SYSTEM_MEMORY_BYTES
            .checked_sub(restart_memory + initial_pressure_memory + DESIRED_REPLAY_HEADROOM_BYTES)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "initial workers leave no room for calibrated pressure: owner={target_memory}, entity={reconstructed_entity_memory}, pressure={initial_pressure_memory}"
                )
            })?;
        let growth_mib = growth_budget / (1024 * 1024);
        anyhow::ensure!(growth_mib > 0, "calibrated pressure growth is empty");
        executor
            .invoke_agent(
                &memory_component,
                &pressure_agent,
                "run_with_memory_and_work",
                data_value!(growth_mib, 120_000_u64),
            )
            .await?;
        let pressure_memory = {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut maximum_observed = 0;
            loop {
                let memory = executor
                    .worker_memory_requirement(&pressure_owned)
                    .await
                    .expect("read pressure worker memory");
                maximum_observed = maximum_observed.max(memory);
                if memory + restart_memory + ATTACHMENT_BYTES > SYSTEM_MEMORY_BYTES {
                    break memory;
                }
                anyhow::ensure!(
                    tokio::time::Instant::now() < deadline,
                    "pressure worker did not consume attachment-upgrade headroom: owner={target_memory}, entity={reconstructed_entity_memory}, initial_pressure={initial_pressure_memory}, requested_growth_mib={growth_mib}, maximum_pressure={maximum_observed}, attachment={ATTACHMENT_BYTES}, pool={SYSTEM_MEMORY_BYTES}"
                );
                tokio::task::yield_now().await;
            }
        };
        assert!(
            pressure_memory + restart_memory <= SYSTEM_MEMORY_BYTES,
            "pressure must fit beside the reconstructed owner and entity: pressure={pressure_memory}, owner={target_memory}, entity={reconstructed_entity_memory}, pool={SYSTEM_MEMORY_BYTES}"
        );

        reconstructed_body.release();
        Ok::<_, anyhow::Error>(original_start)
    };
    let (result, original_start) = tokio::join!(invocation, prepare_replay);
    let evidence = result?.into_typed::<Vec<String>>()?;
    let original_start = original_start?;
    assert_eq!(
        evidence,
        vec!["resource-exhausted", "stdout-resource-exhausted"]
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/incomplete-attachment-upgrade-rejected")
            .await?
            .as_ref(),
        b"durable".as_slice()
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "attachment admission rejection must be an ordinary durable tool outcome"
    );
    let (terminal_index, terminal_response) = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == original_start => params
                .response
                .clone()
                .map(|response| (entry.oplog_index, response)),
            _ => None,
        })
        .expect("incomplete attachment upgrade rejection has one durable terminal");
    let (jump_index, jump) = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Jump(params) if entry.oplog_index < terminal_index => {
                Some((entry.oplog_index, params.jump.clone()))
            }
            _ => None,
        })
        .expect("incomplete atomic tail is jumped before the outer tool terminal");
    assert_eq!(
        jump.end, jump_index,
        "the recovery Jump must delete itself with the abandoned atomic tail"
    );
    assert!(
        jump.start > original_start,
        "the recovery Jump must preserve the outer tool Start"
    );
    let terminal = SerializableToolOperationTerminal::from_value(terminal_response.value())?;
    assert_eq!(
        terminal.body_execution,
        SerializableEntityBodyExecution::Skipped
    );
    assert!(matches!(
        terminal.result,
        Err(SerializableToolRpcError::ResourceExhausted(_))
    ));
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke"
                )
            })
            .count(),
        1
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == original_start
                )
            })
            .count(),
        1
    );
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            checkpoint_arrivals.recv()
        )
        .await
        .is_err(),
        "rejected live repair must not reach the provider's live checkpoint"
    );
    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(&worker_id).await?;
            if metadata.status == AgentStatus::Idle && metadata.pending_invocation_count == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("stable rejection replay did not settle"))??;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            checkpoint_arrivals.recv()
        )
        .await
        .is_err(),
        "recorded skipped rejection must not reexecute the provider body"
    );
    let replayed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        replayed_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == original_start
                )
            })
            .count(),
        1,
        "replay must retain the original attachment rejection terminal"
    );
    checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn capable_stdout_poll_does_not_launch_before_result_await(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "capable-stdout-before-result");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    executor
        .invoke_agent(
            &caller_component,
            &agent_id,
            "hold_capable_stdout_before_result",
            data_value!("/must-not-start-before-result.bin", b"staged".to_vec()),
        )
        .await?;

    let operation = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(operation) = executor
                .active_entity_metadata(&owned_agent_id)
                .await
                .and_then(|active| active.tool_operations.operations.into_iter().next())
                && operation.admission == ToolBodyAdmissionMetadata::Ready
            {
                break operation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("capable operation did not finish eager input staging"))?;
    assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
    assert_eq!(operation.attachment_count, 2);

    executor.interrupt(&worker_id).await?;
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn capable_admission_publication_and_clean_stdout_trap_preserve_boundaries(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 0,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let env = HashMap::from([
        (
            "CRASH_CHECKPOINT_PORT".to_string(),
            checkpoint_port.to_string(),
        ),
        (
            "CRASH_CHECKPOINT_GATE_PORT".to_string(),
            checkpoint_gate_port.to_string(),
        ),
    ]);

    let sync_agent = agent_id!("ToolStreamingCaller", "sync-capable-staging-boundary");
    let sync_worker = executor
        .start_agent_with(
            &caller_component.id,
            sync_agent.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let sync_owner = OwnedAgentId::new(context.default_environment_id, &sync_worker);
    let sync_input = b"sync-staged".to_vec();
    let sync_call = executor.invoke_and_await_agent(
        &caller_component,
        &sync_agent,
        "hold_synchronous_capable_staging",
        data_value!(sync_input.clone()),
    );
    let sync_gates = async {
        let staging_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("synchronous capable staging gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(staging_gate.name, "sync-capable-staging");
        let operation = executor
            .active_entity_metadata(&sync_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("synchronous capable operation is eagerly staged");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Staging);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
        let stdin = operation.stdin.expect("synchronous capable stdin metadata");
        assert_eq!(stdin.accepted_bytes, sync_input.len() as u64);
        assert!(!stdin.terminal_selected);
        staging_gate
            .release
            .send(())
            .expect("release synchronous capable staging gate");

        let body_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("synchronous capable body gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(body_gate.name, "capable-body");
        let operation = executor
            .active_entity_metadata(&sync_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("synchronous capable body is active");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Running);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::Granted);
        let stdout = operation
            .stdout
            .expect("synchronous capable stdout metadata");
        assert_eq!(stdout.mode, ToolAttachmentModeMetadata::CompletionStaged);
        assert_eq!(stdout.delivered_bytes, 0);
        body_gate
            .release
            .send(())
            .expect("release synchronous capable body gate");
    };
    let (sync_result, ()) = tokio::join!(sync_call, sync_gates);
    sync_result?;
    assert_eq!(
        executor
            .get_file_contents(&sync_worker, "/sync-capable-staging.bin")
            .await?,
        sync_input
    );

    let fire_agent = agent_id!("ToolStreamingCaller", "fire-capable-staging-boundary");
    let fire_worker = executor
        .start_agent_with(
            &caller_component.id,
            fire_agent.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let fire_owner = OwnedAgentId::new(context.default_environment_id, &fire_worker);
    let fire_input = b"fire-staged".to_vec();
    let fire_call = executor.invoke_and_await_agent(
        &caller_component,
        &fire_agent,
        "hold_fire_and_forget_capable_staging",
        data_value!(fire_input.clone()),
    );
    let fire_gates = async {
        let staging_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("fire-and-forget capable staging gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(staging_gate.name, "fire-capable-staging");
        let operation = executor
            .active_entity_metadata(&fire_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("fire-and-forget capable operation is eagerly staged");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Staging);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
        staging_gate
            .release
            .send(())
            .expect("release fire-and-forget staging gate");

        let parent_open_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("fire-and-forget parent-open gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(parent_open_gate.name, "fire-capable-ready-parent-open");
        let operation = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Some(operation) = executor
                    .active_entity_metadata(&fire_owner)
                    .await
                    .and_then(|active| active.tool_operations.operations.into_iter().next())
                    && operation.admission == ToolBodyAdmissionMetadata::Ready
                {
                    break operation;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect(
            "fire-and-forget capable operation did not become ready while parent remained open",
        );
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Ready);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
        parent_open_gate
            .release
            .send(())
            .expect("release fire-and-forget parent-open gate");

        let body_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("fire-and-forget capable body gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(body_gate.name, "capable-body");
        let operation = executor
            .active_entity_metadata(&fire_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("fire-and-forget capable body starts after parent end");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Running);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::Granted);
        body_gate
            .release
            .send(())
            .expect("release fire-and-forget capable body gate");
    };
    let (fire_result, ()) = tokio::join!(fire_call, fire_gates);
    fire_result?;
    assert_eq!(
        executor
            .get_file_contents(&fire_worker, "/fire-capable-staging.bin")
            .await?,
        fire_input
    );

    let publication_agent = agent_id!("ToolStreamingCaller", "capable-publication-boundary");
    let publication_worker = executor
        .start_agent_with(
            &caller_component.id,
            publication_agent.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let publication_owner = OwnedAgentId::new(context.default_environment_id, &publication_worker);
    let publication_input = b"publication-staged".to_vec();
    let publication_call = executor.invoke_and_await_agent(
        &caller_component,
        &publication_agent,
        "hold_capable_publication_checkpoint",
        data_value!("/capable-publication.bin", publication_input.clone()),
    );
    let publication_gates = async {
        let provider_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("capable provider publication gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(provider_gate.name, "provider-capable-before-publication");

        let operation = executor
            .active_entity_metadata(&publication_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("capable operation is active before publication");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Running);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::Granted);
        let stdout = operation.stdout.expect("capable stdout metadata");
        assert_eq!(stdout.mode, ToolAttachmentModeMetadata::CompletionStaged);
        assert_eq!(stdout.accepted_bytes, publication_input.len() as u64);
        assert_eq!(stdout.delivered_bytes, 0);
        assert_eq!(stdout.buffered_bytes, publication_input.len());
        assert!(!stdout.terminal_selected);
        assert!(
            checkpoint_arrivals.try_recv().is_err(),
            "caller must not observe capable stdout before lane return"
        );

        provider_gate
            .release
            .send(())
            .expect("release capable provider publication gate");
        let caller_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("caller publication observation gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(caller_gate.name, "caller-observed-capable-publication");
        let active = executor
            .active_entity_metadata(&publication_owner)
            .await
            .expect("owner remains active while caller holds result observation");
        assert!(matches!(
            active.lane.holder,
            Some(OwnerInvocationId::Agent(_))
        ));
        assert_eq!(active.lane.active_invocation_count, 1);
        caller_gate
            .release
            .send(())
            .expect("release caller publication gate");
    };
    let (publication_result, ()) = tokio::join!(publication_call, publication_gates);
    publication_result?;
    assert_eq!(
        executor
            .get_file_contents(&publication_worker, "/capable-publication.bin")
            .await?,
        publication_input
    );
    assert_eq!(
        executor
            .get_file_contents(&publication_worker, "/capable-publication-observed.bin")
            .await?,
        b"observed".as_slice()
    );

    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let trap_env = HashMap::from([
        (
            "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
            provider_checkpoint_port.to_string(),
        ),
        (
            "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
            provider_checkpoint_gate_port.to_string(),
        ),
        (
            "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
            caller_checkpoint_port.to_string(),
        ),
        (
            "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
            caller_checkpoint_gate_port.to_string(),
        ),
    ]);
    let trap_agent = agent_id!("ToolStreamingCaller", "clean-stdout-before-trap");
    let trap_worker = executor
        .start_agent_with(
            &caller_component.id,
            trap_agent.clone(),
            trap_env,
            Vec::new(),
        )
        .await?;
    let trap_owner = OwnedAgentId::new(context.default_environment_id, &trap_worker);
    let trap_call = executor.invoke_and_await_agent(
        &caller_component,
        &trap_agent,
        "clean_stdout_then_trap",
        data_value!(),
    );
    let trap_gates = async {
        let provider_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            provider_checkpoint_arrivals.recv(),
        )
        .await
        .expect("provider clean-stdout gate timed out")
        .expect("provider checkpoint server stopped");
        assert_eq!(provider_gate.name, "provider-clean-stdout-before-trap");
        let caller_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoint_arrivals.recv(),
        )
        .await
        .expect("caller clean-stdout gate timed out")
        .expect("caller checkpoint server stopped");
        assert_eq!(caller_gate.name, "caller-observed-clean-stdout");

        let operation = executor
            .active_entity_metadata(&trap_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("trapping operation remains active at clean stdout EOF");
        let stdout = operation.stdout.expect("trapping stdout metadata");
        assert_eq!(stdout.mode, ToolAttachmentModeMetadata::Live);
        assert!(stdout.terminal_selected);
        assert_eq!(stdout.accepted_bytes, b"marker:".len() as u64);
        assert_eq!(stdout.delivered_bytes, b"marker:".len() as u64);

        caller_gate
            .release
            .send(())
            .expect("release caller clean-stdout gate");
        provider_gate
            .release
            .send(())
            .expect("release provider clean-stdout gate");
    };
    let (trap_result, ()) = tokio::join!(trap_call, trap_gates);
    let trap_error = trap_result.expect_err("non-retryable provider trap must fail the owner");
    assert!(
        trap_error
            .to_string()
            .contains("deterministic streaming tool trap after clean stdout"),
        "original non-retryable trap must reach the owner: {trap_error:?}"
    );
    let trap_oplog = executor
        .get_oplog(&trap_worker, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        trap_oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
            .count(),
        1,
        "non-retryable trap is recorded exactly once"
    );
    if let Some(active) = executor.active_entity_metadata(&trap_owner).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    checkpoint_server.abort();
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn active_stream_crash_replays_pinned_activation_with_fresh_attachments(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "active-stream-crash");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    executor
        .invoke_agent(
            &caller_component,
            &agent_id,
            "hold_open_stdin",
            data_value!(1_u32),
        )
        .await?;
    let original = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(operation) = executor
                .active_entity_metadata(&owned_agent_id)
                .await
                .and_then(|active| active.tool_operations.operations.into_iter().next())
                && operation.start_index.is_some()
                && operation.attachment_count == 2
            {
                break operation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for the durable tool Start"))?;
    let original_start = original.start_index.expect("durable tool Start index");
    assert_eq!(original.attachment_count, 2);

    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        None,
    );
    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    let replayed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(operation) = executor
                .active_entity_metadata(&owned_agent_id)
                .await
                .and_then(|active| active.tool_operations.operations.into_iter().next())
                && operation.start_index.is_some()
                && operation.attachment_count == 2
            {
                break operation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for the replayed durable tool Start"))?;
    assert_eq!(replayed.start_index, Some(original_start));
    assert_eq!(replayed.attachment_count, 2);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "lifecycle recovery must not poison the incomplete entity Start with a durable Error"
    );
    let serialized = serde_json::to_string(&oplog)?;
    for forbidden in ["attachment-id", "endpoint-id", "resource-key"] {
        assert!(
            !serialized.contains(forbidden),
            "transient stream identity `{forbidden}` must not enter the owner oplog"
        );
    }

    executor.interrupt(&worker_id).await?;
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletedReconstructionExclusiveCase {
    Success,
    Divergence,
    BackpressuredStdin,
}

async fn run_completed_reconstruction_exclusive_p2_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    case: CompletedReconstructionExclusiveCase,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 64;
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    let deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        metadata.tools,
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment.clone()),
    );

    let case_name = match case {
        CompletedReconstructionExclusiveCase::Success => "exclusive-p2-success",
        CompletedReconstructionExclusiveCase::Divergence => "exclusive-p2-divergence",
        CompletedReconstructionExclusiveCase::BackpressuredStdin => {
            "exclusive-p2-backpressured-stdin"
        }
    };
    let agent_id = agent_id!("ToolStreamingCaller", case_name);
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(&worker_id).await?;
            if metadata.status == AgentStatus::Idle
                && metadata.pending_invocation_count == 0
                && metadata.last_oplog_index > OplogIndex::INITIAL
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for caller initialization"))??;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let first = vec![0x31u8; 64];
    let second = vec![0x32u8; 64];
    let mut original_start = (case == CompletedReconstructionExclusiveCase::BackpressuredStdin)
        .then(|| executor.gate_next_entity_body_start(&worker_id));
    let mut original_success = executor.gate_next_agent_invocation_success(&worker_id);
    match case {
        CompletedReconstructionExclusiveCase::Success
        | CompletedReconstructionExclusiveCase::Divergence => {
            executor
                .skip_next_wall_clock_now_durability(&owned_agent_id)
                .await?;
        }
        CompletedReconstructionExclusiveCase::BackpressuredStdin => {
            executor
                .skip_next_monotonic_clock_now_durability(&owned_agent_id)
                .await?;
        }
    }
    let invocation = match case {
        CompletedReconstructionExclusiveCase::Success
        | CompletedReconstructionExclusiveCase::Divergence => executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "hold_completed_reconstruction_before_exclusive_clock",
            data_value!(),
        ),
        CompletedReconstructionExclusiveCase::BackpressuredStdin => executor
            .invoke_and_await_agent(
                &caller_component,
                &agent_id,
                "hold_reconstruction_backpressure_before_exclusive_clock",
                data_value!(first.clone(), second.clone()),
            ),
    };
    tokio::pin!(invocation);

    if let Some(start) = original_start.as_mut() {
        tokio::select! {
            () = start.entered() => {}
            result = &mut invocation => {
                result.context("original invocation finished before reaching the entity body start gate")?;
                anyhow::bail!("original invocation succeeded without reaching the entity body start gate");
            }
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                anyhow::bail!("original entity body start gate was not reached");
            }
        }
        wait_for_tool_stdin_state(
            &executor,
            &owned_agent_id,
            first.len() as u64,
            0,
            first.len(),
            first.len(),
            true,
            None,
            true,
            true,
            true,
        )
        .await?;
        start.release();
    }

    let validate_recovery = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            original_success.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("original agent invocation did not reach success gate"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        let reconstruction_start =
            wait_for_completed_entity_terminal(&executor, &worker_id).await?;
        if case == CompletedReconstructionExclusiveCase::Divergence {
            let updated_component = executor
                .update_component(&caller_component.id, caller.wasm_name.as_str())
                .await?;
            environment_state.set_tool_deployment(
                context.default_environment_id,
                caller_component.id,
                updated_component.revision,
                Some(deployment.clone()),
            );
            executor
                .auto_update_worker(&worker_id, updated_component.revision, true)
                .await?;
        }

        let mut reconstruction_body =
            executor.gate_next_completed_entity_reconstruction(&worker_id);
        if case == CompletedReconstructionExclusiveCase::Divergence {
            executor.diverge_next_completed_entity_reconstruction(&worker_id);
        }
        let mut replayed_start = (case == CompletedReconstructionExclusiveCase::BackpressuredStdin)
            .then(|| executor.gate_next_entity_body_start(&worker_id));
        let mut replayed_claim = executor.gate_next_entity_reconstruction_claim(&worker_id);
        executor.simulated_crash(&worker_id).await?;
        drop(original_start);
        original_success.abort_as_restart();
        drop(original_success);
        let claimed_start =
            tokio::time::timeout(std::time::Duration::from_secs(30), replayed_claim.entered())
                .await
                .map_err(|_| anyhow::anyhow!("replayed reconstruction claim was not reached"))?;
        assert_eq!(claimed_start, reconstruction_start);
        let mut replayed_clock = match case {
            CompletedReconstructionExclusiveCase::Success
            | CompletedReconstructionExclusiveCase::Divergence => {
                executor.gate_next_wall_clock_now(&owned_agent_id).await?
            }
            CompletedReconstructionExclusiveCase::BackpressuredStdin => {
                executor
                    .gate_next_monotonic_clock_now(&owned_agent_id)
                    .await?
            }
        };
        replayed_claim.release();
        tokio::time::timeout(std::time::Duration::from_secs(30), replayed_clock.entered())
            .await
            .map_err(|_| anyhow::anyhow!("replayed clock gate was not reached"))?;
        let mut replayed_success = (case != CompletedReconstructionExclusiveCase::Divergence)
            .then(|| executor.gate_next_agent_invocation_success(&worker_id));

        match case {
            CompletedReconstructionExclusiveCase::Success => {
                let replayed_success = replayed_success.as_mut().unwrap();
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    reconstruction_body.entered(),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("completed reconstruction did not reach body validation gate")
                })?;
                replayed_clock.release();
                wait_for_owner_replay_settling(&executor, &owned_agent_id).await?;
                assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(250),
                        replayed_success.entered()
                    )
                    .await
                    .is_err(),
                    "exclusive P2 clock call finished before completed reconstruction validation"
                );
                reconstruction_body.release();
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    replayed_success.entered(),
                )
                .await
                .map_err(|_| anyhow::anyhow!("replayed agent invocation did not finish"))?;
                replayed_success.release();
            }
            CompletedReconstructionExclusiveCase::Divergence => {
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    reconstruction_body.entered(),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("divergent reconstruction did not reach body validation gate")
                })?;
                replayed_clock.release();
                wait_for_owner_replay_settling(&executor, &owned_agent_id).await?;
                assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
                let settling_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                assert!(
                    settling_oplog
                        .iter()
                        .all(|entry| !matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_))),
                    "ReplayFinished finalized the pending update before reconstruction validation"
                );
                let owner_failure = executor.wait_for_tool_owner_failure(&owned_agent_id);
                tokio::pin!(owner_failure);
                assert!(matches!(
                    futures::poll!(owner_failure.as_mut()),
                    std::task::Poll::Pending
                ));
                reconstruction_body.release();
                let owner_failure =
                    tokio::time::timeout(std::time::Duration::from_secs(30), owner_failure)
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!("divergence did not select an owner failure")
                        })??;
                assert!(owner_failure.owner_failure_selected);
                assert_eq!(
                    owner_failure.owner_failure,
                    Some(ToolOwnerFailureMetadata::Infrastructure)
                );
                let failed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                assert!(
                    failed_oplog
                        .iter()
                        .all(|entry| !matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_))),
                    "divergent reconstruction permitted ReplayFinished update finalization"
                );
            }
            CompletedReconstructionExclusiveCase::BackpressuredStdin => {
                let replayed_success = replayed_success.as_mut().unwrap();
                let replayed_start = replayed_start.as_mut().unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(30), replayed_start.entered())
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!("replayed entity body start gate was not reached")
                    })?;
                wait_for_tool_stdin_state(
                    &executor,
                    &owned_agent_id,
                    first.len() as u64,
                    0,
                    first.len(),
                    first.len(),
                    true,
                    None,
                    true,
                    true,
                    true,
                )
                .await?;
                replayed_start.release();
                wait_for_tool_stdin_state(
                    &executor,
                    &owned_agent_id,
                    (first.len() + second.len()) as u64,
                    (first.len() + second.len()) as u64,
                    0,
                    first.len(),
                    false,
                    Some(ToolAttachmentTerminalMetadata::ConsumerCancelled),
                    false,
                    true,
                    false,
                )
                .await?;
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    reconstruction_body.entered(),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("backpressured reconstruction did not reach body validation")
                })?;
                replayed_clock.release();
                wait_for_owner_replay_settling(&executor, &owned_agent_id).await?;
                assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(250),
                        replayed_success.entered()
                    )
                    .await
                    .is_err(),
                    "Store pumping published live before completed body validation"
                );
                reconstruction_body.release();
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    replayed_success.entered(),
                )
                .await
                .map_err(|_| anyhow::anyhow!("replayed agent invocation did not finish"))?;
                replayed_success.release();
            }
        }
        Ok::<_, anyhow::Error>(())
    };

    let (invocation_result, validation_result) = tokio::join!(
        tokio::time::timeout(std::time::Duration::from_secs(60), &mut invocation),
        validate_recovery
    );
    validation_result?;
    let invocation_result = invocation_result
        .map_err(|_| anyhow::anyhow!("exclusive-P2 reconstruction invocation timed out"))?;
    if case == CompletedReconstructionExclusiveCase::Divergence {
        assert!(
            invocation_result.is_err(),
            "divergent reconstruction must fail the owner invocation"
        );
    } else {
        invocation_result?;
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn completed_reconstruction_settles_while_exclusive_p2_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::Success,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn completed_reconstruction_divergence_fails_exclusive_p2_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::Divergence,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn settling_accessor_p2_keeps_backpressured_reconstruction_store_polling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::BackpressuredStdin,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn completed_reconstruction_claim_blocks_concurrent_replay_to_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "completed-reconstruction-barrier");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "hold_completed_reconstruction_barrier",
        data_value!(),
    );
    let crash_and_validate = async {
        let original_body =
            next_crash_checkpoint(&mut provider_checkpoints, "historical-reconstruction-body")
                .await?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 1).await?;
        original_body
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original reconstruction body gate was dropped"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        let original_live =
            next_crash_checkpoint(&mut caller_checkpoints, "reconstruction-live-effect").await?;
        let mut reconstruction_claim = executor.gate_next_entity_reconstruction_claim(&worker_id);
        let mut reconstruction_body =
            executor.gate_next_completed_entity_reconstruction(&worker_id);
        executor.simulated_crash(&worker_id).await?;
        drop(original_live.release);
        let reconstruction_start = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_claim.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("historical reconstruction claim was not reached"))?;
        let barrier = executor
            .drain_terminal_clamp_then_reconstruction_barrier(&owned_agent_id, reconstruction_start)
            .await?;
        tokio::pin!(barrier);
        assert!(
            matches!(futures::poll!(barrier.as_mut()), std::task::Poll::Pending),
            "the primary replay-to-live barrier ignored the atomically registered reconstruction claim"
        );

        reconstruction_claim.release();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_body.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("completed reconstruction body did not settle"))?;
        assert!(
            matches!(futures::poll!(barrier.as_mut()), std::task::Poll::Pending),
            "the primary replay-to-live barrier was released before body validation"
        );
        reconstruction_body.release();
        tokio::time::timeout(std::time::Duration::from_secs(30), barrier)
            .await
            .map_err(|_| anyhow::anyhow!("validated reconstruction did not release the barrier"))?;
        let replayed_live = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoints.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed live effect remained blocked"))?
        .ok_or_else(|| anyhow::anyhow!("caller checkpoint server stopped"))?;
        assert_eq!(replayed_live.name, original_live.name);
        replayed_live
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed live-effect gate was dropped"))?;
        Ok::<_, anyhow::Error>(())
    };
    let _ = tokio::try_join!(invocation, crash_and_validate)?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke"
                )
            })
            .count(),
        1
    );
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn incomplete_custom_durability_waits_for_completed_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let agent_id = agent_id!(
        "ToolStreamingCaller",
        "incomplete-custom-reconstruction-barrier"
    );
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "hold_completed_reconstruction_before_incomplete_custom",
        data_value!(),
    );
    let crash_and_validate = async {
        let original_body =
            next_crash_checkpoint(&mut provider_checkpoints, "historical-reconstruction-body")
                .await?;
        let original_before_custom = next_crash_checkpoint(
            &mut caller_checkpoints,
            "before-reconstruction-custom-effect",
        )
        .await?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 1).await?;
        original_body
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original reconstruction body gate was dropped"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        original_before_custom
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original custom-start gate was dropped"))?;
        let original_custom =
            next_crash_checkpoint(&mut caller_checkpoints, "reconstruction-custom-effect").await?;
        let custom_start = executor
            .get_oplog(&worker_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem-it::reconstruction-barrier-custom-effect" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("recorded custom durability Start was not found"))?;
        let mut reconstruction_claim = executor.gate_next_entity_reconstruction_claim(&worker_id);
        let mut reconstruction_body =
            executor.gate_next_completed_entity_reconstruction(&worker_id);
        // Keep the original gate unresolved while recovery starts so the custom call cannot gain
        // a terminal during teardown.
        executor.simulated_crash(&worker_id).await?;
        let reconstruction_start = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_claim.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("historical reconstruction claim was not reached"))?;
        executor
            .drain_reconstruction_terminal(&owned_agent_id, reconstruction_start)
            .await?;
        reconstruction_claim.release();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_body.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("completed reconstruction body did not settle"))?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            executor.clamp_after_claim(&owned_agent_id, custom_start),
        )
        .await
        .map_err(|_| anyhow::anyhow!("custom durability did not reach replay-to-live"))??;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                caller_checkpoints.recv()
            )
            .await
            .is_err(),
            "the incomplete custom invocation bypassed the primary reconstruction barrier"
        );
        reconstruction_body.release();
        let replayed_custom = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoints.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed custom effect remained blocked"))?
        .ok_or_else(|| anyhow::anyhow!("caller checkpoint server stopped"))?;
        assert_eq!(replayed_custom.name, original_custom.name);
        replayed_custom
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed custom-effect gate was dropped"))?;
        drop(original_custom.release);
        Ok::<_, anyhow::Error>(())
    };
    let _ = tokio::try_join!(invocation, crash_and_validate)?;

    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/reconstruction-custom-order.log")
            .await?,
        b"C".as_slice(),
        "the repaired custom effect must commit exactly once after body validation"
    );
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn deterministic_stream_crash_checkpoint_matrix(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    #[derive(Clone, Copy)]
    enum Checkpoint {
        Incapable {
            name: &'static str,
            checkpoint: &'static str,
            checkpoint_file: Option<&'static str>,
            entity_terminal: bool,
        },
        CapableStaging,
        Capable {
            name: &'static str,
            path: &'static str,
            checkpoint_file: &'static str,
            checkpoint_bytes: &'static [u8],
        },
        CapablePublished {
            path: &'static str,
            checkpoint_file: &'static str,
            checkpoint_bytes: &'static [u8],
        },
    }

    impl Checkpoint {
        fn name(self) -> &'static str {
            match self {
                Self::Incapable { name, .. } | Self::Capable { name, .. } => name,
                Self::CapableStaging => "capable-staging",
                Self::CapablePublished { .. } => "capable-published",
            }
        }

        fn expected_operation(
            self,
        ) -> Option<(ToolBodyAdmissionMetadata, ToolOperationLaneMetadata)> {
            match self {
                Self::Incapable {
                    entity_terminal: false,
                    ..
                } => Some((
                    ToolBodyAdmissionMetadata::Running,
                    ToolOperationLaneMetadata::None,
                )),
                Self::Incapable {
                    entity_terminal: true,
                    ..
                }
                | Self::CapablePublished { .. } => None,
                Self::CapableStaging => Some((
                    ToolBodyAdmissionMetadata::Staging,
                    ToolOperationLaneMetadata::None,
                )),
                Self::Capable { .. } => Some((
                    ToolBodyAdmissionMetadata::Running,
                    ToolOperationLaneMetadata::Granted,
                )),
            }
        }

        fn checkpoint_file(self) -> Option<(&'static str, &'static [u8])> {
            match self {
                Self::Incapable {
                    checkpoint_file: Some(path),
                    ..
                } => Some((path, b"reached")),
                Self::Capable {
                    checkpoint_file,
                    checkpoint_bytes,
                    ..
                }
                | Self::CapablePublished {
                    checkpoint_file,
                    checkpoint_bytes,
                    ..
                } => Some((checkpoint_file, checkpoint_bytes)),
                _ => None,
            }
        }

        fn expects_entity_terminal(self) -> bool {
            matches!(
                self,
                Self::Incapable {
                    entity_terminal: true,
                    ..
                } | Self::CapablePublished { .. }
            )
        }

        fn expects_marker(self) -> bool {
            self.checkpoint_file().is_some()
        }

        fn attachment_progress_ready(self, operation: &ToolOperationMetadata) -> bool {
            let (Some(stdin), Some(stdout)) = (&operation.stdin, &operation.stdout) else {
                return false;
            };
            match self {
                Self::Incapable {
                    checkpoint: "before-input",
                    ..
                } => {
                    stdin.accepted_bytes == 0
                        && stdout.accepted_bytes == 0
                        && !stdin.terminal_selected
                        && !stdout.terminal_selected
                }
                Self::Incapable {
                    checkpoint: "after-input-and-stdout",
                    ..
                } => stdin.accepted_bytes >= 10 && stdout.delivered_bytes >= 17,
                Self::Incapable {
                    checkpoint: "after-eof-before-terminal",
                    ..
                } => {
                    stdin.delivered_bytes >= 10
                        && stdin.terminal_selected
                        && stdout.delivered_bytes >= 12
                        && !stdout.terminal_selected
                }
                Self::Incapable {
                    checkpoint: "after-stdout-terminal",
                    ..
                } => {
                    stdin.delivered_bytes >= 10
                        && stdout.delivered_bytes >= 15
                        && stdout.terminal_selected
                }
                Self::Incapable { .. } => true,
                Self::CapableStaging => stdin.accepted_bytes >= 6,
                Self::Capable {
                    name: "capable-body",
                    ..
                } => {
                    stdin.delivered_bytes >= 12
                        && stdin.terminal_selected
                        && stdout.accepted_bytes >= 15
                }
                Self::Capable {
                    name: "capable-completion",
                    ..
                } => {
                    stdin.delivered_bytes >= 18
                        && stdin.terminal_selected
                        && stdout.accepted_bytes >= 39
                }
                Self::Capable { .. } | Self::CapablePublished { .. } => false,
            }
        }
    }

    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let checkpoints = [
        Checkpoint::Incapable {
            name: "before-input",
            checkpoint: "before-input",
            checkpoint_file: None,
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-input-and-stdout",
            checkpoint: "after-input-and-stdout",
            checkpoint_file: Some("/after-input-and-stdout.checkpoint"),
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-eof-before-terminal",
            checkpoint: "after-eof-before-terminal",
            checkpoint_file: Some("/after-eof-before-terminal.checkpoint"),
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-stdout-terminal",
            checkpoint: "after-stdout-terminal",
            checkpoint_file: Some("/after-stdout-terminal.checkpoint"),
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-entity-terminal",
            checkpoint: "after-terminal-before-result",
            checkpoint_file: Some("/after-terminal-before-result.checkpoint"),
            entity_terminal: true,
        },
        Checkpoint::CapableStaging,
        Checkpoint::Capable {
            name: "capable-body",
            path: "hold-body:/capable-body.checkpoint",
            checkpoint_file: "/capable-body.checkpoint",
            checkpoint_bytes: b"capable-body",
        },
        Checkpoint::Capable {
            name: "capable-completion",
            path: "hold-completion:/capable-completion.checkpoint",
            checkpoint_file: "/capable-completion.checkpoint",
            checkpoint_bytes: b"capable-completion:buffered",
        },
        Checkpoint::CapablePublished {
            path: "order:U:/capable-published.bin",
            checkpoint_file: "/capable-order.log",
            checkpoint_bytes: b"U",
        },
    ];
    let checkpoint_filter = std::env::var("GOLEM_TEST_CRASH_CHECKPOINT").ok();

    for checkpoint in checkpoints {
        if checkpoint_filter
            .as_deref()
            .is_some_and(|filter| filter != checkpoint.name())
        {
            continue;
        }
        let agent_id = agent_id!(
            "ToolStreamingCaller",
            format!("crash-{}", checkpoint.name())
        );
        let worker_id = executor
            .start_agent_with(
                &caller_component.id,
                agent_id.clone(),
                HashMap::from([
                    (
                        "CRASH_CHECKPOINT_PORT".to_string(),
                        checkpoint_port.to_string(),
                    ),
                    (
                        "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                        checkpoint_gate_port.to_string(),
                    ),
                ]),
                Vec::new(),
            )
            .await?;
        let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
        match checkpoint {
            Checkpoint::Incapable { checkpoint, .. } => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_incapable_checkpoint",
                        data_value!(checkpoint),
                    )
                    .await?;
            }
            Checkpoint::CapableStaging => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_capable_staging_checkpoint",
                        data_value!(b"staged".to_vec()),
                    )
                    .await?;
            }
            Checkpoint::Capable { path, name, .. } => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_capable_checkpoint",
                        data_value!(path, name.as_bytes().to_vec()),
                    )
                    .await?;
            }
            Checkpoint::CapablePublished { path, .. } => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_capable_published_checkpoint",
                        data_value!(path, checkpoint.name().as_bytes().to_vec()),
                    )
                    .await?;
            }
        }

        let original_checkpoint = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for original `{}` component gate",
                checkpoint.name()
            )
        })?
        .ok_or_else(|| anyhow::anyhow!("crash checkpoint server stopped"))?;
        assert_eq!(original_checkpoint.name, checkpoint.name());

        let (original_start, original_marker) = if let Some((expected_admission, expected_lane)) =
            checkpoint.expected_operation()
        {
            let (start, marker, admission, lane, attachment_count) =
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    loop {
                        if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                            && let Some(operation) = active.tool_operations.operations.first()
                            && let Some(start) = operation.start_index
                            && operation.admission == expected_admission
                            && operation.lane == expected_lane
                            && operation.attachment_count == 2
                            && checkpoint.attachment_progress_ready(operation)
                            && (!checkpoint.expects_marker()
                                || active.reached_oplog_marker.is_some())
                            && (expected_admission != ToolBodyAdmissionMetadata::Running
                                || active.slots.iter().any(|slot| {
                                    slot.invocations.iter().any(|invocation| {
                                        invocation.invocation_id.start_index() == start
                                            && invocation.store_attached
                                    })
                                }))
                        {
                            break (
                                start,
                                active.reached_oplog_marker,
                                operation.admission,
                                operation.lane,
                                operation.attachment_count,
                            );
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "timed out waiting for original `{}` crash checkpoint",
                        checkpoint.name()
                    )
                })?;
            assert_eq!(
                admission,
                expected_admission,
                "unexpected admission at `{}`",
                checkpoint.name()
            );
            assert_eq!(
                lane,
                expected_lane,
                "unexpected lane state at `{}`",
                checkpoint.name()
            );
            assert_eq!(
                attachment_count,
                2,
                "both fresh attachments must remain active at `{}`",
                checkpoint.name()
            );
            (start, marker)
        } else {
            let terminal_checkpoint =
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    loop {
                        if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                            && (!checkpoint.expects_marker()
                                || active.reached_oplog_marker.is_some())
                            && active.tool_operations.operations.is_empty()
                            && active.slots.iter().all(|slot| slot.invocations.is_empty())
                        {
                            let oplog = executor
                                .get_oplog(&worker_id, OplogIndex::INITIAL)
                                .await
                                .expect("read owner oplog at crash checkpoint");
                            let entity_starts = oplog
                                .iter()
                                .filter_map(|entry| match &entry.entry {
                                    PublicOplogEntry::Start(params)
                                        if params.function_name == "golem::entity::invoke" =>
                                    {
                                        Some(entry.oplog_index)
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<_>>();
                            if entity_starts.len() == 1 {
                                break (entity_starts[0], active.reached_oplog_marker);
                            }
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await;
            match terminal_checkpoint {
                Ok(checkpoint) => checkpoint,
                Err(_) => {
                    let active = executor.active_entity_metadata(&owned_agent_id).await;
                    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                    anyhow::bail!(
                        "timed out waiting for original `{}` crash checkpoint; active metadata: {active:#?}; oplog: {oplog:#?}",
                        checkpoint.name()
                    );
                }
            }
        };
        let original_terminal = if checkpoint.expects_entity_terminal() {
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let terminal = oplog
                .iter()
                .find(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == original_start
                    )
                })
                .expect("post-terminal checkpoint must expose its structured entity terminal");
            Some(serde_json::to_value(&terminal.entry)?)
        } else {
            None
        };

        executor.simulated_crash(&worker_id).await?;
        drop(original_checkpoint.release);

        let replayed_checkpoint = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await;
        let replayed_checkpoint = match replayed_checkpoint {
            Ok(Some(checkpoint)) => checkpoint,
            Ok(None) => anyhow::bail!("crash checkpoint server stopped"),
            Err(_) => {
                let active = executor.active_entity_metadata(&owned_agent_id).await;
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                anyhow::bail!(
                    "timed out waiting for replayed `{}` component gate; active metadata: {active:#?}; oplog: {oplog:#?}",
                    checkpoint.name()
                );
            }
        };
        assert_eq!(replayed_checkpoint.name, checkpoint.name());

        let replay_ready = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let checkpoint_ready = executor
                    .active_entity_metadata(&owned_agent_id)
                    .await
                    .is_some_and(|active| {
                        let marker_ready = original_marker
                            .is_none_or(|marker| active.reached_oplog_marker == Some(marker));
                        marker_ready
                            && match checkpoint.expected_operation() {
                                Some((admission, lane)) => {
                                    active.tool_operations.operations.first().is_some_and(
                                        |operation| {
                                            operation.start_index == Some(original_start)
                                                && operation.admission == admission
                                                && operation.lane == lane
                                                && operation.attachment_count == 2
                                                && checkpoint.attachment_progress_ready(operation)
                                                && (admission != ToolBodyAdmissionMetadata::Running
                                                    || active.slots.iter().any(|slot| {
                                                        slot.invocations.iter().any(|invocation| {
                                                            invocation.invocation_id.start_index()
                                                                == original_start
                                                                && invocation.store_attached
                                                        })
                                                    }))
                                        },
                                    )
                                }
                                None => active.tool_operations.operations.is_empty(),
                            }
                    });
                if checkpoint_ready {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        if replay_ready.is_err() {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!(
                "timed out waiting for replayed `{}` crash checkpoint; active metadata: {active:#?}",
                checkpoint.name()
            );
        }

        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let entity_starts = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            entity_starts,
            vec![original_start],
            "replay must reuse the durable entity Start at `{}`",
            checkpoint.name()
        );
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == original_start
                    )
                })
                .count(),
            usize::from(checkpoint.expects_entity_terminal()),
            "replay must not duplicate the entity terminal at `{}`",
            checkpoint.name()
        );
        if let Some(original_terminal) = original_terminal {
            let replayed_terminal = oplog
                .iter()
                .find(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == original_start
                    )
                })
                .expect("replay must retain the structured entity terminal");
            assert_eq!(
                serde_json::to_value(&replayed_terminal.entry)?,
                original_terminal,
                "replay must preserve the exact structured entity terminal"
            );
        }
        assert!(
            oplog
                .iter()
                .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
            "lifecycle replay must not write an owner Error at `{}`",
            checkpoint.name()
        );
        let serialized = serde_json::to_string(&oplog)?;
        for forbidden in ["attachment-id", "endpoint-id", "resource-key"] {
            assert!(
                !serialized.contains(forbidden),
                "transient stream identity `{forbidden}` must not be durable at `{}`",
                checkpoint.name()
            );
        }

        replayed_checkpoint
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed component gate was dropped before release"))?;
        let cleanup = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if executor
                    .active_entity_metadata(&owned_agent_id)
                    .await
                    .is_none_or(|active| {
                        active.tool_operations.operations.is_empty()
                            && active.lane.holder.is_none()
                            && active.lane.active_invocation_count == 0
                            && active.slots.iter().all(|slot| slot.invocations.is_empty())
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        if cleanup.is_err() {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!(
                "timed out waiting for `{}` checkpoint release cleanup; active metadata: {active:#?}",
                checkpoint.name()
            );
        }
        if let Some((path, expected)) = checkpoint.checkpoint_file() {
            assert_eq!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    executor.get_file_contents(&worker_id, path),
                )
                .await
                .map_err(|_| anyhow::anyhow!("checkpoint filesystem inspection timed out"))??,
                expected,
                "checkpoint filesystem bytes must be exact after replay at `{}`",
                checkpoint.name()
            );
        }
        executor.delete_worker(&worker_id).await?;
    }

    checkpoint_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn capable_terminal_lane_return_and_delayed_publication_survive_crash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "capable-terminal-crash");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let input = b"terminal-before-publication".to_vec();
    let output_path = "/capable-terminal-crash.bin";
    let mut child_completion_gate =
        executor.gate_next_live_entity_body_completion(&worker_id, "streaming");

    let call = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "hold_capable_terminal_checkpoint",
        data_value!(output_path, input.clone()),
    );
    let crash_and_replay = async {
        let original_child_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("original retained child checkpoint timed out"))?
        .ok_or_else(|| anyhow::anyhow!("crash checkpoint server stopped"))?;
        assert_eq!(original_child_gate.name, "capable-terminal-retained-child");
        original_child_gate
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original retained child gate dropped before release"))?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            child_completion_gate.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("original retained child completion gate timed out"))?;
        let mut replayed_child_completion =
            executor.gate_next_incomplete_entity_reconstruction(&worker_id, "streaming");

        let terminal_boundary = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                    && let Some(outer) =
                        active.tool_operations.operations.iter().find(|operation| {
                            operation.winner == ToolOperationWinnerMetadata::Ordinary
                        })
                    && let Some(outer_start) = outer.start_index
                    && outer.admission == ToolBodyAdmissionMetadata::Running
                    && outer.lane == ToolOperationLaneMetadata::None
                    && outer.attachment_count == 2
                    && let Some(stdout) = &outer.stdout
                    && stdout.mode == ToolAttachmentModeMetadata::CompletionStaged
                    && stdout.accepted_bytes == input.len() as u64
                    && stdout.delivered_bytes == 0
                    && stdout.buffered_bytes == input.len()
                    && stdout.terminal_selected
                    && matches!(active.lane.holder, Some(OwnerInvocationId::Agent(_)))
                {
                    let oplog = executor
                        .get_oplog(&worker_id, OplogIndex::INITIAL)
                        .await
                        .expect("read oplog at capable terminal boundary");
                    let entity_starts = oplog
                        .iter()
                        .filter_map(|entry| match &entry.entry {
                            PublicOplogEntry::Start(params)
                                if params.function_name == "golem::entity::invoke" =>
                            {
                                Some((entry.oplog_index, params.parent_start_index))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    let Some(child_start) = entity_starts.iter().find_map(|(start, parent)| {
                        (*parent == Some(outer_start)).then_some(*start)
                    }) else {
                        tokio::task::yield_now().await;
                        continue;
                    };
                    if entity_starts.len() != 2 {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    let Some(terminal) = oplog.iter().find(|entry| {
                        matches!(
                            &entry.entry,
                            PublicOplogEntry::End(params) if params.start_index == outer_start
                        )
                    }) else {
                        tokio::task::yield_now().await;
                        continue;
                    };
                    let durable_effects = oplog
                        .iter()
                        .filter(|entry| {
                            matches!(
                                &entry.entry,
                                PublicOplogEntry::Start(params)
                                    if params.function_name
                                        == "golem::api::generate_idempotency-key"
                                        && params.parent_start_index == Some(child_start)
                            )
                        })
                        .count();
                    let child_has_terminal = oplog.iter().any(|entry| {
                        matches!(
                            &entry.entry,
                            PublicOplogEntry::End(params)
                                if params.start_index == child_start
                        ) || matches!(
                            &entry.entry,
                            PublicOplogEntry::Cancelled(params)
                                if params.start_index == child_start
                        )
                    });
                    if durable_effects == 1 && !child_has_terminal {
                        break (
                            outer_start,
                            child_start,
                            serde_json::to_value(&terminal.entry)
                                .expect("serialize original capable entity terminal"),
                        );
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let (outer_start, child_start, original_terminal) = match terminal_boundary {
            Ok(boundary) => boundary,
            Err(_) => {
                let active = executor.active_entity_metadata(&owned_agent_id).await;
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                anyhow::bail!(
                    "capable terminal/lane-return/completion-staged boundary timed out; active metadata: {active:#?}; oplog: {oplog:#?}"
                );
            }
        };

        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            None,
        );
        executor.simulated_crash(&worker_id).await?;
        drop(child_completion_gate);

        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            replayed_child_completion.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed retained child completion timed out"))?;

        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                    && active.tool_operations.operations.iter().any(|operation| {
                        operation.start_index == Some(outer_start)
                            && operation.attachment_count == 2
                            && operation.stdout.as_ref().is_some_and(|stdout| {
                                stdout.mode == ToolAttachmentModeMetadata::CompletionStaged
                                    && stdout.accepted_bytes == input.len() as u64
                                    && stdout.delivered_bytes == 0
                                    && stdout.terminal_selected
                            })
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("fresh capable replay attachments were not reconstructed"))?;
        replayed_child_completion.release();

        let published_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed capable publication checkpoint timed out"))?
        .ok_or_else(|| anyhow::anyhow!("crash checkpoint server stopped"))?;
        assert_eq!(published_gate.name, "capable-terminal-published");
        published_gate
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("published caller gate dropped before release"))?;

        Ok::<_, anyhow::Error>((outer_start, child_start, original_terminal))
    };
    let (_, (outer_start, child_start, original_terminal)) =
        tokio::try_join!(call, crash_and_replay)?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entity_starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(entity_starts, vec![outer_start, child_start]);
    for start in [outer_start, child_start] {
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == start
                    )
                })
                .count(),
            1,
            "each pinned entity Start must retain exactly one terminal"
        );
    }
    let replayed_outer_terminal = oplog
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::End(params) if params.start_index == outer_start
            )
        })
        .expect("outer capable terminal remains in the oplog");
    assert_eq!(
        serde_json::to_value(&replayed_outer_terminal.entry)?,
        original_terminal
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::api::generate_idempotency-key"
                            && params.parent_start_index == Some(child_start)
                )
            })
            .count(),
        1,
        "recovery must consume the retained child's durable effect exactly once"
    );
    let serialized = serde_json::to_string(&oplog)?;
    for forbidden in ["attachment-id", "endpoint-id", "resource-key"] {
        assert!(!serialized.contains(forbidden));
    }
    assert_eq!(
        executor.get_file_contents(&worker_id, output_path).await?,
        input
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-terminal-published.checkpoint")
            .await?,
        b"reached".as_slice()
    );
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    executor.delete_worker(&worker_id).await?;
    checkpoint_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn typescript_generated_client_streams_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_ts_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_ts_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-ts-provider",
            "TsToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("TsToolStreamingCaller", "ts-live");
    let evidence: TsStreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "markerBeforeEof",
            data_value!(b"typescript-live".to_vec()),
        )
        .await?
        .into_typed()?;
    assert_eq!(evidence.output, b"ts-marker:typescript-live");
    assert_eq!(evidence.bytes_read, 15);

    let failure: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "typedStdoutFailure",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(failure, "resource-exhausted");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn scala_generated_client_streams_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let component_path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let metadata = extract_component_metadata(&component_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "scala:examples",
            "ScalaToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ScalaToolStreamingCaller", "scala-live");
    let evidence: ScalaStreamEvidence = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "markerBeforeEof",
            data_value!("scala-live"),
        )
        .await?
        .into_typed()?;
    assert_eq!(evidence.output, "scala-marker:scala-live");
    assert_eq!(evidence.bytes_read, 10);

    let cleanup: ScalaCleanupEvidence = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "invalidCommandPathCleanup",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(cleanup.error, "invalid-command-path:missing");
    assert!(cleanup.stdin_cancelled);
    assert_eq!(cleanup.stdout_terminal, "failed");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn moonbit_generated_client_streams_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let component_path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let metadata = extract_component_metadata(&component_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "golem:moonbit-examples",
            "MoonBitToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("MoonBitToolStreamingCaller", "moonbit-live");
    let worker_id = executor
        .start_agent(&stored_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let payload = b"moonbit-live".to_vec();
    let payload_value = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions::default())),
        SchemaValue::Binary(BinaryValuePayload {
            bytes: payload.clone(),
            mime_type: None,
        }),
    );
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "marker_before_eof",
            build_input_record(vec![payload_value])?,
        ),
    )
    .await;
    let bytes_read: u64 = match result {
        Ok(result) => result?.into_typed()?,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!("MoonBit streaming call timed out; active metadata: {active:#?}")
        }
    };
    assert_eq!(bytes_read, payload.len() as u64);

    let explicit_failure: String = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "explicit_stdout_failure",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(explicit_failure, "resource-exhausted:ok");

    Ok(())
}
