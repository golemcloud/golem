// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

mod load;

use super::call_agent::{CallAgentHandler, principal_from_request};
use super::error::RequestHandlerError;
use super::route_resolver::ResolvedRouteEntry;
use super::{ResponseBody, RichRequest, RouteExecutionResult};
use crate::config::DurableStreamsConfig;
use crate::service::worker::WorkerService;
use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_api_grpc::proto::golem::worker::InvocationStart;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ReadStreamSlotRequest, ReadStreamSlotSuccess, stream_slot_item,
};
use golem_common::model::IdempotencyKey;
use golem_common::model::invocation_session_public::{
    new_durable_stream_session_id, validate_durable_stream_session_id,
};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::{SchemaType, SchemaValue, schema_value_to_proto_with_streams};
use golem_schema::schema::render::to_json_value;
use golem_service_base::custom_api::CallAgentBehaviour;
use golem_service_base::model::auth::AuthCtx;
use http::{HeaderName, Method, StatusCode};
use load::{DurableStreamLoadLimiter, LoadRejection};
use prost::Message;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const MAX_ITEMS: u32 = 4096;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub struct DurableStreamsHandler {
    worker_service: Arc<WorkerService>,
    call_agent: Arc<CallAgentHandler>,
    limiter: DurableStreamLoadLimiter,
    long_poll_timeout: Duration,
}

