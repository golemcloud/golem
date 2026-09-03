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

use async_trait::async_trait;
use futures_util::{SinkExt, Stream, StreamExt};
use golem_common::model::invocation_session_public::{
    BinaryMessage, BinaryMessageKind, BinaryMessageMetadata, DecimalU64,
    INVOCATION_SESSION_SUBPROTOCOL, INVOCATION_SESSION_VERSION, InvocationSelector,
    MAX_LOGICAL_VALUE_SIZE, MAX_PACKED_U8_SIZE, MAX_WEBSOCKET_MESSAGE_SIZE,
    PublicClientCancelReason, PublicClientMessage, PublicConfigEntry, PublicInvocationOutcome,
    PublicInvocationResult, PublicOutputStreamOutcome, PublicResumeOperation, PublicServerMessage,
    PublicStreamDirection, PublicStreamMapping, decode_binary_message, decode_server_text,
    encode_binary_message, encode_text,
};
use golem_common::schema::public_json::{
    PublicSchemaValueError, PublicStreamReference, PublicStreamReferencePolicy,
    decode_public_schema_value, encode_public_schema_value,
};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::validation::value::validate_value;
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Debug, Formatter};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{Semaphore, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
};

const PIPELINE_ITEMS: usize = 16;
const PIPELINE_BYTES: usize = MAX_WEBSOCKET_MESSAGE_SIZE;
const GENERATED_STREAM_ITEMS: usize = 256;
const GENERATED_INPUT_BYTES: usize = MAX_LOGICAL_VALUE_SIZE;
const GENERATED_INPUT_ITEMS: usize = 256;
const PACKED_U8_FLUSH_DELAY: Duration = Duration::from_millis(2);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, thiserror::Error)]
pub enum SessionTransportError {
    #[error("failed to create WebSocket request: {0}")]
    Request(#[source] tungstenite::Error),
    #[error("failed to connect to the invocation session: {0}")]
    Connect(#[source] tungstenite::Error),
    #[error("server did not select {INVOCATION_SESSION_SUBPROTOCOL}")]
    UnsupportedSubprotocol,
    #[error("invocation session connection closed")]
    Closed,
    #[error("invocation session transport failed: {0}")]
    Transport(#[source] tungstenite::Error),
    #[error("failed to prepare invocation session request: {0}")]
    RequestProvider(String),
    #[error("invalid invocation session message: {0}")]
    Protocol(String),
    #[error("failed to persist invocation session state: {0}")]
    StatePersistence(String),
    #[error("delivery cursor sequence overflow")]
    SequenceOverflow,
    #[error("output channel {channel} delivered sequence {actual} after {previous}")]
    DeliveryOrder {
        channel: u32,
        previous: u64,
        actual: u64,
    },
    #[error("output channel {channel} has conflicting cursors at sequence {sequence}")]
    DeliveryConflict { channel: u32, sequence: u64 },
    #[error("server rebound a public stream mapping within one connection")]
    MappingRebound,
    #[error("server rebound stable public stream {stream_token}")]
    StableMappingRebound { stream_token: String },
    #[error("received a mapping with an invalid channel, token, direction, or high-water")]
    InvalidMapping,
    #[error("received an acceptance or rejection for an unexpected attempt")]
    UnexpectedAttempt,
    #[error("detached invocation has no public session token")]
    MissingSessionToken,
    #[error("invocation session attachment is terminal")]
    AttachmentTerminated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentBinary {
    pub bytes: Vec<u8>,
    pub mime_type: Option<String>,
}

/// Complete public descriptor for one generated streaming invocation.
pub struct GeneratedInvocationRequest {
    pub base_url: reqwest::Url,
    pub security: crate::Security,
    pub selector: InvocationSelector,
    pub config: Vec<PublicConfigEntry>,
    pub idempotency_key: String,
    pub input_graph: SchemaGraph,
    pub output_graph: Option<SchemaGraph>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamLane {
    Json,
    U8,
    Binary,
}

#[derive(Clone, Debug)]
struct InputReference(uuid::Uuid);

struct PendingInput {
    provisional_ref: uuid::Uuid,
    buffer: InputReplayBuffer,
    source: Box<dyn GeneratedInputSource>,
}

#[derive(Clone)]
pub struct GeneratedEncodeContext {
    graph: Arc<SchemaGraph>,
    pending: Arc<Mutex<Vec<PendingInput>>>,
}

impl GeneratedEncodeContext {
    pub fn new(graph: SchemaGraph) -> Self {
        Self {
            graph: Arc::new(graph),
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn register_input<T>(
        &self,
        stream: AgentStream<T>,
        item_type: SchemaType,
        encode: impl Fn(T, &GeneratedEncodeContext) -> Result<SchemaValue, String>
        + Send
        + Sync
        + 'static,
    ) -> Result<SchemaValue, String>
    where
        T: Send + 'static,
    {
        let provisional_ref = uuid::Uuid::new_v4();
        let lane = stream_lane(&self.graph, &item_type)?;
        let buffer = InputReplayBuffer::new(GENERATED_INPUT_BYTES, GENERATED_INPUT_ITEMS);
        self.pending
            .lock()
            .expect("generated input registry mutex poisoned")
            .push(PendingInput {
                provisional_ref,
                buffer,
                source: Box::new(TypedGeneratedInputSource {
                    stream,
                    item_type,
                    lane,
                    encode: Arc::new(encode),
                }),
            });
        Ok(SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
            InputReference(provisional_ref),
        )))
    }

    pub fn encode_value(
        &self,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> Result<serde_json::Value, PublicSchemaValueError> {
        encode_public_schema_value(&self.graph, ty, value, |stream, _| {
            stream
                .take_host_endpoint::<InputReference>()
                .map(|reference| PublicStreamReference::Provisional(reference.0))
                .map_err(|message| {
                    PublicSchemaValueError::new(
                        golem_common::model::invocation_session_public::PublicErrorCode::StreamAlreadyConsumed,
                        message,
                    )
                })
        })
    }

    fn take_pending(&self) -> Vec<PendingInput> {
        std::mem::take(
            &mut *self
                .pending
                .lock()
                .expect("generated input registry mutex poisoned"),
        )
    }
}

#[derive(Clone)]
pub struct GeneratedDecodeContext {
    graph: Arc<SchemaGraph>,
    outputs: Arc<Mutex<HashMap<String, Box<dyn GeneratedOutputSink>>>>,
    commands: mpsc::UnboundedSender<GeneratedCommand>,
}

impl GeneratedDecodeContext {
    fn new(graph: SchemaGraph, commands: mpsc::UnboundedSender<GeneratedCommand>) -> Self {
        Self {
            graph: Arc::new(graph),
            outputs: Arc::new(Mutex::new(HashMap::new())),
            commands,
        }
    }

    pub fn register_output<T>(
        &self,
        stream: SchemaValueStream,
        item_type: SchemaType,
        decode: impl Fn(SchemaValue, &GeneratedDecodeContext) -> Result<T, String>
        + Send
        + Sync
        + 'static,
    ) -> Result<AgentStream<T>, String>
    where
        T: Send + 'static,
    {
        let reference = stream.take_host_endpoint::<PublicStreamReference>()?;
        let PublicStreamReference::Stable(stream_token) = reference else {
            return Err("server returned a provisional output stream reference".to_string());
        };
        let lane = stream_lane(&self.graph, &item_type)?;
        let (sender, receiver) = mpsc::channel(GENERATED_STREAM_ITEMS);
        let mut outputs = self
            .outputs
            .lock()
            .expect("generated output registry mutex poisoned");
        if outputs.contains_key(&stream_token) {
            return Err("output stream reference was consumed more than once".to_string());
        }
        outputs.insert(
            stream_token.clone(),
            Box::new(TypedGeneratedOutputSink {
                sender,
                item_type,
                lane,
                decode: Arc::new(decode),
            }),
        );
        drop(outputs);
        let commands = self.commands.clone();
        Ok(AgentStream::output(receiver, move |reason| {
            let _ = commands.send(GeneratedCommand::CancelOutput {
                stream_token,
                reason,
            });
        }))
    }

    fn decode_value(
        &self,
        ty: &SchemaType,
        value: &serde_json::Value,
    ) -> Result<SchemaValue, PublicSchemaValueError> {
        decode_public_schema_value(
            &self.graph,
            ty,
            value,
            PublicStreamReferencePolicy::Stable,
            |reference, _| Ok(SchemaValueStream::from_host_endpoint(reference)),
        )
    }
}

enum GeneratedCommand {
    CancelOutput {
        stream_token: String,
        reason: PublicClientCancelReason,
    },
}

enum GeneratedInputEvent {
    Input {
        provisional_ref: uuid::Uuid,
        input: AdmittedInput,
    },
    Failed {
        provisional_ref: uuid::Uuid,
        message: String,
    },
}

trait GeneratedInputSource: Send {
    fn spawn(
        self: Box<Self>,
        provisional_ref: uuid::Uuid,
        buffer: InputReplayBuffer,
        context: GeneratedEncodeContext,
        events: mpsc::UnboundedSender<GeneratedInputEvent>,
    ) -> tokio::task::AbortHandle;
}

type GeneratedEncoder<T> =
    dyn Fn(T, &GeneratedEncodeContext) -> Result<SchemaValue, String> + Send + Sync;

struct TypedGeneratedInputSource<T> {
    stream: AgentStream<T>,
    item_type: SchemaType,
    lane: StreamLane,
    encode: Arc<GeneratedEncoder<T>>,
}

impl<T> GeneratedInputSource for TypedGeneratedInputSource<T>
where
    T: Send + 'static,
{
    fn spawn(
        self: Box<Self>,
        provisional_ref: uuid::Uuid,
        buffer: InputReplayBuffer,
        context: GeneratedEncodeContext,
        events: mpsc::UnboundedSender<GeneratedInputEvent>,
    ) -> tokio::task::AbortHandle {
        tokio::spawn(run_generated_input_source(
            *self,
            provisional_ref,
            buffer,
            context,
            events,
        ))
        .abort_handle()
    }
}

#[async_trait]
trait GeneratedOutputSink: Send {
    fn item_type(&self) -> &SchemaType;
    async fn value(
        &mut self,
        value: SchemaValue,
        frame: ReceivedFrame,
        context: &GeneratedDecodeContext,
    ) -> Result<(), SessionTransportError>;
    async fn binary(
        &mut self,
        message: BinaryMessage,
        frame: ReceivedFrame,
        context: &GeneratedDecodeContext,
    ) -> Result<(), SessionTransportError>;
    async fn finish(
        &mut self,
        outcome: PublicOutputStreamOutcome,
        frame: Option<ReceivedFrame>,
    ) -> Result<(), SessionTransportError>;
}

type GeneratedDecoder<T> =
    dyn Fn(SchemaValue, &GeneratedDecodeContext) -> Result<T, String> + Send + Sync;

struct TypedGeneratedOutputSink<T> {
    sender: mpsc::Sender<AgentStreamOutput<T>>,
    item_type: SchemaType,
    lane: StreamLane,
    decode: Arc<GeneratedDecoder<T>>,
}

#[async_trait]
impl<T> GeneratedOutputSink for TypedGeneratedOutputSink<T>
where
    T: Send + 'static,
{
    fn item_type(&self) -> &SchemaType {
        &self.item_type
    }

    async fn value(
        &mut self,
        value: SchemaValue,
        frame: ReceivedFrame,
        context: &GeneratedDecodeContext,
    ) -> Result<(), SessionTransportError> {
        if self.lane != StreamLane::Json {
            return Err(SessionTransportError::Protocol(
                "binary output stream item arrived on the JSON lane".to_string(),
            ));
        }
        let value = (self.decode)(value, context).map_err(SessionTransportError::Protocol)?;
        self.sender
            .send(AgentStreamOutput::Item(value, Some(frame)))
            .await
            .map_err(|_| SessionTransportError::Closed)
    }

    async fn binary(
        &mut self,
        message: BinaryMessage,
        frame: ReceivedFrame,
        context: &GeneratedDecodeContext,
    ) -> Result<(), SessionTransportError> {
        match self.lane {
            StreamLane::Json => Err(SessionTransportError::Protocol(
                "JSON output stream item arrived on the binary lane".to_string(),
            )),
            StreamLane::U8 => {
                if message.metadata.kind != BinaryMessageKind::OutputU8 {
                    return Err(SessionTransportError::Protocol(
                        "output stream used the wrong binary kind".to_string(),
                    ));
                }
                let mut frame = Some(frame);
                let mut values = message.payload.into_iter().peekable();
                while let Some(byte) = values.next() {
                    let schema_value = SchemaValue::U8(byte);
                    validate_value(&context.graph, &self.item_type, &schema_value).map_err(
                        |errors| {
                            SessionTransportError::Protocol(format!(
                                "binary output failed schema validation: {errors:?}"
                            ))
                        },
                    )?;
                    let value = (self.decode)(schema_value, context)
                        .map_err(SessionTransportError::Protocol)?;
                    let delivery = if values.peek().is_none() {
                        frame.take()
                    } else {
                        None
                    };
                    self.sender
                        .send(AgentStreamOutput::Item(value, delivery))
                        .await
                        .map_err(|_| SessionTransportError::Closed)?;
                }
                Ok(())
            }
            StreamLane::Binary => {
                if message.metadata.kind != BinaryMessageKind::OutputBinary {
                    return Err(SessionTransportError::Protocol(
                        "output stream used the wrong binary kind".to_string(),
                    ));
                }
                let value = SchemaValue::Binary(golem_common::schema::BinaryValuePayload {
                    bytes: message.payload,
                    mime_type: message.metadata.mime_type,
                });
                validate_value(&context.graph, &self.item_type, &value).map_err(|errors| {
                    SessionTransportError::Protocol(format!(
                        "binary output failed schema validation: {errors:?}"
                    ))
                })?;
                let value =
                    (self.decode)(value, context).map_err(SessionTransportError::Protocol)?;
                self.sender
                    .send(AgentStreamOutput::Item(value, Some(frame)))
                    .await
                    .map_err(|_| SessionTransportError::Closed)
            }
        }
    }

    async fn finish(
        &mut self,
        outcome: PublicOutputStreamOutcome,
        frame: Option<ReceivedFrame>,
    ) -> Result<(), SessionTransportError> {
        let output = match outcome {
            PublicOutputStreamOutcome::Ok => AgentStreamOutput::End(frame),
            PublicOutputStreamOutcome::Error { message, .. } => {
                AgentStreamOutput::Error(AgentStreamError::Producer(message), frame)
            }
            PublicOutputStreamOutcome::Cancelled { reason } => {
                AgentStreamOutput::Error(AgentStreamError::Cancelled(format!("{reason:?}")), frame)
            }
        };
        self.sender
            .send(output)
            .await
            .map_err(|_| SessionTransportError::Closed)
    }
}

#[derive(Clone, Debug)]
pub enum ReplayableInput {
    Value {
        sequence: u64,
        value: serde_json::Value,
    },
    Binary(BinaryMessage),
    End {
        sequence: u64,
    },
}

pub struct AdmittedInput {
    request: ReplayableInput,
    sent: bool,
    _byte_budget: tokio::sync::OwnedSemaphorePermit,
    _item_budget: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Clone)]
pub struct InputReplayBuffer {
    state: Arc<Mutex<InputReplayState>>,
    byte_budget: Arc<Semaphore>,
    item_budget: Arc<Semaphore>,
    max_unacknowledged_bytes: usize,
}

struct InputReplayState {
    acknowledged_sequence: u64,
    queue: VecDeque<AdmittedInput>,
    sequence_offset: u64,
    sequence_offset_initialized: bool,
    terminal: bool,
}

impl InputReplayBuffer {
    pub fn new(max_unacknowledged_bytes: usize, max_unacknowledged_items: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(InputReplayState {
                acknowledged_sequence: 0,
                queue: VecDeque::new(),
                sequence_offset: 0,
                sequence_offset_initialized: false,
                terminal: false,
            })),
            byte_budget: Arc::new(Semaphore::new(max_unacknowledged_bytes)),
            item_budget: Arc::new(Semaphore::new(max_unacknowledged_items)),
            max_unacknowledged_bytes,
        }
    }

    pub async fn admit(
        &self,
        request: ReplayableInput,
        byte_charge: usize,
    ) -> Result<AdmittedInput, SessionTransportError> {
        let byte_charge = byte_charge.max(1);
        if byte_charge > self.max_unacknowledged_bytes {
            return Err(SessionTransportError::Protocol(format!(
                "one input item exceeds the {}-byte unacknowledged input budget",
                self.max_unacknowledged_bytes
            )));
        }
        let byte_budget = self
            .byte_budget
            .clone()
            .acquire_many_owned(byte_charge as u32)
            .await
            .map_err(|_| SessionTransportError::Closed)?;
        let item_budget = self
            .item_budget
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| SessionTransportError::Closed)?;
        Ok(AdmittedInput {
            request,
            sent: false,
            _byte_budget: byte_budget,
            _item_budget: item_budget,
        })
    }

    pub fn push(&self, input: AdmittedInput) {
        self.state
            .lock()
            .expect("input replay buffer mutex poisoned")
            .queue
            .push_back(input);
    }

    pub fn next_unsent(&self) -> Option<(usize, ReplayableInput, u64)> {
        let state = self
            .state
            .lock()
            .expect("input replay buffer mutex poisoned");
        state
            .queue
            .iter()
            .enumerate()
            .find(|(_, input)| !input.sent)
            .map(|(index, input)| (index, input.request.clone(), state.sequence_offset))
    }

    pub fn mark_sent(&self, index: usize) -> Result<(), SessionTransportError> {
        let mut state = self
            .state
            .lock()
            .expect("input replay buffer mutex poisoned");
        let input = state.queue.get_mut(index).ok_or_else(|| {
            SessionTransportError::Protocol(
                "buffered input disappeared during wire send".to_string(),
            )
        })?;
        input.sent = true;
        if matches!(input.request, ReplayableInput::End { .. }) {
            state.terminal = true;
        }
        Ok(())
    }

    pub fn initialize_high_water(
        &self,
        highest_contiguous_sequence: u64,
        terminal: bool,
    ) -> Result<(), SessionTransportError> {
        let mut state = self
            .state
            .lock()
            .expect("input replay buffer mutex poisoned");
        if !state.sequence_offset_initialized {
            state.sequence_offset = highest_contiguous_sequence;
            state.acknowledged_sequence = highest_contiguous_sequence;
            state.sequence_offset_initialized = true;
        }
        acknowledge_input_state(&mut state, highest_contiguous_sequence, terminal)?;
        if terminal {
            state.terminal = true;
            state.queue.clear();
        } else {
            state.terminal = false;
            for input in &mut state.queue {
                input.sent = false;
            }
        }
        Ok(())
    }

    pub fn acknowledge(
        &self,
        highest_contiguous_sequence: u64,
        terminal: bool,
    ) -> Result<(), SessionTransportError> {
        acknowledge_input_state(
            &mut self
                .state
                .lock()
                .expect("input replay buffer mutex poisoned"),
            highest_contiguous_sequence,
            terminal,
        )
    }

    pub fn clear(&self) {
        self.state
            .lock()
            .expect("input replay buffer mutex poisoned")
            .queue
            .clear();
    }

    pub fn is_empty(&self) -> bool {
        self.state
            .lock()
            .expect("input replay buffer mutex poisoned")
            .queue
            .is_empty()
    }

    pub fn len(&self) -> usize {
        self.state
            .lock()
            .expect("input replay buffer mutex poisoned")
            .queue
            .len()
    }

    pub fn is_terminal(&self) -> bool {
        self.state
            .lock()
            .expect("input replay buffer mutex poisoned")
            .terminal
    }
}

fn acknowledge_input_state(
    state: &mut InputReplayState,
    highest_contiguous_sequence: u64,
    terminal: bool,
) -> Result<(), SessionTransportError> {
    if highest_contiguous_sequence < state.acknowledged_sequence {
        return Err(SessionTransportError::Protocol(
            "input acknowledgement moved the durable high-water backwards".to_string(),
        ));
    }
    let maximum_sent = state.queue.iter().filter(|input| input.sent).try_fold(
        state.acknowledged_sequence,
        |maximum, input| {
            Ok::<_, SessionTransportError>(
                maximum.max(input.request.end_sequence(state.sequence_offset)?),
            )
        },
    )?;
    if highest_contiguous_sequence > maximum_sent {
        return Err(SessionTransportError::Protocol(
            "input acknowledgement advanced beyond data sent by this client".to_string(),
        ));
    }
    while let Some(input) = state.queue.front() {
        let start = input.request.start_sequence(state.sequence_offset)?;
        let end = input.request.end_sequence(state.sequence_offset)?;
        if matches!(&input.request, ReplayableInput::End { .. }) {
            if terminal && end == highest_contiguous_sequence {
                state.queue.pop_front();
            }
            break;
        }
        if end <= highest_contiguous_sequence {
            state.queue.pop_front();
            continue;
        }
        if start < highest_contiguous_sequence {
            return Err(SessionTransportError::Protocol(
                "input acknowledgement split one packed input batch".to_string(),
            ));
        }
        break;
    }
    state.acknowledged_sequence = highest_contiguous_sequence;
    state.terminal |= terminal;
    Ok(())
}

impl ReplayableInput {
    fn start_sequence(&self, sequence_offset: u64) -> Result<u64, SessionTransportError> {
        let sequence = match self {
            Self::Value { sequence, .. } | Self::End { sequence } => *sequence,
            Self::Binary(message) => message.metadata.sequence.0,
        };
        sequence
            .checked_add(sequence_offset)
            .ok_or(SessionTransportError::SequenceOverflow)
    }

    fn end_sequence(&self, sequence_offset: u64) -> Result<u64, SessionTransportError> {
        let start = self.start_sequence(sequence_offset)?;
        let count = match self {
            Self::Value { .. } => 1,
            Self::Binary(message) => message.metadata.item_count.0,
            Self::End { .. } => 0,
        };
        start
            .checked_add(count)
            .ok_or(SessionTransportError::SequenceOverflow)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerFrame {
    Message(PublicServerMessage),
    Binary(BinaryMessage),
}

#[derive(Clone, Debug, PartialEq)]
pub struct InvocationSessionStateSnapshot {
    pub delivered_output_cursors: BTreeMap<String, String>,
    pub pending_operation: Option<PublicClientMessage>,
    pub session_token: Option<String>,
}

pub trait InvocationSessionStateObserver: Send + Sync {
    fn state_changed(&self, state: &InvocationSessionStateSnapshot) -> Result<(), String>;
}

impl InvocationSessionStateObserver for () {
    fn state_changed(&self, _state: &InvocationSessionStateSnapshot) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct DeliveryTracker {
    shared: Arc<SharedSessionState>,
}

struct SharedSessionState {
    state: Mutex<SessionState>,
    observer: Arc<dyn InvocationSessionStateObserver>,
}

struct SessionState {
    connection_channels: BTreeMap<u32, ConnectionBinding>,
    stable_streams: BTreeMap<String, StableStreamBinding>,
    pending_operation: Option<PublicClientMessage>,
    session_token: Option<String>,
}

#[derive(Clone)]
struct ConnectionBinding {
    direction: PublicStreamDirection,
    provisional_ref: Option<uuid::Uuid>,
    stream_token: String,
}

struct StableStreamBinding {
    direction: PublicStreamDirection,
    provisional_ref: Option<uuid::Uuid>,
    schema_evidence: Option<String>,
    delivered: Option<DeliveredCursor>,
}

struct DeliveredCursor {
    sequence: Option<u64>,
    token: String,
}

impl Default for DeliveryTracker {
    fn default() -> Self {
        Self::new(
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: None,
                session_token: None,
            },
            Arc::new(()),
        )
    }
}

impl DeliveryTracker {
    fn new(
        initial: InvocationSessionStateSnapshot,
        observer: Arc<dyn InvocationSessionStateObserver>,
    ) -> Self {
        let stable_streams = initial
            .delivered_output_cursors
            .iter()
            .map(|(stream_token, cursor)| {
                (
                    stream_token.clone(),
                    StableStreamBinding {
                        direction: PublicStreamDirection::Output,
                        provisional_ref: None,
                        schema_evidence: None,
                        delivered: Some(DeliveredCursor {
                            sequence: None,
                            token: cursor.clone(),
                        }),
                    },
                )
            })
            .collect();
        Self {
            shared: Arc::new(SharedSessionState {
                state: Mutex::new(SessionState {
                    connection_channels: BTreeMap::new(),
                    stable_streams,
                    pending_operation: initial.pending_operation,
                    session_token: initial.session_token,
                }),
                observer,
            }),
        }
    }

    pub fn snapshot(&self) -> InvocationSessionStateSnapshot {
        snapshot(
            &self
                .shared
                .state
                .lock()
                .expect("invocation session state mutex poisoned"),
        )
    }

    pub fn delivered_cursor_tokens(&self) -> Vec<String> {
        self.snapshot()
            .delivered_output_cursors
            .into_values()
            .collect()
    }

    pub fn begin_connection(
        &self,
        mappings: &[PublicStreamMapping],
    ) -> Result<(), SessionTransportError> {
        self.shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned")
            .connection_channels
            .clear();
        self.install_mappings(mappings)
    }

    pub fn install_mappings(
        &self,
        mappings: &[PublicStreamMapping],
    ) -> Result<(), SessionTransportError> {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned");
        for mapping in mappings {
            if mapping.channel == 0
                || mapping.stream_token.is_empty()
                || (mapping.direction == PublicStreamDirection::Input)
                    != mapping.input_high_water.is_some()
            {
                return Err(SessionTransportError::InvalidMapping);
            }
            if let Some(existing) = state.connection_channels.get(&mapping.channel)
                && (existing.stream_token != mapping.stream_token
                    || existing.direction != mapping.direction
                    || existing.provisional_ref != mapping.provisional_ref)
            {
                return Err(SessionTransportError::MappingRebound);
            }
            if state.connection_channels.iter().any(|(channel, existing)| {
                *channel != mapping.channel
                    && (existing.stream_token == mapping.stream_token
                        || mapping.provisional_ref.is_some()
                            && existing.provisional_ref == mapping.provisional_ref)
            }) {
                return Err(SessionTransportError::MappingRebound);
            }
            if let Some(existing) = state.stable_streams.get(&mapping.stream_token) {
                if existing.direction != mapping.direction
                    || existing.provisional_ref.is_some()
                        && mapping.provisional_ref.is_some()
                        && existing.provisional_ref != mapping.provisional_ref
                {
                    return Err(SessionTransportError::StableMappingRebound {
                        stream_token: mapping.stream_token.clone(),
                    });
                }
            } else {
                state.stable_streams.insert(
                    mapping.stream_token.clone(),
                    StableStreamBinding {
                        direction: mapping.direction,
                        provisional_ref: mapping.provisional_ref,
                        schema_evidence: None,
                        delivered: None,
                    },
                );
            }
            state.connection_channels.insert(
                mapping.channel,
                ConnectionBinding {
                    direction: mapping.direction,
                    provisional_ref: mapping.provisional_ref,
                    stream_token: mapping.stream_token.clone(),
                },
            );
        }
        Ok(())
    }

    pub fn channel_for_stream(&self, stream_token: &str) -> Option<u32> {
        self.shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned")
            .connection_channels
            .iter()
            .find_map(|(channel, binding)| {
                (binding.stream_token == stream_token).then_some(*channel)
            })
    }

    pub fn stream_for_channel(&self, channel: u32) -> Option<String> {
        self.shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned")
            .connection_channels
            .get(&channel)
            .map(|binding| binding.stream_token.clone())
    }

    pub fn direction_for_channel(&self, channel: u32) -> Option<PublicStreamDirection> {
        self.shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned")
            .connection_channels
            .get(&channel)
            .map(|binding| binding.direction)
    }

    pub fn bind_schema(
        &self,
        stream_token: &str,
        schema_evidence: String,
    ) -> Result<(), SessionTransportError> {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned");
        let binding = state.stable_streams.get_mut(stream_token).ok_or_else(|| {
            SessionTransportError::Protocol(
                "cannot bind schema to an unknown public stream token".to_string(),
            )
        })?;
        if binding
            .schema_evidence
            .as_ref()
            .is_some_and(|existing| existing != &schema_evidence)
        {
            return Err(SessionTransportError::StableMappingRebound {
                stream_token: stream_token.to_string(),
            });
        }
        binding.schema_evidence = Some(schema_evidence);
        Ok(())
    }

    fn delivery(&self, frame: &ServerFrame) -> Result<Option<Delivery>, SessionTransportError> {
        let cursor = match frame {
            ServerFrame::Message(PublicServerMessage::OutputStreamItem {
                channel,
                cursor_token,
                sequence,
                ..
            }) => Some((*channel, sequence.0, cursor_token.clone())),
            ServerFrame::Message(PublicServerMessage::OutputStreamEnd {
                channel,
                cursor_token: Some(cursor_token),
                sequence,
                ..
            }) => Some((*channel, sequence.0, cursor_token.clone())),
            ServerFrame::Binary(BinaryMessage { metadata, .. })
                if matches!(
                    metadata.kind,
                    BinaryMessageKind::OutputU8 | BinaryMessageKind::OutputBinary
                ) && metadata.cursor_token.is_some() =>
            {
                let sequence = metadata
                    .sequence
                    .0
                    .checked_add(metadata.item_count.0.saturating_sub(1))
                    .ok_or(SessionTransportError::SequenceOverflow)?;
                Some((
                    metadata.channel,
                    sequence,
                    metadata.cursor_token.clone().unwrap(),
                ))
            }
            _ => None,
        };
        cursor
            .map(|(channel, sequence, token)| {
                let state = self
                    .shared
                    .state
                    .lock()
                    .expect("invocation session state mutex poisoned");
                let stream_token = state
                    .connection_channels
                    .get(&channel)
                    .filter(|binding| binding.direction == PublicStreamDirection::Output)
                    .map(|binding| binding.stream_token.clone())
                    .ok_or_else(|| {
                        SessionTransportError::Protocol(format!(
                            "output channel {channel} has no current public stream mapping"
                        ))
                    })?;
                Ok(Delivery {
                    tracker: self.clone(),
                    channel,
                    stream_token,
                    sequence,
                    token: Some(token),
                })
            })
            .transpose()
    }

    fn set_attachment_state(
        &self,
        pending_operation: Option<PublicClientMessage>,
        session_token: Option<String>,
    ) -> Result<(), SessionTransportError> {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned");
        let mut changed = snapshot(&state);
        changed.pending_operation.clone_from(&pending_operation);
        if session_token.is_some() {
            changed.session_token.clone_from(&session_token);
        }
        self.shared
            .observer
            .state_changed(&changed)
            .map_err(SessionTransportError::StatePersistence)?;
        state.pending_operation = pending_operation;
        if session_token.is_some() {
            state.session_token = session_token;
        }
        Ok(())
    }
}

fn snapshot(state: &SessionState) -> InvocationSessionStateSnapshot {
    InvocationSessionStateSnapshot {
        delivered_output_cursors: state
            .stable_streams
            .iter()
            .filter_map(|(stream, binding)| {
                binding
                    .delivered
                    .as_ref()
                    .map(|cursor| (stream.clone(), cursor.token.clone()))
            })
            .collect(),
        pending_operation: state.pending_operation.clone(),
        session_token: state.session_token.clone(),
    }
}

impl Debug for DeliveryTracker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeliveryTracker")
            .field("cursor_count", &self.delivered_cursor_tokens().len())
            .finish()
    }
}

struct Delivery {
    tracker: DeliveryTracker,
    channel: u32,
    stream_token: String,
    sequence: u64,
    token: Option<String>,
}

impl Delivery {
    fn mark(&mut self) -> Result<bool, SessionTransportError> {
        let Some(token) = self.token.clone() else {
            return Ok(false);
        };
        let mut state = self
            .tracker
            .shared
            .state
            .lock()
            .expect("invocation session state mutex poisoned");
        let binding = state
            .stable_streams
            .get(&self.stream_token)
            .ok_or_else(|| {
                SessionTransportError::Protocol(
                    "delivered output has no stable public stream mapping".to_string(),
                )
            })?;
        if let Some(previous) = &binding.delivered
            && let Some(previous_sequence) = previous.sequence
        {
            if self.sequence < previous_sequence {
                return Err(SessionTransportError::DeliveryOrder {
                    channel: self.channel,
                    previous: previous_sequence,
                    actual: self.sequence,
                });
            }
            if self.sequence == previous_sequence {
                return if token == previous.token {
                    self.token = None;
                    Ok(false)
                } else {
                    Err(SessionTransportError::DeliveryConflict {
                        channel: self.channel,
                        sequence: self.sequence,
                    })
                };
            }
        }
        let mut changed = snapshot(&state);
        changed
            .delivered_output_cursors
            .insert(self.stream_token.clone(), token.clone());
        self.tracker
            .shared
            .observer
            .state_changed(&changed)
            .map_err(SessionTransportError::StatePersistence)?;
        let binding = state
            .stable_streams
            .get_mut(&self.stream_token)
            .expect("connection mapping has no stable stream binding");
        binding.delivered = Some(DeliveredCursor {
            sequence: Some(self.sequence),
            token,
        });
        self.token = None;
        Ok(true)
    }
}

pub struct ReceivedFrame {
    frame: ServerFrame,
    delivery: Option<Delivery>,
    _admission: tokio::sync::OwnedSemaphorePermit,
}

impl ReceivedFrame {
    pub fn frame(&self) -> &ServerFrame {
        &self.frame
    }

    fn attach_delivery(&mut self, tracker: &DeliveryTracker) -> Result<(), SessionTransportError> {
        self.delivery = tracker.delivery(&self.frame)?;
        Ok(())
    }

    /// Marks the output represented by this frame as delivered to the caller.
    /// Until this is called, its cursor is excluded from resume checkpoints.
    pub fn mark_delivered(&mut self) -> Result<bool, SessionTransportError> {
        self.delivery
            .as_mut()
            .map(Delivery::mark)
            .transpose()
            .map(Option::unwrap_or_default)
    }
}

enum OutboundFrame {
    Message(Message, tokio::sync::OwnedSemaphorePermit),
    Close,
}

#[derive(Clone)]
pub struct InvocationSessionSender {
    outbound: mpsc::Sender<OutboundFrame>,
    outbound_bytes: Arc<Semaphore>,
}

impl InvocationSessionSender {
    pub async fn send_message(
        &self,
        message: &PublicClientMessage,
    ) -> Result<(), SessionTransportError> {
        let text = encode_text(message)
            .map_err(|error| SessionTransportError::Protocol(error.to_string()))?;
        let len = text.len();
        self.send_wire(Message::Text(text.into()), len).await
    }

    pub async fn send_binary(&self, message: &BinaryMessage) -> Result<(), SessionTransportError> {
        let bytes = encode_binary_message(message)
            .map_err(|error| SessionTransportError::Protocol(error.to_string()))?;
        let len = bytes.len();
        self.send_wire(Message::Binary(bytes.into()), len).await
    }

    async fn send_wire(&self, message: Message, size: usize) -> Result<(), SessionTransportError> {
        let permit = self
            .outbound_bytes
            .clone()
            .acquire_many_owned(size.max(1) as u32)
            .await
            .map_err(|_| SessionTransportError::Closed)?;
        self.outbound
            .send(OutboundFrame::Message(message, permit))
            .await
            .map_err(|_| SessionTransportError::Closed)
    }

    pub async fn close(&self) -> Result<(), SessionTransportError> {
        self.outbound
            .send(OutboundFrame::Close)
            .await
            .map_err(|_| SessionTransportError::Closed)
    }
}

pub struct InvocationSessionTransport {
    sender: InvocationSessionSender,
    inbound: mpsc::Receiver<Result<ReceivedFrame, SessionTransportError>>,
    deliveries: DeliveryTracker,
}

impl InvocationSessionTransport {
    pub async fn connect(
        request: impl IntoClientRequest,
        connector: Option<Connector>,
    ) -> Result<Self, SessionTransportError> {
        Self::connect_with_tracker(request, connector, DeliveryTracker::default()).await
    }

    async fn connect_with_tracker(
        request: impl IntoClientRequest,
        connector: Option<Connector>,
        deliveries: DeliveryTracker,
    ) -> Result<Self, SessionTransportError> {
        let mut request = request
            .into_client_request()
            .map_err(SessionTransportError::Request)?;
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            INVOCATION_SESSION_SUBPROTOCOL
                .parse()
                .expect("static subprotocol is a valid header"),
        );
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_WEBSOCKET_MESSAGE_SIZE))
            .max_frame_size(Some(MAX_WEBSOCKET_MESSAGE_SIZE));
        let (socket, response) =
            connect_async_tls_with_config(request, Some(config), false, connector)
                .await
                .map_err(|error| match error {
                    tungstenite::Error::Protocol(
                        tungstenite::error::ProtocolError::SecWebSocketSubProtocolError(_),
                    ) => SessionTransportError::UnsupportedSubprotocol,
                    error => SessionTransportError::Connect(error),
                })?;
        if response
            .headers()
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            != Some(INVOCATION_SESSION_SUBPROTOCOL)
        {
            return Err(SessionTransportError::UnsupportedSubprotocol);
        }

