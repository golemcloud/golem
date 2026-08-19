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

use super::error::WorkerTraceErrorKind;
use super::{bad_request_error, validate_protobuf_agent_id};
use crate::service::worker::{
    InvocationRequestStream, InvocationResponseStream, WorkerService, WorkerServiceError,
};
use futures::{FutureExt, Stream, StreamExt, stream};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::common::Empty;
use golem_api_grpc::proto::golem::worker::v1::worker_service_server::WorkerService as GrpcWorkerService;
use golem_api_grpc::proto::golem::worker::v1::{
    AgentError as GrpcAgentError, CancelInvocationRequest, CancelInvocationResponse,
    CompletePromiseRequest, CompletePromiseResponse, ForkWorkerRequest, ForkWorkerResponse,
    InvokeAgentRequest, InvokeAgentResponse, InvokeAgentSuccess, LaunchNewWorkerRequest,
    LaunchNewWorkerResponse, LaunchNewWorkerSuccessResponse, ProcessOplogEntriesRequest,
    ProcessOplogEntriesResponse, ResumeWorkerRequest, ResumeWorkerResponse, RevertWorkerRequest,
    RevertWorkerResponse, UpdateWorkerRequest, UpdateWorkerResponse, cancel_invocation_response,
    complete_promise_response, fork_worker_response, invoke_agent_response,
    launch_new_worker_response, process_oplog_entries_response, resume_worker_response,
    revert_worker_response, update_worker_response,
};
use golem_api_grpc::proto::golem::worker::{
    InvocationRejected, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    invocation_request, invocation_response,
};
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::worker::AgentUpdateMode;
use golem_common::model::{AgentFingerprint, AgentId, IdempotencyKey};
use golem_common::recorded_grpc_api_request;
use golem_service_base::grpc::proto_agent_id_string;
use golem_service_base::model::auth::AuthCtx;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::Instrument;

fn service_failure_stream(
    reason: InvocationRejectionReason,
    error: String,
    idempotency_key: Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
    agent_id: Option<golem_api_grpc::proto::golem::worker::AgentId>,
) -> InvocationResponseStream {
    Box::pin(stream::once(async move {
        Ok(InvocationResponse {
            response: Some(invocation_response::Response::Rejected(
                InvocationRejected {
                    reason: reason as i32,
                    error,
                    idempotency_key,
                    agent_id,
                    component_revision: None,
                },
            )),
        })
    }))
}

fn request_identity(
    request: &InvocationRequest,
) -> (
    Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
    Option<golem_api_grpc::proto::golem::worker::AgentId>,
) {
    match request.request.as_ref() {
        Some(invocation_request::Request::Start(start)) => {
            (start.idempotency_key.clone(), start.agent_id.clone())
        }
        Some(invocation_request::Request::ResumeAttach(resume)) => {
            (resume.idempotency_key.clone(), None)
        }
        _ => (None, None),
    }
}

fn validated_response_stream<S>(
    inbound: S,
    state: Arc<tokio::sync::Mutex<InvocationSessionState>>,
    initial_requests_checked: Option<tokio::sync::oneshot::Receiver<()>>,
) -> InvocationResponseStream
where
    S: Stream<Item = Result<InvocationResponse, Status>> + Send + Unpin + 'static,
{
    Box::pin(stream::unfold(
        Some((inbound, state, initial_requests_checked)),
        |state| async move {
            let (mut inbound, response_state, initial_requests_checked) = state?;
            if let Some(initial_requests_checked) = initial_requests_checked {
                let _ = initial_requests_checked.await;
            }
            match inbound.next().await {
                Some(Ok(response)) => {
                    let mut state = response_state.lock().await;
                    match state.validate_response(&response) {
                        Ok(()) if state.is_complete() => {
                            drop(state);
                            match inbound.next().await {
                                None => Some((Ok(response), None)),
                                Some(Ok(response_after_terminal)) => {
                                    let details = response_state
                                        .lock()
                                        .await
                                        .validate_response(&response_after_terminal)
                                        .unwrap_err();
                                    Some((Err(Status::internal(details)), None))
                                }
                                Some(Err(error)) => Some((Err(error), None)),
                            }
                        }
                        Ok(()) => {
                            drop(state);
                            Some((Ok(response), Some((inbound, response_state, None))))
                        }
                        Err(details) => Some((Err(Status::internal(details)), None)),
                    }
                }
                Some(Err(error)) => Some((Err(error), None)),
                None if response_state.lock().await.is_complete() => None,
                None => Some((
                    Err(Status::unavailable(
                        "invocation response transport closed before completion",
                    )),
                    None,
                )),
            }
        },
    ))
}

