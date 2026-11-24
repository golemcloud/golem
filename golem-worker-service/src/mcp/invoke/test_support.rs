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

use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::agent_types::AgentTypeResolver;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::component::{ComponentService, ComponentServiceError};
use crate::service::limit::{LimitService, LimitServiceError};
use crate::service::worker::{
    ExistingAgent, RoutedTarget, WorkerClient, WorkerResult, WorkerService, WorkerServiceError,
    WorkerStream,
};
use async_trait::async_trait;
use chrono::Utc;
use futures::future::BoxFuture;
use golem_api_grpc::proto::golem::worker::{InvocationContext, LogEvent};
use golem_common::base_model::component_metadata::KnownExports;
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentMetadataDto, RegisteredAgentType, RegisteredAgentTypeImplementer,
    ResolvedRevert, RevertWorkerTarget,
};
use golem_common::model::agent::{AgentMode, AgentTypeName, Snapshotting};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::caller::Caller;
use golem_common::model::card::StoredCard;
use golem_common::model::component::{
    CanonicalFilePath, ComponentId, ComponentName, ComponentRevision, PluginPriority,
};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff::Hash;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::filesystem::FileByteSelection;
use golem_common::model::oplog::{OplogCursor, OplogIndex};
use golem_common::model::type_id::{TypeId, TypeIdKind};
use golem_common::model::{
    AgentFilter, AgentFingerprint, AgentId, IdempotencyKey, PromiseId, ScanCursor,
};
use golem_common::schema::{AgentConstructorSchema, AgentTypeSchema, SchemaGraph};
use golem_service_base::clients::registry::{
    RegistryService, RegistryServiceError, ResolvedTypeId,
};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use golem_service_base::model::{ComponentFileSystemNode, FileReadResponse, GetOplogResponse};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// The registry of the harness: its environment's current deployment has the one agent type.
#[derive(Clone)]
struct HarnessRegistryService {
    environment_id: EnvironmentId,
    registered: RegisteredAgentType,
}

