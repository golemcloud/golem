// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

mod append;
mod encoding;
mod expiry;
mod fork;
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
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_session_public::{
    new_durable_stream_session_id, validate_durable_stream_session_id,
};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_service_base::custom_api::{CallAgentBehaviour, DurableStreamRoutePolicy};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use load::{DurableStreamLoadLimiter, LoadRejection};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const MAX_ITEMS: u32 = 4096;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

fn route_stream_load_key(
    environment_id: EnvironmentId,
    deployment_revision: DeploymentRevision,
    route_id: golem_service_base::custom_api::RouteId,
    agent_id: &AgentId,
    session: &str,
    slot: &str,
) -> String {
    format!("{environment_id}:{deployment_revision}:{route_id}:{agent_id}:{session}:{slot}")
}

pub struct DurableStreamsHandler {
    worker_service: Arc<WorkerService>,
    call_agent: Arc<CallAgentHandler>,
    limiter: DurableStreamLoadLimiter,
    long_poll_timeout: Duration,
    max_append_body_bytes: usize,
    forks: crate::config::DurableStreamsForksConfig,
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
            forks: config.forks.clone(),
        }
    }

    /// Validates the durable-stream route suffix and dispatches session or slot operations.
    pub async fn handle(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let mut suffix = classify_route(
            behaviour.base_path_variables,
            &route.captured_path_parameters,
            &route.route.path,
        );
        if suffix.reserved {
            return Ok(response(StatusCode::NOT_FOUND));
        }
        let policy = behaviour
            .durable_streams
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Durable Streams route has no resolved policy"))?;
        let public_slot = suffix.slot.clone();
        if let Some(public_slot) = public_slot.as_deref() {
            let Some(slot) = policy.slot_by_public_name(public_slot) else {
                return Ok(response(StatusCode::NOT_FOUND));
            };
            suffix.slot = Some(slot.canonical_name.clone());
        }
        let allow = allowed_methods(&suffix, policy);
        let method_allowed = allow
            .iter()
            .any(|method| request.underlying.method().as_str() == *method);
        let inspect_tombstone = request.underlying.method() == Method::POST
            && suffix
                .slot
                .as_deref()
                .and_then(|slot| policy.slot_by_canonical_name(slot))
                .is_some_and(|slot| !slot.writable());
        if !method_allowed && !inspect_tombstone {
            return Ok(method_not_allowed(&allow));
        }
        let expiry_policy = if matches!(request.underlying.method(), &Method::PUT | &Method::POST) {
            match expiry::parse_expiry_policy(request) {
                Ok(policy) => policy,
                Err(()) => return Ok(response(StatusCode::BAD_REQUEST)),
            }
        } else if has_header(request, "stream-ttl") || has_header(request, "stream-expires-at") {
            return Ok(response(StatusCode::BAD_REQUEST));
        } else {
            None
        };
        if suffix.fork.is_none()
            && [
                "stream-forked-from",
                "stream-fork-offset",
                "stream-fork-sub-offset",
            ]
            .iter()
            .any(|name| has_header(request, name))
        {
            return Ok(response(StatusCode::BAD_REQUEST));
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
        if suffix
            .fork
            .as_deref()
            .is_some_and(|fork| validate_durable_stream_session_id(fork).is_err())
        {
            return Ok(response(StatusCode::NOT_FOUND));
        }
        let root_phantom = behaviour.phantom.then(|| {
            let session = suffix.session.as_deref().unwrap_or_default();
            if behaviour.agent_mode == AgentMode::Ephemeral {
                ephemeral_invocation_phantom_id(&IdempotencyKey::new(session.to_owned()))
            } else {
                stable_phantom(session)
            }
        });
        let root_agent_id = CallAgentHandler::build_agent_id(
            route,
            behaviour.component_id,
            &behaviour.agent_type,
            &behaviour.constructor_input,
            &behaviour.constructor_parameters,
            root_phantom,
        )?;
        let agent_id = match suffix.fork.as_deref() {
            Some(fork) => CallAgentHandler::build_agent_id(
                route,
                behaviour.component_id,
                &behaviour.agent_type,
                &behaviour.constructor_input,
                &behaviour.constructor_parameters,
                Some(fork::fork_phantom_id(&root_agent_id, fork)),
            )?,
            None => root_agent_id.clone(),
        };
        match (request.underlying.method(), suffix.session, suffix.slot) {
            (&Method::PUT, Some(session), Some(slot)) if suffix.fork.is_some() => {
                self.fork(
                    request,
                    route,
                    behaviour,
                    &root_agent_id,
                    &agent_id,
                    suffix.fork.as_deref().unwrap(),
                    &session,
                    &slot,
                    public_slot.as_deref().unwrap(),
                    policy.allow_external_writes,
                    expiry_policy,
                )
                .await
            }
            (&Method::POST, Some(session), Some(slot)) => {
                self.append(
                    request,
                    route,
                    behaviour,
                    &agent_id,
                    &session,
                    &slot,
                    suffix.fork.is_none(),
                    &allow.join(", "),
                    expiry_policy,
                )
                .await
            }
            (&Method::POST, _, _) => unreachable!("POST resource policy checked before dispatch"),
            (&Method::DELETE, Some(session), slot) => {
                self.delete(route, &agent_id, session, slot).await
            }
            (&Method::PUT, Some(session), None) if generated_session => {
                self.create_generated(
                    request,
                    route,
                    behaviour,
                    &agent_id,
                    &session,
                    expiry_policy,
                )
                .await
            }
            (&Method::PUT, Some(session), slot) => {
                if suffix.fork.is_some() {
                    unreachable!("fork session PUT rejected by resource policy");
                }
                self.put(
                    request,
                    route,
                    behaviour,
                    &agent_id,
                    &session,
                    slot.as_deref(),
                    expiry_policy,
                )
                .await
            }
            (&Method::HEAD, Some(session), Some(slot))
            | (&Method::GET, Some(session), Some(slot)) => {
                self.read(request, route, behaviour, &agent_id, session, slot)
                    .await
            }
            (&Method::HEAD, Some(session), None) | (&Method::GET, Some(session), None) => {
                self.describe(request, route, behaviour, &agent_id, &session)
                    .await
            }
            _ => unreachable!("method and resource policy checked before dispatch"),
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
        self.read_slot_admitted(
            route,
            agent_id,
            session,
            slot,
            from_offset,
            max_items,
            wait_millis,
            golem_api_grpc::proto::golem::workerexecutor::v1::StreamSlotReadAdmission::Head,
            None,
        )
        .await
    }

    async fn read_slot_admitted(
        &self,
        route: &ResolvedRouteEntry,
        agent_id: &AgentId,
        session: &str,
        slot: &str,
        from_offset: Vec<u8>,
        max_items: u32,
        wait_millis: u64,
        admission: golem_api_grpc::proto::golem::workerexecutor::v1::StreamSlotReadAdmission,
        invocation_key: Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
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
                    admission: admission as i32,
                    invocation_key,
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
        RequestHandlerError::AgentInvocationFailed(WorkerServiceError::GolemError(
            WorkerExecutorError::InvalidRequest { details },
        )) if details.starts_with("NotFound:") => StatusCode::NOT_FOUND,
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

fn allowed_methods(suffix: &Suffix, policy: &DurableStreamRoutePolicy) -> Vec<&'static str> {
    let Some(canonical_slot) = suffix.slot.as_deref() else {
        return if suffix.fork.is_some() {
            vec!["HEAD", "GET"]
        } else if suffix.session.is_some() {
            let mut methods = vec!["PUT", "HEAD", "GET"];
            if policy.allow_invocation_delete {
                methods.push("DELETE");
            }
            methods
        } else {
            vec!["PUT"]
        };
    };

    let mut methods = vec!["PUT", "HEAD", "GET"];
    if policy.allow_stream_delete {
        methods.push("DELETE");
    }
    if policy.allow_external_writes
        && policy
            .slot_by_canonical_name(canonical_slot)
            .is_some_and(|slot| slot.writable())
    {
        methods.push("POST");
    }
    methods
}

fn method_not_allowed(allow: &[&str]) -> RouteExecutionResult {
    let mut result = response(StatusCode::METHOD_NOT_ALLOWED);
    result.headers.insert(
        http::header::ALLOW,
        allow.join(", ").parse().expect("HTTP methods are valid"),
    );
    result
}

/// Path captures after the route's own variables: `.../{session}/streams/{slot}`.
struct Suffix {
    fork: Option<String>,
    session: Option<String>,
    slot: Option<String>,
    reserved: bool,
}

fn classify_route(
    base_vars: u32,
    variables: &[String],
    path: &[golem_service_base::custom_api::PathSegment],
) -> Suffix {
    let base_vars = base_vars as usize;
    use golem_service_base::custom_api::PathSegment::{Literal, Variable};
    let has_slot_suffix = path.ends_with(&[
        Literal {
            value: "streams".into(),
        },
        Variable {
            display_name: "slot".into(),
        },
    ]);
    let suffix = if has_slot_suffix {
        &path[..path.len() - 2]
    } else {
        path
    };
    let is_fork = suffix.ends_with(&[
        Literal {
            value: "forks".into(),
        },
        Variable {
            display_name: "fork".into(),
        },
        Literal {
            value: "invocations".into(),
        },
        Variable {
            display_name: "session".into(),
        },
    ]) && variables.len() == base_vars + 2 + usize::from(has_slot_suffix);
    let fork = is_fork.then(|| variables.get(base_vars).cloned()).flatten();
    let extra = usize::from(is_fork);
    let session = variables.get(base_vars + extra).cloned();
    let slot = variables.get(base_vars + extra + 1).cloned();
    Suffix {
        fork,
        reserved: slot.as_deref().is_some_and(|s| s.starts_with("__ds")),
        session,
        slot,
    }
}

#[cfg(test)]
fn classify(base_vars: u32, variables: &[String]) -> Suffix {
    classify_route(base_vars, variables, &[])
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
    let headers = HeaderMap::from_iter([(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    )]);
    RouteExecutionResult {
        status,
        headers,
        body: ResponseBody::NoBody,
    }
}

fn body_response(status: StatusCode, body: Vec<u8>, ct: &'static str) -> RouteExecutionResult {
    let mut result = response(status);
    result.body = ResponseBody::PoemBody {
        body: poem::Body::from_bytes(body.into()),
        content_type: Some(ct),
    };
    result
}

fn rejection_response(rejection: LoadRejection) -> RouteExecutionResult {
    let mut result = response(rejection.status_code());
    result
        .headers
        .insert(http::header::RETRY_AFTER, HeaderValue::from_static("1"));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::component::ComponentId;
    use golem_service_base::custom_api::{
        DurableStreamRepresentation, DurableStreamSlot, DurableStreamSlotDirection,
    };
    use test_r::test;

    #[test]
    fn load_rejections_include_retry_after() {
        for rejection in [
            LoadRejection::PerStreamReaders,
            LoadRejection::PerNodeReaders,
            LoadRejection::CatchUpRate,
            LoadRejection::AppendRate,
        ] {
            let response = rejection_response(rejection);
            assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(
                response
                    .headers
                    .get(&http::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok()),
                Some("1")
            );
        }
    }

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

    #[test]
    fn base_path_ending_in_forks_variable_is_not_a_fork_suffix() {
        use golem_service_base::custom_api::PathSegment::{Literal, Variable};

        let base = vec![
            Literal {
                value: "forks".into(),
            },
            Variable {
                display_name: "fork".into(),
            },
        ];
        let session = base
            .iter()
            .cloned()
            .chain([
                Literal {
                    value: "invocations".into(),
                },
                Variable {
                    display_name: "session".into(),
                },
            ])
            .collect::<Vec<_>>();
        let slot = session
            .iter()
            .cloned()
            .chain([
                Literal {
                    value: "streams".into(),
                },
                Variable {
                    display_name: "slot".into(),
                },
            ])
            .collect::<Vec<_>>();
        let fork_session = base
            .iter()
            .cloned()
            .chain([
                Literal {
                    value: "forks".into(),
                },
                Variable {
                    display_name: "fork".into(),
                },
                Literal {
                    value: "invocations".into(),
                },
                Variable {
                    display_name: "session".into(),
                },
            ])
            .collect::<Vec<_>>();
        let fork_slot = fork_session
            .iter()
            .cloned()
            .chain([
                Literal {
                    value: "streams".into(),
                },
                Variable {
                    display_name: "slot".into(),
                },
            ])
            .collect::<Vec<_>>();

        let classified = classify_route(1, &["base".into()], &base);
        assert!(classified.fork.is_none());
        assert!(classified.session.is_none());

        let classified = classify_route(1, &["base".into(), "session".into()], &session);
        assert!(classified.fork.is_none());
        assert_eq!(classified.session.as_deref(), Some("session"));

        let classified =
            classify_route(1, &["base".into(), "session".into(), "slot".into()], &slot);
        assert!(classified.fork.is_none());
        assert_eq!(classified.session.as_deref(), Some("session"));
        assert_eq!(classified.slot.as_deref(), Some("slot"));

        let classified = classify_route(
            1,
            &["base".into(), "fork".into(), "session".into()],
            &fork_session,
        );
        assert_eq!(classified.fork.as_deref(), Some("fork"));
        assert_eq!(classified.session.as_deref(), Some("session"));

        let classified = classify_route(
            1,
            &[
                "base".into(),
                "fork".into(),
                "session".into(),
                "slot".into(),
            ],
            &fork_slot,
        );
        assert_eq!(classified.fork.as_deref(), Some("fork"));
        assert_eq!(classified.session.as_deref(), Some("session"));
        assert_eq!(classified.slot.as_deref(), Some("slot"));
    }

    #[test]
    fn redeployed_route_does_not_inherit_an_old_reader_budget() {
        let environment_id = EnvironmentId::new();
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "LoadTest()".into(),
        };
        let old_key = route_stream_load_key(
            environment_id,
            DeploymentRevision::INITIAL,
            7,
            &agent_id,
            "session",
            "$result",
        );
        let new_key = route_stream_load_key(
            environment_id,
            DeploymentRevision::INITIAL.next().unwrap(),
            7,
            &agent_id,
            "session",
            "$result",
        );
        let limiter = DurableStreamLoadLimiter::new(Default::default());
        let old_reader = limiter.try_acquire_reader(&old_key, Some(1)).unwrap();
        let new_reader = limiter.try_acquire_reader(&new_key, Some(1)).unwrap();

        drop((old_reader, new_reader));
    }

    #[test]
    fn allow_is_derived_from_resource_direction_and_route_policy() {
        let mut policy = DurableStreamRoutePolicy {
            slots: vec![
                DurableStreamSlot {
                    canonical_name: "input".into(),
                    public_name: "requests".into(),
                    direction: DurableStreamSlotDirection::Input,
                    content_type: "application/json".into(),
                    representation: DurableStreamRepresentation::Json,
                },
                DurableStreamSlot {
                    canonical_name: "$result".into(),
                    public_name: "responses".into(),
                    direction: DurableStreamSlotDirection::Output,
                    content_type: "application/json".into(),
                    representation: DurableStreamRepresentation::Json,
                },
            ],
            allow_external_writes: false,
            allow_stream_delete: false,
            allow_invocation_delete: false,
            load: None,
        };
        let suffix = |fork: bool, session: bool, slot: Option<&str>| Suffix {
            fork: fork.then(|| "fork".into()),
            session: session.then(|| "session".into()),
            slot: slot.map(Into::into),
            reserved: false,
        };

        assert_eq!(
            allowed_methods(&suffix(false, false, None), &policy),
            ["PUT"]
        );
        assert_eq!(
            allowed_methods(&suffix(false, true, None), &policy),
            ["PUT", "HEAD", "GET"]
        );
        assert_eq!(
            allowed_methods(&suffix(true, true, None), &policy),
            ["HEAD", "GET"]
        );
        assert_eq!(
            allowed_methods(&suffix(false, true, Some("input")), &policy),
            ["PUT", "HEAD", "GET"]
        );

        policy.allow_external_writes = true;
        policy.allow_stream_delete = true;
        policy.allow_invocation_delete = true;
        assert_eq!(
            allowed_methods(&suffix(false, true, None), &policy),
            ["PUT", "HEAD", "GET", "DELETE"]
        );
        assert_eq!(
            allowed_methods(&suffix(false, true, Some("input")), &policy),
            ["PUT", "HEAD", "GET", "DELETE", "POST"]
        );
        assert_eq!(
            allowed_methods(&suffix(true, true, Some("input")), &policy),
            ["PUT", "HEAD", "GET", "DELETE", "POST"]
        );
        assert_eq!(
            allowed_methods(&suffix(false, true, Some("$result")), &policy),
            ["PUT", "HEAD", "GET", "DELETE"]
        );
    }
}