fn validated_request_tail<S>(
    inbound: S,
    state: Arc<tokio::sync::Mutex<InvocationSessionState>>,
) -> (InvocationRequestStream, tokio::sync::oneshot::Receiver<()>)
where
    S: Stream<Item = Result<InvocationRequest, Status>> + Send + Unpin + 'static,
{
    let (validated_tx, validated_rx) = tokio::sync::mpsc::channel(32);
    let (initial_requests_checked_tx, initial_requests_checked_rx) =
        tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut inbound = inbound;
        let mut initial_requests_checked_tx = Some(initial_requests_checked_tx);
        loop {
            let Some(next) = inbound.next().now_or_never() else {
                let _ = initial_requests_checked_tx.take().unwrap().send(());
                break;
            };
            let Some(next) = next else {
                let _ = initial_requests_checked_tx.take().unwrap().send(());
                return;
            };
            let request = match next {
                Ok(request) => request,
                Err(_) => {
                    let _ = initial_requests_checked_tx.take().unwrap().send(());
                    return;
                }
            };
            let invalid = state
                .lock()
                .await
                .validate_trusted_request(&request)
                .is_err();
            if validated_tx.send(request).await.is_err() {
                let _ = initial_requests_checked_tx.take().unwrap().send(());
                return;
            }
            if invalid {
                let _ = initial_requests_checked_tx.take().unwrap().send(());
                return;
            }
        }
        while let Some(next) = inbound.next().await {
            let request = match next {
                Ok(request) => request,
                Err(_) => return,
            };
            let invalid = state
                .lock()
                .await
                .validate_trusted_request(&request)
                .is_err();
            if validated_tx.send(request).await.is_err() {
                return;
            }
            if invalid {
                return;
            }
        }
    });
    (
        Box::pin(stream::unfold(validated_rx, |mut receiver| async move {
            receiver.recv().await.map(|request| (request, receiver))
        })),
        initial_requests_checked_rx,
    )
}

