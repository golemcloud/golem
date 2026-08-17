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

use super::{WorkerExecutorImpl, extract_owned_agent_id};
use crate::durable_host::stream_session::LiveValueSession;
use crate::grpc::invocation::{CanStartWorker, from_proto_invocation_context};
use crate::services::{HasAll, HasComponentService, HasSchedulerService, UsesAllDeps};
use crate::worker::Worker;
use crate::worker::invocation::agent_method_uses_streams;
use crate::workerctx::WorkerCtx;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::worker::{
    InvocationFrame, InvocationProtocolFailure, InvocationResult, InvocationSessionFinished,
    InvocationStart, invocation_cancel, invocation_frame, invocation_protocol_failure,
    invocation_result, invocation_session_finished,
};
use golem_api_grpc::{expect_invocation_start, validate_invocation_request_tail};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentMode, InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult, IdempotencyKey,
    InvocationStatus, ScheduledAction,
};
use golem_common::schema::SchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

pub(super) type InvocationSessionStream =
    Pin<Box<dyn Stream<Item = Result<InvocationFrame, Status>> + Send + 'static>>;

pub(super) async fn invoke_agent_session<
    Ctx: WorkerCtx,
    Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static,
>(
    executor: &WorkerExecutorImpl<Ctx, Svcs>,
    request: Request<tonic::Streaming<InvocationFrame>>,
) -> Result<Response<InvocationSessionStream>, Status> {
    let inbound = request.into_inner();
    let (frames, receiver) = mpsc::channel(32);
    let executor = (*executor).clone();
    tokio::spawn(async move {
        executor.run_agent_session(inbound, frames).await;
    });
    Ok(Response::new(Box::pin(
        ReceiverStream::new(receiver).map(Ok),
    )))
}

