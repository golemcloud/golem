// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

mod append;
mod encoding;
mod load;
mod read;
mod session;

use super::call_agent::CallAgentHandler;
use super::error::RequestHandlerError;
use super::route_resolver::ResolvedRouteEntry;
use super::{ResponseBody, RichRequest, RouteExecutionResult};
use crate::config::DurableStreamsConfig;
use crate::service::worker::{WorkerService, WorkerServiceError};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ReadStreamSlotRequest, ReadStreamSlotSuccess,
};
use golem_common::model::agent::{AgentMode, ephemeral_invocation_phantom_id};
use golem_common::model::invocation_session_public::{
    new_durable_stream_session_id, validate_durable_stream_session_id,
};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_service_base::custom_api::CallAgentBehaviour;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use load::{DurableStreamLoadLimiter, LoadRejection};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const MAX_ITEMS: u32 = 4096;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub struct DurableStreamsHandler {
    worker_service: Arc<WorkerService>,
    call_agent: Arc<CallAgentHandler>,
    limiter: DurableStreamLoadLimiter,
    long_poll_timeout: Duration,
    max_append_body_bytes: usize,
}

impl DurableStreamsHandler {
    /// Creates the HTTP handler with shared worker access and node-local load limits.
    pub fn new(
        worker_service: Arc<WorkerService>,
        call_agent: Arc<CallAgentHandler>,
        config: &DurableStreamsConfig,
    ) -> Self {
        Self {
            worker_service,
            call_agent,
            limiter: DurableStreamLoadLimiter::new(config.load.clone()),
            long_poll_timeout: config.long_poll_timeout,
            max_append_body_bytes: config.max_append_body_bytes,
        }
    }

    /// Validates the durable-stream route suffix and dispatches session or slot operations.
    pub async fn handle(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let mut suffix = classify(
            behaviour.base_path_variables,
            &route.captured_path_parameters,
        );
        if suffix.reserved {
            return Ok(response(StatusCode::NOT_FOUND));
        }
        if has_header(request, "stream-ttl") || has_header(request, "stream-expires-at") {
            return Ok(response(StatusCode::BAD_REQUEST));
        }
        if has_header(request, "stream-forked-from") {
            return Ok(response(StatusCode::NOT_IMPLEMENTED));
        }

        let generated_session =
            request.underlying.method() == Method::PUT && suffix.session.is_none();
        if generated_session {
            suffix.session = Some(new_durable_stream_session_id());
        }
        if suffix
            .session
            .as_deref()
            .is_some_and(|session| validate_durable_stream_session_id(session).is_err())
        {
            return Ok(response(StatusCode::NOT_FOUND));
        }
        let phantom = behaviour.phantom.then(|| {
            let session = suffix.session.as_deref().unwrap_or_default();
            if behaviour.agent_mode == AgentMode::Ephemeral {
                ephemeral_invocation_phantom_id(&IdempotencyKey::new(session.to_owned()))
            } else {
                stable_phantom(session)
            }
        });
        let agent_id = CallAgentHandler::build_agent_id(
            route,
            behaviour.component_id,
            &behaviour.agent_type,
            &behaviour.constructor_input,
            &behaviour.constructor_parameters,
            phantom,
        )?;
        match (request.underlying.method(), suffix.session, suffix.slot) {
            (&Method::POST, Some(session), Some(slot)) => {
                self.append(request, route, behaviour, &agent_id, &session, &slot)
                    .await
            }
            (&Method::POST, _, _) => Ok(append::read_only_response()),
            (&Method::DELETE, Some(session), slot) => {
                self.delete(route, &agent_id, session, slot).await
            }
            (&Method::PUT, Some(session), None) if generated_session => {
                self.create_generated(request, route, behaviour, &agent_id, &session)
                    .await
            }
            (&Method::PUT, Some(session), slot) => {
                self.put(
                    request,
                    route,
                    behaviour,
                    &agent_id,
                    &session,
                    slot.as_deref(),
                )
                .await
            }
            (&Method::HEAD, Some(session), Some(slot))
            | (&Method::GET, Some(session), Some(slot)) => {
                self.read(request, route, &agent_id, session, slot).await
            }
            (&Method::HEAD, Some(session), None) | (&Method::GET, Some(session), None) => {
                self.describe(request, route, &agent_id, &session).await
            }
            _ => Ok(response(StatusCode::METHOD_NOT_ALLOWED)),
        }
    }

