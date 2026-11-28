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

//! An in-process worker service for the executor under test.
//!
//! The executor reaches the worker service through its production `RemoteWorkerProxy`, which
//! calls the `golem.worker.v1.WorkerService` gRPC API at `public_worker_api`. A test that starts
//! an [`EmulatedWorkerService`] and passes it to
//! [`TestExecutorOverrides::with_worker_service`](crate::TestExecutorOverrides::with_worker_service)
//! points `public_worker_api` at it. The service forwards each call it emulates to the executor
//! under test, as the real worker service does when it routes a call to the executor that owns
//! the agent.
//!
//! It emulates `DeliverCardTransfer`, `ResumeWorker` and the invocation session
//! `InvokeAgentSession`. With [`WorkerServiceOptions::component_owner`], it also emulates
//! `PrepareWorker` and `ForkWorker`, and fills the owner into the start of a session: the calls
//! that the executor sends through the worker service when the target agent lives on another
//! executor. The real worker service finds the owner of the component; the emulated service gets
//! it as an option.
//!
//! It does not emulate routing, authentication beyond the system-only check of
//! `DeliverCardTransfer`, or the error mapping of the real worker service: an executor failure
//! reaches the proxy unchanged, as an internal error. A test therefore checks the error code of
//! the executor, not the code that a production caller sees.
//!
//! Every failure of a unary call is an answer in the response body, never a gRPC status, so the
//! client of the proxy never retries a call and never delivers a card transfer twice. A session
//! passes a status from the executor through unchanged. An RPC that the service does not emulate
//! answers `Status::unimplemented` with its name.

use futures::StreamExt;
use futures::stream::BoxStream;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::worker::v1::worker_service_server::{
    WorkerService, WorkerServiceServer,
};
use golem_api_grpc::proto::golem::worker::v1::{
    AgentError, CancelInvocationRequest, CancelInvocationResponse, CompletePromiseRequest,
    CompletePromiseResponse, DeliverCardTransferRequest, DeliverCardTransferResponse,
    DurableStreamAttachmentControlRequest, DurableStreamAttachmentControlResponse,
    DurableStreamSegmentReadRequest, DurableStreamSegmentReadResponse, ForkWorkerRequest,
    ForkWorkerResponse, InvokeAgentRequest, InvokeAgentResponse, LaunchNewWorkerRequest,
    LaunchNewWorkerResponse, LaunchNewWorkerSuccessResponse, ProcessOplogEntriesRequest,
    ProcessOplogEntriesResponse, ResumeWorkerRequest, ResumeWorkerResponse, RevertWorkerRequest,
    RevertWorkerResponse, UpdateWorkerRequest, UpdateWorkerResponse, WorkerExecutionError,
    agent_error, deliver_card_transfer_response, fork_worker_response, launch_new_worker_response,
    resume_worker_response,
};
use golem_api_grpc::proto::golem::worker::{
    InvocationRequest, InvocationResponse, invocation_request, invocation_response,
};
use golem_api_grpc::proto::golem::workerexecutor::v1 as executor;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::card::{CardId, StoredCard};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_schema::proto::golem::common::Empty;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::GATE_TIMEOUT;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::golem_config::WorkerServiceGrpcConfig;
use golem_worker_executor::services::rpc::RemoteInvocationRpc;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::worker_proxy::RemoteWorkerProxy;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::task::AbortOnDropHandle;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

type ExecutorClient = WorkerExecutorClient<OtelGrpcService<Channel>>;

/// The default gRPC message limit of the real worker service.
const MAX_MESSAGE_SIZE: usize = 32 * 1024 * 1024;

/// A permission-card transfer that the executor sent to the worker service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedCardTransfer {
    pub target_agent_id: AgentId,
    pub environment_id: EnvironmentId,
    pub transfer_id: uuid::Uuid,
    pub source_card_id: CardId,
    pub card: StoredCard,
}

