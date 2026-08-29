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
use golem_client::invocation_session::{
    AdmittedInput, DeliveryTracker, InputReplayBuffer, InvocationSession,
    InvocationSessionRequestProvider, InvocationSessionSender, InvocationSessionStateObserver,
    InvocationSessionStateSnapshot, ReplayableInput, ServerFrame, SessionTransportError,
    send_replayable_input,
};
use golem_common::model::IdempotencyKey;
use golem_common::model::invocation_session_public::{
    BinaryMessage, BinaryMessageKind, BinaryMessageMetadata, DecimalU64,
    INVOCATION_SESSION_VERSION, InvocationSelector, PublicClientCancelReason, PublicClientMessage,
    PublicConfigEntry, PublicInvocationOutcome, PublicInvocationResult, PublicOutputStreamOutcome,
    PublicResumeOperation, PublicServerCancelReason, PublicServerMessage,
};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::schema::agent::{
    AgentMethodSchema, AgentTypeSchema, FieldSource, OutputSchema, ParsedAgentId,
};
use golem_common::schema::public_json::{
    PublicSchemaValueError, PublicStreamReference, PublicStreamReferencePolicy,
    decode_public_schema_value, encode_public_schema_value, encode_public_schema_value_with_charge,
};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::{
    BinaryValuePayload, NamedFieldType, ResultValuePayload, SchemaGraph, SchemaType, SchemaValue,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, ErrorKind, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::sync::CancellationToken;

const PIPELINE_CAPACITY: usize = 16;
const RAW_CHUNK_SIZE: usize = 64 * 1024;
const UNACKNOWLEDGED_INPUT_BYTES: usize = 16 * 1024 * 1024;
const SESSION_CHECKPOINT_VERSION: u8 = 1;

pub(super) enum InvocationSessionMode {
    Start { save_session: Option<PathBuf> },
    Resume { path: PathBuf, takeover: bool },
}

impl InvocationSessionMode {
    pub(super) fn resume_path(&self) -> Option<&Path> {
        match self {
            Self::Start { .. } => None,
            Self::Resume { path, .. } => Some(path),
        }
    }

    pub(super) fn uses_checkpoint(&self) -> bool {
        match self {
            Self::Start { save_session } => save_session.is_some(),
            Self::Resume { .. } => true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InvocationSessionCheckpoint {
    version: u8,
    protocol_version: u8,
    schema_evidence: String,
    selector: InvocationSelector,
    idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_token: Option<String>,
    delivered_output_cursors: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_operation: Option<PendingOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingOperationKind {
    Start,
    Resume,
    Takeover,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PendingOperation {
    request: PublicClientMessage,
}

struct CliSessionRequestProvider {
    ctx: Arc<Context>,
}

#[async_trait::async_trait]
impl InvocationSessionRequestProvider for CliSessionRequestProvider {
    async fn request(&self) -> Result<tungstenite::http::Request<()>, SessionTransportError> {
        websocket_request(&self.ctx)
            .await
            .map_err(|error| SessionTransportError::RequestProvider(error.to_string()))
    }
}

struct CliCheckpointObserver {
    checkpoint: Mutex<Option<InvocationSessionCheckpoint>>,
    path: Option<PathBuf>,
}

impl InvocationSessionStateObserver for CliCheckpointObserver {
    fn state_changed(&self, state: &InvocationSessionStateSnapshot) -> Result<(), String> {
        let mut checkpoint = self
            .checkpoint
            .lock()
            .map_err(|_| "checkpoint state mutex poisoned".to_string())?;
        let Some(checkpoint) = checkpoint.as_mut() else {
            return Ok(());
        };
        checkpoint.session_token = state.session_token.clone();
        checkpoint.delivered_output_cursors = state.delivered_output_cursors.clone();
        checkpoint.pending_operation = state
            .pending_operation
            .clone()
            .map(|request| PendingOperation { request });
        write_checkpoint(
            self.path
                .as_deref()
                .ok_or_else(|| "checkpoint state has no path".to_string())?,
            checkpoint,
        )
        .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug)]
struct InputBinding {
    provisional_ref: uuid::Uuid,
    stream_token: Option<String>,
    channel: Option<u32>,
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
    parent_stream_id: Option<u32>,
    path: String,
    raw_kind: Option<RawStreamKind>,
    next_offset: Option<u64>,
    terminal: bool,
}

enum OutputJob {
    Text(String),
    Raw(Vec<u8>),
    Event(Box<AgentInvocationSessionEvent>),
}

struct QueuedOutput {
    job: OutputJob,
    written: oneshot::Sender<Result<(), String>>,
}

struct OutputChannel {
    tx: mpsc::Sender<QueuedOutput>,
    interrupt: CancellationToken,
    input_failed: CancellationToken,
}

struct InputFailure {
    error: anyhow::Error,
    reason: PublicClientCancelReason,
}

#[derive(Debug, thiserror::Error)]
#[error("stdin input failed")]
struct InputFailureSignal;

struct SessionIdentity {
    agent_id: String,
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
    pub selected_agent_name: String,
    pub session_mode: InvocationSessionMode,
}

pub(super) fn load_session_idempotency_key(path: &Path) -> anyhow::Result<IdempotencyKey> {
    let checkpoint = load_checkpoint(path)?;
    Ok(IdempotencyKey::new(checkpoint.idempotency_key))
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
    let input_graph = public_input_graph(&args.agent_type.schema, &method.input_schema);
    let (method_parameters, mut input_binding) = prepare_method_parameters(
        &args.agent_type.schema,
        &method,
        args.arguments,
        &source_language,
        args.stdin_format,
    )?;
    if args.stdin_format == InvocationStdinFormat::Raw && input_binding.is_none() {
        bail!("--stdin-format raw requires stdin bound to stream<binary> or stream<u8> with '-'");
    }
    validate_stdout_format(
        &args.agent_type.schema,
        &method.output_schema,
        args.stdout_format,
    )?;

    let constructor_parameters = encode_public_schema_value(
        args.parsed_agent_id.parameters.graph(),
        &args.parsed_agent_id.parameters.graph().root,
        args.parsed_agent_id.parameters.value(),
        |_, _| {
            Err(PublicSchemaValueError::new(
                golem_common::model::invocation_session_public::PublicErrorCode::UnsupportedValue,
                "constructor parameters cannot contain streams",
            ))
        },
    )?;
    let method_parameters = encode_public_schema_value(
        &input_graph,
        &input_graph.root,
        &method_parameters,
        |stream, _| {
            stream
                .with_host_endpoint::<uuid::Uuid, _>(|reference| {
                    PublicStreamReference::Provisional(*reference)
                })
                .map_err(|message| {
                    PublicSchemaValueError::new(
                        golem_common::model::invocation_session_public::PublicErrorCode::StreamAlreadyConsumed,
                        message,
                    )
                })
        },
    )?;
    let selector = InvocationSelector {
        agent_type: args.parsed_agent_id.agent_type.to_string(),
        application: args.application_name.clone(),
        constructor_parameters,
        environment: args.environment_name.clone(),
        method: args.method_name.clone(),
        phantom_id: args.parsed_agent_id.phantom_id,
    };
    let schema_evidence = schema_evidence(&args.agent_type, &method)?;
    let config = args
        .config
        .iter()
        .cloned()
        .map(|entry| PublicConfigEntry {
            path: entry.path,
            value: entry.value.into(),
        })
        .collect::<Vec<_>>();
    let idempotency_key_value = args.idempotency_key.value.clone();
    let resuming_from_checkpoint =
        matches!(&args.session_mode, InvocationSessionMode::Resume { .. });
    let (initial, pending_operation_is_retry, checkpoint, checkpoint_path) = match args.session_mode
    {
        InvocationSessionMode::Start { save_session } => {
            let attempt_id = uuid::Uuid::new_v4();
            let initial = PublicClientMessage::InvocationStart {
                attempt_id,
                config: config.clone(),
                idempotency_key: idempotency_key_value.clone(),
                method_parameters: method_parameters.clone(),
                selector: selector.clone(),
                version: INVOCATION_SESSION_VERSION,
            };
            let checkpoint = save_session.as_ref().map(|_| InvocationSessionCheckpoint {
                version: SESSION_CHECKPOINT_VERSION,
                protocol_version: INVOCATION_SESSION_VERSION,
                schema_evidence: schema_evidence.clone(),
                selector: selector.clone(),
                idempotency_key: idempotency_key_value.clone(),
                session_token: None,
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(PendingOperation {
                    request: initial.clone(),
                }),
            });
            (initial, false, checkpoint, save_session)
        }
        InvocationSessionMode::Resume { path, takeover } => {
            let mut checkpoint = load_checkpoint(&path)?;
            if checkpoint.selector != selector
                || checkpoint.idempotency_key != idempotency_key_value
                || checkpoint.schema_evidence != schema_evidence
            {
                bail!("saved invocation session is incompatible with the requested invocation");
            }
            let pending_operation_is_retry = checkpoint.pending_operation.is_some();
            let initial = match checkpoint.pending_operation.as_ref() {
                Some(PendingOperation { request })
                    if matches!(request, PublicClientMessage::InvocationStart { .. }) =>
                {
                    validate_pending_start(
                        request,
                        &selector,
                        &config,
                        &idempotency_key_value,
                        &method_parameters,
                        input_binding.as_mut(),
                    )?;
                    request.clone()
                }
                Some(PendingOperation { request }) => {
                    let kind = pending_operation_kind(request)?;
                    if (kind == PendingOperationKind::Takeover) != takeover {
                        bail!(
                            "saved invocation session has an in-flight {} operation",
                            if kind == PendingOperationKind::Takeover {
                                "takeover"
                            } else {
                                "resume"
                            }
                        );
                    }
                    validate_pending_resume(&checkpoint, request, kind)?;
                    request.clone()
                }
                None => {
                    let kind = if takeover {
                        PendingOperationKind::Takeover
                    } else {
                        PendingOperationKind::Resume
                    };
                    let attempt_id = uuid::Uuid::new_v4();
                    let request = resume_request(&checkpoint, kind, attempt_id)?;
                    checkpoint.pending_operation = Some(PendingOperation {
                        request: request.clone(),
                    });
                    request
                }
            };
            (
                initial,
                pending_operation_is_retry,
                Some(checkpoint),
                Some(path),
            )
        }
    };
    let connector = if ctx.allow_insecure() {
        Some(super::stream::insecure_connector()?)
    } else {
        None
    };
    let interrupt = CancellationToken::new();
    let signal_interrupt = interrupt.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_interrupt.cancel();
        }
    });

    let initial_state = InvocationSessionStateSnapshot {
        delivered_output_cursors: checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.delivered_output_cursors.clone())
            .unwrap_or_default(),
        pending_operation: Some(initial),
        session_token: checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.session_token.clone()),
    };
    let checkpoint_observer = Arc::new(CliCheckpointObserver {
        checkpoint: Mutex::new(checkpoint),
        path: checkpoint_path,
    });
    let request_provider = Arc::new(CliSessionRequestProvider { ctx: ctx.clone() });
    let mut session = tokio::select! {
        biased;
        _ = interrupt.cancelled() => bail!(PipedExitCode(130)),
        result = InvocationSession::open(
            request_provider,
            connector,
            initial_state,
            pending_operation_is_retry,
            checkpoint_observer,
        ) => result.context("failed to connect to the agent invocation session")?,
    };

    let (input_tx, mut input_rx) = mpsc::channel::<AdmittedInput>(PIPELINE_CAPACITY);
    let input_buffer = InputReplayBuffer::new(UNACKNOWLEDGED_INPUT_BYTES, PIPELINE_CAPACITY);
    let (input_failure_tx, mut input_failure_rx) = oneshot::channel::<InputFailure>();
    let input_cancelled = CancellationToken::new();
    let input_discarded = CancellationToken::new();
    let input_failed = CancellationToken::new();
    let stdin_format = args.stdin_format;
    let validate_discarded_input =
        stdin_format == InvocationStdinFormat::Value && !std::io::stdin().is_terminal();
    let has_input = input_binding.is_some();
    if resuming_from_checkpoint && has_input {
        eprintln!(
            "Resumed stdin starts after the server's durable input high-water; stdin bytes lost when the previous CLI process exited cannot be recovered."
        );
    }
    let mut input_failure_open = has_input;
    if let Some(binding) = input_binding.as_ref() {
        session.register_input(
            binding.provisional_ref,
            binding.stream_token.clone(),
            input_buffer.clone(),
        )?;
    }
    if let Some(binding) = input_binding.clone() {
        let reader_cancelled = input_cancelled.clone();
        let reader_discarded = input_discarded.clone();
        let reader_failed = input_failed.clone();
        let reader_source_language = source_language.clone();
        let reader_graph = args.agent_type.schema.clone();
        let reader_tx = input_tx.clone();
        let reader_input_buffer = input_buffer.clone();
        let runtime = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            if let Err(failure) = read_stdin(
                binding,
                stdin_format,
                reader_source_language,
                reader_graph,
                &reader_tx,
                &reader_input_buffer,
                &runtime,
                &reader_cancelled,
                &reader_discarded,
            ) && input_failure_tx.send(failure).is_ok()
            {
                reader_failed.cancel();
            }
        });
    }

    let format = ctx.format();
    let structured = format.is_structured() && args.stdout_format == InvocationStdoutFormat::Value;
    let (output_job_tx, output_rx) = mpsc::channel::<QueuedOutput>(PIPELINE_CAPACITY);
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

    let mut output_streams = HashMap::<u32, OutputStream>::new();
    let bindings = session.delivery_tracker();
    let mut failed = false;
    let mut accepted = false;
    let mut stdin_open = has_input;
    let mut input_terminal = !has_input;
    let mut fatal_input_failure = None;
    let session_identity = SessionIdentity {
        agent_id: args.selected_agent_name.clone(),
    };
    let mut finished = false;

    'session: while !finished {
        let next_unsent_input = input_buffer.next_unsent();
        let sender = session.sender();
        tokio::select! {
            biased;
            _ = interrupt.cancelled() => {
                input_cancelled.cancel();
                input_rx.close();
                let requests = cancel_open_streams(
                    if accepted && !input_terminal {
                        input_binding.as_ref().and_then(|binding| binding.channel)
                    } else {
                        None
                    },
                    &output_streams,
                    PublicClientCancelReason::Cancelled,
                );
                cancel_before_close(&mut session, requests).await;
                bail!(PipedExitCode(130));
            }
            failure = &mut input_failure_rx, if input_failure_open => {
                input_failure_open = false;
                if let Ok(failure) = failure {
                    fatal_input_failure = Some(failure);
                    if accepted {
                        break 'session;
                    }
                }
            }
            result = &mut output_result_rx => {
                match result {
                    Ok(Ok(())) => bail!("invocation output closed before completion"),
                    Ok(Err(error)) if error.downcast_ref::<PipedExitCode>().is_some_and(|exit| exit.0 == 0) => {
                        input_cancelled.cancel();
                        input_rx.close();
                        let requests = cancel_open_streams(
                            if accepted && !input_terminal {
                                input_binding.as_ref().and_then(|binding| binding.channel)
                            } else {
                                None
                            },
                            &output_streams,
                            PublicClientCancelReason::ConsumerDrop,
                        );
                        cancel_before_close(&mut session, requests).await;
                        return Err(error);
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(_) => bail!("invocation output writer stopped unexpectedly"),
                }
            }
            request = input_rx.recv(), if accepted && stdin_open => {
                match request {
                    Some(request) => input_buffer.push(request),
                    None => stdin_open = false,
                }
            }
            result = async {
                let (index, request, sequence_offset) = next_unsent_input
                    .as_ref()
                    .expect("wire send selected without buffered input");
                let channel = input_binding
                    .as_ref()
                    .and_then(|binding| binding.channel)
                    .expect("wire send selected without an input channel");
                send_replayable_input(&sender, request, channel, *sequence_offset).await?;
                Ok::<_, SessionTransportError>(*index)
            }, if accepted && next_unsent_input.is_some() && input_binding.as_ref().and_then(|binding| binding.channel).is_some() => {
                match result {
                    Ok(index) => {
                        input_buffer.mark_sent(index)?;
                        input_terminal = input_buffer.is_terminal();
                    }
                    Err(error) => {
                        tokio::select! {
                            biased;
                            _ = interrupt.cancelled() => bail!(PipedExitCode(130)),
                            result = session.recover_after_send_failure(error) => result?,
                        }
                        accepted = false;
                        output_streams.clear();
                        if let Some(binding) = input_binding.as_mut() {
                            binding.channel = None;
                        }
                        continue 'session;
                    }
                }
            }
            frame = session.receive() => {
                let mut frame = frame?;
                let server_frame = frame.frame().clone();
                if let ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. }) =
                    &server_frame
                {
                    accepted = true;
                    output_streams.clear();
                    if let Some(binding) = input_binding.as_mut() {
                        let (channel, stream_token) = session
                            .input_binding(&input_buffer)
                            .ok_or_else(|| anyhow!("invocation acceptance omitted the stdin stream mapping"))?;
                        binding.stream_token = Some(stream_token);
                        binding.channel = Some(channel);
                        if input_buffer.is_terminal() {
                            input_cancelled.cancel();
                            input_rx.close();
                            while input_rx.try_recv().is_ok() {}
                            input_buffer.clear();
                            input_failure_open = false;
                            input_failure_rx.close();
                            stdin_open = false;
                            input_terminal = true;
                        }
                    }
                    if fatal_input_failure.is_some() {
                        break 'session;
                    }
                }
                if matches!(
                    &server_frame,
                    ServerFrame::Message(PublicServerMessage::InvocationRejected { .. })
                )
                {
                    accepted = false;
                }
                if let ServerFrame::Message(PublicServerMessage::InputStreamAck {
                    channel,
                    terminal,
                    ..
                }) = &server_frame {
                    if input_binding.as_ref().and_then(|binding| binding.channel) != Some(*channel) {
                        bail!("received an acknowledgement for an unknown input channel");
                    }
                    input_terminal |= *terminal;
                }
                if response_cancels_input(
                    &server_frame,
                    input_binding.as_ref().and_then(|binding| binding.channel),
                ) {
                    if validate_discarded_input {
                        input_discarded.cancel();
                    } else {
                        input_cancelled.cancel();
                    }
                    input_rx.close();
                    while input_rx.try_recv().is_ok() {}
                    input_buffer.clear();
                    if !validate_discarded_input {
                        input_failure_open = false;
                        input_failure_rx.close();
                    }
                    stdin_open = false;
                    input_terminal = true;
                }
                if matches!(
                    &server_frame,
                    ServerFrame::Message(PublicServerMessage::InvocationFinished { .. })
                ) && output_streams.values().any(|stream| !stream.terminal)
                {
                    bail!("invocation finished before all output streams became terminal");
                }
                match handle_response(
                    server_frame,
                    &args.agent_type.schema,
                    &method.output_schema,
                    &source_language,
                    args.stdout_format,
                    structured,
                    &idempotency_key_value,
                    &session_identity,
                    &bindings,
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
                frame.mark_delivered()?;
                finished = matches!(
                    frame.frame(),
                    ServerFrame::Message(
                        PublicServerMessage::InvocationFinished { .. }
                            | PublicServerMessage::InvocationRejected { .. }
                    )
                );
            }
        }
    }

    let sender = session.sender();
    if let Some(failure) = fatal_input_failure {
        input_cancelled.cancel();
        input_rx.close();
        while input_rx.try_recv().is_ok() {}
        let input_channel = if accepted && !input_terminal {
            input_binding.as_ref().and_then(|binding| binding.channel)
        } else {
            None
        };
        let cancellation_requests =
            cancel_open_streams(input_channel, &output_streams, failure.reason);
        for request in cancellation_requests {
            send_request(&sender, &request, &interrupt).await?;
        }
        if let Some(channel) = input_channel {
            await_input_cancellation(&mut session, channel, &interrupt).await?;
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), sender.close()).await;
        return Err(failure.error);
    }

    let discarded_input_failure = if input_discarded.is_cancelled() && input_failure_open {
        input_failure_open = false;
        (&mut input_failure_rx).await.ok()
    } else {
        None
    };
    input_cancelled.cancel();
    let _ = session.close().await;
    drop(output_tx);
    input_rx.close();
    output_result_rx
        .await
        .map_err(|_| anyhow!("invocation output writer stopped unexpectedly"))??;

    if let Some(failure) = discarded_input_failure {
        return Err(failure.error);
    }
    if input_failure_open && let Ok(failure) = input_failure_rx.try_recv() {
        return Err(failure.error);
    }
    if failed {
        bail!(NonSuccessfulExit);
    }
    Ok(())
}

