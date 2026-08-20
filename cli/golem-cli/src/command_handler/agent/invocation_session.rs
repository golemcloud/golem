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

use crate::agent_id_display::{SourceLanguage, render_schema_value};
use crate::command::worker::{InvocationStdinFormat, InvocationStdoutFormat};
use crate::command_handler::agent::parse_method_argument_schema_value;
use crate::command_handler::log::render_command_output_document;
use crate::context::Context;
use crate::error::{NonSuccessfulExit, PipedExitCode};
use crate::model::agent::invocation_session::{
    AgentInvocationSessionEvent, AgentInvocationSessionEventKind,
};
use crate::model::format::Format;
use anyhow::{Context as _, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::schema::{
    BinaryValue, RecordValue, SchemaValue as ProtoSchemaValue, SchemaValueStreamReference,
    schema_value,
};
use golem_api_grpc::proto::golem::worker::{
    InputStreamEnd, InputStreamItem, InvocationResponse, PublicInvocationRequest,
    PublicInvocationStart, StreamCancel, StreamCancelReason, StreamCancelRole, input_stream_item,
    invocation_response, invocation_session_completion, invocation_session_result,
    public_invocation_request,
};
use golem_common::model::IdempotencyKey;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::schema::agent::{
    AgentMethodSchema, AgentTypeSchema, OutputSchema, ParsedAgentId,
};
use golem_common::schema::{BinaryValuePayload, SchemaGraph, SchemaType, SchemaValue};
use native_tls::TlsConnector;
use prost::Message as _;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{Connector, connect_async_tls_with_config};
use tokio_util::sync::CancellationToken;

const PIPELINE_CAPACITY: usize = 16;
const RAW_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Clone, Debug)]
struct InputBinding {
    stream_id: u64,
    parameter_name: String,
    item_type: SchemaType,
    raw_kind: Option<RawStreamKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RawStreamKind {
    Binary,
    U8,
}

#[derive(Clone, Debug)]
struct OutputStream {
    item_type: SchemaType,
    parent_stream_id: Option<u64>,
    path: String,
    raw_kind: Option<RawStreamKind>,
    next_offset: u64,
    terminal: bool,
}

enum OutputJob {
    Text(String),
    Raw(Vec<u8>),
    Event(Box<AgentInvocationSessionEvent>),
}

struct OutputChannel {
    tx: mpsc::Sender<OutputJob>,
    interrupt: CancellationToken,
    input_failed: CancellationToken,
}

struct InputFailure {
    error: anyhow::Error,
    reason: StreamCancelReason,
}

#[derive(Debug, thiserror::Error)]
#[error("stdin input failed")]
struct InputFailureSignal;

#[derive(Default)]
struct SessionIdentity {
    agent_id: Option<String>,
    component_revision: Option<u64>,
}

pub(super) struct InvocationSessionArgs {
    pub application_name: String,
    pub environment_name: String,
    pub agent_type: AgentTypeSchema,
    pub parsed_agent_id: ParsedAgentId,
    pub method_name: String,
    pub arguments: Vec<String>,
    pub config: Vec<AgentConfigEntryDto>,
    pub idempotency_key: IdempotencyKey,
    pub stdin_format: InvocationStdinFormat,
    pub stdout_format: InvocationStdoutFormat,
}

pub(super) async fn invoke(ctx: Arc<Context>, args: InvocationSessionArgs) -> anyhow::Result<()> {
    let method = args
        .agent_type
        .methods
        .iter()
        .find(|method| method.name == args.method_name)
        .cloned()
        .ok_or_else(|| anyhow!("Method '{}' not found in agent type", args.method_name))?;
    let source_language = SourceLanguage::from(args.agent_type.source_language.clone());
    let (method_parameters, input_binding) = prepare_method_parameters(
        &args.agent_type.schema,
        &method,
        args.arguments,
        &source_language,
        args.stdin_format,
    )?;
    if args.stdin_format == InvocationStdinFormat::Raw && input_binding.is_none() {
        bail!("--stdin-format raw requires stdin bound to stream<binary> or stream<u8> with '-'");
    }
    let input_stream_id = input_binding.as_ref().map(|binding| binding.stream_id);
    validate_stdout_format(
        &args.agent_type.schema,
        &method.output_schema,
        args.stdout_format,
    )?;

    let constructor_parameters = args
        .parsed_agent_id
        .parameters
        .value()
        .clone()
        .try_into()
        .map_err(anyhow::Error::msg)?;
    let idempotency_key_value = args.idempotency_key.value.clone();
    let start = PublicInvocationRequest {
        request: Some(public_invocation_request::Request::Start(
            PublicInvocationStart {
                application_name: args.application_name,
                environment_name: args.environment_name,
                agent_type_name: args.parsed_agent_id.agent_type.to_string(),
                constructor_parameters: Some(constructor_parameters),
                phantom_id: args.parsed_agent_id.phantom_id.map(Into::into),
                config: args.config.into_iter().map(Into::into).collect(),
                method_name: args.method_name,
                method_parameters: Some(method_parameters),
                idempotency_key: Some(args.idempotency_key.into()),
            },
        )),
    };

    let request = websocket_request(&ctx).await?;
    let connector = if ctx.allow_insecure() {
        Some(Connector::NativeTls(
            TlsConnector::builder()
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true)
                .build()?,
        ))
    } else {
        None
    };
    let (socket, _) = connect_async_tls_with_config(request, None, false, connector)
        .await
        .context("failed to connect to the agent invocation session")?;
    let (mut socket_sink, mut socket_stream) = socket.split();

    let (wire_tx, mut wire_rx) = mpsc::channel::<Message>(PIPELINE_CAPACITY);
    let mut wire_writer = tokio::spawn(async move {
        while let Some(message) = wire_rx.recv().await {
            socket_sink.send(message).await?;
        }
        socket_sink.close().await
    });

