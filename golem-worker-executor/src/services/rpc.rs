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
use super::file_loader::FileLoader;
use super::{HasAgentWebhooksService, HasEnvironmentStateService, HasWebSocketConnectionPool};
use crate::durable_host::stream_session::LiveValueSession;
use crate::durable_host::websocket::WebSocketConnectionPool;
use crate::services::events::Events;
use crate::services::oplog::plugin::OplogProcessorPlugin;
use crate::services::resource_limits::ResourceLimits;
use crate::services::shard::ShardService;
use crate::services::worker_proxy::{InvocationResponseStream, WorkerProxy, WorkerProxyError};
use crate::services::{
    HasActiveWorkers, HasAgentTypesService, HasBlobStoreService, HasCardService,
    HasComponentService, HasConfig, HasEvents, HasExtraDeps, HasFileLoader, HasHttpConnectionPool,
    HasKeyValueService, HasLeakSentinel, HasOplogProcessorPlugin, HasOplogService,
    HasPromiseService, HasQuotaService, HasRdbmsService, HasResourceLimits, HasRpc,
    HasRunningWorkerEnumerationService, HasSchedulerService, HasShardManagerService,
    HasShardService, HasShutdownToken, HasWasmtimeEngine, HasWorkerActivator,
    HasWorkerEnumerationService, HasWorkerForkService, HasWorkerProxy, HasWorkerService,
    active_workers, agent_types, blob_store, card, component, golem_config, key_value, oplog,
    promise, rdbms, scheduler, shard_manager, worker, worker_activator, worker_enumeration,
    worker_fork,
};
use crate::worker::Worker;
use crate::worker::invocation::validate_agent_method_invocation;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use futures::StreamExt;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::{
    InvocationFailure, InvocationFailureKind, InvocationRejected, InvocationRejectionReason,
    InvocationRequest, InvocationStart, invocation_request, invocation_response,
    invocation_session_completion, invocation_session_result,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentInvocationMode, AgentPrincipal, InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::card::{AgentMethodName, AgentResourcePattern, AgentVerb};
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, AgentInvocationResult, IdempotencyKey, OwnedAgentId,
};
use golem_common::schema::SchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;
use wasmtime_wasi_http::HttpConnectionPool;

async fn method_validation_revision<F, Fut>(
    freshness_disposition: InvocationFreshnessDisposition,
    load_existing_revision: F,
) -> Option<ComponentRevision>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Option<ComponentRevision>>,
{
    if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
        None
    } else {
        load_existing_revision().await
    }
}

#[async_trait]
pub trait Rpc: Send + Sync {
    async fn create_demand(
        &self,
        owned_agent_id: &OwnedAgentId,
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
    ) -> Result<SchemaValue, RpcError>;