impl DurableStreamsHandler {
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
        }
    }

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
        if matches!(*request.underlying.method(), Method::POST | Method::DELETE) {
            let mut result = response(StatusCode::METHOD_NOT_ALLOWED);
            result
                .headers
                .insert(http::header::ALLOW, "PUT, HEAD, GET".into());
            return Ok(result);
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
            if behaviour.agent_mode == golem_common::model::agent::AgentMode::Ephemeral {
                golem_common::model::agent::ephemeral_invocation_phantom_id(&IdempotencyKey::new(
                    session.to_owned(),
                ))
            } else {
                stable_phantom(session)
            }
        });
        let agent_id = self.call_agent.build_agent_id(route, behaviour, phantom)?;
        match (request.underlying.method(), suffix.session, suffix.slot) {
            (&Method::PUT, Some(session), None) if generated_session => {
                let created = self
                    .create(request, route, behaviour, &agent_id, &session)
                    .await?;
                let mut result = response(if created.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                });
                result.headers.insert(
                    http::header::LOCATION,
                    format!(
                        "{}/invocations/{session}",
                        request.underlying.uri().path().trim_end_matches('/')
                    ),
                );
                Ok(result)
            }
            (&Method::PUT, Some(session), slot) => {
                if validate_durable_stream_session_id(&session).is_err() {
                    return Ok(response(StatusCode::NOT_FOUND));
                }
                if let Some(slot) = &slot {
                    use tokio::io::AsyncReadExt;
                    let mut body = request.underlying.take_body().into_async_read();
                    if body
                        .read(&mut [0u8; 1])
                        .await
                        .map_err(anyhow::Error::from)?
                        != 0
                    {
                        return Ok(response(StatusCode::BAD_REQUEST));
                    }
                    let args_from_url = behaviour.method_parameters.iter().all(|param| {
                        matches!(
                            param,
                            golem_service_base::custom_api::MethodParameter::Path { .. }
                                | golem_service_base::custom_api::MethodParameter::Query { .. }
                        )
                    });
                    if let Some(metadata) = self
                        .read_slot(route, &agent_id, &session, slot, Vec::new(), 0, 0)
                        .await?
                    {
                        if content_type_mismatch(request, &metadata.content_type)? {
                            return Ok(response(StatusCode::CONFLICT));
                        }
                        if args_from_url {
                            self.create(request, route, behaviour, &agent_id, &session)
                                .await?;
                        }
                        return metadata_response(&metadata, true);
                    }
                    if self
                        .read_slot(route, &agent_id, &session, "", Vec::new(), 0, 0)
                        .await?
                        .is_some()
                    {
                        return Ok(response(StatusCode::NOT_FOUND));
                    }
                    let Some(content_type) = declared_slot_content_type(behaviour, slot)? else {
                        return Ok(response(StatusCode::NOT_FOUND));
                    };
                    if content_type_mismatch(request, content_type)? {
                        return Ok(response(StatusCode::CONFLICT));
                    }
                    if !args_from_url {
                        return Ok(response(StatusCode::BAD_REQUEST));
                    }
                }
                let created = self
                    .create(request, route, behaviour, &agent_id, &session)
                    .await?;
                let mut result = match &slot {
                    Some(slot) => match self
                        .read_slot(route, &agent_id, &session, slot, Vec::new(), 0, 0)
                        .await?
                    {
                        Some(metadata) => metadata_response(&metadata, true)?,
                        None => return Ok(response(StatusCode::NOT_FOUND)),
                    },
                    None => response(StatusCode::OK),
                };
                result.status = if created.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                };
                Ok(result)
            }
            (&Method::HEAD, Some(session), Some(slot))
            | (&Method::GET, Some(session), Some(slot)) => {
                self.read(request, route, &agent_id, session, slot).await
            }
            (&Method::HEAD, Some(session), None) | (&Method::GET, Some(session), None) => {
                let read = self
                    .read_slot(route, &agent_id, &session, "", Vec::new(), 0, 0)
                    .await?;
                let Some(read) = read else {
                    return Ok(response(StatusCode::NOT_FOUND));
                };
                let mut streams = Vec::new();
                let mut closed = true;
                for slot in &read.slots {
                    let Some(metadata) = self
                        .read_slot(route, &agent_id, &session, slot, Vec::new(), 0, 0)
                        .await?
                    else {
                        return Ok(response(StatusCode::NOT_FOUND));
                    };
                    closed &= metadata.closed;
                    streams.push(serde_json::json!({"name":slot,"contentType":metadata.content_type,"nextOffset":offset_text(&metadata.head_offset)?,"closed":metadata.closed,"cancelled":metadata.cancelled}));
                }
                let body = serde_json::to_vec(
                    &serde_json::json!({"session":session,"streams":streams,"closed":closed}),
                )
                .map_err(anyhow::Error::from)?;
                let mut result = body_response(StatusCode::OK, body, "application/json");
                result
                    .headers
                    .insert(http::header::CACHE_CONTROL, "no-store".into());
                result
                    .headers
                    .insert(http::header::CONTENT_TYPE, "application/json".into());
                result
                    .headers
                    .insert(HeaderName::from_static("stream-closed"), closed.to_string());
                if request.underlying.method() == Method::HEAD {
                    result.body = ResponseBody::NoBody;
                }
                Ok(result)
            }
            _ => Ok(response(StatusCode::METHOD_NOT_ALLOWED)),
        }
    }

    async fn create(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
        agent_id: &golem_common::model::AgentId,
        session: &str,
    ) -> Result<
        golem_api_grpc::proto::golem::workerexecutor::v1::CreateStreamSessionSuccess,
        RequestHandlerError,
    > {
        let body = request.parse_request_body(&route.route.body).await?;
        let args = self
            .call_agent
            .resolve_method_arguments(route, request, behaviour, body)?;
        let mut args = args.into_iter();
        let mut transport_id = 0u64;
        let fields = behaviour
            .method_input
            .input_schema
            .fields()
            .iter()
            .filter(|field| {
                matches!(
                    field.source,
                    golem_common::schema::FieldSource::UserSupplied
                )
            })
            .map(|field| {
                if matches!(
                    behaviour
                        .method_input
                        .graph
                        .resolve_ref(&field.schema)
                        .map_err(|e| anyhow::anyhow!(e.to_string()))?,
                    SchemaType::Stream { .. }
                ) {
                    let value =
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(transport_id));
                    transport_id += 1;
                    Ok(value)
                } else {
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("missing non-stream method argument"))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        if args.next().is_some() {
            return Err(anyhow::anyhow!("too many non-stream method arguments").into());
        }
        let input = schema_value_to_proto_with_streams(SchemaValue::Record { fields }, |stream| {
            stream.take_host_endpoint::<u64>()
        })
        .map_err(|e| anyhow::anyhow!("invalid durable stream input: {e}"))?;
        let principal = principal_from_request(request)?;
        let start = InvocationStart {
            agent_id: Some(agent_id.clone().into()),
            method_name: Some(behaviour.method_name.clone()),
            input: Some(input),
            idempotency_key: Some(IdempotencyKey::new(session.to_owned()).into()),
            context: Some(golem_api_grpc::proto::golem::worker::InvocationContext {
                parent: None,
                env: Default::default(),
                tracing: Some(request.invocation_context().into()),
            }),
            auth_ctx: Some(AuthCtx::System.into()),
            principal: Some(principal.into()),
            environment_id: Some(route.route.environment_id.into()),
            config: vec![],
            component_owner_account_id: Some(route.route.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition: 0,
            attempt_id: Some(Uuid::new_v4().into()),
            expected_callee_fingerprint: None,
            durable_input_mappings: vec![],
            scope_card: None,
        };
        self.worker_service
            .create_stream_session(agent_id, start)
            .await
            .map_err(Into::into)
    }

    async fn read(
        &self,
        request: &RichRequest,
        route: &ResolvedRouteEntry,
        agent_id: &golem_common::model::AgentId,
        session: String,
        slot: String,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if validate_durable_stream_session_id(&session).is_err() || slot.starts_with("__ds") {
            return Ok(response(StatusCode::NOT_FOUND));
        }
        if request.underlying.method() == Method::HEAD {
            let read = self
                .read_slot(route, agent_id, &session, &slot, Vec::new(), 0, 0)
                .await?;
            return Ok(read
                .map(|r| metadata_response(&r, true))
                .transpose()?
                .unwrap_or_else(|| response(StatusCode::NOT_FOUND)));
        }
        if ["offset", "live", "cursor"].iter().any(|key| {
            request
                .query_params()
                .get(*key)
                .is_some_and(|values| values.len() != 1)
        }) {
            return Ok(response(StatusCode::BAD_REQUEST));
        }
        let live = request
            .query_params()
            .get("live")
            .and_then(|v| v.first())
            .map(String::as_str);
        if live.is_some_and(|v| !matches!(v, "long-poll" | "sse")) {
            return Ok(response(StatusCode::BAD_REQUEST));
        }
        let cursor = match request.query_params().get("cursor").and_then(|v| v.first()) {
            Some(value) => match value.parse::<u64>() {
                Ok(value) if value <= u64::MAX - 180 => Some(value),
                _ => return Ok(response(StatusCode::BAD_REQUEST)),
            },
            None => None,
        };
        let offset = request
            .query_params()
            .get("offset")
            .and_then(|v| v.first())
            .map(String::as_str)
            .unwrap_or("-1");
        let mut tail = None;
        let from = match offset {
            "-1" => Vec::new(),
            "now" => {
                let Some(mut head) = self
                    .read_slot(route, agent_id, &session, &slot, Vec::new(), 0, 0)
                    .await?
                else {
                    return Ok(response(StatusCode::NOT_FOUND));
                };
                head.next_offset = head.head_offset.clone();
                head.up_to_date = true;
                let from = head.head_offset.clone();
                tail = Some(head);
                from
            }
            value => match golem_common::model::durable_stream::StreamOffsetV1::from_str(value) {
                Ok(v)
                    if v == golem_common::model::durable_stream::StreamOffsetV1::new(
                        golem_common::model::OplogIndex::NONE,
                        0,
                    ) =>
                {
                    Vec::new()
                }
                Ok(v) => v.as_bytes().to_vec(),
                Err(_) => return Ok(response(StatusCode::BAD_REQUEST)),
            },
        };
        let key = format!("{}:{agent_id}:{session}:{slot}", route.route.environment_id);
        let _permit = if live.is_some() {
            match self.limiter.try_acquire_reader(&key) {
                Ok(permit) => Some(permit),
                Err(rejection) => return Ok(rejection_response(rejection)),
            }
        } else {
            if let Err(rejection) = self.limiter.check_catch_up(&key) {
                return Ok(rejection_response(rejection));
            }
            None
        };
        let wait = live
            .map(|_| self.long_poll_timeout.as_millis() as u64)
            .unwrap_or(0);
        let start_offset = offset_text(&from)?;
        let read = match tail {
            Some(head) if live != Some("long-poll") || head.closed => head,
            _ => match self
                .read_slot(route, agent_id, &session, &slot, from, MAX_ITEMS, wait)
                .await?
            {
                Some(read) => read,
                None => return Ok(response(StatusCode::NOT_FOUND)),
            },
        };
        if live == Some("long-poll") && read.items.is_empty() {
            let mut r = metadata_response(&read, false)?;
            r.status = StatusCode::NO_CONTENT;
            r.headers.insert(
                HeaderName::from_static("stream-cursor"),
                live_cursor(cursor)?.to_string(),
            );
            return Ok(r);
        }
        if live == Some("sse") {
            let mut result = metadata_response(&read, false)?;
            result
                .headers
                .insert(http::header::CONTENT_TYPE, "text/event-stream".into());
            if content_type(&read) == "application/octet-stream" {
                result.headers.insert(
                    HeaderName::from_static("stream-sse-data-encoding"),
                    "base64".into(),
                );
            }
            let service = self.worker_service.clone();
            let read_request = ReadStreamSlotRequest {
                agent_id: Some(agent_id.clone().into()),
                environment_id: Some(route.route.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                session,
                slot,
                from_offset: Vec::new(),
                max_items: MAX_ITEMS,
                max_bytes: MAX_BYTES,
                wait_millis: wait,
                expected_method: route_method(route).to_owned(),
            };
            let agent_id = agent_id.clone();
            let stream = futures::stream::try_unfold(
                (Some(read), read_request, _permit, false, cursor),
                move |(next, mut request, permit, done, mut cursor)| {
                    let service = service.clone();
                    let agent_id = agent_id.clone();
                    async move {
                        if done {
                            return Ok(None);
                        }
                        let batch = match next {
                            Some(batch) => batch,
                            None => match service
                                .read_stream_slot(&agent_id, request.clone())
                                .await
                                .map_err(std::io::Error::other)?
                            {
                                Some(batch) => batch,
                                None => return Ok(None),
                            },
                        };
                        let closed = batch.closed && batch.up_to_date;
                        request.from_offset = batch.next_offset.clone();
                        let data = sse_batch(&batch, &mut cursor).map_err(std::io::Error::other)?;
                        Ok::<_, std::io::Error>(Some((
                            bytes::Bytes::from(data),
                            (None, request, permit, closed, cursor),
                        )))
                    }
                },
            );
            result.body = ResponseBody::PoemBody {
                body: poem::Body::from_bytes_stream(stream),
                content_type: Some("text/event-stream"),
            };
            return Ok(result);
        }
        let mut result = data_response(&read)?;
        let etag = format!(
            "\"{}:{}:{}\"",
            read.stream_identity,
            start_offset,
            offset_text(&read.next_offset)?
        );
        result.headers.insert(http::header::ETAG, etag.clone());
        if live.is_some() {
            result
                .headers
                .insert(http::header::CACHE_CONTROL, "no-store".into());
            result.headers.insert(
                HeaderName::from_static("stream-cursor"),
                live_cursor(cursor)?.to_string(),
            );
        } else if read.closed
            && request
                .headers()
                .get_all(http::header::IF_NONE_MATCH)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(','))
                .any(|v| v.trim() == "*" || v.trim().trim_start_matches("W/") == etag)
        {
            result.status = StatusCode::NOT_MODIFIED;
            result.body = ResponseBody::NoBody;
        }
        Ok(result)
    }

    async fn read_slot(
        &self,
        route: &ResolvedRouteEntry,
        agent_id: &golem_common::model::AgentId,
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
            Err(crate::service::worker::WorkerServiceError::AgentNotFound(_))
            | Err(crate::service::worker::WorkerServiceError::GolemError(
                golem_service_base::error::worker_executor::WorkerExecutorError::AgentNotFound {
                    ..
                },
            )) => Ok(None),
            other => other.map_err(Into::into),
        }
    }
}