fn load_checkpoint(path: &Path) -> anyhow::Result<InvocationSessionCheckpoint> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open invocation session file {}", path.display()))?;
    let checkpoint: InvocationSessionCheckpoint = serde_json::from_reader(file)
        .with_context(|| format!("failed to read invocation session file {}", path.display()))?;
    if checkpoint.version != SESSION_CHECKPOINT_VERSION {
        bail!(
            "unsupported invocation session file version {}",
            checkpoint.version
        );
    }
    if checkpoint.protocol_version != INVOCATION_SESSION_VERSION
        || checkpoint.idempotency_key.is_empty()
        || checkpoint.schema_evidence.is_empty()
        || checkpoint.selector.application.is_empty()
        || checkpoint.selector.environment.is_empty()
        || checkpoint.selector.agent_type.is_empty()
        || checkpoint.selector.method.is_empty()
    {
        bail!("invocation session file contains invalid public session metadata");
    }
    if checkpoint
        .delivered_output_cursors
        .iter()
        .any(|(stream, cursor)| stream.is_empty() || cursor.is_empty())
    {
        bail!("invocation session file contains an empty stream or output cursor token");
    }
    let pending_kind = checkpoint
        .pending_operation
        .as_ref()
        .map(|pending| pending_operation_kind(&pending.request))
        .transpose()?;
    if let Some(pending) = &checkpoint.pending_operation {
        golem_common::model::invocation_session_public::encode_text(&pending.request)
            .context("invocation session file contains an invalid pending operation")?;
    }
    if checkpoint.session_token.is_none() && pending_kind != Some(PendingOperationKind::Start) {
        bail!("invocation session file has no session token or pending start operation");
    }
    Ok(checkpoint)
}

