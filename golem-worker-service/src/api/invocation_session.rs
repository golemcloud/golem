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

use crate::invocation_session_token::{
    CursorTokenPayload, InvocationSessionTokenBindings, InvocationSessionTokenKeyring,
    InvocationSessionTokenKind, InvocationSessionTokenPayload, SessionAgentIdentity,
    SessionTokenPayload, StreamTokenPayload, StreamTokenRole, decode_session_agent_identity,
    encode_session_agent_identity,
};
use crate::service::auth::AuthServiceError;
use crate::service::worker::{
    PublicAgentSessionResume, PublicAgentSessionStart, PublicAgentSessionStartError,
    StartedPublicAgentSession, WorkerService, WorkerServiceError,
    decode_public_session_schema_value,
};
use futures::{SinkExt, StreamExt};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, InputStreamEnd, InputStreamItem, InvocationAccepted,
    InvocationRejectionReason, InvocationRequest, InvocationResponse, InvocationSessionResult,
    OutputStreamEnd, OutputStreamError, OutputStreamItem, ResumeOperation, StreamCancel,
    StreamCancelReason, StreamCancelRole, StreamCursor, StreamMappingRole, input_stream_item,
    invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result,
};
use golem_common::SafeDisplay;
use golem_common::model::invocation_session_public::{
    BinaryMessage, BinaryMessageKind, BinaryMessageMetadata, DecimalU64, MAX_LOGICAL_VALUE_SIZE,
    MAX_STREAM_MAPPINGS, MAX_WEBSOCKET_MESSAGE_SIZE, PublicAttachmentRevokedReason,
    PublicClientCancelReason, PublicClientMessage, PublicErrorCode, PublicInputHighWater,
    PublicInvocationOutcome, PublicInvocationResult, PublicOutputStreamOutcome,
    PublicResumeOperation, PublicServerCancelReason, PublicServerMessage, PublicStreamDirection,
    PublicStreamMapping, decode_binary_message, decode_client_text, encode_binary_message,
    encode_text,
};
use golem_common::schema::fingerprint::{
    SchemaFingerprintV1, resolve_stream_element_schema_v1, schema_fingerprint_v1,
};
use golem_common::schema::public_json::{
    PublicSchemaValueError, PublicStreamReference, PublicStreamReferencePolicy,
    decode_public_schema_value_with_charge, encode_public_schema_value_with_charge,
};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::validation::validate_value;
use golem_common::schema::{
    BinaryValuePayload, SchemaGraph, SchemaType, SchemaValue, schema_value_to_proto_with_streams,
};
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::model::auth::AuthCtx;
use poem::web::websocket::{CloseCode, Message, WebSocketStream};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc};
use uuid::Uuid;

const CHANNEL_CAPACITY: usize = 16;
const WRITE_PROGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a session whose client half ended first waits for the private session to end before
/// the WebSocket is closed. Ending the client half closes the private request stream, and the
/// executor persists the durable transport detach before it ends the private response stream, so
/// a client that resumes right after the close handshake completes finds the session detached.
const CLIENT_GONE_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const SESSION_BYTE_BUDGET: usize = 64 * 1024 * 1024;
const OUTPUT_BYTE_BUDGET: usize = 32 * 1024 * 1024;
const INPUT_UNACKNOWLEDGED_BYTE_BUDGET: usize = 16 * 1024 * 1024;
const STREAM_BYTE_BUDGET: usize = 16 * 1024 * 1024;
const STREAM_ITEM_BUDGET: usize = 256;

struct AdmittedRequest {
    request: InvocationRequest,
    _session: OwnedSemaphorePermit,
}

struct PendingInputAdmission {
    terminal: bool,
    _unacknowledged: OwnedSemaphorePermit,
    _stream_bytes: OwnedSemaphorePermit,
    _stream_item: OwnedSemaphorePermit,
}

struct QueuedMessage {
    message: Message,
    stream_channel: Option<u32>,
    drop_if_output_cancelled: bool,
    _session: OwnedSemaphorePermit,
    _output: OwnedSemaphorePermit,
    _stream_bytes: Option<OwnedSemaphorePermit>,
    _stream_item: Option<OwnedSemaphorePermit>,
}

impl QueuedMessage {
    async fn should_drop(&self, budgets: &SessionBudgets) -> bool {
        if !self.drop_if_output_cancelled {
            return false;
        }
        let Some(channel) = self.stream_channel else {
            return false;
        };
        budgets.cancelled_outputs.lock().await.contains(&channel)
    }
}

#[derive(Clone)]
struct SessionBudgets {
    session: Arc<Semaphore>,
    output: Arc<Semaphore>,
    unacknowledged_input: Arc<Semaphore>,
    stream_bytes: Arc<Mutex<HashMap<u32, Arc<Semaphore>>>>,
    stream_items: Arc<Mutex<HashMap<u32, Arc<Semaphore>>>>,
    cancelled_outputs: Arc<Mutex<HashSet<u32>>>,
}

