// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::call_agent::principal_from_request;
use super::error::RequestHandlerError;
use super::http_completion::{BodyGate, GateOutput, GateTerminal};
use super::http_envelope::{
    HttpVersion, RawHeaders, ResponseBodyPolicy, process_request_head, process_response_head,
};
use super::http_session::{HttpSession, HttpSessionEvent, HttpSessionLimits};
use super::mounted_dispatch::{MountBackend, MountFile};
use super::route_resolver::ResolvedRouteEntry;
use super::{ResponseBody, RichRequest, RichRouteBehaviour, RouteExecutionResult};
use crate::service::worker::WorkerService;
use bytes::Bytes;
use futures::StreamExt;
use golem_api_grpc::proto::golem::schema::{SchemaValue as ProtoValue, schema_value};
use golem_api_grpc::proto::golem::worker::{
    InvocationContext, InvocationStart, invocation_session_completion, invocation_session_result,
};
use golem_common::model::agent::{ParsedAgentId, Principal};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::{
    FromSchema, IntoSchema, SchemaGraph, SchemaValue, TypedSchemaValue, try_into_schema_graph,
};
use golem_schema::schema::protobuf::schema_value_to_proto_with_streams;
use golem_schema::schema::validation::validate_value;
use golem_service_base::custom_api::HttpRouterBehaviour;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use http::StatusCode;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use tokio_util::task::AbortOnDropHandle;

const BODY_STREAM_ID: u64 = 1;

#[derive(Clone)]
pub(super) struct RawHandler {
    worker_service: Arc<WorkerService>,
    limits: HttpSessionLimits,
    initial_files: Arc<InitialAgentFilesService>,
}

impl RawHandler {
    pub(super) fn new(
        worker_service: Arc<WorkerService>,
        limits: HttpSessionLimits,
        initial_files: Arc<InitialAgentFilesService>,
    ) -> Self {
        Self {
            worker_service,
            limits,
            initial_files,
        }
    }

    async fn invoke(
        &self,
        request: &mut RichRequest,
        selected: &ResolvedRouteEntry,
        router: &HttpRouterBehaviour,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let version = if request.underlying.version() == http::Version::HTTP_2 {
            HttpVersion::Http2
        } else {
            HttpVersion::Http1
        };
        let method = request.underlying.method().as_str().to_owned();
        let target = request
            .underlying
            .uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("/")
            .to_owned();
        let raw_headers = raw_headers(request.underlying.headers());
        let head = process_request_head(
            &method,
            &target,
            &raw_headers,
            version,
            request.underlying.uri().authority().map(|a| a.as_str()),
        )
        .map_err(|error| RequestHandlerError::RawRequest(error.request_status()))?;

        let has_request_body = head.chunked || head.content_length.is_some_and(|length| length > 0);
        let mut session = self.start_session(request, selected, router, &head)?;
        let mut guard = session.drop_guard();
        let upload_done = Arc::new(AtomicBool::new(false));
        let mut upload = None;
        let response_value = receive_response_head(
            &mut session,
            request.underlying.take_body(),
            &mut upload,
            upload_done.clone(),
        )
        .await?;
        let (status, headers, output_id) = parse_response(response_value)?;
        let response_head = process_response_head(&method, status, &headers)
            .map_err(|_| RequestHandlerError::RawBadGateway)?;

        if response_head.body_policy == ResponseBodyPolicy::Bodyless {
            finish_bodyless_response(&mut session, &mut upload, output_id).await?;
            guard.disarm();
            let mut headers = response_head.headers;
            close_unread_request(&mut headers, has_request_body, &upload_done, version)?;
            // An unknown-size host body lets Hyper's HTTP/1 encoder retain a 304 length.
            // This stream is empty; the guest producer has already been disposed above.
            let body = if version == HttpVersion::Http1
                && response_head.status == StatusCode::NOT_MODIFIED
                && headers.contains_key(http::header::CONTENT_LENGTH)
            {
                ResponseBody::Stream(poem::Body::from_bytes_stream(futures::stream::empty::<
                    Result<Bytes, io::Error>,
                >()))
            } else {
                ResponseBody::NoBody
            };
            return Ok(RouteExecutionResult {
                status: response_head.status,
                headers,
                body,
            });
        }

        let mut headers = response_head.headers.clone();
        close_unread_request(&mut headers, has_request_body, &upload_done, version)?;
        let state = OutputState {
            session,
            upload,
            guard: Some(guard),
            gate: BodyGate::new(&response_head),
            output_id,
            terminal: false,
        };
        let stream = futures::stream::unfold(state, output_step);
        Ok(RouteExecutionResult {
            status: response_head.status,
            headers,
            body: ResponseBody::Stream(poem::Body::from_bytes_stream(stream)),
        })
    }