fn pending_operation_kind(request: &PublicClientMessage) -> anyhow::Result<PendingOperationKind> {
    match request {
        PublicClientMessage::InvocationStart { .. } => Ok(PendingOperationKind::Start),
        PublicClientMessage::ResumeAttach { operation, .. } => Ok(match operation {
            PublicResumeOperation::Resume => PendingOperationKind::Resume,
            PublicResumeOperation::Takeover => PendingOperationKind::Takeover,
        }),
        _ => bail!("invocation session file contains a non-attachment pending operation"),
    }
}

fn validate_pending_start(
    request: &PublicClientMessage,
    selector: &InvocationSelector,
    config: &[PublicConfigEntry],
    idempotency_key: &str,
    method_parameters: &serde_json::Value,
    input_binding: Option<&mut InputBinding>,
) -> anyhow::Result<()> {
    let PublicClientMessage::InvocationStart {
        attempt_id,
        method_parameters: saved_parameters,
        ..
    } = request
    else {
        bail!("pending operation is not an invocation start");
    };
    let mut expected_parameters = method_parameters.clone();
    if let Some(binding) = input_binding {
        let saved_value = saved_parameters
            .as_object()
            .and_then(|parameters| parameters.get(&binding.parameter_name))
            .cloned()
            .ok_or_else(|| anyhow!("pending invocation start omitted its stdin stream"))?;
        binding.provisional_ref = provisional_ref(&saved_value)?;
        expected_parameters
            .as_object_mut()
            .ok_or_else(|| anyhow!("method parameters are not a public record"))?
            .insert(binding.parameter_name.clone(), saved_value);
    }
    let expected = PublicClientMessage::InvocationStart {
        attempt_id: *attempt_id,
        config: config.to_vec(),
        idempotency_key: idempotency_key.to_string(),
        method_parameters: expected_parameters,
        selector: selector.clone(),
        version: INVOCATION_SESSION_VERSION,
    };
    if &expected != request {
        bail!("saved pending invocation start differs from the requested invocation");
    }
    Ok(())
}

fn provisional_ref(value: &serde_json::Value) -> anyhow::Result<uuid::Uuid> {
    value
        .as_object()
        .and_then(|value| value.get("$stream"))
        .and_then(serde_json::Value::as_object)
        .and_then(|stream| stream.get("provisionalRef"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("pending stdin stream has no provisional reference"))?
        .parse()
        .context("pending stdin stream has an invalid provisional reference")
}

fn validate_pending_resume(
    checkpoint: &InvocationSessionCheckpoint,
    request: &PublicClientMessage,
    kind: PendingOperationKind,
) -> anyhow::Result<()> {
    let PublicClientMessage::ResumeAttach { attempt_id, .. } = request else {
        bail!("pending operation is not a resume attachment");
    };
    if resume_request(checkpoint, kind, *attempt_id)? != *request {
        bail!("saved pending resume differs from its public checkpoint evidence");
    }
    Ok(())
}

fn resume_request(
    checkpoint: &InvocationSessionCheckpoint,
    kind: PendingOperationKind,
    attempt_id: uuid::Uuid,
) -> anyhow::Result<PublicClientMessage> {
    let session_token = checkpoint
        .session_token
        .clone()
        .ok_or_else(|| anyhow!("invocation session has not received a public session token"))?;
    Ok(PublicClientMessage::ResumeAttach {
        attempt_id,
        operation: if kind == PendingOperationKind::Takeover {
            PublicResumeOperation::Takeover
        } else {
            PublicResumeOperation::Resume
        },
        output_cursors: checkpoint
            .delivered_output_cursors
            .values()
            .cloned()
            .collect(),
        session_token,
        version: INVOCATION_SESSION_VERSION,
    })
}

fn write_checkpoint(path: &Path, checkpoint: &InvocationSessionCheckpoint) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary invocation session file in {}",
            parent.display()
        )
    })?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), checkpoint)
        .context("failed to serialize invocation session state")?;
    temporary
        .as_file_mut()
        .write_all(b"\n")
        .context("failed to finish invocation session state")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("failed to flush invocation session state")?;
    temporary.persist(path).map_err(|error| {
        anyhow!(
            "failed to replace invocation session file {}: {}",
            path.display(),
            error.error
        )
    })?;
    std::fs::File::open(parent)
        .with_context(|| format!("failed to open session directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync session directory {}", parent.display()))?;
    Ok(())
}