impl SessionBudgets {
    fn new() -> Self {
        Self {
            session: Arc::new(Semaphore::new(SESSION_BYTE_BUDGET)),
            output: Arc::new(Semaphore::new(OUTPUT_BYTE_BUDGET)),
            unacknowledged_input: Arc::new(Semaphore::new(INPUT_UNACKNOWLEDGED_BYTE_BUDGET)),
            stream_bytes: Arc::new(Mutex::new(HashMap::new())),
            stream_items: Arc::new(Mutex::new(HashMap::new())),
            cancelled_outputs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    async fn stream_budget(
        budgets: &Mutex<HashMap<u32, Arc<Semaphore>>>,
        channel: u32,
        permits: usize,
    ) -> Arc<Semaphore> {
        budgets
            .lock()
            .await
            .entry(channel)
            .or_insert_with(|| Arc::new(Semaphore::new(permits)))
            .clone()
    }
}

#[derive(Clone)]
struct Outbound {
    sender: mpsc::Sender<QueuedMessage>,
    budgets: SessionBudgets,
}

impl Outbound {
    async fn send(&self, message: Message, stream: Option<(u32, usize)>) -> Result<(), ()> {
        self.send_with_policy(message, stream, true).await
    }

    async fn send_stream_terminal(
        &self,
        message: Message,
        channel: u32,
        byte_charge: usize,
    ) -> Result<(), ()> {
        self.send_with_policy(message, Some((channel, byte_charge)), false)
            .await
    }

    async fn send_with_policy(
        &self,
        message: Message,
        stream: Option<(u32, usize)>,
        drop_if_output_cancelled: bool,
    ) -> Result<(), ()> {
        let stream_channel = stream.map(|(channel, _)| channel);
        if drop_if_output_cancelled
            && let Some(channel) = stream_channel
            && self
                .budgets
                .cancelled_outputs
                .lock()
                .await
                .contains(&channel)
        {
            return Ok(());
        }
        let bytes = message_size(&message).max(1);
        let session = acquire_bytes(&self.budgets.session, bytes).await?;
        let output = acquire_bytes(&self.budgets.output, bytes).await?;
        let (stream_bytes, stream_item) = if let Some((channel, byte_charge)) = stream {
            let bytes_budget = SessionBudgets::stream_budget(
                &self.budgets.stream_bytes,
                channel,
                STREAM_BYTE_BUDGET,
            )
            .await;
            let item_budget = SessionBudgets::stream_budget(
                &self.budgets.stream_items,
                channel,
                STREAM_ITEM_BUDGET,
            )
            .await;
            (
                Some(acquire_bytes(&bytes_budget, byte_charge.max(1)).await?),
                Some(acquire_bytes(&item_budget, 1).await?),
            )
        } else {
            (None, None)
        };
        self.sender
            .send(QueuedMessage {
                message,
                stream_channel,
                drop_if_output_cancelled,
                _session: session,
                _output: output,
                _stream_bytes: stream_bytes,
                _stream_item: stream_item,
            })
            .await
            .map_err(|_| ())
    }

    async fn cancel_output(&self, channel: u32) {
        self.budgets.cancelled_outputs.lock().await.insert(channel);
    }
}

struct TranslatedFrame {
    message: Message,
    stream_channel: Option<u32>,
    stream_byte_charge: Option<usize>,
    preserve_after_output_cancel: bool,
    cancel_output_before_send: Option<u32>,
}

struct TranslatedRequest {
    request: InvocationRequest,
    input: Option<PendingInput>,
    cancelled_output: Option<u32>,
}

struct PendingInput {
    channel: u32,
    terminal: bool,
    byte_charge: usize,
}

#[derive(Debug)]
struct AdapterError {
    code: PublicErrorCode,
    message: String,
}

impl AdapterError {
    fn new(code: PublicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self::new(PublicErrorCode::ProtocolError, message)
    }
}

fn request_protocol_error(message: String) -> AdapterError {
    let code = if message.contains("conflict") || message.contains("overlaps") {
        PublicErrorCode::InputConflict
    } else if message.contains("expected sequence")
        || message.contains("expected terminal offset")
        || message.contains("expected discarded sequence")
        || message.contains("expected discarded terminal offset")
    {
        PublicErrorCode::InputGap
    } else {
        PublicErrorCode::ProtocolError
    };
    AdapterError::new(code, message)
}

impl From<golem_common::model::invocation_session_public::PublicProtocolError> for AdapterError {
    fn from(value: golem_common::model::invocation_session_public::PublicProtocolError) -> Self {
        Self::new(value.code, value.message)
    }
}

impl From<PublicSchemaValueError> for AdapterError {
    fn from(value: PublicSchemaValueError) -> Self {
        Self::new(value.code, value.message)
    }
}

enum InitialMessage {
    Start {
        start: PublicAgentSessionStart,
        attempt_id: Uuid,
        _admission: OwnedSemaphorePermit,
    },
    Resume {
        resume: PublicAgentSessionResume,
        attempt_id: Uuid,
        token_bindings: InvocationSessionTokenBindings,
        token_key_id: String,
        _admission: OwnedSemaphorePermit,
    },
}

#[derive(Clone)]
struct PrivateMapping {
    mapping: DurableStreamMapping,
    durable_stream_id: Uuid,
    schema_fingerprint: [u8; 32],
    direction: PublicStreamDirection,
}

struct ChannelState {
    transport_id: u64,
    direction: PublicStreamDirection,
    schema: SchemaType,
    provisional_ref: Option<Uuid>,
    durable: Option<PrivateMapping>,
    stream_token: Option<String>,
    next_input_sequence: u64,
    pending_input: VecDeque<PendingInputAdmission>,
    exposed: bool,
    terminal: bool,
}

struct TokenContext {
    bindings: InvocationSessionTokenBindings,
    key_id: String,
    logical_invocation_id: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProvisionalOwner {
    Initial,
    InputItem { channel: u32, sequence: u64 },
}

struct ProvisionalBinding {
    owner: ProvisionalOwner,
    schema: SchemaType,
    transport_id: u64,
}

struct AdapterState {
    protocol_state: InvocationSessionState,
    graph: Option<SchemaGraph>,
    output_schema: Option<SchemaType>,
    identity: Option<SessionAgentIdentity>,
    application: Option<String>,
    environment: Option<String>,
    next_channel: u32,
    next_transport_id: u64,
    channels: HashMap<u32, ChannelState>,
    channel_by_transport: HashMap<u64, u32>,
    provisional_refs: HashMap<Uuid, ProvisionalBinding>,
    private_mappings: HashMap<u64, PrivateMapping>,
    tokens: Option<TokenContext>,
    attachment_epoch: u64,
}

impl AdapterState {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_first_channel(1)
    }

    fn new_connection(connection_id: Uuid) -> Self {
        let range_size = MAX_STREAM_MAPPINGS as u128;
        let range_count = u32::MAX as u128 / range_size;
        let range = connection_id.as_u128() % range_count;
        Self::with_first_channel((range * range_size + 1) as u32)
    }

    fn with_first_channel(first_channel: u32) -> Self {
        Self {
            protocol_state: InvocationSessionState::default(),
            graph: None,
            output_schema: None,
            identity: None,
            application: None,
            environment: None,
            next_channel: first_channel,
            next_transport_id: 1,
            channels: HashMap::new(),
            channel_by_transport: HashMap::new(),
            provisional_refs: HashMap::new(),
            private_mappings: HashMap::new(),
            tokens: None,
            attachment_epoch: 0,
        }
    }

    fn initialize(&mut self, started: &StartedPublicAgentSession) -> Result<(), AdapterError> {
        self.protocol_state
            .validate_trusted_request(&started.initial_request)
            .map_err(AdapterError::protocol)?;
        self.graph = Some(started.schema.clone());
        self.output_schema = started.output_schema.clone();
        self.application = Some(started.application.0.clone());
        self.environment = Some(started.environment.0.clone());
        self.identity = Some(SessionAgentIdentity {
            component_id: started.agent_id.component_id.0,
            component_revision: started.component_revision.get(),
            agent_type: started.agent_type.0.clone(),
            agent_id: started.agent_id.agent_id.clone(),
            method: started.method.clone(),
        });
        Ok(())
    }

    fn allocate_channel(&mut self) -> Result<u32, PublicSchemaValueError> {
        if self.channels.len() >= MAX_STREAM_MAPPINGS {
            return Err(PublicSchemaValueError::new(
                PublicErrorCode::ResourceExhausted,
                "stream mapping limit exceeded",
            ));
        }
        let channel = self.next_channel;
        self.next_channel = self.next_channel.checked_add(1).ok_or_else(|| {
            PublicSchemaValueError::new(
                PublicErrorCode::ResourceExhausted,
                "public channel space exhausted",
            )
        })?;
        Ok(channel)
    }

    fn allocate_transport_id(&mut self) -> Result<u64, PublicSchemaValueError> {
        loop {
            let transport_id = self.next_transport_id;
            self.next_transport_id = self.next_transport_id.checked_add(1).ok_or_else(|| {
                PublicSchemaValueError::new(
                    PublicErrorCode::ResourceExhausted,
                    "private transport stream ID space exhausted",
                )
            })?;
            if !self.channel_by_transport.contains_key(&transport_id)
                && !self.private_mappings.contains_key(&transport_id)
            {
                return Ok(transport_id);
            }
        }
    }

    fn register_provisional(
        &mut self,
        reference: PublicStreamReference,
        schema: Option<&SchemaType>,
        owner: ProvisionalOwner,
    ) -> Result<SchemaValueStream, PublicSchemaValueError> {
        let PublicStreamReference::Provisional(provisional_ref) = reference else {
            return Err(PublicSchemaValueError::new(
                PublicErrorCode::TokenInvalid,
                "stable stream tokens are not valid before acceptance",
            ));
        };
        let schema = schema.cloned().ok_or_else(|| {
            PublicSchemaValueError::new(
                PublicErrorCode::UnsupportedValue,
                "dynamically typed streams cannot cross the public boundary",
            )
        })?;
        if let Some(existing) = self.provisional_refs.get(&provisional_ref) {
            if existing.owner == owner && existing.schema == schema {
                return Ok(SchemaValueStream::from_host_endpoint(existing.transport_id));
            }
            let (code, message) = if existing.owner == owner {
                (
                    PublicErrorCode::StreamConflict,
                    "provisional stream reference schema changed during an exact retry",
                )
            } else {
                (
                    PublicErrorCode::StreamAlreadyConsumed,
                    "provisional stream reference was rebound to another input item",
                )
            };
            return Err(PublicSchemaValueError::new(code, message));
        }
        let channel = self.allocate_channel()?;
        let transport_id = self.allocate_transport_id()?;
        self.channel_by_transport.insert(transport_id, channel);
        self.provisional_refs.insert(
            provisional_ref,
            ProvisionalBinding {
                owner,
                schema: schema.clone(),
                transport_id,
            },
        );
        self.channels.insert(
            channel,
            ChannelState {
                transport_id,
                direction: PublicStreamDirection::Input,
                schema,
                provisional_ref: Some(provisional_ref),
                durable: None,
                stream_token: None,
                next_input_sequence: 0,
                pending_input: VecDeque::new(),
                exposed: false,
                terminal: false,
            },
        );
        Ok(SchemaValueStream::from_host_endpoint(transport_id))
    }

    fn rollback_provisional_registrations(
        &mut self,
        existing: &HashSet<Uuid>,
        next_channel: u32,
        next_transport_id: u64,
    ) {
        let added = self
            .provisional_refs
            .keys()
            .filter(|reference| !existing.contains(reference))
            .copied()
            .collect::<Vec<_>>();
        for reference in added {
            if let Some(binding) = self.provisional_refs.remove(&reference)
                && let Some(channel) = self.channel_by_transport.remove(&binding.transport_id)
            {
                self.channels.remove(&channel);
            }
        }
        self.next_channel = next_channel;
        self.next_transport_id = next_transport_id;
    }

    fn replayed_public_mapping(
        &self,
        mapping: &DurableStreamMapping,
    ) -> Result<Option<PublicStreamMapping>, AdapterError> {
        let Some(channel) = self
            .channel_by_transport
            .get(&mapping.transport_stream_id)
            .copied()
        else {
            return Ok(None);
        };
        let state = self
            .channels
            .get(&channel)
            .ok_or_else(|| AdapterError::protocol("stream channel is unavailable"))?;
        let Some(durable) = &state.durable else {
            return Ok(None);
        };
        if durable.mapping != *mapping {
            return Err(AdapterError::new(
                PublicErrorCode::StreamConflict,
                "replayed stream mapping changed its durable identity",
            ));
        }
        let stream_token = state
            .stream_token
            .clone()
            .ok_or_else(|| AdapterError::protocol("exposed stream has no public token"))?;
        Ok(Some(PublicStreamMapping {
            channel,
            direction: state.direction,
            input_high_water: (state.direction == PublicStreamDirection::Input).then_some(
                PublicInputHighWater {
                    sequence: DecimalU64(state.next_input_sequence),
                    terminal: state.terminal,
                },
            ),
            provisional_ref: state.provisional_ref,
            stream_token,
        }))
    }

    fn add_private_mapping(&mut self, mapping: DurableStreamMapping) -> Result<(), AdapterError> {
        let handle = mapping
            .handle
            .as_ref()
            .ok_or_else(|| AdapterError::protocol("private stream mapping has no handle"))?;
        let durable_stream_id = required_uuid(handle.stream_id.as_ref(), "durable stream id")?;
        let schema_fingerprint: [u8; 32] = handle
            .element_schema_fingerprint
            .as_slice()
            .try_into()
            .map_err(|_| {
                AdapterError::protocol("private stream schema fingerprint has wrong size")
            })?;
        let direction = match StreamMappingRole::try_from(mapping.role) {
            Ok(StreamMappingRole::Input) => PublicStreamDirection::Input,
            Ok(StreamMappingRole::Output) => PublicStreamDirection::Output,
            _ => {
                return Err(AdapterError::protocol(
                    "private stream mapping has invalid role",
                ));
            }
        };
        if self
            .private_mappings
            .contains_key(&mapping.transport_stream_id)
            || self
                .channel_by_transport
                .get(&mapping.transport_stream_id)
                .and_then(|channel| self.channels.get(channel))
                .is_some_and(|channel| channel.durable.is_some())
        {
            return Err(AdapterError::new(
                PublicErrorCode::StreamConflict,
                "private stream mapping was rebound",
            ));
        }
        self.next_transport_id = self
            .next_transport_id
            .max(mapping.transport_stream_id.saturating_add(1));
        self.private_mappings.insert(
            mapping.transport_stream_id,
            PrivateMapping {
                mapping,
                durable_stream_id,
                schema_fingerprint,
                direction,
            },
        );
        Ok(())
    }

    fn expose_transport(
        &mut self,
        transport_id: u64,
        expected_schema: Option<&SchemaType>,
        keyring: &InvocationSessionTokenKeyring,
    ) -> Result<PublicStreamMapping, AdapterError> {
        let private = self.private_mappings.remove(&transport_id).ok_or_else(|| {
            AdapterError::protocol("stream value has no preceding private mapping")
        })?;
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| AdapterError::protocol("session schema is unavailable"))?;
        let schema = if let Some(schema) = expected_schema {
            schema.clone()
        } else if let Some(channel) = self.channel_by_transport.get(&transport_id) {
            self.channels
                .get(channel)
                .map(|state| state.schema.clone())
                .ok_or_else(|| AdapterError::protocol("stream channel is unavailable"))?
        } else {
            resolve_stream_element_schema_v1(graph, SchemaFingerprintV1(private.schema_fingerprint))
                .map_err(|_| AdapterError::protocol("stream schema fingerprint is invalid"))?
                .map(|graph| graph.root)
                .ok_or_else(|| AdapterError::protocol("stream schema is not in the pinned graph"))?
        };
        let expected_fingerprint = schema_fingerprint_v1(graph, Some(&schema))
            .map_err(|_| AdapterError::protocol("stream schema cannot be fingerprinted"))?
            .0;
        if expected_fingerprint != private.schema_fingerprint {
            return Err(AdapterError::new(
                PublicErrorCode::StreamConflict,
                "stream mapping schema does not match its structural coordinate",
            ));
        }

        let channel = if let Some(channel) = self.channel_by_transport.get(&transport_id).copied() {
            channel
        } else {
            let channel = self.allocate_channel().map_err(AdapterError::from)?;
            self.channel_by_transport.insert(transport_id, channel);
            self.channels.insert(
                channel,
                ChannelState {
                    transport_id,
                    direction: private.direction,
                    schema,
                    provisional_ref: None,
                    durable: None,
                    stream_token: None,
                    next_input_sequence: 0,
                    pending_input: VecDeque::new(),
                    exposed: false,
                    terminal: false,
                },
            );
            channel
        };
        let tokens = self
            .tokens
            .as_ref()
            .ok_or_else(|| AdapterError::protocol("invocation was not accepted"))?;
        let stream_token = sign_stream_token(keyring, tokens, &private)?;
        let state = self
            .channels
            .get_mut(&channel)
            .ok_or_else(|| AdapterError::protocol("stream channel is unavailable"))?;
        if state.direction != private.direction {
            return Err(AdapterError::new(
                PublicErrorCode::StreamConflict,
                "stream direction changed while binding its durable mapping",
            ));
        }
        state.durable = Some(private.clone());
        state.stream_token = Some(stream_token.clone());
        state.exposed = true;
        let input_high_water = if state.direction == PublicStreamDirection::Input {
            let high_water = private.mapping.high_water.as_ref();
            let terminal = high_water.map(|value| value.terminal).unwrap_or(false);
            let sequence = high_water
                .map(|value| {
                    if value.terminal {
                        Ok(value.highest_contiguous_sequence)
                    } else {
                        value
                            .highest_contiguous_sequence
                            .checked_add(1)
                            .ok_or_else(|| {
                                AdapterError::new(
                                    PublicErrorCode::InvalidSequence,
                                    "private input high-water sequence overflow",
                                )
                            })
                    }
                })
                .transpose()?
                .unwrap_or_default();
            state.next_input_sequence = sequence;
            state.terminal = terminal;
            Some(PublicInputHighWater {
                sequence: DecimalU64(sequence),
                terminal,
            })
        } else {
            None
        };
        Ok(PublicStreamMapping {
            channel,
            direction: state.direction,
            input_high_water,
            provisional_ref: state.provisional_ref,
            stream_token,
        })
    }

    fn expose_output_reference(
        &mut self,
        stream: &SchemaValueStream,
        schema: Option<&SchemaType>,
        keyring: &InvocationSessionTokenKeyring,
    ) -> Result<(PublicStreamReference, PublicStreamMapping), PublicSchemaValueError> {
        let transport_id = stream.take_host_endpoint::<u64>().map_err(|_| {
            PublicSchemaValueError::new(
                PublicErrorCode::StreamAlreadyConsumed,
                "output stream reference was already consumed",
            )
        })?;
        if let Some(channel) = self.channel_by_transport.get(&transport_id).copied() {
            let state = self.channels.get(&channel).ok_or_else(|| {
                PublicSchemaValueError::new(
                    PublicErrorCode::ProtocolError,
                    "stream channel is unavailable",
                )
            })?;
            if state.durable.is_some() {
                if state.direction != PublicStreamDirection::Output
                    || schema.is_some_and(|schema| schema != &state.schema)
                {
                    return Err(PublicSchemaValueError::new(
                        PublicErrorCode::StreamConflict,
                        "replayed output stream reference changed its schema or direction",
                    ));
                }
                let stream_token = state.stream_token.clone().ok_or_else(|| {
                    PublicSchemaValueError::new(
                        PublicErrorCode::ProtocolError,
                        "exposed stream has no public token",
                    )
                })?;
                let mapping = PublicStreamMapping {
                    channel,
                    direction: state.direction,
                    input_high_water: None,
                    provisional_ref: state.provisional_ref,
                    stream_token: stream_token.clone(),
                };
                return Ok((PublicStreamReference::Stable(stream_token), mapping));
            }
        }
        let mapping = self
            .expose_transport(transport_id, schema, keyring)
            .map_err(|error| PublicSchemaValueError::new(error.code, error.message))?;
        Ok((
            PublicStreamReference::Stable(mapping.stream_token.clone()),
            mapping,
        ))
    }
}

pub async fn serve_public_invocation_session(
    socket: WebSocketStream,
    worker_service: Arc<WorkerService>,
    keyring: Arc<InvocationSessionTokenKeyring>,
    auth: AuthCtx,
) {
    let (mut websocket_sink, mut websocket_stream) = socket.split();
    let budgets = SessionBudgets::new();
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<QueuedMessage>(CHANNEL_CAPACITY);
    let outbound = Outbound {
        sender: outbound_tx,
        budgets: budgets.clone(),
    };
    let writer_budgets = budgets.clone();
    let writer = tokio::spawn(async move {
        // A queued close frame is held back and sent as the final frame once the session ends,
        // so the client cannot observe the close handshake before the private session has been
        // drained. Everything queued after a close frame is dropped.
        let mut pending_close = None;
        let mut sink_open = true;
        while let Some(message) = outbound_rx.recv().await {
            if !sink_open || pending_close.is_some() || message.should_drop(&writer_budgets).await {
                continue;
            }
            if message.message.is_close() {
                pending_close = Some(message.message);
                continue;
            }
            if !matches!(
                tokio::time::timeout(WRITE_PROGRESS_TIMEOUT, websocket_sink.send(message.message))
                    .await,
                Ok(Ok(()))
            ) {
                sink_open = false;
            }
        }
        if let Some(close) = pending_close
            && sink_open
        {
            let _ = tokio::time::timeout(WRITE_PROGRESS_TIMEOUT, websocket_sink.send(close)).await;
        }
        let _ = tokio::time::timeout(WRITE_PROGRESS_TIMEOUT, websocket_sink.close()).await;
    });
    let bindings = token_bindings(&auth);
    let initial = match receive_initial(
        &mut websocket_stream,
        &outbound,
        &keyring,
        &bindings,
        &budgets,
    )
    .await
    {
        Ok(Some(initial)) => initial,
        Ok(None) | Err(()) => {
            drop(outbound);
            let _ = writer.await;
            return;
        }
    };
    let (attempt_id, token_bindings, token_key_id) = match &initial {
        InitialMessage::Start { attempt_id, .. } => (
            *attempt_id,
            bindings.clone(),
            keyring.active_key_id().to_string(),
        ),
        InitialMessage::Resume {
            attempt_id,
            token_bindings,
            token_key_id,
            ..
        } => (*attempt_id, token_bindings.clone(), token_key_id.clone()),
    };
    let (request_sender, request_receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let tail = tokio_stream::wrappers::ReceiverStream::new(request_receiver)
        .map(|admitted: AdmittedRequest| admitted.request);
    let mut initial_state = AdapterState::new_connection(Uuid::new_v4());
    let started = match initial {
        InitialMessage::Start {
            start, _admission, ..
        } => {
            let result = worker_service
                .invoke_public_agent_session_v1(
                    start,
                    Box::pin(tail),
                    auth.clone(),
                    |reference, schema| {
                        initial_state.register_provisional(
                            reference,
                            schema,
                            ProvisionalOwner::Initial,
                        )
                    },
                )
                .await;
            drop(_admission);
            result
        }
        InitialMessage::Resume {
            resume, _admission, ..
        } => {
            let result = worker_service
                .resume_public_agent_session_v1(resume, Box::pin(tail), auth.clone())
                .await;
            drop(_admission);
            result
        }
    };
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            let (code, message) = public_start_error(error);
            let _ = send_text(
                &outbound,
                &PublicServerMessage::InvocationRejected {
                    attempt_id: Some(attempt_id),
                    code,
                    message,
                    retryable: false,
                    version: 1,
                },
            )
            .await;
            let _ = outbound
                .send(
                    Message::close_with(CloseCode::Normal, "session rejected"),
                    None,
                )
                .await;
            drop(outbound);
            let _ = writer.await;
            return;
        }
    };
    if initial_state.initialize(&started).is_err() {
        let message = safe_rejection_message(PublicErrorCode::InternalError);
        send_rejection(
            &outbound,
            Some(attempt_id),
            PublicErrorCode::InternalError,
            message.clone(),
        )
        .await;
        close_error(&outbound, &message).await;
        drop(outbound);
        let _ = writer.await;
        return;
    }
    let state = Arc::new(Mutex::new(initial_state));
    let responses = started.responses;
    {
        let client = handle_client_messages(
            websocket_stream,
            request_sender,
            outbound.clone(),
            state.clone(),
            budgets,
            attempt_id,
        );
        let server = handle_private_responses(
            responses,
            outbound.clone(),
            state,
            keyring,
            token_bindings,
            token_key_id,
            attempt_id,
        );
        tokio::pin!(client, server);
        tokio::select! {
            _ = &mut client => {
                let _ = tokio::time::timeout(CLIENT_GONE_DRAIN_TIMEOUT, &mut server).await;
            }
            _ = &mut server => {}
        }
    }
    drop(outbound);
    let _ = writer.await;
}