fn decode_invocation_freshness_disposition(value: i32) -> InvocationFreshnessDisposition {
    if value == golem::worker::InvocationFreshnessDisposition::KnownFresh as i32 {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    async fn invoke_agent_internal(
        &self,
        request: &InvocationStart,
        method_parameters: Option<SchemaValue>,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let freshness_disposition =
            decode_invocation_freshness_disposition(request.freshness_disposition);

        let idempotency_key: Option<IdempotencyKey> =
            request.idempotency_key.clone().map(|k| k.into());

        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh
            && idempotency_key.is_none()
        {
            return Err(WorkerExecutorError::invalid_request(
                "KnownFresh requires an idempotency key",
            ));
        }

        let mode = request.mode();

        let ik = idempotency_key.unwrap_or(IdempotencyKey::fresh());
        let final_agent_id: AgentId = request
            .agent_id
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        if matches!(
            mode,
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup
        ) {
            if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                return Err(WorkerExecutorError::invalid_request(
                    "KnownFresh cannot be used for an invocation lookup",
                ));
            }
            let inv_status = match self.get_or_create_pending_for_lookup(request).await? {
                Some(worker) => match worker.lookup_invocation_result(&ik).await {
                    crate::model::LookupResult::Complete(Ok(_)) => InvocationStatus::Complete,
                    crate::model::LookupResult::Complete(Err(err)) => return Err(err),
                    crate::model::LookupResult::Pending => InvocationStatus::Pending,
                    crate::model::LookupResult::New | crate::model::LookupResult::Interrupted => {
                        InvocationStatus::Unknown
                    }
                },
                None => InvocationStatus::Unknown,
            };
            return Ok(AgentInvocationOutput {
                result: AgentInvocationResult::AgentInitialization,
                consumed_fuel: None,
                invocation_status: Some(inv_status),
                component_revision: None,
                agent_id: Some(final_agent_id),
                idempotency_key: Some(ik),
                oplog_index: None,
                agent_fingerprint: None,
            });
        }

        let method_name =
            request
                .method_name
                .clone()
                .ok_or(WorkerExecutorError::invalid_request(
                    "method_name is required for non-lookup invocations",
                ))?;

        let method_parameters = method_parameters.ok_or(WorkerExecutorError::invalid_request(
            "input is required for non-lookup invocations",
        ))?;

        let schedule_at: Option<DateTime<Utc>> = request
            .schedule_at
            .as_ref()
            .and_then(|ts| DateTime::from_timestamp(ts.seconds, ts.nanos as u32));

        let account_id: AccountId = request
            .component_owner_account_id
            .ok_or(WorkerExecutorError::invalid_request("account_id not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("Invalid account id: {e}"))
            })?;

        let owned_agent_id =
            extract_owned_agent_id(request, |r| &r.agent_id, |r| &r.environment_id)?;

        Worker::<Ctx>::validate_invocation_freshness(
            self,
            &owned_agent_id,
            &ik,
            freshness_disposition,
        )
        .await?;

        let principal: Principal = request
            .principal
            .clone()
            .map(|p| p.try_into())
            .transpose()
            .map_err(|e: String| {
                WorkerExecutorError::invalid_request(format!("failed converting principal: {e}"))
            })?
            .unwrap_or_else(Principal::anonymous);

        let invocation_context = self
            .limit_invocation_context_stack_depth(from_proto_invocation_context(&request.context));
        let worker_creation_principal = principal.clone();

        let invocation = AgentInvocation::AgentMethod {
            idempotency_key: ik.clone(),
            method_name: method_name.clone(),
            input: method_parameters.clone(),
            invocation_context,
            principal,
        };

        match mode {
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await => {
                let worker = self
                    .get_or_create_pending_with_freshness(request, freshness_disposition)
                    .await?;
                let component_revision = worker.get_last_known_status().await.component_revision;
                let component = self
                    .component_service()
                    .get_metadata(owned_agent_id.component_id(), Some(component_revision))
                    .await?;
                let parsed_agent_id =
                    ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata)
                        .map_err(WorkerExecutorError::invalid_request)?;
                let streaming = agent_method_uses_streams(
                    &component.metadata,
                    Some(&parsed_agent_id),
                    &method_name,
                    &method_parameters,
                )?;
                let mut invocation_output = if streaming {
                    let fingerprint = worker.get_initial_worker_metadata().fingerprint;
                    AgentInvocationOutput {
                        result: AgentInvocationResult::AgentMethod {
                            output: worker
                                .clone()
                                .invoke_live_streaming(invocation, cancellation)
                                .await?,
                        },
                        consumed_fuel: None,
                        invocation_status: None,
                        component_revision: Some(component.revision),
                        agent_id: None,
                        idempotency_key: None,
                        oplog_index: None,
                        agent_fingerprint: Some(fingerprint),
                    }
                } else {
                    worker.invoke_and_await(invocation).await?
                };
                invocation_output.agent_id = Some(final_agent_id);
                invocation_output.idempotency_key = Some(ik);
                Ok(invocation_output)
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule => {
                match schedule_at {
                    Some(scheduled_time) => {
                        let component = self
                            .component_service()
                            .get_metadata(owned_agent_id.component_id(), None)
                            .await?;
                        let parsed_agent_id = ParsedAgentId::parse(
                            &owned_agent_id.agent_id.agent_id,
                            &component.metadata,
                        )
                        .map_err(WorkerExecutorError::invalid_request)?;
                        let agent_mode = component
                            .metadata
                            .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
                            .map(|agent_type| agent_type.mode)
                            .ok_or_else(|| {
                                WorkerExecutorError::invalid_request(
                                    "Scheduled invocation target is not a registered agent type",
                                )
                            })?;
                        let action = if agent_mode == AgentMode::Ephemeral {
                            ScheduledAction::InvokeEphemeral {
                                account_id,
                                owned_agent_id,
                                invocation: Box::new(invocation),
                                component_revision: component.revision,
                                env: request.env().unwrap_or_default(),
                                config: request.config()?,
                                parent: request.parent(),
                                creation_principal: Box::new(worker_creation_principal),
                            }
                        } else {
                            let worker = self
                                .get_or_create_pending_with_freshness(
                                    request,
                                    freshness_disposition,
                                )
                                .await?;
                            let target_worker_fingerprint =
                                worker.get_initial_worker_metadata().fingerprint;
                            ScheduledAction::Invoke {
                                account_id,
                                owned_agent_id,
                                invocation: Box::new(invocation),
                                target_worker_fingerprint,
                            }
                        };
                        self.scheduler_service()
                            .schedule(scheduled_time, action)
                            .await;
                        Ok(AgentInvocationOutput {
                            result: AgentInvocationResult::AgentInitialization,
                            consumed_fuel: None,
                            invocation_status: None,
                            component_revision: None,
                            agent_id: Some(final_agent_id),
                            idempotency_key: Some(ik),
                            oplog_index: None,
                            agent_fingerprint: None,
                        })
                    }
                    None => {
                        let worker = self
                            .get_or_create_pending_with_freshness(request, freshness_disposition)
                            .await?;
                        match worker.clone().invoke(invocation).await? {
                            crate::worker::ResultOrSubscription::Finished(Err(err)) => {
                                return Err(err);
                            }
                            crate::worker::ResultOrSubscription::Finished(Ok(_)) => {}
                            crate::worker::ResultOrSubscription::Pending(_) => {
                                Worker::start_if_needed(worker).await?;
                            }
                        }
                        Ok(AgentInvocationOutput {
                            result: AgentInvocationResult::AgentInitialization,
                            consumed_fuel: None,
                            invocation_status: None,
                            component_revision: None,
                            agent_id: Some(final_agent_id),
                            idempotency_key: Some(ik),
                            oplog_index: None,
                            agent_fingerprint: None,
                        })
                    }
                }
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup => {
                unreachable!("Lookup mode handled above")
            }
        }
    }

    async fn run_agent_session(
        &self,
        mut inbound: tonic::Streaming<InvocationFrame>,
        frames: mpsc::Sender<InvocationFrame>,
    ) {
        let start = match inbound.message().await {
            Ok(Some(frame)) => match expect_invocation_start(frame) {
                Ok(start) => start,
                Err(error) => {
                    send_protocol_failure(
                        &frames,
                        invocation_protocol_failure::Kind::Protocol,
                        error,
                    )
                    .await;
                    return;
                }
            },
            Ok(None) => {
                send_protocol_failure(
                    &frames,
                    invocation_protocol_failure::Kind::Transport,
                    "invocation request transport closed before start".to_string(),
                )
                .await;
                return;
            }
            Err(error) => {
                send_protocol_failure(
                    &frames,
                    invocation_protocol_failure::Kind::Transport,
                    error.to_string(),
                )
                .await;
                return;
            }
        };

        let session = LiveValueSession::new(2, frames.clone());
        let input =
            if start.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup {
                Ok(None)
            } else {
                match start.input.clone() {
                    Some(input) => session.decode(input).await.map(Some),
                    None => Err("invocation start has no input".to_string()),
                }
            };
        let input = match input {
            Ok(input) => input,
            Err(error) => {
                send_protocol_failure(&frames, invocation_protocol_failure::Kind::Protocol, error)
                    .await;
                session.cancel();
                return;
            }
        };

        let cancellation = tokio_util::sync::CancellationToken::new();
        let _cancel_on_drop = cancellation.clone().drop_guard();
        let invocation = self.invoke_agent_internal(&start, input, cancellation);
        tokio::pin!(invocation);
        let output = loop {
            tokio::select! {
                result = &mut invocation => break result,
                frame = inbound.message() => match frame {
                    Ok(Some(frame)) => {
                        if !route_live_request_frame(&session, &frames, frame).await {
                            return;
                        }
                    }
                    Ok(None) => {
                        fail_session_transport(
                            &session,
                            &frames,
                            "invocation request transport closed before the result".to_string(),
                        )
                        .await;
                        return;
                    }
                    Err(error) => {
                        fail_session_transport(&session, &frames, error.to_string()).await;
                        return;
                    }
                }
            }
        };

        let output = match output {
            Ok(output) => output,
            Err(error) => {
                send_worker_failure(&frames, error).await;
                session.cancel();
                return;
            }
        };
        let result = match &output.result {
            AgentInvocationResult::AgentMethod { output } => match session.encode(output) {
                Ok(output) => Some(invocation_result::Result::MethodResult(output)),
                Err(error) => {
                    send_protocol_failure(
                        &frames,
                        invocation_protocol_failure::Kind::Protocol,
                        error,
                    )
                    .await;
                    session.cancel();
                    return;
                }
            },
            _ => Some(invocation_result::Result::NoResult(golem::common::Empty {})),
        };
        if frames
            .send(InvocationFrame {
                frame: Some(invocation_frame::Frame::Result(InvocationResult {
                    result,
                    fuel_consumed: output.consumed_fuel,
                    component_revision: output.component_revision.map(|revision| revision.get()),
                    status: output.invocation_status.map(|status| {
                        golem_api_grpc::proto::golem::worker::InvocationStatus::from(status) as i32
                    }),
                    oplog_index: output.oplog_index.map(u64::from),
                    agent_fingerprint: output
                        .agent_fingerprint
                        .map(|fingerprint| fingerprint.0.into()),
                    agent_id: output.agent_id.map(Into::into),
                    idempotency_key: output.idempotency_key.map(Into::into),
                })),
            })
            .await
            .is_err()
        {
            session.cancel();
            return;
        }

        let idle = session.wait_idle();
        tokio::pin!(idle);
        loop {
            tokio::select! {
                () = &mut idle => {
                    let _ = frames.send(InvocationFrame {
                        frame: Some(invocation_frame::Frame::Finished(
                            InvocationSessionFinished {
                                outcome: Some(invocation_session_finished::Outcome::Success(
                                    golem::common::Empty {},
                                )),
                            },
                        )),
                    }).await;
                    return;
                }
                frame = inbound.message() => match frame {
                    Ok(Some(frame)) => {
                        if !route_live_request_frame(&session, &frames, frame).await {
                            return;
                        }
                    }
                    Ok(None) => {
                        fail_session_transport(
                            &session,
                            &frames,
                            "invocation request transport closed before completion".to_string(),
                        )
                        .await;
                        return;
                    }
                    Err(error) => {
                        fail_session_transport(&session, &frames, error.to_string()).await;
                        return;
                    }
                }
            }
        }
    }
}

async fn send_protocol_failure(
    frames: &mpsc::Sender<InvocationFrame>,
    kind: invocation_protocol_failure::Kind,
    details: String,
) {
    let _ = frames
        .send(InvocationFrame {
            frame: Some(invocation_frame::Frame::Finished(
                InvocationSessionFinished {
                    outcome: Some(invocation_session_finished::Outcome::ProtocolFailure(
                        InvocationProtocolFailure {
                            kind: kind as i32,
                            details,
                        },
                    )),
                },
            )),
        })
        .await;
}

async fn send_worker_failure(frames: &mpsc::Sender<InvocationFrame>, error: WorkerExecutorError) {
    let _ = frames
        .send(InvocationFrame {
            frame: Some(invocation_frame::Frame::Finished(
                InvocationSessionFinished {
                    outcome: Some(invocation_session_finished::Outcome::Failure(error.into())),
                },
            )),
        })
        .await;
}

async fn fail_session_transport(
    session: &LiveValueSession,
    frames: &mpsc::Sender<InvocationFrame>,
    details: String,
) {
    session.fail(details.clone());
    send_protocol_failure(
        frames,
        invocation_protocol_failure::Kind::Transport,
        details,
    )
    .await;
}

async fn route_live_request_frame(
    session: &LiveValueSession,
    frames: &mpsc::Sender<InvocationFrame>,
    frame: InvocationFrame,
) -> bool {
    let frame = match validate_invocation_request_tail(frame) {
        Ok(frame) => frame.frame.expect("validated request frame has a payload"),
        Err(details) => {
            session.fail(details.clone());
            send_protocol_failure(frames, invocation_protocol_failure::Kind::Protocol, details)
                .await;
            return false;
        }
    };
    match frame {
        invocation_frame::Frame::Cancel(cancel) => {
            let kind = match invocation_cancel::Kind::try_from(cancel.kind) {
                Ok(kind) => kind,
                Err(_) => {
                    let details = format!("invalid invocation cancellation kind {}", cancel.kind);
                    session.fail(details.clone());
                    send_protocol_failure(
                        frames,
                        invocation_protocol_failure::Kind::Protocol,
                        details,
                    )
                    .await;
                    return false;
                }
            };
            match kind {
                invocation_cancel::Kind::Semantic => session.cancel(),
                invocation_cancel::Kind::Transport => {
                    let details = cancel
                        .details
                        .unwrap_or_else(|| "invocation request transport failed".to_string());
                    fail_session_transport(session, frames, details).await;
                }
                invocation_cancel::Kind::Protocol => {
                    let details = cancel
                        .details
                        .unwrap_or_else(|| "invalid invocation request frame".to_string());
                    session.fail(details.clone());
                    send_protocol_failure(
                        frames,
                        invocation_protocol_failure::Kind::Protocol,
                        details,
                    )
                    .await;
                }
            }
            false
        }
        frame => match session.route_stream_frame(frame).await {
            Ok(true) => true,
            Ok(false) => {
                let details = "unexpected frame on invocation request stream".to_string();
                session.fail(details.clone());
                send_protocol_failure(frames, invocation_protocol_failure::Kind::Protocol, details)
                    .await;
                false
            }
            Err(details) => {
                session.fail(details.clone());
                send_protocol_failure(frames, invocation_protocol_failure::Kind::Protocol, details)
                    .await;
                false
            }
        },
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::decode_invocation_freshness_disposition;
    use golem_common::model::agent::InvocationFreshnessDisposition;
    use test_r::test;

    #[test]
    fn invocation_freshness_defaults_unknown_values_to_may_exist() {
        assert_eq!(
            decode_invocation_freshness_disposition(0),
            InvocationFreshnessDisposition::MayExist
        );
        assert_eq!(
            decode_invocation_freshness_disposition(i32::MAX),
            InvocationFreshnessDisposition::MayExist
        );
    }

    #[test]
    fn invocation_freshness_decodes_known_fresh_explicitly() {
        assert_eq!(
            decode_invocation_freshness_disposition(
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                    as i32
            ),
            InvocationFreshnessDisposition::KnownFresh
        );
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::route_live_request_frame;
    use crate::durable_host::stream_session::LiveValueSession;
    use golem_api_grpc::invocation_cancel_frame;
    use golem_api_grpc::proto::golem::worker::invocation_session_finished::Outcome;
    use golem_api_grpc::proto::golem::worker::{
        InvocationSessionFinished, invocation_cancel, invocation_frame, invocation_protocol_failure,
    };
    use test_r::test;
    use tokio::sync::mpsc;

    #[test]
    async fn semantic_cancellation_does_not_report_transport_failure() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(2, frames.clone());

        assert!(
            !route_live_request_frame(
                &session,
                &frames,
                invocation_cancel_frame(invocation_cancel::Kind::Semantic, None),
            )
            .await
        );
        assert!(frame_rx.try_recv().is_err());
    }

    #[test]
    async fn transport_cancellation_reports_typed_failure() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(2, frames.clone());

        assert!(
            !route_live_request_frame(
                &session,
                &frames,
                invocation_cancel_frame(
                    invocation_cancel::Kind::Transport,
                    Some("connection lost".to_string()),
                ),
            )
            .await
        );
        let frame = frame_rx.recv().await.unwrap().frame.unwrap();
        assert!(matches!(
            frame,
            invocation_frame::Frame::Finished(InvocationSessionFinished {
                outcome: Some(Outcome::ProtocolFailure(failure)),
            }) if failure.kind == invocation_protocol_failure::Kind::Transport as i32
                && failure.details == "connection lost"
        ));
    }

    #[test]
    async fn protocol_cancellation_reports_typed_failure() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(2, frames.clone());

        assert!(
            !route_live_request_frame(
                &session,
                &frames,
                invocation_cancel_frame(
                    invocation_cancel::Kind::Protocol,
                    Some("duplicate start".to_string()),
                ),
            )
            .await
        );
        let frame = frame_rx.recv().await.unwrap().frame.unwrap();
        assert!(matches!(
            frame,
            invocation_frame::Frame::Finished(InvocationSessionFinished {
                outcome: Some(Outcome::ProtocolFailure(failure)),
            }) if failure.kind == invocation_protocol_failure::Kind::Protocol as i32
                && failure.details == "duplicate start"
        ));
    }
}