fn schema_evidence(
    agent_type: &AgentTypeSchema,
    method: &AgentMethodSchema,
) -> anyhow::Result<String> {
    let encoded = serde_json::to_vec(&(&agent_type.schema, &agent_type.constructor, method))?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

fn public_input_graph(
    graph: &SchemaGraph,
    input_schema: &golem_common::schema::agent::InputSchema,
) -> SchemaGraph {
    let fields = input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .map(|field| NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        })
        .collect();
    SchemaGraph {
        defs: graph.defs.clone(),
        root: SchemaType::record(fields),
    }
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
) -> anyhow::Result<(SchemaValue, Option<InputBinding>)> {
    let fields = method
        .input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .collect::<Vec<_>>();
    if fields.len() != arguments.len() {
        bail!(
            "wrong number of parameters: expected {}, got {}",
            fields.len(),
            arguments.len()
        );
    }

    let mut values = Vec::with_capacity(fields.len());
    let mut input_binding = None;
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
            let provisional_ref = uuid::Uuid::new_v4();
            values.push(SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                provisional_ref,
            )));
            input_binding = Some(InputBinding {
                provisional_ref,
                stream_token: None,
                channel: None,
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
            values.push(parsed);
        }
    }

    Ok((SchemaValue::Record { fields: values }, input_binding))
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
    tx: &mpsc::Sender<AdmittedInput>,
    input_buffer: &InputReplayBuffer,
    runtime: &tokio::runtime::Handle,
    cancelled: &CancellationToken,
    discarded: &CancellationToken,
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
                            reason: PublicClientCancelReason::SourceUnavailable,
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
                            reason: PublicClientCancelReason::SourceUnavailable,
                        });
                    }
                };
                let (value, byte_charge) = encode_public_schema_value_with_charge(
                    &graph,
                    &binding.item_type,
                    &value,
                    |_, _| {
                        Err(PublicSchemaValueError::new(
                            golem_common::model::invocation_session_public::PublicErrorCode::UnsupportedValue,
                            "stdin values cannot contain nested streams",
                        ))
                    },
                )
                .map_err(|error| InputFailure {
                    error: error.into(),
                    reason: PublicClientCancelReason::SourceUnavailable,
                })?;
                let request = ReplayableInput::Value {
                    sequence: offset,
                    value,
                };
                if !queue_input(
                    tx,
                    input_buffer,
                    runtime,
                    request,
                    byte_charge,
                    cancelled,
                    discarded,
                )? {
                    if cancelled.is_cancelled() {
                        return Ok(());
                    }
                    if discarded.is_cancelled() {
                        offset = offset.checked_add(1).ok_or_else(|| InputFailure {
                            error: anyhow!("stdin stream offset overflow"),
                            reason: PublicClientCancelReason::SourceUnavailable,
                        })?;
                        continue;
                    }
                    return Err(InputFailure {
                        error: anyhow!("invocation session ended while reading stdin"),
                        reason: PublicClientCancelReason::SourceUnavailable,
                    });
                }
                offset = offset.checked_add(1).ok_or_else(|| InputFailure {
                    error: anyhow!("stdin stream offset overflow"),
                    reason: PublicClientCancelReason::SourceUnavailable,
                })?;
            }
        }
        InvocationStdinFormat::Raw => {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = vec![0_u8; RAW_CHUNK_SIZE];
            loop {
                if cancelled.is_cancelled() || discarded.is_cancelled() {
                    return Ok(());
                }
                let mut count = 0;
                let mut eof = false;
                while count < RAW_CHUNK_SIZE {
                    if cancelled.is_cancelled() || discarded.is_cancelled() {
                        return Ok(());
                    }
                    match stdin.read(&mut buffer[count..]) {
                        Ok(0) => {
                            eof = true;
                            break;
                        }
                        Ok(read) => {
                            count += read;
                            if cancelled.is_cancelled() || discarded.is_cancelled() {
                                return Ok(());
                            }
                        }
                        Err(error) => {
                            return Err(InputFailure {
                                error: anyhow!(
                                    "failed to read raw stdin for parameter '{}': {error}",
                                    binding.parameter_name
                                ),
                                reason: PublicClientCancelReason::SourceUnavailable,
                            });
                        }
                    }
                }
                if count == 0 {
                    break;
                }
                let (kind, item_count) = match binding.raw_kind {
                    Some(RawStreamKind::Binary) => (BinaryMessageKind::InputBinary, 1),
                    Some(RawStreamKind::U8) => (BinaryMessageKind::InputU8, count as u64),
                    None => unreachable!("raw stdin was validated before reading"),
                };
                let request = ReplayableInput::Binary(BinaryMessage {
                    metadata: BinaryMessageMetadata {
                        channel: 1,
                        cursor_token: None,
                        item_count: DecimalU64(item_count),
                        kind,
                        mime_type: None,
                        sequence: DecimalU64(offset),
                        version: INVOCATION_SESSION_VERSION,
                    },
                    payload: buffer[..count].to_vec(),
                });
                if !queue_input(
                    tx,
                    input_buffer,
                    runtime,
                    request,
                    count,
                    cancelled,
                    discarded,
                )? {
                    if cancelled.is_cancelled() || discarded.is_cancelled() {
                        return Ok(());
                    }
                    return Err(InputFailure {
                        error: anyhow!("invocation session ended while reading stdin"),
                        reason: PublicClientCancelReason::SourceUnavailable,
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
                        reason: PublicClientCancelReason::SourceUnavailable,
                    })?;
                if eof {
                    break;
                }
            }
        }
    }
    if cancelled.is_cancelled() || discarded.is_cancelled() {
        return Ok(());
    }
    if !queue_input(
        tx,
        input_buffer,
        runtime,
        ReplayableInput::End { sequence: offset },
        1,
        cancelled,
        discarded,
    )? {
        if cancelled.is_cancelled() || discarded.is_cancelled() {
            return Ok(());
        }
        return Err(InputFailure {
            error: anyhow!("invocation session ended before stdin reached EOF"),
            reason: PublicClientCancelReason::SourceUnavailable,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn queue_input(
    tx: &mpsc::Sender<AdmittedInput>,
    input_buffer: &InputReplayBuffer,
    runtime: &tokio::runtime::Handle,
    request: ReplayableInput,
    byte_charge: usize,
    cancelled: &CancellationToken,
    discarded: &CancellationToken,
) -> Result<bool, InputFailure> {
    let admitted = runtime.block_on(async {
        tokio::select! {
            _ = cancelled.cancelled() => None,
            _ = discarded.cancelled() => None,
            result = input_buffer.admit(request, byte_charge) => Some(result),
        }
    });
    let Some(admitted) = admitted else {
        return Ok(false);
    };
    let admitted = admitted.map_err(|error| InputFailure {
        error: error.into(),
        reason: PublicClientCancelReason::SourceUnavailable,
    })?;
    match tx.blocking_send(admitted) {
        Ok(()) => Ok(true),
        Err(_) if cancelled.is_cancelled() || discarded.is_cancelled() => Ok(false),
        Err(_) => Err(InputFailure {
            error: anyhow!("invocation session ended while reading stdin"),
            reason: PublicClientCancelReason::SourceUnavailable,
        }),
    }
}

fn cancel_open_streams(
    input: Option<u32>,
    output_streams: &HashMap<u32, OutputStream>,
    reason: PublicClientCancelReason,
) -> Vec<PublicClientMessage> {
    let mut requests = Vec::new();
    if let Some(channel) = input {
        requests.push(PublicClientMessage::StreamCancel {
            channel,
            reason,
            version: INVOCATION_SESSION_VERSION,
        });
    }

    let mut open_outputs = output_streams
        .iter()
        .filter_map(|(channel, stream)| (!stream.terminal).then_some(*channel))
        .collect::<Vec<_>>();
    open_outputs.sort_unstable();
    for channel in open_outputs {
        requests.push(PublicClientMessage::StreamCancel {
            channel,
            reason,
            version: INVOCATION_SESSION_VERSION,
        });
    }
    requests
}

async fn send_request(
    sender: &InvocationSessionSender,
    request: &PublicClientMessage,
    interrupt: &CancellationToken,
) -> anyhow::Result<()> {
    tokio::select! {
        biased;
        _ = interrupt.cancelled() => bail!(PipedExitCode(130)),
        result = sender.send_message(request) => result.map_err(Into::into),
    }
}

async fn cancel_before_close(session: &mut InvocationSession, requests: Vec<PublicClientMessage>) {
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        let sender = session.sender();
        for request in requests {
            if sender.send_message(&request).await.is_err() {
                return;
            }
        }
        loop {
            let Ok(mut frame) = session.receive().await else {
                return;
            };
            let terminal = matches!(
                frame.frame(),
                ServerFrame::Message(
                    PublicServerMessage::InvocationFinished { .. }
                        | PublicServerMessage::InvocationRejected { .. }
                        | PublicServerMessage::AttachmentRevoked { .. }
                )
            );
            if frame.mark_delivered().is_err() || terminal {
                return;
            }
        }
    })
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), session.close()).await;
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

