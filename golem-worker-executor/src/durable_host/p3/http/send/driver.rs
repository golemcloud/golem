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

use super::*;
use crate::durable_host::durability::{
    AsyncRetryDecision, DurableExecutionState, InFunctionRetryState, SemanticTrapRetryOverride,
};
use crate::durable_host::http::transport::{HttpTransport, TransportInvalidation};
use crate::services::oplog::OplogError;
use async_trait::async_trait;
use futures::future::{Either, select};
use futures::{StreamExt, stream};
use golem_common::model::{RetryPolicyState, Timestamp};
use golem_service_base::error::worker_executor::InterruptKind;
use std::time::Duration;

pub(super) struct PhysicalResponse {
    pub(super) response: http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
    pub(super) io: Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
    pub(super) invalidate: TransportInvalidation,
}

pub(super) struct SendFailure {
    pub(super) error: ErrorCode,
    pub(super) semantic_override: Option<SemanticTrapRetryOverride>,
}

pub(super) struct StatusFallback {
    pub(super) response: PhysicalResponse,
    pub(super) semantic_override: Option<SemanticTrapRetryOverride>,
}

pub(super) enum DriverExit {
    Response(Box<PhysicalResponse>),
    StatusFallback(Box<StatusFallback>),
    HttpError(Box<SendFailure>),
    Interrupted(InterruptKind),
    RecoveryError(anyhow::Error),
}

enum SendPhase {
    Dispatch,
    Drain {
        delay: Duration,
        failure: Box<ErrorCode>,
    },
    Backoff(Duration),
    Done,
}

enum SendEvent {
    Finish(DriverExit),
    Drain {
        delay: Duration,
        failure: Box<ErrorCode>,
    },
    Wait(Duration),
    Drained(bool),
    Waited(Result<(), InterruptKind>),
}

fn failed(error: ErrorCode, semantic_override: Option<SemanticTrapRetryOverride>) -> DriverExit {
    DriverExit::HttpError(Box::new(SendFailure {
        error,
        semantic_override,
    }))
}

fn transition(phase: SendPhase, event: SendEvent) -> (SendPhase, Option<DriverExit>) {
    match (phase, event) {
        (SendPhase::Dispatch, SendEvent::Finish(exit)) => (SendPhase::Done, Some(exit)),
        (SendPhase::Dispatch, SendEvent::Drain { delay, failure }) => {
            (SendPhase::Drain { delay, failure }, None)
        }
        (SendPhase::Dispatch, SendEvent::Wait(delay)) => (SendPhase::Backoff(delay), None),
        (SendPhase::Drain { delay, .. }, SendEvent::Drained(true)) => {
            (SendPhase::Backoff(delay), None)
        }
        (SendPhase::Drain { failure, .. }, SendEvent::Drained(false)) => {
            (SendPhase::Done, Some(failed(*failure, None)))
        }
        (SendPhase::Backoff(_), SendEvent::Waited(Ok(()))) => (SendPhase::Dispatch, None),
        (SendPhase::Backoff(_), SendEvent::Waited(Err(kind))) => {
            (SendPhase::Done, Some(DriverExit::Interrupted(kind)))
        }
        _ => unreachable!("invalid HTTP send retry transition"),
    }
}

/// Restricts durable waits to bodies that can still reach a resendable terminal.
struct BodyRetryHost<'a, H> {
    inner: &'a mut H,
    durable_wait: bool,
}

#[async_trait]
impl<H: InFunctionRetryHost + Send + Sync> InFunctionRetryHost for BodyRetryHost<'_, H> {
    fn current_retry_point(&self) -> OplogIndex {
        self.inner.current_retry_point()
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        self.inner.named_retry_policies().await
    }

    async fn current_retry_state_for(&self, index: OplogIndex) -> Option<RetryPolicyState> {
        self.inner.current_retry_state_for(index).await
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        let state = self.inner.durable_execution_state();
        DurableExecutionState {
            durable_wait: self.durable_wait,
            ..state
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        state: Option<RetryPolicyState>,
        retry_at: Timestamp,
    ) -> Result<(), OplogError> {
        self.inner
            .append_retry_error_entry(retry_from, state, retry_at)
            .await
    }
}