/// The only way to turn a wire-level freshness disposition into the internal
/// [`InvocationFreshnessDisposition`]. Decoding and trust-sanitization are
/// deliberately fused into a single function so that no gRPC entry point can
/// accept a `KnownFresh` claim from the wire without going through the
/// trusted-caller check: `KnownFresh` allows the executor to skip the
/// ephemeral existence check, so it must never be honored for external
/// callers.
fn sanitize_invocation_freshness_disposition(
    wire_value: i32,
    trusted_internal_caller: bool,
) -> InvocationFreshnessDisposition {
    let decoded = if wire_value
        == golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh as i32
    {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    };
    if trusted_internal_caller {
        decoded
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

pub struct WorkerGrpcApi {
    worker_service: Arc<WorkerService>,
}

#[async_trait::async_trait]
impl GrpcWorkerService for WorkerGrpcApi {
    async fn launch_new_worker(
        &self,
        request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "launch_new_worker",
            component_id = ComponentId::render_proto(request.component_id),
            name = request.name
        );

        let response = match self
            .launch_new_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok((agent_id, component_version, fingerprint)) => record.succeed(
                launch_new_worker_response::Result::Success(LaunchNewWorkerSuccessResponse {
                    agent_id: Some(agent_id.into()),
                    component_version: component_version.into(),
                    instance_id: Some(fingerprint.0.into()),
                }),
            ),
            Err(error) => record.fail(
                launch_new_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(LaunchNewWorkerResponse {
            result: Some(response),
        }))
    }

    async fn complete_promise(
        &self,
        request: Request<CompletePromiseRequest>,
    ) -> Result<Response<CompletePromiseResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "complete_promise",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let response = match self
            .complete_promise(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(complete_promise_response::Result::Success(result)),
            Err(error) => record.fail(
                complete_promise_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CompletePromiseResponse {
            result: Some(response),
        }))
    }

    async fn resume_worker(
        &self,
        request: Request<ResumeWorkerRequest>,
    ) -> Result<Response<ResumeWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "resume_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let response = match self
            .resume_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(resume_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                resume_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ResumeWorkerResponse {
            result: Some(response),
        }))
    }

    async fn update_worker(
        &self,
        request: Request<UpdateWorkerRequest>,
    ) -> Result<Response<UpdateWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "update_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let response = match self
            .update_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(update_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                update_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateWorkerResponse {
            result: Some(response),
        }))
    }

    async fn fork_worker(
        &self,
        request: Request<ForkWorkerRequest>,
    ) -> Result<Response<ForkWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "fork_worker",
            source_agent_id = proto_agent_id_string(&request.source_agent_id),
            target_agent_id = proto_agent_id_string(&request.target_agent_id),
        );

        let response = match self
            .fork_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(fork_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                fork_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ForkWorkerResponse {
            result: Some(response),
        }))
    }

    async fn revert_worker(
        &self,
        request: Request<RevertWorkerRequest>,
    ) -> Result<Response<RevertWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "revert_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let response = match self
            .revert_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(revert_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                revert_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(RevertWorkerResponse {
            result: Some(response),
        }))
    }

    async fn invoke_agent(
        &self,
        request: Request<InvokeAgentRequest>,
    ) -> Result<Response<InvokeAgentResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "invoke_agent",
            agent_id = proto_agent_id_string(&request.agent_id),
            method_name = request.method_name
        );

        let response = match self
            .invoke_agent(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(invoke_agent_response::Result::Success(result)),
            Err(error) => record.fail(
                invoke_agent_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InvokeAgentResponse {
            result: Some(response),
        }))
    }

    type InvokeAgentSessionStream = InvocationResponseStream;

    async fn invoke_agent_session(
        &self,
        request: Request<tonic::Streaming<InvocationRequest>>,
    ) -> Result<Response<Self::InvokeAgentSessionStream>, Status> {
        let mut inbound = request.into_inner();
        let first = match inbound.message().await {
            Ok(Some(request)) => request,
            Ok(None) => {
                return Ok(Response::new(service_failure_stream(
                    InvocationRejectionReason::Protocol,
                    "invocation request ended before start".to_string(),
                    None,
                    None,
                )));
            }
            Err(error) => {
                return Err(error);
            }
        };
        let (idempotency_key, agent_id) = request_identity(&first);
        let mut state = InvocationSessionState::default();
        if let Err(error) = state.validate_trusted_request(&first) {
            return Ok(Response::new(service_failure_stream(
                InvocationRejectionReason::Protocol,
                error,
                idempotency_key,
                agent_id,
            )));
        }
        let mut start = match first
            .request
            .expect("validated invocation request has a payload")
        {
            invocation_request::Request::Start(start) => start,
            invocation_request::Request::ResumeAttach(_) => {
                return Ok(Response::new(service_failure_stream(
                    InvocationRejectionReason::ResumeUnsupported,
                    "resume-attach is not supported by live sessions".to_string(),
                    idempotency_key,
                    None,
                )));
            }
            _ => unreachable!("the session validator requires start or resume-attach first"),
        };
        let auth = match start.auth_ctx.clone() {
            Some(auth) => match auth.try_into() {
                Ok(auth) => auth,
                Err(error) => {
                    return Ok(Response::new(service_failure_stream(
                        InvocationRejectionReason::Validation,
                        format!("failed converting auth_ctx: {error}"),
                        idempotency_key,
                        agent_id,
                    )));
                }
            },
            None => {
                return Ok(Response::new(service_failure_stream(
                    InvocationRejectionReason::Validation,
                    "auth_ctx not found".to_string(),
                    idempotency_key,
                    agent_id,
                )));
            }
        };
        let trusted_internal_caller = matches!(&auth, AuthCtx::System | AuthCtx::Agent(_));
        start.freshness_disposition = match sanitize_invocation_freshness_disposition(
            start.freshness_disposition,
            trusted_internal_caller,
        ) {
            InvocationFreshnessDisposition::MayExist => {
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32
            }
            InvocationFreshnessDisposition::KnownFresh => {
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                    as i32
            }
        };
        let state = Arc::new(tokio::sync::Mutex::new(state));
        let (tail, initial_requests_checked) = validated_request_tail(inbound, state.clone());
        match self
            .worker_service
            .invoke_agent_session(start, tail, trusted_internal_caller, auth)
            .await
        {
            Ok(response) => Ok(Response::new(validated_response_stream(
                response,
                state,
                Some(initial_requests_checked),
            ))),
            Err(error) => {
                let reason = match &error {
                    WorkerServiceError::AuthError(_) | WorkerServiceError::LimitError(_) => {
                        InvocationRejectionReason::Unauthorized
                    }
                    WorkerServiceError::ComponentNotFound(_)
                    | WorkerServiceError::AgentNotFound(_)
                    | WorkerServiceError::AccountIdNotFound(_) => {
                        InvocationRejectionReason::NotFound
                    }
                    WorkerServiceError::TypeChecker(_) => InvocationRejectionReason::Validation,
                    WorkerServiceError::InternalCallError(_) => InvocationRejectionReason::Internal,
                    _ => InvocationRejectionReason::Internal,
                };
                Ok(Response::new(service_failure_stream(
                    reason,
                    error.to_string(),
                    idempotency_key,
                    agent_id,
                )))
            }
        }
    }

    async fn cancel_invocation(
        &self,
        request: Request<CancelInvocationRequest>,
    ) -> Result<Response<CancelInvocationResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "cancel_invocation",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let response = match self
            .cancel_invocation_inner(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(canceled) => record.succeed(cancel_invocation_response::Result::Success(canceled)),
            Err(error) => record.fail(
                cancel_invocation_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CancelInvocationResponse {
            result: Some(response),
        }))
    }

    async fn process_oplog_entries(
        &self,
        request: Request<ProcessOplogEntriesRequest>,
    ) -> Result<Response<ProcessOplogEntriesResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "process_oplog_entries",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let response = match self
            .process_oplog_entries_inner(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(process_oplog_entries_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                process_oplog_entries_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ProcessOplogEntriesResponse {
            result: Some(response),
        }))
    }
}