    let interrupt = CancellationToken::new();
    let signal_interrupt = interrupt.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_interrupt.cancel();
        }
    });

    let (input_tx, mut input_rx) = mpsc::channel::<PublicInvocationRequest>(PIPELINE_CAPACITY);
    let (input_failure_tx, mut input_failure_rx) = oneshot::channel::<InputFailure>();
    let input_cancelled = CancellationToken::new();
    let input_failed = CancellationToken::new();
    let stdin_format = args.stdin_format;
    let stdin_source_language = source_language.clone();
    let stdin_graph = args.agent_type.schema.clone();
    let has_input = input_binding.is_some();
    let mut input_failure_open = has_input;
    if let Some(binding) = input_binding {
        let reader_cancelled = input_cancelled.clone();
        let reader_failed = input_failed.clone();
        std::thread::spawn(move || {
            if let Err(failure) = read_stdin(
                binding,
                stdin_format,
                stdin_source_language,
                stdin_graph,
                &input_tx,
                &reader_cancelled,
            ) && input_failure_tx.send(failure).is_ok()
            {
                reader_failed.cancel();
            }
        });
    }

    let format = ctx.format();
    let structured = format.is_structured() && args.stdout_format == InvocationStdoutFormat::Value;
    let (output_job_tx, output_rx) = mpsc::channel::<OutputJob>(PIPELINE_CAPACITY);
    let output_tx = OutputChannel {
        tx: output_job_tx,
        interrupt: interrupt.clone(),
        input_failed: input_failed.clone(),
    };
    let (output_result_tx, mut output_result_rx) = oneshot::channel();
    let colorize = ctx.should_colorize();
    std::thread::spawn(move || {
        let _ = output_result_tx.send(write_output(output_rx, format, colorize));
    });

    let mut state = InvocationSessionState::default();
    state
        .validate_public_request(&start)
        .map_err(anyhow::Error::msg)?;
    send_request(&wire_tx, start, &interrupt).await?;

    let mut output_streams = HashMap::<u64, OutputStream>::new();
    let mut failed = false;
    let mut accepted = false;
    let mut stdin_open = has_input;
    let mut input_terminal = !has_input;
    let mut pending_input_items = 0_usize;
    let mut acknowledged_input_offset = 0_u64;
    let mut pending_input_request = None;
    let mut fatal_input_failure = None;
    let mut session_identity = SessionIdentity::default();
    let mut wire_complete = false;

    'session: while !state.is_complete() {
        tokio::select! {
            biased;
            _ = interrupt.cancelled() => {
                input_cancelled.cancel();
                input_rx.close();
                cancel_open_streams(
                    &mut state,
                    &wire_tx,
                    if accepted && !input_terminal {
                        input_stream_id.map(|stream_id| (stream_id, acknowledged_input_offset))
                    } else {
                        None
                    },
                    &output_streams,
                    StreamCancelReason::Cancelled,
                    "invocation interrupted by the client",
                );
                drop(wire_tx);
                if !wire_complete && tokio::time::timeout(Duration::from_secs(3), &mut wire_writer).await.is_err() {
                    wire_writer.abort();
                }
                bail!(PipedExitCode(130));
            }
            failure = &mut input_failure_rx, if input_failure_open => {
                input_failure_open = false;
                if let Ok(failure) = failure {
                    fatal_input_failure = Some(failure);
                    break 'session;
                }
            }
            result = &mut wire_writer, if !wire_complete => {
                match result {
                    Ok(Ok(())) => bail!("agent invocation session connection closed before completion"),
                    Ok(Err(error)) if is_connection_closed(&error) => {
                        wire_complete = true;
                        input_cancelled.cancel();
                        input_rx.close();
                        while input_rx.try_recv().is_ok() {}
                        pending_input_request = None;
                        stdin_open = false;
                    }
                    Ok(Err(error)) => return Err(error.into()),
                    Err(error) => return Err(error.into()),
                }
            }
            result = &mut output_result_rx => {
                match result {
                    Ok(Ok(())) => bail!("invocation output closed before completion"),
                    Ok(Err(error)) if error.downcast_ref::<PipedExitCode>().is_some_and(|exit| exit.0 == 0) => {
                        input_cancelled.cancel();
                        input_rx.close();
                        cancel_open_streams(
                            &mut state,
                            &wire_tx,
                            if accepted && !input_terminal {
                                input_stream_id.map(|stream_id| (stream_id, acknowledged_input_offset))
                            } else {
                                None
                            },
                            &output_streams,
                            StreamCancelReason::Cancelled,
                            "invocation output was closed by the consumer",
                        );
                        drop(wire_tx);
                        if !wire_complete && tokio::time::timeout(Duration::from_secs(3), &mut wire_writer).await.is_err() {
                            wire_writer.abort();
                        }
                        return Err(error);
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(_) => bail!("invocation output writer stopped unexpectedly"),
                }
            }
            request = input_rx.recv(), if accepted && stdin_open && pending_input_items < PIPELINE_CAPACITY && pending_input_request.is_none() => {
                match request {
                    Some(request) => pending_input_request = Some(request),
                    None => stdin_open = false,
                }
            }
            permit = wire_tx.clone().reserve_owned(), if pending_input_request.is_some() => {
                let permit = permit.map_err(|_| anyhow!("agent invocation session connection closed"))?;
                let request = pending_input_request
                    .take()
                    .expect("wire send selected without a pending input request");
                state.validate_public_request(&request).map_err(anyhow::Error::msg)?;
                if matches!(request.request, Some(public_invocation_request::Request::InputItem(_))) {
                    pending_input_items += 1;
                }
                if matches!(request.request, Some(public_invocation_request::Request::InputEnd(_))) {
                    input_terminal = true;
                }
                permit.send(encode_request(request)?);
            }
            frame = socket_stream.next() => {
                let response = match receive_response(
                    frame,
                    &wire_tx,
                    &interrupt,
                    &input_failed,
                ).await {
                    Ok(Some(response)) => response,
                    Ok(None) => continue,
                    Err(error) if error.downcast_ref::<InputFailureSignal>().is_some() => {
                        input_failure_open = false;
                        fatal_input_failure = Some(take_input_failure(&mut input_failure_rx)?);
                        break 'session;
                    }
                    Err(error) => return Err(error),
                };
                state.validate_response(&response).map_err(anyhow::Error::msg)?;
                if matches!(response.response, Some(invocation_response::Response::Accepted(_))) {
                    accepted = true;
                }
                if let Some(invocation_response::Response::InputAck(ack)) = response.response.as_ref() {
                    pending_input_items = pending_input_items.checked_sub(1).ok_or_else(|| anyhow!("received an input acknowledgement without a pending item"))?;
                    acknowledged_input_offset = ack.sequence.checked_add(ack.logical_item_count).ok_or_else(|| anyhow!("input acknowledgement offset overflow"))?;
                }
                if response_cancels_input(&response, input_stream_id) {
                    input_cancelled.cancel();
                    input_rx.close();
                    while input_rx.try_recv().is_ok() {}
                    pending_input_request = None;
                    input_failure_open = false;
                    input_failure_rx.close();
                    stdin_open = false;
                    input_terminal = true;
                }
                match handle_response(
                    response,
                    &args.agent_type.schema,
                    &method.output_schema,
                    &source_language,
                    args.stdout_format,
                    structured,
                    &idempotency_key_value,
                    &mut session_identity,
                    &mut output_streams,
                    &output_tx,
                ).await {
                    Ok(response_failed) => failed |= response_failed,
                    Err(error) if error.downcast_ref::<InputFailureSignal>().is_some() => {
                        input_failure_open = false;
                        fatal_input_failure = Some(take_input_failure(&mut input_failure_rx)?);
                        break 'session;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    if let Some(failure) = fatal_input_failure {
        input_cancelled.cancel();
        input_rx.close();
        while input_rx.try_recv().is_ok() {}
        cancel_open_streams(
            &mut state,
            &wire_tx,
            if accepted && !input_terminal {
                input_stream_id.map(|stream_id| (stream_id, acknowledged_input_offset))
            } else {
                None
            },
            &output_streams,
            failure.reason,
            &failure.error.to_string(),
        );
        drop(wire_tx);
        if !wire_complete
            && tokio::time::timeout(Duration::from_secs(3), &mut wire_writer)
                .await
                .is_err()
        {
            wire_writer.abort();
        }
        return Err(failure.error);
    }

    if let Err(error) = await_clean_close(
        &mut socket_stream,
        &wire_tx,
        &interrupt,
        &mut output_result_rx,
    )
    .await
    {
        input_cancelled.cancel();
        input_rx.close();
        cancel_open_streams(
            &mut state,
            &wire_tx,
            if accepted && !input_terminal {
                input_stream_id.map(|stream_id| (stream_id, acknowledged_input_offset))
            } else {
                None
            },
            &output_streams,
            StreamCancelReason::Cancelled,
            "invocation session stopped while waiting for the server to close",
        );
        drop(wire_tx);
        if !wire_complete
            && tokio::time::timeout(Duration::from_secs(3), &mut wire_writer)
                .await
                .is_err()
        {
            wire_writer.abort();
        }
        return Err(error);
    }

    input_cancelled.cancel();
    drop(wire_tx);
    drop(output_tx);
    input_rx.close();

    let mut output_complete = false;
    while !wire_complete || !output_complete {
        tokio::select! {
            biased;
            _ = interrupt.cancelled() => {
                wire_writer.abort();
                bail!(PipedExitCode(130));
            }
            result = &mut output_result_rx, if !output_complete => {
                match result {
                    Ok(result) => result?,
                    Err(_) => bail!("invocation output writer stopped unexpectedly"),
                }
                output_complete = true;
            }
            result = &mut wire_writer, if !wire_complete => {
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) if state.is_complete() && is_connection_closed(&error) => {}
                    Ok(Err(error)) => return Err(error.into()),
                    Err(error) => return Err(error.into()),
                }
                wire_complete = true;
            }
        }
    }

    if input_failure_open && let Ok(failure) = input_failure_rx.try_recv() {
        return Err(failure.error);
    }
    if failed {
        bail!(NonSuccessfulExit);
    }
    Ok(())
}

async fn websocket_request(ctx: &Context) -> anyhow::Result<tungstenite::http::Request<()>> {
    let mut url = ctx.worker_service_url().clone();
    let websocket_scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        scheme => bail!("unsupported service URL scheme '{scheme}'"),
    };
    url.set_scheme(websocket_scheme)
        .map_err(|_| anyhow!("failed to derive WebSocket service URL"))?;
    url.set_path("/v1/agents/invoke-agent-session");
    url.set_query(None);
    url.set_fragment(None);
    let mut request = url.as_str().into_client_request()?;
    let token = ctx.auth_token().await?;
    request.headers_mut().insert(
        tungstenite::http::header::AUTHORIZATION,
        format!("Bearer {}", token.secret())
            .parse()
            .context("invalid authorization token")?,
    );
    Ok(request)
}

fn prepare_method_parameters(
    graph: &SchemaGraph,
    method: &AgentMethodSchema,
    arguments: Vec<String>,
    source_language: &SourceLanguage,
    stdin_format: InvocationStdinFormat,
) -> anyhow::Result<(ProtoSchemaValue, Option<InputBinding>)> {
    let fields = method.input_schema.fields();
    if fields.len() != arguments.len() {
        bail!(
            "wrong number of parameters: expected {}, got {}",
            fields.len(),
            arguments.len()
        );
    }

    let mut values = Vec::with_capacity(fields.len());
    let mut input_binding = None;
    let mut next_stream_id = 1_u64;
    for (field, argument) in fields.iter().zip(arguments) {
        let resolved = graph
            .resolve_ref(&field.schema)
            .map_err(|error| anyhow!(error.to_string()))?;
        if argument == "-" {
            let SchemaType::Stream { inner, .. } = resolved else {
                bail!(
                    "stdin marker '-' can only be used for a direct stream parameter; '{}' is not a stream",
                    field.name
                );
            };
            if input_binding.is_some() {
                bail!("stdin can only be bound to one stream parameter");
            }
            let item_type = inner
                .as_deref()
                .cloned()
                .ok_or_else(|| anyhow!("untyped streams cannot be bound to stdin"))?;
            if golem_common::schema::agent::contains_stream_in_graph(graph, &item_type) {
                bail!(
                    "stdin stream parameter '{}' cannot contain nested streams",
                    field.name
                );
            }
            let raw_kind = match stdin_format {
                InvocationStdinFormat::Value => None,
                InvocationStdinFormat::Raw => {
                    Some(raw_stream_kind(graph, &item_type).ok_or_else(|| {
                        anyhow!("--stdin-format raw requires stream<binary> or stream<u8>")
                    })?)
                }
            };
            let stream_id = next_stream_id;
            next_stream_id += 2;
            values.push(ProtoSchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id },
                )),
            });
            input_binding = Some(InputBinding {
                stream_id,
                parameter_name: field.name.clone(),
                item_type,
                raw_kind,
            });
        } else {
            if golem_common::schema::agent::contains_stream_in_graph(graph, &field.schema) {
                bail!(
                    "stream parameter '{}' must be a direct stream bound to stdin with '-'",
                    field.name
                );
            }
            let parsed = parse_method_argument_schema_value(
                &argument,
                graph,
                &field.schema,
                source_language,
            )
            .map_err(|error| anyhow!("invalid value for '{}': {}", field.name, error.message))?;
            values.push(parsed.try_into().map_err(anyhow::Error::msg)?);
        }
    }

    Ok((
        ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: values,
            })),
        },
        input_binding,
    ))
}