        Ok(Self::from_socket(socket, deliveries))
    }

    fn from_socket(socket: Socket, deliveries: DeliveryTracker) -> Self {
        let (outbound_tx, outbound_rx) = mpsc::channel(PIPELINE_ITEMS);
        let (inbound_tx, inbound_rx) = mpsc::channel(PIPELINE_ITEMS);
        let outbound_bytes = Arc::new(Semaphore::new(PIPELINE_BYTES));
        let inbound_bytes = Arc::new(Semaphore::new(PIPELINE_BYTES));
        tokio::spawn(run_socket(socket, outbound_rx, inbound_tx, inbound_bytes));
        Self {
            sender: InvocationSessionSender {
                outbound: outbound_tx,
                outbound_bytes,
            },
            inbound: inbound_rx,
            deliveries,
        }
    }

    pub fn sender(&self) -> InvocationSessionSender {
        self.sender.clone()
    }

    pub fn delivery_tracker(&self) -> DeliveryTracker {
        self.deliveries.clone()
    }

    pub async fn send_message(
        &self,
        message: &PublicClientMessage,
    ) -> Result<(), SessionTransportError> {
        self.sender.send_message(message).await
    }

    pub async fn send_binary(&self, message: &BinaryMessage) -> Result<(), SessionTransportError> {
        self.sender.send_binary(message).await
    }

    pub async fn receive(&mut self) -> Result<ReceivedFrame, SessionTransportError> {
        self.inbound
            .recv()
            .await
            .ok_or(SessionTransportError::Closed)?
    }

    pub async fn close(&self) -> Result<(), SessionTransportError> {
        self.sender.close().await
    }
}

