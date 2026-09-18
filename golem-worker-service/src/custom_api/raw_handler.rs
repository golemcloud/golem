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
use golem_api_grpc::proto::golem::schema::{
    ListValue, RecordValue, SchemaValue as ProtoValue, SchemaValueStreamReference, schema_value,
};
use golem_api_grpc::proto::golem::worker::{
    InvocationContext, InvocationStart, invocation_session_completion, invocation_session_result,
};
use golem_common::model::agent::{ParsedAgentId, Principal};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_service_base::custom_api::HttpRouterBehaviour;
use golem_service_base::model::auth::AuthCtx;
use http::StatusCode;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::task::AbortOnDropHandle;

const BODY_STREAM_ID: u64 = 1;

#[derive(Clone)]
pub(super) struct RawHandler {
    worker_service: Arc<WorkerService>,
    limits: HttpSessionLimits,
}

impl RawHandler {
    pub(super) fn new(worker_service: Arc<WorkerService>, limits: HttpSessionLimits) -> Self {
        Self {
            worker_service,
            limits,
        }
    }

    async fn invoke(
        &self,
        request: &mut RichRequest,
        selected: &ResolvedRouteEntry,
        router: &HttpRouterBehaviour,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let handler = router
            .handler
            .as_ref()
            .ok_or_else(|| RequestHandlerError::RawBadGateway)?;
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
        let input = request_value(&head, selected);
        let mut session = HttpSession::start(
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
        .map_err(map_session_error)?;
        let mut guard = session.drop_guard();
        let mut body = Some(request.underlying.take_body());
        let upload_done = Arc::new(AtomicBool::new(false));
        let mut upload = None;

        let response_value = loop {
            match session
                .events
                .recv()
                .await
                .ok_or(RequestHandlerError::RawBadGateway)?
            {
                HttpSessionEvent::Accepted(_) => {
                    if upload.is_none() {
                        upload = Some(start_upload(
                            body.take().ok_or(RequestHandlerError::RawBadGateway)?,
                            session.input.clone(),
                            session.cancellation.clone(),
                            upload_done.clone(),
                        ));
                    }
                }
                HttpSessionEvent::Result(result) => {
                    break match result.result {
                        Some(invocation_session_result::Result::MethodResult(value)) => value,
                        _ => return Err(RequestHandlerError::RawBadGateway),
                    };
                }
                HttpSessionEvent::Error(error) => return Err(map_session_error(error)),
                HttpSessionEvent::Finished(_) => return Err(RequestHandlerError::RawBadGateway),
                HttpSessionEvent::StreamCancel(cancel)
                    if cancel.role() == golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer => {
                        stop_upload(&session, &mut upload).await;
                    }
                _ => return Err(RequestHandlerError::RawBadGateway),
            }
        };
        let (status, headers, output_id) = parse_response(response_value)?;
        let response_head = process_response_head(&method, status, &headers)
            .map_err(|_| RequestHandlerError::RawBadGateway)?;

        if response_head.body_policy == ResponseBodyPolicy::Bodyless {
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
                        stop_upload(&session, &mut upload).await;
                        break;
                    }
                    HttpSessionEvent::StreamCancel(cancel)
                        if cancel.role() == golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer => {
                            stop_upload(&session, &mut upload).await;
                        }
                    HttpSessionEvent::OutputItem(_)
                    | HttpSessionEvent::OutputEnd(_)
                    | HttpSessionEvent::StreamCancel(_) => {}
                    HttpSessionEvent::Error(error) => return Err(map_session_error(error)),
                    _ => return Err(RequestHandlerError::RawBadGateway),
                }
            }
            guard.disarm();
            let mut headers = response_head.headers;
            if has_request_body
                && !upload_done.load(Ordering::Acquire)
                && version == HttpVersion::Http1
            {
                headers
                    .try_insert(
                        http::header::CONNECTION,
                        http::HeaderValue::from_static("close"),
                    )
                    .map_err(|_| RequestHandlerError::RawBadGateway)?;
            }
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

        let early_input = has_request_body && !upload_done.load(Ordering::Acquire);
        let mut headers = response_head.headers.clone();
        if early_input && version == HttpVersion::Http1 {
            headers
                .try_insert(
                    http::header::CONNECTION,
                    http::HeaderValue::from_static("close"),
                )
                .map_err(|_| RequestHandlerError::RawBadGateway)?;
        }
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
}

