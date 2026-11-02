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

use super::{
    FromSchema, InvocationFailure, InvocationFailureKind, InvocationRejected,
    InvocationRejectionReason, InvocationResponse, InvocationSessionCompletion, InvocationStart,
    OplogError, SchemaValueStream, SessionError, StreamSession, ToolInvocationOutput,
    WorkerExecutionError, WorkerExecutorError, decode_recursive_stream_value, invocation_response,
    invocation_session_completion, invocation_session_result, mpsc,
};

pub(super) async fn persisted_invocation_result(
    streams: &StreamSession,
    native_tool: bool,
) -> Result<
    Option<(
        invocation_session_result::Result,
        Vec<golem_api_grpc::proto::golem::worker::DurableStreamMapping>,
    )>,
    SessionError,
> {
    let Some(result) = streams.persisted_result().await? else {
        return Ok(None);
    };
    let mappings = result.proto_mappings();
    if native_tool {
        let value = decode_recursive_stream_value(result.value, |_, _| {
            Ok(SchemaValueStream::from_host_endpoint(()))
        })?;
        let output = ToolInvocationOutput::from_value(&value).map_err(|error| error.to_string())?;
        let public = match output.outcome {
            Ok(result) => golem_common::model::oplog::PublicExternalToolResult::Success(result),
            Err(error) => golem_common::model::oplog::PublicExternalToolResult::Failure(error),
        };
        Ok(Some((
            invocation_session_result::Result::ToolResult(public.try_into()?),
            Vec::new(),
        )))
    } else {
        Ok(Some((
            invocation_session_result::Result::MethodResult(result.value),
            mappings,
        )))
    }
}

/// Reads the committed invocation result and session terminal together.
///
/// The session commits its result before its successful terminal, so the terminal is read first:
/// a successful terminal seen by the first read guarantees that the second read sees the result.
pub(super) async fn persisted_result_and_finished(
    streams: &StreamSession,
    native_tool: bool,
) -> Result<
    (
        Option<(
            invocation_session_result::Result,
            Vec<golem_api_grpc::proto::golem::worker::DurableStreamMapping>,
        )>,
        Option<Result<(), Vec<u8>>>,
    ),
    SessionError,
> {
    let finished = streams.persisted_finished().await?;
    let result = persisted_invocation_result(streams, native_tool).await?;
    Ok((result, finished))
}

pub(super) async fn send_attachment_revocation(
    responses: &mpsc::Sender<InvocationResponse>,
    revocation: Result<(), SessionError>,
) {
    match revocation {
        Ok(()) => {
            let _ = responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::AttachmentRevoked(
                        golem_api_grpc::proto::golem::worker::AttachmentRevoked {
                            details: "the durable attachment was fenced by a later epoch"
                                .to_string(),
                        },
                    )),
                })
                .await;
        }
        Err(error) => send_protocol_failure(responses, error).await,
    }
}

pub(super) async fn send_resumed_finished(
    responses: &mpsc::Sender<InvocationResponse>,
    result: Result<(), Vec<u8>>,
) {
    let outcome = match result {
        Ok(()) => invocation_session_completion::Outcome::Success(
            golem_schema::proto::golem::common::Empty {},
        ),
        Err(details) => invocation_session_completion::Outcome::Failure(InvocationFailure {
            kind: InvocationFailureKind::Execution as i32,
            code: "persisted-invocation-failure".to_string(),
            message: String::from_utf8_lossy(&details).into_owned(),
            worker_error: None,
        }),
    };
    let _ = responses
        .send(InvocationResponse {
            response: Some(invocation_response::Response::Finished(
                InvocationSessionCompletion {
                    outcome: Some(outcome),
                },
            )),
        })
        .await;
}

pub(super) fn pre_acceptance_rejection_reason(
    error: &WorkerExecutorError,
) -> InvocationRejectionReason {
    match error {
        WorkerExecutorError::InvalidRequest { details } => {
            if details.starts_with("IdempotencyConflict:") {
                InvocationRejectionReason::IdempotencyConflict
            } else if details.starts_with("AttemptConflict:") {
                InvocationRejectionReason::AttemptConflict
            } else if details.starts_with("ResourceExhausted:") {
                InvocationRejectionReason::ResourceExhausted
            } else if details.starts_with("StaleEpoch:") {
                InvocationRejectionReason::StaleEpoch
            } else if details.starts_with("InvalidEpoch:") {
                InvocationRejectionReason::InvalidEpoch
            } else if details.starts_with("InvalidAttachmentState:") {
                InvocationRejectionReason::InvalidAttachmentState
            } else if details.starts_with("Unauthorized:") {
                InvocationRejectionReason::Unauthorized
            } else if details.starts_with("NotFound:") {
                InvocationRejectionReason::NotFound
            } else {
                InvocationRejectionReason::Validation
            }
        }
        WorkerExecutorError::ParamTypeMismatch { .. }
        | WorkerExecutorError::NoValueInMessage
        | WorkerExecutorError::ValueMismatch { .. } => InvocationRejectionReason::Validation,
        WorkerExecutorError::AgentNotFound { .. }
        | WorkerExecutorError::ComponentNotFound { .. }
        | WorkerExecutorError::PromiseNotFound { .. } => InvocationRejectionReason::NotFound,
        WorkerExecutorError::InvalidAccount => InvocationRejectionReason::Unauthorized,
        _ => InvocationRejectionReason::Internal,
    }
}