    fn start_session(
        &self,
        request: &RichRequest,
        selected: &ResolvedRouteEntry,
        router: &HttpRouterBehaviour,
        head: &super::http_envelope::RequestHead,
    ) -> Result<HttpSession, RequestHandlerError> {
        let handler = router
            .handler
            .as_ref()
            .ok_or(RequestHandlerError::RawBadGateway)?;
        let key = IdempotencyKey::fresh();
        let logical = ParsedAgentId::try_new(
            router.agent_type.clone(),
            TypedSchemaValue::new(
                router.constructor_input.graph.clone(),
                SchemaValue::Record { fields: vec![] },
            ),
            None,
        )
        .map_err(|_| RequestHandlerError::RawInternal)?;
        let final_id = logical
            .with_ephemeral_invocation_phantom(&key)
            .map_err(|_| RequestHandlerError::RawInternal)?;
        let agent_id = AgentId {
            component_id: router.component_id,
            agent_id: final_id.to_string(),
        };
        let principal: Principal = principal_from_request(request)?;
        let context = InvocationContext {
            parent: None,
            env: Default::default(),
            tracing: Some(request.invocation_context().into()),
        };
        let input = request_value(head, selected);
        HttpSession::start(
            InvocationStart {
                agent_id: Some(agent_id.into()),
                method_name: Some(handler.method_name.clone()),
                input: Some(input),
                idempotency_key: Some(key.into()),
                context: Some(context),
                auth_ctx: Some(AuthCtx::System.into()),
                principal: Some(principal.into()),
                environment_id: Some(selected.route.environment_id.into()),
                config: vec![],
                component_owner_account_id: Some(selected.route.account_id.into()),
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                freshness_disposition:
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                        as i32,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                ..Default::default()
            },
            self.worker_service.clone(),
            self.limits.clone(),
        )
        .map_err(map_session_error)
    }
}

impl MountBackend for RawHandler {
    async fn file(
        &mut self,
        request: &mut RichRequest,
        selected: &ResolvedRouteEntry,
        file: MountFile<'_>,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
        match file {
            MountFile::Initial(entry) => super::immutable_files::serve(
                &self.initial_files,
                selected.route.environment_id,
                entry,
                request.underlying.method(),
                request.underlying.headers(),
            )
            .await
            .map(Some),
            MountFile::Live {
                agent_id,
                path,
                directory_request,
            } => {
                super::live_files::serve(
                    &self.worker_service,
                    request,
                    selected,
                    agent_id,
                    &path,
                    directory_request,
                )
                .await
            }
        }
    }

    async fn handler(
        &mut self,
        request: &mut RichRequest,
        selected: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let RichRouteBehaviour::HttpRouter(router) = &selected.route.behavior else {
            return Err(RequestHandlerError::RawInternal);
        };
        self.invoke(request, selected, router).await
    }
}

async fn receive_response_head(
    session: &mut HttpSession,
    body: poem::Body,
    upload: &mut Option<AbortOnDropHandle<()>>,
    upload_done: Arc<AtomicBool>,
) -> Result<ProtoValue, RequestHandlerError> {
    let mut body = Some(body);
    loop {
        match session
            .events
            .recv()
            .await
            .ok_or(RequestHandlerError::RawBadGateway)?
        {
            HttpSessionEvent::Accepted(_) => {
                if upload.is_none() {
                    *upload = Some(start_upload(
                        body.take().ok_or(RequestHandlerError::RawBadGateway)?,
                        session.input.clone(),
                        session.cancellation.clone(),
                        upload_done.clone(),
                    ));
                }
            }
            HttpSessionEvent::Result(result) => {
                return match result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => Ok(value),
                    _ => Err(RequestHandlerError::RawBadGateway),
                };
            }
            HttpSessionEvent::StreamCancel(cancel)
                if cancel.role()
                    == golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer =>
            {
                stop_upload(session, upload).await;
            }
            HttpSessionEvent::Error(error) => return Err(map_session_error(error)),
            _ => return Err(RequestHandlerError::RawBadGateway),
        }
    }
}