fn validate_stdout_format(
    graph: &SchemaGraph,
    output: &OutputSchema,
    format: InvocationStdoutFormat,
) -> anyhow::Result<()> {
    if format == InvocationStdoutFormat::Value {
        return Ok(());
    }
    let ty = output
        .schema()
        .ok_or_else(|| anyhow!("--stdout-format raw requires a stream result"))?;
    let resolved = graph
        .resolve_ref(ty)
        .map_err(|error| anyhow!(error.to_string()))?;
    let SchemaType::Stream {
        inner: Some(inner), ..
    } = resolved
    else {
        bail!("--stdout-format raw requires one direct stream result");
    };
    if raw_stream_kind(graph, inner).is_none() {
        bail!("--stdout-format raw requires stream<binary> or stream<u8>");
    }
    Ok(())
}

fn raw_stream_kind(graph: &SchemaGraph, ty: &SchemaType) -> Option<RawStreamKind> {
    match graph.resolve_ref(ty).ok()? {
        SchemaType::Binary { .. } => Some(RawStreamKind::Binary),
        SchemaType::U8 { .. } => Some(RawStreamKind::U8),
        _ => None,
    }
}

fn read_stdin(
    binding: InputBinding,
    format: InvocationStdinFormat,
    source_language: SourceLanguage,
    graph: SchemaGraph,
    tx: &mpsc::Sender<PublicInvocationRequest>,
    cancelled: &CancellationToken,
) -> Result<(), InputFailure> {
    let mut offset = 0_u64;
    match format {
        InvocationStdinFormat::Value => {
            let mut stdin = BufReader::new(std::io::stdin().lock());
            let mut line = String::new();
            loop {
                if cancelled.is_cancelled() {
                    return Ok(());
                }
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                    }
                    Err(error) => {
                        return Err(InputFailure {
                            error: anyhow!(
                                "failed to read stdin for parameter '{}' at line {}: {error}",
                                binding.parameter_name,
                                offset + 1
                            ),
                            reason: StreamCancelReason::Transport,
                        });
                    }
                }
                if cancelled.is_cancelled() {
                    return Ok(());
                }
                let value = match parse_method_argument_schema_value(
                    &line,
                    &graph,
                    &binding.item_type,
                    &source_language,
                ) {
                    Ok(value) => value,
                    Err(parse_error) => {
                        return Err(InputFailure {
                            error: anyhow!(
                                "invalid stdin value for parameter '{}' at line {}: {parse_error}",
                                binding.parameter_name,
                                offset + 1
                            ),
                            reason: StreamCancelReason::Protocol,
                        });
                    }
                };
                let request =
                    input_value_request(binding.stream_id, offset, value).map_err(|error| {
                        InputFailure {
                            error,
                            reason: StreamCancelReason::Protocol,
                        }
                    })?;
                if tx.blocking_send(request).is_err() {
                    if cancelled.is_cancelled() {
                        return Ok(());
                    }
                    return Err(InputFailure {
                        error: anyhow!("invocation session ended while reading stdin"),
                        reason: StreamCancelReason::Transport,
                    });
                }
                offset = offset.checked_add(1).ok_or_else(|| InputFailure {
                    error: anyhow!("stdin stream offset overflow"),
                    reason: StreamCancelReason::Protocol,
                })?;
            }
        }
        InvocationStdinFormat::Raw => {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = vec![0_u8; RAW_CHUNK_SIZE];
            loop {
                if cancelled.is_cancelled() {
                    return Ok(());
                }
                let mut count = 0;
                let mut eof = false;
                while count < RAW_CHUNK_SIZE {
                    if cancelled.is_cancelled() {
                        return Ok(());
                    }
                    match stdin.read(&mut buffer[count..]) {
                        Ok(0) => {
                            eof = true;
                            break;
                        }
                        Ok(read) => {
                            count += read;
                            if cancelled.is_cancelled() {
                                return Ok(());
                            }
                        }
                        Err(error) => {
                            return Err(InputFailure {
                                error: anyhow!(
                                    "failed to read raw stdin for parameter '{}': {error}",
                                    binding.parameter_name
                                ),
                                reason: StreamCancelReason::Transport,
                            });
                        }
                    }
                }
                if count == 0 {
                    break;
                }
                let payload = match binding.raw_kind {
                    Some(RawStreamKind::Binary) => input_stream_item::Payload::Value(
                        SchemaValue::Binary(BinaryValuePayload {
                            bytes: buffer[..count].to_vec(),
                            mime_type: None,
                        })
                        .try_into()
                        .map_err(|error| InputFailure {
                            error: anyhow::Error::msg(error),
                            reason: StreamCancelReason::Protocol,
                        })?,
                    ),
                    Some(RawStreamKind::U8) => {
                        input_stream_item::Payload::PackedU8(buffer[..count].to_vec())
                    }
                    None => unreachable!("raw stdin was validated before reading"),
                };
                if tx
                    .blocking_send(PublicInvocationRequest {
                        request: Some(public_invocation_request::Request::InputItem(
                            InputStreamItem {
                                stream_id: binding.stream_id,
                                sequence: offset,
                                payload: Some(payload),
                            },
                        )),
                    })
                    .is_err()
                {
                    if cancelled.is_cancelled() {
                        return Ok(());
                    }
                    return Err(InputFailure {
                        error: anyhow!("invocation session ended while reading stdin"),
                        reason: StreamCancelReason::Transport,
                    });
                }
                let logical_item_count = if binding.raw_kind == Some(RawStreamKind::U8) {
                    count as u64
                } else {
                    1
                };
                offset = offset
                    .checked_add(logical_item_count)
                    .ok_or_else(|| InputFailure {
                        error: anyhow!("stdin stream offset overflow"),
                        reason: StreamCancelReason::Protocol,
                    })?;
                if eof {
                    break;
                }
            }
        }
    }
    if cancelled.is_cancelled() {
        return Ok(());
    }
    if tx
        .blocking_send(PublicInvocationRequest {
            request: Some(public_invocation_request::Request::InputEnd(
                InputStreamEnd {
                    stream_id: binding.stream_id,
                    offset,
                },
            )),
        })
        .is_err()
    {
        if cancelled.is_cancelled() {
            return Ok(());
        }
        return Err(InputFailure {
            error: anyhow!("invocation session ended before stdin reached EOF"),
            reason: StreamCancelReason::Transport,
        });
    }
    Ok(())
}

fn input_cancel_request(
    stream_id: u64,
    offset: u64,
    reason: StreamCancelReason,
    details: String,
) -> PublicInvocationRequest {
    PublicInvocationRequest {
        request: Some(public_invocation_request::Request::StreamCancel(
            StreamCancel {
                stream_id,
                offset,
                role: StreamCancelRole::InputProducer as i32,
                reason: reason as i32,
                details: Some(details),
            },
        )),
    }
}

fn output_cancel_request(
    stream_id: u64,
    offset: u64,
    reason: StreamCancelReason,
    details: String,
) -> PublicInvocationRequest {
    PublicInvocationRequest {
        request: Some(public_invocation_request::Request::StreamCancel(
            StreamCancel {
                stream_id,
                offset,
                role: StreamCancelRole::OutputConsumer as i32,
                reason: reason as i32,
                details: Some(details),
            },
        )),
    }
}

fn cancel_open_streams(
    state: &mut InvocationSessionState,
    wire_tx: &mpsc::Sender<Message>,
    input: Option<(u64, u64)>,
    output_streams: &HashMap<u64, OutputStream>,
    reason: StreamCancelReason,
    details: &str,
) {
    if let Some((stream_id, offset)) = input {
        let request = input_cancel_request(stream_id, offset, reason, details.to_string());
        if state.validate_public_request(&request).is_ok() {
            try_send_request(wire_tx, request);
        }
    }

    let mut open_outputs = output_streams
        .iter()
        .filter_map(|(stream_id, stream)| {
            (!stream.terminal).then_some((*stream_id, stream.next_offset))
        })
        .collect::<Vec<_>>();
    open_outputs.sort_unstable_by_key(|(stream_id, _)| *stream_id);
    for (stream_id, offset) in open_outputs {
        let request = output_cancel_request(stream_id, offset, reason, details.to_string());
        if state.validate_public_request(&request).is_ok() {
            try_send_request(wire_tx, request);
        }
    }
}

fn input_value_request(
    stream_id: u64,
    sequence: u64,
    value: SchemaValue,
) -> anyhow::Result<PublicInvocationRequest> {
    Ok(PublicInvocationRequest {
        request: Some(public_invocation_request::Request::InputItem(
            InputStreamItem {
                stream_id,
                sequence,
                payload: Some(input_stream_item::Payload::Value(
                    value.try_into().map_err(anyhow::Error::msg)?,
                )),
            },
        )),
    })
}

async fn send_request(
    tx: &mpsc::Sender<Message>,
    request: PublicInvocationRequest,
    interrupt: &CancellationToken,
) -> anyhow::Result<()> {
    send_message(tx, encode_request(request)?, interrupt).await
}

fn try_send_request(tx: &mpsc::Sender<Message>, request: PublicInvocationRequest) {
    if let Ok(message) = encode_request(request) {
        let _ = tx.try_send(message);
    }
}