fn response_cancels_input(response: &ServerFrame, input_channel: Option<u32>) -> bool {
    let Some(input_channel) = input_channel else {
        return false;
    };
    matches!(
        response,
        ServerFrame::Message(PublicServerMessage::StreamCancel { channel, .. })
            if *channel == input_channel
    )
}

async fn await_input_cancellation(
    session: &mut InvocationSession,
    input_channel: u32,
    interrupt: &CancellationToken,
) -> anyhow::Result<()> {
    loop {
        let mut frame = tokio::select! {
            biased;
            _ = interrupt.cancelled() => bail!(PipedExitCode(130)),
            result = session.receive() => result?,
        };
        let terminal = response_cancels_input(frame.frame(), Some(input_channel))
            || matches!(
                frame.frame(),
                ServerFrame::Message(
                    PublicServerMessage::InvocationFinished { .. }
                        | PublicServerMessage::InvocationRejected { .. }
                        | PublicServerMessage::AttachmentRevoked { .. }
                )
            );
        frame.mark_delivered()?;
        if terminal {
            return Ok(());
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_response(
    response: ServerFrame,
    graph: &SchemaGraph,
    output_schema: &OutputSchema,
    source_language: &SourceLanguage,
    stdout_format: InvocationStdoutFormat,
    structured: bool,
    idempotency_key: &str,
    session_identity: &SessionIdentity,
    bindings: &DeliveryTracker,
    output_streams: &mut HashMap<u32, OutputStream>,
    output_tx: &OutputChannel,
) -> anyhow::Result<bool> {
    let mut failed = false;
    match response {
        ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. }) => {
            if structured {
                let mut event = event(AgentInvocationSessionEventKind::Accepted, idempotency_key);
                event.agent_id = Some(session_identity.agent_id.clone());
                emit(output_tx, OutputJob::Event(event)).await?;
            }
        }
        ServerFrame::Message(PublicServerMessage::InvocationRejected { code, message, .. }) => {
            failed = true;
            let reason = code.as_str().to_string();
            if structured {
                let mut event = event(AgentInvocationSessionEventKind::Rejected, idempotency_key);
                event.agent_id = Some(session_identity.agent_id.clone());
                event.reason = Some(reason);
                event.error = Some(message);
                emit(output_tx, OutputJob::Event(event)).await?;
            } else {
                eprintln!("Invocation rejected ({reason}): {message}");
            }
        }
        ServerFrame::Message(PublicServerMessage::InvocationResult { result, .. }) => {
            match result {
                PublicInvocationResult::Value { value } => {
                    let Some(output_type) = output_schema.schema() else {
                        bail!("session returned a value for a unit-returning method");
                    };
                    let value = decode_output_value(graph, output_type, &value, bindings)?;
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
                        event.agent_id = Some(session_identity.agent_id.clone());
                        event.value = Some(schema_value_to_json(graph, output_type, &value)?);
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
                PublicInvocationResult::None => {
                    if !matches!(output_schema, OutputSchema::Unit) {
                        bail!("session returned no result for a value-returning method");
                    }
                    if structured {
                        let mut event =
                            event(AgentInvocationSessionEventKind::Result, idempotency_key);
                        event.agent_id = Some(session_identity.agent_id.clone());
                        emit(output_tx, OutputJob::Event(event)).await?;
                    } else {
                        emit(output_tx, OutputJob::Text("void".to_string())).await?;
                    }
                }
            }
        }
        ServerFrame::Message(PublicServerMessage::OutputStreamItem {
            channel,
            sequence,
            value,
            ..
        }) => {
            let stream = output_streams
                .get(&channel)
                .ok_or_else(|| anyhow!("output stream {channel} has no schema"))?;
            let value = decode_output_value(graph, &stream.item_type, &value, bindings)?;
            handle_output_item(
                channel,
                sequence.0,
                value,
                graph,
                source_language,
                stdout_format,
                structured,
                idempotency_key,
                output_streams,
                output_tx,
            )
            .await?;
        }
        ServerFrame::Binary(message) => {
            let channel = message.metadata.channel;
            match message.metadata.kind {
                BinaryMessageKind::OutputBinary => {
                    handle_output_item(
                        channel,
                        message.metadata.sequence.0,
                        SchemaValue::Binary(BinaryValuePayload {
                            bytes: message.payload,
                            mime_type: message.metadata.mime_type,
                        }),
                        graph,
                        source_language,
                        stdout_format,
                        structured,
                        idempotency_key,
                        output_streams,
                        output_tx,
                    )
                    .await?;
                }
                BinaryMessageKind::OutputU8 => {
                    for (index, value) in message.payload.into_iter().enumerate() {
                        let sequence = message
                            .metadata
                            .sequence
                            .0
                            .checked_add(index as u64)
                            .ok_or_else(|| anyhow!("output stream sequence overflow"))?;
                        handle_output_item(
                            channel,
                            sequence,
                            SchemaValue::U8(value),
                            graph,
                            source_language,
                            stdout_format,
                            structured,
                            idempotency_key,
                            output_streams,
                            output_tx,
                        )
                        .await?;
                    }
                }
                _ => bail!("server sent a client-to-server binary message kind"),
            }
        }
        ServerFrame::Message(PublicServerMessage::OutputStreamEnd {
            channel,
            sequence,
            outcome,
            ..
        }) => {
            let stream = output_streams
                .get_mut(&channel)
                .ok_or_else(|| anyhow!("output stream {channel} has no schema"))?;
            if stream.terminal {
                bail!("output stream {channel} received a second terminal event");
            }
            if stream
                .next_offset
                .is_some_and(|expected| expected != sequence.0)
            {
                bail!("output stream {channel} terminal sequence is not contiguous");
            }
            stream.terminal = true;
            let (kind, error, reason) = match outcome {
                PublicOutputStreamOutcome::Ok => (AgentInvocationSessionEventKind::End, None, None),
                PublicOutputStreamOutcome::Error { code, message } => {
                    failed = true;
                    (
                        AgentInvocationSessionEventKind::StreamError,
                        Some(message),
                        Some(code.as_str().to_string()),
                    )
                }
                PublicOutputStreamOutcome::Cancelled { reason } => {
                    failed |= server_cancellation_is_failure(reason);
                    (
                        AgentInvocationSessionEventKind::StreamCancel,
                        None,
                        Some(format!("{reason:?}")),
                    )
                }
            };
            if structured {
                let mut event = event(kind, idempotency_key);
                event.stream_id = Some(channel as u64);
                event.parent_stream_id = stream.parent_stream_id.map(u64::from);
                event.path = Some(stream.path.clone());
                event.offset = Some(sequence.0);
                event.error = error;
                event.reason = reason;
                emit(output_tx, OutputJob::Event(event)).await?;
            } else if let Some(error) = error {
                eprintln!("Output stream {channel} failed: {error}");
            }
        }
        ServerFrame::Message(PublicServerMessage::InputStreamAck { .. }) => {}
        ServerFrame::Message(PublicServerMessage::StreamCancel {
            channel, reason, ..
        }) => {
            failed |= server_cancellation_is_failure(reason);
            let output_stream = if let Some(stream) = output_streams.get_mut(&channel) {
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
                event.stream_id = Some(channel as u64);
                if let Some(stream) = output_stream {
                    event.parent_stream_id = stream.parent_stream_id.map(u64::from);
                    event.path = Some(stream.path);
                }
                event.reason = Some(format!("{reason:?}"));
                emit(output_tx, OutputJob::Event(event)).await?;
            } else {
                eprintln!("Stream {channel} was cancelled");
            }
        }
        ServerFrame::Message(PublicServerMessage::AttachmentRevoked { reason, .. }) => {
            bail!("invocation attachment was revoked: {reason:?}");
        }
        ServerFrame::Message(PublicServerMessage::InvocationFinished { outcome, .. }) => {
            let (outcome, error) = match outcome {
                PublicInvocationOutcome::Success => ("success".to_string(), None),
                PublicInvocationOutcome::Failure { code, message } => {
                    failed = true;
                    (code.as_str().to_string(), Some(message))
                }
            };
            if structured {
                let mut event = event(AgentInvocationSessionEventKind::Finished, idempotency_key);
                event.agent_id = Some(session_identity.agent_id.clone());
                event.outcome = Some(outcome);
                event.error = error;
                emit(output_tx, OutputJob::Event(event)).await?;
            } else if let Some(error) = error {
                eprintln!("Invocation failed: {error}");
            }
        }
    }
    Ok(failed)
}

fn server_cancellation_is_failure(reason: PublicServerCancelReason) -> bool {
    matches!(
        reason,
        PublicServerCancelReason::TransportDetached
            | PublicServerCancelReason::SourceUnavailable
            | PublicServerCancelReason::ProducerDeleted
            | PublicServerCancelReason::InvocationFailed
            | PublicServerCancelReason::ProtocolError
    )
}

#[allow(clippy::too_many_arguments)]
async fn handle_output_item(
    channel: u32,
    sequence: u64,
    value: SchemaValue,
    graph: &SchemaGraph,
    source_language: &SourceLanguage,
    stdout_format: InvocationStdoutFormat,
    structured: bool,
    idempotency_key: &str,
    output_streams: &mut HashMap<u32, OutputStream>,
    output_tx: &OutputChannel,
) -> anyhow::Result<()> {
    let stream = output_streams
        .get(&channel)
        .cloned()
        .ok_or_else(|| anyhow!("output stream {channel} has no schema"))?;
    if stream.terminal {
        bail!("output stream {channel} produced an item after its terminal event");
    }
    if stream
        .next_offset
        .is_some_and(|expected| expected != sequence)
    {
        bail!("output stream {channel} item sequence is not contiguous");
    }
    discover_streams(
        graph,
        &stream.item_type,
        &value,
        &format!("{}[{sequence}]", stream.path),
        Some(channel),
        InvocationStdoutFormat::Value,
        output_streams,
    )?;
    let next_offset = sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("output stream offset overflow"))?;
    output_streams
        .get_mut(&channel)
        .expect("output stream disappeared while processing an item")
        .next_offset = Some(next_offset);
    if structured {
        let mut event = event(AgentInvocationSessionEventKind::Item, idempotency_key);
        event.stream_id = Some(channel as u64);
        event.parent_stream_id = stream.parent_stream_id.map(u64::from);
        event.path = Some(stream.path);
        event.offset = Some(sequence);
        event.value = Some(schema_value_to_json(graph, &stream.item_type, &value)?);
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
            let rendered = if path != stream.path || output_streams.len() > 1 || stream.path != "$"
            {
                format!("{path}: {rendered}")
            } else {
                rendered
            };
            emit(output_tx, OutputJob::Text(rendered)).await?;
        }
    }
    Ok(())
}