#[async_trait]
pub trait InvocationSessionRequestProvider: Send + Sync {
    async fn request(&self) -> Result<tungstenite::http::Request<()>, SessionTransportError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSendOutcome {
    Sent,
    Reconnected,
}

pub struct InvocationSession {
    transport: InvocationSessionTransport,
    request_provider: Arc<dyn InvocationSessionRequestProvider>,
    connector: Option<Connector>,
    deliveries: DeliveryTracker,
    inputs: Vec<RegisteredInput>,
    attachment_state: AttachmentState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttachmentState {
    Pending {
        exact_retry: bool,
    },
    Accepted {
        attempt_id: uuid::Uuid,
        replay_revocation_eligible: bool,
    },
    Terminal,
}

struct RegisteredInput {
    buffer: InputReplayBuffer,
    channel: Option<u32>,
    provisional_ref: uuid::Uuid,
    stream_token: Option<String>,
}

impl InvocationSession {
    pub async fn open(
        request_provider: Arc<dyn InvocationSessionRequestProvider>,
        connector: Option<Connector>,
        initial: InvocationSessionStateSnapshot,
        pending_operation_is_retry: bool,
        observer: Arc<dyn InvocationSessionStateObserver>,
    ) -> Result<Self, SessionTransportError> {
        if initial.pending_operation.is_none() {
            return Err(SessionTransportError::Protocol(
                "opening an invocation session requires a pending attachment operation".to_string(),
            ));
        }
        observer
            .state_changed(&initial)
            .map_err(SessionTransportError::StatePersistence)?;
        let deliveries = DeliveryTracker::new(initial, observer);
        let transport =
            connect_for_pending_operation(&request_provider, connector.clone(), &deliveries)
                .await?;
        Ok(Self {
            transport,
            request_provider,
            connector,
            deliveries,
            inputs: Vec::new(),
            attachment_state: AttachmentState::Pending {
                exact_retry: pending_operation_is_retry,
            },
        })
    }

    pub fn sender(&self) -> InvocationSessionSender {
        self.transport.sender()
    }

    pub fn delivery_tracker(&self) -> DeliveryTracker {
        self.deliveries.clone()
    }

    pub fn state(&self) -> InvocationSessionStateSnapshot {
        self.deliveries.snapshot()
    }

    pub fn is_accepted(&self) -> bool {
        matches!(self.attachment_state, AttachmentState::Accepted { .. })
    }

    pub fn register_input(
        &mut self,
        provisional_ref: uuid::Uuid,
        stream_token: Option<String>,
        buffer: InputReplayBuffer,
    ) -> Result<(), SessionTransportError> {
        if self.inputs.iter().any(|input| {
            input.provisional_ref == provisional_ref
                || stream_token.is_some() && input.stream_token == stream_token
        }) {
            return Err(SessionTransportError::Protocol(
                "input stream was registered more than once".to_string(),
            ));
        }
        self.inputs.push(RegisteredInput {
            buffer,
            channel: None,
            provisional_ref,
            stream_token,
        });
        Ok(())
    }

    pub fn input_binding(&self, buffer: &InputReplayBuffer) -> Option<(u32, String)> {
        self.inputs
            .iter()
            .find(|input| Arc::ptr_eq(&input.buffer.state, &buffer.state))
            .and_then(|input| Some((input.channel?, input.stream_token.clone()?)))
    }

    fn unregister_input(&mut self, provisional_ref: uuid::Uuid) {
        self.inputs
            .retain(|input| input.provisional_ref != provisional_ref);
    }

    pub async fn send_next_input(
        &mut self,
        buffer: &InputReplayBuffer,
    ) -> Result<Option<SessionSendOutcome>, SessionTransportError> {
        let channel = self
            .inputs
            .iter()
            .find(|input| Arc::ptr_eq(&input.buffer.state, &buffer.state))
            .and_then(|input| input.channel);
        let Some((index, request, sequence_offset)) = buffer.next_unsent() else {
            return Ok(None);
        };
        let Some(channel) = channel else {
            return Ok(None);
        };
        let outcome = match &request {
            ReplayableInput::Value { sequence, value } => {
                self.send_message(&PublicClientMessage::InputStreamItem {
                    channel,
                    sequence: golem_common::model::invocation_session_public::DecimalU64(
                        sequence
                            .checked_add(sequence_offset)
                            .ok_or(SessionTransportError::SequenceOverflow)?,
                    ),
                    value: value.clone(),
                    version: 1,
                })
                .await?
            }
            ReplayableInput::Binary(message) => {
                let mut message = message.clone();
                message.metadata.channel = channel;
                message.metadata.sequence =
                    golem_common::model::invocation_session_public::DecimalU64(
                        message
                            .metadata
                            .sequence
                            .0
                            .checked_add(sequence_offset)
                            .ok_or(SessionTransportError::SequenceOverflow)?,
                    );
                self.send_binary(&message).await?
            }
            ReplayableInput::End { sequence } => {
                self.send_message(&PublicClientMessage::InputStreamEnd {
                    channel,
                    sequence: golem_common::model::invocation_session_public::DecimalU64(
                        sequence
                            .checked_add(sequence_offset)
                            .ok_or(SessionTransportError::SequenceOverflow)?,
                    ),
                    version: 1,
                })
                .await?
            }
        };
        if outcome == SessionSendOutcome::Sent {
            buffer.mark_sent(index)?;
        }
        Ok(Some(outcome))
    }

    pub async fn send_message(
        &mut self,
        message: &PublicClientMessage,
    ) -> Result<SessionSendOutcome, SessionTransportError> {
        if self.attachment_state == AttachmentState::Terminal {
            return Err(SessionTransportError::AttachmentTerminated);
        }
        match self.transport.send_message(message).await {
            Ok(()) => Ok(SessionSendOutcome::Sent),
            Err(error) if is_transport_detach(&error) => {
                self.recover().await?;
                Ok(SessionSendOutcome::Reconnected)
            }
            Err(error) => Err(error),
        }
    }

    pub async fn send_binary(
        &mut self,
        message: &BinaryMessage,
    ) -> Result<SessionSendOutcome, SessionTransportError> {
        if self.attachment_state == AttachmentState::Terminal {
            return Err(SessionTransportError::AttachmentTerminated);
        }
        match self.transport.send_binary(message).await {
            Ok(()) => Ok(SessionSendOutcome::Sent),
            Err(error) if is_transport_detach(&error) => {
                self.recover().await?;
                Ok(SessionSendOutcome::Reconnected)
            }
            Err(error) => Err(error),
        }
    }

    pub async fn recover_after_send_failure(
        &mut self,
        error: SessionTransportError,
    ) -> Result<(), SessionTransportError> {
        if !is_transport_detach(&error) {
            return Err(error);
        }
        self.recover().await
    }