async fn dispatch<Ctx: WorkerCtx>(
    physical: &PhysicalSendRequest<Ctx>,
    transport: &(impl HttpTransport + ?Sized),
    host: &mut (impl InFunctionRetryHost + Send + Sync),
    retry_state: &mut InFunctionRetryState,
    inline_retry_eligible: bool,
    target: (&str, &str),
    interrupt: impl Future<Output = InterruptKind> + Unpin,
) -> SendEvent {
    let attempt = match select(Box::pin(physical.attempt(transport)), interrupt).await {
        Either::Left((result, _)) => result,
        Either::Right((kind, _)) => return SendEvent::Finish(DriverExit::Interrupted(kind)),
    };
    if let Some(error) = physical.body.recovery.error() {
        return SendEvent::Finish(DriverExit::RecoveryError(error.into()));
    }
    let (method, uri) = target;
    match attempt {
        Ok((response, io, invalidate)) => {
            let response = PhysicalResponse {
                response,
                io,
                invalidate,
            };
            let properties = RetryContext::http_with_response(
                method,
                uri,
                Some(response.response.status().as_u16()),
                "http-status",
            );
            let policy = if inline_retry_eligible {
                status_retry_policy(host, &properties).await
            } else {
                None
            };
            let Some(policy) = policy else {
                return SendEvent::Finish(DriverExit::Response(Box::new(response)));
            };
            let durable_wait = host.durable_execution_state().durable_wait
                && physical.body.may_replay_after_send_failure();
            let mut host = BodyRetryHost {
                inner: host,
                durable_wait,
            };
            match retry_state
                .decide_retry_for_named_policy(&mut host, "http-status-retry", &properties, &policy)
                .await
            {
                AsyncRetryDecision::RetryAfterDelay(delay) => {
                    (response.invalidate)();
                    drop(response);
                    physical.body.abandon_active_live_view();
                    SendEvent::Drain {
                        delay,
                        failure: Box::new(ErrorCode::InternalError(Some(
                            "request body could not be replayed in-function".to_string(),
                        ))),
                    }
                }
                AsyncRetryDecision::FallBackToTrap(semantic_override) => {
                    SendEvent::Finish(DriverExit::StatusFallback(Box::new(StatusFallback {
                        response,
                        semantic_override,
                    })))
                }
                AsyncRetryDecision::Exhausted => {
                    SendEvent::Finish(DriverExit::Response(Box::new(response)))
                }
            }
        }
        Err(error)
            if inline_retry_eligible
                && classify_serializable_http_error_code(&serialize_error_code(&error))
                    == HostFailureKind::Transient =>
        {
            let properties = RetryContext::http_with_response(method, uri, None, "transient");
            let durable_wait = host.durable_execution_state().durable_wait
                && physical.body.may_replay_after_send_failure();
            let mut host = BodyRetryHost {
                inner: host,
                durable_wait,
            };
            let label = if physical.body.frames_consumed() {
                "request-body-write"
            } else {
                "in-task"
            };
            match retry_state
                .decide_retry_with_properties(&mut host, label, &properties)
                .await
            {
                AsyncRetryDecision::RetryAfterDelay(delay) => {
                    if physical.body.can_replay_after_send_failure() {
                        SendEvent::Wait(delay)
                    } else if durable_wait {
                        physical.body.abandon_active_live_view();
                        SendEvent::Drain {
                            delay,
                            failure: Box::new(error),
                        }
                    } else {
                        SendEvent::Finish(failed(error, None))
                    }
                }
                AsyncRetryDecision::FallBackToTrap(semantic_override) => {
                    SendEvent::Finish(failed(error, semantic_override))
                }
                AsyncRetryDecision::Exhausted => SendEvent::Finish(failed(error, None)),
            }
        }
        Err(error) => SendEvent::Finish(failed(error, None)),
    }
}

pub(super) async fn drive_send<Ctx, H, I, IF, W, WF>(
    physical: &PhysicalSendRequest<Ctx>,
    transport: &(impl HttpTransport + ?Sized),
    retry_host: &mut H,
    inline_retry_eligible: bool,
    target: (&str, &str),
    interrupt: I,
    wait: W,
) -> DriverExit
where
    Ctx: WorkerCtx,
    H: InFunctionRetryHost + Send + Sync,
    I: Fn() -> IF + Sync,
    IF: Future<Output = InterruptKind> + Unpin,
    W: Fn(Duration) -> WF + Sync,
    WF: Future<Output = Result<(), InterruptKind>>,
{
    let interrupt = &interrupt;
    let wait = &wait;
    let steps = stream::unfold(
        (SendPhase::Dispatch, InFunctionRetryState::new(), retry_host),
        move |(phase, mut retry_state, host)| async move {
            let event = match &phase {
                SendPhase::Dispatch => {
                    dispatch(
                        physical,
                        transport,
                        host,
                        &mut retry_state,
                        inline_retry_eligible,
                        target,
                        interrupt(),
                    )
                    .await
                }
                SendPhase::Drain { .. } => SendEvent::Drained(matches!(
                    physical.body.drain_to_terminal().await,
                    DurableRequestBodyDrainOutcome::Replayable,
                )),
                SendPhase::Backoff(delay) => SendEvent::Waited(wait(*delay).await),
                SendPhase::Done => return None,
            };
            let (phase, exit) = transition(phase, event);
            Some((exit, (phase, retry_state, host)))
        },
    );
    let exits = steps.filter_map(futures::future::ready);
    futures::pin_mut!(exits);
    exits
        .next()
        .await
        .expect("the HTTP retry driver must return a terminal outcome")
}