fn decode_output_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &serde_json::Value,
    bindings: &DeliveryTracker,
) -> anyhow::Result<SchemaValue> {
    decode_public_schema_value(
        graph,
        ty,
        value,
        PublicStreamReferencePolicy::Stable,
        |reference, element_type| {
            let PublicStreamReference::Stable(token) = reference else {
                unreachable!("stable stream policy returned a provisional reference")
            };
            let channel = bindings.channel_for_stream(&token).ok_or_else(|| {
                PublicSchemaValueError::new(
                    golem_common::model::invocation_session_public::PublicErrorCode::InvalidChannel,
                    "stream value has no public channel mapping",
                )
            })?;
            let schema_evidence = serde_json::to_string(&(graph, element_type)).map_err(|_| {
                PublicSchemaValueError::new(
                    golem_common::model::invocation_session_public::PublicErrorCode::ValidationError,
                    "failed to canonicalize stream element schema",
                )
            })?;
            bindings
                .bind_schema(&token, schema_evidence)
                .map_err(|error| {
                    PublicSchemaValueError::new(
                        golem_common::model::invocation_session_public::PublicErrorCode::StreamConflict,
                        error.to_string(),
                    )
                })?;
            Ok(SchemaValueStream::from_host_endpoint(channel))
        },
    )
    .map_err(Into::into)
}

fn event(
    kind: AgentInvocationSessionEventKind,
    idempotency_key: &str,
) -> Box<AgentInvocationSessionEvent> {
    Box::new(AgentInvocationSessionEvent::new(kind, idempotency_key))
}

async fn emit(output: &OutputChannel, job: OutputJob) -> anyhow::Result<()> {
    let (written, completed) = oneshot::channel();
    tokio::select! {
        biased;
        _ = output.input_failed.cancelled() => bail!(InputFailureSignal),
        _ = output.interrupt.cancelled() => bail!(PipedExitCode(130)),
        result = output.tx.send(QueuedOutput { job, written }) => {
            result.map_err(|_| anyhow!("invocation output closed unexpectedly"))?;
        },
    }
    tokio::select! {
        biased;
        _ = output.input_failed.cancelled() => bail!(InputFailureSignal),
        _ = output.interrupt.cancelled() => bail!(PipedExitCode(130)),
        result = completed => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error == "broken-pipe" => bail!(PipedExitCode(0)),
            Ok(Err(error)) => bail!(error),
            Err(_) => bail!("invocation output closed before acknowledging a write"),
        },
    }
}

fn write_output(
    mut rx: mpsc::Receiver<QueuedOutput>,
    format: Format,
    colorize: bool,
) -> anyhow::Result<()> {
    while let Some(QueuedOutput { job, written }) = rx.blocking_recv() {
        let bytes = match render_output_job(job, format, colorize) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = written.send(Err(error.to_string()));
                return Err(error);
            }
        };
        let stdout = std::io::stdout();
        if let Err(error) = write_and_flush(&mut stdout.lock(), &bytes) {
            let diagnostic = if error
                .downcast_ref::<PipedExitCode>()
                .is_some_and(|exit| exit.0 == 0)
            {
                "broken-pipe".to_string()
            } else {
                error.to_string()
            };
            let _ = written.send(Err(diagnostic));
            return Err(error);
        }
        let _ = written.send(Ok(()));
    }
    Ok(())
}