#[async_trait]
impl RegistryService for HarnessRegistryService {
    async fn resolve_mcp_import(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
        _: bool,
    ) -> Result<golem_service_base::model::mcp_import::McpImportObservation, RegistryServiceError>
    {
        panic!("unexpected MCP discovery")
    }
    async fn get_mcp_runtime_credential(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
    ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
    {
        panic!("unexpected MCP credential request")
    }
    async fn report_mcp_resource_unauthorized(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
        _: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        panic!("unexpected MCP feedback")
    }
    async fn authenticate_token(
        &self,
        _: &golem_common::model::auth::TokenSecret,
    ) -> Result<AuthCtx, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_resource_limits(
        &self,
        _: AccountId,
        _metering: golem_service_base::clients::registry::ResourceUsageMetering,
    ) -> Result<golem_service_base::model::ResourceLimits, RegistryServiceError> {
        unimplemented!()
    }

    async fn update_worker_connection_limit(
        &self,
        _: AccountId,
        _: &golem_common::base_model::AgentId,
        _: bool,
    ) -> Result<(), RegistryServiceError> {
        unimplemented!()
    }

    async fn batch_update_resource_usage(
        &self,
        _: HashMap<AccountId, golem_service_base::clients::registry::ResourceUsageUpdate>,
    ) -> Result<golem_service_base::model::AccountResourceLimits, RegistryServiceError> {
        unimplemented!()
    }

    async fn download_component(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_component_metadata(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_deployed_component_metadata(
        &self,
        _: ComponentId,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_all_deployed_component_revisions(
        &self,
        _: ComponentId,
    ) -> Result<Vec<Component>, RegistryServiceError> {
        unimplemented!()
    }

    async fn resolve_component(
        &self,
        _: AccountId,
        _: ApplicationId,
        _: EnvironmentId,
        _: &str,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_all_agent_types(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<golem_common::model::agent::RegisteredAgentType>, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_agent_type(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
        _: &AgentTypeName,
    ) -> Result<golem_common::model::agent::RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }

    async fn resolve_agent_type_by_names(
        &self,
        _: &golem_common::model::application::ApplicationName,
        _: &golem_common::model::environment::EnvironmentName,
        _: &AgentTypeName,
        _: Option<golem_common::model::deployment::DeploymentRevision>,
        _: Option<&str>,
        _: &AuthCtx,
    ) -> Result<golem_common::model::agent::ResolvedAgentType, RegistryServiceError> {
        unimplemented!()
    }

    async fn resolve_type_id(
        &self,
        environment_id: EnvironmentId,
        type_id: TypeId,
    ) -> Result<Option<ResolvedTypeId>, RegistryServiceError> {
        Ok(
            (environment_id == self.environment_id && type_id == self.registered.type_id).then(
                || ResolvedTypeId {
                    kind: TypeIdKind::AgentType,
                    name: self.registered.agent_type.type_name.0.clone(),
                    implemented_by: Some(self.registered.implemented_by.clone()),
                },
            ),
        )
    }

    async fn get_deployed_agent_type(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
    ) -> Result<Option<RegisteredAgentType>, RegistryServiceError> {
        Ok((environment_id == self.environment_id
            && *agent_type_name == self.registered.agent_type.type_name)
            .then(|| self.registered.clone()))
    }

    async fn get_active_routes_for_domain(
        &self,
        _: &golem_common::model::domain_registration::Domain,
    ) -> Result<golem_service_base::custom_api::CompiledRoutes, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_active_compiled_mcps_for_domain(
        &self,
        _: &golem_common::model::domain_registration::Domain,
    ) -> Result<golem_service_base::mcp::CompiledMcp, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_current_environment_state(
        &self,
        _: EnvironmentId,
    ) -> Result<golem_service_base::model::environment::EnvironmentState, RegistryServiceError>
    {
        unimplemented!()
    }

    async fn get_resource_definition_by_id(
        &self,
        _: golem_common::model::quota::ResourceDefinitionId,
    ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_resource_definition_by_name(
        &self,
        _: EnvironmentId,
        _: golem_common::model::quota::ResourceName,
    ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }

    async fn subscribe_registry_invalidations(
        &self,
        _: Option<u64>,
    ) -> Result<
        Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<
                            golem_common::model::agent::RegistryInvalidationEvent,
                            RegistryServiceError,
                        >,
                    > + Send,
            >,
        >,
        RegistryServiceError,
    > {
        unimplemented!()
    }

    async fn run_registry_invalidation_event_subscriber(
        &self,
        _: &'static str,
        _: Option<tokio_util::sync::CancellationToken>,
        _: Arc<dyn golem_service_base::clients::registry::RegistryInvalidationHandler>,
    ) {
        unimplemented!()
    }
}

/// The definition of the harness's native tool `native`: one command without input or output
/// streams.
fn native_tool_definition() -> golem_common::schema::tool::Tool {
    use golem_common::schema::tool::{
        CommandBody, CommandNode, CommandTree, Doc, Positionals, Tool,
    };
    Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        http_mount: None,
        schema: SchemaGraph::empty(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "native".to_string(),
                aliases: vec![],
                doc: Doc::default(),
                http: None,
                globals: Default::default(),
                subcommands: vec![],
                body: Some(CommandBody {
                    positionals: Positionals::default(),
                    options: vec![],
                    flags: vec![],
                    constraints: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                    result: None,
                    errors: vec![],
                    annotations: None,
                }),
            }],
        },
    }
}

struct StaticComponentService {
    component: Component,
    tool_state: golem_common::model::tool::ToolDeploymentState,
}

#[async_trait]
impl ComponentService for StaticComponentService {
    async fn invalidate_current(&self) {}

    async fn get_tool_deployment_state(
        &self,
        _: EnvironmentId,
        component_id: ComponentId,
        _: ComponentRevision,
    ) -> Result<Option<Arc<golem_common::model::tool::ToolDeploymentState>>, ComponentServiceError>
    {
        Ok((self.component.id == component_id).then(|| Arc::new(self.tool_state.clone())))
    }

    async fn get_current_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component> {
        (self.component.id == component_id).then(|| self.component.clone())
    }

    async fn get_current_by_id_uncached(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError> {
        if self.component.id == component_id {
            Ok(self.component.clone())
        } else {
            Err(ComponentServiceError::ComponentNotFound)
        }
    }

    async fn get_revision(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Component, ComponentServiceError> {
        if self.component.id == component_id && self.component.revision == component_revision {
            Ok(self.component.clone())
        } else {
            Err(ComponentServiceError::ComponentNotFound)
        }
    }

    async fn get_all_revisions(
        &self,
        component_id: ComponentId,
    ) -> Result<Vec<Component>, ComponentServiceError> {
        if self.component.id == component_id {
            Ok(vec![self.component.clone()])
        } else {
            Err(ComponentServiceError::ComponentNotFound)
        }
    }
}

struct AllowAllAuthService;

#[async_trait]
impl AuthService for AllowAllAuthService {
    async fn authenticate_token(
        &self,
        _: golem_common::model::auth::TokenSecret,
    ) -> Result<AuthCtx, AuthServiceError> {
        unimplemented!()
    }
}

struct NoopLimitService;

#[async_trait]
impl LimitService for NoopLimitService {
    async fn update_worker_connection_limit(
        &self,
        _: AccountId,
        _: &AgentId,
        _: bool,
    ) -> Result<(), LimitServiceError> {
        Ok(())
    }
}

pub(crate) struct FileReadCall {
    pub agent_id: AgentId,
    pub target: RoutedTarget,
    pub path: String,
    pub selection: FileByteSelection,
    pub environment_id: EnvironmentId,
    pub account_id: AccountId,
    pub auth: AuthCtx,
}

#[derive(Default)]
pub(crate) struct FileReadMock {
    pub calls: Mutex<Vec<FileReadCall>>,
    pub responses: Mutex<VecDeque<BoxFuture<'static, WorkerResult<FileReadResponse>>>>,
}

pub(crate) struct CreateCall {
    pub target: RoutedTarget,
    pub auth: AuthCtx,
    pub caller: Caller,
    pub idempotency_key: Option<IdempotencyKey>,
    pub environment_id: EnvironmentId,
    pub environment_variables: HashMap<String, String>,
    pub config: Vec<AgentConfigEntryDto>,
    pub ignore_already_existing: bool,
}

#[derive(Default)]
pub(crate) struct CreateMock {
    pub calls: Mutex<Vec<CreateCall>>,
    pub responses: Mutex<VecDeque<BoxFuture<'static, WorkerResult<AgentFingerprint>>>>,
}

pub(crate) struct RecordedInvocationContext {
    pub auth: AuthCtx,
    pub caller: golem_api_grpc::proto::golem::component::Caller,
    pub context: Option<InvocationContext>,
    pub idempotency_key: IdempotencyKey,
    pub request_key: golem_common::model::InvocationRequestKey,
    pub freshness: golem_common::model::agent::InvocationFreshnessDisposition,
}

/// How the recording worker client answers an invocation session, which a native tool call
/// opens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NativeSessions {
    /// The session is refused.
    Refuse,
    /// The session is accepted and never answers; its request stream is kept for the test.
    Hold,
    /// As [`Self::Hold`], but the session answers with a successful tool result when the test
    /// calls [`InvocationHarness::release_native_session`].
    HoldUntilReleased,
}

/// The invocation sessions that the recording worker client received.
pub(crate) struct NativeSessionLog {
    pub(crate) mode: NativeSessions,
    /// The idempotency key of each session's start.
    pub(crate) keys: Vec<Arc<str>>,
    pub(crate) starts: Vec<golem_api_grpc::proto::golem::worker::InvocationStart>,
    pub(crate) public_session_creation:
        Option<golem_api_grpc::proto::golem::workerexecutor::v1::CreateStreamSessionSuccess>,
    pub(crate) appends:
        Vec<golem_api_grpc::proto::golem::workerexecutor::v1::AppendToStreamSlotRequest>,
    /// The rest of each held session's request stream, after its start.
    pub(crate) tails: VecDeque<crate::service::worker::InvocationRequestStream>,
    /// The release of each session held until released, oldest first.
    pub(crate) releases: VecDeque<tokio::sync::oneshot::Sender<()>>,
    /// The number of sessions received so far.
    pub(crate) opened: tokio::sync::watch::Sender<usize>,
}

struct RecordingWorkerClient {
    component_id: ComponentId,
    agent_ids: Arc<Mutex<Vec<AgentId>>>,
    targets: Arc<Mutex<Vec<RoutedTarget>>>,
    metadata: Arc<Mutex<HashMap<AgentId, AgentMetadataDto>>>,
    metadata_reads: Arc<Mutex<Vec<AgentId>>>,
    prepared_agent_ids: Arc<Mutex<Vec<AgentId>>>,
    method_params: Arc<Mutex<Vec<Option<golem_common::model::agent::AgentMethodInput>>>>,
    durable_stream_controls: Arc<Mutex<Vec<
        golem_api_grpc::proto::golem::workerexecutor::v1::DurableStreamAttachmentControlRequest,
    >>>,
    durable_stream_forks: Arc<Mutex<Vec<
        golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotRequest,
    >>>,
    durable_stream_fork_result: Arc<Mutex<Option<
        golem_api_grpc::proto::golem::workerexecutor::v1::fork_stream_slot_response::Result,
    >>>,
    durable_stream_reads: Arc<Mutex<VecDeque<
        Option<golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotSuccess>,
    >>>,
    contexts: Arc<Mutex<Vec<RecordedInvocationContext>>>,
    invocation_output: AgentInvocationOutput,
    file_reads: Arc<FileReadMock>,
    creates: Arc<CreateMock>,
    native_sessions: Arc<Mutex<NativeSessionLog>>,
}

#[async_trait]
impl WorkerClient for RecordingWorkerClient {
    async fn prepare(
        &self,
        target: &RoutedTarget,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        account_id: AccountId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
        invocation_context: Option<InvocationContext>,
        caller: golem_api_grpc::proto::golem::component::Caller,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        _invocation: Option<golem_api_grpc::proto::golem::worker::InvocationStart>,
    ) -> WorkerResult<(AgentId, AgentFingerprint)> {
        let agent_id = target.agent_id;
        let _ = (
            environment_variables,
            config,
            ignore_already_existing,
            account_id,
            environment_id,
            auth_ctx,
            invocation_context,
            caller,
        );
        self.prepared_agent_ids.lock().unwrap().push(agent_id);
        Ok((agent_id, AgentFingerprint::new()))
    }

    async fn create(
        &self,
        target: &RoutedTarget,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        _: AccountId,
        environment_id: EnvironmentId,
        auth: AuthCtx,
        _: Option<InvocationContext>,
        caller: golem_api_grpc::proto::golem::component::Caller,
        idempotency_key: Option<IdempotencyKey>,
        _: Option<golem_api_grpc::proto::golem::worker::InvocationStart>,
    ) -> WorkerResult<(AgentId, AgentFingerprint)> {
        self.creates.calls.lock().unwrap().push(CreateCall {
            target: target.clone(),
            auth,
            caller: Caller::try_from(caller).unwrap(),
            idempotency_key,
            environment_id,
            environment_variables,
            config,
            ignore_already_existing,
        });
        let response = self
            .creates
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted worker create response");
        response
            .await
            .map(|fingerprint| (target.agent_id, fingerprint))
    }

    async fn connect(
        &self,
        _: &AgentId,
        _: EnvironmentId,
        _: AccountId,
        _: AuthCtx,
    ) -> WorkerResult<WorkerStream<LogEvent>> {
        unimplemented!()
    }

    async fn delete(
        &self,
        _: &AgentId,
        _: EnvironmentId,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn complete_promise(
        &self,
        _: &PromiseId,
        _: Vec<u8>,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<bool> {
        unimplemented!()
    }

    async fn interrupt(
        &self,
        _: &AgentId,
        _: bool,
        _: EnvironmentId,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn get_metadata(
        &self,
        agent_id: &AgentId,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<ExistingAgent> {
        self.metadata_reads.lock().unwrap().push(*agent_id);
        self.metadata
            .lock()
            .unwrap()
            .get(agent_id)
            .cloned()
            .map(|metadata| ExistingAgent {
                metadata,
                component_id: self.component_id,
            })
            .ok_or(WorkerServiceError::AgentNotFound(*agent_id))
    }

    async fn find_metadata(
        &self,
        _: TypeId,
        _: Option<AgentFilter>,
        _: ScanCursor,
        _: u64,
        _: bool,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<ExistingAgent>)> {
        unimplemented!()
    }

    async fn resume(
        &self,
        _: &AgentId,
        _: bool,
        _: EnvironmentId,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn update(
        &self,
        _: &AgentId,
        _: golem_common::model::agent::AgentUpdateMode,
        _: ComponentRevision,
        _: bool,
        _: EnvironmentId,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<OplogIndex> {
        Ok(OplogIndex::INITIAL)
    }

    async fn get_oplog(
        &self,
        _: &AgentId,
        _: OplogIndex,
        _: Option<OplogCursor>,
        _: u64,
        _: Option<crate::service::worker::OplogRangeRead>,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> Result<GetOplogResponse, crate::service::worker::WorkerServiceError> {
        unimplemented!()
    }

    async fn search_oplog(
        &self,
        _: &AgentId,
        _: Option<OplogCursor>,
        _: u64,
        _: String,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> Result<GetOplogResponse, crate::service::worker::WorkerServiceError> {
        unimplemented!()
    }

    async fn get_oplog_entry(
        &self,
        _: &AgentId,
        _: OplogIndex,
        _: Option<golem_common::model::component::ComponentRevision>,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> Result<
        golem_common::model::oplog::PublicOplogEntryWithIndex,
        crate::service::worker::WorkerServiceError,
    > {
        unimplemented!()
    }

    async fn get_file_system_node(
        &self,
        _: &AgentId,
        _: CanonicalFilePath,
        _: EnvironmentId,
        _: AccountId,
        _: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        unimplemented!()
    }

    async fn get_agent_wallet(
        &self,
        _: &AgentId,
        _: EnvironmentId,
        _: AccountId,
        _: AuthCtx,
    ) -> WorkerResult<Vec<StoredCard>> {
        unimplemented!()
    }

    async fn get_file_contents(
        &self,
        target: &RoutedTarget,
        path: CanonicalFilePath,
        selection: FileByteSelection,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> WorkerResult<golem_service_base::model::FileReadResponse> {
        self.file_reads.calls.lock().unwrap().push(FileReadCall {
            agent_id: target.agent_id,
            target: target.clone(),
            path: path.to_string(),
            selection,
            environment_id,
            account_id,
            auth,
        });
        let response = self
            .file_reads
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected file read");
        response.await
    }

    async fn activate_plugin(
        &self,
        _: &AgentId,
        _: PluginPriority,
        _: EnvironmentId,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn deactivate_plugin(
        &self,
        _: &AgentId,
        _: PluginPriority,
        _: EnvironmentId,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn fork_worker(
        &self,
        _: &AgentId,
        _: &AgentId,
        _: OplogIndex,
        _: EnvironmentId,
        _: AccountId,
        _: AccountEmail,
        _: Caller,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn revert_worker(
        &self,
        _: &AgentId,
        _: RevertWorkerTarget,
        _: Option<ResolvedRevert>,
        _: EnvironmentId,
        _: AgentFingerprint,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn resolve_revert_last_invocations(
        &self,
        _: &AgentId,
        _: u64,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<ResolvedRevert> {
        unimplemented!()
    }

    async fn cancel_invocation(
        &self,
        _: &AgentId,
        _: &golem_common::model::InvocationId,
        _: EnvironmentId,
        _: AuthCtx,
        _: Option<golem_api_grpc::proto::golem::worker::ScheduledInvocationCancellation>,
        _: Option<AgentFingerprint>,
    ) -> WorkerResult<bool> {
        unimplemented!()
    }

    async fn get_invocation(
        &self,
        _: &AgentId,
        _: &golem_common::model::InvocationId,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<Option<golem_api_grpc::proto::golem::workerexecutor::v1::InvocationSummary>>
    {
        unimplemented!()
    }

    async fn list_invocations(
        &self,
        _: &AgentId,
        _: golem_api_grpc::proto::golem::workerexecutor::v1::ListInvocationsRequest,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<golem_api_grpc::proto::golem::workerexecutor::v1::ListInvocationsSuccess>
    {
        unimplemented!()
    }

    async fn lookup_invocations(
        &self,
        _: &AgentId,
        _: &IdempotencyKey,
        _: Option<String>,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<Vec<golem_api_grpc::proto::golem::workerexecutor::v1::InvocationSummary>>
    {
        unimplemented!()
    }

    async fn invoke_agent(
        &self,
        target: &RoutedTarget,
        method_name: Option<String>,
        method_params: Option<golem_common::model::agent::AgentMethodInput>,
        _: i32,
        _: Option<::prost_types::Timestamp>,
        request_key: golem_common::model::InvocationRequestKey,
        context: Option<InvocationContext>,
        freshness: golem_common::model::agent::InvocationFreshnessDisposition,
        _: Vec<golem_common::model::agent::AgentConfigEntryDto>,
        _: EnvironmentId,
        _: AccountId,
        auth: AuthCtx,
        caller: golem_api_grpc::proto::golem::component::Caller,
        _: Option<golem_api_grpc::proto::golem::worker::EncodedScopeCard>,
        _: Option<AgentFingerprint>,
    ) -> WorkerResult<AgentInvocationOutput> {
        self.agent_ids.lock().unwrap().push(target.agent_id);
        self.targets.lock().unwrap().push(target.clone());
        self.method_params.lock().unwrap().push(method_params);
        self.contexts
            .lock()
            .unwrap()
            .push(RecordedInvocationContext {
                auth,
                caller,
                context,
                idempotency_key: request_key.owner_key(method_name.as_deref().unwrap_or_default()),
                request_key,
                freshness,
            });
        Ok(self.invocation_output.clone())
    }

    async fn create_stream_session(
        &self,
        _: &AgentId,
        request: golem_api_grpc::proto::golem::workerexecutor::v1::CreateStreamSessionRequest,
    ) -> WorkerResult<golem_api_grpc::proto::golem::workerexecutor::v1::CreateStreamSessionSuccess>
    {
        let start = request.invocation.expect("session invocation");
        assert!(start.captured_method_input.is_none());
        let input = start.input.clone().expect("native input");
        self.method_params.lock().unwrap().push(Some(
            golem_common::model::agent::AgentMethodInput::Native(
                golem_common::model::agent::NativeMethodInput::capture(input, vec![])
                    .unwrap()
                    .0,
            ),
        ));
        let mut log = self.native_sessions.lock().unwrap();
        log.keys.push(Arc::from(request.public_session_id));
        log.starts.push(start);
        if let Some(success) = &log.public_session_creation {
            return Ok(success.clone());
        }
        Err(WorkerServiceError::Internal(
            "durable stream sessions are not supported by this worker client".to_string(),
        ))
    }

    async fn invoke_agent_session(
        &self,
        _: &AgentId,
        request: crate::service::worker::InvocationRequestStream,
    ) -> WorkerResult<crate::service::worker::InvocationResponseStream> {
        let (start, tail) = futures::StreamExt::into_future(request).await;
        if let Some(golem_api_grpc::proto::golem::worker::InvocationRequest {
            request:
                Some(golem_api_grpc::proto::golem::worker::invocation_request::Request::Start(start)),
        }) = &start
        {
            if let Some(input) = &start.captured_method_input {
                self.method_params.lock().unwrap().push(Some(
                    golem_common::serialization::deserialize(input).unwrap(),
                ));
            }
            self.native_sessions
                .lock()
                .unwrap()
                .starts
                .push(start.clone());
        }
        let key = match start.and_then(|start| start.request) {
            Some(golem_api_grpc::proto::golem::worker::invocation_request::Request::Start(
                start,
            )) => start.idempotency_key.map(|key| key.value),
            _ => None,
        };
        let mut log = self.native_sessions.lock().unwrap();
        log.keys.push(Arc::from(key.unwrap_or_default()));
        let answer: crate::service::worker::InvocationResponseStream = match log.mode {
            NativeSessions::Refuse => {
                log.opened.send_modify(|opened| *opened += 1);
                return Err(WorkerServiceError::Internal(
                    "invocation session refused".to_string(),
                ));
            }
            NativeSessions::Hold => {
                log.tails.push_back(tail);
                Box::pin(futures::stream::pending())
            }
            NativeSessions::HoldUntilReleased => {
                log.tails.push_back(tail);
                let (release, released) = tokio::sync::oneshot::channel();
                log.releases.push_back(release);
                let answer = log.starts.last().map(succeeded_tool_session);
                Box::pin(futures::StreamExt::flatten(futures::stream::once(
                    async move {
                        let _ = released.await;
                        futures::stream::iter(answer.into_iter().flatten().map(Ok))
                    },
                )))
            }
        };
        log.opened.send_modify(|opened| *opened += 1);
        Ok(answer)
    }

    async fn control_export_stream(
        &self,
        _: &AgentId,
        request: golem_api_grpc::proto::golem::workerexecutor::v1::DurableStreamAttachmentControlRequest,
    ) -> WorkerResult<golem_api_grpc::proto::golem::workerexecutor::v1::ExportStreamControlResult>
    {
        self.durable_stream_controls.lock().unwrap().push(request);
        Ok(golem_api_grpc::proto::golem::workerexecutor::v1::ExportStreamControlResult::Applied)
    }

    async fn fork_stream_slot(
        &self,
        _: &AgentId,
        request: golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotRequest,
    ) -> WorkerResult<
        golem_api_grpc::proto::golem::workerexecutor::v1::fork_stream_slot_response::Result,
    > {
        use golem_api_grpc::proto::golem::workerexecutor::v1::{
            ForkStreamSlotRejection, fork_stream_slot_rejection, fork_stream_slot_response,
        };
        self.durable_stream_forks.lock().unwrap().push(request);
        Ok(self
            .durable_stream_fork_result
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| {
                fork_stream_slot_response::Result::Rejected(ForkStreamSlotRejection {
                    reason: fork_stream_slot_rejection::Reason::NotFound as i32,
                    ..Default::default()
                })
            }))
    }

    async fn append_to_stream_slot(
        &self,
        _: &AgentId,
        request: golem_api_grpc::proto::golem::workerexecutor::v1::AppendToStreamSlotRequest,
    ) -> WorkerResult<golem_api_grpc::proto::golem::workerexecutor::v1::AppendToStreamSlotResponse>
    {
        self.native_sessions.lock().unwrap().appends.push(request);
        Ok(golem_api_grpc::proto::golem::workerexecutor::v1::AppendToStreamSlotResponse {
            result: Some(golem_api_grpc::proto::golem::workerexecutor::v1::append_to_stream_slot_response::Result::Accepted(Default::default())),
            stream_head_offset: Default::default(),
            stream_closed: Some(false),
            ..Default::default()
        })
    }

    async fn read_stream_slot(
        &self,
        _: &AgentId,
        _: golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotRequest,
    ) -> WorkerResult<Option<golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotSuccess>>
    {
        self.durable_stream_reads
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| {
                WorkerServiceError::Internal("no scripted durable stream read response".to_string())
            })
    }

    async fn deliver_card_transfer(
        &self,
        _: &AgentId,
        _: EnvironmentId,
        _: uuid::Uuid,
        _: golem_common::model::card::CardId,
        _: golem_common::model::card::StoredCard,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }

    async fn process_oplog_entries(
        &self,
        _: &AgentId,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
        _: golem_common::model::InvocationId,
        _: AccountId,
        _: HashMap<String, String>,
        _: golem_api_grpc::proto::golem::worker::AgentMetadata,
        _: OplogIndex,
        _: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        _: AuthCtx,
    ) -> WorkerResult<()> {
        unimplemented!()
    }
}

pub(crate) struct InvocationHarness {
    pub(crate) worker_service: Arc<WorkerService>,
    pub(crate) component_id: ComponentId,
    pub(crate) environment_id: EnvironmentId,
    pub(crate) account_id: AccountId,
    pub(crate) account_email: AccountEmail,
    pub(crate) file_reads: Arc<FileReadMock>,
    pub(crate) creates: Arc<CreateMock>,
    pub(crate) metadata: Arc<Mutex<HashMap<AgentId, AgentMetadataDto>>>,
    pub(crate) metadata_reads: Arc<Mutex<Vec<AgentId>>>,
    agent_ids: Arc<Mutex<Vec<AgentId>>>,
    pub(crate) targets: Arc<Mutex<Vec<RoutedTarget>>>,
    method_params: Arc<Mutex<Vec<Option<golem_common::model::agent::AgentMethodInput>>>>,
    durable_stream_controls: Arc<Mutex<Vec<
        golem_api_grpc::proto::golem::workerexecutor::v1::DurableStreamAttachmentControlRequest,
    >>>,
    durable_stream_forks: Arc<Mutex<Vec<
        golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotRequest,
    >>>,
    durable_stream_fork_result: Arc<Mutex<Option<
        golem_api_grpc::proto::golem::workerexecutor::v1::fork_stream_slot_response::Result,
    >>>,
    durable_stream_reads: Arc<Mutex<VecDeque<
        Option<golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotSuccess>,
    >>>,
    pub(crate) contexts: Arc<Mutex<Vec<RecordedInvocationContext>>>,
    pub(crate) native_sessions: Arc<Mutex<NativeSessionLog>>,
    pub(crate) agent_type: AgentTypeSchema,
    /// The type ID that the registry assigns to the harness's agent type.
    pub(crate) type_id: TypeId,
}

impl InvocationHarness {
    pub(crate) fn add_keyed_agent(&self, key: &golem_common::model::agent::AgentKey) -> AgentId {
        let agent_id = AgentId::keyed(self.environment_id, self.type_id, key);
        self.metadata.lock().unwrap().insert(
            agent_id,
            AgentMetadataDto {
                agent_id,
                owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
                agent_type_name: Some(key.agent_type().clone()),
                canonical_key: Some(key.to_string()),
                origin: golem_common::model::agent::AgentOrigin::Keyed,
                constructor_parameters: Some(key.canonical_params().to_string()),
                environment_id: self.environment_id,
                created_by: self.account_id,
                env: Default::default(),
                config: vec![],
                status: golem_common::model::AgentStatus::Idle,
                underlying_status: None,
                component_revision: ComponentRevision::INITIAL,
                retry_count: 0,
                pending_invocation_count: 0,
                updates: vec![],
                created_at: golem_common::model::Timestamp::now_utc(),
                last_error: None,
                last_error_kind: None,
                component_size: 0,
                total_linear_memory_size: 0,
                exported_resource_instances: vec![],
                active_plugins: Default::default(),
                skipped_regions: vec![],
                deleted_regions: vec![],
                last_oplog_index: OplogIndex::INITIAL,
                fingerprint: AgentFingerprint::new(),
            },
        );
        agent_id
    }

    pub(crate) fn new(
        invocation_output: AgentInvocationOutput,
        constructor: AgentConstructorSchema,
        methods: Vec<golem_common::schema::AgentMethodSchema>,
    ) -> Self {
        Self::new_with_agent_mode(invocation_output, AgentMode::Durable, constructor, methods)
    }

    /// A harness whose agent type `mcp-agent` has one method `run` without parameters or
    /// output. Its invocations answer at once.
    pub(crate) fn with_run_method(agent_mode: AgentMode) -> Self {
        use golem_common::schema::{AgentMethodSchema, InputSchema, OutputSchema};
        Self::new_with_agent_mode(
            AgentInvocationOutput {
                result: golem_common::model::AgentInvocationResult::AgentInitialization,
                consumed_fuel: None,
                invocation_status: None,
                component_revision: None,
                agent_id: None,
                idempotency_key: None,
                oplog_index: None,
                agent_fingerprint: None,
            },
            agent_mode,
            AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            vec![AgentMethodSchema {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            }],
        )
    }

    pub(crate) fn new_with_agent_mode(
        invocation_output: AgentInvocationOutput,
        agent_mode: AgentMode,
        constructor: AgentConstructorSchema,
        methods: Vec<golem_common::schema::AgentMethodSchema>,
    ) -> Self {
        let agent_type = AgentTypeSchema {
            kind: golem_common::schema::agent::AgentTypeKind::Regular,
            type_name: AgentTypeName("mcp-agent".to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor,
            methods,
            dependencies: vec![],
            mode: agent_mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(golem_schema::Empty {}),
            config: vec![],
        };
        Self::with_agent_type(
            invocation_output,
            agent_type,
            ComponentId(Uuid::new_v4()),
            EnvironmentId(Uuid::new_v4()),
        )
    }

    /// A harness whose component `component_id` of the environment `environment_id`
    /// implements `agent_type` in the current deployment.
    pub(crate) fn with_agent_type(
        invocation_output: AgentInvocationOutput,
        agent_type: AgentTypeSchema,
        component_id: ComponentId,
        environment_id: EnvironmentId,
    ) -> Self {
        let account_id = AccountId(Uuid::new_v4());
        let account_email = golem_common::model::account::AccountEmail::new("mcp@golem");
        let component = Component {
            id: component_id,
            revision: ComponentRevision::INITIAL,
            environment_id,
            component_name: ComponentName("mcp-component".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId(Uuid::new_v4()),
            account_id,
            account_email: account_email.clone(),
            application_name: ApplicationName::try_from("test-app".to_string()).unwrap(),
            environment_name: EnvironmentName::try_from("test-env").unwrap(),
            component_size: 0,
            metadata: ComponentMetadata::from_parts(
                KnownExports::default(),
                vec![],
                vec![agent_type.clone()],
                BTreeMap::new(),
            ),
            created_at: Utc::now(),
            wasm_hash: Hash::empty(),
            package_metadata: None,
            object_store_key: String::new(),
        };
        let agent_ids = Arc::new(Mutex::new(Vec::new()));
        let targets = Arc::new(Mutex::new(Vec::new()));
        let metadata = Arc::new(Mutex::new(HashMap::new()));
        let metadata_reads = Arc::new(Mutex::new(Vec::new()));
        let prepared_agent_ids = Arc::new(Mutex::new(Vec::new()));
        let method_params = Arc::new(Mutex::new(Vec::new()));
        let durable_stream_controls = Arc::new(Mutex::new(Vec::new()));
        let durable_stream_forks = Arc::new(Mutex::new(Vec::new()));
        let durable_stream_fork_result = Arc::new(Mutex::new(None));
        let durable_stream_reads = Arc::new(Mutex::new(VecDeque::new()));
        let file_reads = Arc::new(FileReadMock::default());
        let creates = Arc::new(CreateMock::default());
        let contexts = Arc::new(Mutex::new(Vec::new()));
        let native_sessions = Arc::new(Mutex::new(NativeSessionLog {
            mode: NativeSessions::Refuse,
            keys: Vec::new(),
            starts: Vec::new(),
            public_session_creation: None,
            appends: Vec::new(),
            tails: VecDeque::new(),
            releases: VecDeque::new(),
            opened: tokio::sync::watch::Sender::new(0),
        }));
        let worker_client = Arc::new(RecordingWorkerClient {
            component_id,
            agent_ids: agent_ids.clone(),
            targets: targets.clone(),
            metadata: metadata.clone(),
            metadata_reads: metadata_reads.clone(),
            prepared_agent_ids,
            method_params: method_params.clone(),
            durable_stream_controls: durable_stream_controls.clone(),
            durable_stream_forks: durable_stream_forks.clone(),
            durable_stream_fork_result: durable_stream_fork_result.clone(),
            durable_stream_reads: durable_stream_reads.clone(),
            contexts: contexts.clone(),
            invocation_output,
            file_reads: file_reads.clone(),
            creates: creates.clone(),
            native_sessions: native_sessions.clone(),
        });

        let type_id = TypeId::try_from(42u32).unwrap();
        let registry = Arc::new(HarnessRegistryService {
            environment_id,
            registered: RegisteredAgentType {
                type_id,
                agent_type: agent_type.clone(),
                implemented_by: RegisteredAgentTypeImplementer {
                    component_id,
                    component_revision: ComponentRevision::INITIAL,
                    component_name: component.component_name.0.clone(),
                    account_id,
                    account_email: account_email.clone(),
                },
            },
        });

        Self {
            worker_service: Arc::new(WorkerService::new(
                Arc::new(StaticComponentService {
                    tool_state: crate::service::component::tests::tool_deployment_state(
                        &component,
                        "native",
                        native_tool_definition(),
                        [
                            golem_common::model::tool::ToolBindingOwner::ComponentBaseline {
                                component_id,
                            },
                        ],
                    ),
                    component,
                }),
                Arc::new(AllowAllAuthService),
                Arc::new(NoopLimitService),
                worker_client,
                Arc::new(AgentResolutionCache::new(
                    registry.clone(),
                    1,
                    Duration::from_secs(60),
                    Duration::from_secs(60),
                )),
                Arc::new(AgentTypeResolver::new(
                    registry,
                    1,
                    Duration::from_secs(60),
                    Duration::from_secs(60),
                )),
                "https://golem.test".parse().unwrap(),
            )),
            component_id,
            environment_id,
            account_id,
            account_email,
            file_reads,
            creates,
            metadata,
            metadata_reads,
            agent_ids,
            method_params,
            durable_stream_controls,
            durable_stream_forks,
            durable_stream_fork_result,
            durable_stream_reads,
            contexts,
            native_sessions,
            agent_type,
            type_id,
            targets,
        }
    }

    /// An MCP deployment of `domain` at `revision` that exports the harness's agent type and,
    /// when `native` is set, one native tool of the harness's component.
    pub(crate) fn compiled_mcp(
        &self,
        domain: &str,
        revision: golem_common::model::deployment::DeploymentRevision,
        native: bool,
    ) -> golem_service_base::mcp::CompiledMcp {
        golem_service_base::mcp::CompiledMcp {
            account_id: self.account_id,
            account_email: self.account_email.clone(),
            application_name: ApplicationName::try_from("test-app".to_string()).unwrap(),
            environment_id: self.environment_id,
            environment_name: EnvironmentName::try_from("test-env").unwrap(),
            deployment_revision: revision,
            domain: golem_common::model::domain_registration::Domain(domain.to_string()),
            security_scheme_name: None,
            security_scheme: None,
            registered_agent_types: vec![golem_common::schema::RegisteredAgentTypeSchema {
                agent_type: self.agent_type.clone(),
                implemented_by: golem_common::model::agent::RegisteredAgentTypeImplementer {
                    component_id: self.component_id,
                    component_revision: ComponentRevision::INITIAL,
                    component_name: "mcp-component".to_string(),
                    account_id: self.account_id,
                    account_email: self.account_email.clone(),
                },
            }],
            tools: if native {
                vec![self.native_export()]
            } else {
                vec![]
            },
        }
    }

    /// A native tool `native` of the harness's component, without input or output streams.
    fn native_export(&self) -> golem_service_base::mcp::CompiledMcpToolExport {
        let mut export = golem_service_base::mcp::native_tool::compile_native_tool_exports(
            self.component_id,
            "test:owner".try_into().unwrap(),
            "native".try_into().unwrap(),
            &native_tool_definition(),
            None,
            None,
        )
        .unwrap()
        .remove(0);
        export.owner_component_name = ComponentName("mcp-component".to_string());
        export
    }

    /// Answers the next invocation sessions as `mode` says.
    pub(crate) fn answer_native_sessions(&self, mode: NativeSessions) {
        self.native_sessions.lock().unwrap().mode = mode;
    }

    /// Waits until `count` invocation sessions were received.
    pub(crate) async fn native_sessions_opened(&self, count: usize) {
        let mut opened = self.native_sessions.lock().unwrap().opened.subscribe();
        tokio::time::timeout(
            Duration::from_secs(5),
            opened.wait_for(|opened| *opened >= count),
        )
        .await
        .expect("the native sessions open")
        .unwrap();
    }

    /// The idempotency keys of the invocation sessions received so far.
    pub(crate) fn native_session_keys(&self) -> Vec<Arc<str>> {
        self.native_sessions.lock().unwrap().keys.clone()
    }

    /// Lets the oldest session held until released answer with its successful tool result.
    pub(crate) fn release_native_session(&self) {
        let release = self
            .native_sessions
            .lock()
            .unwrap()
            .releases
            .pop_front()
            .expect("a native session held until released");
        let _ = release.send(());
    }

    /// The request stream of the oldest held session, after its start.
    pub(crate) fn take_native_session_tail(
        &self,
    ) -> crate::service::worker::InvocationRequestStream {
        self.native_sessions
            .lock()
            .unwrap()
            .tails
            .pop_front()
            .expect("a held native session")
    }

    pub(crate) fn recorded_agent_id(&self) -> AgentId {
        self.agent_ids.lock().unwrap()[0]
    }

    /// The target of the first invocation.
    pub(crate) fn recorded_target(&self) -> RoutedTarget {
        self.targets.lock().unwrap()[0].clone()
    }

    /// The constructor of the first invocation's target, and whether the target is the keyed
    /// agent of that constructor rather than an agent at a given ID.
    pub(crate) fn recorded_constructor(&self) -> (golem_common::model::agent::AgentKey, bool) {
        use golem_api_grpc::proto::golem::worker::agent_target::Target;
        let (target, constructor, _) = self.recorded_target().invoke_fields();
        match (target.target, constructor) {
            (Some(Target::Key(key)), None) => (key.try_into().unwrap(), true),
            (Some(Target::Id(_)), Some(constructor)) => (constructor.try_into().unwrap(), false),
            other => panic!("the target has no constructor: {other:?}"),
        }
    }

    pub(crate) fn recorded_method_inputs(
        &self,
    ) -> Vec<Option<golem_common::model::agent::AgentMethodInput>> {
        self.method_params.lock().unwrap().clone()
    }

    /// The native method parameters recorded for the first invocation.
    pub(crate) fn recorded_method_params(&self) -> golem_common::schema::SchemaValue {
        let golem_common::model::agent::AgentMethodInput::Native(input) =
            self.method_params.lock().unwrap()[0]
                .clone()
                .expect("method params were recorded")
        else {
            panic!("expected native MCP input")
        };
        input
            .decode(|_, _| Err("unexpected MCP stream".into()))
            .unwrap()
    }

    pub(crate) fn recorded_durable_stream_controls(
        &self,
    ) -> Vec<golem_api_grpc::proto::golem::workerexecutor::v1::DurableStreamAttachmentControlRequest>
    {
        self.durable_stream_controls.lock().unwrap().clone()
    }

    pub(crate) fn recorded_durable_stream_forks(
        &self,
    ) -> Vec<golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotRequest> {
        self.durable_stream_forks.lock().unwrap().clone()
    }

    pub(crate) fn script_durable_stream_fork_success(
        &self,
        success: golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotSuccess,
        reads: Vec<golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotSuccess>,
    ) {
        *self.durable_stream_fork_result.lock().unwrap() = Some(
            golem_api_grpc::proto::golem::workerexecutor::v1::fork_stream_slot_response::Result::Success(
                success,
            ),
        );
        self.durable_stream_reads
            .lock()
            .unwrap()
            .extend(reads.into_iter().map(Some));
    }

    pub(crate) fn script_durable_stream_reads(
        &self,
        reads: Vec<Option<golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotSuccess>>,
    ) {
        self.durable_stream_reads.lock().unwrap().extend(reads);
    }
}

/// The answers of an executor to the external-tool session `start` that succeeds without a
/// result value or byte streams: its acceptance, its result and its completion. The executor
/// names the agent that runs the tool `AgentId::from_parts(0, 20)`.
fn succeeded_tool_session(
    start: &golem_api_grpc::proto::golem::worker::InvocationStart,
) -> [golem_api_grpc::proto::golem::worker::InvocationResponse; 3] {
    use golem_api_grpc::proto::golem::worker::{
        InvocationAccepted, InvocationResponse, InvocationSessionCompletion,
        InvocationSessionResult, PublicExternalToolResult, PublicToolInvocationResult,
        invocation_response, invocation_session_completion, invocation_session_result,
        public_external_tool_result,
    };
    let invocation_id = Some(golem_schema::proto::golem::common::Uuid {
        high_bits: 0,
        low_bits: 1,
    });
    let agent_id = Some(AgentId::from_parts(0, 20).into());
    let tool = start.external_tool.clone().unwrap_or_default();
    [
        invocation_response::Response::Accepted(InvocationAccepted {
            agent_id,
            idempotency_key: invocation_id,
            tool_name: Some(tool.tool_name),
            command_path: tool.command_path,
            ..Default::default()
        }),
        invocation_response::Response::Result(InvocationSessionResult {
            agent_id,
            idempotency_key: invocation_id,
            result: Some(invocation_session_result::Result::ToolResult(
                PublicExternalToolResult {
                    result: Some(public_external_tool_result::Result::Success(
                        PublicToolInvocationResult { result: None },
                    )),
                },
            )),
            ..Default::default()
        }),
        invocation_response::Response::Finished(InvocationSessionCompletion {
            outcome: Some(invocation_session_completion::Outcome::Success(
                Default::default(),
            )),
        }),
    ]
    .map(|response| InvocationResponse {
        response: Some(response),
    })
}

/// The JSON-RPC messages of a streamed MCP response, which must have status 200.
pub(crate) async fn streamed_messages(response: poem::Response) -> Vec<serde_json::Value> {
    let status = response.status();
    let text = response.into_body().into_string().await.unwrap();
    assert_eq!(status, poem::http::StatusCode::OK, "{text}");
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}