    pub async fn receive(&mut self) -> Result<ReceivedFrame, SessionTransportError> {
        loop {
            let mut frame = match self.transport.receive().await {
                Ok(frame) => frame,
                Err(error) if is_transport_detach(&error) => {
                    self.recover().await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let message = match frame.frame() {
                ServerFrame::Message(message) => message,
                ServerFrame::Binary(_) => {
                    if let AttachmentState::Accepted { attempt_id, .. } = self.attachment_state {
                        self.attachment_state = AttachmentState::Accepted {
                            attempt_id,
                            replay_revocation_eligible: false,
                        };
                    }
                    frame.attach_delivery(&self.deliveries)?;
                    return Ok(frame);
                }
            };
            if !matches!(message, PublicServerMessage::AttachmentRevoked { .. })
                && let AttachmentState::Accepted { attempt_id, .. } = self.attachment_state
            {
                self.attachment_state = AttachmentState::Accepted {
                    attempt_id,
                    replay_revocation_eligible: false,
                };
            }
            match message {
                PublicServerMessage::InvocationAccepted {
                    attempt_id,
                    mappings,
                    session_token,
                    ..
                } => {
                    let AttachmentState::Pending { exact_retry } = self.attachment_state else {
                        return Err(SessionTransportError::UnexpectedAttempt);
                    };
                    let pending = self
                        .deliveries
                        .snapshot()
                        .pending_operation
                        .ok_or(SessionTransportError::UnexpectedAttempt)?;
                    if operation_attempt_id(&pending) != Some(*attempt_id) {
                        return Err(SessionTransportError::UnexpectedAttempt);
                    }
                    self.deliveries.begin_connection(mappings)?;
                    self.install_input_mappings(mappings, true)?;
                    self.deliveries
                        .set_attachment_state(None, Some(session_token.clone()))?;
                    self.attachment_state = AttachmentState::Accepted {
                        attempt_id: *attempt_id,
                        replay_revocation_eligible: exact_retry,
                    };
                }
                PublicServerMessage::InvocationRejected { attempt_id, .. } => {
                    match self.attachment_state {
                        AttachmentState::Pending { .. } => {
                            if let Some(attempt_id) = attempt_id {
                                let pending = self
                                    .deliveries
                                    .snapshot()
                                    .pending_operation
                                    .ok_or(SessionTransportError::UnexpectedAttempt)?;
                                if operation_attempt_id(&pending) != Some(*attempt_id) {
                                    return Err(SessionTransportError::UnexpectedAttempt);
                                }
                            }
                        }
                        AttachmentState::Accepted {
                            attempt_id: accepted_attempt_id,
                            ..
                        } if *attempt_id == Some(accepted_attempt_id) => {}
                        _ => return Err(SessionTransportError::UnexpectedAttempt),
                    }
                    self.deliveries.set_attachment_state(None, None)?;
                    self.attachment_state = AttachmentState::Terminal;
                }
                PublicServerMessage::AttachmentRevoked { .. }
                    if matches!(
                        self.attachment_state,
                        AttachmentState::Accepted {
                            replay_revocation_eligible: true,
                            ..
                        }
                    ) =>
                {
                    self.recover().await?;
                    continue;
                }
                PublicServerMessage::AttachmentRevoked { .. } => {
                    self.attachment_state = AttachmentState::Terminal;
                }
                PublicServerMessage::InvocationFinished { .. } => {
                    self.attachment_state = AttachmentState::Terminal;
                }
                PublicServerMessage::InputStreamAck {
                    channel,
                    highest_contiguous_sequence,
                    mappings,
                    terminal,
                    ..
                } => {
                    self.deliveries.install_mappings(mappings)?;
                    let input = self
                        .inputs
                        .iter()
                        .find(|input| input.channel == Some(*channel))
                        .ok_or_else(|| {
                            SessionTransportError::Protocol(
                                "received an acknowledgement for an unknown input channel"
                                    .to_string(),
                            )
                        })?;
                    input
                        .buffer
                        .acknowledge(highest_contiguous_sequence.0, *terminal)?;
                    self.install_input_mappings(mappings, false)?;
                }
                message => self.deliveries.install_mappings(server_mappings(message))?,
            }
            frame.attach_delivery(&self.deliveries)?;
            return Ok(frame);
        }
    }

    pub async fn close(&self) -> Result<(), SessionTransportError> {
        self.transport.close().await
    }

    fn install_new_resume(
        &self,
        operation: PublicResumeOperation,
    ) -> Result<(), SessionTransportError> {
        let state = self.deliveries.snapshot();
        let session_token = state
            .session_token
            .clone()
            .ok_or(SessionTransportError::MissingSessionToken)?;
        let request = PublicClientMessage::ResumeAttach {
            attempt_id: uuid::Uuid::new_v4(),
            operation,
            output_cursors: state.delivered_output_cursors.into_values().collect(),
            session_token,
            version: 1,
        };
        self.deliveries.set_attachment_state(Some(request), None)
    }

    async fn recover(&mut self) -> Result<(), SessionTransportError> {
        let exact_retry = match self.attachment_state {
            AttachmentState::Pending { .. } => true,
            AttachmentState::Accepted { .. } => {
                self.install_new_resume(PublicResumeOperation::Resume)?;
                false
            }
            AttachmentState::Terminal => {
                return Err(SessionTransportError::AttachmentTerminated);
            }
        };
        self.attachment_state = AttachmentState::Pending { exact_retry };
        self.deliveries.begin_connection(&[])?;
        for input in &mut self.inputs {
            input.channel = None;
        }
        self.transport = connect_for_pending_operation(
            &self.request_provider,
            self.connector.clone(),
            &self.deliveries,
        )
        .await?;
        Ok(())
    }

    fn install_input_mappings(
        &mut self,
        mappings: &[PublicStreamMapping],
        require_all: bool,
    ) -> Result<(), SessionTransportError> {
        let input_mappings = mappings
            .iter()
            .filter(|mapping| mapping.direction == PublicStreamDirection::Input)
            .collect::<Vec<_>>();
        let allow_single_fallback = self.inputs.len() == 1
            && input_mappings.len() == 1
            && self.inputs[0].stream_token.is_none();
        let mut installed = vec![false; self.inputs.len()];
        for mapping in &input_mappings {
            let matches = self
                .inputs
                .iter()
                .enumerate()
                .filter_map(|(index, input)| {
                    input
                        .stream_token
                        .as_ref()
                        .map(|token| token == &mapping.stream_token)
                        .unwrap_or(mapping.provisional_ref == Some(input.provisional_ref))
                        .then_some(index)
                })
                .collect::<Vec<_>>();
            let index = match matches.as_slice() {
                [index] => *index,
                [] if allow_single_fallback => 0,
                [] => continue,
                _ => {
                    return Err(SessionTransportError::Protocol(
                        "public input mapping matched multiple input streams".to_string(),
                    ));
                }
            };
            if installed[index] {
                return Err(SessionTransportError::Protocol(
                    "multiple public mappings identified the same input stream".to_string(),
                ));
            }
            installed[index] = true;
            let input = &mut self.inputs[index];
            input.stream_token = Some(mapping.stream_token.clone());
            input.channel = Some(mapping.channel);
            let high_water = mapping
                .input_high_water
                .as_ref()
                .ok_or(SessionTransportError::InvalidMapping)?;
            input
                .buffer
                .initialize_high_water(high_water.sequence.0, high_water.terminal)?;
        }
        if require_all && installed.iter().any(|installed| !installed) {
            return Err(SessionTransportError::Protocol(
                "invocation acceptance omitted a registered input stream mapping".to_string(),
            ));
        }
        Ok(())
    }
}

pub async fn send_replayable_input(
    sender: &InvocationSessionSender,
    request: &ReplayableInput,
    channel: u32,
    sequence_offset: u64,
) -> Result<(), SessionTransportError> {
    match request {
        ReplayableInput::Value { sequence, value } => {
            sender
                .send_message(&PublicClientMessage::InputStreamItem {
                    channel,
                    sequence: golem_common::model::invocation_session_public::DecimalU64(
                        sequence
                            .checked_add(sequence_offset)
                            .ok_or(SessionTransportError::SequenceOverflow)?,
                    ),
                    value: value.clone(),
                    version: 1,
                })
                .await
        }
        ReplayableInput::Binary(message) => {
            let mut message = message.clone();
            message.metadata.channel = channel;
            message.metadata.sequence = golem_common::model::invocation_session_public::DecimalU64(
                message
                    .metadata
                    .sequence
                    .0
                    .checked_add(sequence_offset)
                    .ok_or(SessionTransportError::SequenceOverflow)?,
            );
            sender.send_binary(&message).await
        }
        ReplayableInput::End { sequence } => {
            sender
                .send_message(&PublicClientMessage::InputStreamEnd {
                    channel,
                    sequence: golem_common::model::invocation_session_public::DecimalU64(
                        sequence
                            .checked_add(sequence_offset)
                            .ok_or(SessionTransportError::SequenceOverflow)?,
                    ),
                    version: 1,
                })
                .await
        }
    }
}

async fn connect_for_pending_operation(
    request_provider: &Arc<dyn InvocationSessionRequestProvider>,
    connector: Option<Connector>,
    deliveries: &DeliveryTracker,
) -> Result<InvocationSessionTransport, SessionTransportError> {
    let operation = deliveries.snapshot().pending_operation.ok_or_else(|| {
        SessionTransportError::Protocol(
            "cannot connect without a pending attachment operation".to_string(),
        )
    })?;
    let mut delay = None;
    loop {
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let request = request_provider.request().await?;
        match InvocationSessionTransport::connect_with_tracker(
            request,
            connector.clone(),
            deliveries.clone(),
        )
        .await
        {
            Ok(transport) => match transport.send_message(&operation).await {
                Ok(()) => return Ok(transport),
                Err(error) if is_transport_detach(&error) => {}
                Err(error) => return Err(error),
            },
            Err(error) if is_retryable_connect(&error) => {}
            Err(error) => return Err(error),
        }
        delay = Some(
            delay
                .map(|delay: Duration| (delay * 2).min(Duration::from_secs(3)))
                .unwrap_or(Duration::from_millis(50)),
        );
    }
}

fn operation_attempt_id(operation: &PublicClientMessage) -> Option<uuid::Uuid> {
    match operation {
        PublicClientMessage::InvocationStart { attempt_id, .. }
        | PublicClientMessage::ResumeAttach { attempt_id, .. } => Some(*attempt_id),
        _ => None,
    }
}

fn server_mappings(message: &PublicServerMessage) -> &[PublicStreamMapping] {
    match message {
        PublicServerMessage::InvocationAccepted { mappings, .. }
        | PublicServerMessage::InvocationResult { mappings, .. }
        | PublicServerMessage::OutputStreamItem { mappings, .. }
        | PublicServerMessage::InputStreamAck { mappings, .. } => mappings,
        _ => &[],
    }
}

fn is_transport_detach(error: &SessionTransportError) -> bool {
    matches!(
        error,
        SessionTransportError::Closed | SessionTransportError::Transport(_)
    )
}

fn is_retryable_connect(error: &SessionTransportError) -> bool {
    matches!(
        error,
        SessionTransportError::Connect(error)
            if !matches!(error, tungstenite::Error::Http(_))
    ) || is_transport_detach(error)
}

async fn run_socket(
    mut socket: Socket,
    mut outbound: mpsc::Receiver<OutboundFrame>,
    inbound: mpsc::Sender<Result<ReceivedFrame, SessionTransportError>>,
    inbound_bytes: Arc<Semaphore>,
) {
    loop {
        tokio::select! {
            outbound = outbound.recv() => match outbound {
                Some(OutboundFrame::Message(message, _permit)) => {
                    if let Err(error) = socket.send(message).await {
                        let _ = inbound.send(Err(SessionTransportError::Transport(error))).await;
                        return;
                    }
                }
                Some(OutboundFrame::Close) | None => {
                    let _ = socket.close(None).await;
                    return;
                }
            },
            incoming = socket.next() => {
                let (frame, permit) = match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let permit = match inbound_bytes.clone().acquire_many_owned(text.len().max(1) as u32).await {
                            Ok(permit) => permit,
                            Err(_) => return,
                        };
                        match decode_server_text(text.as_bytes()) {
                        Ok(message) => (ServerFrame::Message(message), permit),
                        Err(error) => {
                            let _ = inbound.send(Err(SessionTransportError::Protocol(error.to_string()))).await;
                            return;
                        }
                    }
                    },
                    Some(Ok(Message::Binary(bytes))) => {
                        let permit = match inbound_bytes.clone().acquire_many_owned(bytes.len().max(1) as u32).await {
                            Ok(permit) => permit,
                            Err(_) => return,
                        };
                        match decode_binary_message(&bytes) {
                        Ok(message) => (ServerFrame::Binary(message), permit),
                        Err(error) => {
                            let _ = inbound.send(Err(SessionTransportError::Protocol(error.to_string()))).await;
                            return;
                        }
                    }
                    },
                    Some(Ok(Message::Ping(payload))) => {
                        if let Err(error) = socket.send(Message::Pong(payload)).await {
                            let _ = inbound.send(Err(SessionTransportError::Transport(error))).await;
                            return;
                        }
                        continue;
                    }
                    Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(message)) => {
                        let _ = inbound.send(Err(SessionTransportError::Protocol(format!(
                            "unexpected WebSocket frame: {message:?}"
                        )))).await;
                        return;
                    }
                    Some(Err(error)) => {
                        let _ = inbound.send(Err(SessionTransportError::Transport(error))).await;
                        return;
                    }
                };
                if inbound.send(Ok(ReceivedFrame {
                    frame,
                    delivery: None,
                    _admission: permit,
                })).await.is_err() {
                    return;
                }
            }
        }
    }
}

struct GeneratedSessionRequestProvider {
    base_url: reqwest::Url,
    security: crate::Security,
}

#[async_trait]
impl InvocationSessionRequestProvider for GeneratedSessionRequestProvider {
    async fn request(&self) -> Result<tungstenite::http::Request<()>, SessionTransportError> {
        let mut url = self.base_url.clone();
        let websocket_scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            scheme => {
                return Err(SessionTransportError::RequestProvider(format!(
                    "unsupported service URL scheme '{scheme}'"
                )));
            }
        };
        url.set_scheme(websocket_scheme).map_err(|_| {
            SessionTransportError::RequestProvider(
                "failed to derive WebSocket service URL".to_string(),
            )
        })?;
        url.set_path("/v1/agents/invoke-agent-session");
        url.set_query(None);
        url.set_fragment(None);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(SessionTransportError::Request)?;
        if let crate::Security::Bearer(token) = &self.security {
            request.headers_mut().insert(
                tungstenite::http::header::AUTHORIZATION,
                format!("Bearer {token}").parse().map_err(|error| {
                    SessionTransportError::RequestProvider(format!(
                        "invalid authorization token: {error}"
                    ))
                })?,
            );
        }
        Ok(request)
    }
}

pub fn encode_generated_streamless_value(
    graph: &SchemaGraph,
    value: &SchemaValue,
) -> Result<serde_json::Value, PublicSchemaValueError> {
    encode_public_schema_value(graph, &graph.root, value, |_, _| {
        Err(PublicSchemaValueError::new(
            golem_common::model::invocation_session_public::PublicErrorCode::UnsupportedValue,
            "stream values are not allowed here",
        ))
    })
}

pub async fn invoke_generated<T>(
    request: GeneratedInvocationRequest,
    method_parameters: SchemaValue,
    encode_context: GeneratedEncodeContext,
    decode: impl FnOnce(Option<SchemaValue>, &GeneratedDecodeContext) -> Result<T, String>
    + Send
    + 'static,
) -> Result<T, SessionTransportError>
where
    T: Send + 'static,
{
    let method_parameters = encode_context
        .encode_value(&request.input_graph.root, &method_parameters)
        .map_err(|error| SessionTransportError::Protocol(error.to_string()))?;
    let start = PublicClientMessage::InvocationStart {
        attempt_id: uuid::Uuid::new_v4(),
        config: request.config,
        idempotency_key: request.idempotency_key,
        method_parameters,
        selector: Box::new(request.selector),
        version: INVOCATION_SESSION_VERSION,
    };
    let provider: Arc<dyn InvocationSessionRequestProvider> =
        Arc::new(GeneratedSessionRequestProvider {
            base_url: request.base_url,
            security: request.security,
        });
    let initial = InvocationSessionStateSnapshot {
        delivered_output_cursors: BTreeMap::new(),
        pending_operation: Some(start),
        session_token: None,
    };
    let session = InvocationSession::open(provider, None, initial, false, Arc::new(())).await?;
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(run_generated_invocation(
        session,
        encode_context,
        request.output_graph,
        Box::new(decode),
        result_tx,
    ));
    let mut task = GeneratedTaskAbortGuard::new(task.abort_handle());
    let result = result_rx.await.map_err(|_| SessionTransportError::Closed)?;
    task.disarm();
    result
}

type GeneratedResultDecoder<T> =
    dyn FnOnce(Option<SchemaValue>, &GeneratedDecodeContext) -> Result<T, String> + Send;

struct GeneratedTaskAbortGuard(Option<tokio::task::AbortHandle>);

impl GeneratedTaskAbortGuard {
    fn new(handle: tokio::task::AbortHandle) -> Self {
        Self(Some(handle))
    }

