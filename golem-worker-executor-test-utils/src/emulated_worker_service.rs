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
//! It does not emulate routing, authentication beyond the system-only check of
//! `DeliverCardTransfer`, or the error mapping of the real worker service: an executor failure
//! reaches the proxy unchanged, as an internal error. A test therefore checks the error code of
//! the executor, not the code that a production caller sees.
//!
//! Every failure is an answer in the response body, never a gRPC status, so the client of the
//! proxy never retries a call and never delivers a card transfer twice. An RPC that the service
//! does not emulate answers `Status::unimplemented` with its name.

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
    LaunchNewWorkerResponse, ProcessOplogEntriesRequest, ProcessOplogEntriesResponse,
    ResumeWorkerRequest, ResumeWorkerResponse, RevertWorkerRequest, RevertWorkerResponse,
    UpdateWorkerRequest, UpdateWorkerResponse, WorkerExecutionError, agent_error,
    deliver_card_transfer_response, resume_worker_response,
};
use golem_api_grpc::proto::golem::worker::{InvocationRequest, InvocationResponse};
use golem_api_grpc::proto::golem::workerexecutor::v1 as executor;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::model::AgentId;
use golem_common::model::card::{CardId, StoredCard};
use golem_common::model::environment::EnvironmentId;
use golem_schema::proto::golem::common::Empty;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::GATE_TIMEOUT;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::task::AbortOnDropHandle;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

type ExecutorClient = WorkerExecutorClient<OtelGrpcService<Channel>>;

/// A permission-card transfer that the executor sent to the worker service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedCardTransfer {
    pub target_agent_id: AgentId,
    pub environment_id: EnvironmentId,
    pub transfer_id: uuid::Uuid,
    pub source_card_id: CardId,
    pub card: StoredCard,
}

/// What an [`EmulatedWorkerService`] does with the calls it gets.
#[derive(Clone, Debug)]
pub struct WorkerServiceOptions {
    deliver_card_transfers: bool,
    record_card_transfer_attempts: bool,
}

impl Default for WorkerServiceOptions {
    fn default() -> Self {
        Self {
            deliver_card_transfers: true,
            record_card_transfer_attempts: false,
        }
    }
}

impl WorkerServiceOptions {
    /// Answers each card transfer with success without delivering it, and records it.
    pub fn accept_card_transfers_without_delivery(self) -> Self {
        Self {
            deliver_card_transfers: false,
            record_card_transfer_attempts: true,
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
}

struct State {
    options: WorkerServiceOptions,
    executor: watch::Sender<Option<ExecutorClient>>,
    card_transfers: Mutex<Vec<RecordedCardTransfer>>,
    lost_card_transfer_responses: Mutex<HashSet<uuid::Uuid>>,
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
            options,
            executor: watch::Sender::new(None),
            card_transfers: Mutex::default(),
            lost_card_transfer_responses: Mutex::default(),
        });
        let service = WorkerServiceServer::new(Handler(state.clone()))
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip);
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

    /// Sends the calls to `client` from now on. Each start of a test executor that uses the
    /// service calls it.
    pub(crate) fn attach(&self, client: ExecutorClient) {
        self.state.executor.send_replace(Some(client));
    }
}

struct Handler(Arc<State>);

impl Handler {
    /// The client of the executor under test. A call that arrives before an executor started
    /// waits for one, for at most [`GATE_TIMEOUT`].
    async fn executor(&self) -> Result<ExecutorClient, AgentError> {
        let mut executor = self.0.executor.subscribe();
        match tokio::time::timeout(GATE_TIMEOUT, executor.wait_for(Option::is_some)).await {
            Ok(Ok(client)) => Ok(client.clone().expect("waited for a client")),
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
            Some(executor::resume_worker_response::Result::Success(_)) => Ok(()),
            Some(executor::resume_worker_response::Result::Failure(error)) => {
                Err(executor_failure(error))
            }
            None => Err(no_result("resume_worker")),
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
        _request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        unimplemented("PrepareWorker")
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
        _request: Request<ForkWorkerRequest>,
    ) -> Result<Response<ForkWorkerResponse>, Status> {
        unimplemented("ForkWorker")
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

    async fn invoke_agent_session(
        &self,
        _request: Request<tonic::Streaming<InvocationRequest>>,
    ) -> Result<Response<Self::InvokeAgentSessionStream>, Status> {
        unimplemented("InvokeAgentSession")
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