async fn finish_bodyless_response(
    session: &mut HttpSession,
    upload: &mut Option<AbortOnDropHandle<()>>,
    output_id: u64,
) -> Result<(), RequestHandlerError> {
    session
        .input
        .dispose_output(output_id)
        .await
        .map_err(map_session_error)?;
    loop {
        match session
            .events
            .recv()
            .await
            .ok_or(RequestHandlerError::RawBadGateway)?
        {
            HttpSessionEvent::Finished(finished)
                if matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ) =>
            {
                stop_upload(session, upload).await;
                return Ok(());
            }
            HttpSessionEvent::StreamCancel(cancel)
                if cancel.role()
                    == golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer =>
            {
                stop_upload(session, upload).await;
            }
            HttpSessionEvent::OutputItem(_)
            | HttpSessionEvent::OutputEnd(_)
            | HttpSessionEvent::StreamCancel(_) => {}
            HttpSessionEvent::Error(error) => return Err(map_session_error(error)),
            _ => return Err(RequestHandlerError::RawBadGateway),
        }
    }
}

fn close_unread_request(
    headers: &mut http::HeaderMap,
    has_request_body: bool,
    upload_done: &AtomicBool,
    version: HttpVersion,
) -> Result<(), RequestHandlerError> {
    if has_request_body && !upload_done.load(Ordering::Acquire) && version == HttpVersion::Http1 {
        headers
            .try_insert(
                http::header::CONNECTION,
                http::HeaderValue::from_static("close"),
            )
            .map_err(|_| RequestHandlerError::RawBadGateway)?;
    }
    Ok(())
}

fn raw_headers(headers: &http::HeaderMap) -> RawHeaders {
    headers
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect()
}

#[derive(IntoSchema, FromSchema)]
struct CanonicalHeader {
    name: String,
    value: Vec<u8>,
}

#[derive(IntoSchema)]
struct CanonicalRequest {
    method: String,
    scheme: String,
    authority: String,
    path: String,
    query: Option<String>,
    headers: Vec<CanonicalHeader>,
    body: SchemaValueStream,
}

#[derive(IntoSchema)]
struct CanonicalRequestInput {
    request: CanonicalRequest,
}

#[derive(IntoSchema, FromSchema)]
struct CanonicalResponseHead {
    status: u16,
    headers: Vec<CanonicalHeader>,
}

fn request_value(
    head: &super::http_envelope::RequestHead,
    selected: &ResolvedRouteEntry,
) -> ProtoValue {
    let value = CanonicalRequestInput {
        request: CanonicalRequest {
            method: head.method.clone(),
            scheme: selected.public_scheme.clone(),
            authority: selected.public_authority.clone(),
            path: head.path.clone(),
            query: head.query.clone(),
            headers: head
                .headers
                .iter()
                .map(|(name, value)| CanonicalHeader {
                    name: name.as_str().to_owned(),
                    value: value.as_bytes().to_vec(),
                })
                .collect(),
            body: SchemaValueStream::from_host_endpoint(BODY_STREAM_ID),
        },
    }
    .to_value();
    schema_value_to_proto_with_streams(value, |stream| stream.take_host_endpoint::<u64>())
        .expect("request contains only the known body stream")
}

fn parse_response(value: ProtoValue) -> Result<(u16, RawHeaders, u64), RequestHandlerError> {
    static HEAD_SCHEMA: LazyLock<SchemaGraph> = LazyLock::new(|| {
        try_into_schema_graph::<CanonicalResponseHead>().expect("canonical HTTP response schema")
    });
    let Some(schema_value::Value::RecordValue(response)) = value.value else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    let [status, headers, body] = response.fields.as_slice() else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    let Some(schema_value::Value::StreamReference(body)) = &body.value else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    let value = SchemaValue::Record {
        fields: vec![
            status
                .clone()
                .try_into()
                .map_err(|_| RequestHandlerError::RawBadGateway)?,
            headers
                .clone()
                .try_into()
                .map_err(|_| RequestHandlerError::RawBadGateway)?,
        ],
    };
    validate_value(&HEAD_SCHEMA, &HEAD_SCHEMA.root, &value)
        .map_err(|_| RequestHandlerError::RawBadGateway)?;
    let response = CanonicalResponseHead::from_value(&value)
        .map_err(|_| RequestHandlerError::RawBadGateway)?;
    let headers = response
        .headers
        .into_iter()
        .map(|header| (header.name.into_bytes(), header.value))
        .collect();
    Ok((response.status, headers, body.stream_id))
}