fn take_input_failure(
    receiver: &mut oneshot::Receiver<InputFailure>,
) -> anyhow::Result<InputFailure> {
    receiver.try_recv().map_err(|error| match error {
        oneshot::error::TryRecvError::Empty => {
            anyhow!("stdin failure signal arrived before its diagnostic")
        }
        oneshot::error::TryRecvError::Closed => {
            anyhow!("stdin failure signal arrived without a diagnostic")
        }
    })
}

fn encode_request(request: PublicInvocationRequest) -> anyhow::Result<Message> {
    let mut bytes = Vec::new();
    request.encode(&mut bytes)?;
    Ok(Message::Binary(bytes.into()))
}

async fn send_message(
    tx: &mpsc::Sender<Message>,
    message: Message,
    interrupt: &CancellationToken,
) -> anyhow::Result<()> {
    tokio::select! {
        biased;
        _ = interrupt.cancelled() => bail!(PipedExitCode(130)),
        result = tx.send(message) => result.map_err(|_| anyhow!("agent invocation session connection closed")),
    }
}

async fn send_active_message(
    tx: &mpsc::Sender<Message>,
    message: Message,
    interrupt: &CancellationToken,
    input_failed: &CancellationToken,
) -> anyhow::Result<()> {
    tokio::select! {
        biased;
        _ = input_failed.cancelled() => bail!(InputFailureSignal),
        _ = interrupt.cancelled() => bail!(PipedExitCode(130)),
        result = tx.send(message) => result.map_err(|_| anyhow!("agent invocation session connection closed")),
    }
}

async fn receive_response(
    frame: Option<Result<Message, tungstenite::Error>>,
    wire_tx: &mpsc::Sender<Message>,
    interrupt: &CancellationToken,
    input_failed: &CancellationToken,
) -> anyhow::Result<Option<InvocationResponse>> {
    match frame {
        Some(Ok(Message::Binary(bytes))) => Ok(Some(InvocationResponse::decode(bytes.as_slice())?)),
        Some(Ok(Message::Ping(payload))) => {
            send_active_message(wire_tx, Message::Pong(payload), interrupt, input_failed).await?;
            Ok(None)
        }
        Some(Ok(Message::Pong(_))) => Ok(None),
        Some(Ok(Message::Close(close))) => {
            bail!("agent invocation session closed before completion: {close:?}");
        }
        Some(Ok(message)) => {
            bail!("unexpected WebSocket frame in invocation session: {message:?}");
        }
        Some(Err(error)) => Err(error.into()),
        None => bail!("agent invocation session ended before completion"),
    }
}

fn response_cancels_input(response: &InvocationResponse, input_stream_id: Option<u64>) -> bool {
    let Some(input_stream_id) = input_stream_id else {
        return false;
    };
    matches!(
        response.response.as_ref(),
        Some(invocation_response::Response::StreamCancel(cancel))
            if cancel.stream_id == input_stream_id
                && cancel.role() == StreamCancelRole::InputConsumer
    )
}