fn content_type_mismatch(
    request: &RichRequest,
    expected: &str,
) -> Result<bool, RequestHandlerError> {
    Ok(request
        .header_string_value("content-type")?
        .is_some_and(|actual| {
            !actual
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case(expected)
        }))
}

fn declared_slot_content_type(
    behaviour: &CallAgentBehaviour,
    slot: &str,
) -> Result<Option<&'static str>, RequestHandlerError> {
    use golem_common::schema::{FieldSource, OutputSchema, SchemaGraph};
    fn stream_type(
        graph: &SchemaGraph,
        ty: &SchemaType,
    ) -> Result<Option<&'static str>, RequestHandlerError> {
        let ty = graph
            .resolve_ref(ty)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        match ty {
            SchemaType::Stream {
                inner: Some(inner), ..
            } => Ok(Some(
                if matches!(
                    graph
                        .resolve_ref(inner)
                        .map_err(|e| anyhow::anyhow!(e.to_string()))?,
                    SchemaType::U8 { .. }
                ) {
                    "application/octet-stream"
                } else {
                    "application/json"
                },
            )),
            _ => Ok(None),
        }
    }
    if let Some(field) = behaviour
        .method_input
        .input_schema
        .fields()
        .iter()
        .find(|field| field.name == slot && matches!(field.source, FieldSource::UserSupplied))
        && let Some(content_type) = stream_type(&behaviour.method_input.graph, &field.schema)?
    {
        return Ok(Some(content_type));
    }
    let graph = &behaviour.expected_agent_response.graph;
    let OutputSchema::Single(output) = &behaviour.expected_agent_response.output_schema else {
        return Ok(None);
    };
    if slot == "$result" {
        return Ok(stream_type(graph, output)?.or_else(|| {
            (!golem_common::schema::agent::contains_stream_in_graph(graph, output))
                .then_some("application/json")
        }));
    }
    if let SchemaType::Record { fields, .. } = graph
        .resolve_ref(output)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        && let Some(field) = fields.iter().find(|field| field.name == slot)
    {
        return stream_type(graph, &field.body);
    }
    Ok(None)
}