    fn abort(&self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GeneratedTaskAbortGuard {
    fn drop(&mut self) {
        self.abort();
    }
}

async fn run_generated_invocation<T>(
    mut session: InvocationSession,
    encode_context: GeneratedEncodeContext,
    output_graph: Option<SchemaGraph>,
    decode: Box<GeneratedResultDecoder<T>>,
    result_tx: tokio::sync::oneshot::Sender<Result<T, SessionTransportError>>,
) where
    T: Send + 'static,
{
    let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let output_context = output_graph
        .clone()
        .map(|graph| GeneratedDecodeContext::new(graph, commands_tx));
    let mut decode = Some(decode);
    let mut result_tx = Some(result_tx);
    let mut inputs = HashMap::<uuid::Uuid, InputReplayBuffer>::new();
    let mut input_tasks = HashMap::<uuid::Uuid, GeneratedTaskAbortGuard>::new();
    let mut failed_inputs = HashMap::<uuid::Uuid, String>::new();
    let mut pending_output_cancellations = HashMap::<String, PublicClientCancelReason>::new();
    let mut cancelled_outputs = HashSet::<String>::new();

    if let Err(error) = install_generated_inputs(
        &mut session,
        &encode_context,
        &input_tx,
        &mut inputs,
        &mut input_tasks,
    ) {
        send_generated_result(&mut result_tx, Err(error));
        return;
    }

    loop {
        drain_generated_events(
            &mut commands_rx,
            &mut input_rx,
            output_context.as_ref(),
            &inputs,
            &mut pending_output_cancellations,
            &mut cancelled_outputs,
            &mut failed_inputs,
        );
        if let Err(error) = install_generated_inputs(
            &mut session,
            &encode_context,
            &input_tx,
            &mut inputs,
            &mut input_tasks,
        ) {
            send_generated_result(&mut result_tx, Err(error));
            return;
        }
        if session.is_accepted()
            && let Err(error) =
                cancel_pending_outputs(&mut session, &mut pending_output_cancellations).await
        {
            send_generated_result(&mut result_tx, Err(error));
            return;
        }
        if session.is_accepted()
            && let Err(error) = flush_generated_inputs(&mut session, &inputs).await
        {
            send_generated_result(&mut result_tx, Err(error));
            return;
        }
        if session.is_accepted()
            && let Err(error) =
                cancel_failed_inputs(&mut session, &inputs, &mut failed_inputs).await
        {
            send_generated_result(&mut result_tx, Err(error));
            return;
        }
        if generated_invocation_is_complete(
            &result_tx,
            output_context.as_ref(),
            &pending_output_cancellations,
            &failed_inputs,
        ) {
            return;
        }

        tokio::select! {
            biased;
            Some(command) = commands_rx.recv() => {
                admit_generated_command(
                    command,
                    output_context.as_ref(),
                    &mut pending_output_cancellations,
                    &mut cancelled_outputs,
                );
            }
            Some(event) = input_rx.recv() => {
                admit_generated_input_event(event, &inputs, &mut failed_inputs);
            }
            frame = session.receive() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(error) => {
                        send_generated_result(&mut result_tx, Err(error));
                        fail_generated_outputs(output_context.as_ref(), "invocation session transport failed").await;
                        return;
                    }
                };
                match handle_generated_frame(
                    frame,
                    &mut session,
                    output_graph.as_ref(),
                    output_context.as_ref(),
                    &mut decode,
                    &mut result_tx,
                    &mut inputs,
                    &mut input_tasks,
                    &mut pending_output_cancellations,
                    &mut cancelled_outputs,
                ).await {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(error) => {
                        send_generated_result(&mut result_tx, Err(error));
                        fail_generated_outputs(output_context.as_ref(), "invocation session protocol failed").await;
                        return;
                    }
                }
            }
            else => {
                send_generated_result(&mut result_tx, Err(SessionTransportError::Closed));
                return;
            }
        }
    }
}

fn drain_generated_events(
    commands: &mut mpsc::UnboundedReceiver<GeneratedCommand>,
    input_events: &mut mpsc::UnboundedReceiver<GeneratedInputEvent>,
    output_context: Option<&GeneratedDecodeContext>,
    inputs: &HashMap<uuid::Uuid, InputReplayBuffer>,
    pending_output_cancellations: &mut HashMap<String, PublicClientCancelReason>,
    cancelled_outputs: &mut HashSet<String>,
    failed_inputs: &mut HashMap<uuid::Uuid, String>,
) {
    while let Ok(command) = commands.try_recv() {
        admit_generated_command(
            command,
            output_context,
            pending_output_cancellations,
            cancelled_outputs,
        );
    }
    while let Ok(event) = input_events.try_recv() {
        admit_generated_input_event(event, inputs, failed_inputs);
    }
}

fn admit_generated_command(
    command: GeneratedCommand,
    output_context: Option<&GeneratedDecodeContext>,
    pending_output_cancellations: &mut HashMap<String, PublicClientCancelReason>,
    cancelled_outputs: &mut HashSet<String>,
) {
    let GeneratedCommand::CancelOutput {
        stream_token,
        reason,
    } = command;
    let registered = output_context.is_some_and(|context| {
        context
            .outputs
            .lock()
            .expect("generated output registry mutex poisoned")
            .remove(&stream_token)
            .is_some()
    });
    if registered {
        cancelled_outputs.insert(stream_token.clone());
        pending_output_cancellations.insert(stream_token, reason);
    }
}

fn admit_generated_input_event(
    event: GeneratedInputEvent,
    inputs: &HashMap<uuid::Uuid, InputReplayBuffer>,
    failed_inputs: &mut HashMap<uuid::Uuid, String>,
) {
    match event {
        GeneratedInputEvent::Input {
            provisional_ref,
            input,
        } => {
            if let Some(buffer) = inputs.get(&provisional_ref) {
                buffer.push(input);
            }
        }
        GeneratedInputEvent::Failed {
            provisional_ref,
            message,
        } => {
            if inputs.contains_key(&provisional_ref) {
                failed_inputs.insert(provisional_ref, message);
            }
        }
    }
}

fn generated_invocation_is_complete<T>(
    result: &Option<tokio::sync::oneshot::Sender<Result<T, SessionTransportError>>>,
    output: Option<&GeneratedDecodeContext>,
    pending_output_cancellations: &HashMap<String, PublicClientCancelReason>,
    failed_inputs: &HashMap<uuid::Uuid, String>,
) -> bool {
    result.is_none()
        && pending_output_cancellations.is_empty()
        && failed_inputs.is_empty()
        && output.is_none_or(|context| {
            context
                .outputs
                .lock()
                .expect("generated output registry mutex poisoned")
                .is_empty()
        })
}

async fn cancel_pending_outputs(
    session: &mut InvocationSession,
    pending: &mut HashMap<String, PublicClientCancelReason>,
) -> Result<(), SessionTransportError> {
    let ready = pending
        .iter()
        .filter_map(|(stream_token, reason)| {
            session
                .delivery_tracker()
                .channel_for_stream(stream_token)
                .map(|channel| (stream_token.clone(), *reason, channel))
        })
        .collect::<Vec<_>>();
    for (stream_token, reason, channel) in ready {
        let outcome = session
            .send_message(&PublicClientMessage::StreamCancel {
                channel,
                reason,
                version: INVOCATION_SESSION_VERSION,
            })
            .await?;
        match outcome {
            SessionSendOutcome::Sent => {
                pending.remove(&stream_token);
            }
            SessionSendOutcome::Reconnected => break,
        }
    }
    Ok(())
}

fn install_generated_inputs(
    session: &mut InvocationSession,
    context: &GeneratedEncodeContext,
    events: &mpsc::UnboundedSender<GeneratedInputEvent>,
    inputs: &mut HashMap<uuid::Uuid, InputReplayBuffer>,
    tasks: &mut HashMap<uuid::Uuid, GeneratedTaskAbortGuard>,
) -> Result<(), SessionTransportError> {
    for pending in context.take_pending() {
        session.register_input(pending.provisional_ref, None, pending.buffer.clone())?;
        let task = pending.source.spawn(
            pending.provisional_ref,
            pending.buffer.clone(),
            context.clone(),
            events.clone(),
        );
        inputs.insert(pending.provisional_ref, pending.buffer);
        tasks.insert(pending.provisional_ref, GeneratedTaskAbortGuard::new(task));
    }
    Ok(())
}

async fn flush_generated_inputs(
    session: &mut InvocationSession,
    inputs: &HashMap<uuid::Uuid, InputReplayBuffer>,
) -> Result<(), SessionTransportError> {
    for buffer in inputs.values() {
        while let Some(outcome) = session.send_next_input(buffer).await? {
            if outcome == SessionSendOutcome::Reconnected {
                break;
            }
        }
    }
    Ok(())
}

async fn cancel_failed_inputs(
    session: &mut InvocationSession,
    inputs: &HashMap<uuid::Uuid, InputReplayBuffer>,
    failed: &mut HashMap<uuid::Uuid, String>,
) -> Result<(), SessionTransportError> {
    let ready = failed
        .keys()
        .filter_map(|reference| {
            let buffer = inputs.get(reference)?;
            session
                .input_binding(buffer)
                .map(|(channel, _)| (*reference, channel))
        })
        .collect::<Vec<_>>();
    for (reference, channel) in ready {
        let outcome = session
            .send_message(&PublicClientMessage::StreamCancel {
                channel,
                reason: PublicClientCancelReason::SourceUnavailable,
                version: INVOCATION_SESSION_VERSION,
            })
            .await?;
        match outcome {
            SessionSendOutcome::Sent => {
                failed.remove(&reference);
            }
            SessionSendOutcome::Reconnected => break,
        }
    }
    Ok(())
}

fn send_generated_result<T>(
    sender: &mut Option<tokio::sync::oneshot::Sender<Result<T, SessionTransportError>>>,
    result: Result<T, SessionTransportError>,
) {
    if let Some(sender) = sender.take() {
        let _ = sender.send(result);
    }
}

async fn handle_generated_frame<T>(
    frame: ReceivedFrame,
    session: &mut InvocationSession,
    output_graph: Option<&SchemaGraph>,
    output_context: Option<&GeneratedDecodeContext>,
    decode: &mut Option<Box<GeneratedResultDecoder<T>>>,
    result_tx: &mut Option<tokio::sync::oneshot::Sender<Result<T, SessionTransportError>>>,
    inputs: &mut HashMap<uuid::Uuid, InputReplayBuffer>,
    input_tasks: &mut HashMap<uuid::Uuid, GeneratedTaskAbortGuard>,
    pending_output_cancellations: &mut HashMap<String, PublicClientCancelReason>,
    cancelled_outputs: &mut HashSet<String>,
) -> Result<bool, SessionTransportError>
where
    T: Send + 'static,
{
    match frame.frame() {
        ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. })
        | ServerFrame::Message(PublicServerMessage::InputStreamAck { .. }) => Ok(false),
        ServerFrame::Message(PublicServerMessage::InvocationRejected { code, message, .. }) => {
            send_generated_result(
                result_tx,
                Err(SessionTransportError::Protocol(format!(
                    "{}: {message}",
                    code.as_str()
                ))),
            );
            Ok(true)
        }
        ServerFrame::Message(PublicServerMessage::InvocationResult { result, .. }) => {
            let value = match result {
                PublicInvocationResult::None => None,
                PublicInvocationResult::Value { value } => {
                    let graph = output_graph.ok_or_else(|| {
                        SessionTransportError::Protocol(
                            "server returned a value for a unit method".to_string(),
                        )
                    })?;
                    let context = output_context.expect("output graph has a decode context");
                    Some(
                        context
                            .decode_value(&graph.root, value)
                            .map_err(|error| SessionTransportError::Protocol(error.to_string()))?,
                    )
                }
            };
            let decoder = decode.take().ok_or_else(|| {
                SessionTransportError::Protocol(
                    "server returned the invocation result more than once".to_string(),
                )
            })?;
            let fallback_context;
            let context = if let Some(context) = output_context {
                context
            } else {
                fallback_context = GeneratedDecodeContext::new(
                    SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
                    mpsc::unbounded_channel().0,
                );
                &fallback_context
            };
            let decoded = decoder(value, context).map_err(SessionTransportError::Protocol)?;
            send_generated_result(result_tx, Ok(decoded));
            Ok(false)
        }
        ServerFrame::Message(PublicServerMessage::OutputStreamItem { channel, value, .. }) => {
            let channel = *channel;
            let context = output_context.ok_or_else(|| {
                SessionTransportError::Protocol(
                    "server returned output stream data for a unit method".to_string(),
                )
            })?;
            let stream_token = session
                .delivery_tracker()
                .stream_for_channel(channel)
                .ok_or_else(|| {
                    SessionTransportError::Protocol("unknown output channel".to_string())
                })?;
            if cancelled_outputs.contains(&stream_token) {
                return Ok(false);
            }
            let mut sink = context
                .outputs
                .lock()
                .expect("generated output registry mutex poisoned")
                .remove(&stream_token)
                .ok_or_else(|| {
                    SessionTransportError::Protocol("unknown output stream".to_string())
                })?;
            let value = context
                .decode_value(sink.item_type(), value)
                .map_err(|error| SessionTransportError::Protocol(error.to_string()))?;
            if let Err(error) = sink.value(value, frame, context).await {
                if matches!(error, SessionTransportError::Closed) {
                    cancelled_outputs.insert(stream_token.clone());
                    pending_output_cancellations
                        .insert(stream_token, PublicClientCancelReason::ConsumerDrop);
                    return Ok(false);
                }
                return Err(error);
            }
            context
                .outputs
                .lock()
                .expect("generated output registry mutex poisoned")
                .insert(stream_token, sink);
            Ok(false)
        }
        ServerFrame::Binary(message) => {
            let message = message.clone();
            let channel = message.metadata.channel;
            let context = output_context.ok_or_else(|| {
                SessionTransportError::Protocol(
                    "server returned binary output for a unit method".to_string(),
                )
            })?;
            let stream_token = session
                .delivery_tracker()
                .stream_for_channel(channel)
                .ok_or_else(|| {
                    SessionTransportError::Protocol("unknown output channel".to_string())
                })?;
            if cancelled_outputs.contains(&stream_token) {
                return Ok(false);
            }
            let mut sink = context
                .outputs
                .lock()
                .expect("generated output registry mutex poisoned")
                .remove(&stream_token)
                .ok_or_else(|| {
                    SessionTransportError::Protocol("unknown output stream".to_string())
                })?;
            if let Err(error) = sink.binary(message, frame, context).await {
                if matches!(error, SessionTransportError::Closed) {
                    cancelled_outputs.insert(stream_token.clone());
                    pending_output_cancellations
                        .insert(stream_token, PublicClientCancelReason::ConsumerDrop);
                    return Ok(false);
                }
                return Err(error);
            }
            context
                .outputs
                .lock()
                .expect("generated output registry mutex poisoned")
                .insert(stream_token, sink);
            Ok(false)
        }
        ServerFrame::Message(PublicServerMessage::OutputStreamEnd {
            channel, outcome, ..
        }) => {
            let context = output_context.ok_or_else(|| {
                SessionTransportError::Protocol(
                    "server returned an output terminal for a unit method".to_string(),
                )
            })?;
            let stream_token = session
                .delivery_tracker()
                .stream_for_channel(*channel)
                .ok_or_else(|| {
                    SessionTransportError::Protocol("unknown output channel".to_string())
                })?;
            if cancelled_outputs.contains(&stream_token) {
                return Ok(false);
            }
            let mut sink = context
                .outputs
                .lock()
                .expect("generated output registry mutex poisoned")
                .remove(&stream_token)
                .ok_or_else(|| {
                    SessionTransportError::Protocol("unknown output stream".to_string())
                })?;
            if let Err(error) = sink.finish(outcome.clone(), Some(frame)).await
                && !matches!(error, SessionTransportError::Closed)
            {
                return Err(error);
            }
            Ok(false)
        }
        ServerFrame::Message(PublicServerMessage::StreamCancel {
            channel, reason, ..
        }) => {
            match session.delivery_tracker().direction_for_channel(*channel) {
                Some(PublicStreamDirection::Output) => {
                    let context = output_context.ok_or_else(|| {
                        SessionTransportError::Protocol(
                            "server cancelled an output for a unit method".to_string(),
                        )
                    })?;
                    let stream_token = session
                        .delivery_tracker()
                        .stream_for_channel(*channel)
                        .ok_or_else(|| {
                            SessionTransportError::Protocol("unknown output channel".to_string())
                        })?;
                    let sink = context
                        .outputs
                        .lock()
                        .expect("generated output registry mutex poisoned")
                        .remove(&stream_token);
                    if let Some(mut sink) = sink {
                        let result = sink
                            .finish(
                                PublicOutputStreamOutcome::Cancelled { reason: *reason },
                                None,
                            )
                            .await;
                        if let Err(error) = result
                            && !matches!(error, SessionTransportError::Closed)
                        {
                            return Err(error);
                        }
                    }
                }
                Some(PublicStreamDirection::Input) => {
                    let token = session
                        .delivery_tracker()
                        .stream_for_channel(*channel)
                        .ok_or_else(|| {
                            SessionTransportError::Protocol("unknown input channel".to_string())
                        })?;
                    let reference = input_tasks.keys().copied().find(|reference| {
                        session.inputs.iter().any(|input| {
                            input.provisional_ref == *reference
                                && input.stream_token.as_deref() == Some(token.as_str())
                        })
                    });
                    if let Some(reference) = reference {
                        if let Some(task) = input_tasks.remove(&reference) {
                            task.abort();
                        }
                        inputs.remove(&reference);
                        session.unregister_input(reference);
                    }
                }
                None => {
                    return Err(SessionTransportError::Protocol(
                        "unknown cancelled channel".to_string(),
                    ));
                }
            }
            Ok(false)
        }
        ServerFrame::Message(PublicServerMessage::AttachmentRevoked { .. }) => {
            Err(SessionTransportError::AttachmentTerminated)
        }
        ServerFrame::Message(PublicServerMessage::InvocationFinished { outcome, .. }) => {
            for task in input_tasks.values() {
                task.abort();
            }
            input_tasks.clear();
            inputs.clear();
            let failure = match outcome {
                PublicInvocationOutcome::Success => None,
                PublicInvocationOutcome::Failure { code, message } => {
                    Some(format!("{}: {message}", code.as_str()))
                }
            };
            if let Some(message) = failure {
                send_generated_result(
                    result_tx,
                    Err(SessionTransportError::Protocol(message.clone())),
                );
                fail_generated_outputs(output_context, &message).await;
            } else if result_tx.is_some() {
                send_generated_result(
                    result_tx,
                    Err(SessionTransportError::Protocol(
                        "invocation finished before returning its result".to_string(),
                    )),
                );
            }
            Ok(false)
        }
    }
}