    /// Executes an awaited invocation whose recursive value tree contains live
    /// streams. This is a non-durable session: implementations must not route
    /// the value through ordinary protobuf/oplog serialization.
    async fn invoke_and_await_streaming(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
    ) -> Result<SchemaValue, RpcError> {
        Err(RpcError::ProtocolError {
            details: "live streaming invocation is not supported by this RPC implementation"
                .to_string(),
        })
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

struct RemoteLiveRequestGuard {
    session: LiveValueSession,
    requests: Option<mpsc::Sender<InvocationRequest>>,
}

impl RemoteLiveRequestGuard {
    fn new(session: LiveValueSession, requests: mpsc::Sender<InvocationRequest>) -> Self {
        Self {
            session,
            requests: Some(requests),
        }
    }

    fn disarm(&mut self) {
        self.requests = None;
    }
}

impl Drop for RemoteLiveRequestGuard {
    fn drop(&mut self) {
        let Some(requests) = self.requests.take() else {
            return;
        };
        if self.session.is_cancelled() {
            return;
        }
        self.session.cancel();
        drop(requests);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
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
            WorkerExecutorError::InvalidRequest { details } => RpcError::ProtocolError { details },
            _ => RpcError::RemoteInternalError {
                details: value.to_string(),
            },
        }
    }
}

impl From<WorkerProxyError> for RpcError {
    fn from(value: WorkerProxyError) -> Self {
        match value {
            WorkerProxyError::BadRequest(errors) => RpcError::ProtocolError {
                details: errors.join(", "),
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
            WitRpcError::RemoteAgentError(err) => Self::RemoteInternalError {
                details: format!("{err:?}"),
            },
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
    stream_capacity: usize,
}

impl RemoteInvocationRpc {
    pub fn new(worker_proxy: Arc<dyn WorkerProxy>, shard_service: Arc<dyn ShardService>) -> Self {
        Self::new_with_stream_capacity(worker_proxy, shard_service, 32)
    }

    pub fn new_with_stream_capacity(
        worker_proxy: Arc<dyn WorkerProxy>,
        shard_service: Arc<dyn ShardService>,
        stream_capacity: usize,
    ) -> Self {
        assert!(
            stream_capacity > 0,
            "live stream bus capacity must be non-zero"
        );
        Self {
            worker_proxy,
            _shard_service: shard_service,
            stream_capacity,
        }
    }
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

/// Rpc implementation simply calling the public Golem Worker API for invocation
#[async_trait]
impl Rpc for RemoteInvocationRpc {
    async fn create_demand(
        &self,
        owned_agent_id: &OwnedAgentId,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        debug!("Ensuring remote target worker exists");

        let principal = caller_agent_principal(self_agent_id);

        let fingerprint = self
            .worker_proxy
            .start(
                owned_agent_id,
                self_agent_id,
                HashMap::from_iter(self_env.to_vec()),
                self_stack,
                config,
                principal,
                auth_ctx,
            )
            .await?;

        Ok(Box::new(LoggingDemand::new(
            owned_agent_id.agent_id(),
            fingerprint,
        )))
    }

    async fn invoke_and_await(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<SchemaValue, RpcError> {
        let principal = caller_agent_principal(self_agent_id);

        let output = self
            .worker_proxy
            .invoke_agent(
                &owned_agent_id.agent_id(),
                method_name,
                method_parameters,
                AgentInvocationMode::Await,
                None,
                idempotency_key,
                freshness_disposition,
                self_agent_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_stack,
                config,
                principal,
                owned_agent_id.environment_id,
                auth_ctx,
            )
            .await?;

        match output.result {
            golem_common::model::AgentInvocationResult::AgentMethod { output } => Ok(output),
            _ => Err(RpcError::RemoteInternalError {
                details:
                    "Expected a result from agent invoke_and_await but got a non-method result"
                        .to_string(),
            }),
        }
    }

    async fn invoke_and_await_streaming(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: SchemaValue,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<SchemaValue, RpcError> {
        let state = Arc::new(tokio::sync::Mutex::new(InvocationSessionState::default()));
        let (requests, mut request_rx) = mpsc::channel(32);
        let (wire_requests, receiver) = mpsc::channel(32);
        let request_state = state.clone();
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                if request_state
                    .lock()
                    .await
                    .validate_trusted_request(&request)
                    .is_err()
                {
                    return;
                }
                if wire_requests.send(request).await.is_err() {
                    return;
                }
            }
        });
        let session =
            LiveValueSession::new_client_with_capacity(requests.clone(), self.stream_capacity);
        let (input, input_stream_ids) = session
            .encode_pending(&method_parameters)
            .map_err(|details| RpcError::ProtocolError { details })?;
        requests
            .send(InvocationRequest {
                request: Some(invocation_request::Request::Start(InvocationStart {
                    agent_id: Some(owned_agent_id.agent_id().into()),
                    method_name: Some(method_name),
                    input: Some(input),
                    idempotency_key: idempotency_key.map(Into::into),
                    context: Some(golem_api_grpc::proto::golem::worker::InvocationContext {
                        parent: Some(self_agent_id.clone().into()),
                        env: HashMap::from_iter(self_env.to_vec()),
                        tracing: Some(self_stack.into()),
                    }),
                    auth_ctx: Some(auth_ctx.clone().into()),
                    principal: Some(caller_agent_principal(self_agent_id).into()),
                    environment_id: Some(owned_agent_id.environment_id.into()),
                    config: config.into_iter().map(Into::into).collect(),
                    component_owner_account_id: None,
                    mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                    schedule_at: None,
                    freshness_disposition: golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                        as i32,
                })),
            })
            .await
            .map_err(|_| RpcError::ProtocolError {
                details: "live invocation request ended before start".to_string(),
            })?;
        let mut cancel_on_drop = RemoteLiveRequestGuard::new(session.clone(), requests.clone());
        let mut inbound = match self
            .worker_proxy
            .invoke_agent_session(Box::pin(ReceiverStream::new(receiver)))
            .await
        {
            Ok(inbound) => inbound,
            Err(error) => {
                session.cancel();
                return Err(error.into());
            }
        };

        while let Some(response) = inbound.next().await {
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    let details = error.to_string();
                    fail_live_response_transport(&session, details).await;
                    return Err(error.into());
                }
            };
            if let Err(details) = state.lock().await.validate_response(&response) {
                fail_live_response_transport(&session, details.clone()).await;
                return Err(RpcError::ProtocolError { details });
            }
            match response.response {
                Some(invocation_response::Response::Accepted(_)) => {
                    session.activate_exported_streams(&input_stream_ids);
                }
                Some(invocation_response::Response::Rejected(rejected)) => {
                    if let Err(error) =
                        confirm_terminal_response_is_last(&mut inbound, &state).await
                    {
                        session.fail(error.to_string());
                        return Err(error);
                    }
                    let error = rpc_error_from_rejection(rejected);
                    session.fail(error.to_string());
                    return Err(error);
                }
                Some(invocation_response::Response::Result(result)) => {
                    let output = match result.result {
                        Some(invocation_session_result::Result::MethodResult(output)) => {
                            match session.decode(output).await {
                                Ok(output) => output,
                                Err(details) => {
                                    fail_live_response_transport(&session, details.clone()).await;
                                    return Err(RpcError::ProtocolError { details });
                                }
                            }
                        }
                        Some(invocation_session_result::Result::NoResult(_)) | None => {
                            let details =
                                "streaming agent invocation returned no method result".to_string();
                            fail_live_response_transport(&session, details.clone()).await;
                            return Err(RpcError::ProtocolError { details });
                        }
                    };
                    spawn_live_response_router(session, inbound, requests, state);
                    cancel_on_drop.disarm();
                    return Ok(output);
                }
                Some(invocation_response::Response::Finished(finished)) => {
                    if let Err(error) =
                        confirm_terminal_response_is_last(&mut inbound, &state).await
                    {
                        session.fail(error.to_string());
                        return Err(error);
                    }
                    let error = rpc_error_from_invocation_finished(finished);
                    session.fail(error.to_string());
                    return Err(error);
                }
                Some(response) => match session.route_response(response).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let details =
                            "unexpected response before the invocation result".to_string();
                        fail_live_response_transport(&session, details.clone()).await;
                        return Err(RpcError::ProtocolError { details });
                    }
                    Err(details) => {
                        fail_live_response_transport(&session, details.clone()).await;
                        return Err(RpcError::ProtocolError { details });
                    }
                },
                None => unreachable!("response state validation rejects empty frames"),
            }
        }
        let details = "invocation response ended before publishing a result".to_string();
        fail_live_response_transport(&session, details.clone()).await;
        Err(RpcError::ProtocolError { details })
    }