/// What the invocation sessions through an [`EmulatedWorkerService`] carried.
#[derive(Debug, Default)]
pub struct RemoteRpcEvidence {
    /// The request messages of every session, including each `Start`.
    pub session_requests: AtomicUsize,
    /// The `Accepted` responses that joined the observer of an existing invocation.
    pub joined_observer_acceptances: AtomicUsize,
    /// The `ResumeAttach` requests.
    pub resume_attach_requests: AtomicUsize,
}

/// What an [`EmulatedWorkerService`] does with the calls it gets.
#[derive(Clone, Debug)]
pub struct WorkerServiceOptions {
    deliver_card_transfers: bool,
    record_card_transfer_attempts: bool,
    component_owner: Option<AccountId>,
    hold_after_resume: Option<Arc<Notify>>,
    lose_resume_response: bool,
}

impl Default for WorkerServiceOptions {
    fn default() -> Self {
        Self {
            deliver_card_transfers: true,
            record_card_transfer_attempts: false,
            component_owner: None,
            hold_after_resume: None,
            lose_resume_response: false,
        }
    }
}

impl WorkerServiceOptions {
    /// Answers each card transfer with success without delivering it, and records it.
    pub fn accept_card_transfers_without_delivery(self) -> Self {
        Self {
            deliver_card_transfers: false,
            record_card_transfer_attempts: true,
            ..self
        }
    }

    /// Records each card transfer when it arrives, before the service delivers it, whatever the
    /// delivery answers. By default the service records a transfer only after it was delivered.
    pub fn record_card_transfer_attempts(self) -> Self {
        Self {
            record_card_transfer_attempts: true,
            ..self
        }
    }

    /// Emulates `PrepareWorker`, `ForkWorker` and `InvokeAgentSession` with `account_id` as the
    /// owner of every component: the service sets it in each request it forwards, and in the
    /// `Start` of each session. Without an owner, `PrepareWorker` and `ForkWorker` answer
    /// `Status::unimplemented`, and a session is forwarded unchanged.
    pub fn component_owner(self, account_id: AccountId) -> Self {
        Self {
            component_owner: Some(account_id),
            ..self
        }
    }

    /// Makes the first successful `ResumeWorker` notify `notify`, then never answer.
    pub fn hold_after_resume(self, notify: Arc<Notify>) -> Self {
        Self {
            hold_after_resume: Some(notify),
            ..self
        }
    }

    /// Makes the service answer the first successful `ResumeWorker` that it does not hold with
    /// the error "lost fork resume response", after the executor resumed the agent.
    pub fn lose_resume_response(self) -> Self {
        Self {
            lose_resume_response: true,
            ..self
        }
    }
}

/// The executor under test that the service forwards to.
#[derive(Clone)]
struct Attached {
    client: ExecutorClient,
    /// The component service of the executor, where `PrepareWorker` finds the environment of a
    /// component. The production bootstrap of the test utils gives none.
    component_service: Option<Arc<dyn ComponentService>>,
}

struct State {
    options: WorkerServiceOptions,
    executor: watch::Sender<Option<Attached>>,
    card_transfers: Mutex<Vec<RecordedCardTransfer>>,
    lost_card_transfer_responses: Mutex<HashSet<uuid::Uuid>>,
    held_resume: Mutex<Option<Arc<Notify>>>,
    lose_resume_response: AtomicBool,
    evidence: Arc<RemoteRpcEvidence>,
}

/// An in-process worker service that forwards the calls of the executor under test back to it.
/// See the [module documentation](self).
#[derive(Clone)]
pub struct EmulatedWorkerService {
    state: Arc<State>,
    port: u16,
    _server: Arc<AbortOnDropHandle<()>>,
}