async fn fail_generated_outputs(context: Option<&GeneratedDecodeContext>, message: &str) {
    let Some(context) = context else {
        return;
    };
    let outputs = std::mem::take(
        &mut *context
            .outputs
            .lock()
            .expect("generated output registry mutex poisoned"),
    );
    for (_, mut output) in outputs {
        let _ = output
            .finish(
                PublicOutputStreamOutcome::Error {
                    code: golem_common::model::invocation_session_public::PublicErrorCode::InvocationFailed,
                    message: message.to_string(),
                },
                None,
            )
            .await;
    }
}

async fn run_generated_input_source<T>(
    mut source: TypedGeneratedInputSource<T>,
    provisional_ref: uuid::Uuid,
    buffer: InputReplayBuffer,
    context: GeneratedEncodeContext,
    events: mpsc::UnboundedSender<GeneratedInputEvent>,
) where
    T: Send + 'static,
{
    let mut sequence = 0_u64;
    loop {
        let item = match source.stream.next().await {
            Some(Ok(item)) => item,
            Some(Err(error)) => {
                let _ = events.send(GeneratedInputEvent::Failed {
                    provisional_ref,
                    message: error.to_string(),
                });
                return;
            }
            None => {
                let request = ReplayableInput::End { sequence };
                if let Ok(input) = buffer.admit(request, 1).await {
                    let _ = events.send(GeneratedInputEvent::Input {
                        provisional_ref,
                        input,
                    });
                }
                return;
            }
        };
        let value = match (source.encode)(item, &context) {
            Ok(value) => value,
            Err(message) => {
                let _ = events.send(GeneratedInputEvent::Failed {
                    provisional_ref,
                    message,
                });
                return;
            }
        };
        if source.lane != StreamLane::Json
            && let Err(errors) = validate_value(&context.graph, &source.item_type, &value)
        {
            let _ = events.send(GeneratedInputEvent::Failed {
                provisional_ref,
                message: format!("binary input failed schema validation: {errors:?}"),
            });
            return;
        }
        let mut after_batch: Option<Result<(), String>> = None;
        let request = match source.lane {
            StreamLane::Json => {
                let value = match context.encode_value(&source.item_type, &value) {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = events.send(GeneratedInputEvent::Failed {
                            provisional_ref,
                            message: error.to_string(),
                        });
                        return;
                    }
                };
                let request = ReplayableInput::Value { sequence, value };
                sequence = match sequence.checked_add(1) {
                    Some(sequence) => sequence,
                    None => return,
                };
                request
            }
            StreamLane::Binary => {
                let SchemaValue::Binary(binary) = value else {
                    let _ = events.send(GeneratedInputEvent::Failed {
                        provisional_ref,
                        message: "binary stream encoder returned a non-binary value".to_string(),
                    });
                    return;
                };
                let request = ReplayableInput::Binary(BinaryMessage {
                    metadata: BinaryMessageMetadata {
                        channel: 0,
                        cursor_token: None,
                        item_count: DecimalU64(1),
                        kind: BinaryMessageKind::InputBinary,
                        mime_type: binary.mime_type,
                        sequence: DecimalU64(sequence),
                        version: INVOCATION_SESSION_VERSION,
                    },
                    payload: binary.bytes,
                });
                sequence = match sequence.checked_add(1) {
                    Some(sequence) => sequence,
                    None => return,
                };
                request
            }
            StreamLane::U8 => {
                let SchemaValue::U8(first) = value else {
                    let _ = events.send(GeneratedInputEvent::Failed {
                        provisional_ref,
                        message: "u8 stream encoder returned a non-u8 value".to_string(),
                    });
                    return;
                };
                let mut payload = Vec::with_capacity(4096);
                payload.push(first);
                while payload.len() < MAX_PACKED_U8_SIZE {
                    match tokio::time::timeout(PACKED_U8_FLUSH_DELAY, source.stream.next()).await {
                        Err(_) => break,
                        Ok(None) => {
                            after_batch = Some(Ok(()));
                            break;
                        }
                        Ok(Some(Err(error))) => {
                            after_batch = Some(Err(error.to_string()));
                            break;
                        }
                        Ok(Some(Ok(item))) => match (source.encode)(item, &context) {
                            Ok(value @ SchemaValue::U8(_)) => {
                                if let Err(errors) =
                                    validate_value(&context.graph, &source.item_type, &value)
                                {
                                    after_batch = Some(Err(format!(
                                        "binary input failed schema validation: {errors:?}"
                                    )));
                                    break;
                                }
                                let SchemaValue::U8(byte) = value else {
                                    unreachable!()
                                };
                                payload.push(byte);
                            }
                            Ok(_) => {
                                after_batch = Some(Err(
                                    "u8 stream encoder returned a non-u8 value".to_string(),
                                ));
                                break;
                            }
                            Err(error) => {
                                after_batch = Some(Err(error));
                                break;
                            }
                        },
                    }
                }
                let count = payload.len() as u64;
                let request = ReplayableInput::Binary(BinaryMessage {
                    metadata: BinaryMessageMetadata {
                        channel: 0,
                        cursor_token: None,
                        item_count: DecimalU64(count),
                        kind: BinaryMessageKind::InputU8,
                        mime_type: None,
                        sequence: DecimalU64(sequence),
                        version: INVOCATION_SESSION_VERSION,
                    },
                    payload,
                });
                sequence = match sequence.checked_add(count) {
                    Some(sequence) => sequence,
                    None => return,
                };
                request
            }
        };
        let byte_charge = match &request {
            ReplayableInput::Value { value, .. } => value.to_string().len(),
            ReplayableInput::Binary(message) => message.payload.len(),
            ReplayableInput::End { .. } => 1,
        };
        let input = match buffer.admit(request, byte_charge).await {
            Ok(input) => input,
            Err(error) => {
                let _ = events.send(GeneratedInputEvent::Failed {
                    provisional_ref,
                    message: error.to_string(),
                });
                return;
            }
        };
        if events
            .send(GeneratedInputEvent::Input {
                provisional_ref,
                input,
            })
            .is_err()
        {
            return;
        }
        if let Some(terminal) = after_batch {
            match terminal {
                Ok(()) => {
                    let request = ReplayableInput::End { sequence };
                    if let Ok(input) = buffer.admit(request, 1).await {
                        let _ = events.send(GeneratedInputEvent::Input {
                            provisional_ref,
                            input,
                        });
                    }
                }
                Err(message) => {
                    let _ = events.send(GeneratedInputEvent::Failed {
                        provisional_ref,
                        message,
                    });
                }
            }
            return;
        }
    }
}