async fn receive_initial<S>(
    websocket: &mut S,
    outbound: &Outbound,
    keyring: &InvocationSessionTokenKeyring,
    bindings: &InvocationSessionTokenBindings,
    budgets: &SessionBudgets,
) -> Result<Option<InitialMessage>, ()>
where
    S: futures::Stream<Item = std::io::Result<Message>> + Unpin,
{
    loop {
        let Some(message) = websocket.next().await else {
            return Ok(None);
        };
        let message = match message {
            Ok(message) => message,
            Err(_) => return Ok(None),
        };
        match message {
            Message::Ping(payload) => {
                if outbound.send(Message::pong(payload), None).await.is_err() {
                    return Ok(None);
                }
            }
            Message::Pong(_) => {}
            Message::Close(_) => return Ok(None),
            Message::Binary(bytes) => {
                if bytes.len() > MAX_WEBSOCKET_MESSAGE_SIZE {
                    close_too_large(outbound).await;
                    return Err(());
                }
                close_protocol(outbound, "first application message must be text").await;
                return Err(());
            }
            message @ Message::Text(_) => {
                if message_size(&message) > MAX_WEBSOCKET_MESSAGE_SIZE {
                    close_too_large(outbound).await;
                    return Err(());
                }
                let admission =
                    acquire_bytes(&budgets.session, message_size(&message).max(1)).await?;
                let Message::Text(text) = message else {
                    unreachable!()
                };
                let message = match decode_client_text(text.as_bytes()) {
                    Ok(message) => message,
                    Err(error) => {
                        close_protocol(outbound, &error.message).await;
                        return Err(());
                    }
                };
                return match message {
                    PublicClientMessage::InvocationStart {
                        attempt_id,
                        config,
                        idempotency_key,
                        method_parameters,
                        selector,
                        ..
                    } => Ok(Some(InitialMessage::Start {
                        start: PublicAgentSessionStart {
                            selector: *selector,
                            config,
                            idempotency_key,
                            attempt_id,
                            method_parameters,
                        },
                        attempt_id,
                        _admission: admission,
                    })),
                    PublicClientMessage::ResumeAttach {
                        attempt_id,
                        operation,
                        output_cursors,
                        session_token,
                        ..
                    } => {
                        let verified = match keyring.verify(
                            &session_token,
                            InvocationSessionTokenKind::Session,
                            &bindings.account,
                            &bindings.effective_principal,
                        ) {
                            Ok(verified) => verified,
                            Err(error) => {
                                send_rejection(
                                    outbound,
                                    Some(attempt_id),
                                    error.code,
                                    error.to_string(),
                                )
                                .await;
                                return Err(());
                            }
                        };
                        let token_bindings = verified.bindings.clone();
                        let InvocationSessionTokenPayload::Session(session) = verified.payload
                        else {
                            unreachable!("token kind was verified")
                        };
                        let token_key_id = session.stream_key_id.clone();
                        let identity = match decode_session_agent_identity(&session.agent) {
                            Ok(identity) => identity,
                            Err(error) => {
                                send_rejection(
                                    outbound,
                                    Some(attempt_id),
                                    error.code,
                                    error.to_string(),
                                )
                                .await;
                                return Err(());
                            }
                        };
                        let mut cursors = Vec::with_capacity(output_cursors.len());
                        let mut streams = HashSet::new();
                        for token in output_cursors {
                            let verified = match keyring.verify(
                                &token,
                                InvocationSessionTokenKind::Cursor,
                                &bindings.account,
                                &bindings.effective_principal,
                            ) {
                                Ok(verified) => verified,
                                Err(error) => {
                                    send_rejection(
                                        outbound,
                                        Some(attempt_id),
                                        error.code,
                                        error.to_string(),
                                    )
                                    .await;
                                    return Err(());
                                }
                            };
                            let InvocationSessionTokenPayload::Cursor(cursor) = verified.payload
                            else {
                                unreachable!("token kind was verified")
                            };
                            if cursor.parent_logical_invocation_id != session.logical_invocation_id
                                || !streams.insert(cursor.output_durable_stream_id)
                            {
                                send_rejection(
                                    outbound,
                                    Some(attempt_id),
                                    PublicErrorCode::InvalidCursor,
                                    "cursor does not belong to this session or is duplicated",
                                )
                                .await;
                                return Err(());
                            }
                            cursors.push(StreamCursor {
                                stream_id: Some(cursor.output_durable_stream_id.into()),
                                last_observed_offset: Some(cursor.durable_offset),
                            });
                        }
                        Ok(Some(InitialMessage::Resume {
                            resume: PublicAgentSessionResume {
                                identity,
                                session,
                                attempt_id,
                                operation: match operation {
                                    PublicResumeOperation::Resume => ResumeOperation::Resume,
                                    PublicResumeOperation::Takeover => ResumeOperation::Takeover,
                                },
                                cursors,
                            },
                            attempt_id,
                            token_bindings,
                            token_key_id,
                            _admission: admission,
                        }))
                    }
                    _ => {
                        close_protocol(
                            outbound,
                            "first application message must start or resume an invocation",
                        )
                        .await;
                        Err(())
                    }
                };
            }
        }
    }
}