    /// Reads one slot of a session. The empty slot name addresses the session
    /// itself and returns its slot list. `None` means the agent does not exist.
    async fn read_slot(
        &self,
        route: &ResolvedRouteEntry,
        agent_id: &AgentId,
        session: &str,
        slot: &str,
        from_offset: Vec<u8>,
        max_items: u32,
        wait_millis: u64,
    ) -> Result<Option<ReadStreamSlotSuccess>, RequestHandlerError> {
        let result = self
            .worker_service
            .read_stream_slot(
                agent_id,
                ReadStreamSlotRequest {
                    agent_id: Some(agent_id.clone().into()),
                    environment_id: Some(route.route.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    session: session.into(),
                    slot: slot.into(),
                    from_offset,
                    max_items,
                    max_bytes: MAX_BYTES,
                    wait_millis,
                    expected_method: route_method(route).to_owned(),
                },
            )
            .await;
        match result {
            Err(WorkerServiceError::AgentNotFound(_))
            | Err(WorkerServiceError::GolemError(WorkerExecutorError::AgentNotFound { .. })) => {
                Ok(None)
            }
            other => other.map_err(Into::into),
        }
    }
}

pub(super) fn error_response(
    error: RequestHandlerError,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    use crate::service::worker::CallWorkerExecutorError;
    let status = match &error {
        RequestHandlerError::AgentInvocationFailed(WorkerServiceError::GolemError(
            WorkerExecutorError::InvalidRequest { details },
        )) if details.starts_with("IdempotencyConflict:") => StatusCode::CONFLICT,
        RequestHandlerError::AgentInvocationFailed(WorkerServiceError::AgentNotFound(_))
        | RequestHandlerError::AgentInvocationFailed(WorkerServiceError::GolemError(
            WorkerExecutorError::AgentNotFound { .. }
            | WorkerExecutorError::ComponentNotFound { .. },
        )) => StatusCode::NOT_FOUND,
        RequestHandlerError::AgentInvocationFailed(WorkerServiceError::InternalCallError(
            CallWorkerExecutorError::FailedToGetRoutingTable(_)
            | CallWorkerExecutorError::FailedToConnectToPod(_),
        ))
        | RequestHandlerError::AgentInvocationFailed(WorkerServiceError::GolemError(
            WorkerExecutorError::InvalidShardId { .. } | WorkerExecutorError::ShardingNotReady,
        )) => StatusCode::SERVICE_UNAVAILABLE,
        RequestHandlerError::AgentInvocationFailed(WorkerServiceError::TypeChecker(_))
        | RequestHandlerError::AgentInvocationFailed(WorkerServiceError::GolemError(
            WorkerExecutorError::InvalidRequest { .. }
            | WorkerExecutorError::ParamTypeMismatch { .. }
            | WorkerExecutorError::ValueMismatch { .. },
        ))
        | RequestHandlerError::BodyIsNotValidJson { .. }
        | RequestHandlerError::JsonBodyParsingFailed { .. }
        | RequestHandlerError::ValueParsingFailed { .. }
        | RequestHandlerError::MissingValue { .. }
        | RequestHandlerError::TooManyValues { .. }
        | RequestHandlerError::HeaderIsNotAscii { .. } => StatusCode::BAD_REQUEST,
        _ => return Err(error),
    };
    let mut result = response(status);
    if status == StatusCode::SERVICE_UNAVAILABLE {
        result
            .headers
            .insert(http::header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    Ok(result)
}

fn route_method(route: &ResolvedRouteEntry) -> &str {
    match &route.route.behavior {
        super::RichRouteBehaviour::CallAgent(behaviour) => &behaviour.method_name,
        _ => unreachable!("Durable Streams handler requires an agent route"),
    }
}

/// Path captures after the route's own variables: `.../{session}/streams/{slot}`.
struct Suffix {
    session: Option<String>,
    slot: Option<String>,
    reserved: bool,
}

fn classify(base_vars: u32, variables: &[String]) -> Suffix {
    let base_vars = base_vars as usize;
    let session = variables.get(base_vars).cloned();
    let slot = variables.get(base_vars + 1).cloned();
    Suffix {
        reserved: slot.as_deref().is_some_and(|s| s.starts_with("__ds")),
        session,
        slot,
    }
}

fn has_header(r: &RichRequest, n: &str) -> bool {
    r.headers().contains_key(n)
}

fn stable_phantom(session: &str) -> Uuid {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(session.as_bytes());
    let mut b = [0; 16];
    b.copy_from_slice(&hash[..16]);
    b[6] = (b[6] & 15) | 4;
    b[8] = (b[8] & 63) | 128;
    Uuid::from_bytes(b)
}

fn response(status: StatusCode) -> RouteExecutionResult {
    RouteExecutionResult {
        status,
        headers: HeaderMap::new(),
        body: ResponseBody::NoBody,
    }
}

fn body_response(status: StatusCode, body: Vec<u8>, ct: &'static str) -> RouteExecutionResult {
    RouteExecutionResult {
        status,
        headers: HeaderMap::new(),
        body: ResponseBody::PoemBody {
            body: poem::Body::from_bytes(body.into()),
            content_type: Some(ct),
        },
    }
}

fn rejection_response(rejection: LoadRejection) -> RouteExecutionResult {
    let mut result = response(rejection.status_code());
    if result.status == StatusCode::SERVICE_UNAVAILABLE {
        result
            .headers
            .insert(http::header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn suffix_uses_compiled_base_capture_count() {
        let mut captured = vec!["tenant".into(), "base-session".into()];
        assert!(classify(2, &captured).session.is_none());
        captured.push("actual-session".into());
        let session = classify(2, &captured);
        assert_eq!(session.session.as_deref(), Some("actual-session"));
        assert!(session.slot.is_none());
        captured.push("$result".into());
        let stream = classify(2, &captured);
        assert_eq!(stream.session.as_deref(), Some("actual-session"));
        assert_eq!(stream.slot.as_deref(), Some("$result"));
        assert!(!stream.reserved);
        captured[3] = "__ds-control".into();
        assert!(classify(2, &captured).reserved);
    }
}
