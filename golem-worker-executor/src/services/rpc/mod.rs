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

use super::agent_webhooks::AgentWebhooksService;
use super::direct_invocation_auth::DirectInvocationAuthService;
use super::environment_state::EnvironmentStateService;
use super::external_durable_stream::ExternalDurableStreamService;
use super::file_loader::FileLoader;
use super::{
    HasAgentWebhooksService, HasEnvironmentStateService, HasExternalDurableStreamService,
    HasMcpTransport, HasWebSocketConnectionPool,
};
use crate::durable_host::durable_session::durable_stream_mapping_to_proto;
use crate::durable_host::websocket::WebSocketConnectionPool;
use crate::grpc::{build_durable_streaming_request, decode_invocation_input};
use crate::services::agent_filesystem_snapshots::AgentFilesystemSnapshots;
use crate::services::events::Events;
use crate::services::oplog::plugin::OplogProcessorPlugin;
use crate::services::resource_limits::ResourceLimits;
use crate::services::shard::ShardService;
use crate::services::worker_proxy::{InvocationResponseStream, WorkerProxy, WorkerProxyError};
use crate::services::{
    HasActiveAgents, HasAgentFilesystemSnapshots, HasAgentTypesService, HasBlobStoreService,
    HasCardService, HasComponentService, HasConfig, HasEvents, HasExtraDeps, HasFileLoader,
    HasHttpConnectionPool, HasKeyValueService, HasLeakSentinel, HasNativeToolCatalog,
    HasOplogProcessorPlugin, HasOplogService, HasPromiseService, HasQuotaService, HasRdbmsService,
    HasResourceLimits, HasRpc, HasRunningWorkerEnumerationService, HasSchedulerService,
    HasShardManagerService, HasShardService, HasShutdownToken, HasWasmtimeEngine,
    HasWorkerActivator, HasWorkerEnumerationService, HasWorkerForkService, HasWorkerProxy,
    HasWorkerService, active_agents, agent_types, blob_store, card, component, golem_config,
    key_value, oplog, promise, rdbms, scheduler, shard_manager, worker, worker_activator,
    worker_enumeration, worker_fork,
};
use crate::worker::Worker;
use crate::worker::invocation::validate_agent_method_invocation;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use futures::StreamExt;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, InvocationAccepted, InvocationFailure, InvocationFailureKind,
    InvocationRejected, InvocationRejectionReason, InvocationRequest, InvocationStart,
    ResumeAttach, ResumeOperation, StreamInvocationIdentity, invocation_request,
    invocation_response, invocation_session_completion, invocation_session_result,
};
use golem_common::base_model::durable_stream::{
    DurableStreamReadRequest, StreamAttachmentControlRequest,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::{
    AgentError as ModelAgentError, AgentInvocationMode, AgentPrincipal,
    InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::card::{AgentMethodName, AgentResourcePattern, AgentVerb, ScopeCard};
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, AgentInvocationResult, IdempotencyKey, OwnedAgentId,
};
use golem_common::schema::SchemaValue;
use golem_schema::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;
use wasmtime_wasi_http::HttpConnectionPool;

/// Selects the component revision an invocation's method is validated against: `None` for the
/// deployed revision, `Some` for the revision an existing agent is pinned to.
///
/// `KnownFresh` has already been proven to name no existing agent, so the deployed revision is
/// selected without a lookup. Otherwise the existing agent's revision is loaded, and a lookup
/// that *fails* is propagated rather than folded into `None`. `None` is not a safe default here:
/// an existing agent pinned to an older revision, validated against the deployed one, can have a
/// method it does have rejected before execution is even attempted, and a storage outage would
/// then surface as a protocol error.
async fn method_validation_revision<F, Fut>(
    freshness_disposition: InvocationFreshnessDisposition,
    load_existing_revision: F,
) -> Result<Option<ComponentRevision>, WorkerExecutorError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Option<ComponentRevision>, WorkerExecutorError>>,
{
    if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
        Ok(None)
    } else {
        load_existing_revision().await
    }
}