impl MountBackend for RawHandler {
    async fn file(
        &mut self,
        _: &mut RichRequest,
        _: &ResolvedRouteEntry,
        _: MountFile<'_>,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
        Ok(Some(RouteExecutionResult {
            status: StatusCode::NOT_IMPLEMENTED,
            headers: http::HeaderMap::new(),
            body: ResponseBody::NoBody,
        }))
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

fn raw_headers(headers: &http::HeaderMap) -> RawHeaders {
    headers
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect()
}

fn bytes_value(bytes: &[u8]) -> ProtoValue {
    ProtoValue {
        value: Some(schema_value::Value::ListValue(ListValue {
            elements: bytes
                .iter()
                .map(|byte| ProtoValue {
                    value: Some(schema_value::Value::U8Value(*byte as u32)),
                })
                .collect(),
        })),
    }
}

fn request_value(
    head: &super::http_envelope::RequestHead,
    selected: &ResolvedRouteEntry,
) -> ProtoValue {
    let headers = head
        .headers
        .iter()
        .map(|(name, value)| ProtoValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![
                    ProtoValue {
                        value: Some(schema_value::Value::StringValue(name.as_str().to_owned())),
                    },
                    bytes_value(value.as_bytes()),
                ],
            })),
        })
        .collect();
    let query = ProtoValue {
        value: Some(schema_value::Value::OptionValue(Box::new(
            golem_api_grpc::proto::golem::schema::OptionValue {
                inner: head.query.as_ref().map(|v| {
                    Box::new(ProtoValue {
                        value: Some(schema_value::Value::StringValue(v.clone())),
                    })
                }),
            },
        ))),
    };
    ProtoValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![ProtoValue {
                value: Some(schema_value::Value::RecordValue(RecordValue {
                    fields: vec![
                        ProtoValue {
                            value: Some(schema_value::Value::StringValue(head.method.clone())),
                        },
                        ProtoValue {
                            value: Some(schema_value::Value::StringValue(
                                selected.public_scheme.clone(),
                            )),
                        },
                        ProtoValue {
                            value: Some(schema_value::Value::StringValue(
                                selected.public_authority.clone(),
                            )),
                        },
                        ProtoValue {
                            value: Some(schema_value::Value::StringValue(head.path.clone())),
                        },
                        query,
                        ProtoValue {
                            value: Some(schema_value::Value::ListValue(ListValue {
                                elements: headers,
                            })),
                        },
                        ProtoValue {
                            value: Some(schema_value::Value::StreamReference(
                                SchemaValueStreamReference {
                                    stream_id: BODY_STREAM_ID,
                                },
                            )),
                        },
                    ],
                })),
            }],
        })),
    }
}

fn parse_response(value: ProtoValue) -> Result<(u16, RawHeaders, u64), RequestHandlerError> {
    let Some(schema_value::Value::RecordValue(response)) = value.value else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    let [status, headers, body] = response.fields.as_slice() else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    let Some(schema_value::Value::U16Value(status)) = status.value else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    let Some(schema_value::Value::ListValue(headers)) = &headers.value else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    let mut raw = Vec::with_capacity(headers.elements.len());
    for header in &headers.elements {
        let Some(schema_value::Value::RecordValue(header)) = &header.value else {
            return Err(RequestHandlerError::RawBadGateway);
        };
        let [name, value] = header.fields.as_slice() else {
            return Err(RequestHandlerError::RawBadGateway);
        };
        let Some(schema_value::Value::StringValue(name)) = &name.value else {
            return Err(RequestHandlerError::RawBadGateway);
        };
        raw.push((name.as_bytes().to_vec(), decode_bytes(value)?));
    }
    let Some(schema_value::Value::StreamReference(body)) = &body.value else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    Ok((
        u16::try_from(status).map_err(|_| RequestHandlerError::RawBadGateway)?,
        raw,
        body.stream_id,
    ))
}