async fn handle_client_messages<S>(
    mut websocket: S,
    requests: mpsc::Sender<AdmittedRequest>,
    outbound: Outbound,
    state: Arc<Mutex<AdapterState>>,
    budgets: SessionBudgets,
    attempt_id: Uuid,
) where
    S: futures::Stream<Item = std::io::Result<Message>> + Unpin,
{
    while let Some(message) = websocket.next().await {
        let message = match message {
            Ok(message) => message,
            Err(_) => return,
        };
        let request = match message {
            Message::Ping(payload) => {
                if outbound.send(Message::pong(payload), None).await.is_err() {
                    return;
                }
                continue;
            }
            Message::Pong(_) => continue,
            Message::Close(_) => return,
            message @ Message::Text(_) => {
                if message_size(&message) > MAX_WEBSOCKET_MESSAGE_SIZE {
                    close_too_large(&outbound).await;
                    return;
                }
                let session = match acquire_bytes(&budgets.session, message_size(&message).max(1))
                    .await
                {
                    Ok(permit) => permit,
                    Err(()) => {
                        let message = safe_rejection_message(PublicErrorCode::ResourceExhausted);
                        send_rejection(
                            &outbound,
                            Some(attempt_id),
                            PublicErrorCode::ResourceExhausted,
                            message.clone(),
                        )
                        .await;
                        close_protocol(&outbound, &message).await;
                        return;
                    }
                };
                let Message::Text(text) = message else {
                    unreachable!()
                };
                match decode_client_text(text.as_bytes()) {
                    Ok(message) => translate_client_text(message, &state).await,
                    Err(error) => Err(AdapterError::from(error)),
                }
                .map(|request| (request, session, text.len().max(1)))
            }
            message @ Message::Binary(_) => {
                if message_size(&message) > MAX_WEBSOCKET_MESSAGE_SIZE {
                    close_too_large(&outbound).await;
                    return;
                }
                let session = match acquire_bytes(&budgets.session, message_size(&message).max(1))
                    .await
                {
                    Ok(permit) => permit,
                    Err(()) => {
                        let message = safe_rejection_message(PublicErrorCode::ResourceExhausted);
                        send_rejection(
                            &outbound,
                            Some(attempt_id),
                            PublicErrorCode::ResourceExhausted,
                            message.clone(),
                        )
                        .await;
                        close_protocol(&outbound, &message).await;
                        return;
                    }
                };
                let Message::Binary(bytes) = message else {
                    unreachable!()
                };
                let size = bytes.len().max(1);
                match decode_binary_message(&bytes) {
                    Ok(message) => translate_client_binary(message, &state).await,
                    Err(error) => Err(AdapterError::from(error)),
                }
                .map(|request| (request, session, size))
            }
        };
        let (request, session, _frame_size) = match request {
            Ok(request) => request,
            Err(error) => {
                let safe_message = safe_rejection_message(error.code);
                send_rejection(
                    &outbound,
                    Some(attempt_id),
                    error.code,
                    safe_message.clone(),
                )
                .await;
                close_protocol(&outbound, &safe_message).await;
                return;
            }
        };
        let TranslatedRequest {
            request,
            input,
            cancelled_output,
        } = request;
        if let Some(channel) = cancelled_output {
            outbound.cancel_output(channel).await;
        }
        if let Some(input) = input {
            let admission = match acquire_input_admission(
                &budgets,
                input.terminal,
                input.channel,
                input.byte_charge,
            )
            .await
            {
                Ok(admission) => admission,
                Err(error) => {
                    let safe_message = safe_rejection_message(error.code);
                    send_rejection(
                        &outbound,
                        Some(attempt_id),
                        error.code,
                        safe_message.clone(),
                    )
                    .await;
                    close_protocol(&outbound, &safe_message).await;
                    return;
                }
            };
            let mut state = state.lock().await;
            let Some(channel) = state.channels.get_mut(&input.channel) else {
                drop(state);
                close_protocol(
                    &outbound,
                    "input channel disappeared while admitting a frame",
                )
                .await;
                return;
            };
            channel.pending_input.push_back(admission);
        }
        if requests
            .send(AdmittedRequest {
                request,
                _session: session,
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn translate_client_text(
    message: PublicClientMessage,
    state: &Arc<Mutex<AdapterState>>,
) -> Result<TranslatedRequest, AdapterError> {
    let mut state = state.lock().await;
    match message {
        PublicClientMessage::InputStreamItem {
            channel,
            sequence,
            value,
            ..
        } => {
            let (transport_id, durable_stream_id, epoch, schema) = input_channel(&state, channel)?;
            let graph = state.graph.clone().unwrap();
            let provisional_refs = state
                .provisional_refs
                .keys()
                .copied()
                .collect::<HashSet<_>>();
            let next_channel = state.next_channel;
            let next_transport_id = state.next_transport_id;
            let decoded = decode_public_schema_value_with_charge(
                &graph,
                &schema,
                &value,
                PublicStreamReferencePolicy::Provisional,
                |reference, schema| {
                    state.register_provisional(
                        reference,
                        schema,
                        ProvisionalOwner::InputItem {
                            channel,
                            sequence: sequence.0,
                        },
                    )
                },
            );
            let (decoded, byte_charge) = match decoded {
                Ok(decoded) => decoded,
                Err(error) => {
                    state.rollback_provisional_registrations(
                        &provisional_refs,
                        next_channel,
                        next_transport_id,
                    );
                    return Err(error.into());
                }
            };
            let value = match schema_value_to_proto_with_streams(decoded, |stream| {
                stream.take_host_endpoint::<u64>()
            }) {
                Ok(value) => value,
                Err(_) => {
                    state.rollback_provisional_registrations(
                        &provisional_refs,
                        next_channel,
                        next_transport_id,
                    );
                    return Err(AdapterError::protocol(
                        "failed to transfer nested input stream",
                    ));
                }
            };
            let request = request(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: transport_id,
                sequence: sequence.0,
                payload: Some(input_stream_item::Payload::Value(value)),
                durable_stream_id: Some(durable_stream_id.into()),
                epoch,
            }));
            if let Err(error) = state
                .protocol_state
                .validate_trusted_request(&request)
                .map_err(request_protocol_error)
            {
                state.rollback_provisional_registrations(
                    &provisional_refs,
                    next_channel,
                    next_transport_id,
                );
                return Err(error);
            }
            let channel_state = state.channels.get_mut(&channel).unwrap();
            if sequence.0 == channel_state.next_input_sequence {
                channel_state.next_input_sequence = sequence.0.checked_add(1).ok_or_else(|| {
                    AdapterError::new(PublicErrorCode::InvalidSequence, "sequence overflow")
                })?;
            }
            Ok(TranslatedRequest {
                request,
                input: Some(PendingInput {
                    channel,
                    terminal: false,
                    byte_charge,
                }),
                cancelled_output: None,
            })
        }
        PublicClientMessage::InputStreamEnd {
            channel, sequence, ..
        } => {
            let (transport_id, durable_stream_id, epoch, _) = input_channel(&state, channel)?;
            let request = request(invocation_request::Request::InputEnd(InputStreamEnd {
                transport_stream_id: transport_id,
                sequence: sequence.0,
                durable_stream_id: Some(durable_stream_id.into()),
                epoch,
            }));
            state
                .protocol_state
                .validate_trusted_request(&request)
                .map_err(request_protocol_error)?;
            Ok(TranslatedRequest {
                request,
                input: Some(PendingInput {
                    channel,
                    terminal: true,
                    byte_charge: 1,
                }),
                cancelled_output: None,
            })
        }
        PublicClientMessage::StreamCancel {
            channel, reason, ..
        } => {
            let cancelled_output = state
                .channels
                .get(&channel)
                .filter(|channel| channel.direction == PublicStreamDirection::Output)
                .map(|_| channel);
            let request = translate_cancel(&state, channel, reason)?;
            state
                .protocol_state
                .validate_received_trusted_request(&request)
                .map_err(request_protocol_error)?;
            let channel_state = state.channels.get_mut(&channel).unwrap();
            channel_state.terminal = true;
            channel_state.pending_input.clear();
            Ok(TranslatedRequest {
                request,
                input: None,
                cancelled_output,
            })
        }
        PublicClientMessage::InvocationStart { .. } | PublicClientMessage::ResumeAttach { .. } => {
            Err(AdapterError::protocol(
                "start and resume messages are only valid as the first application message",
            ))
        }
    }
}

async fn translate_client_binary(
    message: BinaryMessage,
    state: &Arc<Mutex<AdapterState>>,
) -> Result<TranslatedRequest, AdapterError> {
    let mut state = state.lock().await;
    let channel = message.metadata.channel;
    let (transport_id, durable_stream_id, epoch, schema) = input_channel(&state, channel)?;
    let graph = state.graph.as_ref().unwrap();
    let lane = binary_lane(graph, &schema);
    let (payload, byte_charge) = match (lane, message.metadata.kind) {
        (Some(BinaryMessageKind::InputU8), BinaryMessageKind::InputU8) => {
            if message.metadata.item_count.0 != message.payload.len() as u64 {
                return Err(AdapterError::new(
                    PublicErrorCode::InvalidSequence,
                    "u8 binary item count does not match payload size",
                ));
            }
            validate_packed_u8(
                graph,
                &schema,
                &message.payload,
                PublicErrorCode::ValidationError,
            )?;
            let byte_charge = message.payload.len().checked_mul(2).ok_or_else(|| {
                AdapterError::new(
                    PublicErrorCode::ResourceExhausted,
                    "u8 byte charge overflow",
                )
            })?;
            (
                input_stream_item::Payload::PackedU8(message.payload),
                byte_charge,
            )
        }
        (Some(BinaryMessageKind::InputBinary), BinaryMessageKind::InputBinary) => {
            if message.metadata.item_count.0 != 1 {
                return Err(AdapterError::new(
                    PublicErrorCode::InvalidSequence,
                    "binary stream frames contain exactly one logical item",
                ));
            }
            let value = SchemaValue::Binary(BinaryValuePayload {
                bytes: message.payload,
                mime_type: message.metadata.mime_type,
            });
            validate_direct_value(graph, &schema, &value, PublicErrorCode::ValidationError)?;
            let byte_charge = direct_binary_charge(&value)?;
            let value: ProtoSchemaValue = value
                .try_into()
                .map_err(|_| AdapterError::protocol("failed to encode binary input item"))?;
            (input_stream_item::Payload::Value(value), byte_charge)
        }
        _ => {
            return Err(AdapterError::new(
                PublicErrorCode::InvalidChannel,
                "binary frame kind does not match the channel element schema",
            ));
        }
    };
    let next_input_sequence = message
        .metadata
        .sequence
        .0
        .checked_add(message.metadata.item_count.0)
        .ok_or_else(|| AdapterError::new(PublicErrorCode::InvalidSequence, "sequence overflow"))?;
    let request = request(invocation_request::Request::InputItem(InputStreamItem {
        transport_stream_id: transport_id,
        sequence: message.metadata.sequence.0,
        payload: Some(payload),
        durable_stream_id: Some(durable_stream_id.into()),
        epoch,
    }));
    state
        .protocol_state
        .validate_trusted_request(&request)
        .map_err(request_protocol_error)?;
    let channel_state = state.channels.get_mut(&channel).unwrap();
    if message.metadata.sequence.0 == channel_state.next_input_sequence {
        channel_state.next_input_sequence = next_input_sequence;
    }
    Ok(TranslatedRequest {
        request,
        input: Some(PendingInput {
            channel,
            terminal: false,
            byte_charge,
        }),
        cancelled_output: None,
    })
}

async fn handle_private_responses<S>(
    mut responses: S,
    outbound: Outbound,
    state: Arc<Mutex<AdapterState>>,
    keyring: Arc<InvocationSessionTokenKeyring>,
    bindings: InvocationSessionTokenBindings,
    token_key_id: String,
    attempt_id: Uuid,
) where
    S: futures::Stream<Item = Result<InvocationResponse, tonic::Status>> + Unpin,
{
    while let Some(response) = responses.next().await {
        let response = match response {
            Ok(response) => response,
            Err(status) => {
                tracing::warn!(
                    attempt_id = %attempt_id,
                    error = %status,
                    "Invocation session transport failed"
                );
                fail_session(
                    &outbound,
                    &state,
                    attempt_id,
                    PublicErrorCode::InvocationFailed,
                )
                .await;
                return;
            }
        };
        let translated = translate_private_response(
            response,
            &state,
            &keyring,
            &bindings,
            &token_key_id,
            attempt_id,
        )
        .await;
        let messages = match translated {
            Ok(messages) => messages,
            Err(error) => {
                tracing::warn!(
                    attempt_id = %attempt_id,
                    error = ?error,
                    "Invocation session response translation failed"
                );
                fail_session(
                    &outbound,
                    &state,
                    attempt_id,
                    PublicErrorCode::InternalError,
                )
                .await;
                return;
            }
        };
        for message in messages {
            if let Some(channel) = message.cancel_output_before_send {
                outbound.cancel_output(channel).await;
            }
            let result = if message.preserve_after_output_cancel {
                let Some((channel, byte_charge)) =
                    message.stream_channel.zip(message.stream_byte_charge)
                else {
                    return;
                };
                outbound
                    .send_stream_terminal(message.message, channel, byte_charge)
                    .await
            } else {
                outbound
                    .send(
                        message.message,
                        message.stream_channel.zip(message.stream_byte_charge),
                    )
                    .await
            };
            if result.is_err() {
                return;
            }
        }
        if state.lock().await.protocol_state.is_complete() {
            let _ = outbound
                .send(
                    Message::close_with(CloseCode::Normal, "session complete"),
                    None,
                )
                .await;
            return;
        }
    }
    tracing::warn!(
        attempt_id = %attempt_id,
        "Invocation session response stream ended before the session completed"
    );
    fail_session(
        &outbound,
        &state,
        attempt_id,
        PublicErrorCode::InvocationFailed,
    )
    .await;
}

async fn fail_session(
    outbound: &Outbound,
    state: &Arc<Mutex<AdapterState>>,
    attempt_id: Uuid,
    code: PublicErrorCode,
) {
    let (accepted, open_channels) = {
        let mut state = state.lock().await;
        let accepted = state.tokens.is_some();
        let open_channels = if accepted {
            state
                .channels
                .iter_mut()
                .filter_map(|(channel, state)| {
                    if state.exposed && !state.terminal {
                        state.terminal = true;
                        state.pending_input.clear();
                        Some(*channel)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        (accepted, open_channels)
    };
    if !accepted {
        send_rejection(
            outbound,
            Some(attempt_id),
            code,
            safe_rejection_message(code),
        )
        .await;
        let _ = outbound
            .send(
                Message::close_with(CloseCode::Error, "invocation failed"),
                None,
            )
            .await;
        return;
    }
    for channel in open_channels {
        if send_text(
            outbound,
            &PublicServerMessage::StreamCancel {
                channel,
                reason: PublicServerCancelReason::InvocationFailed,
                version: 1,
            },
        )
        .await
        .is_err()
        {
            return;
        }
    }
    if send_text(
        outbound,
        &PublicServerMessage::InvocationFinished {
            outcome: PublicInvocationOutcome::Failure {
                code,
                message: safe_rejection_message(code),
            },
            version: 1,
        },
    )
    .await
    .is_ok()
    {
        let _ = outbound
            .send(
                Message::close_with(CloseCode::Normal, "invocation finished"),
                None,
            )
            .await;
    }
}

async fn translate_private_response(
    response: InvocationResponse,
    state: &Arc<Mutex<AdapterState>>,
    keyring: &InvocationSessionTokenKeyring,
    bindings: &InvocationSessionTokenBindings,
    token_key_id: &str,
    attempt_id: Uuid,
) -> Result<Vec<TranslatedFrame>, AdapterError> {
    let mut state = state.lock().await;
    state
        .protocol_state
        .validate_response(&response)
        .map_err(AdapterError::protocol)?;
    let response = response
        .response
        .ok_or_else(|| AdapterError::protocol("private response has no payload"))?;
    match response {
        invocation_response::Response::Accepted(accepted) => {
            let message = translate_accepted(
                &mut state,
                accepted,
                keyring,
                bindings,
                token_key_id,
                attempt_id,
            )?;
            Ok(vec![frame(text_message(&message)?)])
        }
        invocation_response::Response::Rejected(rejected) => {
            let code = rejection_code(rejected.reason);
            Ok(vec![frame(text_message(
                &PublicServerMessage::InvocationRejected {
                    attempt_id: Some(attempt_id),
                    code,
                    message: safe_rejection_message(code),
                    retryable: matches!(code, PublicErrorCode::ResourceExhausted),
                    version: 1,
                },
            )?)])
        }
        invocation_response::Response::Result(result) => {
            translate_result(&mut state, result, keyring)
        }
        invocation_response::Response::OutputItem(item) => {
            translate_output_item(&mut state, item, keyring)
        }
        invocation_response::Response::OutputEnd(end) => {
            translate_output_end(&mut state, end, keyring, PublicOutputStreamOutcome::Ok)
        }
        invocation_response::Response::OutputError(error) => {
            translate_output_error(&mut state, error, keyring)
        }
        invocation_response::Response::InputAck(ack) => {
            let mut mappings = Vec::new();
            for mapping in ack.new_stream_mappings {
                let transport_id = mapping.transport_stream_id;
                if let Some(mapping) = state.replayed_public_mapping(&mapping)? {
                    mappings.push(mapping);
                } else {
                    state.add_private_mapping(mapping)?;
                    mappings.push(state.expose_transport(transport_id, None, keyring)?);
                }
            }
            let channel = channel_for_transport(&state, ack.transport_stream_id)?;
            let channel_state = state.channels.get_mut(&channel).ok_or_else(|| {
                AdapterError::protocol("acknowledged input channel is unavailable")
            })?;
            let admission = channel_state.pending_input.pop_front().ok_or_else(|| {
                AdapterError::protocol("input acknowledgement has no pending public frame")
            })?;
            let terminal = admission.terminal;
            let highest_contiguous_sequence = if terminal {
                ack.highest_contiguous_sequence
            } else {
                ack.highest_contiguous_sequence
                    .checked_add(1)
                    .ok_or_else(|| {
                        AdapterError::new(
                            PublicErrorCode::InvalidSequence,
                            "private input acknowledgement sequence overflow",
                        )
                    })?
            };
            channel_state.next_input_sequence = highest_contiguous_sequence;
            if terminal {
                channel_state.terminal = true;
            }
            Ok(vec![frame(text_message(
                &PublicServerMessage::InputStreamAck {
                    channel,
                    highest_contiguous_sequence: DecimalU64(highest_contiguous_sequence),
                    mappings,
                    terminal,
                    version: 1,
                },
            )?)])
        }
        invocation_response::Response::StreamCancel(cancel) => {
            let channel = channel_for_transport(&state, cancel.transport_stream_id)?;
            if let Some(channel_state) = state.channels.get_mut(&channel) {
                channel_state.terminal = true;
                channel_state.pending_input.clear();
            }
            Ok(vec![cancel_frame(
                text_message(&PublicServerMessage::StreamCancel {
                    channel,
                    reason: server_cancel_reason(cancel.reason),
                    version: 1,
                })?,
                channel,
            )])
        }
        invocation_response::Response::AttachmentRevoked(_revoked) => Ok(vec![frame(
            text_message(&PublicServerMessage::AttachmentRevoked {
                reason: PublicAttachmentRevokedReason::Replaced,
                version: 1,
            })?,
        )]),
        invocation_response::Response::Finished(finished) => {
            let outcome = match finished.outcome {
                Some(invocation_session_completion::Outcome::Success(_)) => {
                    PublicInvocationOutcome::Success
                }
                Some(invocation_session_completion::Outcome::Failure(failure)) => {
                    tracing::warn!(
                        attempt_id = %attempt_id,
                        kind = failure.kind,
                        code = %failure.code,
                        message = %failure.message,
                        worker_error = ?failure.worker_error,
                        "Invocation session finished with failure"
                    );
                    PublicInvocationOutcome::Failure {
                        code: PublicErrorCode::InvocationFailed,
                        message: "invocation failed".to_string(),
                    }
                }
                None => {
                    tracing::warn!(
                        attempt_id = %attempt_id,
                        "Invocation session finished without an outcome"
                    );
                    PublicInvocationOutcome::Failure {
                        code: PublicErrorCode::InvocationFailed,
                        message: "invocation failed".to_string(),
                    }
                }
            };
            Ok(vec![frame(text_message(
                &PublicServerMessage::InvocationFinished {
                    outcome,
                    version: 1,
                },
            )?)])
        }
    }
}

fn translate_accepted(
    state: &mut AdapterState,
    accepted: InvocationAccepted,
    keyring: &InvocationSessionTokenKeyring,
    bindings: &InvocationSessionTokenBindings,
    token_key_id: &str,
    attempt_id: Uuid,
) -> Result<PublicServerMessage, AdapterError> {
    let accepted_attempt = accepted
        .attempt_id
        .as_ref()
        .map(|value| required_uuid(Some(value), "accepted attempt id"))
        .transpose()?
        .unwrap_or(attempt_id);
    if accepted_attempt != attempt_id {
        return Err(AdapterError::new(
            PublicErrorCode::AttemptConflict,
            "accepted attempt does not match the requested attempt",
        ));
    }
    let attachment_id = accepted
        .attachment_id
        .as_ref()
        .map(|value| required_uuid(Some(value), "attachment id"))
        .transpose()?
        .unwrap_or(Uuid::nil());
    let callee_incarnation = accepted
        .callee_fingerprint
        .as_ref()
        .map(|value| required_uuid(Some(value), "callee fingerprint"))
        .transpose()?
        .unwrap_or_else(|| deterministic_uuid(b"non-durable-callee"));
    let idempotency_key: golem_common::model::IdempotencyKey = accepted
        .idempotency_key
        .clone()
        .ok_or_else(|| AdapterError::protocol("accepted response has no idempotency key"))?
        .into();
    let agent_id: golem_common::model::AgentId = accepted
        .agent_id
        .clone()
        .ok_or_else(|| AdapterError::protocol("accepted response has no agent identity"))?
        .try_into()
        .map_err(|_| AdapterError::protocol("accepted response has invalid agent identity"))?;
    let identity = state
        .identity
        .as_mut()
        .ok_or_else(|| AdapterError::protocol("session identity is unavailable"))?;
    if identity.component_id != agent_id.component_id.0 || identity.agent_id != agent_id.agent_id {
        return Err(AdapterError::protocol(
            "accepted agent identity differs from the pinned invocation",
        ));
    }
    if let Some(component_revision) = accepted.component_revision {
        identity.component_revision = component_revision;
    }
    if let Some(method_name) = accepted.method_name
        && method_name != identity.method
    {
        return Err(AdapterError::protocol(
            "accepted method differs from the pinned invocation",
        ));
    }
    let logical_invocation_id =
        invocation_uuid(&agent_id, &idempotency_key.value, callee_incarnation);
    state.tokens = Some(TokenContext {
        bindings: bindings.clone(),
        key_id: token_key_id.to_string(),
        logical_invocation_id,
    });
    state.attachment_epoch = accepted.epoch;
    let encoded_identity = encode_session_agent_identity(identity)
        .map_err(|error| AdapterError::new(error.code, error.to_string()))?;
    let session_token = keyring
        .sign(
            bindings,
            &InvocationSessionTokenPayload::Session(SessionTokenPayload {
                application: state.application.clone().unwrap_or_default(),
                environment: state.environment.clone().unwrap_or_default(),
                agent: encoded_identity,
                idempotency_key: idempotency_key.value.clone(),
                logical_invocation_id,
                attachment_id,
                expected_attachment_generation: accepted.epoch,
                callee_incarnation,
                stream_key_id: token_key_id.to_string(),
            }),
        )
        .map_err(|error| AdapterError::new(error.code, error.to_string()))?;
    for mapping in accepted.stream_mappings {
        state.add_private_mapping(mapping)?;
    }
    let transport_ids = state.private_mappings.keys().copied().collect::<Vec<_>>();
    let mut mappings = Vec::with_capacity(transport_ids.len());
    for transport_id in transport_ids {
        mappings.push(state.expose_transport(transport_id, None, keyring)?);
    }
    Ok(PublicServerMessage::InvocationAccepted {
        attempt_id,
        idempotency_key: idempotency_key.value,
        mappings,
        session_token,
        version: 1,
    })
}

fn translate_result(
    state: &mut AdapterState,
    result: InvocationSessionResult,
    keyring: &InvocationSessionTokenKeyring,
) -> Result<Vec<TranslatedFrame>, AdapterError> {
    for mapping in result.new_stream_mappings {
        if state.replayed_public_mapping(&mapping)?.is_none() {
            state.add_private_mapping(mapping)?;
        }
    }
    let mut mappings = Vec::new();
    let result = match result.result {
        Some(invocation_session_result::Result::NoResult(_)) => PublicInvocationResult::None,
        Some(invocation_session_result::Result::MethodResult(value)) => {
            let schema = state.output_schema.clone().ok_or_else(|| {
                AdapterError::protocol("private invocation returned a value for a unit method")
            })?;
            let value = decode_public_session_schema_value(value)
                .map_err(|_| AdapterError::protocol("private result value is malformed"))?;
            let graph = state.graph.clone().unwrap();
            let (value, _) = encode_public_schema_value_with_charge(
                &graph,
                &schema,
                &value,
                |stream, element| {
                    let (reference, mapping) =
                        state.expose_output_reference(stream, element, keyring)?;
                    mappings.push(mapping);
                    Ok(reference)
                },
            )?;
            PublicInvocationResult::Value { value }
        }
        None => return Err(AdapterError::protocol("private result has no payload")),
    };
    Ok(vec![frame(text_message(
        &PublicServerMessage::InvocationResult {
            mappings,
            result,
            version: 1,
        },
    )?)])
}

fn translate_output_item(
    state: &mut AdapterState,
    item: OutputStreamItem,
    keyring: &InvocationSessionTokenKeyring,
) -> Result<Vec<TranslatedFrame>, AdapterError> {
    for mapping in item.new_stream_mappings {
        if state.replayed_public_mapping(&mapping)?.is_none() {
            state.add_private_mapping(mapping)?;
        }
    }
    let channel = channel_for_transport(state, item.transport_stream_id)?;
    let schema = state.channels.get(&channel).unwrap().schema.clone();
    let cursor_token = sign_cursor_token(keyring, state, channel, item.durable_offset.clone())?;
    if !item.packed_u8.is_empty() {
        let graph = state.graph.as_ref().unwrap();
        validate_packed_u8(
            graph,
            &schema,
            &item.packed_u8,
            PublicErrorCode::ProtocolError,
        )?;
        let byte_charge = item.packed_u8.len().checked_mul(2).ok_or_else(|| {
            AdapterError::new(
                PublicErrorCode::ResourceExhausted,
                "u8 byte charge overflow",
            )
        })?;
        let message = BinaryMessage {
            metadata: BinaryMessageMetadata {
                channel,
                cursor_token: Some(cursor_token),
                item_count: DecimalU64(item.logical_item_count),
                kind: BinaryMessageKind::OutputU8,
                mime_type: None,
                sequence: DecimalU64(item.producer_sequence),
                version: 1,
            },
            payload: item.packed_u8,
        };
        return Ok(vec![stream_frame(
            Message::binary(encode_binary_message(&message)?),
            channel,
            byte_charge,
        )]);
    }
    let value = item
        .value
        .ok_or_else(|| AdapterError::protocol("private output item has no value"))?;
    let value = decode_public_session_schema_value(value)
        .map_err(|_| AdapterError::protocol("private output item is malformed"))?;
    if binary_lane(state.graph.as_ref().unwrap(), &schema) == Some(BinaryMessageKind::InputBinary) {
        validate_direct_value(
            state.graph.as_ref().unwrap(),
            &schema,
            &value,
            PublicErrorCode::ProtocolError,
        )?;
        let byte_charge = direct_binary_charge(&value)?;
        let SchemaValue::Binary(binary) = value else {
            return Err(AdapterError::protocol(
                "private binary stream item has the wrong schema value",
            ));
        };
        let message = BinaryMessage {
            metadata: BinaryMessageMetadata {
                channel,
                cursor_token: Some(cursor_token),
                item_count: DecimalU64(1),
                kind: BinaryMessageKind::OutputBinary,
                mime_type: binary.mime_type,
                sequence: DecimalU64(item.producer_sequence),
                version: 1,
            },
            payload: binary.bytes,
        };
        return Ok(vec![stream_frame(
            Message::binary(encode_binary_message(&message)?),
            channel,
            byte_charge,
        )]);
    }
    let graph = state.graph.clone().unwrap();
    let mut mappings = Vec::new();
    let (value, byte_charge) =
        encode_public_schema_value_with_charge(&graph, &schema, &value, |stream, element| {
            let (reference, mapping) = state.expose_output_reference(stream, element, keyring)?;
            mappings.push(mapping);
            Ok(reference)
        })?;
    Ok(vec![stream_frame(
        text_message(&PublicServerMessage::OutputStreamItem {
            channel,
            cursor_token,
            mappings,
            sequence: DecimalU64(item.producer_sequence),
            value,
            version: 1,
        })?,
        channel,
        byte_charge,
    )])
}

fn translate_output_end(
    state: &mut AdapterState,
    end: OutputStreamEnd,
    keyring: &InvocationSessionTokenKeyring,
    outcome: PublicOutputStreamOutcome,
) -> Result<Vec<TranslatedFrame>, AdapterError> {
    let channel = channel_for_transport(state, end.transport_stream_id)?;
    let cursor_token = sign_cursor_token(keyring, state, channel, end.durable_offset)?;
    state.channels.get_mut(&channel).unwrap().terminal = true;
    Ok(vec![stream_terminal_frame(
        text_message(&PublicServerMessage::OutputStreamEnd {
            channel,
            cursor_token: Some(cursor_token),
            outcome,
            sequence: DecimalU64(end.producer_sequence),
            version: 1,
        })?,
        channel,
        1,
    )])
}

fn translate_output_error(
    state: &mut AdapterState,
    error: OutputStreamError,
    keyring: &InvocationSessionTokenKeyring,
) -> Result<Vec<TranslatedFrame>, AdapterError> {
    translate_output_end(
        state,
        OutputStreamEnd {
            transport_stream_id: error.transport_stream_id,
            producer_sequence: error.producer_sequence,
            durable_stream_id: error.durable_stream_id,
            durable_offset: error.durable_offset,
            epoch: error.epoch,
        },
        keyring,
        PublicOutputStreamOutcome::Error {
            code: PublicErrorCode::ProducerError,
            message: "output producer failed".to_string(),
        },
    )
}

fn input_channel(
    state: &AdapterState,
    channel: u32,
) -> Result<(u64, Uuid, u64, SchemaType), AdapterError> {
    let channel_state = state.channels.get(&channel).ok_or_else(|| {
        AdapterError::new(PublicErrorCode::InvalidChannel, "unknown stream channel")
    })?;
    if channel_state.direction != PublicStreamDirection::Input || !channel_state.exposed {
        return Err(AdapterError::new(
            PublicErrorCode::InvalidChannel,
            "stream channel is not an input channel",
        ));
    }
    let durable = channel_state
        .durable
        .as_ref()
        .ok_or_else(|| AdapterError::protocol("input channel has no durable mapping"))?;
    Ok((
        channel_state.transport_id,
        durable.durable_stream_id,
        state.attachment_epoch,
        channel_state.schema.clone(),
    ))
}

fn translate_cancel(
    state: &AdapterState,
    channel: u32,
    reason: PublicClientCancelReason,
) -> Result<InvocationRequest, AdapterError> {
    let attachment_epoch = state.attachment_epoch;
    let channel_state = state.channels.get(&channel).ok_or_else(|| {
        AdapterError::new(PublicErrorCode::InvalidChannel, "unknown stream channel")
    })?;
    if !channel_state.exposed
        || (channel_state.terminal && channel_state.direction == PublicStreamDirection::Input)
    {
        return Err(AdapterError::new(
            PublicErrorCode::InvalidChannel,
            "stream channel is not open",
        ));
    }
    let durable = channel_state
        .durable
        .as_ref()
        .ok_or_else(|| AdapterError::protocol("stream channel has no durable mapping"))?;
    let role = match channel_state.direction {
        PublicStreamDirection::Input => StreamCancelRole::InputProducer,
        PublicStreamDirection::Output => StreamCancelRole::OutputConsumer,
    };
    let reason = match reason {
        PublicClientCancelReason::Cancelled => StreamCancelReason::Cancelled,
        PublicClientCancelReason::ConsumerDrop => StreamCancelReason::ConsumerDrop,
        PublicClientCancelReason::SourceUnavailable => StreamCancelReason::SourceUnavailable,
    };
    Ok(request(invocation_request::Request::StreamCancel(
        StreamCancel {
            transport_stream_id: channel_state.transport_id,
            producer_sequence: 0,
            role: role as i32,
            reason: reason as i32,
            details: None,
            durable_stream_id: Some(durable.durable_stream_id.into()),
            epoch: attachment_epoch,
            durable_offset: Vec::new(),
        },
    )))
}

fn sign_stream_token(
    keyring: &InvocationSessionTokenKeyring,
    tokens: &TokenContext,
    private: &PrivateMapping,
) -> Result<String, AdapterError> {
    let handle = private.mapping.handle.as_ref().unwrap();
    let producer: golem_common::model::AgentId = handle
        .producer
        .clone()
        .ok_or_else(|| AdapterError::protocol("stream mapping has no producer"))?
        .try_into()
        .map_err(|_| AdapterError::protocol("stream mapping producer is invalid"))?;
    let producer_incarnation = required_uuid(
        handle.expected_producer_fingerprint.as_ref(),
        "producer fingerprint",
    )?;
    let component_revision = handle
        .component_revision
        .ok_or_else(|| AdapterError::protocol("stream mapping has no pinned component revision"))?;
    let mapping_id = mapping_uuid(
        tokens.logical_invocation_id,
        private.durable_stream_id,
        private.direction,
    );
    keyring
        .sign_with_key_id(
            &tokens.key_id,
            &tokens.bindings,
            &InvocationSessionTokenPayload::Stream(StreamTokenPayload {
                parent_logical_invocation_id: tokens.logical_invocation_id,
                durable_stream_id: private.durable_stream_id,
                producer: producer.to_string(),
                producer_incarnation,
                component_revision,
                schema_fingerprint: private.schema_fingerprint,
                role: match private.direction {
                    PublicStreamDirection::Input => StreamTokenRole::Input,
                    PublicStreamDirection::Output => StreamTokenRole::Output,
                },
                durable_mapping_id: mapping_id,
            }),
        )
        .map_err(|error| AdapterError::new(error.code, error.to_string()))
}

fn sign_cursor_token(
    keyring: &InvocationSessionTokenKeyring,
    state: &AdapterState,
    channel: u32,
    durable_offset: Vec<u8>,
) -> Result<String, AdapterError> {
    let channel = state
        .channels
        .get(&channel)
        .ok_or_else(|| AdapterError::protocol("output channel is unavailable"))?;
    let durable = channel
        .durable
        .as_ref()
        .ok_or_else(|| AdapterError::protocol("output channel has no durable mapping"))?;
    let tokens = state
        .tokens
        .as_ref()
        .ok_or_else(|| AdapterError::protocol("invocation was not accepted"))?;
    keyring
        .sign(
            &tokens.bindings,
            &InvocationSessionTokenPayload::Cursor(CursorTokenPayload {
                parent_logical_invocation_id: tokens.logical_invocation_id,
                output_durable_stream_id: durable.durable_stream_id,
                durable_offset,
            }),
        )
        .map_err(|error| AdapterError::new(error.code, error.to_string()))
}

fn token_bindings(auth: &AuthCtx) -> InvocationSessionTokenBindings {
    let effective_principal = match auth {
        AuthCtx::System => "system".to_string(),
        AuthCtx::User(user) => format!("user:{}", user.account_id),
        AuthCtx::Agent(agent) => format!("agent:{}", agent.account_id),
        AuthCtx::AdminImpersonation(admin) => format!(
            "admin:{}:as:{}",
            admin.admin_account_id, admin.target_account_id
        ),
    };
    InvocationSessionTokenBindings {
        account: auth.access_account_id().to_string(),
        effective_principal,
        issued_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
}

fn public_start_error(error: PublicAgentSessionStartError) -> (PublicErrorCode, String) {
    match error {
        PublicAgentSessionStartError::Protocol(error) => (error.code, error.message),
        PublicAgentSessionStartError::Worker(error) => {
            let code = match &error {
                WorkerServiceError::TypeChecker(_)
                | WorkerServiceError::RegistryServiceError(RegistryServiceError::BadRequest(_)) => {
                    PublicErrorCode::ValidationError
                }
                WorkerServiceError::AuthError(AuthServiceError::Unauthorized(_))
                | WorkerServiceError::RegistryServiceError(
                    RegistryServiceError::Unauthorized(_)
                    | RegistryServiceError::CouldNotAuthenticate(_),
                ) => PublicErrorCode::Unauthorized,
                WorkerServiceError::ComponentNotFound(_)
                | WorkerServiceError::AgentNotFound(_)
                | WorkerServiceError::RegistryServiceError(RegistryServiceError::NotFound(_)) => {
                    PublicErrorCode::NotFound
                }
                _ => PublicErrorCode::InternalError,
            };
            (code, error.to_safe_string())
        }
    }
}

fn rejection_code(reason: i32) -> PublicErrorCode {
    match InvocationRejectionReason::try_from(reason) {
        Ok(InvocationRejectionReason::Validation) => PublicErrorCode::ValidationError,
        Ok(InvocationRejectionReason::Unauthorized) => PublicErrorCode::Unauthorized,
        Ok(InvocationRejectionReason::NotFound) => PublicErrorCode::NotFound,
        Ok(InvocationRejectionReason::Protocol) => PublicErrorCode::ProtocolError,
        Ok(InvocationRejectionReason::IdempotencyConflict) => PublicErrorCode::IdempotencyConflict,
        Ok(InvocationRejectionReason::AttemptConflict) => PublicErrorCode::AttemptConflict,
        Ok(InvocationRejectionReason::StaleEpoch) => PublicErrorCode::StaleSession,
        Ok(InvocationRejectionReason::InvalidEpoch) => PublicErrorCode::FutureSession,
        Ok(InvocationRejectionReason::InvalidAttachmentState) => {
            PublicErrorCode::InvalidAttachmentState
        }
        Ok(InvocationRejectionReason::InputConflict) => PublicErrorCode::InputConflict,
        Ok(InvocationRejectionReason::InputGap) => PublicErrorCode::InputGap,
        Ok(InvocationRejectionReason::ResourceExhausted) => PublicErrorCode::ResourceExhausted,
        _ => PublicErrorCode::InternalError,
    }
}

fn safe_rejection_message(code: PublicErrorCode) -> String {
    match code {
        PublicErrorCode::UnsupportedSubprotocol => "required WebSocket subprotocol is unsupported",
        PublicErrorCode::AuthenticationFailed => "authentication failed",
        PublicErrorCode::ValidationError => "invocation validation failed",
        PublicErrorCode::Unauthorized => "invocation is not authorized",
        PublicErrorCode::NotFound => "invocation target was not found",
        PublicErrorCode::UnsupportedValue => "invocation contains an unsupported value",
        PublicErrorCode::MalformedMessage => "invocation message is malformed",
        PublicErrorCode::UnsupportedVersion => "invocation protocol version is unsupported",
        PublicErrorCode::InvalidChannel => "stream channel is invalid",
        PublicErrorCode::InvalidSequence => "stream sequence is invalid",
        PublicErrorCode::StreamAlreadyConsumed => "stream reference was already consumed",
        PublicErrorCode::StreamConflict => "stream mapping conflicts with the session",
        PublicErrorCode::IdempotencyConflict => "idempotency key conflict",
        PublicErrorCode::AttemptConflict => "attempt conflict",
        PublicErrorCode::StaleSession => "session attachment is stale",
        PublicErrorCode::FutureSession => "session attachment generation is invalid",
        PublicErrorCode::InvalidAttachmentState => "session attachment state is invalid",
        PublicErrorCode::InputConflict => "input conflicts with durable history",
        PublicErrorCode::InputGap => "input sequence has a gap",
        PublicErrorCode::InvalidCursor => "output cursor is invalid",
        PublicErrorCode::TokenInvalid => "session token is invalid",
        PublicErrorCode::ResourceExhausted => "session resource limit exceeded",
        PublicErrorCode::ProducerError => "stream producer failed",
        PublicErrorCode::InvocationFailed => "invocation failed",
        PublicErrorCode::ProtocolError => "invocation protocol failed",
        PublicErrorCode::InternalError => "invocation failed",
    }
    .to_string()
}

fn binary_lane(graph: &SchemaGraph, schema: &SchemaType) -> Option<BinaryMessageKind> {
    match schema {
        SchemaType::Ref { id, .. } => graph
            .lookup(id)
            .and_then(|definition| binary_lane(graph, &definition.body)),
        SchemaType::U8 { .. } => Some(BinaryMessageKind::InputU8),
        SchemaType::Binary { .. } => Some(BinaryMessageKind::InputBinary),
        _ => None,
    }
}

fn validate_direct_value(
    graph: &SchemaGraph,
    schema: &SchemaType,
    value: &SchemaValue,
    code: PublicErrorCode,
) -> Result<(), AdapterError> {
    validate_value(graph, schema, value)
        .map_err(|_| AdapterError::new(code, "binary item does not satisfy its element schema"))
}

fn validate_packed_u8(
    graph: &SchemaGraph,
    schema: &SchemaType,
    payload: &[u8],
    code: PublicErrorCode,
) -> Result<(), AdapterError> {
    let mut validated = [false; 256];
    for byte in payload {
        let index = *byte as usize;
        if !validated[index] {
            validate_direct_value(graph, schema, &SchemaValue::U8(*byte), code)?;
            validated[index] = true;
        }
    }
    Ok(())
}

fn direct_binary_charge(value: &SchemaValue) -> Result<usize, AdapterError> {
    let SchemaValue::Binary(binary) = value else {
        return Err(AdapterError::protocol(
            "binary lane value has the wrong schema value",
        ));
    };
    let charge = 1_usize
        .checked_add(binary.bytes.len())
        .and_then(|charge| {
            charge.checked_add(
                binary
                    .mime_type
                    .as_ref()
                    .map(|mime| mime.len())
                    .unwrap_or_default(),
            )
        })
        .ok_or_else(|| {
            AdapterError::new(
                PublicErrorCode::ResourceExhausted,
                "binary item byte charge overflow",
            )
        })?;
    if charge > MAX_LOGICAL_VALUE_SIZE {
        return Err(AdapterError::new(
            PublicErrorCode::ResourceExhausted,
            "binary item exceeds the logical value byte limit",
        ));
    }
    Ok(charge)
}

fn channel_for_transport(state: &AdapterState, transport_id: u64) -> Result<u32, AdapterError> {
    state
        .channel_by_transport
        .get(&transport_id)
        .copied()
        .ok_or_else(|| {
            AdapterError::new(
                PublicErrorCode::InvalidChannel,
                "private response references an unknown transport stream",
            )
        })
}

fn required_uuid(
    value: Option<&golem_api_grpc::proto::golem::common::Uuid>,
    name: &str,
) -> Result<Uuid, AdapterError> {
    Ok(value
        .cloned()
        .ok_or_else(|| AdapterError::protocol(format!("private {name} is missing")))?
        .into())
}

fn invocation_uuid(
    agent_id: &golem_common::model::AgentId,
    idempotency_key: &str,
    callee_incarnation: Uuid,
) -> Uuid {
    let identity = format!("{}\0{idempotency_key}\0{callee_incarnation}", agent_id);
    Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes())
}

fn deterministic_uuid(value: &[u8]) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, value)
}

fn mapping_uuid(
    logical_invocation_id: Uuid,
    durable_stream_id: Uuid,
    direction: PublicStreamDirection,
) -> Uuid {
    let mut value = Vec::with_capacity(33);
    value.extend_from_slice(durable_stream_id.as_bytes());
    value.push(match direction {
        PublicStreamDirection::Input => 1,
        PublicStreamDirection::Output => 2,
    });
    Uuid::new_v5(&logical_invocation_id, &value)
}

fn server_cancel_reason(reason: i32) -> PublicServerCancelReason {
    match StreamCancelReason::try_from(reason) {
        Ok(StreamCancelReason::ConsumerDrop) => PublicServerCancelReason::ConsumerDrop,
        Ok(StreamCancelReason::Transport) => PublicServerCancelReason::TransportDetached,
        Ok(StreamCancelReason::SourceUnavailable) => PublicServerCancelReason::SourceUnavailable,
        Ok(StreamCancelReason::ProducerDeleting) => PublicServerCancelReason::ProducerDeleted,
        Ok(StreamCancelReason::InvocationFailed) => PublicServerCancelReason::InvocationFailed,
        Ok(StreamCancelReason::Protocol) => PublicServerCancelReason::ProtocolError,
        _ => PublicServerCancelReason::Cancelled,
    }
}

fn request(request: invocation_request::Request) -> InvocationRequest {
    InvocationRequest {
        request: Some(request),
    }
}

fn text_message(message: &PublicServerMessage) -> Result<Message, AdapterError> {
    Ok(Message::text(encode_text(message)?))
}

fn frame(message: Message) -> TranslatedFrame {
    TranslatedFrame {
        message,
        stream_channel: None,
        stream_byte_charge: None,
        preserve_after_output_cancel: false,
        cancel_output_before_send: None,
    }
}

fn stream_frame(message: Message, channel: u32, byte_charge: usize) -> TranslatedFrame {
    TranslatedFrame {
        message,
        stream_channel: Some(channel),
        stream_byte_charge: Some(byte_charge),
        preserve_after_output_cancel: false,
        cancel_output_before_send: None,
    }
}

fn stream_terminal_frame(message: Message, channel: u32, byte_charge: usize) -> TranslatedFrame {
    TranslatedFrame {
        message,
        stream_channel: Some(channel),
        stream_byte_charge: Some(byte_charge),
        preserve_after_output_cancel: true,
        cancel_output_before_send: None,
    }
}

fn cancel_frame(message: Message, channel: u32) -> TranslatedFrame {
    TranslatedFrame {
        message,
        stream_channel: None,
        stream_byte_charge: None,
        preserve_after_output_cancel: false,
        cancel_output_before_send: Some(channel),
    }
}

fn message_size(message: &Message) -> usize {
    match message {
        Message::Text(text) => text.len(),
        Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => bytes.len(),
        Message::Close(Some((_, reason))) => reason.len(),
        Message::Close(None) => 0,
    }
}

async fn acquire_bytes(
    semaphore: &Arc<Semaphore>,
    bytes: usize,
) -> Result<OwnedSemaphorePermit, ()> {
    let bytes = u32::try_from(bytes).map_err(|_| ())?;
    tokio::time::timeout(
        WRITE_PROGRESS_TIMEOUT,
        semaphore.clone().acquire_many_owned(bytes),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}

async fn acquire_input_admission(
    budgets: &SessionBudgets,
    terminal: bool,
    channel: u32,
    bytes: usize,
) -> Result<PendingInputAdmission, AdapterError> {
    let unacknowledged = acquire_bytes(&budgets.unacknowledged_input, bytes)
        .await
        .map_err(|_| {
            AdapterError::new(
                PublicErrorCode::ResourceExhausted,
                "unacknowledged input byte budget exhausted",
            )
        })?;
    let stream_bytes =
        SessionBudgets::stream_budget(&budgets.stream_bytes, channel, STREAM_BYTE_BUDGET).await;
    let stream_items =
        SessionBudgets::stream_budget(&budgets.stream_items, channel, STREAM_ITEM_BUDGET).await;
    let stream_bytes = acquire_bytes(&stream_bytes, bytes).await.map_err(|_| {
        AdapterError::new(
            PublicErrorCode::ResourceExhausted,
            "input stream byte budget exhausted",
        )
    })?;
    let stream_item = acquire_bytes(&stream_items, 1).await.map_err(|_| {
        AdapterError::new(
            PublicErrorCode::ResourceExhausted,
            "input stream item budget exhausted",
        )
    })?;
    Ok(PendingInputAdmission {
        terminal,
        _unacknowledged: unacknowledged,
        _stream_bytes: stream_bytes,
        _stream_item: stream_item,
    })
}

async fn send_text(sender: &Outbound, message: &PublicServerMessage) -> Result<(), ()> {
    let message = text_message(message).map_err(|_| ())?;
    sender.send(message, None).await
}

async fn send_rejection(
    sender: &Outbound,
    attempt_id: Option<Uuid>,
    code: PublicErrorCode,
    message: impl Into<String>,
) {
    let _ = send_text(
        sender,
        &PublicServerMessage::InvocationRejected {
            attempt_id,
            code,
            message: message.into(),
            retryable: false,
            version: 1,
        },
    )
    .await;
}

async fn close_protocol(sender: &Outbound, reason: &str) {
    let _ = sender
        .send(
            Message::close_with(CloseCode::Protocol, bounded_close_reason(reason)),
            None,
        )
        .await;
}

async fn close_error(sender: &Outbound, reason: &str) {
    let _ = sender
        .send(
            Message::close_with(CloseCode::Error, bounded_close_reason(reason)),
            None,
        )
        .await;
}

async fn close_too_large(sender: &Outbound) {
    let _ = sender
        .send(
            Message::close_with(CloseCode::Size, "application message exceeds 32 MiB"),
            None,
        )
        .await;
}

fn bounded_close_reason(reason: &str) -> String {
    const MAX_BYTES: usize = 123;
    if reason.len() <= MAX_BYTES {
        return reason.to_string();
    }
    let mut end = MAX_BYTES;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InvocationSessionTokenConfig, InvocationSessionTokenKeyConfig};
    use golem_api_grpc::proto::golem::common::{EnvironmentId, Uuid as ProtoUuid};
    use golem_api_grpc::proto::golem::component::ComponentId;
    use golem_api_grpc::proto::golem::schema::{
        SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, schema_value,
    };
    use golem_api_grpc::proto::golem::worker::{
        AgentId as ProtoAgentId, DurableStreamHandle, IdempotencyKey as ProtoIdempotencyKey,
        InputStreamAck, InvocationStart, StreamInvocationIdentity,
    };
    use golem_common::base_model::base64::Base64;
    use golem_common::schema::schema_type::{NumericBound, NumericRestrictions};
    use golem_common::schema::{BinaryRestrictions, MetadataEnvelope};
    use test_r::test;

    fn proto_uuid(value: u64) -> ProtoUuid {
        ProtoUuid {
            high_bits: 0,
            low_bits: value,
        }
    }

    fn proto_agent_id() -> ProtoAgentId {
        ProtoAgentId {
            component_id: Some(ComponentId {
                value: Some(proto_uuid(20)),
            }),
            name: "agent".to_string(),
        }
    }

    fn idempotency_key() -> Option<ProtoIdempotencyKey> {
        Some(ProtoIdempotencyKey {
            value: "session-key".to_string(),
        })
    }

    fn stream_value(transport_id: u64) -> ProtoSchemaValue {
        ProtoSchemaValue {
            value: Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference {
                    stream_id: transport_id,
                },
            )),
        }
    }

    fn durable_offset(value: u64) -> Vec<u8> {
        let mut offset = vec![0; 24];
        offset[0] = 1;
        offset[8..16].copy_from_slice(&value.to_be_bytes());
        offset
    }

    fn private_mapping(
        transport_id: u64,
        role: StreamMappingRole,
        schema_fingerprint: [u8; 32],
    ) -> DurableStreamMapping {
        DurableStreamMapping {
            transport_stream_id: transport_id,
            handle: Some(DurableStreamHandle {
                format_version: 1,
                stream_id: Some(proto_uuid(100 + transport_id)),
                producer_environment_id: Some(EnvironmentId {
                    value: Some(proto_uuid(3)),
                }),
                producer: Some(proto_agent_id()),
                expected_producer_fingerprint: Some(proto_uuid(4)),
                source_invocation: Some(StreamInvocationIdentity {
                    callee_environment_id: Some(EnvironmentId {
                        value: Some(proto_uuid(3)),
                    }),
                    callee: Some(proto_agent_id()),
                    callee_fingerprint: Some(proto_uuid(4)),
                    idempotency_key: idempotency_key(),
                }),
                component_revision: Some(12),
                element_schema_fingerprint: schema_fingerprint.to_vec(),
            }),
            high_water: None,
            role: role as i32,
        }
    }

    fn active_input_state() -> Arc<Mutex<AdapterState>> {
        active_input_state_with_schema(SchemaType::u8())
    }

    fn active_input_state_with_schema(schema: SchemaType) -> Arc<Mutex<AdapterState>> {
        let graph = SchemaGraph {
            defs: Vec::new(),
            root: SchemaType::stream(Some(schema.clone())),
        };
        let fingerprint = schema_fingerprint_v1(&graph, Some(&schema)).unwrap().0;
        let mapping = private_mapping(7, StreamMappingRole::Input, fingerprint);
        let mut protocol_state = InvocationSessionState::default();
        protocol_state
            .validate_trusted_request(&request(invocation_request::Request::Start(
                InvocationStart {
                    input: Some(stream_value(7)),
                    idempotency_key: idempotency_key(),
                    ..Default::default()
                },
            )))
            .unwrap();
        protocol_state
            .validate_response(&InvocationResponse {
                response: Some(invocation_response::Response::Accepted(
                    InvocationAccepted {
                        agent_id: Some(proto_agent_id()),
                        idempotency_key: idempotency_key(),
                        component_revision: Some(12),
                        attachment_id: Some(proto_uuid(1)),
                        attempt_id: Some(proto_uuid(2)),
                        epoch: 1,
                        stream_mappings: vec![mapping.clone()],
                        environment_id: Some(EnvironmentId {
                            value: Some(proto_uuid(3)),
                        }),
                        callee_fingerprint: Some(proto_uuid(4)),
                        method_name: Some("run".to_string()),
                    },
                )),
            })
            .unwrap();
        let private = PrivateMapping {
            mapping,
            durable_stream_id: Uuid::from_u128(107),
            schema_fingerprint: fingerprint,
            direction: PublicStreamDirection::Input,
        };
        let mut channels = HashMap::new();
        channels.insert(
            1,
            ChannelState {
                transport_id: 7,
                direction: PublicStreamDirection::Input,
                schema,
                provisional_ref: None,
                durable: Some(private),
                stream_token: Some("stream-token".to_string()),
                next_input_sequence: 0,
                pending_input: VecDeque::new(),
                exposed: true,
                terminal: false,
            },
        );
        Arc::new(Mutex::new(AdapterState {
            protocol_state,
            graph: Some(graph),
            output_schema: None,
            identity: None,
            application: Some("app".to_string()),
            environment: Some("env".to_string()),
            next_channel: 2,
            next_transport_id: 8,
            channels,
            channel_by_transport: HashMap::from([(7, 1)]),
            provisional_refs: HashMap::new(),
            private_mappings: HashMap::new(),
            tokens: Some(TokenContext {
                bindings: token_bindings(&AuthCtx::system()),
                key_id: keyring().active_key_id().to_string(),
                logical_invocation_id: Uuid::from_u128(10),
            }),
            attachment_epoch: 1,
        }))
    }

    fn keyring() -> InvocationSessionTokenKeyring {
        InvocationSessionTokenKeyring::new(&InvocationSessionTokenConfig::default()).unwrap()
    }

    fn input_item(sequence: u64, value: u8) -> PublicClientMessage {
        PublicClientMessage::InputStreamItem {
            channel: 1,
            sequence: DecimalU64(sequence),
            value: serde_json::json!(value),
            version: 1,
        }
    }

    fn input_ack(sequence: u64, offset: Vec<u8>) -> InvocationResponse {
        InvocationResponse {
            response: Some(invocation_response::Response::InputAck(InputStreamAck {
                transport_stream_id: 7,
                highest_contiguous_sequence: sequence,
                logical_item_count: 1,
                durable_stream_id: Some(proto_uuid(107)),
                resulting_offset: offset,
                epoch: 1,
                new_stream_mappings: Vec::new(),
            })),
        }
    }

    async fn admit_input(state: &Arc<Mutex<AdapterState>>, terminal: bool) {
        let admission = acquire_input_admission(&SessionBudgets::new(), terminal, 1, 1)
            .await
            .unwrap();
        state
            .lock()
            .await
            .channels
            .get_mut(&1)
            .unwrap()
            .pending_input
            .push_back(admission);
    }

    #[test]
    async fn input_retries_gaps_and_terminal_high_water_use_public_sequences() {
        let state = active_input_state();
        translate_client_text(input_item(0, 7), &state)
            .await
            .unwrap();
        translate_client_text(input_item(0, 7), &state)
            .await
            .unwrap();
        let Err(conflict) = translate_client_text(input_item(0, 8), &state).await else {
            panic!("conflicting input retry was accepted")
        };
        assert_eq!(conflict.code, PublicErrorCode::InputConflict);
        let Err(gap) = translate_client_text(input_item(2, 7), &state).await else {
            panic!("input sequence gap was accepted")
        };
        assert_eq!(gap.code, PublicErrorCode::InputGap);

        admit_input(&state, false).await;
        admit_input(&state, false).await;
        for _ in 0..2 {
            let frames = translate_private_response(
                input_ack(0, durable_offset(1)),
                &state,
                &keyring(),
                &token_bindings(&AuthCtx::system()),
                keyring().active_key_id(),
                Uuid::from_u128(2),
            )
            .await
            .unwrap();
            let Message::Text(text) = &frames[0].message else {
                panic!("input acknowledgement was not text")
            };
            assert!(matches!(
                golem_common::model::invocation_session_public::decode_server_text(text.as_bytes())
                    .unwrap(),
                PublicServerMessage::InputStreamAck {
                    highest_contiguous_sequence: DecimalU64(1),
                    terminal: false,
                    ..
                }
            ));
        }

        translate_client_text(
            PublicClientMessage::InputStreamEnd {
                channel: 1,
                sequence: DecimalU64(1),
                version: 1,
            },
            &state,
        )
        .await
        .unwrap();
        admit_input(&state, true).await;
        let frames = translate_private_response(
            input_ack(1, durable_offset(2)),
            &state,
            &keyring(),
            &token_bindings(&AuthCtx::system()),
            keyring().active_key_id(),
            Uuid::from_u128(2),
        )
        .await
        .unwrap();
        let Message::Text(text) = &frames[0].message else {
            panic!("terminal acknowledgement was not text")
        };
        assert!(matches!(
            golem_common::model::invocation_session_public::decode_server_text(text.as_bytes())
                .unwrap(),
            PublicServerMessage::InputStreamAck {
                highest_contiguous_sequence: DecimalU64(1),
                terminal: true,
                ..
            }
        ));
    }

    #[test]
    fn connections_receive_fresh_channel_ranges() {
        let mut first = AdapterState::new_connection(Uuid::from_u128(0));
        let mut second = AdapterState::new_connection(Uuid::from_u128(1_u128 << 96));

        let first_channel = first.allocate_channel().unwrap();
        let second_channel = second.allocate_channel().unwrap();

        assert_ne!(first_channel, second_channel);
        assert!(first_channel > 0);
        assert!(second_channel > 0);
    }

    #[test]
    fn distinct_connections_never_reuse_public_channel_ranges() {
        let mut first = AdapterState::new_connection(Uuid::from_u128(0));
        let mut reconnected = AdapterState::new_connection(Uuid::from_u128(1));

        let first_channel = first.allocate_channel().unwrap();
        let reconnected_channel = reconnected.allocate_channel().unwrap();

        assert_ne!(
            first_channel, reconnected_channel,
            "a reconnect must receive a fresh public channel range"
        );
    }

    #[test]
    fn resumed_acceptance_refreshes_session_token_with_active_key() {
        let old_key = InvocationSessionTokenKeyConfig {
            id: "old".to_string(),
            key: Base64(vec![7; 32]),
        };
        let keyring = InvocationSessionTokenKeyring::new(&InvocationSessionTokenConfig {
            issuer: "test-deployment".to_string(),
            active_key: InvocationSessionTokenKeyConfig {
                id: "new".to_string(),
                key: Base64(vec![9; 32]),
            },
            verify_only_keys: vec![old_key],
        })
        .unwrap();
        let bindings = token_bindings(&AuthCtx::system());
        let mut state = AdapterState::new();
        state.identity = Some(SessionAgentIdentity {
            component_id: Uuid::from_u128(20),
            component_revision: 12,
            agent_type: "test-agent".to_string(),
            agent_id: "agent".to_string(),
            method: "run".to_string(),
        });
        state.application = Some("app".to_string());
        state.environment = Some("env".to_string());

        let accepted = translate_accepted(
            &mut state,
            InvocationAccepted {
                agent_id: Some(proto_agent_id()),
                idempotency_key: idempotency_key(),
                component_revision: Some(12),
                attachment_id: Some(proto_uuid(1)),
                attempt_id: Some(proto_uuid(2)),
                epoch: 2,
                stream_mappings: Vec::new(),
                environment_id: Some(EnvironmentId {
                    value: Some(proto_uuid(3)),
                }),
                callee_fingerprint: Some(proto_uuid(4)),
                method_name: Some("run".to_string()),
            },
            &keyring,
            &bindings,
            "old",
            Uuid::from_u128(2),
        )
        .unwrap();
        let PublicServerMessage::InvocationAccepted { session_token, .. } = accepted else {
            panic!("acceptance translated to the wrong public message")
        };
        let verified = keyring
            .verify(
                &session_token,
                InvocationSessionTokenKind::Session,
                &bindings.account,
                &bindings.effective_principal,
            )
            .unwrap();

        assert_eq!(verified.key_id, keyring.active_key_id());
        let InvocationSessionTokenPayload::Session(session) = verified.payload else {
            panic!("verified token has the wrong kind")
        };
        assert_eq!(session.stream_key_id, "old");
        assert_eq!(state.tokens.unwrap().key_id, "old");
    }

    #[test]
    fn accepted_transport_ids_are_reserved_and_exact_provisional_retries_are_stable() {
        let schema = SchemaType::u8();
        let graph = SchemaGraph::anonymous(schema.clone());
        let fingerprint = schema_fingerprint_v1(&graph, Some(&schema)).unwrap().0;
        let mut state = AdapterState::new();
        state.graph = Some(graph);
        state
            .add_private_mapping(private_mapping(1, StreamMappingRole::Input, fingerprint))
            .unwrap();

        let provisional_ref = Uuid::new_v4();
        let owner = ProvisionalOwner::InputItem {
            channel: 9,
            sequence: 3,
        };
        let first = state
            .register_provisional(
                PublicStreamReference::Provisional(provisional_ref),
                Some(&schema),
                owner,
            )
            .unwrap();
        let retry = state
            .register_provisional(
                PublicStreamReference::Provisional(provisional_ref),
                Some(&schema),
                owner,
            )
            .unwrap();
        assert_eq!(first.take_host_endpoint::<u64>().unwrap(), 2);
        assert_eq!(retry.take_host_endpoint::<u64>().unwrap(), 2);
        assert_eq!(state.channel_by_transport.get(&1), None);
        assert_eq!(state.channel_by_transport.get(&2), Some(&1));

        let error = state
            .register_provisional(
                PublicStreamReference::Provisional(provisional_ref),
                Some(&schema),
                ProvisionalOwner::InputItem {
                    channel: 9,
                    sequence: 4,
                },
            )
            .unwrap_err();
        assert_eq!(error.code, PublicErrorCode::StreamAlreadyConsumed);
    }

    #[test]
    async fn binary_lanes_enforce_schema_restrictions_and_logical_byte_budgets() {
        let binary_schema = SchemaType::binary(BinaryRestrictions {
            mime_types: Some(vec!["application/octet-stream".to_string()]),
            min_bytes: Some(2),
            max_bytes: Some(3),
        });
        let restricted_binary = active_input_state_with_schema(binary_schema);
        for (payload, mime_type) in [
            (vec![1], Some("application/octet-stream".to_string())),
            (vec![1, 2], Some("text/plain".to_string())),
            (
                vec![1, 2, 3, 4],
                Some("application/octet-stream".to_string()),
            ),
        ] {
            let Err(error) = translate_client_binary(
                BinaryMessage {
                    metadata: BinaryMessageMetadata {
                        channel: 1,
                        cursor_token: None,
                        item_count: DecimalU64(1),
                        kind: BinaryMessageKind::InputBinary,
                        mime_type,
                        sequence: DecimalU64(0),
                        version: 1,
                    },
                    payload,
                },
                &restricted_binary,
            )
            .await
            else {
                panic!("binary value outside its schema restrictions was accepted")
            };
            assert_eq!(error.code, PublicErrorCode::ValidationError);
        }

        translate_client_binary(
            BinaryMessage {
                metadata: BinaryMessageMetadata {
                    channel: 1,
                    cursor_token: None,
                    item_count: DecimalU64(1),
                    kind: BinaryMessageKind::InputBinary,
                    mime_type: Some("application/octet-stream".to_string()),
                    sequence: DecimalU64(0),
                    version: 1,
                },
                payload: vec![1, 2],
            },
            &restricted_binary,
        )
        .await
        .unwrap();

        let restricted_u8 = SchemaType::U8 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Unsigned(10)),
                max: Some(NumericBound::Unsigned(20)),
                unit: None,
            }),
            metadata: MetadataEnvelope::default(),
        };
        let Err(error) = translate_client_binary(
            BinaryMessage {
                metadata: BinaryMessageMetadata {
                    channel: 1,
                    cursor_token: None,
                    item_count: DecimalU64(1),
                    kind: BinaryMessageKind::InputU8,
                    mime_type: None,
                    sequence: DecimalU64(0),
                    version: 1,
                },
                payload: vec![9],
            },
            &active_input_state_with_schema(restricted_u8),
        )
        .await
        else {
            panic!("packed u8 value outside its numeric restrictions was accepted")
        };
        assert_eq!(error.code, PublicErrorCode::ValidationError);

        let maximum_binary = translate_client_binary(
            BinaryMessage {
                metadata: BinaryMessageMetadata {
                    channel: 1,
                    cursor_token: None,
                    item_count: DecimalU64(1),
                    kind: BinaryMessageKind::InputBinary,
                    mime_type: None,
                    sequence: DecimalU64(0),
                    version: 1,
                },
                payload: vec![0; MAX_LOGICAL_VALUE_SIZE - 1],
            },
            &active_input_state_with_schema(SchemaType::binary(BinaryRestrictions::default())),
        )
        .await
        .unwrap();
        let pending = maximum_binary.input.unwrap();
        assert_eq!(pending.byte_charge, MAX_LOGICAL_VALUE_SIZE);
        let budgets = SessionBudgets::new();
        let admission = acquire_input_admission(
            &budgets,
            pending.terminal,
            pending.channel,
            pending.byte_charge,
        )
        .await
        .unwrap();
        assert_eq!(budgets.unacknowledged_input.available_permits(), 0);
        drop(admission);
    }

    #[test]
    async fn byte_and_item_permits_are_held_until_the_downstream_owner_releases_them() {
        let budgets = SessionBudgets::new();
        let mut admissions = Vec::new();
        for _ in 0..STREAM_ITEM_BUDGET {
            admissions.push(
                acquire_input_admission(&budgets, false, 1, 1)
                    .await
                    .unwrap(),
            );
        }
        let items =
            SessionBudgets::stream_budget(&budgets.stream_items, 1, STREAM_ITEM_BUDGET).await;
        assert_eq!(items.available_permits(), 0);
        assert_eq!(
            budgets.unacknowledged_input.available_permits(),
            INPUT_UNACKNOWLEDGED_BYTE_BUDGET - STREAM_ITEM_BUDGET
        );
        admissions.pop();
        assert_eq!(items.available_permits(), 1);

        let (sender, mut receiver) = mpsc::channel(1);
        let outbound = Outbound {
            sender,
            budgets: budgets.clone(),
        };
        outbound
            .send(Message::text("four"), Some((2, 4)))
            .await
            .unwrap();
        assert_eq!(budgets.output.available_permits(), OUTPUT_BYTE_BUDGET - 4);
        drop(receiver.recv().await.unwrap());
        assert_eq!(budgets.output.available_permits(), OUTPUT_BYTE_BUDGET);

        outbound
            .send(
                Message::binary(vec![0; MAX_LOGICAL_VALUE_SIZE + 128]),
                Some((3, MAX_LOGICAL_VALUE_SIZE)),
            )
            .await
            .unwrap();
        let stream_bytes =
            SessionBudgets::stream_budget(&budgets.stream_bytes, 3, STREAM_BYTE_BUDGET).await;
        assert_eq!(stream_bytes.available_permits(), 0);
        drop(receiver.recv().await.unwrap());
        assert_eq!(stream_bytes.available_permits(), STREAM_BYTE_BUDGET);
    }

    #[test]
    async fn output_cancellation_drops_queued_items_but_preserves_a_queued_terminal() {
        let budgets = SessionBudgets::new();
        let (sender, mut receiver) = mpsc::channel(2);
        let outbound = Outbound {
            sender,
            budgets: budgets.clone(),
        };
        outbound
            .send(Message::text("item"), Some((7, 1)))
            .await
            .unwrap();
        outbound
            .send_stream_terminal(Message::text("terminal"), 7, 1)
            .await
            .unwrap();

        outbound.cancel_output(7).await;

        let item = receiver.recv().await.unwrap();
        assert!(item.should_drop(&budgets).await);
        let terminal = receiver.recv().await.unwrap();
        assert!(!terminal.should_drop(&budgets).await);
        assert!(matches!(terminal.message, Message::Text(text) if text == "terminal"));
    }

    #[test]
    async fn fatal_failure_terminalizes_all_open_streams_before_finishing() {
        let mut state = AdapterState::new();
        state.tokens = Some(TokenContext {
            bindings: token_bindings(&AuthCtx::system()),
            key_id: keyring().active_key_id().to_string(),
            logical_invocation_id: Uuid::from_u128(10),
        });
        for channel in [1, 2] {
            state.channels.insert(
                channel,
                ChannelState {
                    transport_id: channel as u64,
                    direction: if channel == 1 {
                        PublicStreamDirection::Input
                    } else {
                        PublicStreamDirection::Output
                    },
                    schema: SchemaType::u8(),
                    provisional_ref: None,
                    durable: None,
                    stream_token: None,
                    next_input_sequence: 0,
                    pending_input: VecDeque::new(),
                    exposed: true,
                    terminal: false,
                },
            );
        }
        let state = Arc::new(Mutex::new(state));
        let budgets = SessionBudgets::new();
        let (sender, mut receiver) = mpsc::channel(8);
        let outbound = Outbound { sender, budgets };
        fail_session(
            &outbound,
            &state,
            Uuid::from_u128(2),
            PublicErrorCode::InvocationFailed,
        )
        .await;

        let mut cancelled = HashSet::new();
        for _ in 0..2 {
            let Message::Text(text) = receiver.recv().await.unwrap().message else {
                panic!("stream terminal was not text")
            };
            let PublicServerMessage::StreamCancel {
                channel,
                reason: PublicServerCancelReason::InvocationFailed,
                ..
            } = golem_common::model::invocation_session_public::decode_server_text(text.as_bytes())
                .unwrap()
            else {
                panic!("fatal stream terminal had the wrong shape")
            };
            cancelled.insert(channel);
        }
        assert_eq!(cancelled, HashSet::from([1, 2]));
        let Message::Text(text) = receiver.recv().await.unwrap().message else {
            panic!("invocation terminal was not text")
        };
        assert!(matches!(
            golem_common::model::invocation_session_public::decode_server_text(text.as_bytes())
                .unwrap(),
            PublicServerMessage::InvocationFinished { .. }
        ));
        assert!(matches!(
            receiver.recv().await.unwrap().message,
            Message::Close(Some((CloseCode::Normal, _)))
        ));
    }

    #[test]
    async fn received_output_cancellation_may_race_with_queued_terminal() {
        let graph = SchemaGraph {
            defs: Vec::new(),
            root: SchemaType::stream(Some(SchemaType::u8())),
        };
        let fingerprint = schema_fingerprint_v1(&graph, Some(&SchemaType::u8()))
            .unwrap()
            .0;
        let mapping = private_mapping(8, StreamMappingRole::Output, fingerprint);
        let mut protocol_state = InvocationSessionState::default();
        protocol_state
            .validate_trusted_request(&request(invocation_request::Request::Start(
                InvocationStart {
                    input: Some(ProtoSchemaValue {
                        value: Some(schema_value::Value::U8Value(1)),
                    }),
                    idempotency_key: idempotency_key(),
                    ..Default::default()
                },
            )))
            .unwrap();
        protocol_state
            .validate_response(&InvocationResponse {
                response: Some(invocation_response::Response::Accepted(
                    InvocationAccepted {
                        agent_id: Some(proto_agent_id()),
                        idempotency_key: idempotency_key(),
                        component_revision: Some(12),
                        attachment_id: Some(proto_uuid(1)),
                        attempt_id: Some(proto_uuid(2)),
                        epoch: 1,
                        stream_mappings: Vec::new(),
                        environment_id: Some(EnvironmentId {
                            value: Some(proto_uuid(3)),
                        }),
                        callee_fingerprint: Some(proto_uuid(4)),
                        method_name: Some("run".to_string()),
                    },
                )),
            })
            .unwrap();
        protocol_state
            .validate_response(&InvocationResponse {
                response: Some(invocation_response::Response::Result(
                    InvocationSessionResult {
                        result: Some(invocation_session_result::Result::MethodResult(
                            stream_value(8),
                        )),
                        component_revision: Some(12),
                        agent_id: Some(proto_agent_id()),
                        idempotency_key: idempotency_key(),
                        new_stream_mappings: vec![mapping.clone()],
                        ..Default::default()
                    },
                )),
            })
            .unwrap();
        protocol_state
            .validate_response(&InvocationResponse {
                response: Some(invocation_response::Response::OutputEnd(OutputStreamEnd {
                    transport_stream_id: 8,
                    producer_sequence: 0,
                    durable_stream_id: Some(proto_uuid(108)),
                    durable_offset: durable_offset(1),
                    epoch: 1,
                })),
            })
            .unwrap();

        let private = PrivateMapping {
            mapping,
            durable_stream_id: Uuid::from_u128(108),
            schema_fingerprint: fingerprint,
            direction: PublicStreamDirection::Output,
        };
        let state = Arc::new(Mutex::new(AdapterState {
            protocol_state,
            graph: Some(graph),
            output_schema: None,
            identity: None,
            application: Some("app".to_string()),
            environment: Some("env".to_string()),
            next_channel: 2,
            next_transport_id: 9,
            channels: HashMap::from([(
                1,
                ChannelState {
                    transport_id: 8,
                    direction: PublicStreamDirection::Output,
                    schema: SchemaType::u8(),
                    provisional_ref: None,
                    durable: Some(private),
                    stream_token: Some("stream-token".to_string()),
                    next_input_sequence: 0,
                    pending_input: VecDeque::new(),
                    exposed: true,
                    terminal: true,
                },
            )]),
            channel_by_transport: HashMap::from([(8, 1)]),
            provisional_refs: HashMap::new(),
            private_mappings: HashMap::new(),
            tokens: None,
            attachment_epoch: 1,
        }));

        translate_client_text(
            PublicClientMessage::StreamCancel {
                channel: 1,
                reason: PublicClientCancelReason::ConsumerDrop,
                version: 1,
            },
            &state,
        )
        .await
        .expect("a received cancellation may race with a terminal not yet observed by the client");
    }

    #[test]
    async fn dynamically_discovered_output_uses_binary_frames_and_opaque_tokens() {
        let graph = SchemaGraph {
            defs: Vec::new(),
            root: SchemaType::stream(Some(SchemaType::u8())),
        };
        let fingerprint = schema_fingerprint_v1(&graph, Some(&SchemaType::u8()))
            .unwrap()
            .0;
        let mut state = AdapterState::new();
        state.graph = Some(graph);
        state.output_schema = Some(SchemaType::stream(Some(SchemaType::u8())));
        state.identity = Some(SessionAgentIdentity {
            component_id: Uuid::from_u128(20),
            component_revision: 12,
            agent_type: "test-agent".to_string(),
            agent_id: "agent".to_string(),
            method: "run".to_string(),
        });
        state.application = Some("app".to_string());
        state.environment = Some("env".to_string());
        state
            .protocol_state
            .validate_trusted_request(&request(invocation_request::Request::Start(
                InvocationStart {
                    input: Some(ProtoSchemaValue {
                        value: Some(schema_value::Value::U8Value(1)),
                    }),
                    idempotency_key: idempotency_key(),
                    ..Default::default()
                },
            )))
            .unwrap();
        let state = Arc::new(Mutex::new(state));
        let keyring = keyring();
        let bindings = token_bindings(&AuthCtx::system());
        let attempt_id = "00000000-0000-4000-8000-000000000002"
            .parse::<Uuid>()
            .unwrap();
        let accepted = InvocationResponse {
            response: Some(invocation_response::Response::Accepted(
                InvocationAccepted {
                    agent_id: Some(proto_agent_id()),
                    idempotency_key: idempotency_key(),
                    component_revision: Some(12),
                    attachment_id: Some(proto_uuid(1)),
                    attempt_id: Some(attempt_id.into()),
                    epoch: 1,
                    stream_mappings: Vec::new(),
                    environment_id: Some(EnvironmentId {
                        value: Some(proto_uuid(3)),
                    }),
                    callee_fingerprint: Some(proto_uuid(4)),
                    method_name: Some("run".to_string()),
                },
            )),
        };
        let accepted_frames = translate_private_response(
            accepted,
            &state,
            &keyring,
            &bindings,
            keyring.active_key_id(),
            attempt_id,
        )
        .await
        .unwrap();
        let Message::Text(accepted_text) = &accepted_frames[0].message else {
            panic!("acceptance was not text")
        };
        let accepted = golem_common::model::invocation_session_public::decode_server_text(
            accepted_text.as_bytes(),
        )
        .unwrap();
        let PublicServerMessage::InvocationAccepted { session_token, .. } = accepted else {
            panic!("acceptance translated to the wrong public message")
        };
        assert!(matches!(
            keyring
                .verify(
                    &session_token,
                    InvocationSessionTokenKind::Session,
                    &bindings.account,
                    &bindings.effective_principal,
                )
                .unwrap()
                .payload,
            InvocationSessionTokenPayload::Session(_)
        ));

        let mapping = private_mapping(8, StreamMappingRole::Output, fingerprint);
        let result = InvocationResponse {
            response: Some(invocation_response::Response::Result(
                InvocationSessionResult {
                    result: Some(invocation_session_result::Result::MethodResult(
                        stream_value(8),
                    )),
                    component_revision: Some(12),
                    agent_id: Some(proto_agent_id()),
                    idempotency_key: idempotency_key(),
                    new_stream_mappings: vec![mapping.clone()],
                    ..Default::default()
                },
            )),
        };
        let result_frames = translate_private_response(
            result,
            &state,
            &keyring,
            &bindings,
            keyring.active_key_id(),
            attempt_id,
        )
        .await
        .unwrap();
        let Message::Text(result_text) = &result_frames[0].message else {
            panic!("invocation result was not text")
        };
        let result = golem_common::model::invocation_session_public::decode_server_text(
            result_text.as_bytes(),
        )
        .unwrap();
        let PublicServerMessage::InvocationResult {
            mappings,
            result: PublicInvocationResult::Value { value },
            ..
        } = result
        else {
            panic!("stream result translated to the wrong public message")
        };
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].direction, PublicStreamDirection::Output);
        assert_eq!(mappings[0].channel, 1);
        assert_eq!(
            value["$stream"]["streamToken"],
            serde_json::Value::String(mappings[0].stream_token.clone())
        );
        assert!(matches!(
            keyring
                .verify(
                    &mappings[0].stream_token,
                    InvocationSessionTokenKind::Stream,
                    &bindings.account,
                    &bindings.effective_principal,
                )
                .unwrap()
                .payload,
            InvocationSessionTokenPayload::Stream(StreamTokenPayload {
                role: StreamTokenRole::Output,
                ..
            })
        ));

        let output = InvocationResponse {
            response: Some(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    transport_stream_id: 8,
                    producer_sequence: 0,
                    value: None,
                    durable_stream_id: Some(proto_uuid(108)),
                    durable_offset: durable_offset(1),
                    epoch: 1,
                    new_stream_mappings: Vec::new(),
                    packed_u8: vec![3, 5, 8],
                    logical_item_count: 3,
                },
            )),
        };
        let output_frames = translate_private_response(
            output,
            &state,
            &keyring,
            &bindings,
            keyring.active_key_id(),
            attempt_id,
        )
        .await
        .unwrap();
        assert_eq!(output_frames[0].stream_channel, Some(1));
        let Message::Binary(binary) = &output_frames[0].message else {
            panic!("packed output was not binary")
        };
        let decoded = decode_binary_message(binary).unwrap();
        assert_eq!(decoded.metadata.kind, BinaryMessageKind::OutputU8);
        assert_eq!(decoded.metadata.sequence, DecimalU64(0));
        assert_eq!(decoded.metadata.item_count, DecimalU64(3));
        assert_eq!(decoded.payload, vec![3, 5, 8]);
        let cursor = decoded.metadata.cursor_token.unwrap();
        assert!(matches!(
            keyring
                .verify(
                    &cursor,
                    InvocationSessionTokenKind::Cursor,
                    &bindings.account,
                    &bindings.effective_principal,
                )
                .unwrap()
                .payload,
            InvocationSessionTokenPayload::Cursor(CursorTokenPayload {
                output_durable_stream_id,
                ..
            }) if output_durable_stream_id == Uuid::from_u128(108)
        ));

        let replayed_frames = translate_result(
            &mut *state.lock().await,
            InvocationSessionResult {
                result: Some(invocation_session_result::Result::MethodResult(
                    stream_value(8),
                )),
                new_stream_mappings: vec![mapping.clone()],
                ..Default::default()
            },
            &keyring,
        )
        .unwrap();
        let Message::Text(replayed_text) = &replayed_frames[0].message else {
            panic!("replayed invocation result was not text")
        };
        let PublicServerMessage::InvocationResult {
            mappings: replayed_mappings,
            ..
        } = golem_common::model::invocation_session_public::decode_server_text(
            replayed_text.as_bytes(),
        )
        .unwrap()
        else {
            panic!("replayed invocation result translated to the wrong public message")
        };
        assert_eq!(replayed_mappings, mappings);

        let mut rebound = mapping;
        rebound.handle.as_mut().unwrap().stream_id = Some(proto_uuid(999));
        let Err(error) = translate_result(
            &mut *state.lock().await,
            InvocationSessionResult {
                result: Some(invocation_session_result::Result::MethodResult(
                    stream_value(8),
                )),
                new_stream_mappings: vec![rebound],
                ..Default::default()
            },
            &keyring,
        ) else {
            panic!("rebound durable output mapping was accepted")
        };
        assert_eq!(error.code, PublicErrorCode::StreamConflict);

        let output_error = InvocationResponse {
            response: Some(invocation_response::Response::OutputError(
                OutputStreamError {
                    transport_stream_id: 8,
                    producer_sequence: 3,
                    details: "upstream failed".to_string(),
                    durable_stream_id: Some(proto_uuid(108)),
                    durable_offset: durable_offset(2),
                    epoch: 1,
                },
            )),
        };
        let error_frames = translate_private_response(
            output_error,
            &state,
            &keyring,
            &bindings,
            keyring.active_key_id(),
            attempt_id,
        )
        .await
        .unwrap();
        assert_eq!(error_frames[0].stream_channel, Some(1));
        let Message::Text(error_text) = &error_frames[0].message else {
            panic!("output error was not text")
        };
        assert!(matches!(
            golem_common::model::invocation_session_public::decode_server_text(
                error_text.as_bytes()
            )
            .unwrap(),
            PublicServerMessage::OutputStreamEnd {
                channel: 1,
                sequence: DecimalU64(3),
                outcome: PublicOutputStreamOutcome::Error {
                    code: PublicErrorCode::ProducerError,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    async fn resume_message_verifies_session_and_cursor_tokens() {
        let keyring = keyring();
        let bindings = token_bindings(&AuthCtx::system());
        let logical_invocation_id = Uuid::from_u128(10);
        let identity = SessionAgentIdentity {
            component_id: Uuid::from_u128(20),
            component_revision: 12,
            agent_type: "test-agent".to_string(),
            agent_id: "agent".to_string(),
            method: "run".to_string(),
        };
        let session = SessionTokenPayload {
            application: "app".to_string(),
            environment: "env".to_string(),
            agent: encode_session_agent_identity(&identity).unwrap(),
            idempotency_key: "session-key".to_string(),
            logical_invocation_id,
            attachment_id: Uuid::from_u128(1),
            expected_attachment_generation: 1,
            callee_incarnation: Uuid::from_u128(4),
            stream_key_id: keyring.active_key_id().to_string(),
        };
        let session_token = keyring
            .sign(&bindings, &InvocationSessionTokenPayload::Session(session))
            .unwrap();
        let cursor_token = keyring
            .sign(
                &bindings,
                &InvocationSessionTokenPayload::Cursor(CursorTokenPayload {
                    parent_logical_invocation_id: logical_invocation_id,
                    output_durable_stream_id: Uuid::from_u128(108),
                    durable_offset: durable_offset(4),
                }),
            )
            .unwrap();
        let message = PublicClientMessage::ResumeAttach {
            attempt_id: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
            operation: PublicResumeOperation::Takeover,
            output_cursors: vec![cursor_token],
            session_token,
            version: 1,
        };
        let websocket =
            futures::stream::iter(vec![Ok(Message::text(encode_text(&message).unwrap()))]);
        futures::pin_mut!(websocket);
        let budgets = SessionBudgets::new();
        let (sender, _receiver) = mpsc::channel(1);
        let outbound = Outbound {
            sender,
            budgets: budgets.clone(),
        };
        let initial = receive_initial(&mut websocket, &outbound, &keyring, &bindings, &budgets)
            .await
            .unwrap()
            .unwrap();
        let InitialMessage::Resume { resume, .. } = initial else {
            panic!("resume message was decoded as a start")
        };
        assert_eq!(resume.identity, identity);
        assert_eq!(resume.operation, ResumeOperation::Takeover);
        assert_eq!(resume.cursors.len(), 1);
        assert_eq!(
            Uuid::from(resume.cursors[0].stream_id.unwrap()),
            Uuid::from_u128(108)
        );
        assert_eq!(
            resume.cursors[0].last_observed_offset,
            Some(durable_offset(4))
        );
    }
}