impl EmulatedWorkerService {
    pub async fn start(options: WorkerServiceOptions) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let state = Arc::new(State {
            held_resume: Mutex::new(options.hold_after_resume.clone()),
            lose_resume_response: AtomicBool::new(options.lose_resume_response),
            options,
            executor: watch::Sender::new(None),
            card_transfers: Mutex::default(),
            lost_card_transfer_responses: Mutex::default(),
            evidence: Arc::default(),
        });
        let service = WorkerServiceServer::new(Handler(state.clone()))
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip)
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE);
        let server = tokio::spawn(async move {
            if let Err(error) = tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
            {
                tracing::error!("emulated worker service stopped: {error}");
            }
        });
        Ok(Self {
            state,
            port,
            _server: Arc::new(AbortOnDropHandle::new(server)),
        })
    }

    /// The port on `127.0.0.1` that the service listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Makes the service answer the next successful delivery of `transfer_id` with an error
    /// after it delivered the transfer.
    pub fn lose_response_of_card_transfer(&self, transfer_id: uuid::Uuid) {
        self.state
            .lost_card_transfer_responses
            .lock()
            .unwrap()
            .insert(transfer_id);
    }

    /// The card transfers the service recorded, in order. See [`WorkerServiceOptions`] for when
    /// it records one.
    pub fn card_transfers(&self) -> Vec<RecordedCardTransfer> {
        self.state.card_transfers.lock().unwrap().clone()
    }

    /// What the invocation sessions through the service carried.
    pub fn session_evidence(&self) -> Arc<RemoteRpcEvidence> {
        self.state.evidence.clone()
    }

    /// A remote RPC over a production `RemoteWorkerProxy` that calls this service, as an
    /// executor calls the worker service for a target on another executor.
    pub fn remote_rpc(&self, shard_service: Arc<dyn ShardService>) -> RemoteInvocationRpc {
        let config = WorkerServiceGrpcConfig {
            host: "127.0.0.1".to_string(),
            port: self.port,
            ..WorkerServiceGrpcConfig::default()
        };
        RemoteInvocationRpc::new(Arc::new(RemoteWorkerProxy::new(&config)), shard_service)
    }

    /// Sends the calls to `client` from now on. Each start of a test executor that uses the
    /// service calls it.
    pub(crate) fn attach(
        &self,
        client: ExecutorClient,
        component_service: Option<Arc<dyn ComponentService>>,
    ) {
        self.state.executor.send_replace(Some(Attached {
            client,
            component_service,
        }));
    }
}

struct Handler(Arc<State>);

impl Handler {
    /// The client of the executor under test. A call that arrives before an executor started
    /// waits for one, for at most [`GATE_TIMEOUT`].
    async fn executor(&self) -> Result<Attached, AgentError> {
        let mut executor = self.0.executor.subscribe();
        match tokio::time::timeout(GATE_TIMEOUT, executor.wait_for(Option::is_some)).await {
            Ok(Ok(attached)) => Ok(attached.clone().expect("waited for an executor")),
            _ => Err(internal_error(WorkerExecutorError::runtime(
                "emulated worker service: no executor started",
            ))),
        }
    }

    async fn deliver(&self, request: DeliverCardTransferRequest) -> Result<(), AgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or_else(|| bad_request("auth_ctx not found"))?
            .try_into()
            .map_err(|error| bad_request(format!("failed converting auth_ctx: {error}")))?;
        auth.authorize_system_only("deliver permission card transfer")
            .map_err(|error| AgentError {
                error: Some(agent_error::Error::Unauthorized(ErrorBody {
                    error: error.to_string(),
                    code: String::new(),
                })),
            })?;

        let options = &self.0.options;
        let transfer = recorded_transfer(&request)?;
        if options.record_card_transfer_attempts {
            self.0.card_transfers.lock().unwrap().push(transfer.clone());
        }
        if !options.deliver_card_transfers {
            return Ok(());
        }

        let response = self
            .executor()
            .await?
            .client
            .deliver_card_transfer(executor::DeliverCardTransferRequest {
                target_agent_id: request.target_agent_id,
                environment_id: request.environment_id,
                transfer_id: request.transfer_id,
                card: request.card,
                auth_ctx: request.auth_ctx,
                source_card_id: request.source_card_id,
            })
            .await
            .map_err(transport_error)?
            .into_inner();
        match response.result {
            Some(executor::deliver_card_transfer_response::Result::Success(_)) => {}
            Some(executor::deliver_card_transfer_response::Result::Failure(error)) => {
                return Err(executor_failure(error));
            }
            None => return Err(no_result("deliver_card_transfer")),
        }