#[cfg(test)]
fn write_output_to<W: Write>(
    mut rx: mpsc::Receiver<QueuedOutput>,
    format: Format,
    colorize: bool,
    output: &mut W,
) -> anyhow::Result<()> {
    while let Some(QueuedOutput { job, written }) = rx.blocking_recv() {
        let bytes = render_output_job(job, format, colorize)?;
        write_and_flush(output, &bytes)?;
        let _ = written.send(Ok(()));
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

fn raw_output_bytes(stream: &OutputStream, value: SchemaValue) -> anyhow::Result<Vec<u8>> {
    match (stream.raw_kind, value) {
        (Some(RawStreamKind::Binary), SchemaValue::Binary(value)) => Ok(value.bytes),
        (Some(RawStreamKind::U8), SchemaValue::U8(value)) => Ok(vec![value]),
        _ => bail!("raw output stream item does not match its declared type"),
    }
}

fn render_text_fragments(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
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
    value: &SchemaValue,
    path: &str,
    source_language: &SourceLanguage,
    fragments: &mut Vec<(String, String)>,
) -> anyhow::Result<()> {
    let ty = graph
        .resolve_ref(ty)
        .map_err(|error| anyhow!(error.to_string()))?;

    if matches!(ty, SchemaType::Stream { .. }) {
        if matches!(value, SchemaValue::Stream(_)) {
            return Ok(());
        }
        bail!("stream value at {path} is not a stream reference");
    }

    if !schema_value_contains_stream(value) {
        fragments.push((
            path.to_string(),
            render_schema_value(graph, ty, value, source_language),
        ));
        return Ok(());
    }

    match (ty, value) {
        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: values }) => {
            if fields.len() != values.len() {
                bail!("record value at {path} has the wrong number of fields");
            }
            for (field, value) in fields.iter().zip(values) {
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
        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: values }) => {
            if elements.len() != values.len() {
                bail!("tuple value at {path} has the wrong number of elements");
            }
            for (index, (ty, value)) in elements.iter().zip(values).enumerate() {
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
        (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
            for (index, value) in elements.iter().enumerate() {
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
        (SchemaType::FixedList { element, .. }, SchemaValue::FixedList { elements }) => {
            for (index, value) in elements.iter().enumerate() {
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
            SchemaValue::Map { entries },
        ) => {
            for (index, (key_value, value)) in entries.iter().enumerate() {
                collect_text_fragments(
                    graph,
                    key,
                    key_value,
                    &format!("{path}[{index}].key"),
                    source_language,
                    fragments,
                )?;
                collect_text_fragments(
                    graph,
                    value_type,
                    value,
                    &format!("{path}[{index}].value"),
                    source_language,
                    fragments,
                )?;
            }
        }
        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: value }) => {
            if let Some(value) = value.as_deref() {
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
        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(variant)) => {
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
        (SchemaType::Result { spec, .. }, SchemaValue::Result(result)) => match result {
            ResultValuePayload::Ok { value: Some(value) } => collect_text_fragments(
                graph,
                spec.ok
                    .as_deref()
                    .ok_or_else(|| anyhow!("unexpected ok payload at {path}"))?,
                value,
                &format!("{path}.ok"),
                source_language,
                fragments,
            )?,
            ResultValuePayload::Err { value: Some(value) } => collect_text_fragments(
                graph,
                spec.err
                    .as_deref()
                    .ok_or_else(|| anyhow!("unexpected err payload at {path}"))?,
                value,
                &format!("{path}.err"),
                source_language,
                fragments,
            )?,
            ResultValuePayload::Ok { value: None } | ResultValuePayload::Err { value: None } => {}
        },
        (SchemaType::Union { spec, .. }, SchemaValue::Union(union)) => {
            let branch = spec
                .branches
                .iter()
                .find(|branch| branch.tag == union.tag)
                .ok_or_else(|| anyhow!("unknown union branch '{}' at {path}", union.tag))?;
            collect_text_fragments(
                graph,
                &branch.body,
                &union.body,
                &format!("{path}.{}", union.tag),
                source_language,
                fragments,
            )?;
        }
        _ => bail!("invocation value at {path} does not match its declared schema"),
    }
    Ok(())
}

fn schema_value_contains_stream(value: &SchemaValue) -> bool {
    match value {
        SchemaValue::Stream(_) => true,
        SchemaValue::Record { fields } => fields.iter().any(schema_value_contains_stream),
        SchemaValue::Variant(variant) => variant
            .payload
            .as_deref()
            .is_some_and(schema_value_contains_stream),
        SchemaValue::Tuple { elements }
        | SchemaValue::List { elements }
        | SchemaValue::FixedList { elements } => elements.iter().any(schema_value_contains_stream),
        SchemaValue::Map { entries } => entries.iter().any(|(key, value)| {
            schema_value_contains_stream(key) || schema_value_contains_stream(value)
        }),
        SchemaValue::Option { inner } => inner.as_deref().is_some_and(schema_value_contains_stream),
        SchemaValue::Result(ResultValuePayload::Ok { value })
        | SchemaValue::Result(ResultValuePayload::Err { value }) => {
            value.as_deref().is_some_and(schema_value_contains_stream)
        }
        SchemaValue::Union(union) => schema_value_contains_stream(&union.body),
        _ => false,
    }
}

fn discover_streams(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    path: &str,
    parent_stream_id: Option<u32>,
    stdout_format: InvocationStdoutFormat,
    output: &mut HashMap<u32, OutputStream>,
) -> anyhow::Result<()> {
    let ty = graph
        .resolve_ref(ty)
        .map_err(|error| anyhow!(error.to_string()))?;
    match (ty, value) {
        (
            SchemaType::Stream {
                inner: Some(inner), ..
            },
            SchemaValue::Stream(reference),
        ) => {
            let channel = reference
                .with_host_endpoint::<u32, _>(|channel| *channel)
                .map_err(anyhow::Error::msg)?;
            let raw_kind = if stdout_format == InvocationStdoutFormat::Raw {
                raw_stream_kind(graph, inner)
            } else {
                None
            };
            if output
                .insert(
                    channel,
                    OutputStream {
                        item_type: (**inner).clone(),
                        parent_stream_id,
                        path: path.to_string(),
                        raw_kind,
                        next_offset: None,
                        terminal: false,
                    },
                )
                .is_some()
            {
                bail!("output stream {channel} was discovered more than once");
            }
        }
        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: values }) => {
            for (field, value) in fields.iter().zip(values) {
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
        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: values }) => {
            for (index, (ty, value)) in elements.iter().zip(values).enumerate() {
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
        (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
            for (index, value) in elements.iter().enumerate() {
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
        (SchemaType::FixedList { element, .. }, SchemaValue::FixedList { elements }) => {
            for (index, value) in elements.iter().enumerate() {
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
            SchemaValue::Map { entries },
        ) => {
            for (index, (key_value, value)) in entries.iter().enumerate() {
                discover_streams(
                    graph,
                    key,
                    key_value,
                    &format!("{path}[{index}].key"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?;
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
        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: Some(value) }) => {
            discover_streams(
                graph,
                inner,
                value,
                &format!("{path}.some"),
                parent_stream_id,
                stdout_format,
                output,
            )?
        }
        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(variant)) => {
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
        (SchemaType::Result { spec, .. }, SchemaValue::Result(result)) => match result {
            ResultValuePayload::Ok { value: Some(value) } if spec.ok.is_some() => discover_streams(
                graph,
                spec.ok.as_deref().unwrap(),
                value,
                &format!("{path}.ok"),
                parent_stream_id,
                stdout_format,
                output,
            )?,
            ResultValuePayload::Err { value: Some(value) } if spec.err.is_some() => {
                discover_streams(
                    graph,
                    spec.err.as_deref().unwrap(),
                    value,
                    &format!("{path}.err"),
                    parent_stream_id,
                    stdout_format,
                    output,
                )?
            }
            _ => {}
        },
        (SchemaType::Union { spec, .. }, SchemaValue::Union(union)) => {
            if let Some(branch) = spec.branches.iter().find(|branch| branch.tag == union.tag) {
                discover_streams(
                    graph,
                    &branch.body,
                    &union.body,
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

fn schema_value_to_json(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> anyhow::Result<serde_json::Value> {
    if !schema_value_contains_stream(value) {
        return golem_common::schema::render::to_json_value(graph, ty, value).map_err(Into::into);
    }

    let ty = graph
        .resolve_ref(ty)
        .map_err(|error| anyhow!(error.to_string()))?;
    Ok(match (ty, value) {
        (SchemaType::Stream { .. }, SchemaValue::Stream(reference)) => {
            let channel = reference
                .with_host_endpoint::<u32, _>(|channel| *channel)
                .map_err(anyhow::Error::msg)?;
            serde_json::json!({ "$stream": channel })
        }
        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: values }) => {
            if fields.len() != values.len() {
                bail!("record result has the wrong number of fields");
            }
            let mut result = serde_json::Map::new();
            for (field, value) in fields.iter().zip(values) {
                result.insert(
                    field.name.clone(),
                    schema_value_to_json(graph, &field.body, value)?,
                );
            }
            serde_json::Value::Object(result)
        }
        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: values }) => {
            if elements.len() != values.len() {
                bail!("tuple result has the wrong number of elements");
            }
            serde_json::Value::Array(
                elements
                    .iter()
                    .zip(values)
                    .map(|(ty, value)| schema_value_to_json(graph, ty, value))
                    .collect::<Result<_, _>>()?,
            )
        }
        (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
            serde_json::Value::Array(
                elements
                    .iter()
                    .map(|value| schema_value_to_json(graph, element, value))
                    .collect::<Result<_, _>>()?,
            )
        }
        (SchemaType::FixedList { element, .. }, SchemaValue::FixedList { elements }) => {
            serde_json::Value::Array(
                elements
                    .iter()
                    .map(|value| schema_value_to_json(graph, element, value))
                    .collect::<Result<_, _>>()?,
            )
        }
        (
            SchemaType::Map {
                key,
                value: value_type,
                ..
            },
            SchemaValue::Map { entries },
        ) => serde_json::Value::Array(
            entries
                .iter()
                .map(|(key_value, value)| {
                    Ok(serde_json::Value::Array(vec![
                        schema_value_to_json(graph, key, key_value)?,
                        schema_value_to_json(graph, value_type, value)?,
                    ]))
                })
                .collect::<anyhow::Result<_>>()?,
        ),
        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: value }) => value
            .as_deref()
            .map(|value| schema_value_to_json(graph, inner, value))
            .transpose()?
            .unwrap_or(serde_json::Value::Null),
        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(variant)) => {
            let case = cases
                .get(variant.case as usize)
                .ok_or_else(|| anyhow!("variant case is out of range"))?;
            match (&case.payload, variant.payload.as_deref()) {
                (None, None) => case.name.clone().into(),
                (Some(ty), Some(value)) => serde_json::json!({
                    case.name.clone(): schema_value_to_json(graph, ty, value)?
                }),
                _ => bail!("variant payload does not match its declared case"),
            }
        }
        (SchemaType::Result { spec, .. }, SchemaValue::Result(result)) => match result {
            ResultValuePayload::Ok { value: Some(value) } => serde_json::json!({
                "ok": schema_value_to_json(
                    graph,
                    spec.ok.as_deref().ok_or_else(|| anyhow!("unexpected ok payload"))?,
                    value,
                )?
            }),
            ResultValuePayload::Err { value: Some(value) } => serde_json::json!({
                "err": schema_value_to_json(
                    graph,
                    spec.err.as_deref().ok_or_else(|| anyhow!("unexpected err payload"))?,
                    value,
                )?
            }),
            ResultValuePayload::Ok { value: None } => serde_json::json!({ "ok": null }),
            ResultValuePayload::Err { value: None } => serde_json::json!({ "err": null }),
        },
        (SchemaType::Union { spec, .. }, SchemaValue::Union(union)) => {
            let branch = spec
                .branches
                .iter()
                .find(|branch| branch.tag == union.tag)
                .ok_or_else(|| anyhow!("unknown union branch '{}'", union.tag))?;
            schema_value_to_json(graph, &branch.body, &union.body)?
        }
        _ => bail!("invocation result does not match its declared schema"),
    })
}

#[cfg(test)]
mod public_tests {
    use super::*;
    use golem_common::model::invocation_session_public::{
        PublicStreamDirection, PublicStreamMapping,
    };
    use golem_common::schema::metadata::MetadataEnvelope;
    use test_r::test;

    fn checkpoint() -> InvocationSessionCheckpoint {
        InvocationSessionCheckpoint {
            version: SESSION_CHECKPOINT_VERSION,
            protocol_version: INVOCATION_SESSION_VERSION,
            schema_evidence: "schema-v1".to_string(),
            selector: InvocationSelector {
                agent_type: "counter".to_string(),
                application: "app".to_string(),
                constructor_parameters: serde_json::json!({}),
                environment: "prod".to_string(),
                method: "run".to_string(),
                phantom_id: None,
            },
            idempotency_key: "invocation-key".to_string(),
            session_token: Some("opaque-session-token".to_string()),
            delivered_output_cursors: BTreeMap::from([(
                "opaque-stream-token".to_string(),
                "opaque-cursor".to_string(),
            )]),
            pending_operation: None,
        }
    }

    #[test]
    fn public_checkpoint_is_atomically_replaced_and_validated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invocation-session.json");
        let mut expected = checkpoint();
        write_checkpoint(&path, &expected).unwrap();
        assert_eq!(load_checkpoint(&path).unwrap().selector, expected.selector);

        expected
            .delivered_output_cursors
            .insert("opaque-stream-token".to_string(), "new-cursor".to_string());
        write_checkpoint(&path, &expected).unwrap();
        assert_eq!(
            load_checkpoint(&path).unwrap().delivered_output_cursors,
            BTreeMap::from([("opaque-stream-token".to_string(), "new-cursor".to_string(),)])
        );
        let encoded = std::fs::read_to_string(path).unwrap();
        for private_name in [
            "attachmentId",
            "epoch",
            "calleeFingerprint",
            "durableStreamId",
            "offset",
        ] {
            assert!(!encoded.contains(private_name));
        }
    }

    #[test]
    fn resume_uses_opaque_public_tokens_and_the_supplied_attempt() {
        let checkpoint = checkpoint();
        let attempt_id = uuid::Uuid::new_v4();
        let request =
            resume_request(&checkpoint, PendingOperationKind::Takeover, attempt_id).unwrap();
        assert_eq!(
            request,
            PublicClientMessage::ResumeAttach {
                attempt_id,
                operation: PublicResumeOperation::Takeover,
                output_cursors: vec!["opaque-cursor".to_string()],
                session_token: "opaque-session-token".to_string(),
                version: INVOCATION_SESSION_VERSION,
            }
        );
    }

    #[test]
    fn pending_start_reuses_its_frozen_attempt_and_provisional_reference() {
        let saved_reference = uuid::Uuid::new_v4();
        let current_reference = uuid::Uuid::new_v4();
        let selector = checkpoint().selector;
        let request = PublicClientMessage::InvocationStart {
            attempt_id: uuid::Uuid::new_v4(),
            config: Vec::new(),
            idempotency_key: "invocation-key".to_string(),
            method_parameters: serde_json::json!({
                "source":{"$stream":{"provisionalRef":saved_reference}}
            }),
            selector: selector.clone(),
            version: INVOCATION_SESSION_VERSION,
        };
        let mut binding = InputBinding {
            provisional_ref: current_reference,
            stream_token: None,
            channel: None,
            parameter_name: "source".to_string(),
            item_type: SchemaType::u8(),
            raw_kind: None,
        };
        validate_pending_start(
            &request,
            &selector,
            &[],
            "invocation-key",
            &serde_json::json!({
                "source":{"$stream":{"provisionalRef":current_reference}}
            }),
            Some(&mut binding),
        )
        .unwrap();
        assert_eq!(binding.provisional_ref, saved_reference);
    }

    #[test]
    fn nested_output_streams_use_public_channels() {
        let graph = SchemaGraph::empty();
        let ty = SchemaType::Record {
            fields: vec![NamedFieldType {
                name: "items".to_string(),
                body: SchemaType::stream(Some(SchemaType::u32())),
                metadata: MetadataEnvelope::default(),
            }],
            metadata: MetadataEnvelope::default(),
        };
        let value = SchemaValue::Record {
            fields: vec![SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                7_u32,
            ))],
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
        assert_eq!(streams[&7].path, "$.items");
        assert_eq!(
            schema_value_to_json(&graph, &ty, &value).unwrap(),
            serde_json::json!({"items":{"$stream":7}})
        );
    }

    #[test]
    fn cancellation_contains_only_public_channel_identity() {
        let streams = HashMap::from([(
            4,
            OutputStream {
                item_type: SchemaType::u8(),
                parent_stream_id: None,
                path: "$".to_string(),
                raw_kind: Some(RawStreamKind::U8),
                next_offset: Some(3),
                terminal: false,
            },
        )]);
        assert_eq!(
            cancel_open_streams(None, &streams, PublicClientCancelReason::ConsumerDrop),
            vec![PublicClientMessage::StreamCancel {
                channel: 4,
                reason: PublicClientCancelReason::ConsumerDrop,
                version: INVOCATION_SESSION_VERSION,
            }]
        );
    }

    #[test]
    fn explicit_and_consumer_drop_cancellation_are_not_cli_failures() {
        assert!(!server_cancellation_is_failure(
            PublicServerCancelReason::Cancelled
        ));
        assert!(!server_cancellation_is_failure(
            PublicServerCancelReason::ConsumerDrop
        ));
        for reason in [
            PublicServerCancelReason::TransportDetached,
            PublicServerCancelReason::SourceUnavailable,
            PublicServerCancelReason::ProducerDeleted,
            PublicServerCancelReason::InvocationFailed,
            PublicServerCancelReason::ProtocolError,
        ] {
            assert!(server_cancellation_is_failure(reason));
        }
    }

    #[test]
    fn public_mapping_rebinding_is_rejected() {
        let bindings = DeliveryTracker::default();
        let mapping = PublicStreamMapping {
            channel: 3,
            direction: PublicStreamDirection::Output,
            input_high_water: None,
            provisional_ref: None,
            stream_token: "stream-one".to_string(),
        };
        bindings.begin_connection(&[mapping.clone()]).unwrap();
        let rebound = PublicStreamMapping {
            channel: 4,
            ..mapping
        };
        assert!(bindings.install_mappings(&[rebound]).is_err());
    }

    #[test]
    async fn terminal_input_acknowledgement_does_not_consume_item_capacity() {
        let buffer = InputReplayBuffer::new(2, 2);
        buffer.push(
            buffer
                .admit(
                    ReplayableInput::Value {
                        sequence: 0,
                        value: serde_json::json!(1),
                    },
                    1,
                )
                .await
                .unwrap(),
        );
        buffer.push(
            buffer
                .admit(ReplayableInput::End { sequence: 1 }, 1)
                .await
                .unwrap(),
        );
        buffer.mark_sent(0).unwrap();
        buffer.mark_sent(1).unwrap();
        buffer.acknowledge(1, false).unwrap();
        assert_eq!(buffer.len(), 1);
        buffer.acknowledge(1, true).unwrap();
        assert!(buffer.is_empty());
    }

    struct BrokenOnFlush;

    impl Write for BrokenOnFlush {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(ErrorKind::BrokenPipe, "closed"))
        }
    }

    #[test]
    fn output_is_not_acknowledged_until_flush_succeeds() {
        let (tx, rx) = mpsc::channel(1);
        let (written, completed) = oneshot::channel();
        assert!(
            tx.try_send(QueuedOutput {
                job: OutputJob::Raw(vec![1]),
                written,
            })
            .is_ok()
        );
        drop(tx);
        let error = write_output_to(rx, Format::Text, false, &mut BrokenOnFlush).unwrap_err();
        assert_eq!(error.downcast_ref::<PipedExitCode>().unwrap().0, 0);
        assert!(completed.blocking_recv().is_err());
    }
}