fn stream_lane(graph: &SchemaGraph, ty: &SchemaType) -> Result<StreamLane, String> {
    let mut ty = ty;
    let mut seen = Vec::new();
    while let SchemaType::Ref { id, .. } = ty {
        if seen.contains(id) {
            return Err("stream item schema contains a reference cycle".to_string());
        }
        seen.push(id.clone());
        ty = &graph
            .lookup(id)
            .ok_or_else(|| format!("stream item references missing type '{id}'"))?
            .body;
    }
    Ok(match ty {
        SchemaType::U8 { .. } => StreamLane::U8,
        SchemaType::Binary { .. } => StreamLane::Binary,
        _ => StreamLane::Json,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum AgentStreamError {
    #[error("stream producer failed: {0}")]
    Producer(String),
    #[error("stream was cancelled: {0}")]
    Cancelled(String),
    #[error(transparent)]
    Transport(#[from] SessionTransportError),
}

enum AgentStreamEvent<T> {
    Item {
        value: T,
        delivery: Option<Delivery>,
    },
    End {
        delivery: Option<Delivery>,
    },
    Failed {
        error: AgentStreamError,
        delivery: Option<Delivery>,
    },
}

type BoxAgentStream<T> = Pin<Box<dyn Stream<Item = AgentStreamEvent<T>> + Send>>;

pub enum AgentStreamOutput<T> {
    Item(T, Option<ReceivedFrame>),
    End(Option<ReceivedFrame>),
    Error(AgentStreamError, Option<ReceivedFrame>),
}

/// An affine asynchronous stream used by generated Rust clients.
///
/// It is intentionally not `Clone`. Input ownership moves into an invocation,
/// and output cursor state advances only when `poll_next` yields an item.
pub struct AgentStream<T> {
    inner: BoxAgentStream<T>,
    cancel: Option<Box<dyn FnOnce(PublicClientCancelReason) + Send>>,
    terminal: bool,
}

impl<T> Debug for AgentStream<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentStream")
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl<T> AgentStream<T>
where
    T: Send + 'static,
{
    pub fn input<S, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<T, E>> + Send + 'static,
        E: std::fmt::Display,
    {
        Self {
            inner: Box::pin(stream.map(|item| {
                item.map(|value| AgentStreamEvent::Item {
                    value,
                    delivery: None,
                })
                .unwrap_or_else(|error| AgentStreamEvent::Failed {
                    error: AgentStreamError::Producer(error.to_string()),
                    delivery: None,
                })
            })),
            cancel: None,
            terminal: false,
        }
    }

    #[doc(hidden)]
    pub fn output(
        receiver: mpsc::Receiver<AgentStreamOutput<T>>,
        cancel: impl FnOnce(PublicClientCancelReason) + Send + 'static,
    ) -> Self {
        Self {
            inner: Box::pin(futures_util::stream::unfold(
                receiver,
                |mut receiver| async move {
                    receiver.recv().await.map(|item| {
                        let item = match item {
                            AgentStreamOutput::Item(value, frame) => AgentStreamEvent::Item {
                                value,
                                delivery: take_delivery(frame),
                            },
                            AgentStreamOutput::End(frame) => AgentStreamEvent::End {
                                delivery: take_delivery(frame),
                            },
                            AgentStreamOutput::Error(error, frame) => AgentStreamEvent::Failed {
                                error,
                                delivery: take_delivery(frame),
                            },
                        };
                        (item, receiver)
                    })
                },
            )),
            cancel: Some(Box::new(cancel)),
            terminal: false,
        }
    }

    /// Explicitly cancels an open output stream.
    pub fn cancel(&mut self) {
        if !self.terminal {
            self.terminal = true;
            if let Some(cancel) = self.cancel.take() {
                cancel(PublicClientCancelReason::Cancelled);
            }
        }
    }
}

fn take_delivery(frame: Option<ReceivedFrame>) -> Option<Delivery> {
    frame.and_then(|mut frame| frame.delivery.take())
}

fn mark_stream_delivery(delivery: &mut Option<Delivery>) -> Result<(), AgentStreamError> {
    if let Some(delivery) = delivery.as_mut() {
        delivery.mark()?;
    }
    Ok(())
}

impl<T> Stream for AgentStream<T> {
    type Item = Result<T, AgentStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.terminal {
            return Poll::Ready(None);
        }
        let result = self.inner.as_mut().poll_next(cx);
        match result {
            Poll::Ready(Some(AgentStreamEvent::Item {
                value,
                mut delivery,
            })) => {
                if let Err(error) = mark_stream_delivery(&mut delivery) {
                    self.terminal = true;
                    self.cancel = None;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(Some(Ok(value)))
            }
            Poll::Ready(Some(AgentStreamEvent::End { mut delivery })) => {
                self.terminal = true;
                self.cancel = None;
                match mark_stream_delivery(&mut delivery) {
                    Ok(()) => Poll::Ready(None),
                    Err(error) => Poll::Ready(Some(Err(error))),
                }
            }
            Poll::Ready(Some(AgentStreamEvent::Failed {
                error,
                mut delivery,
            })) => {
                self.terminal = true;
                self.cancel = None;
                match mark_stream_delivery(&mut delivery) {
                    Ok(()) => Poll::Ready(Some(Err(error))),
                    Err(delivery_error) => Poll::Ready(Some(Err(delivery_error))),
                }
            }
            Poll::Ready(None) => {
                self.terminal = true;
                self.cancel = None;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for AgentStream<T> {
    fn drop(&mut self) {
        if !self.terminal
            && let Some(cancel) = self.cancel.take()
        {
            cancel(PublicClientCancelReason::ConsumerDrop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BinaryFixture {
        vectors: Vec<BinaryVector>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BinaryVector {
        name: String,
        frame_base64: String,
    }

    #[test]
    fn binary_codec_matches_all_frozen_public_v1_frames() {
        let fixture: BinaryFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/stream-session-v1/binary-messages.json"
        ))
        .unwrap();
        for vector in fixture.vectors {
            let golden = base64::engine::general_purpose::STANDARD
                .decode(&vector.frame_base64)
                .unwrap();
            let decoded = decode_binary_message(&golden)
                .unwrap_or_else(|error| panic!("{} did not decode: {error}", vector.name));
            assert_eq!(
                encode_binary_message(&decoded).unwrap(),
                golden,
                "{}",
                vector.name
            );
        }
    }
    use golem_common::model::invocation_session_public::{
        DecimalU64, InvocationSelector, PublicAttachmentRevokedReason, PublicErrorCode,
        PublicInputHighWater, PublicInvocationOutcome, PublicInvocationResult,
        PublicOutputStreamOutcome, decode_client_text,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use test_r::test;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    struct StaticRequestProvider(String);

    #[async_trait]
    impl InvocationSessionRequestProvider for StaticRequestProvider {
        async fn request(&self) -> Result<tungstenite::http::Request<()>, SessionTransportError> {
            self.0
                .as_str()
                .into_client_request()
                .map_err(SessionTransportError::Request)
        }
    }

    fn start_request(attempt_id: uuid::Uuid) -> PublicClientMessage {
        PublicClientMessage::InvocationStart {
            attempt_id,
            config: Vec::new(),
            idempotency_key: "invocation-key".to_string(),
            method_parameters: serde_json::json!({}),
            selector: Box::new(InvocationSelector {
                agent_type: "agent".to_string(),
                application: "app".to_string(),
                constructor_parameters: serde_json::json!({}),
                environment: "env".to_string(),
                method: "run".to_string(),
                phantom_id: None,
            }),
            version: 1,
        }
    }

    async fn receive_client_message(
        socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    ) -> PublicClientMessage {
        let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
            panic!("expected client text message")
        };
        decode_client_text(text.as_bytes()).unwrap()
    }

    fn select_session_subprotocol(_request: &Request, mut response: Response) -> Response {
        response.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            INVOCATION_SESSION_SUBPROTOCOL.parse().unwrap(),
        );
        response
    }

    fn permit() -> tokio::sync::OwnedSemaphorePermit {
        Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap()
    }

    fn output_frame(tracker: &DeliveryTracker, sequence: u64, token: &str) -> ReceivedFrame {
        tracker
            .install_mappings(&[PublicStreamMapping {
                channel: 7,
                direction: PublicStreamDirection::Output,
                input_high_water: None,
                provisional_ref: None,
                stream_token: "stream-seven".to_string(),
            }])
            .unwrap();
        let frame = ServerFrame::Message(PublicServerMessage::OutputStreamEnd {
            channel: 7,
            cursor_token: Some(token.to_string()),
            outcome: PublicOutputStreamOutcome::Ok,
            sequence: DecimalU64(sequence),
            version: 1,
        });
        ReceivedFrame {
            delivery: tracker.delivery(&frame).unwrap(),
            frame,
            _admission: permit(),
        }
    }

    struct FailOnceObserver {
        calls: AtomicUsize,
    }

    impl InvocationSessionStateObserver for FailOnceObserver {
        fn state_changed(&self, _state: &InvocationSessionStateSnapshot) -> Result<(), String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("injected checkpoint failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn cursor_advances_only_after_delivery() {
        let tracker = DeliveryTracker::default();
        let mut frame = output_frame(&tracker, 3, "cursor-three");

        assert!(tracker.delivered_cursor_tokens().is_empty());
        assert!(frame.mark_delivered().unwrap());
        assert_eq!(tracker.delivered_cursor_tokens(), vec!["cursor-three"]);
        assert!(!frame.mark_delivered().unwrap());
    }

    #[test]
    fn cursor_persistence_failure_does_not_advance_and_can_be_retried() {
        let tracker = DeliveryTracker::new(
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: None,
                session_token: None,
            },
            Arc::new(FailOnceObserver {
                calls: AtomicUsize::new(0),
            }),
        );
        let mut frame = output_frame(&tracker, 3, "cursor-three");

        assert!(matches!(
            frame.mark_delivered(),
            Err(SessionTransportError::StatePersistence(_))
        ));
        assert!(tracker.delivered_cursor_tokens().is_empty());
        assert!(frame.mark_delivered().unwrap());
        assert_eq!(tracker.delivered_cursor_tokens(), vec!["cursor-three"]);
    }

    #[test]
    fn delayed_delivery_keeps_its_stable_stream_identity_after_channel_reuse() {
        let tracker = DeliveryTracker::default();
        let mut frame = output_frame(&tracker, 3, "cursor-three");
        tracker
            .begin_connection(&[PublicStreamMapping {
                channel: 7,
                direction: PublicStreamDirection::Output,
                input_high_water: None,
                provisional_ref: None,
                stream_token: "replacement-stream".to_string(),
            }])
            .unwrap();

        assert!(frame.mark_delivered().unwrap());
        assert_eq!(
            tracker.snapshot().delivered_output_cursors,
            BTreeMap::from([("stream-seven".to_string(), "cursor-three".to_string())])
        );
    }

    #[test]
    fn attachment_persistence_failure_does_not_advance_and_can_be_retried() {
        let tracker = DeliveryTracker::new(
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: None,
                session_token: None,
            },
            Arc::new(FailOnceObserver {
                calls: AtomicUsize::new(0),
            }),
        );
        let pending = start_request(uuid::Uuid::new_v4());

        assert!(matches!(
            tracker.set_attachment_state(Some(pending.clone()), None),
            Err(SessionTransportError::StatePersistence(_))
        ));
        assert_eq!(tracker.snapshot().pending_operation, None);
        tracker
            .set_attachment_state(Some(pending.clone()), None)
            .unwrap();
        assert_eq!(tracker.snapshot().pending_operation, Some(pending));
    }

    #[test]
    fn stable_mapping_rejects_direction_changes_across_connections() {
        let tracker = DeliveryTracker::default();
        tracker
            .begin_connection(&[PublicStreamMapping {
                channel: 2,
                direction: PublicStreamDirection::Output,
                input_high_water: None,
                provisional_ref: None,
                stream_token: "stable-stream".to_string(),
            }])
            .unwrap();
        let error = tracker
            .begin_connection(&[PublicStreamMapping {
                channel: 3,
                direction: PublicStreamDirection::Input,
                input_high_water: Some(
                    golem_common::model::invocation_session_public::PublicInputHighWater {
                        sequence: DecimalU64(0),
                        terminal: false,
                    },
                ),
                provisional_ref: None,
                stream_token: "stable-stream".to_string(),
            }])
            .unwrap_err();
        assert!(matches!(
            error,
            SessionTransportError::StableMappingRebound { .. }
        ));
    }

    #[test]
    async fn affine_output_marks_delivery_when_polled_and_cancels_on_drop() {
        let tracker = DeliveryTracker::default();
        let (tx, rx) = mpsc::channel(1);
        tx.send(AgentStreamOutput::Item(
            5_u32,
            Some(output_frame(&tracker, 1, "cursor-one")),
        ))
        .await
        .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_on_drop = cancelled.clone();
        let mut stream =
            AgentStream::output(rx, move |_| cancelled_on_drop.store(true, Ordering::SeqCst));

        assert!(tracker.delivered_cursor_tokens().is_empty());
        assert_eq!(stream.next().await.unwrap().unwrap(), 5);
        assert_eq!(tracker.delivered_cursor_tokens(), vec!["cursor-one"]);
        drop(stream);
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    async fn affine_output_marks_clean_terminal_delivery() {
        let tracker = DeliveryTracker::default();
        let (tx, rx) = mpsc::channel::<AgentStreamOutput<u32>>(1);
        tx.send(AgentStreamOutput::End(Some(output_frame(
            &tracker,
            4,
            "terminal-cursor",
        ))))
        .await
        .unwrap();
        let mut stream = AgentStream::output(rx, |_| panic!("clean stream was cancelled"));

        assert!(stream.next().await.is_none());
        assert_eq!(tracker.delivered_cursor_tokens(), vec!["terminal-cursor"]);
    }

    #[test]
    async fn affine_output_marks_failed_terminal_delivery() {
        let tracker = DeliveryTracker::default();
        let (tx, rx) = mpsc::channel::<AgentStreamOutput<u32>>(1);
        tx.send(AgentStreamOutput::Error(
            AgentStreamError::Producer("failed".to_string()),
            Some(output_frame(&tracker, 5, "failed-terminal-cursor")),
        ))
        .await
        .unwrap();
        let mut stream = AgentStream::output(rx, |_| panic!("failed stream was cancelled"));

        assert!(matches!(
            stream.next().await,
            Some(Err(AgentStreamError::Producer(message))) if message == "failed"
        ));
        assert_eq!(
            tracker.delivered_cursor_tokens(),
            vec!["failed-terminal-cursor"]
        );
    }

    #[test]
    async fn affine_output_marks_cancelled_terminal_delivery() {
        let tracker = DeliveryTracker::default();
        let (tx, rx) = mpsc::channel::<AgentStreamOutput<u32>>(1);
        tx.send(AgentStreamOutput::Error(
            AgentStreamError::Cancelled("consumer-drop".to_string()),
            Some(output_frame(&tracker, 6, "cancelled-terminal-cursor")),
        ))
        .await
        .unwrap();
        let mut stream =
            AgentStream::output(rx, |_| panic!("cancelled stream was cancelled twice"));

        assert!(matches!(
            stream.next().await,
            Some(Err(AgentStreamError::Cancelled(reason))) if reason == "consumer-drop"
        ));
        assert_eq!(
            tracker.delivered_cursor_tokens(),
            vec!["cancelled-terminal-cursor"]
        );
    }

    #[test]
    fn packed_output_delivery_uses_the_last_logical_sequence() {
        let tracker = DeliveryTracker::default();
        tracker
            .begin_connection(&[PublicStreamMapping {
                channel: 3,
                direction: PublicStreamDirection::Output,
                input_high_water: None,
                provisional_ref: None,
                stream_token: "packed-stream".to_string(),
            }])
            .unwrap();
        let frame = ServerFrame::Binary(BinaryMessage {
            metadata: golem_common::model::invocation_session_public::BinaryMessageMetadata {
                channel: 3,
                cursor_token: Some("packed-cursor".to_string()),
                item_count: DecimalU64(4),
                kind: BinaryMessageKind::OutputU8,
                mime_type: None,
                sequence: DecimalU64(8),
                version: 1,
            },
            payload: vec![1, 2, 3, 4],
        });
        let mut first = tracker.delivery(&frame).unwrap().unwrap();
        first.mark().unwrap();

        let older = ServerFrame::Message(PublicServerMessage::OutputStreamEnd {
            channel: 3,
            cursor_token: Some("older".to_string()),
            outcome: PublicOutputStreamOutcome::Ok,
            sequence: DecimalU64(10),
            version: 1,
        });
        let error = tracker
            .delivery(&older)
            .unwrap()
            .unwrap()
            .mark()
            .unwrap_err();
        assert!(matches!(error, SessionTransportError::DeliveryOrder { .. }));
    }

    #[test]
    async fn generated_packed_u8_output_advances_only_after_the_final_byte() {
        let tracker = DeliveryTracker::default();
        tracker
            .begin_connection(&[PublicStreamMapping {
                channel: 3,
                direction: PublicStreamDirection::Output,
                input_high_water: None,
                provisional_ref: None,
                stream_token: "packed-stream".to_string(),
            }])
            .unwrap();
        let graph = SchemaGraph::anonymous(SchemaType::stream(Some(SchemaType::u8())));
        let (commands, _commands_rx) = mpsc::unbounded_channel();
        let context = GeneratedDecodeContext::new(graph, commands);
        let mut output = context
            .register_output(
                SchemaValueStream::from_host_endpoint(PublicStreamReference::Stable(
                    "packed-stream".to_string(),
                )),
                SchemaType::u8(),
                |value, _| match value {
                    SchemaValue::U8(byte) => Ok(byte),
                    _ => Err("expected u8".to_string()),
                },
            )
            .unwrap();
        let message = BinaryMessage {
            metadata: BinaryMessageMetadata {
                channel: 3,
                cursor_token: Some("packed-cursor".to_string()),
                item_count: DecimalU64(4),
                kind: BinaryMessageKind::OutputU8,
                mime_type: None,
                sequence: DecimalU64(8),
                version: 1,
            },
            payload: vec![1, 2, 3, 4],
        };
        let server_frame = ServerFrame::Binary(message.clone());
        let frame = ReceivedFrame {
            delivery: tracker.delivery(&server_frame).unwrap(),
            frame: server_frame,
            _admission: permit(),
        };
        let mut sink = context
            .outputs
            .lock()
            .unwrap()
            .remove("packed-stream")
            .unwrap();
        sink.binary(message, frame, &context).await.unwrap();

        for expected in [1, 2, 3] {
            assert_eq!(output.next().await.unwrap().unwrap(), expected);
            assert!(tracker.delivered_cursor_tokens().is_empty());
        }
        assert_eq!(output.next().await.unwrap().unwrap(), 4);
        assert_eq!(tracker.delivered_cursor_tokens(), vec!["packed-cursor"]);
    }

    #[test]
    fn generated_duplicate_output_registration_preserves_the_original_consumer() {
        let graph = SchemaGraph::anonymous(SchemaType::stream(Some(SchemaType::u8())));
        let (commands, mut commands_rx) = mpsc::unbounded_channel();
        let context = GeneratedDecodeContext::new(graph, commands);
        let first = context
            .register_output(
                SchemaValueStream::from_host_endpoint(PublicStreamReference::Stable(
                    "same-stream".to_string(),
                )),
                SchemaType::u8(),
                |value, _| Ok(value),
            )
            .unwrap();
        let duplicate = context.register_output(
            SchemaValueStream::from_host_endpoint(PublicStreamReference::Stable(
                "same-stream".to_string(),
            )),
            SchemaType::u8(),
            |value, _| Ok(value),
        );

        assert_eq!(
            duplicate.err().unwrap(),
            "output stream reference was consumed more than once"
        );
        assert!(context.outputs.lock().unwrap().contains_key("same-stream"));
        drop(first);
        assert!(matches!(
            commands_rx.try_recv(),
            Ok(GeneratedCommand::CancelOutput {
                stream_token,
                reason: PublicClientCancelReason::ConsumerDrop,
            }) if stream_token == "same-stream"
        ));
    }

    #[test]
    fn generated_completion_admits_queued_cancellations_before_finishing() {
        let output_graph = SchemaGraph::anonymous(SchemaType::stream(Some(SchemaType::string())));
        let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
        let output_context = GeneratedDecodeContext::new(output_graph, commands_tx);
        let output = output_context
            .register_output(
                SchemaValueStream::from_host_endpoint(PublicStreamReference::Stable(
                    "output-stream".to_string(),
                )),
                SchemaType::string(),
                |value, _| Ok(value),
            )
            .unwrap();
        drop(output);

        let provisional_ref = uuid::Uuid::new_v4();
        let inputs = HashMap::from([(
            provisional_ref,
            InputReplayBuffer::new(GENERATED_INPUT_BYTES, GENERATED_INPUT_ITEMS),
        )]);
        let (input_tx, mut input_rx) = mpsc::unbounded_channel();
        input_tx
            .send(GeneratedInputEvent::Failed {
                provisional_ref,
                message: "source failed".to_string(),
            })
            .unwrap();
        let mut pending_output_cancellations = HashMap::new();
        let mut cancelled_outputs = HashSet::new();
        let mut failed_inputs = HashMap::new();

        drain_generated_events(
            &mut commands_rx,
            &mut input_rx,
            Some(&output_context),
            &inputs,
            &mut pending_output_cancellations,
            &mut cancelled_outputs,
            &mut failed_inputs,
        );

        assert_eq!(
            pending_output_cancellations.get("output-stream"),
            Some(&PublicClientCancelReason::ConsumerDrop)
        );
        assert!(cancelled_outputs.contains("output-stream"));
        assert_eq!(
            failed_inputs.get(&provisional_ref),
            Some(&"source failed".to_string())
        );
        assert!(!generated_invocation_is_complete::<()>(
            &None,
            Some(&output_context),
            &pending_output_cancellations,
            &failed_inputs,
        ));
    }

    #[test]
    async fn closed_generated_output_delivery_retains_stable_cancellation_intent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: vec![PublicStreamMapping {
                            channel: 3,
                            direction: PublicStreamDirection::Output,
                            input_high_water: None,
                            provisional_ref: None,
                            stream_token: "output-stream".to_string(),
                        }],
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            while let Some(frame) = socket.next().await {
                if matches!(frame.unwrap(), Message::Close(_)) {
                    break;
                }
            }
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        session.receive().await.unwrap();

        let output_graph = SchemaGraph::anonymous(SchemaType::stream(Some(SchemaType::string())));
        let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
        let output_context = GeneratedDecodeContext::new(output_graph.clone(), commands_tx);
        let output = output_context
            .register_output(
                SchemaValueStream::from_host_endpoint(PublicStreamReference::Stable(
                    "output-stream".to_string(),
                )),
                SchemaType::string(),
                |value, _| Ok(value),
            )
            .unwrap();
        drop(output);

        let server_frame = ServerFrame::Message(PublicServerMessage::OutputStreamItem {
            channel: 3,
            cursor_token: "cursor-one".to_string(),
            mappings: Vec::new(),
            sequence: DecimalU64(0),
            value: serde_json::json!("seven"),
            version: 1,
        });
        let frame = ReceivedFrame {
            delivery: session.delivery_tracker().delivery(&server_frame).unwrap(),
            frame: server_frame,
            _admission: permit(),
        };
        let mut decode: Option<Box<GeneratedResultDecoder<()>>> = None;
        let mut result_tx: Option<tokio::sync::oneshot::Sender<Result<(), SessionTransportError>>> =
            None;
        let mut inputs = HashMap::new();
        let mut input_tasks = HashMap::new();
        let mut pending_output_cancellations = HashMap::new();
        let mut cancelled_outputs = HashSet::new();

        assert!(
            !handle_generated_frame(
                frame,
                &mut session,
                Some(&output_graph),
                Some(&output_context),
                &mut decode,
                &mut result_tx,
                &mut inputs,
                &mut input_tasks,
                &mut pending_output_cancellations,
                &mut cancelled_outputs,
            )
            .await
            .unwrap()
        );
        assert_eq!(
            pending_output_cancellations.get("output-stream"),
            Some(&PublicClientCancelReason::ConsumerDrop)
        );
        assert!(cancelled_outputs.contains("output-stream"));

        let command = commands_rx.try_recv().unwrap();
        admit_generated_command(
            command,
            Some(&output_context),
            &mut pending_output_cancellations,
            &mut cancelled_outputs,
        );
        assert_eq!(
            pending_output_cancellations.get("output-stream"),
            Some(&PublicClientCancelReason::ConsumerDrop)
        );

        session.close().await.unwrap();
        server.await.unwrap();
    }

    #[test]
    async fn generated_u8_input_packs_bytes_and_preserves_natural_end() {
        let graph = SchemaGraph::anonymous(SchemaType::stream(Some(SchemaType::u8())));
        let context = GeneratedEncodeContext::new(graph.clone());
        let stream = AgentStream::input(futures_util::stream::iter([
            Ok::<_, String>(1_u8),
            Ok(2_u8),
            Ok(3_u8),
        ]));
        let value = context
            .register_input(stream, SchemaType::u8(), |byte, _| {
                Ok(SchemaValue::U8(byte))
            })
            .unwrap();
        context.encode_value(&graph.root, &value).unwrap();
        let pending = context.take_pending().pop().unwrap();
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let task = pending
            .source
            .spawn(pending.provisional_ref, pending.buffer, context, events);

        let GeneratedInputEvent::Input { input, .. } = events_rx.recv().await.unwrap() else {
            panic!("expected packed input")
        };
        let ReplayableInput::Binary(message) = input.request else {
            panic!("expected binary input lane")
        };
        assert_eq!(message.metadata.kind, BinaryMessageKind::InputU8);
        assert_eq!(message.metadata.item_count, DecimalU64(3));
        assert_eq!(message.payload, vec![1, 2, 3]);

        let GeneratedInputEvent::Input { input, .. } = events_rx.recv().await.unwrap() else {
            panic!("expected natural input end")
        };
        assert!(matches!(
            input.request,
            ReplayableInput::End { sequence: 3 }
        ));
        task.abort();
    }

    #[test]
    async fn shared_runtime_retries_the_exact_pending_attempt_after_lost_acceptance() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let expected = start_request(attempt_id);
        let server_expected = expected.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_expected);
            socket.close(None).await.unwrap();

            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            let retry = receive_client_message(&mut socket).await;
            assert_eq!(retry, server_expected);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: Vec::new(),
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(expected),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(5), session.receive())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            frame.frame(),
            ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. })
        ));
        assert_eq!(
            session.attachment_state,
            AttachmentState::Accepted {
                attempt_id,
                replay_revocation_eligible: true,
            }
        );
        server.await.unwrap();
    }

    #[test]
    async fn shared_runtime_recovers_an_outbound_send_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: Vec::new(),
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            socket.close(None).await.unwrap();

            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            let resume = receive_client_message(&mut socket).await;
            let PublicClientMessage::ResumeAttach {
                attempt_id,
                operation,
                output_cursors,
                session_token,
                ..
            } = resume
            else {
                panic!("expected resume after outbound failure")
            };
            assert_eq!(operation, PublicResumeOperation::Resume);
            assert!(output_cursors.is_empty());
            assert_eq!(session_token, "session-token");
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: Vec::new(),
                        session_token: "session-token-2".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        session.receive().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let sender = session.sender();
        let error = loop {
            match sender
                .send_message(&PublicClientMessage::StreamCancel {
                    channel: 1,
                    reason: golem_common::model::invocation_session_public::PublicClientCancelReason::Cancelled,
                    version: 1,
                })
                .await
            {
                Ok(()) => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(error) => break error,
            }
        };
        tokio::time::timeout(
            Duration::from_secs(5),
            session.recover_after_send_failure(error),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            session.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. })
        ));
        assert!(matches!(
            session.attachment_state,
            AttachmentState::Accepted {
                replay_revocation_eligible: false,
                ..
            }
        ));
        server.await.unwrap();
    }

    #[test]
    async fn generated_output_cancellation_is_retried_on_the_fresh_resume_channel() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: vec![PublicStreamMapping {
                            channel: 3,
                            direction: PublicStreamDirection::Output,
                            input_high_water: None,
                            provisional_ref: None,
                            stream_token: "output-stream".to_string(),
                        }],
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            while let Some(frame) = socket.next().await {
                if matches!(frame.unwrap(), Message::Close(_)) {
                    break;
                }
            }
            let _ = closed_tx.send(());

            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            let resume = receive_client_message(&mut socket).await;
            let PublicClientMessage::ResumeAttach {
                attempt_id,
                session_token,
                ..
            } = resume
            else {
                panic!("expected resume after cancellation send failure")
            };
            assert_eq!(session_token, "session-token");
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: vec![PublicStreamMapping {
                            channel: 9,
                            direction: PublicStreamDirection::Output,
                            input_high_water: None,
                            provisional_ref: None,
                            stream_token: "output-stream".to_string(),
                        }],
                        session_token: "session-token-2".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            assert!(matches!(
                receive_client_message(&mut socket).await,
                PublicClientMessage::StreamCancel {
                    channel: 9,
                    reason: PublicClientCancelReason::ConsumerDrop,
                    ..
                }
            ));
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        session.receive().await.unwrap();
        session.close().await.unwrap();
        closed_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut pending = HashMap::from([(
            "output-stream".to_string(),
            PublicClientCancelReason::ConsumerDrop,
        )]);
        cancel_pending_outputs(&mut session, &mut pending)
            .await
            .unwrap();
        assert!(pending.contains_key("output-stream"));
        assert!(matches!(
            session.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. })
        ));
        cancel_pending_outputs(&mut session, &mut pending)
            .await
            .unwrap();
        assert!(pending.is_empty());
        server.await.unwrap();
    }

    #[test]
    async fn generated_input_failure_cancellation_is_retried_after_resume() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let provisional_ref = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: vec![PublicStreamMapping {
                            channel: 4,
                            direction: PublicStreamDirection::Input,
                            input_high_water: Some(PublicInputHighWater {
                                sequence: DecimalU64(0),
                                terminal: false,
                            }),
                            provisional_ref: Some(provisional_ref),
                            stream_token: "input-stream".to_string(),
                        }],
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            while let Some(frame) = socket.next().await {
                if matches!(frame.unwrap(), Message::Close(_)) {
                    break;
                }
            }
            let _ = closed_tx.send(());

            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            let resume = receive_client_message(&mut socket).await;
            let PublicClientMessage::ResumeAttach {
                attempt_id,
                session_token,
                ..
            } = resume
            else {
                panic!("expected resume after input cancellation send failure")
            };
            assert_eq!(session_token, "session-token");
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: vec![PublicStreamMapping {
                            channel: 10,
                            direction: PublicStreamDirection::Input,
                            input_high_water: Some(PublicInputHighWater {
                                sequence: DecimalU64(0),
                                terminal: false,
                            }),
                            provisional_ref: Some(provisional_ref),
                            stream_token: "input-stream".to_string(),
                        }],
                        session_token: "session-token-2".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            assert!(matches!(
                receive_client_message(&mut socket).await,
                PublicClientMessage::StreamCancel {
                    channel: 10,
                    reason: PublicClientCancelReason::SourceUnavailable,
                    ..
                }
            ));
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        let buffer = InputReplayBuffer::new(GENERATED_INPUT_BYTES, GENERATED_INPUT_ITEMS);
        session
            .register_input(provisional_ref, None, buffer.clone())
            .unwrap();
        session.receive().await.unwrap();
        session.close().await.unwrap();
        closed_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let inputs = HashMap::from([(provisional_ref, buffer)]);
        let mut failed = HashMap::from([(provisional_ref, "source failed".to_string())]);
        cancel_failed_inputs(&mut session, &inputs, &mut failed)
            .await
            .unwrap();
        assert!(failed.contains_key(&provisional_ref));
        assert!(matches!(
            session.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. })
        ));
        cancel_failed_inputs(&mut session, &inputs, &mut failed)
            .await
            .unwrap();
        assert!(failed.is_empty());
        server.await.unwrap();
    }

    #[test]
    async fn rejection_is_terminal_and_cannot_create_a_fresh_resume() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationRejected {
                        attempt_id: Some(attempt_id),
                        code: PublicErrorCode::ValidationError,
                        message: "rejected".to_string(),
                        retryable: false,
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            socket.close(None).await.unwrap();
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        assert!(matches!(
            session.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::InvocationRejected { .. })
        ));
        assert!(matches!(
            session.receive().await,
            Err(SessionTransportError::AttachmentTerminated)
        ));
        server.await.unwrap();
    }

    #[test]
    async fn post_acceptance_rejection_is_terminal_for_the_accepted_attempt() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: Vec::new(),
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationRejected {
                        attempt_id: Some(attempt_id),
                        code: PublicErrorCode::InputConflict,
                        message: "input conflict".to_string(),
                        retryable: true,
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            socket.close(None).await.unwrap();
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        session.receive().await.unwrap();
        assert!(matches!(
            session.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::InvocationRejected { .. })
        ));
        assert_eq!(session.attachment_state, AttachmentState::Terminal);
        assert!(matches!(
            session.receive().await,
            Err(SessionTransportError::AttachmentTerminated)
        ));
        server.await.unwrap();
    }

    #[test]
    async fn checkpoint_loaded_retry_recovers_an_immediate_replay_revocation() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: Vec::new(),
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::AttachmentRevoked {
                        reason: PublicAttachmentRevokedReason::Replaced,
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();

            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            let resume = receive_client_message(&mut socket).await;
            let PublicClientMessage::ResumeAttach {
                attempt_id: resume_attempt_id,
                operation,
                session_token,
                ..
            } = resume
            else {
                panic!("expected fresh resume after replay revocation")
            };
            assert_ne!(resume_attempt_id, attempt_id);
            assert_eq!(operation, PublicResumeOperation::Resume);
            assert_eq!(session_token, "session-token");
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id: resume_attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: Vec::new(),
                        session_token: "session-token-2".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            true,
            Arc::new(()),
        )
        .await
        .unwrap();
        session.receive().await.unwrap();
        assert!(matches!(
            session.attachment_state,
            AttachmentState::Accepted {
                replay_revocation_eligible: true,
                ..
            }
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), session.receive())
                .await
                .unwrap()
                .unwrap()
                .frame(),
            ServerFrame::Message(PublicServerMessage::InvocationAccepted { .. })
        ));
        assert!(matches!(
            session.attachment_state,
            AttachmentState::Accepted {
                replay_revocation_eligible: false,
                ..
            }
        ));
        server.await.unwrap();
    }

    #[test]
    async fn active_frame_expires_replay_revocation_recovery() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            for message in [
                PublicServerMessage::InvocationAccepted {
                    attempt_id,
                    idempotency_key: "invocation-key".to_string(),
                    mappings: Vec::new(),
                    session_token: "session-token".to_string(),
                    version: 1,
                },
                PublicServerMessage::InvocationResult {
                    mappings: Vec::new(),
                    result: PublicInvocationResult::None,
                    version: 1,
                },
                PublicServerMessage::AttachmentRevoked {
                    reason: PublicAttachmentRevokedReason::Replaced,
                    version: 1,
                },
            ] {
                socket
                    .send(Message::Text(encode_text(&message).unwrap().into()))
                    .await
                    .unwrap();
            }
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            true,
            Arc::new(()),
        )
        .await
        .unwrap();
        session.receive().await.unwrap();
        assert!(matches!(
            session.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::InvocationResult { .. })
        ));
        assert!(matches!(
            session.attachment_state,
            AttachmentState::Accepted {
                replay_revocation_eligible: false,
                ..
            }
        ));
        assert!(matches!(
            session.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::AttachmentRevoked { .. })
        ));
        assert_eq!(session.attachment_state, AttachmentState::Terminal);
        server.await.unwrap();
    }

    #[test]
    async fn resumed_input_fallback_cannot_replace_a_known_stable_token() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: vec![PublicStreamMapping {
                            channel: 9,
                            direction: PublicStreamDirection::Input,
                            input_high_water: Some(PublicInputHighWater {
                                sequence: DecimalU64(3),
                                terminal: false,
                            }),
                            provisional_ref: None,
                            stream_token: "replacement-input".to_string(),
                        }],
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        session
            .register_input(
                uuid::Uuid::new_v4(),
                Some("known-input".to_string()),
                InputReplayBuffer::new(16, 1),
            )
            .unwrap();
        assert!(matches!(
            session.receive().await,
            Err(SessionTransportError::Protocol(_))
        ));
        server.await.unwrap();
    }

    #[test]
    async fn resumed_input_fallback_recovers_an_unpersisted_stable_token() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempt_id = uuid::Uuid::new_v4();
        let initial = start_request(attempt_id);
        let server_initial = initial.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(socket, |request: &Request, response: Response| {
                Ok(select_session_subprotocol(request, response))
            })
            .await
            .unwrap();
            assert_eq!(receive_client_message(&mut socket).await, server_initial);
            socket
                .send(Message::Text(
                    encode_text(&PublicServerMessage::InvocationAccepted {
                        attempt_id,
                        idempotency_key: "invocation-key".to_string(),
                        mappings: vec![PublicStreamMapping {
                            channel: 9,
                            direction: PublicStreamDirection::Input,
                            input_high_water: Some(PublicInputHighWater {
                                sequence: DecimalU64(3),
                                terminal: false,
                            }),
                            provisional_ref: None,
                            stream_token: "stable-input".to_string(),
                        }],
                        session_token: "session-token".to_string(),
                        version: 1,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let buffer = InputReplayBuffer::new(16, 1);
        let mut session = InvocationSession::open(
            Arc::new(StaticRequestProvider(format!("ws://{address}"))),
            None,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(initial),
                session_token: None,
            },
            false,
            Arc::new(()),
        )
        .await
        .unwrap();
        session
            .register_input(uuid::Uuid::new_v4(), None, buffer.clone())
            .unwrap();
        session.receive().await.unwrap();
        assert_eq!(
            session.input_binding(&buffer),
            Some((9, "stable-input".to_string()))
        );
        server.await.unwrap();
    }

    #[test]
    async fn transport_requires_and_validates_the_selected_subprotocol() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket =
                accept_hdr_async(socket, |request: &Request, mut response: Response| {
                    assert_eq!(
                        request
                            .headers()
                            .get(SEC_WEBSOCKET_PROTOCOL)
                            .unwrap()
                            .to_str()
                            .unwrap(),
                        INVOCATION_SESSION_SUBPROTOCOL
                    );
                    response.headers_mut().insert(
                        SEC_WEBSOCKET_PROTOCOL,
                        INVOCATION_SESSION_SUBPROTOCOL.parse().unwrap(),
                    );
                    Ok(response)
                })
                .await
                .unwrap();
            let message = PublicServerMessage::InvocationFinished {
                outcome: PublicInvocationOutcome::Success,
                version: 1,
            };
            socket
                .send(Message::Text(encode_text(&message).unwrap().into()))
                .await
                .unwrap();
        });

        let mut transport = InvocationSessionTransport::connect(format!("ws://{address}"), None)
            .await
            .unwrap();
        assert!(matches!(
            transport.receive().await.unwrap().frame(),
            ServerFrame::Message(PublicServerMessage::InvocationFinished { .. })
        ));
        server.await.unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            accept_hdr_async(socket, |_request: &Request, response: Response| {
                Ok(response)
            })
            .await
            .unwrap();
        });
        let error = match InvocationSessionTransport::connect(format!("ws://{address}"), None).await
        {
            Ok(_) => panic!("connection without a selected subprotocol succeeded"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            SessionTransportError::UnsupportedSubprotocol
        ));
        server.await.unwrap();
    }
}