        if !options.record_card_transfer_attempts {
            self.0.card_transfers.lock().unwrap().push(transfer.clone());
        }
        if self
            .0
            .lost_card_transfer_responses
            .lock()
            .unwrap()
            .remove(&transfer.transfer_id)
        {
            return Err(internal_error(WorkerExecutorError::runtime(
                "simulated lost card-transfer response",
            )));
        }
        Ok(())
    }

    async fn resume(&self, request: ResumeWorkerRequest) -> Result<(), AgentError> {
        let response = self
            .executor()
            .await?
            .client
            .resume_worker(executor::ResumeWorkerRequest {
                agent_id: request.agent_id,
                force: request.force,
                environment_id: request.environment_id,
                auth_ctx: request.auth_ctx,
                caller: Some(crate::callers::anonymous().into()),
                expected_fingerprint: None,
            })
            .await
            .map_err(transport_error)?
            .into_inner();
        match response.result {
            Some(executor::resume_worker_response::Result::Success(_)) => {}
            Some(executor::resume_worker_response::Result::Failure(error)) => {
                return Err(executor_failure(error));
            }
            None => return Err(no_result("resume_worker")),
        }
        let held = self.0.held_resume.lock().unwrap().take();
        if let Some(held) = held {
            held.notify_one();
            std::future::pending::<()>().await;
        }
        if self.0.lose_resume_response.swap(false, Ordering::SeqCst) {
            return Err(internal_error(WorkerExecutorError::runtime(
                "lost fork resume response",
            )));
        }
        Ok(())
    }

    async fn prepare(
        &self,
        owner: AccountId,
        request: LaunchNewWorkerRequest,
    ) -> Result<LaunchNewWorkerSuccessResponse, AgentError> {
        let executor = self.executor().await?;
        let component_id: ComponentId = request
            .component_id
            .ok_or_else(|| bad_request("Missing component_id"))?
            .try_into()
            .map_err(|error| bad_request(format!("invalid component_id: {error}")))?;
        let component = executor
            .component_service
            .ok_or_else(|| {
                internal_error(WorkerExecutorError::runtime(
                    "emulated worker service: the executor has no component service",
                ))
            })?
            .get_metadata(component_id, None)
            .await
            .map_err(internal_error)?;
        let response = executor
            .client
            .clone()
            .prepare_worker(executor::CreateWorkerRequest {
                constructor: request.constructor,
                unkeyed_id: request.unkeyed_id,
                invocation: request.invocation,
                component_id: request.component_id,
                env: request.env,
                config: request.config,
                component_owner_account_id: Some(owner.into()),
                environment_id: Some(component.environment_id.into()),
                ignore_already_existing: request.ignore_already_existing,
                auth_ctx: request.auth_ctx,
                caller: request.caller,
                invocation_context: request.context,
                idempotency_key: request.idempotency_key,
                freshness_disposition: request.freshness_disposition,
            })
            .await
            .map_err(transport_error)?
            .into_inner();
        match response.result {
            Some(executor::create_worker_response::Result::Success(success)) => {
                Ok(LaunchNewWorkerSuccessResponse {
                    agent_id: success.agent_id,
                    component_version: component.revision.into(),
                    instance_id: success.instance_id,
                })
            }
            Some(executor::create_worker_response::Result::Failure(error)) => {
                Err(executor_failure(error))
            }
            None => Err(no_result("prepare_worker")),
        }
    }

    async fn fork(&self, owner: AccountId, request: ForkWorkerRequest) -> Result<(), AgentError> {
        let response = self
            .executor()
            .await?
            .client
            .fork_worker(executor::ForkWorkerRequest {
                source_agent_id: request.source_agent_id,
                target_agent_id: request.target_agent_id,
                oplog_index_cutoff: request.oplog_index_cutoff,
                environment_id: request.environment_id,
                component_owner_account_id: Some(owner.into()),
                component_owner_account_email: String::new(),
                auth_ctx: request.auth_ctx,
                caller: request.caller,
                expected_fingerprint: None,
            })
            .await
            .map_err(transport_error)?
            .into_inner();
        match response.result {
            Some(executor::fork_worker_response::Result::Success(_)) => Ok(()),
            Some(executor::fork_worker_response::Result::Failure(error)) => {
                Err(executor_failure(error))
            }
            None => Err(no_result("fork_worker")),
        }
    }
}