#[async_trait]
pub trait Rpc: Send + Sync {
    async fn create_demand(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, RpcError>;

    async fn invoke_and_await(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError>;

    async fn invoke_and_await_streaming(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: IdempotencyKey,
        _method_name: String,
        _method_parameters: ProtoSchemaValue,
        _input_mappings: Vec<DurableStreamMapping>,
        _expected_callee_fingerprint: AgentFingerprint,
        _attempt_id: uuid::Uuid,
        _origin_invocation: StreamInvocationIdentity,
        _accepted_inputs: tokio::sync::oneshot::Sender<InvocationAccepted>,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _scope_card: Option<ScopeCard>,
    ) -> Result<DurableRpcInvocationResult, RpcError> {
        Err(RpcError::ProtocolError {
            details: "durable streaming invocation is not supported by this RPC implementation"
                .to_string(),
        })
    }

    async fn control_durable_stream_attachment(
        &self,
        _request: StreamAttachmentControlRequest,
        _auth_ctx: &AuthCtx,
    ) -> Result<bool, DurableStreamRemoteError<RpcError>> {
        Err(RpcError::ProtocolError {
            details:
                "durable stream attachment control is not supported by this RPC implementation"
                    .to_string(),
        }
        .into())
    }

    async fn read_durable_stream_segment(
        &self,
        _request: DurableStreamReadRequest,
        _auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<RpcError>> {
        Err(RpcError::ProtocolError {
            details: "durable stream segment reads are not supported by this RPC implementation"
                .to_string(),
        }
        .into())
    }

    async fn invoke(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RpcError>;
}

pub struct DurableRpcInvocationResult {
    pub value: ProtoSchemaValue,
    pub output_mappings: Vec<DurableStreamMapping>,
}

/// Transport-level failure of a read or control sent to a remote durable stream producer, kept
/// separate from durable invocation errors. `Unavailable` means the producer is temporarily
/// unreachable (recovering, fenced, being deleted or migrating) and the operation may be retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableStreamRemoteError<E> {
    Unavailable,
    Other(E),
}

impl<E> From<E> for DurableStreamRemoteError<E> {
    fn from(error: E) -> Self {
        Self::Other(error)
    }
}

impl<E> DurableStreamRemoteError<E> {
    pub fn map_other<F>(self, map: impl FnOnce(E) -> F) -> DurableStreamRemoteError<F> {
        match self {
            Self::Unavailable => DurableStreamRemoteError::Unavailable,
            Self::Other(error) => DurableStreamRemoteError::Other(map(error)),
        }
    }

    /// Runs `operation` until it succeeds or fails with anything but `Unavailable`, backing off
    /// from 100ms up to 1s between attempts. Callers bound the wait if the target may never
    /// come back.
    pub(crate) async fn retry_while_unavailable<T, Fut>(
        mut operation: impl FnMut() -> Fut,
    ) -> Result<T, E>
    where
        Fut: Future<Output = Result<T, Self>>,
    {
        let mut delay = std::time::Duration::from_millis(100);
        loop {
            match operation().await {
                Ok(value) => return Ok(value),
                Err(Self::Other(error)) => return Err(error),
                Err(Self::Unavailable) => {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(std::time::Duration::from_secs(1));
                }
            }
        }
    }

    pub(crate) fn from_producer(
        error: crate::durable_host::durable_stream::StreamStoreError,
        map: impl FnOnce(String) -> E,
    ) -> Self {
        match error {
            // A fenced store is as unavailable here as one awaiting recovery: the stream lives on
            // with the shard's new owner.
            crate::durable_host::durable_stream::StreamStoreError::RecoveryRequired
            | crate::durable_host::durable_stream::StreamStoreError::Fenced(_) => Self::Unavailable,
            error => Self::Other(map(error.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
    RemoteAgentError { error: Box<ModelAgentError> },
}

impl From<SerializableRpcError> for RpcError {
    fn from(value: SerializableRpcError) -> Self {
        match value {
            SerializableRpcError::ProtocolError { details } => Self::ProtocolError { details },
            SerializableRpcError::Denied { details } => Self::Denied { details },
            SerializableRpcError::NotFound { details } => Self::NotFound { details },
            SerializableRpcError::RemoteInternalError { details } => {
                Self::RemoteInternalError { details }
            }
            SerializableRpcError::RemoteAgentError { error } => Self::RemoteAgentError { error },
        }
    }
}

impl From<RpcError> for SerializableRpcError {
    fn from(value: RpcError) -> Self {
        match value {
            RpcError::ProtocolError { details } => SerializableRpcError::ProtocolError { details },
            RpcError::Denied { details } => SerializableRpcError::Denied { details },
            RpcError::NotFound { details } => SerializableRpcError::NotFound { details },
            RpcError::RemoteInternalError { details } => {
                SerializableRpcError::RemoteInternalError { details }
            }
            RpcError::RemoteAgentError { error } => {
                SerializableRpcError::RemoteAgentError { error }
            }
        }
    }
}

impl Display for RpcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::ProtocolError { details } => write!(f, "Protocol error: {details}"),
            RpcError::Denied { details } => write!(f, "Denied: {details}"),
            RpcError::NotFound { details } => write!(f, "Not found: {details}"),
            RpcError::RemoteInternalError { details } => {
                write!(f, "Remote internal error: {details}")
            }
            RpcError::RemoteAgentError { error } => write!(f, "Remote agent error: {error}"),
        }
    }
}

impl std::error::Error for RpcError {}

impl From<tonic::transport::Error> for RpcError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::ProtocolError {
            details: format!("gRPC Transport error: {value}"),
        }
    }
}

impl From<tonic::Status> for RpcError {
    fn from(value: tonic::Status) -> Self {
        Self::ProtocolError {
            details: format!("gRPC error: {value}"),
        }
    }
}

impl From<WorkerExecutorError> for RpcError {
    fn from(value: WorkerExecutorError) -> Self {
        match value {
            WorkerExecutorError::AgentAlreadyExists { agent_id } => RpcError::Denied {
                details: format!("Worker {agent_id} already exists"),
            },
            WorkerExecutorError::AgentNotFound { agent_id } => RpcError::NotFound {
                details: format!("Worker {agent_id} not found"),
            },
            WorkerExecutorError::ComponentNotFound { component_id } => RpcError::NotFound {
                details: format!("Component {component_id} not found"),
            },
            WorkerExecutorError::InvalidAccount => RpcError::Denied {
                details: "Invalid account".to_string(),
            },
            WorkerExecutorError::PermissionDenied { details } => RpcError::Denied { details },
            WorkerExecutorError::InvalidRequest { details } => RpcError::RemoteAgentError {
                error: Box::new(ModelAgentError::InvalidInput(details)),
            },
            _ => RpcError::RemoteInternalError {
                details: value.to_string(),
            },
        }
    }
}

impl From<WorkerProxyError> for RpcError {
    fn from(value: WorkerProxyError) -> Self {
        match value {
            WorkerProxyError::BadRequest(errors) => RpcError::RemoteAgentError {
                error: Box::new(ModelAgentError::InvalidInput(errors.join(", "))),
            },
            WorkerProxyError::Unauthorized(error) => RpcError::Denied { details: error },
            WorkerProxyError::LimitExceeded(error) => RpcError::Denied { details: error },
            WorkerProxyError::NotFound(error) => RpcError::NotFound { details: error },
            WorkerProxyError::AlreadyExists(error) => RpcError::Denied { details: error },
            WorkerProxyError::InternalError(error) => error.into(),
        }
    }
}

impl From<crate::preview2::golem::agent::host::RpcError> for RpcError {
    fn from(value: crate::preview2::golem::agent::host::RpcError) -> Self {
        use crate::preview2::golem::agent::host::RpcError as WitRpcError;
        match value {
            WitRpcError::ProtocolError(details) => Self::ProtocolError { details },
            WitRpcError::Denied(details) => Self::Denied { details },
            WitRpcError::NotFound(details) => Self::NotFound { details },
            WitRpcError::RemoteInternalError(details) => Self::RemoteInternalError { details },
            WitRpcError::RemoteAgentError(err) => {
                match golem_common::schema::agent::wit::decode_agent_error(err) {
                    Ok(error) => Self::RemoteAgentError {
                        error: Box::new(error),
                    },
                    Err(err) => Self::RemoteInternalError {
                        details: format!("Failed to decode remote agent error: {err}"),
                    },
                }
            }
        }
    }
}

impl From<RpcError> for crate::preview2::golem::agent::host::RpcError {
    fn from(value: RpcError) -> Self {
        match value {
            RpcError::ProtocolError { details } => Self::ProtocolError(details),
            RpcError::Denied { details } => Self::Denied(details),
            RpcError::NotFound { details } => Self::NotFound(details),
            RpcError::RemoteInternalError { details } => Self::RemoteInternalError(details),
            RpcError::RemoteAgentError { error } => {
                match golem_common::schema::agent::wit::encode_agent_error(&error) {
                    Ok(error) => Self::RemoteAgentError(error),
                    Err(err) => Self::RemoteInternalError(format!(
                        "Failed to encode remote agent error: {err}"
                    )),
                }
            }
        }
    }
}

pub trait RpcDemand: Send + Sync {
    /// The fingerprint of the target worker this demand was established for.
    fn fingerprint(&self) -> AgentFingerprint;
}

pub struct RemoteInvocationRpc {
    worker_proxy: Arc<dyn WorkerProxy>,
    _shard_service: Arc<dyn ShardService>,
}

struct LoggingDemand {
    agent_id: AgentId,
    fingerprint: AgentFingerprint,
}

pub struct ReplayedDemand {
    fingerprint: AgentFingerprint,
}

impl ReplayedDemand {
    pub fn new(fingerprint: AgentFingerprint) -> Self {
        Self { fingerprint }
    }
}

impl RpcDemand for ReplayedDemand {
    fn fingerprint(&self) -> AgentFingerprint {
        self.fingerprint
    }
}

impl LoggingDemand {
    pub fn new(agent_id: AgentId, fingerprint: AgentFingerprint) -> Self {
        log::debug!("Initializing RPC connection for worker {agent_id}");
        Self {
            agent_id,
            fingerprint,
        }
    }
}

impl RpcDemand for LoggingDemand {
    fn fingerprint(&self) -> AgentFingerprint {
        self.fingerprint
    }
}

impl Drop for LoggingDemand {
    fn drop(&mut self) {
        log::debug!("Dropping RPC connection for worker {}", self.agent_id);
    }
}

fn rpc_error_from_failure(failure: InvocationFailure) -> RpcError {
    if let Some(worker_error) = failure.worker_error {
        return WorkerExecutorError::try_from(worker_error)
            .map(Into::into)
            .unwrap_or_else(|error| RpcError::RemoteInternalError {
                details: format!("failed to decode worker execution error: {error}"),
            });
    }
    match InvocationFailureKind::try_from(failure.kind).unwrap_or(InvocationFailureKind::Internal) {
        InvocationFailureKind::Protocol | InvocationFailureKind::Transport => {
            RpcError::ProtocolError {
                details: failure.message,
            }
        }
        InvocationFailureKind::Execution | InvocationFailureKind::Internal => {
            RpcError::RemoteInternalError {
                details: failure.message,
            }
        }
        InvocationFailureKind::Unspecified => RpcError::ProtocolError {
            details: failure.message,
        },
    }
}

fn caller_agent_principal(self_agent_id: &AgentId) -> Principal {
    Principal::Agent(AgentPrincipal {
        agent_id: self_agent_id.clone(),
    })
}

pub struct DirectWorkerInvocationRpc<Ctx: WorkerCtx> {
    remote_rpc: Arc<RemoteInvocationRpc>,
    direct_invocation_auth: Arc<dyn DirectInvocationAuthService>,
    active_agents: Arc<active_agents::ActiveAgents<Ctx>>,
    engine: Arc<wasmtime::Engine>,
    linker: Arc<wasmtime::component::Linker<Ctx>>,
    runtime: Handle,
    card_service: Arc<dyn card::CardService>,
    component_service: Arc<dyn component::ComponentService>,
    shard_manager_service: Arc<dyn shard_manager::ShardManagerService>,
    quota_service: Arc<dyn crate::services::quota::QuotaService>,
    worker_fork: Arc<dyn worker_fork::WorkerForkService>,
    worker_service: Arc<dyn worker::WorkerService>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService>,
    promise_service: Arc<dyn promise::PromiseService>,
    golem_config: Arc<golem_config::GolemConfig>,
    shard_service: Arc<dyn ShardService>,
    key_value_service: Arc<dyn key_value::KeyValueService>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService>,
    rdbms_service: Arc<dyn rdbms::RdbmsService>,
    oplog_service: Arc<dyn oplog::OplogService>,
    scheduler_service: Arc<dyn scheduler::SchedulerService>,
    worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx>>,
    events: Arc<Events>,
    file_loader: Arc<FileLoader>,
    oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
    resource_limits: Arc<dyn ResourceLimits>,
    shutdown_token: tokio_util::sync::CancellationToken,
    environment_state_service: Arc<dyn EnvironmentStateService>,
    agent_filesystem_snapshots: Arc<AgentFilesystemSnapshots>,
    native_tool_catalog: Arc<crate::native_tool::NativeToolCatalog<Ctx>>,
    agent_types_service: Arc<dyn agent_types::AgentTypesService>,
    agent_webhooks_service: Arc<AgentWebhooksService>,
    external_durable_streams: Arc<dyn ExternalDurableStreamService>,
    http_connection_pool: Option<HttpConnectionPool>,
    websocket_connection_pool: WebSocketConnectionPool,
    mcp_transport: Arc<super::mcp::McpTransport>,
    extra_deps: Ctx::ExtraDeps,
    leak_sentinel: Arc<()>,
}

mod dependencies;
mod direct;
mod remote;

#[cfg(test)]
#[path = "tests.rs"]
mod protocol_tests;