fn decode_bytes(value: &ProtoValue) -> Result<Vec<u8>, RequestHandlerError> {
    let value: SchemaValue = value
        .clone()
        .try_into()
        .map_err(|_| RequestHandlerError::RawBadGateway)?;
    Vec::<u8>::from_value(&value).map_err(|_| RequestHandlerError::RawBadGateway)
}

fn start_upload(
    body: poem::Body,
    input: super::http_session::HttpSessionInput,
    cancellation: super::http_session::HttpSessionCancellation,
    done: Arc<AtomicBool>,
) -> AbortOnDropHandle<()> {
    AbortOnDropHandle::new(tokio::spawn(async move {
        let mut stream = body.into_bytes_stream();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                cancellation.cancel();
                return;
            };
            if input.send_chunk(chunk.to_vec()).await.is_err() {
                return;
            }
        }
        let _ = input.finish().await;
        done.store(true, Ordering::Release);
    }))
}

async fn stop_upload(session: &HttpSession, upload: &mut Option<AbortOnDropHandle<()>>) {
    if let Some(upload) = upload.take() {
        upload.abort();
        let _ = upload.await;
    }
    let _ = session.input.dispose().await;
}

struct OutputState {
    session: HttpSession,
    upload: Option<AbortOnDropHandle<()>>,
    guard: Option<super::http_session::HttpSessionDropGuard>,
    gate: BodyGate,
    output_id: u64,
    terminal: bool,
}

async fn output_step(mut state: OutputState) -> Option<(Result<Bytes, io::Error>, OutputState)> {
    if state.terminal {
        return None;
    }
    loop {
        let Some(event) = state.session.events.recv().await else {
            state.terminal = true;
            return Some((
                Err(io::Error::other(
                    "raw HTTP session ended without completion",
                )),
                state,
            ));
        };
        let output = match event {
            HttpSessionEvent::OutputItem(item) if item.transport_stream_id == state.output_id => {
                match item.value.as_ref().map(decode_bytes) {
                    Some(Ok(bytes)) => state.gate.push(Bytes::from(bytes)),
                    _ => state.gate.producer_failure(),
                }
            }
            HttpSessionEvent::OutputEnd(end) if end.transport_stream_id == state.output_id => {
                state.gate.body_eof()
            }
            HttpSessionEvent::OutputError(_) => state.gate.producer_failure(),
            HttpSessionEvent::Finished(finished)
                if matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ) =>
            {
                stop_upload(&state.session, &mut state.upload).await;
                state.gate.session_success()
            }
            HttpSessionEvent::Finished(_) | HttpSessionEvent::Error(_) => {
                state.gate.session_failure()
            }
            HttpSessionEvent::StreamCancel(cancel)
                if cancel.role
                    == golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer
                        as i32 =>
            {
                stop_upload(&state.session, &mut state.upload).await;
                continue;
            }
            HttpSessionEvent::StreamCancel(cancel)
                if cancel.role
                    == golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputProducer
                        as i32 =>
            {
                state.gate.producer_failure()
            }
            _ => continue,
        };
        if let Some(item) = gate_item(&mut state, output) {
            return Some((item, state));
        }
        if state.terminal {
            return None;
        }
    }
}

fn gate_item(state: &mut OutputState, output: GateOutput) -> Option<Result<Bytes, io::Error>> {
    if matches!(output.terminal, Some(GateTerminal::Complete)) {
        if let Some(mut guard) = state.guard.take() {
            guard.disarm();
        }
        state.terminal = true;
    }
    if let Some(bytes) = output.bytes {
        return Some(Ok(bytes));
    }
    match output.terminal {
        Some(GateTerminal::Complete) => None,
        Some(GateTerminal::Abort(_)) => {
            state.session.cancellation.cancel();
            state.terminal = true;
            Some(Err(io::Error::other("raw HTTP response failed")))
        }
        None => None,
    }
}

fn map_session_error(error: super::http_session::HttpSessionError) -> RequestHandlerError {
    match error {
        super::http_session::HttpSessionError::Deadline => RequestHandlerError::RawDeadline,
        _ => RequestHandlerError::RawBadGateway,
    }
}

#[cfg(test)]
mod tests;