fn decode_bytes(value: &ProtoValue) -> Result<Vec<u8>, RequestHandlerError> {
    let Some(schema_value::Value::ListValue(list)) = &value.value else {
        return Err(RequestHandlerError::RawBadGateway);
    };
    list.elements
        .iter()
        .map(|value| match value.value {
            Some(schema_value::Value::U8Value(byte)) => {
                u8::try_from(byte).map_err(|_| RequestHandlerError::RawBadGateway)
            }
            _ => Err(RequestHandlerError::RawBadGateway),
        })
        .collect()
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
mod tests {
    use super::*;
    use test_r::test;

    fn value(value: schema_value::Value) -> ProtoValue {
        ProtoValue { value: Some(value) }
    }

    #[test]
    fn canonical_byte_lists_are_not_packed() {
        let encoded = bytes_value(&[0, 127, 255]);
        let schema_value::Value::ListValue(list) = encoded.value.unwrap() else {
            panic!("not a list")
        };
        assert_eq!(list.elements.len(), 3);
        assert_eq!(
            decode_bytes(&value(schema_value::Value::ListValue(list))).unwrap(),
            [0, 127, 255]
        );
    }

    #[test]
    fn strict_response_head_decoder_accepts_canonical_shape() {
        let header = value(schema_value::Value::RecordValue(RecordValue {
            fields: vec![
                value(schema_value::Value::StringValue("x-test".into())),
                bytes_value(b"yes"),
            ],
        }));
        let response = value(schema_value::Value::RecordValue(RecordValue {
            fields: vec![
                value(schema_value::Value::U16Value(201)),
                value(schema_value::Value::ListValue(ListValue {
                    elements: vec![header],
                })),
                value(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 9 },
                )),
            ],
        }));
        assert_eq!(
            parse_response(response).unwrap(),
            (201, vec![(b"x-test".to_vec(), b"yes".to_vec())], 9)
        );
    }

    #[test]
    fn response_decoder_rejects_non_byte_header_values() {
        let header = value(schema_value::Value::RecordValue(RecordValue {
            fields: vec![
                value(schema_value::Value::StringValue("x".into())),
                value(schema_value::Value::StringValue("not bytes".into())),
            ],
        }));
        let response = value(schema_value::Value::RecordValue(RecordValue {
            fields: vec![
                value(schema_value::Value::U16Value(200)),
                value(schema_value::Value::ListValue(ListValue {
                    elements: vec![header],
                })),
                value(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 1 },
                )),
            ],
        }));
        assert!(parse_response(response).is_err());
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn late_stream_error_never_writes_successful_chunked_eof() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let wait = Arc::new(tokio::sync::Mutex::new(Some(wait)));
        let endpoint = poem::endpoint::make(move |_| {
            let wait = wait.clone();
            async move {
                let wait = wait.lock().await.take().unwrap();
                let chunks = futures::stream::once(async {
                    Ok::<_, io::Error>(Bytes::from_static(b"before-error"))
                })
                .chain(futures::stream::once(async move {
                    wait.await.unwrap();
                    Err(io::Error::other("late failure"))
                }));
                poem::Response::builder().body(poem::Body::from_bytes_stream(chunks))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
        let _server =
            AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(acceptor, endpoint)));
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut wire = Vec::new();
        let mut buffer = [0; 1024];
        while !wire.ends_with(b"before-error\r\n") {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0, "closed before first response chunk");
            wire.extend_from_slice(&buffer[..count]);
        }
        release.send(()).unwrap();
        // Either a reset or an incomplete EOF signals failure; a zero chunk does not.
        let _ = socket.read_to_end(&mut wire).await;
        assert!(
            wire.windows(b"transfer-encoding: chunked".len())
                .any(|part| part == b"transfer-encoding: chunked")
        );
        assert!(
            !wire.ends_with(b"0\r\n\r\n"),
            "late failure emitted successful EOF: {wire:?}"
        );
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn h2_reset_drops_backpressured_response_body() {
        struct DropProbe(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let (dropped, wait_drop) = tokio::sync::oneshot::channel();
        let probe = Arc::new(tokio::sync::Mutex::new(Some(DropProbe(Some(dropped)))));
        let endpoint = poem::endpoint::make(move |_| {
            let probe = probe.clone();
            async move {
                let probe = probe.lock().await.take().unwrap();
                let chunks = futures::stream::unfold(probe, |probe| async move {
                    Some((Ok::<_, io::Error>(Bytes::from(vec![0xab; 65536])), probe))
                });
                poem::Response::builder().body(poem::Body::from_bytes_stream(chunks))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
        let _server =
            AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(acceptor, endpoint)));
        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let (mut client, connection) = h2::client::handshake(socket).await.unwrap();
        let _connection = AbortOnDropHandle::new(tokio::spawn(connection));
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://{address}/"))
            .body(())
            .unwrap();
        let (response, mut request_body) = client.send_request(request, false).unwrap();
        request_body
            .send_data(Bytes::from_static(b"x"), true)
            .unwrap();
        let response = response.await.unwrap();
        request_body.send_reset(h2::Reason::CANCEL);
        drop(response);
        tokio::time::timeout(std::time::Duration::from_secs(3), wait_drop)
            .await
            .unwrap()
            .unwrap();
    }

    async fn raw_status(request: &'static [u8]) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let endpoint = poem::endpoint::make(|_| async {
            poem::Response::builder()
                .status(StatusCode::NO_CONTENT)
                .finish()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
        let _server =
            AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(acceptor, endpoint)));
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket.write_all(request).await.unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        let status = response
            .split(|byte| *byte == b' ')
            .nth(1)
            .expect("HTTP response status");
        std::str::from_utf8(status).unwrap().parse().unwrap()
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn official_transport_accepts_equal_duplicate_content_length() {
        assert_eq!(
            raw_status(
                b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .await,
            StatusCode::NO_CONTENT.as_u16()
        );
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn official_transport_rejects_unsupported_bare_transfer_encoding() {
        assert_eq!(
            raw_status(
                b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip\r\nConnection: close\r\n\r\n"
            )
            .await,
            StatusCode::BAD_REQUEST.as_u16()
        );
    }
}
