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
use golem_api_grpc::invocation_session_protocol::{
    OPLOG_UNREADABLE_FAILURE_CODE, oplog_read_failure,
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
    if let Err(error) = revocation {
        send_protocol_failure(responses, error).await;
        return;
    }
    let _ = responses
        .send(InvocationResponse {
            response: Some(invocation_response::Response::AttachmentRevoked(
                golem_api_grpc::proto::golem::worker::AttachmentRevoked {
                    details: "the durable attachment was fenced by a later epoch".to_string(),
                },
            )),
        })
        .await;
}

/// Answers an attachment check that failed. A check that the attachment state refuses revokes the
/// attachment. A failed oplog read, a refused write and an incomplete output keep their type: the
/// session finishes with the worker error, so the refusal is not taken as a revocation.
pub(super) async fn send_failed_attachment_check(
    responses: &mpsc::Sender<InvocationResponse>,
    error: SessionError,
) {
    match error {
        SessionError::Failed(_) => send_attachment_revocation(responses, Ok(())).await,
        error => send_protocol_failure(responses, error).await,
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
        | WorkerExecutorError::ValueMismatch { .. }
        | WorkerExecutorError::InvocationCancelled => InvocationRejectionReason::Validation,
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
    idempotency_key: Option<golem_schema::proto::golem::common::Uuid>,
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
        (start.mode == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32)
            .then(|| {
                start
                    .idempotency_key
                    .as_ref()?
                    .value
                    .parse::<golem_common::model::InvocationId>()
                    .ok()
                    .map(Into::into)
            })
            .flatten(),
        super::request::start_agent_id(start),
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
        error @ SessionError::IncompleteOutput(_) => Some(
            error
                .clone()
                .into_worker_executor_error(WorkerExecutorError::runtime)
                .into(),
        ),
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
        error @ SessionError::IncompleteOutput(_) => {
            send_worker_failure(
                responses,
                error.into_worker_executor_error(WorkerExecutorError::runtime),
            )
            .await
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
    let worker_error = WorkerExecutionError::from(error);
    let code = if oplog_read_failure(&worker_error).is_some() {
        OPLOG_UNREADABLE_FAILURE_CODE
    } else {
        "worker-execution"
    };
    send_failure(
        responses,
        InvocationFailureKind::Execution,
        code,
        message,
        Some(worker_error),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::oplog::{OplogReadError, UnreadableOplog};
    use futures::FutureExt;
    use golem_common::model::{AgentId, OplogIndex};
    use test_r::test;

    fn failure_code(error: WorkerExecutorError) -> (String, Option<WorkerExecutionError>) {
        let (responses, mut rx) = mpsc::channel(1);
        send_worker_failure(&responses, error)
            .now_or_never()
            .unwrap();
        let Some(invocation_response::Response::Finished(InvocationSessionCompletion {
            outcome: Some(invocation_session_completion::Outcome::Failure(failure)),
        })) = rx.try_recv().unwrap().response
        else {
            panic!("expected a failure frame");
        };
        (failure.code, failure.worker_error)
    }

    /// Every failure that a failed oplog read caused gets the read code and a typed read
    /// marker, also when a failed resume wraps it. Other failures get neither.
    #[test]
    fn a_failed_oplog_read_keeps_its_marker_and_code_through_a_failed_resume() {
        let agent_id = AgentId::from_parts(7, 0x43);
        let failed = WorkerExecutorError::from(UnreadableOplog::new(
            &agent_id,
            OplogReadError::Gap {
                start: OplogIndex::from_u64(43),
                end: OplogIndex::from_u64(44),
            },
        ));
        let unavailable = WorkerExecutorError::oplog_read_unavailable(
            "primary storage of agent 7 did not answer",
        );
        [
            failed.clone(),
            unavailable.clone(),
            WorkerExecutorError::failed_to_resume_worker(agent_id, failed),
            WorkerExecutorError::failed_to_resume_worker(agent_id, unavailable),
        ]
        .into_iter()
        .for_each(|error| {
            let (code, worker_error) = failure_code(error.clone());
            assert_eq!(code, OPLOG_UNREADABLE_FAILURE_CODE, "{error:?}");
            assert!(
                worker_error.as_ref().and_then(oplog_read_failure).is_some(),
                "{error:?}"
            );
        });
        [
            WorkerExecutorError::recovery_required("trapped body cleanup failed"),
            WorkerExecutorError::runtime("oplog entry 43 has unknown serialization version 255"),
            WorkerExecutorError::failed_to_resume_worker(
                agent_id,
                WorkerExecutorError::runtime("the component did not instantiate"),
            ),
        ]
        .into_iter()
        .for_each(|error| {
            let (code, worker_error) = failure_code(error.clone());
            assert_eq!(code, "worker-execution", "{error:?}");
            assert!(
                worker_error.as_ref().and_then(oplog_read_failure).is_none(),
                "{error:?}"
            );
        });
    }

    /// An attachment check that the attachment state refuses revokes the attachment, whatever its
    /// text. A failed read keeps its typed worker error and is no revocation.
    #[test]
    fn a_failed_attachment_check_revokes_only_on_a_refused_attachment_state() {
        let (responses, mut rx) = mpsc::channel(1);
        [
            SessionError::from("durable session has no attachment authority"),
            SessionError::from("an attachment state refusal with any text"),
        ]
        .into_iter()
        .for_each(|error| {
            send_failed_attachment_check(&responses, error)
                .now_or_never()
                .unwrap();
            assert!(matches!(
                rx.try_recv().unwrap().response,
                Some(invocation_response::Response::AttachmentRevoked(_))
            ));
        });

        let agent_id = AgentId::from_parts(7, 0x43);
        let unreadable = UnreadableOplog::new(
            &agent_id,
            OplogReadError::Gap {
                start: OplogIndex::from_u64(43),
                end: OplogIndex::from_u64(44),
            },
        );
        send_failed_attachment_check(&responses, SessionError::Unreadable(unreadable.clone()))
            .now_or_never()
            .unwrap();
        let Some(invocation_response::Response::Finished(InvocationSessionCompletion {
            outcome: Some(invocation_session_completion::Outcome::Failure(failure)),
        })) = rx.try_recv().unwrap().response
        else {
            panic!("a failed read is no revocation");
        };
        assert_eq!(
            failure.worker_error,
            Some(WorkerExecutorError::from(unreadable).into())
        );
    }
}