pub(super) fn error_response(
    error: RequestHandlerError,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    use crate::service::worker::{CallWorkerExecutorError, WorkerServiceError};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
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
        result.headers.insert(http::header::RETRY_AFTER, "1".into());
    }
    Ok(result)
}

fn route_method(route: &ResolvedRouteEntry) -> &str {
    match &route.route.behavior {
        super::RichRouteBehaviour::CallAgent(behaviour) => &behaviour.method_name,
        _ => unreachable!("Durable Streams handler requires an agent route"),
    }
}

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
fn offset_text(v: &[u8]) -> Result<String, RequestHandlerError> {
    use golem_common::model::durable_stream::StreamOffsetV1;
    let offset = if v.is_empty() {
        StreamOffsetV1::new(golem_common::model::OplogIndex::NONE, 0)
    } else {
        let bytes = v
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid internal stream offset length"))?;
        StreamOffsetV1::from_bytes(bytes).map_err(anyhow::Error::msg)?
    };
    Ok(offset.to_string())
}
fn response(status: StatusCode) -> RouteExecutionResult {
    RouteExecutionResult {
        status,
        headers: HashMap::new(),
        body: ResponseBody::NoBody,
    }
}
fn body_response(status: StatusCode, body: Vec<u8>, ct: &'static str) -> RouteExecutionResult {
    RouteExecutionResult {
        status,
        headers: HashMap::new(),
        body: ResponseBody::PoemBody {
            body: poem::Body::from_bytes(body.into()),
            content_type: Some(ct),
        },
    }
}
fn metadata_response(
    r: &ReadStreamSlotSuccess,
    head: bool,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let mut out = response(StatusCode::OK);
    headers(&mut out, r, head)?;
    out.headers
        .insert(http::header::CACHE_CONTROL, "no-store".into());
    out.headers.insert(
        HeaderName::from_static("x-content-type-options"),
        "nosniff".into(),
    );
    out.headers.insert(
        http::header::ETAG,
        format!(
            "\"{}:{}:{}\"",
            r.stream_identity,
            offset_text(&[])?,
            offset_text(&r.head_offset)?
        ),
    );
    if head {
        out.headers.insert(
            HeaderName::from_static("stream-next-offset"),
            offset_text(&r.head_offset)?,
        );
    }
    if !head {
        out.body = ResponseBody::PoemBody {
            body: poem::Body::empty(),
            content_type: Some(content_type(r)),
        };
    }
    Ok(out)
}
fn headers(
    out: &mut RouteExecutionResult,
    r: &ReadStreamSlotSuccess,
    head: bool,
) -> Result<(), RequestHandlerError> {
    out.headers
        .insert(http::header::CONTENT_TYPE, content_type(r).into());
    out.headers.insert(
        HeaderName::from_static("stream-next-offset"),
        offset_text(&r.next_offset)?,
    );
    out.headers.insert(
        HeaderName::from_static("stream-closed"),
        (r.closed && (head || r.up_to_date)).to_string(),
    );
    if r.cancelled && (head || r.up_to_date) {
        out.headers
            .insert(HeaderName::from_static("stream-cancelled"), "true".into());
    }
    if r.up_to_date {
        out.headers
            .insert(HeaderName::from_static("stream-up-to-date"), "true".into());
    }
    Ok(())
}
fn content_type(r: &ReadStreamSlotSuccess) -> &'static str {
    if r.content_type == "application/octet-stream" {
        "application/octet-stream"
    } else {
        "application/json"
    }
}
fn data_response(r: &ReadStreamSlotSuccess) -> Result<RouteExecutionResult, RequestHandlerError> {
    let body = render_items(r)?;
    let mut out = body_response(StatusCode::OK, body, content_type(r));
    headers(&mut out, r, false)?;
    out.headers.insert(
        http::header::CACHE_CONTROL,
        if r.closed && !r.up_to_date {
            "public, max-age=31536000, immutable"
        } else {
            "no-store"
        }
        .into(),
    );
    Ok(out)
}
fn render_items(r: &ReadStreamSlotSuccess) -> Result<Vec<u8>, RequestHandlerError> {
    if content_type(r) == "application/octet-stream" {
        let mut bytes = Vec::new();
        for item in &r.items {
            let Some(stream_slot_item::Content::PackedU8(value)) = &item.content else {
                return Err(
                    anyhow::anyhow!("binary stream item is not a packed byte payload").into(),
                );
            };
            bytes.extend_from_slice(value);
        }
        return Ok(bytes);
    }
    let graph = r
        .element_schema
        .clone()
        .ok_or_else(|| anyhow::anyhow!("stream schema missing"))?
        .try_into()
        .map_err(|e| anyhow::anyhow!("invalid stream schema: {e}"))?;
    let mut values = Vec::new();
    for item in &r.items {
        let Some(stream_slot_item::Content::Value(bytes)) = &item.content else {
            return Err(anyhow::anyhow!("JSON stream item is not a schema value").into());
        };
        let proto = ProtoSchemaValue::decode(bytes.as_slice()).map_err(anyhow::Error::from)?;
        let value: SchemaValue = proto.try_into().map_err(anyhow::Error::msg)?;
        values.push(to_json_value(&graph, &graph.root, &value).map_err(anyhow::Error::msg)?);
    }
    serde_json::to_vec(&values).map_err(|e| anyhow::anyhow!(e).into())
}
fn live_cursor(previous: Option<u64>) -> Result<u64, RequestHandlerError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(anyhow::Error::from)?
        .as_secs();
    interval_cursor(seconds, previous, fastrand::u64(1..=180))
}