fn recorded_transfer(
    request: &DeliverCardTransferRequest,
) -> Result<RecordedCardTransfer, AgentError> {
    let target_agent_id: AgentId = request
        .target_agent_id
        .ok_or_else(|| bad_request("Missing target_agent_id"))?
        .try_into()
        .map_err(|error| bad_request(format!("invalid target_agent_id: {error}")))?;
    let environment_id: EnvironmentId = request
        .environment_id
        .ok_or_else(|| bad_request("Missing environment_id"))?
        .try_into()
        .map_err(|error| bad_request(format!("invalid environment_id: {error}")))?;
    let transfer_id = request
        .transfer_id
        .ok_or_else(|| bad_request("Missing transfer_id"))?
        .into();
    let source_card_id = CardId(
        request
            .source_card_id
            .ok_or_else(|| bad_request("Missing source_card_id"))?
            .into(),
    );
    let card: StoredCard = desert_rust::deserialize(&request.card)
        .map_err(|error| bad_request(format!("invalid card: {error}")))?;
    Ok(RecordedCardTransfer {
        target_agent_id,
        environment_id,
        transfer_id,
        source_card_id,
        card,
    })
}

fn internal_error(error: WorkerExecutorError) -> AgentError {
    executor_failure(error.into())
}

fn executor_failure(error: WorkerExecutionError) -> AgentError {
    AgentError {
        error: Some(agent_error::Error::InternalError(error)),
    }
}

fn transport_error(status: Status) -> AgentError {
    internal_error(WorkerExecutorError::unknown(format!(
        "emulated worker service: the executor did not answer: {status}"
    )))
}

fn no_result(rpc: &str) -> AgentError {
    internal_error(WorkerExecutorError::unknown(format!(
        "emulated worker service: the executor's {rpc} returned no result"
    )))
}

fn bad_request(error: impl Into<String>) -> AgentError {
    AgentError {
        error: Some(agent_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.into()],
            code: String::new(),
        })),
    }
}

fn unimplemented<T>(rpc: &str) -> Result<Response<T>, Status> {
    Err(Status::unimplemented(format!(
        "the emulated worker service does not emulate {rpc}"
    )))
}

#[tonic::async_trait]
impl WorkerService for Handler {
    async fn launch_new_worker(
        &self,
        _request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        unimplemented("LaunchNewWorker")
    }

    async fn prepare_worker(
        &self,
        request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        let Some(owner) = self.0.options.component_owner else {
            return unimplemented("PrepareWorker");
        };
        let result = match self.prepare(owner, request.into_inner()).await {
            Ok(success) => launch_new_worker_response::Result::Success(success),
            Err(error) => launch_new_worker_response::Result::Error(error),
        };
        Ok(Response::new(LaunchNewWorkerResponse {
            result: Some(result),
        }))
    }

    async fn update_worker(
        &self,
        _request: Request<UpdateWorkerRequest>,
    ) -> Result<Response<UpdateWorkerResponse>, Status> {
        unimplemented("UpdateWorker")
    }

    async fn resume_worker(
        &self,
        request: Request<ResumeWorkerRequest>,
    ) -> Result<Response<ResumeWorkerResponse>, Status> {
        let result = match self.resume(request.into_inner()).await {
            Ok(()) => resume_worker_response::Result::Success(Empty {}),
            Err(error) => resume_worker_response::Result::Error(error),
        };
        Ok(Response::new(ResumeWorkerResponse {
            result: Some(result),
        }))
    }

    async fn fork_worker(
        &self,
        request: Request<ForkWorkerRequest>,
    ) -> Result<Response<ForkWorkerResponse>, Status> {
        let Some(owner) = self.0.options.component_owner else {
            return unimplemented("ForkWorker");
        };
        let result = match self.fork(owner, request.into_inner()).await {
            Ok(()) => fork_worker_response::Result::Success(Empty {}),
            Err(error) => fork_worker_response::Result::Error(error),
        };
        Ok(Response::new(ForkWorkerResponse {
            result: Some(result),
        }))
    }