async fn await_clean_close<S>(
    socket_stream: &mut S,
    wire_tx: &mpsc::Sender<Message>,
    interrupt: &CancellationToken,
    output_result_rx: &mut oneshot::Receiver<anyhow::Result<()>>,
) -> anyhow::Result<()>
where
    S: futures_util::Stream<Item = Result<Message, tungstenite::Error>> + Unpin,
{
    let close_timeout = tokio::time::sleep(Duration::from_secs(3));
    tokio::pin!(close_timeout);
    loop {
        tokio::select! {
            biased;
            _ = interrupt.cancelled() => bail!(PipedExitCode(130)),
            result = &mut *output_result_rx => {
                match result {
                    Ok(result) => return result,
                    Err(_) => bail!("invocation output writer stopped unexpectedly"),
                }
            }
            _ = &mut close_timeout => {
                bail!("invocation session did not close after completion");
            }
            frame = socket_stream.next() => {
                match frame {
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(Message::Ping(payload))) => {
                        send_message(wire_tx, Message::Pong(payload), interrupt).await?;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => {
                        bail!("invocation session sent an event after completion");
                    }
                    Some(Ok(message)) => {
                        bail!("unexpected WebSocket frame after invocation completion: {message:?}");
                    }
                    Some(Err(error)) if is_connection_closed(&error) => return Ok(()),
                    Some(Err(error)) => return Err(error.into()),
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_response(
    response: InvocationResponse,
    graph: &SchemaGraph,
    output_schema: &OutputSchema,
    source_language: &SourceLanguage,
    stdout_format: InvocationStdoutFormat,
    structured: bool,
    idempotency_key: &str,
    session_identity: &mut SessionIdentity,
    output_streams: &mut HashMap<u64, OutputStream>,
    output_tx: &OutputChannel,
) -> anyhow::Result<bool> {
    let mut failed = false;
    match response.response {
        Some(invocation_response::Response::Accepted(accepted)) => {
            session_identity.agent_id = accepted.agent_id.as_ref().map(|id| id.name.clone());
            session_identity.component_revision = accepted.component_revision;
            if structured {
                let mut event = event(AgentInvocationSessionEventKind::Accepted, idempotency_key);
                event.agent_id = session_identity.agent_id.clone();
                event.component_revision = session_identity.component_revision;
                emit(output_tx, OutputJob::Event(event)).await?;
            }
        }
        Some(invocation_response::Response::Rejected(rejected)) => {
            failed = true;
            let reason = format!("{:?}", rejected.reason());
            if structured {
                let mut event = event(AgentInvocationSessionEventKind::Rejected, idempotency_key);
                event.agent_id = rejected.agent_id.map(|id| id.name);
                event.component_revision = rejected.component_revision;
                event.reason = Some(reason);
                event.error = Some(rejected.error);
                emit(output_tx, OutputJob::Event(event)).await?;
            } else {
                eprintln!("Invocation rejected ({reason}): {}", rejected.error);
            }
        }
        Some(invocation_response::Response::Result(result)) => {
            let result_agent_id = result
                .agent_id
                .map(|id| id.name)
                .or_else(|| session_identity.agent_id.clone());
            let result_component_revision = result
                .component_revision
                .or(session_identity.component_revision);
            match result.result {
                Some(invocation_session_result::Result::MethodResult(value)) => {
                    let Some(output_type) = output_schema.schema() else {
                        if structured {
                            let mut event =
                                event(AgentInvocationSessionEventKind::Result, idempotency_key);
                            event.agent_id = result_agent_id;
                            event.component_revision = result_component_revision;
                            emit(output_tx, OutputJob::Event(event)).await?;
                        } else {
                            emit(output_tx, OutputJob::Text("void".to_string())).await?;
                        }
                        return Ok(failed);
                    };
                    discover_streams(
                        graph,
                        output_type,
                        &value,
                        "$",
                        None,
                        stdout_format,
                        output_streams,
                    )?;
                    if structured {
                        let mut event =
                            event(AgentInvocationSessionEventKind::Result, idempotency_key);
                        event.agent_id = result_agent_id;
                        event.component_revision = result_component_revision;
                        event.value = Some(proto_value_to_json(graph, output_type, &value)?);
                        emit(output_tx, OutputJob::Event(event)).await?;
                    } else if stdout_format == InvocationStdoutFormat::Value {
                        for (path, rendered) in
                            render_text_fragments(graph, output_type, &value, "$", source_language)?
                        {
                            let rendered = if path == "$" {
                                rendered
                            } else {
                                format!("{path}: {rendered}")
                            };
                            emit(output_tx, OutputJob::Text(rendered)).await?;
                        }
                    }
                }
                Some(invocation_session_result::Result::NoResult(_)) => {
                    if !matches!(output_schema, OutputSchema::Unit) {
                        bail!("session returned no result for a value-returning method");
                    }
                    if structured {
                        let mut event =
                            event(AgentInvocationSessionEventKind::Result, idempotency_key);
                        event.agent_id = result_agent_id;
                        event.component_revision = result_component_revision;
                        emit(output_tx, OutputJob::Event(event)).await?;
                    } else {
                        emit(output_tx, OutputJob::Text("void".to_string())).await?;
                    }
                }
                None => bail!("invocation result has no value"),
            }
        }
        Some(invocation_response::Response::OutputItem(item)) => {
            let stream = output_streams
                .get(&item.stream_id)
                .cloned()
                .ok_or_else(|| anyhow!("output stream {} has no schema", item.stream_id))?;
            let value = item
                .value
                .ok_or_else(|| anyhow!("output stream item has no value"))?;
            discover_streams(
                graph,
                &stream.item_type,
                &value,
                &format!("{}[{}]", stream.path, item.offset),
                Some(item.stream_id),
                InvocationStdoutFormat::Value,
                output_streams,
            )?;
            output_streams
                .get_mut(&item.stream_id)
                .expect("output stream disappeared while processing an item")
                .next_offset = item
                .offset
                .checked_add(1)
                .ok_or_else(|| anyhow!("output stream offset overflow"))?;
            if structured {
                let mut event = event(AgentInvocationSessionEventKind::Item, idempotency_key);
                event.stream_id = Some(item.stream_id);
                event.parent_stream_id = stream.parent_stream_id;
                event.path = Some(stream.path);
                event.offset = Some(item.offset);
                event.value = Some(proto_value_to_json(graph, &stream.item_type, &value)?);
                emit(output_tx, OutputJob::Event(event)).await?;
            } else if stdout_format == InvocationStdoutFormat::Raw {
                emit(output_tx, OutputJob::Raw(raw_output_bytes(&stream, value)?)).await?;
            } else {
                for (path, rendered) in render_text_fragments(
                    graph,
                    &stream.item_type,
                    &value,
                    &stream.path,
                    source_language,
                )? {
                    let rendered =
                        if path != stream.path || output_streams.len() > 1 || stream.path != "$" {
                            format!("{path}: {rendered}")
                        } else {
                            rendered
                        };
                    emit(output_tx, OutputJob::Text(rendered)).await?;
                }
            }
        }
        Some(invocation_response::Response::OutputEnd(end)) => {
            output_streams
                .get_mut(&end.stream_id)
                .ok_or_else(|| anyhow!("output stream {} has no schema", end.stream_id))?
                .terminal = true;
            if structured {
                let stream = output_streams
                    .get(&end.stream_id)
                    .ok_or_else(|| anyhow!("output stream {} has no schema", end.stream_id))?;
                let mut event = event(AgentInvocationSessionEventKind::End, idempotency_key);
                event.stream_id = Some(end.stream_id);
                event.parent_stream_id = stream.parent_stream_id;
                event.path = Some(stream.path.clone());
                event.offset = Some(end.offset);
                emit(output_tx, OutputJob::Event(event)).await?;
            }
        }
        Some(invocation_response::Response::OutputError(error)) => {
            failed = true;
            output_streams
                .get_mut(&error.stream_id)
                .ok_or_else(|| anyhow!("output stream {} has no schema", error.stream_id))?
                .terminal = true;
            if structured {
                let stream = output_streams
                    .get(&error.stream_id)
                    .ok_or_else(|| anyhow!("output stream {} has no schema", error.stream_id))?;
                let mut event = event(
                    AgentInvocationSessionEventKind::StreamError,
                    idempotency_key,
                );
                event.stream_id = Some(error.stream_id);
                event.parent_stream_id = stream.parent_stream_id;
                event.path = Some(stream.path.clone());
                event.offset = Some(error.offset);
                event.error = Some(error.details);
                emit(output_tx, OutputJob::Event(event)).await?;
            } else {
                eprintln!(
                    "Output stream {} failed: {}",
                    error.stream_id, error.details
                );
            }
        }
        Some(invocation_response::Response::InputAck(_)) => {}
        Some(invocation_response::Response::StreamCancel(cancel)) => {
            failed |= cancel.reason() != StreamCancelReason::Cancelled;
            let output_stream = if cancel.role() == StreamCancelRole::OutputProducer {
                let stream = output_streams
                    .get_mut(&cancel.stream_id)
                    .ok_or_else(|| anyhow!("output stream {} has no schema", cancel.stream_id))?;
                stream.terminal = true;
                Some(stream.clone())
            } else {
                None
            };
            if structured {
                let mut event = event(
                    AgentInvocationSessionEventKind::StreamCancel,
                    idempotency_key,
                );
                event.stream_id = Some(cancel.stream_id);
                if let Some(stream) = output_stream {
                    event.parent_stream_id = stream.parent_stream_id;
                    event.path = Some(stream.path);
                }
                event.offset = Some(cancel.offset);
                event.reason = Some(format!("{:?}", cancel.reason()));
                event.error = cancel.details;
                emit(output_tx, OutputJob::Event(event)).await?;
            } else {
                eprintln!("Stream {} was cancelled", cancel.stream_id);
            }
        }
        Some(invocation_response::Response::AttachmentRevoked(revoked)) => {
            bail!("invocation attachment was revoked: {}", revoked.details);
        }
        Some(invocation_response::Response::Finished(finished)) => {
            let (outcome, error) = match finished.outcome {
                Some(invocation_session_completion::Outcome::Success(_)) => {
                    ("success".to_string(), None)
                }
                Some(invocation_session_completion::Outcome::Failure(failure)) => {
                    failed = true;
                    (format!("{:?}", failure.kind()), Some(failure.message))
                }
                None => bail!("invocation completion has no outcome"),
            };
            if structured {
                let mut event = event(AgentInvocationSessionEventKind::Finished, idempotency_key);
                event.agent_id = session_identity.agent_id.clone();
                event.component_revision = session_identity.component_revision;
                event.outcome = Some(outcome);
                event.error = error;
                emit(output_tx, OutputJob::Event(event)).await?;
            } else if let Some(error) = error {
                eprintln!("Invocation failed: {error}");
            }
        }
        None => bail!("empty invocation response"),
    }
    Ok(failed)
}

fn event(
    kind: AgentInvocationSessionEventKind,
    idempotency_key: &str,
) -> Box<AgentInvocationSessionEvent> {
    Box::new(AgentInvocationSessionEvent::new(kind, idempotency_key))
}

async fn emit(output: &OutputChannel, job: OutputJob) -> anyhow::Result<()> {
    tokio::select! {
        biased;
        _ = output.input_failed.cancelled() => bail!(InputFailureSignal),
        _ = output.interrupt.cancelled() => bail!(PipedExitCode(130)),
        result = output.tx.send(job) => result.map_err(|_| anyhow!("invocation output closed unexpectedly")),
    }
}

fn write_output(
    mut rx: mpsc::Receiver<OutputJob>,
    format: Format,
    colorize: bool,
) -> anyhow::Result<()> {
    while let Some(job) = rx.blocking_recv() {
        let bytes = render_output_job(job, format, colorize)?;
        let stdout = std::io::stdout();
        write_and_flush(&mut stdout.lock(), &bytes)?;
    }
    Ok(())
}

#[cfg(test)]
fn write_output_to<W: Write>(
    mut rx: mpsc::Receiver<OutputJob>,
    format: Format,
    colorize: bool,
    output: &mut W,
) -> anyhow::Result<()> {
    while let Some(job) = rx.blocking_recv() {
        let bytes = render_output_job(job, format, colorize)?;
        write_and_flush(output, &bytes)?;
    }
    Ok(())
}

fn render_output_job(job: OutputJob, format: Format, colorize: bool) -> anyhow::Result<Vec<u8>> {
    match job {
        OutputJob::Text(mut text) => {
            text.push('\n');
            Ok(text.into_bytes())
        }
        OutputJob::Raw(bytes) => Ok(bytes),
        OutputJob::Event(event) => {
            let mut document = render_command_output_document(format, colorize, *event)?;
            document.push('\n');
            Ok(document.into_bytes())
        }
    }
}

fn write_and_flush(output: &mut impl Write, bytes: &[u8]) -> anyhow::Result<()> {
    if let Err(error) = output.write_all(bytes).and_then(|()| output.flush()) {
        if error.kind() == ErrorKind::BrokenPipe {
            bail!(PipedExitCode(0));
        }
        return Err(error.into());
    }
    Ok(())
}

fn raw_output_bytes(stream: &OutputStream, value: ProtoSchemaValue) -> anyhow::Result<Vec<u8>> {
    match (stream.raw_kind, value.value) {
        (
            Some(RawStreamKind::Binary),
            Some(schema_value::Value::BinaryValue(BinaryValue { bytes, .. })),
        ) => Ok(bytes),
        (Some(RawStreamKind::U8), Some(schema_value::Value::U8Value(value))) => Ok(vec![
            u8::try_from(value).map_err(|_| anyhow!("u8 stream item is out of range"))?,
        ]),
        _ => bail!("raw output stream item does not match its declared type"),
    }
}

fn render_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: ProtoSchemaValue,
    source_language: &SourceLanguage,
) -> anyhow::Result<String> {
    let value = SchemaValue::try_from(value).map_err(anyhow::Error::msg)?;
    Ok(render_schema_value(graph, ty, &value, source_language))
}

fn render_text_fragments(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &ProtoSchemaValue,
    path: &str,
    source_language: &SourceLanguage,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut fragments = Vec::new();
    collect_text_fragments(graph, ty, value, path, source_language, &mut fragments)?;
    Ok(fragments)
}

fn collect_text_fragments(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &ProtoSchemaValue,
    path: &str,
    source_language: &SourceLanguage,
    fragments: &mut Vec<(String, String)>,
) -> anyhow::Result<()> {
    let ty = graph
        .resolve_ref(ty)
        .map_err(|error| anyhow!(error.to_string()))?;
    let value_body = value
        .value
        .as_ref()
        .ok_or_else(|| anyhow!("schema value at {path} is empty"))?;

    if matches!(ty, SchemaType::Stream { .. }) {
        if matches!(value_body, schema_value::Value::StreamReference(_)) {
            return Ok(());
        }
        bail!("stream value at {path} is not a stream reference");
    }

    if let Ok(rendered) = render_value(graph, ty, value.clone(), source_language) {
        fragments.push((path.to_string(), rendered));
        return Ok(());
    }

    match (ty, value_body) {
        (SchemaType::Record { fields, .. }, schema_value::Value::RecordValue(record)) => {
            if fields.len() != record.fields.len() {
                bail!("record value at {path} has the wrong number of fields");
            }
            for (field, value) in fields.iter().zip(&record.fields) {
                collect_text_fragments(
                    graph,
                    &field.body,
                    value,
                    &format!("{path}.{}", field.name),
                    source_language,
                    fragments,
                )?;
            }
        }
        (SchemaType::Tuple { elements, .. }, schema_value::Value::TupleValue(tuple)) => {
            if elements.len() != tuple.elements.len() {
                bail!("tuple value at {path} has the wrong number of elements");
            }
            for (index, (ty, value)) in elements.iter().zip(&tuple.elements).enumerate() {
                collect_text_fragments(
                    graph,
                    ty,
                    value,
                    &format!("{path}[{index}]"),
                    source_language,
                    fragments,
                )?;
            }
        }
        (SchemaType::List { element, .. }, schema_value::Value::ListValue(list)) => {
            for (index, value) in list.elements.iter().enumerate() {
                collect_text_fragments(
                    graph,
                    element,
                    value,
                    &format!("{path}[{index}]"),
                    source_language,
                    fragments,
                )?;
            }
        }
        (SchemaType::FixedList { element, .. }, schema_value::Value::FixedListValue(list)) => {
            for (index, value) in list.elements.iter().enumerate() {
                collect_text_fragments(
                    graph,
                    element,
                    value,
                    &format!("{path}[{index}]"),
                    source_language,
                    fragments,
                )?;
            }
        }
        (
            SchemaType::Map {
                key,
                value: value_type,
                ..
            },
            schema_value::Value::MapValue(map),
        ) => {
            for (index, entry) in map.entries.iter().enumerate() {
                collect_text_fragments(
                    graph,
                    key,
                    entry
                        .key
                        .as_ref()
                        .ok_or_else(|| anyhow!("map entry at {path}[{index}] has no key"))?,
                    &format!("{path}[{index}].key"),
                    source_language,
                    fragments,
                )?;
                collect_text_fragments(
                    graph,
                    value_type,
                    entry
                        .value
                        .as_ref()
                        .ok_or_else(|| anyhow!("map entry at {path}[{index}] has no value"))?,
                    &format!("{path}[{index}].value"),
                    source_language,
                    fragments,
                )?;
            }
        }
        (SchemaType::Option { inner, .. }, schema_value::Value::OptionValue(option)) => {
            if let Some(value) = option.inner.as_deref() {
                collect_text_fragments(
                    graph,
                    inner,
                    value,
                    &format!("{path}.some"),
                    source_language,
                    fragments,
                )?;
            }
        }
        (SchemaType::Variant { cases, .. }, schema_value::Value::VariantValue(variant)) => {
            let case = cases
                .get(variant.case as usize)
                .ok_or_else(|| anyhow!("variant case at {path} is out of range"))?;
            if let (Some(ty), Some(value)) = (&case.payload, variant.payload.as_deref()) {
                collect_text_fragments(
                    graph,
                    ty,
                    value,
                    &format!("{path}.{}", case.name),
                    source_language,
                    fragments,
                )?;
            }
        }
        (SchemaType::Result { spec, .. }, schema_value::Value::ResultValue(result)) => {
            use golem_api_grpc::proto::golem::schema::result_value::Result;
            match result.result.as_ref() {
                Some(Result::Ok(value)) => collect_text_fragments(
                    graph,
                    spec.ok
                        .as_deref()
                        .ok_or_else(|| anyhow!("unexpected ok payload at {path}"))?,
                    value,
                    &format!("{path}.ok"),
                    source_language,
                    fragments,
                )?,
                Some(Result::Err(value)) => collect_text_fragments(
                    graph,
                    spec.err
                        .as_deref()
                        .ok_or_else(|| anyhow!("unexpected err payload at {path}"))?,
                    value,
                    &format!("{path}.err"),
                    source_language,
                    fragments,
                )?,
                Some(Result::OkUnit(_)) | Some(Result::ErrUnit(_)) => {}
                None => bail!("result value at {path} has no case"),
            }
        }
        (SchemaType::Union { spec, .. }, schema_value::Value::UnionValue(union)) => {
            let branch = spec
                .branches
                .iter()
                .find(|branch| branch.tag == union.tag)
                .ok_or_else(|| anyhow!("unknown union branch '{}' at {path}", union.tag))?;
            collect_text_fragments(
                graph,
                &branch.body,
                union
                    .body
                    .as_deref()
                    .ok_or_else(|| anyhow!("union value at {path} has no body"))?,
                &format!("{path}.{}", union.tag),
                source_language,
                fragments,
            )?;
        }
        _ => bail!("invocation value at {path} does not match its declared schema"),
    }
    Ok(())
}

fn discover_streams(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &ProtoSchemaValue,
    path: &str,
    parent_stream_id: Option<u64>,
    stdout_format: InvocationStdoutFormat,
    output: &mut HashMap<u64, OutputStream>,
) -> anyhow::Result<()> {
    let ty = graph
        .resolve_ref(ty)
        .map_err(|error| anyhow!(error.to_string()))?;
    let Some(value) = value.value.as_ref() else {
        bail!("schema value at {path} is empty");
    };
    match (ty, value) {
        (
            SchemaType::Stream {
                inner: Some(inner), ..
            },
            schema_value::Value::StreamReference(reference),
        ) => {
            let raw_kind = if stdout_format == InvocationStdoutFormat::Raw {
                raw_stream_kind(graph, inner)
            } else {
                None
            };
            if output
                .insert(
                    reference.stream_id,
                    OutputStream {
                        item_type: (**inner).clone(),
                        parent_stream_id,
                        path: path.to_string(),
                        raw_kind,
                        next_offset: 0,
                        terminal: false,
                    },
                )
                .is_some()
            {
                bail!(
                    "output stream {} was discovered more than once",
                    reference.stream_id
                );
            }
        }
        (SchemaType::Record { fields, .. }, schema_value::Value::RecordValue(record)) => {
            for (field, value) in fields.iter().zip(&record.fields) {
                discover_streams(
                    graph,
                    &field.body,
                    value,
                    &format!("{path}.{}", field.name),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
            }
        }
        (SchemaType::Tuple { elements, .. }, schema_value::Value::TupleValue(tuple)) => {
            for (index, (ty, value)) in elements.iter().zip(&tuple.elements).enumerate() {
                discover_streams(
                    graph,
                    ty,
                    value,
                    &format!("{path}[{index}]"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
            }
        }
        (SchemaType::List { element, .. }, schema_value::Value::ListValue(list)) => {
            for (index, value) in list.elements.iter().enumerate() {
                discover_streams(
                    graph,
                    element,
                    value,
                    &format!("{path}[{index}]"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
            }
        }
        (SchemaType::FixedList { element, .. }, schema_value::Value::FixedListValue(list)) => {
            for (index, value) in list.elements.iter().enumerate() {
                discover_streams(
                    graph,
                    element,
                    value,
                    &format!("{path}[{index}]"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
            }
        }
        (
            SchemaType::Map {
                key,
                value: value_type,
                ..
            },
            schema_value::Value::MapValue(map),
        ) => {
            for (index, entry) in map.entries.iter().enumerate() {
                if let Some(value) = &entry.key {
                    discover_streams(
                        graph,
                        key,
                        value,
                        &format!("{path}[{index}].key"),
                        parent_stream_id,
                        stdout_format,
                        output,
                    )?;
                }
                if let Some(value) = &entry.value {
                    discover_streams(
                        graph,
                        value_type,
                        value,
                        &format!("{path}[{index}].value"),
                        parent_stream_id,
                        stdout_format,
                        output,
                    )?;
                }
            }
        }
        (SchemaType::Option { inner, .. }, schema_value::Value::OptionValue(option)) => {
            if let Some(value) = &option.inner {
                discover_streams(
                    graph,
                    inner,
                    value,
                    &format!("{path}.some"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
            }
        }
        (SchemaType::Variant { cases, .. }, schema_value::Value::VariantValue(variant)) => {
            if let Some(case) = cases.get(variant.case as usize)
                && let (Some(ty), Some(value)) = (&case.payload, &variant.payload)
            {
                discover_streams(
                    graph,
                    ty,
                    value,
                    &format!("{path}.{}", case.name),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
            }
        }
        (SchemaType::Result { spec, .. }, schema_value::Value::ResultValue(result)) => {
            use golem_api_grpc::proto::golem::schema::result_value::Result;
            match result.result.as_ref() {
                Some(Result::Ok(value)) if spec.ok.is_some() => discover_streams(
                    graph,
                    spec.ok.as_deref().unwrap(),
                    value,
                    &format!("{path}.ok"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?,
                Some(Result::Err(value)) if spec.err.is_some() => discover_streams(
                    graph,
                    spec.err.as_deref().unwrap(),
                    value,
                    &format!("{path}.err"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?,
                _ => {}
            }
        }
        (SchemaType::Union { spec, .. }, schema_value::Value::UnionValue(union)) => {
            if let Some(branch) = spec.branches.iter().find(|branch| branch.tag == union.tag)
                && let Some(value) = &union.body
            {
                discover_streams(
                    graph,
                    &branch.body,
                    value,
                    &format!("{path}.{}", union.tag),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
            }
        }
        _ if stdout_format == InvocationStdoutFormat::Raw => {
            bail!("raw output value at {path} does not match its declared direct stream schema")
        }
        _ => {}
    }
    Ok(())
}

fn proto_value_to_json(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &ProtoSchemaValue,
) -> anyhow::Result<serde_json::Value> {
    if let Ok(value) = SchemaValue::try_from(value.clone()) {
        return golem_common::schema::render::to_json_value(graph, ty, &value).map_err(Into::into);
    }

    let ty = graph
        .resolve_ref(ty)
        .map_err(|error| anyhow!(error.to_string()))?;
    let value = value
        .value
        .as_ref()
        .ok_or_else(|| anyhow!("empty schema value"))?;
    Ok(match (ty, value) {
        (SchemaType::Stream { .. }, schema_value::Value::StreamReference(reference)) => {
            serde_json::json!({ "$stream": reference.stream_id })
        }
        (SchemaType::Record { fields, .. }, schema_value::Value::RecordValue(record)) => {
            if fields.len() != record.fields.len() {
                bail!("record result has the wrong number of fields");
            }
            let mut result = serde_json::Map::new();
            for (field, value) in fields.iter().zip(&record.fields) {
                result.insert(
                    field.name.clone(),
                    proto_value_to_json(graph, &field.body, value)?,
                );
            }
            serde_json::Value::Object(result)
        }
        (SchemaType::Tuple { elements, .. }, schema_value::Value::TupleValue(tuple)) => {
            if elements.len() != tuple.elements.len() {
                bail!("tuple result has the wrong number of elements");
            }
            serde_json::Value::Array(
                elements
                    .iter()
                    .zip(&tuple.elements)
                    .map(|(ty, value)| proto_value_to_json(graph, ty, value))
                    .collect::<Result<_, _>>()?,
            )
        }
        (SchemaType::List { element, .. }, schema_value::Value::ListValue(list)) => {
            serde_json::Value::Array(
                list.elements
                    .iter()
                    .map(|value| proto_value_to_json(graph, element, value))
                    .collect::<Result<_, _>>()?,
            )
        }
        (SchemaType::FixedList { element, .. }, schema_value::Value::FixedListValue(list)) => {
            serde_json::Value::Array(
                list.elements
                    .iter()
                    .map(|value| proto_value_to_json(graph, element, value))
                    .collect::<Result<_, _>>()?,
            )
        }
        (
            SchemaType::Map {
                key,
                value: value_type,
                ..
            },
            schema_value::Value::MapValue(map),
        ) => serde_json::Value::Array(
            map.entries
                .iter()
                .map(|entry| {
                    Ok(serde_json::Value::Array(vec![
                        proto_value_to_json(
                            graph,
                            key,
                            entry
                                .key
                                .as_ref()
                                .ok_or_else(|| anyhow!("map entry has no key"))?,
                        )?,
                        proto_value_to_json(
                            graph,
                            value_type,
                            entry
                                .value
                                .as_ref()
                                .ok_or_else(|| anyhow!("map entry has no value"))?,
                        )?,
                    ]))
                })
                .collect::<anyhow::Result<_>>()?,
        ),
        (SchemaType::Option { inner, .. }, schema_value::Value::OptionValue(option)) => option
            .inner
            .as_deref()
            .map(|value| proto_value_to_json(graph, inner, value))
            .transpose()?
            .unwrap_or(serde_json::Value::Null),
        (SchemaType::Variant { cases, .. }, schema_value::Value::VariantValue(variant)) => {
            let case = cases
                .get(variant.case as usize)
                .ok_or_else(|| anyhow!("variant case is out of range"))?;
            match (&case.payload, variant.payload.as_deref()) {
                (None, None) => case.name.clone().into(),
                (Some(ty), Some(value)) => serde_json::json!({
                    case.name.clone(): proto_value_to_json(graph, ty, value)?
                }),
                _ => bail!("variant payload does not match its declared case"),
            }
        }
        (SchemaType::Result { spec, .. }, schema_value::Value::ResultValue(result)) => {
            use golem_api_grpc::proto::golem::schema::result_value::Result;
            match result.result.as_ref() {
                Some(Result::Ok(value)) => serde_json::json!({
                    "ok": proto_value_to_json(
                        graph,
                        spec.ok.as_deref().ok_or_else(|| anyhow!("unexpected ok payload"))?,
                        value,
                    )?
                }),
                Some(Result::Err(value)) => serde_json::json!({
                    "err": proto_value_to_json(
                        graph,
                        spec.err.as_deref().ok_or_else(|| anyhow!("unexpected err payload"))?,
                        value,
                    )?
                }),
                Some(Result::OkUnit(_)) => serde_json::json!({ "ok": null }),
                Some(Result::ErrUnit(_)) => serde_json::json!({ "err": null }),
                None => bail!("result has no case"),
            }
        }
        (SchemaType::Union { spec, .. }, schema_value::Value::UnionValue(union)) => {
            let branch = spec
                .branches
                .iter()
                .find(|branch| branch.tag == union.tag)
                .ok_or_else(|| anyhow!("unknown union branch '{}'", union.tag))?;
            proto_value_to_json(
                graph,
                &branch.body,
                union
                    .body
                    .as_deref()
                    .ok_or_else(|| anyhow!("union has no body"))?,
            )?
        }
        _ => bail!("invocation result does not match its declared schema"),
    })
}

fn is_connection_closed(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed
    ) || matches!(error, tungstenite::Error::Io(error) if error.kind() == ErrorKind::BrokenPipe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::agent::{InputSchema, NamedField};
    use golem_common::schema::metadata::MetadataEnvelope;
    use golem_common::schema::schema_type::{BinaryRestrictions, NamedFieldType};
    use test_r::test;

    fn method(parameters: Vec<NamedField>, output_schema: OutputSchema) -> AgentMethodSchema {
        AgentMethodSchema {
            name: "test".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(parameters),
            output_schema,
            http_endpoint: Vec::new(),
            read_only: None,
        }
    }

    #[test]
    fn raw_stream_kind_resolves_refs() {
        let graph = SchemaGraph::empty();
        assert_eq!(
            raw_stream_kind(&graph, &SchemaType::u8()),
            Some(RawStreamKind::U8)
        );
        assert_eq!(raw_stream_kind(&graph, &SchemaType::string()), None);
    }

    #[test]
    fn value_stdin_parser_preserves_blank_strings() {
        let value = parse_method_argument_schema_value(
            "",
            &SchemaGraph::empty(),
            &SchemaType::string(),
            &SourceLanguage::Rust,
        )
        .unwrap();
        assert_eq!(value, SchemaValue::String(String::new()));
    }

    #[test]
    fn input_failure_uses_producer_cancellation() {
        let request = input_cancel_request(
            1,
            4,
            StreamCancelReason::Protocol,
            "invalid input".to_string(),
        );
        let Some(public_invocation_request::Request::StreamCancel(cancel)) = request.request else {
            panic!("expected stream cancellation");
        };
        assert_eq!(cancel.stream_id, 1);
        assert_eq!(cancel.offset, 4);
        assert_eq!(cancel.role(), StreamCancelRole::InputProducer);
        assert_eq!(cancel.reason(), StreamCancelReason::Protocol);
    }

    #[test]
    fn discovers_nested_output_streams() {
        let graph = SchemaGraph::empty();
        let ty = SchemaType::Record {
            fields: vec![NamedFieldType {
                name: "items".to_string(),
                body: SchemaType::stream(Some(SchemaType::u32())),
                metadata: MetadataEnvelope::default(),
            }],
            metadata: MetadataEnvelope::default(),
        };
        let value = ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![ProtoSchemaValue {
                    value: Some(schema_value::Value::StreamReference(
                        SchemaValueStreamReference { stream_id: 3 },
                    )),
                }],
            })),
        };
        let mut streams = HashMap::new();
        discover_streams(
            &graph,
            &ty,
            &value,
            "$",
            None,
            InvocationStdoutFormat::Value,
            &mut streams,
        )
        .unwrap();
        assert_eq!(streams.get(&3).unwrap().path, "$.items");
    }

    #[test]
    async fn raw_output_rejects_result_that_does_not_match_stream_schema() {
        let graph = SchemaGraph::empty();
        let output_schema =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8()))));
        let response = InvocationResponse {
            response: Some(invocation_response::Response::Result(
                golem_api_grpc::proto::golem::worker::InvocationSessionResult {
                    result: Some(invocation_session_result::Result::MethodResult(
                        SchemaValue::U8(1).try_into().unwrap(),
                    )),
                    ..Default::default()
                },
            )),
        };
        let (tx, _rx) = mpsc::channel(1);
        let output = OutputChannel {
            tx,
            interrupt: CancellationToken::new(),
            input_failed: CancellationToken::new(),
        };

        let result = handle_response(
            response,
            &graph,
            &output_schema,
            &SourceLanguage::Rust,
            InvocationStdoutFormat::Raw,
            false,
            "session-key",
            &mut SessionIdentity::default(),
            &mut HashMap::new(),
            &output,
        )
        .await;

        assert!(
            result.is_err(),
            "raw output must reject a scalar result for a declared stream schema"
        );
    }

    #[test]
    fn text_rendering_preserves_scalar_fragments_around_streams() {
        let graph = SchemaGraph::empty();
        let ty = SchemaType::Tuple {
            elements: vec![
                SchemaType::string(),
                SchemaType::stream(Some(SchemaType::u32())),
            ],
            metadata: MetadataEnvelope::default(),
        };
        let value = ProtoSchemaValue {
            value: Some(schema_value::Value::TupleValue(
                golem_api_grpc::proto::golem::schema::TupleValue {
                    elements: vec![
                        SchemaValue::String("metadata".to_string())
                            .try_into()
                            .unwrap(),
                        ProtoSchemaValue {
                            value: Some(schema_value::Value::StreamReference(
                                SchemaValueStreamReference { stream_id: 3 },
                            )),
                        },
                    ],
                },
            )),
        };

        assert_eq!(
            render_text_fragments(&graph, &ty, &value, "$", &SourceLanguage::Rust).unwrap(),
            vec![("$[0]".to_string(), "\"metadata\"".to_string())]
        );
    }

    #[test]
    fn direct_stream_parameter_binds_stdin_with_odd_client_id() {
        let graph = SchemaGraph::empty();
        let method = method(
            vec![NamedField::user_supplied(
                "values",
                SchemaType::stream(Some(SchemaType::u32())),
            )],
            OutputSchema::Unit,
        );
        let (parameters, binding) = prepare_method_parameters(
            &graph,
            &method,
            vec!["-".to_string()],
            &SourceLanguage::Rust,
            InvocationStdinFormat::Value,
        )
        .unwrap();

        let binding = binding.unwrap();
        assert_eq!(binding.stream_id, 1);
        assert_eq!(binding.parameter_name, "values");
        let Some(schema_value::Value::RecordValue(record)) = parameters.value else {
            panic!("expected parameter record");
        };
        assert!(matches!(
            record.fields[0].value,
            Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: 1 }
            ))
        ));
    }

    #[test]
    fn multiple_and_nested_input_streams_are_rejected() {
        let graph = SchemaGraph::empty();
        let direct = method(
            vec![
                NamedField::user_supplied("left", SchemaType::stream(Some(SchemaType::u32()))),
                NamedField::user_supplied("right", SchemaType::stream(Some(SchemaType::u32()))),
            ],
            OutputSchema::Unit,
        );
        assert!(
            prepare_method_parameters(
                &graph,
                &direct,
                vec!["-".to_string(), "-".to_string()],
                &SourceLanguage::Rust,
                InvocationStdinFormat::Value,
            )
            .unwrap_err()
            .to_string()
            .contains("one stream parameter")
        );

        let nested = method(
            vec![NamedField::user_supplied(
                "nested",
                SchemaType::record(vec![NamedFieldType {
                    name: "values".to_string(),
                    body: SchemaType::stream(Some(SchemaType::u32())),
                    metadata: MetadataEnvelope::default(),
                }]),
            )],
            OutputSchema::Unit,
        );
        assert!(
            prepare_method_parameters(
                &graph,
                &nested,
                vec!["-".to_string()],
                &SourceLanguage::Rust,
                InvocationStdinFormat::Value,
            )
            .unwrap_err()
            .to_string()
            .contains("direct stream")
        );

        let nested_item = method(
            vec![NamedField::user_supplied(
                "nested-item",
                SchemaType::stream(Some(SchemaType::record(vec![NamedFieldType {
                    name: "values".to_string(),
                    body: SchemaType::stream(Some(SchemaType::u32())),
                    metadata: MetadataEnvelope::default(),
                }]))),
            )],
            OutputSchema::Unit,
        );
        assert!(
            prepare_method_parameters(
                &graph,
                &nested_item,
                vec!["-".to_string()],
                &SourceLanguage::Rust,
                InvocationStdinFormat::Value,
            )
            .unwrap_err()
            .to_string()
            .contains("cannot contain nested streams")
        );
    }

    #[test]
    fn raw_formats_accept_only_direct_byte_streams() {
        let graph = SchemaGraph::empty();
        let invalid_input = method(
            vec![NamedField::user_supplied(
                "values",
                SchemaType::stream(Some(SchemaType::u32())),
            )],
            OutputSchema::Unit,
        );
        assert!(
            prepare_method_parameters(
                &graph,
                &invalid_input,
                vec!["-".to_string()],
                &SourceLanguage::Rust,
                InvocationStdinFormat::Raw,
            )
            .unwrap_err()
            .to_string()
            .contains("stream<binary> or stream<u8>")
        );

        validate_stdout_format(
            &graph,
            &OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::binary(
                BinaryRestrictions::default(),
            ))))),
            InvocationStdoutFormat::Raw,
        )
        .unwrap();
        assert!(
            validate_stdout_format(
                &graph,
                &OutputSchema::Single(Box::new(SchemaType::record(vec![NamedFieldType {
                    name: "bytes".to_string(),
                    body: SchemaType::stream(Some(SchemaType::u8())),
                    metadata: MetadataEnvelope::default(),
                }]))),
                InvocationStdoutFormat::Raw,
            )
            .unwrap_err()
            .to_string()
            .contains("one direct stream")
        );
    }

    #[test]
    fn structured_values_preserve_field_names_and_stream_references() {
        let graph = SchemaGraph::empty();
        let ty = SchemaType::record(vec![
            NamedFieldType {
                name: "count".to_string(),
                body: SchemaType::u32(),
                metadata: MetadataEnvelope::default(),
            },
            NamedFieldType {
                name: "items".to_string(),
                body: SchemaType::stream(Some(SchemaType::string())),
                metadata: MetadataEnvelope::default(),
            },
        ]);
        let value = ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![
                    SchemaValue::U32(2).try_into().unwrap(),
                    ProtoSchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id: 9 },
                        )),
                    },
                ],
            })),
        };

        assert_eq!(
            proto_value_to_json(&graph, &ty, &value).unwrap(),
            serde_json::json!({ "count": 2, "items": { "$stream": 9 } })
        );
    }

    #[test]
    fn input_cancellation_is_detected_for_the_bound_stream_only() {
        let response = InvocationResponse {
            response: Some(invocation_response::Response::StreamCancel(
                golem_api_grpc::proto::golem::worker::StreamCancel {
                    stream_id: 1,
                    offset: 0,
                    role: StreamCancelRole::InputConsumer as i32,
                    reason: StreamCancelReason::Cancelled as i32,
                    details: None,
                },
            )),
        };
        assert!(response_cancels_input(&response, Some(1)));
        assert!(!response_cancels_input(&response, Some(3)));
    }

    #[test]
    async fn connection_truncation_before_finish_is_an_error() {
        let (wire_tx, _wire_rx) = mpsc::channel(1);
        let error = receive_response(
            None,
            &wire_tx,
            &CancellationToken::new(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("ended before completion"));
    }

    #[test]
    async fn event_after_finish_is_an_error() {
        let (wire_tx, _wire_rx) = mpsc::channel(1);
        let (_output_tx, mut output_rx) = oneshot::channel();
        let mut frames = futures_util::stream::iter([Ok(Message::Binary(Vec::new().into()))]);
        let error = await_clean_close(
            &mut frames,
            &wire_tx,
            &CancellationToken::new(),
            &mut output_rx,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("event after completion"));
    }

    #[test]
    async fn clean_close_wait_is_interrupted() {
        let (wire_tx, _wire_rx) = mpsc::channel(1);
        let (_output_tx, mut output_rx) = oneshot::channel();
        let mut frames = futures_util::stream::pending();
        let interrupt = CancellationToken::new();
        interrupt.cancel();

        let error = await_clean_close(&mut frames, &wire_tx, &interrupt, &mut output_rx)
            .await
            .unwrap_err();
        assert_eq!(error.downcast_ref::<PipedExitCode>().unwrap().0, 130);
    }

    #[test]
    async fn clean_close_wait_reports_broken_pipe() {
        let (wire_tx, _wire_rx) = mpsc::channel(1);
        let (output_tx, mut output_rx) = oneshot::channel();
        let mut frames = futures_util::stream::pending();
        output_tx
            .send(Err(anyhow!(PipedExitCode(0))))
            .expect("failed to send output error");

        let error = await_clean_close(
            &mut frames,
            &wire_tx,
            &CancellationToken::new(),
            &mut output_rx,
        )
        .await
        .unwrap_err();
        assert_eq!(error.downcast_ref::<PipedExitCode>().unwrap().0, 0);
    }

    #[test]
    async fn saturated_wire_send_is_interrupted() {
        let (wire_tx, _wire_rx) = mpsc::channel(1);
        wire_tx.try_send(Message::Ping(Vec::new().into())).unwrap();
        let interrupt = CancellationToken::new();
        let cancel = interrupt.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            send_message(&wire_tx, Message::Pong(Vec::new().into()), &interrupt),
        )
        .await
        .expect("saturated send did not observe cancellation")
        .unwrap_err();
        assert_eq!(error.downcast_ref::<PipedExitCode>().unwrap().0, 130);
    }

    #[test]
    async fn saturated_pong_send_observes_input_failure() {
        let (wire_tx, _wire_rx) = mpsc::channel(1);
        wire_tx.try_send(Message::Ping(Vec::new().into())).unwrap();
        let input_failed = CancellationToken::new();
        let cancel = input_failed.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            receive_response(
                Some(Ok(Message::Ping(Vec::new().into()))),
                &wire_tx,
                &CancellationToken::new(),
                &input_failed,
            ),
        )
        .await
        .expect("saturated Pong send did not observe input failure")
        .unwrap_err();
        assert!(error.downcast_ref::<InputFailureSignal>().is_some());
    }

    #[test]
    async fn saturated_output_send_observes_input_failure() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(OutputJob::Text("first".to_string())).unwrap();
        let input_failed = CancellationToken::new();
        let cancel = input_failed.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });
        let output = OutputChannel {
            tx,
            interrupt: CancellationToken::new(),
            input_failed,
        };

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            emit(&output, OutputJob::Text("second".to_string())),
        )
        .await
        .expect("saturated output send did not observe input failure")
        .unwrap_err();
        assert!(error.downcast_ref::<InputFailureSignal>().is_some());
    }

    struct BrokenOnFlush {
        bytes: Vec<u8>,
    }

    impl Write for BrokenOnFlush {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(ErrorKind::BrokenPipe, "closed"))
        }
    }

    #[test]
    fn output_flush_detects_broken_pipe_after_one_raw_item() {
        let (output_tx, output_rx) = mpsc::channel(1);
        assert!(output_tx.try_send(OutputJob::Raw(vec![1])).is_ok());
        drop(output_tx);
        let mut output = BrokenOnFlush { bytes: Vec::new() };

        let error = write_output_to(output_rx, Format::Text, false, &mut output).unwrap_err();
        assert_eq!(output.bytes, vec![1]);
        assert_eq!(error.downcast_ref::<PipedExitCode>().unwrap().0, 0);
    }
}