fn interval_cursor(
    seconds: u64,
    previous: Option<u64>,
    jitter: u64,
) -> Result<u64, RequestHandlerError> {
    // Protocol epoch: October 9, 2024; each cursor interval is 20 seconds.
    let interval = seconds.saturating_sub(1_728_432_000) / 20;
    match previous {
        Some(previous) if previous >= interval => previous
            .checked_add(jitter)
            .ok_or_else(|| anyhow::anyhow!("live cursor overflow").into()),
        _ => Ok(interval),
    }
}

fn sse_batch(
    r: &ReadStreamSlotSuccess,
    cursor: &mut Option<u64>,
) -> Result<Vec<u8>, RequestHandlerError> {
    use base64::Engine;
    let binary = content_type(r) == "application/octet-stream";
    let mut body = String::new();
    if !r.items.is_empty() {
        let bytes = render_items(r)?;
        let data = if binary {
            base64::engine::general_purpose::STANDARD.encode(bytes)
        } else {
            String::from_utf8(bytes).map_err(anyhow::Error::from)?
        };
        body.push_str("event: data\ndata: ");
        body.push_str(&data);
        body.push_str("\n\n");
    }
    let mut control = serde_json::json!({
        "streamNextOffset": offset_text(&r.next_offset)?,
        "upToDate": r.up_to_date,
    });
    if r.closed && r.up_to_date {
        control["streamClosed"] = true.into();
    } else {
        let next = live_cursor(*cursor)?;
        control["streamCursor"] = next.to_string().into();
        *cursor = Some(next);
    }
    body.push_str(&format!("event: control\ndata: {control}\n\n",));
    Ok(body.into_bytes())
}
fn rejection_response(rejection: LoadRejection) -> RouteExecutionResult {
    let mut result = response(rejection.status_code());
    if result.status == StatusCode::SERVICE_UNAVAILABLE {
        result.headers.insert(http::header::RETRY_AFTER, "1".into());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::workerexecutor::v1::StreamSlotItem;
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

    fn binary_batch() -> ReadStreamSlotSuccess {
        ReadStreamSlotSuccess {
            content_type: "application/octet-stream".into(),
            items: vec![StreamSlotItem {
                offset: Vec::new(),
                content: Some(stream_slot_item::Content::PackedU8(vec![0, 255, 10, 13])),
            }],
            stream_identity: "identity".into(),
            ..Default::default()
        }
    }

    fn control(batch: &ReadStreamSlotSuccess) -> serde_json::Value {
        let text = String::from_utf8(sse_batch(batch, &mut None).unwrap()).unwrap();
        let control = text.split("event: control\ndata: ").nth(1).unwrap();
        serde_json::from_str(control.trim()).unwrap()
    }

    #[test]
    fn binary_sse_encodes_payload_not_control() {
        let batch = binary_batch();
        let text = String::from_utf8(sse_batch(&batch, &mut None).unwrap()).unwrap();
        assert!(text.starts_with("event: data\ndata: AP8KDQ==\n\n"));
        let control = control(&batch);
        assert_eq!(control["upToDate"], false);
        assert!(control["streamCursor"].is_string());
        assert!(control.get("streamClosed").is_none());
    }

    #[test]
    fn cursor_is_decimal() {
        let cursor = control(&binary_batch())["streamCursor"]
            .as_str()
            .expect("open SSE control event must include a cursor")
            .to_owned();

        assert!(
            !cursor.is_empty() && cursor.bytes().all(|byte| byte.is_ascii_digit()),
            "PROTOCOL.md section 10.1 requires a decimal interval number, got {cursor:?}"
        );
    }

    #[test]
    fn live_cursor_intervals_and_echo_progression() {
        assert_eq!(interval_cursor(1_728_432_019, None, 7).unwrap(), 0);
        assert_eq!(interval_cursor(1_728_432_020, None, 7).unwrap(), 1);
        assert_eq!(interval_cursor(1_728_432_200, Some(9), 7).unwrap(), 10);
        assert_eq!(interval_cursor(1_728_432_200, Some(10), 7).unwrap(), 17);
        assert_eq!(interval_cursor(1_728_432_200, Some(80), 7).unwrap(), 87);
        assert!(interval_cursor(1_728_432_200, Some(u64::MAX), 7).is_err());

        let mut cursor = Some(1_000_000_000);
        for _ in 0..3 {
            let previous = cursor.unwrap();
            let bytes = sse_batch(&binary_batch(), &mut cursor).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            let control: serde_json::Value =
                serde_json::from_str(text.split("event: control\ndata: ").nth(1).unwrap().trim())
                    .unwrap();
            let next = control["streamCursor"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert!(next > previous && next <= previous + 180);
            assert_eq!(cursor, Some(next));
        }
    }

    #[test]
    fn rendering_rejects_mixed_payloads_instead_of_dropping_items() {
        let mut batch = binary_batch();
        batch.items.push(StreamSlotItem {
            offset: Vec::new(),
            content: Some(stream_slot_item::Content::Value(Vec::new())),
        });
        assert!(render_items(&batch).is_err());
        assert!(sse_batch(&batch, &mut None).is_err());
        batch.content_type = "application/json".into();
        batch.element_schema =
            Some(golem_common::schema::SchemaGraph::anonymous(SchemaType::string()).into());
        assert!(render_items(&batch).is_err());
        batch.items[0].content = None;
        assert!(render_items(&batch).is_err());
    }

    #[test]
    fn json_sse_preserves_unicode_and_escapes_message_newlines() {
        let values = ["árvíz\nline two", "quotes: \" and \\"];
        let batch = ReadStreamSlotSuccess {
            content_type: "application/json".into(),
            element_schema: Some(
                golem_common::schema::SchemaGraph::anonymous(SchemaType::string()).into(),
            ),
            items: values
                .iter()
                .map(|value| StreamSlotItem {
                    offset: Vec::new(),
                    content: Some(stream_slot_item::Content::Value(
                        schema_value_to_proto_with_streams(
                            SchemaValue::String((*value).into()),
                            |stream| stream.take_host_endpoint::<u64>(),
                        )
                        .unwrap()
                        .encode_to_vec(),
                    )),
                })
                .collect(),
            ..Default::default()
        };
        let expected = serde_json::json!(["árvíz\nline two", "quotes: \" and \\"]);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&render_items(&batch).unwrap()).unwrap(),
            expected
        );
        let text = String::from_utf8(sse_batch(&batch, &mut None).unwrap()).unwrap();
        let data = text
            .strip_prefix("event: data\ndata: ")
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert!(!data.contains('\n'));
        assert!(data.contains("árvíz"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(data).unwrap(),
            expected
        );
    }

    #[test]
    fn closed_historic_page_is_cacheable_but_not_eof() {
        let mut batch = binary_batch();
        batch.closed = true;
        let page = data_response(&batch).unwrap();
        let closed = HeaderName::from_static("stream-closed");
        assert_eq!(page.headers[&closed], "false");
        assert_eq!(
            page.headers[&http::header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            metadata_response(&batch, true).unwrap().headers[&closed],
            "true"
        );
        batch.up_to_date = true;
        let final_page = data_response(&batch).unwrap();
        assert_eq!(final_page.headers[&closed], "true");
        assert_eq!(final_page.headers[&http::header::CACHE_CONTROL], "no-store");
    }

    #[test]
    fn closed_sse_only_signals_eof_after_final_batch() {
        let mut batch = binary_batch();
        batch.closed = true;
        assert!(control(&batch).get("streamClosed").is_none());
        batch.up_to_date = true;
        let final_control = control(&batch);
        assert_eq!(final_control["streamClosed"], true);
        assert!(final_control.get("streamCursor").is_none());
    }

    #[test]
    fn empty_stream_cursor_is_canonical_and_not_a_request_sentinel() {
        let empty = offset_text(&[]).unwrap();
        assert_eq!(empty.len(), 48);
        assert!(golem_common::model::durable_stream::StreamOffsetV1::from_str(&empty).is_ok());
        assert_ne!(empty, "-1");
        assert_ne!(empty, "now");
    }

    #[test]
    fn malformed_internal_offsets_fail_instead_of_resetting_the_cursor() {
        let mut reserved = vec![0; 24];
        reserved[0] = 1;
        reserved[20] = 1;
        for invalid in [vec![0; 23], vec![0; 25], vec![0; 24], reserved] {
            assert!(offset_text(&invalid).is_err());
            let mut batch = binary_batch();
            batch.next_offset = invalid.clone();
            assert!(data_response(&batch).is_err());
            assert!(sse_batch(&batch, &mut None).is_err());
            assert!(metadata_response(&batch, true).is_err());
            batch.next_offset.clear();
            batch.head_offset = invalid;
            assert!(metadata_response(&batch, true).is_err());
        }
    }

    #[test]
    fn head_includes_content_type_and_tail_without_body() {
        let mut batch = binary_batch();
        batch.head_offset = golem_common::model::durable_stream::StreamOffsetV1::new(
            golem_common::model::OplogIndex::from_u64(23),
            7,
        )
        .as_bytes()
        .to_vec();
        let result = metadata_response(&batch, true).unwrap();
        assert!(matches!(result.body, ResponseBody::NoBody));
        assert_eq!(
            result.headers[&http::header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert_eq!(
            result.headers[&HeaderName::from_static("stream-next-offset")],
            offset_text(&batch.head_offset).unwrap()
        );
        assert_eq!(result.headers[&http::header::CACHE_CONTROL], "no-store");
    }
}