    async fn invoke(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RpcError> {
        let principal = caller_agent_principal(self_agent_id);

        self.worker_proxy
            .invoke_agent(
                &owned_agent_id.agent_id(),
                method_name,
                method_parameters,
                AgentInvocationMode::Schedule,
                None,
                idempotency_key,
                freshness_disposition,
                self_agent_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_stack,
                config,
                principal,
                owned_agent_id.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }
}

fn spawn_live_response_router(
    session: LiveValueSession,
    mut inbound: InvocationResponseStream,
    requests: mpsc::Sender<InvocationRequest>,
    state: Arc<tokio::sync::Mutex<InvocationSessionState>>,
) {
    tokio::spawn(async move {
        let _requests = requests;
        while let Some(response) = inbound.next().await {
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    fail_live_response_transport(&session, error.to_string()).await;
                    return;
                }
            };
            if let Err(details) = state.lock().await.validate_response(&response) {
                fail_live_response_transport(&session, details).await;
                return;
            }
            match response.response {
                Some(invocation_response::Response::Finished(finished)) => {
                    if let Err(error) =
                        confirm_terminal_response_is_last(&mut inbound, &state).await
                    {
                        session.fail(error.to_string());
                        return;
                    }
                    match finished.outcome {
                        Some(invocation_session_completion::Outcome::Success(_)) => {
                            if let Err(details) = session.finish_invocation().await {
                                session.fail(details);
                            }
                        }
                        Some(invocation_session_completion::Outcome::Failure(failure)) => {
                            session.fail(rpc_error_from_failure(failure).to_string());
                        }
                        None => session.fail("invocation completion has no outcome".to_string()),
                    }
                    return;
                }
                Some(response) => match session.route_response(response).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let details = "unexpected response after the invocation result".to_string();
                        fail_live_response_transport(&session, details).await;
                        return;
                    }
                    Err(details) => {
                        fail_live_response_transport(&session, details).await;
                        return;
                    }
                },
                None => unreachable!("response state validation rejects empty frames"),
            }
        }
        if !state.lock().await.is_complete() {
            fail_live_response_transport(
                &session,
                "invocation response ended before completion".to_string(),
            )
            .await;
        }
    });
}