pub(super) async fn send_unvalidated_rejection(
    responses: &mpsc::Sender<InvocationResponse>,
    reason: InvocationRejectionReason,
    error: String,
    idempotency_key: Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
    agent_id: Option<golem_api_grpc::proto::golem::worker::AgentId>,
    worker_error: Option<WorkerExecutionError>,
) {
    let _ = responses
        .send(InvocationResponse {
            response: Some(invocation_response::Response::Rejected(
                InvocationRejected {
                    reason: reason as i32,
                    error,
                    idempotency_key,
                    agent_id,
                    component_revision: None,
                    worker_error,
                },
            )),
        })
        .await;
}

pub(super) async fn send_rejection(
    responses: &mpsc::Sender<InvocationResponse>,
    reason: InvocationRejectionReason,
    error: String,
    start: &InvocationStart,
) {
    send_rejection_with_worker_error(responses, reason, error, None, start).await;
}

pub(super) async fn send_rejection_with_worker_error(
    responses: &mpsc::Sender<InvocationResponse>,
    reason: InvocationRejectionReason,
    error: String,
    worker_error: Option<WorkerExecutionError>,
    start: &InvocationStart,
) {
    send_unvalidated_rejection(
        responses,
        reason,
        error,
        start.idempotency_key.clone(),
        start.agent_id.clone(),
        worker_error,
    )
    .await;
}

async fn send_failure(
    responses: &mpsc::Sender<InvocationResponse>,
    kind: InvocationFailureKind,
    code: &str,
    message: String,
    worker_error: Option<WorkerExecutionError>,
) {
    tracing::warn!(
        kind = ?kind,
        code,
        message = %message,
        worker_error = ?worker_error,
        "Invocation session finished with failure"
    );
    let _ = responses
        .send(InvocationResponse {
            response: Some(invocation_response::Response::Finished(
                InvocationSessionCompletion {
                    outcome: Some(invocation_session_completion::Outcome::Failure(
                        InvocationFailure {
                            kind: kind as i32,
                            code: code.to_string(),
                            message,
                            worker_error,
                        },
                    )),
                },
            )),
        })
        .await;
}

/// The worker error a session rejection carries for `error`. A refused write is rejected as the
/// worker error it is, which the worker service reads as a routing miss; an unreadable oplog is
/// rejected as the failure of its agent.
pub(super) fn rejection_worker_error(error: &SessionError) -> Option<WorkerExecutionError> {
    match error {
        SessionError::Fenced(fence) => {
            Some(WorkerExecutorError::from(OplogError::Fenced(fence.clone())).into())
        }
        SessionError::Unreadable(unreadable) => {
            Some(WorkerExecutorError::from(unreadable.clone()).into())
        }
        SessionError::Failed(_) => None,
    }
}

/// Finishes the session on a failure the protocol reports as text. A refused oplog write is not
/// one: it goes out as the worker error it is, which the worker service reads as a routing miss
/// and retries on the shard's new owner.
pub(super) async fn send_protocol_failure(
    responses: &mpsc::Sender<InvocationResponse>,
    error: impl Into<SessionError>,
) {
    match error.into() {
        SessionError::Fenced(fence) => {
            send_worker_failure(
                responses,
                WorkerExecutorError::from(OplogError::Fenced(fence)),
            )
            .await
        }
        SessionError::Unreadable(unreadable) => {
            send_worker_failure(responses, WorkerExecutorError::from(unreadable)).await
        }
        SessionError::Failed(details) => {
            send_failure(
                responses,
                InvocationFailureKind::Protocol,
                "protocol",
                details,
                None,
            )
            .await
        }
    }
}

pub(super) async fn send_worker_failure(
    responses: &mpsc::Sender<InvocationResponse>,
    error: WorkerExecutorError,
) {
    let message = error.to_string();
    send_failure(
        responses,
        InvocationFailureKind::Execution,
        "worker-execution",
        message,
        Some(error.into()),
    )
    .await;
}