    async fn revert_worker(
        &self,
        _request: Request<RevertWorkerRequest>,
    ) -> Result<Response<RevertWorkerResponse>, Status> {
        unimplemented("RevertWorker")
    }

    async fn complete_promise(
        &self,
        _request: Request<CompletePromiseRequest>,
    ) -> Result<Response<CompletePromiseResponse>, Status> {
        unimplemented("CompletePromise")
    }

    async fn invoke_agent(
        &self,
        _request: Request<InvokeAgentRequest>,
    ) -> Result<Response<InvokeAgentResponse>, Status> {
        unimplemented("InvokeAgent")
    }

    type InvokeAgentSessionStream = BoxStream<'static, Result<InvocationResponse, Status>>;

    /// Forwards a session to the executor message by message, in both directions, and counts
    /// what it carries in [`RemoteRpcEvidence`]. A status of the executor reaches the proxy
    /// unchanged.
    async fn invoke_agent_session(
        &self,
        request: Request<tonic::Streaming<InvocationRequest>>,
    ) -> Result<Response<Self::InvokeAgentSessionStream>, Status> {
        let mut client = match self.executor().await {
            Ok(executor) => executor.client,
            Err(_) => {
                return Err(Status::internal(
                    "emulated worker service: no executor started",
                ));
            }
        };
        let owner = self.0.options.component_owner;
        let evidence = self.0.evidence.clone();
        let requests = request
            .into_inner()
            .take_while(|request| std::future::ready(request.is_ok()))
            .filter_map(|request| std::future::ready(request.ok()))
            .map({
                let evidence = evidence.clone();
                move |mut request: InvocationRequest| {
                    evidence.session_requests.fetch_add(1, Ordering::SeqCst);
                    match &mut request.request {
                        Some(invocation_request::Request::Start(start)) => {
                            if let Some(owner) = owner {
                                start.component_owner_account_id = Some(owner.into());
                            }
                        }
                        Some(invocation_request::Request::ResumeAttach(_)) => {
                            evidence
                                .resume_attach_requests
                                .fetch_add(1, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                    request
                }
            });
        let responses = client
            .invoke_agent_session(Box::pin(requests))
            .await?
            .into_inner()
            .inspect(move |response| {
                if let Ok(response) = response
                    && let Some(invocation_response::Response::Accepted(accepted)) =
                        &response.response
                    && accepted.joined_origin_observer
                {
                    evidence
                        .joined_observer_acceptances
                        .fetch_add(1, Ordering::SeqCst);
                }
            });
        Ok(Response::new(responses.boxed()))
    }

    async fn cancel_invocation(
        &self,
        _request: Request<CancelInvocationRequest>,
    ) -> Result<Response<CancelInvocationResponse>, Status> {
        unimplemented("CancelInvocation")
    }

    async fn control_durable_stream_attachment(
        &self,
        _request: Request<DurableStreamAttachmentControlRequest>,
    ) -> Result<Response<DurableStreamAttachmentControlResponse>, Status> {
        unimplemented("ControlDurableStreamAttachment")
    }

    async fn read_durable_stream_segment(
        &self,
        _request: Request<DurableStreamSegmentReadRequest>,
    ) -> Result<Response<DurableStreamSegmentReadResponse>, Status> {
        unimplemented("ReadDurableStreamSegment")
    }

    async fn process_oplog_entries(
        &self,
        _request: Request<ProcessOplogEntriesRequest>,
    ) -> Result<Response<ProcessOplogEntriesResponse>, Status> {
        unimplemented("ProcessOplogEntries")
    }

    async fn deliver_card_transfer(
        &self,
        request: Request<DeliverCardTransferRequest>,
    ) -> Result<Response<DeliverCardTransferResponse>, Status> {
        let result = match self.deliver(request.into_inner()).await {
            Ok(()) => deliver_card_transfer_response::Result::Success(Empty {}),
            Err(error) => deliver_card_transfer_response::Result::Error(error),
        };
        Ok(Response::new(DeliverCardTransferResponse {
            result: Some(result),
        }))
    }
}