async fn confirm_terminal_response_is_last(
    inbound: &mut InvocationResponseStream,
    state: &Arc<tokio::sync::Mutex<InvocationSessionState>>,
) -> Result<(), RpcError> {
    match inbound.next().await {
        None => Ok(()),
        Some(Err(error)) => Err(error.into()),
        Some(Ok(response)) => {
            let details = state.lock().await.validate_response(&response).unwrap_err();
            Err(RpcError::ProtocolError { details })
        }
    }
}

async fn fail_live_response_transport(session: &LiveValueSession, details: String) {
    session.fail(details);
}

fn rpc_error_from_rejection(rejected: InvocationRejected) -> RpcError {
    match InvocationRejectionReason::try_from(rejected.reason)
        .unwrap_or(InvocationRejectionReason::Internal)
    {
        InvocationRejectionReason::Unauthorized => RpcError::Denied {
            details: rejected.error,
        },
        InvocationRejectionReason::NotFound => RpcError::NotFound {
            details: rejected.error,
        },
        InvocationRejectionReason::Internal => RpcError::RemoteInternalError {
            details: rejected.error,
        },
        _ => RpcError::ProtocolError {
            details: rejected.error,
        },
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

fn rpc_error_from_invocation_finished(
    finished: golem_api_grpc::proto::golem::worker::InvocationSessionCompletion,
) -> RpcError {
    match finished.outcome {
        Some(invocation_session_completion::Outcome::Failure(failure)) => {
            rpc_error_from_failure(failure)
        }
        Some(invocation_session_completion::Outcome::Success(_)) => RpcError::ProtocolError {
            details: "invocation completed successfully before publishing a result".to_string(),
        },
        None => RpcError::ProtocolError {
            details: "invocation completion has no outcome".to_string(),
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
    active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
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
    agent_types_service: Arc<dyn agent_types::AgentTypesService>,
    agent_webhooks_service: Arc<AgentWebhooksService>,
    http_connection_pool: Option<HttpConnectionPool>,
    websocket_connection_pool: WebSocketConnectionPool,
    extra_deps: Ctx::ExtraDeps,
    leak_sentinel: Arc<()>,
}

impl<Ctx: WorkerCtx> Clone for DirectWorkerInvocationRpc<Ctx> {
    fn clone(&self) -> Self {
        Self {
            remote_rpc: self.remote_rpc.clone(),
            direct_invocation_auth: self.direct_invocation_auth.clone(),
            active_workers: self.active_workers.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            card_service: self.card_service.clone(),
            component_service: self.component_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            quota_service: self.quota_service.clone(),
            worker_fork: self.worker_fork.clone(),
            worker_service: self.worker_service.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            rdbms_service: self.rdbms_service.clone(),
            oplog_service: self.oplog_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            worker_activator: self.worker_activator.clone(),
            events: self.events.clone(),
            file_loader: self.file_loader.clone(),
            oplog_processor_plugin: self.oplog_processor_plugin.clone(),
            resource_limits: self.resource_limits.clone(),
            shutdown_token: self.shutdown_token.clone(),
            environment_state_service: self.environment_state_service.clone(),
            agent_types_service: self.agent_types_service.clone(),
            agent_webhooks_service: self.agent_webhooks_service.clone(),
            http_connection_pool: self.http_connection_pool.clone(),
            websocket_connection_pool: self.websocket_connection_pool.clone(),
            extra_deps: self.extra_deps.clone(),
            leak_sentinel: self.leak_sentinel.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> HasEvents for DirectWorkerInvocationRpc<Ctx> {
    fn events(&self) -> Arc<Events> {
        self.events.clone()
    }
}

impl<Ctx: WorkerCtx> HasActiveWorkers<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>> {
        self.active_workers.clone()
    }
}

impl<Ctx: WorkerCtx> HasAgentTypesService for DirectWorkerInvocationRpc<Ctx> {
    fn agent_types(&self) -> Arc<dyn agent_types::AgentTypesService> {
        self.agent_types_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasAgentWebhooksService for DirectWorkerInvocationRpc<Ctx> {
    fn agent_webhooks(&self) -> Arc<AgentWebhooksService> {
        self.agent_webhooks_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasComponentService for DirectWorkerInvocationRpc<Ctx> {
    fn component_service(&self) -> Arc<dyn component::ComponentService> {
        self.component_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasCardService for DirectWorkerInvocationRpc<Ctx> {
    fn card_service(&self) -> Arc<dyn card::CardService> {
        self.card_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasConfig for DirectWorkerInvocationRpc<Ctx> {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.golem_config.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService> {
        self.worker_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerEnumerationService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_enumeration_service(&self) -> Arc<dyn worker_enumeration::WorkerEnumerationService> {
        self.worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRunningWorkerEnumerationService for DirectWorkerInvocationRpc<Ctx> {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService> {
        self.running_worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasPromiseService for DirectWorkerInvocationRpc<Ctx> {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService> {
        self.promise_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWasmtimeEngine<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn engine(&self) -> Arc<wasmtime::Engine> {
        self.engine.clone()
    }

    fn linker(&self) -> Arc<wasmtime::component::Linker<Ctx>> {
        self.linker.clone()
    }

    fn runtime(&self) -> Handle {
        self.runtime.clone()
    }
}

impl<Ctx: WorkerCtx> HasKeyValueService for DirectWorkerInvocationRpc<Ctx> {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService> {
        self.key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasBlobStoreService for DirectWorkerInvocationRpc<Ctx> {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService> {
        self.blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasSchedulerService for DirectWorkerInvocationRpc<Ctx> {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService> {
        self.scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogService for DirectWorkerInvocationRpc<Ctx> {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerForkService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_fork_service(&self) -> Arc<dyn worker_fork::WorkerForkService> {
        self.worker_fork.clone()
    }
}

impl<Ctx: WorkerCtx> HasRpc for DirectWorkerInvocationRpc<Ctx> {
    fn rpc(&self) -> Arc<dyn Rpc> {
        Arc::new(self.clone())
    }
}

impl<Ctx: WorkerCtx> HasLeakSentinel for DirectWorkerInvocationRpc<Ctx> {
    fn leak_sentinel(&self) -> Arc<()> {
        self.leak_sentinel.clone()
    }
}

impl<Ctx: WorkerCtx> HasExtraDeps<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.extra_deps.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardService for DirectWorkerInvocationRpc<Ctx> {
    fn shard_service(&self) -> Arc<dyn ShardService> {
        self.shard_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardManagerService for DirectWorkerInvocationRpc<Ctx> {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService> {
        self.shard_manager_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasQuotaService for DirectWorkerInvocationRpc<Ctx> {
    fn quota_service(&self) -> Arc<dyn crate::services::quota::QuotaService> {
        self.quota_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerActivator<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn worker_activator(&self) -> Arc<dyn worker_activator::WorkerActivator<Ctx>> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerProxy for DirectWorkerInvocationRpc<Ctx> {
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.remote_rpc.worker_proxy.clone()
    }
}

impl<Ctx: WorkerCtx> HasFileLoader for DirectWorkerInvocationRpc<Ctx> {
    fn file_loader(&self) -> Arc<FileLoader> {
        self.file_loader.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for DirectWorkerInvocationRpc<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin> {
        self.oplog_processor_plugin.clone()
    }
}

impl<Ctx: WorkerCtx> HasRdbmsService for DirectWorkerInvocationRpc<Ctx> {
    fn rdbms_service(&self) -> Arc<dyn rdbms::RdbmsService> {
        self.rdbms_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasResourceLimits for DirectWorkerInvocationRpc<Ctx> {
    fn resource_limits(&self) -> Arc<dyn ResourceLimits> {
        self.resource_limits.clone()
    }
}

impl<Ctx: WorkerCtx> HasShutdownToken for DirectWorkerInvocationRpc<Ctx> {
    fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.shutdown_token.clone()
    }
}

impl<Ctx: WorkerCtx> HasHttpConnectionPool for DirectWorkerInvocationRpc<Ctx> {
    fn http_connection_pool(&self) -> Option<HttpConnectionPool> {
        self.http_connection_pool.clone()
    }
}

impl<Ctx: WorkerCtx> HasWebSocketConnectionPool for DirectWorkerInvocationRpc<Ctx> {
    fn websocket_connection_pool(&self) -> WebSocketConnectionPool {
        self.websocket_connection_pool.clone()
    }
}

impl<Ctx: WorkerCtx> HasEnvironmentStateService for DirectWorkerInvocationRpc<Ctx> {
    fn environment_state_service(&self) -> Arc<dyn EnvironmentStateService> {
        self.environment_state_service.clone()
    }
}

#[allow(clippy::too_many_arguments)]
impl<Ctx: WorkerCtx> DirectWorkerInvocationRpc<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        remote_rpc: Arc<RemoteInvocationRpc>,
        direct_invocation_auth: Arc<dyn DirectInvocationAuthService>,
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        card_service: Arc<dyn card::CardService>,
        component_service: Arc<dyn component::ComponentService>,
        worker_fork: Arc<dyn worker_fork::WorkerForkService>,
        worker_service: Arc<dyn worker::WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService,
        >,
        promise_service: Arc<dyn promise::PromiseService>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn ShardService>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService>,
        quota_service: Arc<dyn crate::services::quota::QuotaService>,
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
        agent_types_service: Arc<dyn agent_types::AgentTypesService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        http_connection_pool: Option<HttpConnectionPool>,
        websocket_connection_pool: WebSocketConnectionPool,
        extra_deps: Ctx::ExtraDeps,
        leak_sentinel: Arc<()>,
    ) -> Self {
        Self {
            remote_rpc,
            direct_invocation_auth,
            active_workers,
            engine,
            linker,
            runtime,
            card_service,
            component_service,
            shard_manager_service,
            quota_service,
            worker_fork,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            shard_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            oplog_service,
            scheduler_service,
            worker_activator,
            events,
            file_loader,
            oplog_processor_plugin,
            resource_limits,
            shutdown_token,
            environment_state_service,
            agent_types_service,
            agent_webhooks_service,
            http_connection_pool,
            websocket_connection_pool,
            extra_deps,
            leak_sentinel,
        }
    }
    /// Rewrites the `OwnedAgentId` so that `environment_id` comes from the
    /// target component's metadata rather than from the caller. This ensures
    /// that auth checks, shard routing, and all downstream code use the
    /// component-authoritative environment.
    async fn canonicalize_owned_agent_id(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<OwnedAgentId, RpcError> {
        let component = self
            .component_service()
            .get_metadata(owned_agent_id.component_id(), None)
            .await
            .map_err(|e| RpcError::RemoteInternalError {
                details: format!("Failed to resolve target component metadata: {e}"),
            })?;
        Ok(OwnedAgentId::new(
            component.environment_id,
            &owned_agent_id.agent_id,
        ))
    }

    async fn validate_method_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        method_parameters: &SchemaValue,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<bool, RpcError> {
        let component_revision = method_validation_revision(freshness_disposition, || async {
            Worker::<Ctx>::get_latest_metadata(self, owned_agent_id)
                .await
                .map(|metadata| metadata.last_known_status.component_revision)
        })
        .await;
        let component = self
            .component_service()
            .get_metadata(owned_agent_id.component_id(), component_revision)
            .await?;
        let parsed_agent_id =
            ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata)
                .map_err(|details| RpcError::ProtocolError { details })?;
        validate_agent_method_invocation(
            &component.metadata,
            Some(&parsed_agent_id),
            method_name,
            method_parameters,
        )
        .map_err(Into::into)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Rpc for DirectWorkerInvocationRpc<Ctx> {
    async fn create_demand(
        &self,
        owned_agent_id: &OwnedAgentId,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;

        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_ok()
        {
            debug!(target_agent_id = %owned_agent_id, "Ensuring local target worker exists");

            self.direct_invocation_auth
                .check(
                    self_created_by,
                    owned_agent_id,
                    AgentVerb::Invoke,
                    AgentResourcePattern::Any,
                    auth_ctx,
                )
                .await?;

            let worker = Worker::get_or_create_running(
                self,
                owned_agent_id,
                Some(self_env.to_vec()),
                config,
                None,
                Some(self_agent_id.clone()),
                &self_stack,
                Principal::Agent(AgentPrincipal {
                    agent_id: self_agent_id.clone(),
                }),
            )
            .await?;

            let fingerprint = worker.get_initial_worker_metadata().fingerprint;
            Ok(Box::new(LoggingDemand::new(
                owned_agent_id.agent_id(),
                fingerprint,
            )))
        } else {
            self.remote_rpc
                .create_demand(
                    owned_agent_id,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                )
                .await
        }
    }

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
    ) -> Result<SchemaValue, RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;

        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh
            && idempotency_key.is_none()
        {
            return Err(RpcError::ProtocolError {
                details: "KnownFresh requires an idempotency key".to_string(),
            });
        }

        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_ok()
        {
            debug!(target_agent_id = %owned_agent_id, "Local direct agent invoke_and_await");

            self.direct_invocation_auth
                .check(
                    self_created_by,
                    owned_agent_id,
                    AgentVerb::Invoke,
                    AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
                    auth_ctx,
                )
                .await?;

            if self
                .validate_method_invocation(
                    owned_agent_id,
                    &method_name,
                    &method_parameters,
                    freshness_disposition,
                )
                .await?
            {
                return Err(RpcError::ProtocolError {
                    details: "live streams require the attached streaming RPC".to_string(),
                });
            }

            let principal = caller_agent_principal(self_agent_id);
            let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh());
            Worker::<Ctx>::validate_invocation_freshness(
                self,
                owned_agent_id,
                &idempotency_key,
                freshness_disposition,
            )
            .await?;
            let worker = Worker::get_or_create_suspended_with_freshness(
                self,
                owned_agent_id,
                Some(self_env.to_vec()),
                config,
                None,
                Some(self_agent_id.clone()),
                &self_stack,
                principal.clone(),
                freshness_disposition,
            )
            .await?;

            let invocation = AgentInvocation::AgentMethod {
                idempotency_key,
                method_name,
                input: method_parameters,
                invocation_context: self_stack,
                principal,
            };

            let output = worker.invoke_and_await(invocation).await?;

            match output.result {
                AgentInvocationResult::AgentMethod { output } => Ok(output),
                _ => Err(RpcError::RemoteInternalError {
                    details:
                        "Expected a result from agent invoke_and_await but got a non-method result"
                            .to_string(),
                }),
            }
        } else {
            self.remote_rpc
                .invoke_and_await(
                    owned_agent_id,
                    Some(idempotency_key.unwrap_or(IdempotencyKey::fresh())),
                    freshness_disposition,
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                )
                .await
        }
    }

    async fn invoke_and_await_streaming(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<SchemaValue, RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;
        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_err()
        {
            return self
                .remote_rpc
                .invoke_and_await_streaming(
                    owned_agent_id,
                    idempotency_key,
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                )
                .await;
        }

        self.direct_invocation_auth
            .check(
                self_created_by,
                owned_agent_id,
                AgentVerb::Invoke,
                AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
                auth_ctx,
            )
            .await?;
        self.validate_method_invocation(
            owned_agent_id,
            &method_name,
            &method_parameters,
            InvocationFreshnessDisposition::MayExist,
        )
        .await?;
        let principal = caller_agent_principal(self_agent_id);
        let worker = Worker::get_or_create_suspended(
            self,
            owned_agent_id,
            Some(self_env.to_vec()),
            config,
            None,
            Some(self_agent_id.clone()),
            &self_stack,
            principal.clone(),
        )
        .await?;
        let invocation = AgentInvocation::AgentMethod {
            idempotency_key: idempotency_key.unwrap_or(IdempotencyKey::fresh()),
            method_name,
            input: method_parameters,
            invocation_context: self_stack,
            principal,
        };
        let cancellation = tokio_util::sync::CancellationToken::new();
        worker
            .invoke_live_streaming(invocation, cancellation)
            .await
            .map_err(Into::into)
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
    ) -> Result<(), RpcError> {
        let owned_agent_id = &self.canonicalize_owned_agent_id(owned_agent_id).await?;

        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh
            && idempotency_key.is_none()
        {
            return Err(RpcError::ProtocolError {
                details: "KnownFresh requires an idempotency key".to_string(),
            });
        }

        if self
            .shard_service()
            .check_worker(&owned_agent_id.agent_id)
            .is_ok()
        {
            debug!(target_agent_id = %owned_agent_id, "Local direct agent invoke (fire-and-forget)");

            self.direct_invocation_auth
                .check(
                    self_created_by,
                    owned_agent_id,
                    AgentVerb::Invoke,
                    AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
                    auth_ctx,
                )
                .await?;

            if self
                .validate_method_invocation(
                    owned_agent_id,
                    &method_name,
                    &method_parameters,
                    freshness_disposition,
                )
                .await?
            {
                return Err(RpcError::ProtocolError {
                    details: "live streams cannot be used in fire-and-forget invocations"
                        .to_string(),
                });
            }

            let principal = caller_agent_principal(self_agent_id);
            let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh());
            Worker::<Ctx>::validate_invocation_freshness(
                self,
                owned_agent_id,
                &idempotency_key,
                freshness_disposition,
            )
            .await?;
            let worker = Worker::get_or_create_suspended_with_freshness(
                self,
                owned_agent_id,
                Some(self_env.to_vec()),
                config,
                None,
                Some(self_agent_id.clone()),
                &self_stack,
                principal.clone(),
                freshness_disposition,
            )
            .await?;

            let invocation = AgentInvocation::AgentMethod {
                idempotency_key,
                method_name,
                input: method_parameters,
                invocation_context: self_stack,
                principal,
            };

            match worker.clone().invoke(invocation).await? {
                crate::worker::ResultOrSubscription::Finished(Err(err)) => Err(err.into()),
                crate::worker::ResultOrSubscription::Finished(Ok(_)) => Ok(()),
                crate::worker::ResultOrSubscription::Pending(_) => {
                    Worker::start_if_needed(worker).await?;
                    Ok(())
                }
            }
        } else {
            self.remote_rpc
                .invoke(
                    owned_agent_id,
                    Some(idempotency_key.unwrap_or(IdempotencyKey::fresh())),
                    freshness_disposition,
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    config,
                    auth_ctx,
                )
                .await
        }
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::{
        RemoteLiveRequestGuard, RpcError, method_validation_revision, rpc_error_from_failure,
    };
    use crate::durable_host::stream_session::LiveValueSession;
    use golem_api_grpc::proto::golem::worker::{InvocationFailure, InvocationFailureKind};
    use golem_common::model::agent::InvocationFreshnessDisposition;
    use golem_common::model::component::ComponentRevision;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use std::cell::Cell;
    use test_r::test;
    use tokio::sync::mpsc;

    #[test]
    async fn known_fresh_method_validation_uses_selected_revision_without_metadata_probe() {
        let probed_existing_worker = Cell::new(false);
        let revision =
            method_validation_revision(InvocationFreshnessDisposition::KnownFresh, || async {
                probed_existing_worker.set(true);
                Some(ComponentRevision::INITIAL)
            })
            .await;

        assert_eq!(revision, None);
        assert!(!probed_existing_worker.get());
    }

    #[test]
    async fn may_exist_method_validation_uses_existing_worker_revision() {
        let probed_existing_worker = Cell::new(false);
        let existing_revision = ComponentRevision::new(7).unwrap();
        let revision =
            method_validation_revision(InvocationFreshnessDisposition::MayExist, || async {
                probed_existing_worker.set(true);
                Some(existing_revision)
            })
            .await;

        assert_eq!(revision, Some(existing_revision));
        assert!(probed_existing_worker.get());
    }

    #[test]
    fn typed_worker_failure_preserves_rpc_error_category() {
        let worker_error = WorkerExecutorError::invalid_request("bad invocation");
        let error = rpc_error_from_failure(InvocationFailure {
            kind: InvocationFailureKind::Execution as i32,
            code: "worker-execution".to_string(),
            message: worker_error.to_string(),
            worker_error: Some(worker_error.into()),
        });

        assert_eq!(
            error,
            RpcError::ProtocolError {
                details: "bad invocation".to_string(),
            }
        );
    }

    #[test]
    async fn dropped_remote_request_cancels_the_session_and_closes_requests() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new_client(frames.clone());
        let guard = RemoteLiveRequestGuard::new(session.clone(), frames);

        drop(guard);

        assert!(session.is_cancelled());
        drop(session);
        assert!(frame_rx.recv().await.is_none());
    }
}