impl WorkerGrpcApi {
    pub fn new(worker_service: Arc<WorkerService>) -> Self {
        Self { worker_service }
    }

    async fn launch_new_worker(
        &self,
        request: LaunchNewWorkerRequest,
    ) -> Result<(AgentId, ComponentRevision, AgentFingerprint), GrpcAgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;

        let component_id: golem_common::model::component::ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;

        let agent_id = AgentId {
            component_id,
            agent_id: request.name,
        };

        let config = request
            .config
            .into_iter()
            .map(AgentConfigEntryDto::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| bad_request_error(format!("failed converting config: {e}")))?;

        let (latest_component_revision, fingerprint) = self
            .worker_service
            .create(
                &agent_id,
                request.env,
                config,
                request.ignore_already_existing,
                auth,
                request.context,
                request.principal,
            )
            .await?;

        Ok((agent_id, latest_component_revision, fingerprint))
    }

    async fn complete_promise(
        &self,
        request: CompletePromiseRequest,
    ) -> Result<bool, GrpcAgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let agent_id = validate_protobuf_agent_id(request.agent_id)?;

        let parameters = request
            .complete_parameters
            .ok_or_else(|| bad_request_error("Missing complete parameters"))?;

        let result = self
            .worker_service
            .complete_promise(&agent_id, parameters.oplog_idx, parameters.data, auth)
            .await?;

        Ok(result)
    }

    async fn resume_worker(&self, request: ResumeWorkerRequest) -> Result<(), GrpcAgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let agent_id = validate_protobuf_agent_id(request.agent_id)?;

        self.worker_service
            .resume(&agent_id, request.force.unwrap_or(false), auth)
            .await?;

        Ok(())
    }

    async fn update_worker(&self, request: UpdateWorkerRequest) -> Result<(), GrpcAgentError> {
        let worker_update_mode: AgentUpdateMode = request.mode().into();
        let disable_wakeup = request.disable_wakeup;
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let agent_id = validate_protobuf_agent_id(request.agent_id)?;
        let target_revision: ComponentRevision = request
            .target_revision
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting target_revision: {e}")))?;

        self.worker_service
            .update(
                &agent_id,
                worker_update_mode,
                target_revision,
                disable_wakeup,
                auth,
            )
            .await?;

        Ok(())
    }

    async fn fork_worker(&self, request: ForkWorkerRequest) -> Result<(), GrpcAgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let source_agent_id = validate_protobuf_agent_id(request.source_agent_id)?;
        let target_agent_id = validate_protobuf_agent_id(request.target_agent_id)?;
        let oplog_idx = OplogIndex::from_u64(request.oplog_index_cutoff);

        self.worker_service
            .fork_worker(&source_agent_id, &target_agent_id, oplog_idx, auth)
            .await?;

        Ok(())
    }

    async fn revert_worker(&self, request: RevertWorkerRequest) -> Result<(), GrpcAgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;

        let agent_id = validate_protobuf_agent_id(request.agent_id)?;

        let target = request
            .target
            .ok_or_else(|| bad_request_error("Missing target"))?
            .try_into()
            .map_err(|err| bad_request_error(format!("Invalid target {err}")))?;

        self.worker_service
            .revert_worker(&agent_id, target, auth)
            .await?;

        Ok(())
    }

    async fn invoke_agent(
        &self,
        request: InvokeAgentRequest,
    ) -> Result<InvokeAgentSuccess, GrpcAgentError> {
        let config = request
            .config
            .iter()
            .cloned()
            .map(|entry| entry.try_into().map_err(WorkerServiceError::TypeChecker))
            .collect::<Result<Vec<_>, _>>()?;

        let is_lookup =
            request.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup;

        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let trusted_internal_caller = matches!(&auth, AuthCtx::System | AuthCtx::Agent(_));
        let freshness_disposition = sanitize_invocation_freshness_disposition(
            request.freshness_disposition,
            trusted_internal_caller,
        );

        let agent_id = validate_protobuf_agent_id(request.agent_id)?;

        if !is_lookup {
            if request.method_name.is_none() {
                return Err(bad_request_error(
                    "method_name is required for non-lookup invocations",
                ));
            }
            if request.method_parameters.is_none() {
                return Err(bad_request_error(
                    "method_parameters is required for non-lookup invocations",
                ));
            }
        }

        let principal = request
            .principal
            .unwrap_or_else(|| golem_common::model::agent::Principal::anonymous().into());

        let output = self
            .worker_service
            .invoke_agent(
                &agent_id,
                request.method_name,
                request.method_parameters,
                request.mode,
                request.schedule_at,
                request.idempotency_key.map(|k| k.into()),
                request.context,
                trusted_internal_caller,
                freshness_disposition,
                config,
                auth,
                principal,
            )
            .await?;

        let result_value = match &output.result {
            golem_common::model::AgentInvocationResult::AgentMethod { output } => {
                Some(output.clone().try_into().map_err(|error| {
                    WorkerServiceError::Internal(format!(
                        "agent output cannot cross the gRPC boundary: {error}"
                    ))
                })?)
            }
            _ => None,
        };
        let proto_status = output
            .invocation_status
            .map(|s| golem_api_grpc::proto::golem::worker::InvocationStatus::from(s) as i32);
        Ok(InvokeAgentSuccess {
            result: result_value,
            fuel_consumed: output.consumed_fuel,
            component_revision: output.component_revision.map(|r| r.get()),
            status: proto_status,
            oplog_index: output.oplog_index.map(u64::from),
            agent_fingerprint: output.agent_fingerprint.map(|fp| fp.0.into()),
            agent_id: output.agent_id.map(Into::into),
            idempotency_key: output.idempotency_key.map(Into::into),
        })
    }

    async fn process_oplog_entries_inner(
        &self,
        request: ProcessOplogEntriesRequest,
    ) -> Result<(), GrpcAgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;

        let agent_id = validate_protobuf_agent_id(request.agent_id)?;

        let environment_id = request
            .environment_id
            .ok_or_else(|| bad_request_error("Missing environment_id"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("invalid environment_id: {e}")))?;

        let component_revision: ComponentRevision = request
            .component_revision
            .try_into()
            .map_err(|e| bad_request_error(format!("invalid component_revision: {e}")))?;

        let idempotency_key: IdempotencyKey = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency_key"))?
            .into();

        let account_id = request
            .account_id
            .ok_or_else(|| bad_request_error("Missing account_id"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("invalid account_id: {e}")))?;

        let metadata = request
            .metadata
            .ok_or_else(|| bad_request_error("Missing metadata"))?;

        self.worker_service
            .process_oplog_entries(
                &agent_id,
                environment_id,
                component_revision,
                idempotency_key,
                account_id,
                request.config,
                metadata,
                OplogIndex::from_u64(request.first_entry_index),
                request.entries,
                auth,
            )
            .await?;

        Ok(())
    }

    async fn cancel_invocation_inner(
        &self,
        request: CancelInvocationRequest,
    ) -> Result<bool, GrpcAgentError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;

        let agent_id = validate_protobuf_agent_id(request.agent_id)?;

        let idempotency_key: IdempotencyKey = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency_key"))?
            .into();

        let canceled = self
            .worker_service
            .cancel_invocation(&agent_id, &idempotency_key, auth)
            .await?;

        Ok(canceled)
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::sanitize_invocation_freshness_disposition;
    use golem_common::model::agent::InvocationFreshnessDisposition;
    use test_r::test;

    const KNOWN_FRESH_WIRE: i32 =
        golem_api_grpc::proto::golem::worker::v1::InvocationFreshnessDisposition::KnownFresh as i32;

    #[test]
    fn invocation_freshness_defaults_unknown_values_to_may_exist() {
        assert_eq!(
            sanitize_invocation_freshness_disposition(0, true),
            InvocationFreshnessDisposition::MayExist
        );
        assert_eq!(
            sanitize_invocation_freshness_disposition(i32::MAX, true),
            InvocationFreshnessDisposition::MayExist
        );
    }

    #[test]
    fn invocation_freshness_preserves_known_fresh_for_trusted_callers() {
        assert_eq!(
            sanitize_invocation_freshness_disposition(KNOWN_FRESH_WIRE, true),
            InvocationFreshnessDisposition::KnownFresh
        );
    }

    #[test]
    fn invocation_freshness_is_downgraded_for_untrusted_callers() {
        assert_eq!(
            sanitize_invocation_freshness_disposition(KNOWN_FRESH_WIRE, false),
            InvocationFreshnessDisposition::MayExist
        );
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::{validated_request_tail, validated_response_stream};
    use futures::{FutureExt, StreamExt, stream};
    use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
    use golem_api_grpc::proto::golem::common::Empty;
    use golem_api_grpc::proto::golem::schema::{SchemaValue, schema_value};
    use golem_api_grpc::proto::golem::worker::{
        AgentId, IdempotencyKey, InvocationAccepted, InvocationRequest, InvocationResponse,
        InvocationSessionCompletion, InvocationSessionResult, InvocationStart, invocation_request,
        invocation_response, invocation_session_completion, invocation_session_result,
    };
    use std::sync::Arc;
    use test_r::test;
    use tonic::Status;

    fn key() -> Option<IdempotencyKey> {
        Some(IdempotencyKey {
            value: "session-key".to_string(),
        })
    }

    fn agent_id() -> Option<AgentId> {
        Some(AgentId {
            component_id: None,
            name: "agent".to_string(),
        })
    }

    fn start() -> InvocationRequest {
        InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                input: Some(SchemaValue {
                    value: Some(schema_value::Value::U8Value(1)),
                }),
                idempotency_key: key(),
                ..Default::default()
            })),
        }
    }

    fn state_after_start() -> Arc<tokio::sync::Mutex<InvocationSessionState>> {
        let mut state = InvocationSessionState::default();
        state.validate_trusted_request(&start()).unwrap();
        Arc::new(tokio::sync::Mutex::new(state))
    }

    fn response(response: invocation_response::Response) -> InvocationResponse {
        InvocationResponse {
            response: Some(response),
        }
    }

    fn accepted() -> InvocationResponse {
        response(invocation_response::Response::Accepted(
            InvocationAccepted {
                agent_id: agent_id(),
                idempotency_key: key(),
                component_revision: Some(1),
            },
        ))
    }

    fn result() -> InvocationResponse {
        response(invocation_response::Response::Result(
            InvocationSessionResult {
                result: Some(invocation_session_result::Result::NoResult(Empty {})),
                component_revision: Some(1),
                agent_id: agent_id(),
                idempotency_key: key(),
                ..Default::default()
            },
        ))
    }

    fn successful_completion() -> InvocationResponse {
        response(invocation_response::Response::Finished(
            InvocationSessionCompletion {
                outcome: Some(invocation_session_completion::Outcome::Success(Empty {})),
            },
        ))
    }

    #[test]
    async fn request_transport_error_closes_the_internal_request_stream() {
        let inbound = stream::iter([Err(Status::unavailable("request transport failed"))]);
        let (mut tail, initial_requests_checked) =
            validated_request_tail(inbound, state_after_start());

        initial_requests_checked.await.unwrap();
        assert!(tail.next().await.is_none());
    }

    #[test]
    async fn malformed_request_tail_is_forwarded_for_semantic_terminalization() {
        let malformed = start();
        let inbound = stream::iter([Ok::<_, Status>(malformed.clone())]);
        let (mut tail, initial_requests_checked) =
            validated_request_tail(inbound, state_after_start());

        initial_requests_checked.await.unwrap();
        assert_eq!(tail.next().await, Some(malformed));
        assert!(tail.next().await.is_none());
    }

    #[test]
    async fn response_transport_error_is_preserved() {
        let inbound = stream::iter([Err(Status::unavailable("response transport failed"))]);
        let mut responses = validated_response_stream(inbound, state_after_start(), None);

        let error = responses.next().await.unwrap().unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unavailable);
        assert!(error.message().contains("response transport failed"));
        assert!(responses.next().await.is_none());
    }

    #[test]
    async fn valid_response_preserves_stream_free_lifecycle() {
        let inbound = stream::iter([
            Ok::<_, Status>(accepted()),
            Ok(result()),
            Ok(successful_completion()),
        ]);
        let mut responses = validated_response_stream(inbound, state_after_start(), None);

        assert!(matches!(
            responses.next().await.unwrap().unwrap().response,
            Some(invocation_response::Response::Accepted(_))
        ));
        assert!(matches!(
            responses.next().await.unwrap().unwrap().response,
            Some(invocation_response::Response::Result(_))
        ));
        assert!(matches!(
            responses.next().await.unwrap().unwrap().response,
            Some(invocation_response::Response::Finished(_))
        ));
        assert!(responses.next().await.is_none());
    }

    #[test]
    async fn terminal_response_is_withheld_until_the_upstream_closes_cleanly() {
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        sender.send(Ok::<_, Status>(accepted())).await.unwrap();
        sender.send(Ok(result())).await.unwrap();
        sender.send(Ok(successful_completion())).await.unwrap();
        let mut responses = validated_response_stream(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
            state_after_start(),
            None,
        );

        assert!(responses.next().await.unwrap().is_ok());
        assert!(responses.next().await.unwrap().is_ok());
        assert!(responses.next().now_or_never().is_none());

        drop(sender);
        assert!(matches!(
            responses.next().await.unwrap().unwrap().response,
            Some(invocation_response::Response::Finished(_))
        ));
        assert!(responses.next().await.is_none());
    }

    #[test]
    async fn response_error_after_terminal_suppresses_the_terminal() {
        let inbound = stream::iter([
            Ok::<_, Status>(accepted()),
            Ok(result()),
            Ok(successful_completion()),
            Err(Status::unavailable(
                "response transport failed after terminal",
            )),
        ]);
        let mut responses = validated_response_stream(inbound, state_after_start(), None);

        assert!(responses.next().await.unwrap().is_ok());
        assert!(responses.next().await.unwrap().is_ok());
        let error = responses.next().await.unwrap().unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unavailable);
        assert!(error.message().contains("after terminal"));
        assert!(responses.next().await.is_none());
    }

    #[test]
    async fn repeated_response_terminal_is_a_protocol_status() {
        let inbound = stream::iter([
            Ok::<_, Status>(accepted()),
            Ok(result()),
            Ok(successful_completion()),
            Ok(successful_completion()),
        ]);
        let mut responses = validated_response_stream(inbound, state_after_start(), None);

        assert!(responses.next().await.unwrap().is_ok());
        assert!(responses.next().await.unwrap().is_ok());
        let error = responses.next().await.unwrap().unwrap_err();
        assert_eq!(error.code(), tonic::Code::Internal);
        assert!(error.message().contains("after completion"));
        assert!(responses.next().await.is_none());
    }
}
