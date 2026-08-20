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

use crate::durable_host::schema_value_stream::contains_stream;
use crate::durable_host::stream_bus::{
    AuxiliaryLiveStreamSubscriber, LiveStreamEventPayload, LiveStreamPublishError,
    LiveStreamPublisher, PrimaryLiveStreamSubscriber,
};
#[cfg(test)]
use crate::durable_host::stream_transport::LiveStreamTracker;
use crate::durable_host::stream_transport::{
    LiveStreamEndpoint, LiveStreamPeer, SourceLifecycle, input_stream_pair,
};
use golem_api_grpc::proto::golem::common::Empty;
use golem_api_grpc::proto::golem::schema::{
    FixedListValue, ListValue, MapEntry, MapValue, OptionValue, RecordValue, ResultValue,
    SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, TupleValue, UnionValue,
    VariantValue, result_value as proto_result_value, schema_value as proto_schema_value,
};
use golem_api_grpc::proto::golem::worker::{
    InputStreamAck, InputStreamEnd, InputStreamItem, InvocationRequest, InvocationResponse,
    OutputStreamEnd, OutputStreamError, OutputStreamItem, StreamCancel, StreamCancelReason,
    StreamCancelRole, input_stream_item, invocation_request, invocation_response,
};
use golem_schema::schema::SchemaValue;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[must_use = "imported stream registrations must be committed or rolled back"]
#[derive(Default)]
struct ImportedRegistrationBatch {
    stream_ids: Vec<u64>,
}

impl ImportedRegistrationBatch {
    fn is_empty(&self) -> bool {
        self.stream_ids.is_empty()
    }
}

#[derive(Clone)]
struct ImportedStreamRoute {
    lifecycle: Arc<SourceLifecycle>,
    publisher: LiveStreamPublisher<SchemaValue>,
    next_sequence: Arc<tokio::sync::Mutex<u64>>,
    acknowledgements: Arc<tokio::sync::Mutex<InputAcknowledgementState>>,
    completed: tokio_util::sync::CancellationToken,
}

#[derive(Default)]
struct InputAcknowledgementState {
    next_offset: u64,
    consumer_closed: bool,
}

enum ImportedStreamState {
    Registering(ImportedStreamRoute),
    Active(ImportedStreamRoute),
}

enum ImportedTerminal {
    End,
    Error(String),
    Cancel(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InputItemAdmission {
    Acknowledged(InputStreamAck),
    ConsumerClosed,
}

#[derive(Default)]
struct ImportedStreams {
    states: HashMap<u64, ImportedStreamState>,
    cancelled: HashMap<u64, ImportedStreamRoute>,
}

struct ExportedStreamRoute {
    lifecycle: Arc<SourceLifecycle>,
    publisher: LiveStreamPublisher<SchemaValue>,
    cancelled: tokio_util::sync::CancellationToken,
    activated: tokio_util::sync::CancellationToken,
    acknowledgements: Option<mpsc::Sender<InputStreamAck>>,
    announced: Arc<AtomicBool>,
    next_sent_offset: Arc<AtomicU64>,
    terminal_sent: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionSide {
    Client,
    Server,
}

#[derive(Clone)]
enum SessionFrames {
    Requests(mpsc::Sender<InvocationRequest>),
    Responses(mpsc::Sender<InvocationResponse>),
}

enum OutboundStreamMessage {
    Request(Box<invocation_request::Request>),
    Response(Box<invocation_response::Response>),
}

#[derive(Default)]
struct SessionActivity {
    active: AtomicUsize,
    changed: tokio::sync::Notify,
}

impl SessionActivity {
    fn start(self: &Arc<Self>) -> SessionActivityGuard {
        self.active.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_waiters();
        SessionActivityGuard(self.clone())
    }

    fn is_idle(&self) -> bool {
        self.active.load(Ordering::Acquire) == 0
    }

    async fn wait_until(&self, remaining: usize) {
        loop {
            let changed = self.changed.notified();
            if self.active.load(Ordering::Acquire) <= remaining {
                return;
            }
            changed.await;
        }
    }
}

struct SessionActivityGuard(Arc<SessionActivity>);

impl Drop for SessionActivityGuard {
    fn drop(&mut self) {
        let previous = self.0.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "live invocation activity count underflow");
        self.0.changed.notify_waiters();
    }
}

struct LiveValueSessionInner {
    next_stream_id: std::sync::atomic::AtomicU64,
    expected_remote_parity: u64,
    side: SessionSide,
    frames: SessionFrames,
    exported: Mutex<HashMap<u64, ExportedStreamRoute>>,
    imported: Mutex<ImportedStreams>,
    imported_changed: tokio::sync::Notify,
    seen_remote_stream_ids: Mutex<HashSet<u64>>,
    activity: Arc<SessionActivity>,
    cancelled: tokio_util::sync::CancellationToken,
    failure: Mutex<Option<String>>,
    stream_capacity: usize,
}

/// Converts recursive live values to and from the session protocol. The two
/// peers allocate disjoint odd and even stream IDs, so sibling streams cannot
/// alias even when nested streams are discovered in later items.
#[derive(Clone)]
pub(crate) struct LiveValueSession {
    inner: Arc<LiveValueSessionInner>,
}

impl LiveValueSession {
    #[cfg(test)]
    pub(crate) fn new_client(frames: mpsc::Sender<InvocationRequest>) -> Self {
        Self::new_client_with_capacity(frames, 32)
    }

    #[cfg(test)]
    pub(crate) fn new_server(frames: mpsc::Sender<InvocationResponse>) -> Self {
        Self::new_server_with_capacity(frames, 32)
    }

    pub(crate) fn new_client_with_capacity(
        frames: mpsc::Sender<InvocationRequest>,
        stream_capacity: usize,
    ) -> Self {
        Self::new_with_capacity(
            SessionSide::Client,
            SessionFrames::Requests(frames),
            stream_capacity,
        )
    }

    pub(crate) fn new_server_with_capacity(
        frames: mpsc::Sender<InvocationResponse>,
        stream_capacity: usize,
    ) -> Self {
        Self::new_with_capacity(
            SessionSide::Server,
            SessionFrames::Responses(frames),
            stream_capacity,
        )
    }

    fn new_with_capacity(side: SessionSide, frames: SessionFrames, stream_capacity: usize) -> Self {
        assert!(
            stream_capacity > 0,
            "live stream bus capacity must be non-zero"
        );
        let first_local_stream_id = match side {
            SessionSide::Client => 1,
            SessionSide::Server => 2,
        };
        Self {
            inner: Arc::new(LiveValueSessionInner {
                next_stream_id: std::sync::atomic::AtomicU64::new(first_local_stream_id),
                expected_remote_parity: 1 - (first_local_stream_id & 1),
                side,
                frames,
                exported: Mutex::new(HashMap::new()),
                imported: Mutex::new(ImportedStreams::default()),
                imported_changed: tokio::sync::Notify::new(),
                seen_remote_stream_ids: Mutex::new(HashSet::new()),
                activity: Arc::new(SessionActivity::default()),
                cancelled: tokio_util::sync::CancellationToken::new(),
                failure: Mutex::new(None),
                stream_capacity,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn encode(&self, value: &SchemaValue) -> Result<ProtoSchemaValue, String> {
        let (value, stream_ids) = self.encode_with_registered_streams(value)?;
        self.activate_exported_streams(&stream_ids);
        Ok(value)
    }

    pub(crate) fn encode_pending(
        &self,
        value: &SchemaValue,
    ) -> Result<(ProtoSchemaValue, Vec<u64>), String> {
        self.encode_with_registered_streams(value)
    }

    pub(crate) fn activate_exported_streams(&self, stream_ids: &[u64]) {
        let exported = self
            .inner
            .exported
            .lock()
            .expect("live stream map mutex poisoned");
        for stream_id in stream_ids {
            if let Some(route) = exported.get(stream_id) {
                route.announced.store(true, Ordering::Release);
                route.activated.cancel();
            }
        }
    }

    fn encode_with_registered_streams(
        &self,
        value: &SchemaValue,
    ) -> Result<(ProtoSchemaValue, Vec<u64>), String> {
        let mut registered_stream_ids = Vec::new();
        match self.encode_inner(value, &mut registered_stream_ids) {
            Ok(value) => Ok((value, registered_stream_ids)),
            Err(error) => {
                self.discard_exported_streams(&registered_stream_ids);
                Err(error)
            }
        }
    }

    fn discard_exported_streams(&self, stream_ids: &[u64]) {
        let mut exported = self
            .inner
            .exported
            .lock()
            .expect("live stream map mutex poisoned");
        for id in stream_ids {
            if let Some(route) = exported.remove(id) {
                route.cancelled.cancel();
            }
        }
    }

    fn encode_inner(
        &self,
        value: &SchemaValue,
        registered_stream_ids: &mut Vec<u64>,
    ) -> Result<ProtoSchemaValue, String> {
        if !contains_stream(value) {
            return value.clone().try_into();
        }

        let value = match value {
            SchemaValue::Stream(stream) => {
                let id = self.allocate_local_stream_id()?;
                let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>()?;
                let lifecycle = endpoint.lifecycle();
                let publisher = endpoint.publisher();
                self.spawn_exported(id, endpoint.activate(), lifecycle, publisher)?;
                registered_stream_ids.push(id);
                proto_schema_value::Value::StreamReference(SchemaValueStreamReference {
                    stream_id: id,
                })
            }
            SchemaValue::Record { fields } => proto_schema_value::Value::RecordValue(RecordValue {
                fields: fields
                    .iter()
                    .map(|field| self.encode_inner(field, registered_stream_ids))
                    .collect::<Result<_, _>>()?,
            }),
            SchemaValue::Variant(value) => {
                proto_schema_value::Value::VariantValue(Box::new(VariantValue {
                    case: value.case,
                    payload: value
                        .payload
                        .as_deref()
                        .map(|payload| self.encode_inner(payload, registered_stream_ids))
                        .transpose()?
                        .map(Box::new),
                }))
            }
            SchemaValue::Tuple { elements } => proto_schema_value::Value::TupleValue(TupleValue {
                elements: elements
                    .iter()
                    .map(|element| self.encode_inner(element, registered_stream_ids))
                    .collect::<Result<_, _>>()?,
            }),
            SchemaValue::List { elements } => proto_schema_value::Value::ListValue(ListValue {
                elements: elements
                    .iter()
                    .map(|element| self.encode_inner(element, registered_stream_ids))
                    .collect::<Result<_, _>>()?,
            }),
            SchemaValue::FixedList { elements } => {
                proto_schema_value::Value::FixedListValue(FixedListValue {
                    elements: elements
                        .iter()
                        .map(|element| self.encode_inner(element, registered_stream_ids))
                        .collect::<Result<_, _>>()?,
                })
            }
            SchemaValue::Map { entries } => proto_schema_value::Value::MapValue(MapValue {
                entries: entries
                    .iter()
                    .map(|(key, value)| {
                        Ok(MapEntry {
                            key: Some(self.encode_inner(key, registered_stream_ids)?),
                            value: Some(self.encode_inner(value, registered_stream_ids)?),
                        })
                    })
                    .collect::<Result<_, String>>()?,
            }),
            SchemaValue::Option { inner } => {
                proto_schema_value::Value::OptionValue(Box::new(OptionValue {
                    inner: inner
                        .as_deref()
                        .map(|inner| self.encode_inner(inner, registered_stream_ids))
                        .transpose()?
                        .map(Box::new),
                }))
            }
            SchemaValue::Result(result) => {
                let result = match result {
                    golem_schema::schema::schema_value::ResultValuePayload::Ok { value } => {
                        match value.as_deref() {
                            Some(value) => proto_result_value::Result::Ok(Box::new(
                                self.encode_inner(value, registered_stream_ids)?,
                            )),
                            None => proto_result_value::Result::OkUnit(Empty {}),
                        }
                    }
                    golem_schema::schema::schema_value::ResultValuePayload::Err { value } => {
                        match value.as_deref() {
                            Some(value) => proto_result_value::Result::Err(Box::new(
                                self.encode_inner(value, registered_stream_ids)?,
                            )),
                            None => proto_result_value::Result::ErrUnit(Empty {}),
                        }
                    }
                };
                proto_schema_value::Value::ResultValue(Box::new(ResultValue {
                    result: Some(result),
                }))
            }
            SchemaValue::Union(value) => {
                proto_schema_value::Value::UnionValue(Box::new(UnionValue {
                    tag: value.tag.clone(),
                    body: Some(Box::new(
                        self.encode_inner(&value.body, registered_stream_ids)?,
                    )),
                }))
            }
            _ => {
                return Err(
                    "a stream-bearing live value has an unsupported structural shape".to_string(),
                );
            }
        };
        Ok(ProtoSchemaValue { value: Some(value) })
    }

    pub(crate) async fn decode(&self, value: ProtoSchemaValue) -> Result<SchemaValue, String> {
        self.decode_with_rollback(value, true).await
    }

    pub(crate) async fn decode_start(
        &self,
        value: ProtoSchemaValue,
    ) -> Result<SchemaValue, String> {
        self.decode_with_rollback(value, false).await
    }

    async fn decode_with_rollback(
        &self,
        value: ProtoSchemaValue,
        notify_remote_on_rollback: bool,
    ) -> Result<SchemaValue, String> {
        let _activity = self.inner.activity.start();
        let (value, registrations) = self.decode_with_registered_streams(value);
        match value {
            Ok(value) => {
                self.commit_imported_streams(registrations);
                Ok(value)
            }
            Err(error) => {
                if notify_remote_on_rollback {
                    self.rollback_imported_streams(registrations).await;
                } else {
                    self.rollback_imported_streams_silently(registrations);
                }
                Err(error)
            }
        }
    }

    fn decode_with_registered_streams(
        &self,
        value: ProtoSchemaValue,
    ) -> (Result<SchemaValue, String>, ImportedRegistrationBatch) {
        let mut registrations = ImportedRegistrationBatch::default();
        let value = self.decode_inner(value, &mut registrations);
        (value, registrations)
    }

    fn decode_inner(
        &self,
        value: ProtoSchemaValue,
        registrations: &mut ImportedRegistrationBatch,
    ) -> Result<SchemaValue, String> {
        match value
            .value
            .ok_or_else(|| "schema value has no value".to_string())?
        {
            proto_schema_value::Value::StreamReference(reference) => {
                self.validate_remote_id(reference.stream_id)?;
                let (peer, stream) =
                    input_stream_pair(self.inner.stream_capacity, &self.inner.cancelled)?;
                self.spawn_imported(reference.stream_id, peer)?;
                registrations.stream_ids.push(reference.stream_id);
                Ok(SchemaValue::Stream(stream))
            }
            proto_schema_value::Value::RecordValue(value) => Ok(SchemaValue::Record {
                fields: value
                    .fields
                    .into_iter()
                    .map(|field| self.decode_inner(field, registrations))
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::VariantValue(value) => Ok(SchemaValue::Variant(
                golem_schema::schema::schema_value::VariantValuePayload {
                    case: value.case,
                    payload: value
                        .payload
                        .map(|payload| self.decode_inner(*payload, registrations).map(Box::new))
                        .transpose()?,
                },
            )),
            proto_schema_value::Value::TupleValue(value) => Ok(SchemaValue::Tuple {
                elements: value
                    .elements
                    .into_iter()
                    .map(|element| self.decode_inner(element, registrations))
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::ListValue(value) => Ok(SchemaValue::List {
                elements: value
                    .elements
                    .into_iter()
                    .map(|element| self.decode_inner(element, registrations))
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::FixedListValue(value) => Ok(SchemaValue::FixedList {
                elements: value
                    .elements
                    .into_iter()
                    .map(|element| self.decode_inner(element, registrations))
                    .collect::<Result<_, _>>()?,
            }),
            proto_schema_value::Value::MapValue(value) => Ok(SchemaValue::Map {
                entries: value
                    .entries
                    .into_iter()
                    .map(|entry| {
                        Ok((
                            self.decode_inner(
                                entry
                                    .key
                                    .ok_or_else(|| "live map entry has no key".to_string())?,
                                registrations,
                            )?,
                            self.decode_inner(
                                entry
                                    .value
                                    .ok_or_else(|| "live map entry has no value".to_string())?,
                                registrations,
                            )?,
                        ))
                    })
                    .collect::<Result<_, String>>()?,
            }),
            proto_schema_value::Value::OptionValue(value) => Ok(SchemaValue::Option {
                inner: value
                    .inner
                    .map(|inner| self.decode_inner(*inner, registrations).map(Box::new))
                    .transpose()?,
            }),
            proto_schema_value::Value::ResultValue(value) => {
                let result = match value
                    .result
                    .ok_or_else(|| "result value has no result arm".to_string())?
                {
                    proto_result_value::Result::Ok(value) => {
                        golem_schema::schema::schema_value::ResultValuePayload::Ok {
                            value: Some(Box::new(self.decode_inner(*value, registrations)?)),
                        }
                    }
                    proto_result_value::Result::Err(value) => {
                        golem_schema::schema::schema_value::ResultValuePayload::Err {
                            value: Some(Box::new(self.decode_inner(*value, registrations)?)),
                        }
                    }
                    proto_result_value::Result::OkUnit(_) => {
                        golem_schema::schema::schema_value::ResultValuePayload::Ok { value: None }
                    }
                    proto_result_value::Result::ErrUnit(_) => {
                        golem_schema::schema::schema_value::ResultValuePayload::Err { value: None }
                    }
                };
                Ok(SchemaValue::Result(result))
            }
            proto_schema_value::Value::UnionValue(value) => Ok(SchemaValue::Union(
                golem_schema::schema::schema_value::UnionValuePayload {
                    tag: value.tag,
                    body: Box::new(
                        self.decode_inner(
                            *value
                                .body
                                .ok_or_else(|| "live union value has no body".to_string())?,
                            registrations,
                        )?,
                    ),
                },
            )),
            value => ProtoSchemaValue { value: Some(value) }.try_into(),
        }
    }

    pub(crate) async fn route_request(
        &self,
        request: invocation_request::Request,
    ) -> Result<bool, String> {
        let _activity = self.inner.activity.start();
        match request {
            invocation_request::Request::InputItem(item) => {
                if let InputItemAdmission::Acknowledged(ack) = self.admit_input_item(item).await? {
                    let route = self.imported_route(ack.stream_id)?;
                    let mut acknowledgements = route.acknowledgements.lock().await;
                    if acknowledgements.consumer_closed {
                        return Ok(true);
                    }
                    let next_offset = ack
                        .sequence
                        .checked_add(ack.logical_item_count)
                        .ok_or_else(|| {
                            format!("input stream {} ACK offset overflow", ack.stream_id)
                        })?;
                    if !self
                        .send_outbound(OutboundStreamMessage::Response(Box::new(
                            invocation_response::Response::InputAck(ack),
                        )))
                        .await
                    {
                        return Err(
                            "invocation response stream closed before input ACK".to_string()
                        );
                    }
                    acknowledgements.next_offset = next_offset;
                }
                Ok(true)
            }
            invocation_request::Request::InputEnd(end) => {
                self.terminate_imported(end.stream_id, end.offset, ImportedTerminal::End)
                    .await?;
                Ok(true)
            }
            invocation_request::Request::StreamCancel(cancel) => {
                self.route_stream_cancel(cancel).await?;
                Ok(true)
            }
            invocation_request::Request::Start(_)
            | invocation_request::Request::ResumeAttach(_) => Ok(false),
        }
    }

    pub(crate) async fn route_response(
        &self,
        response: invocation_response::Response,
    ) -> Result<bool, String> {
        let _activity = self.inner.activity.start();
        match response {
            invocation_response::Response::OutputItem(item) => {
                let value = item
                    .value
                    .ok_or_else(|| format!("output stream {} item has no value", item.stream_id))?;
                self.admit_imported_value(item.stream_id, item.offset, value)
                    .await?;
                Ok(true)
            }
            invocation_response::Response::OutputEnd(end) => {
                self.terminate_imported(end.stream_id, end.offset, ImportedTerminal::End)
                    .await?;
                Ok(true)
            }
            invocation_response::Response::OutputError(error) => {
                self.terminate_imported(
                    error.stream_id,
                    error.offset,
                    ImportedTerminal::Error(error.details),
                )
                .await?;
                Ok(true)
            }
            invocation_response::Response::InputAck(ack) => {
                let sender = self
                    .inner
                    .exported
                    .lock()
                    .expect("live stream map mutex poisoned")
                    .get(&ack.stream_id)
                    .and_then(|route| route.acknowledgements.clone())
                    .ok_or_else(|| {
                        format!("acknowledgement for unknown input stream {}", ack.stream_id)
                    })?;
                sender
                    .send(ack)
                    .await
                    .map_err(|_| "input stream is no longer awaiting an ACK".to_string())?;
                Ok(true)
            }
            invocation_response::Response::StreamCancel(cancel) => {
                self.route_stream_cancel(cancel).await?;
                Ok(true)
            }
            invocation_response::Response::Accepted(_)
            | invocation_response::Response::Rejected(_)
            | invocation_response::Response::Result(_)
            | invocation_response::Response::AttachmentRevoked(_)
            | invocation_response::Response::Finished(_) => Ok(false),
        }
    }

    async fn admit_imported_value(
        &self,
        stream_id: u64,
        offset: u64,
        value: ProtoSchemaValue,
    ) -> Result<(), String> {
        let route = self.imported_route(stream_id)?;
        let mut next_sequence = route.next_sequence.lock().await;
        if offset != *next_sequence {
            return Err(format!(
                "output stream {stream_id} expected offset {}, got {offset}",
                *next_sequence
            ));
        }
        let following_offset = offset
            .checked_add(1)
            .ok_or_else(|| format!("output stream {stream_id} offset overflow"))?;
        let (value, registrations) = self.decode_with_registered_streams(value);
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                self.rollback_imported_streams(registrations).await;
                return Err(error);
            }
        };
        match route.publisher.publish_item(value).await {
            Ok(published_offset) if published_offset == offset => {
                self.commit_imported_streams(registrations);
                *next_sequence = following_offset;
                Ok(())
            }
            Ok(published_offset) => {
                self.rollback_imported_streams(registrations).await;
                Err(format!(
                    "output stream {stream_id} published offset {published_offset}, expected {offset}"
                ))
            }
            Err(error) => {
                self.rollback_imported_streams(registrations).await;
                Err(format!(
                    "failed to admit output stream {stream_id} item: {error:?}"
                ))
            }
        }
    }

    async fn terminate_imported(
        &self,
        stream_id: u64,
        offset: u64,
        terminal: ImportedTerminal,
    ) -> Result<(), String> {
        let route = self.imported_route(stream_id)?;
        let next_sequence = *route.next_sequence.lock().await;
        if offset != next_sequence {
            return Err(format!(
                "stream {stream_id} expected terminal offset {next_sequence}, got {offset}"
            ));
        }
        if self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned")
            .cancelled
            .contains_key(&stream_id)
        {
            self.remove_cancelled_imported(stream_id);
            return Ok(());
        }
        let result = match terminal {
            ImportedTerminal::End => route.publisher.publish_end().await,
            ImportedTerminal::Error(error) => route.publisher.publish_error(error).await,
            ImportedTerminal::Cancel(details) => route.publisher.publish_cancel(details).await,
        };
        if let Err(error) = result {
            if error == LiveStreamPublishError::Closed
                && self.remove_cancelled_imported(stream_id).is_some()
            {
                return Ok(());
            }
            return Err(format!(
                "failed to publish terminal for stream {stream_id}: {error:?}"
            ));
        }
        route.lifecycle.finish();
        if self.remove_imported(stream_id).is_none() {
            self.remove_cancelled_imported(stream_id);
        }
        Ok(())
    }

    async fn route_stream_cancel(&self, cancel: StreamCancel) -> Result<(), String> {
        let role = StreamCancelRole::try_from(cancel.role)
            .map_err(|_| format!("invalid stream cancellation role {}", cancel.role))?;
        match (self.inner.side, role) {
            (SessionSide::Server, StreamCancelRole::InputProducer)
            | (SessionSide::Client, StreamCancelRole::OutputProducer) => {
                self.terminate_imported(
                    cancel.stream_id,
                    cancel.offset,
                    ImportedTerminal::Cancel(
                        cancel
                            .details
                            .unwrap_or_else(|| "stream producer cancelled".to_string()),
                    ),
                )
                .await
            }
            (SessionSide::Server, StreamCancelRole::OutputConsumer) => {
                self.cancel_for_output_consumer(cancel).await
            }
            (SessionSide::Client, StreamCancelRole::InputConsumer) => {
                self.cancel_exported(cancel.stream_id)
            }
            _ => Err(format!(
                "unexpected {:?} stream cancellation for {:?} session",
                role, self.inner.side
            )),
        }
    }

    pub(crate) async fn admit_input_item(
        &self,
        item: InputStreamItem,
    ) -> Result<InputItemAdmission, String> {
        let stream_id = item.stream_id;
        let route = self.imported_route(stream_id)?;
        let mut next_sequence = route.next_sequence.lock().await;
        if item.sequence != *next_sequence {
            return Err(format!(
                "input stream {stream_id} expected sequence {}, got {}",
                *next_sequence, item.sequence
            ));
        }

        let logical_item_count = match item.payload.as_ref() {
            Some(input_stream_item::Payload::Value(_)) => 1,
            Some(input_stream_item::Payload::PackedU8(bytes)) if !bytes.is_empty() => {
                u64::try_from(bytes.len())
                    .map_err(|_| format!("input stream {stream_id} logical item count overflow"))?
            }
            Some(input_stream_item::Payload::PackedU8(_)) => {
                return Err("packed-u8 input item must not be empty".to_string());
            }
            None => return Err("input stream item has no payload".to_string()),
        };
        let following_sequence = item
            .sequence
            .checked_add(logical_item_count)
            .ok_or_else(|| format!("input stream {stream_id} sequence overflow"))?;

        let mut values = Vec::new();
        match item.payload {
            Some(input_stream_item::Payload::Value(value)) => {
                let (value, registrations) = self.decode_with_registered_streams(value);
                match value {
                    Ok(value) => values.push((value, registrations)),
                    Err(error) => {
                        self.rollback_imported_streams(registrations).await;
                        return Err(error);
                    }
                }
            }
            Some(input_stream_item::Payload::PackedU8(bytes)) if !bytes.is_empty() => {
                values.extend(
                    bytes.into_iter().map(|value| {
                        (SchemaValue::U8(value), ImportedRegistrationBatch::default())
                    }),
                );
            }
            Some(input_stream_item::Payload::PackedU8(_)) => {
                return Err("packed-u8 input item must not be empty".to_string());
            }
            None => return Err("input stream item has no payload".to_string()),
        }

        let mut values = values.into_iter().enumerate();
        while let Some((index, (value, registrations))) = values.next() {
            let expected_offset = item.sequence + index as u64;
            match route.publisher.publish_item(value).await {
                Ok(offset) if offset == expected_offset => {
                    self.commit_imported_streams(registrations);
                }
                Ok(offset) => {
                    self.rollback_imported_streams(registrations).await;
                    self.fail(format!(
                        "input stream {stream_id} published offset {offset}, expected {expected_offset}"
                    ));
                    return Err(format!(
                        "input stream {stream_id} published offset {offset}, expected {expected_offset}"
                    ));
                }
                Err(LiveStreamPublishError::Closed) => {
                    self.rollback_imported_streams(registrations).await;
                    for (_, (_, registrations)) in values {
                        self.rollback_imported_streams(registrations).await;
                    }
                    *next_sequence = following_sequence;
                    return Ok(InputItemAdmission::ConsumerClosed);
                }
                Err(error) => {
                    self.rollback_imported_streams(registrations).await;
                    return Err(format!(
                        "failed to admit input stream {stream_id} item: {error:?}"
                    ));
                }
            }
        }
        *next_sequence = following_sequence;
        Ok(InputItemAdmission::Acknowledged(InputStreamAck {
            stream_id,
            sequence: item.sequence,
            logical_item_count,
        }))
    }

    pub(crate) async fn wait_idle(&self) {
        loop {
            let activity_changed = self.inner.activity.changed.notified();
            let imported_changed = self.inner.imported_changed.notified();
            let imported_settled = self
                .inner
                .imported
                .lock()
                .expect("live stream map mutex poisoned")
                .states
                .values()
                .all(|state| matches!(state, ImportedStreamState::Active(_)));
            if self.inner.activity.is_idle() && imported_settled {
                return;
            }
            tokio::select! {
                _ = activity_changed => {}
                _ = imported_changed => {}
            }
        }
    }

    pub(crate) fn cancel(&self) {
        self.inner.cancelled.cancel();
        let (imported, cancelled_imported) = {
            let mut imported = self
                .inner
                .imported
                .lock()
                .expect("live stream map mutex poisoned");
            (
                std::mem::take(&mut imported.states),
                std::mem::take(&mut imported.cancelled),
            )
        };
        for state in imported.into_values() {
            let route = match state {
                ImportedStreamState::Registering(route) | ImportedStreamState::Active(route) => {
                    route
                }
            };
            route.publisher.close();
            route.lifecycle.finish();
            route.completed.cancel();
        }
        for route in cancelled_imported.into_values() {
            route.completed.cancel();
        }
        let exported = std::mem::take(
            &mut *self
                .inner
                .exported
                .lock()
                .expect("live stream map mutex poisoned"),
        );
        for route in exported.into_values() {
            route.cancelled.cancel();
        }
        self.inner.imported_changed.notify_waiters();
    }

    async fn cancel_for_output_consumer(&self, cancel: StreamCancel) -> Result<(), String> {
        let imported = std::mem::take(
            &mut self
                .inner
                .imported
                .lock()
                .expect("live stream map mutex poisoned")
                .states,
        );
        let mut imported_terminals = Vec::with_capacity(imported.len());
        for (stream_id, state) in imported {
            let route = match state {
                ImportedStreamState::Registering(route) | ImportedStreamState::Active(route) => {
                    route
                }
            };
            imported_terminals.push((stream_id, route.next_sequence.clone()));
            route.publisher.close();
            route.lifecycle.finish();
            route.completed.cancel();
        }

        let exported = std::mem::take(
            &mut *self
                .inner
                .exported
                .lock()
                .expect("live stream map mutex poisoned"),
        );
        self.inner.cancelled.cancel();
        let mut exported_terminals = Vec::with_capacity(exported.len());
        for (stream_id, route) in exported {
            exported_terminals.push((
                stream_id,
                route.announced,
                route.next_sent_offset,
                route.terminal_sent,
            ));
            route.cancelled.cancel();
        }
        self.inner.imported_changed.notify_waiters();

        let frames = match &self.inner.frames {
            SessionFrames::Responses(frames) => frames.clone(),
            SessionFrames::Requests(_) => {
                return Err(
                    "output-consumer cancellation is only valid for server sessions".into(),
                );
            }
        };
        self.inner.activity.wait_until(1).await;
        imported_terminals.sort_unstable_by_key(|(stream_id, _)| *stream_id);
        for (stream_id, next_sequence) in imported_terminals {
            let offset = *next_sequence.lock().await;
            if frames
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::StreamCancel(StreamCancel {
                        stream_id,
                        offset,
                        role: StreamCancelRole::InputConsumer as i32,
                        reason: cancel.reason,
                        details: cancel.details.clone(),
                    })),
                })
                .await
                .is_err()
            {
                return Ok(());
            }
        }

        exported_terminals.sort_unstable_by_key(|(stream_id, _, _, _)| *stream_id);
        for (stream_id, announced, next_sent_offset, terminal_sent) in exported_terminals {
            if !announced.load(Ordering::Acquire) || terminal_sent.load(Ordering::Acquire) {
                continue;
            }
            let offset = if stream_id == cancel.stream_id {
                cancel.offset
            } else {
                next_sent_offset.load(Ordering::Acquire)
            };
            if frames
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::StreamCancel(StreamCancel {
                        stream_id,
                        offset,
                        role: StreamCancelRole::OutputProducer as i32,
                        reason: cancel.reason,
                        details: cancel.details.clone(),
                    })),
                })
                .await
                .is_err()
            {
                return Ok(());
            }
        }
        Ok(())
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.cancelled.is_cancelled()
    }

    #[allow(dead_code)]
    pub(crate) fn subscribe_output_tail(
        &self,
        stream_id: u64,
    ) -> Result<AuxiliaryLiveStreamSubscriber<SchemaValue>, String> {
        self.inner
            .exported
            .lock()
            .expect("live stream map mutex poisoned")
            .get(&stream_id)
            .map(|route| route.publisher.subscribe_tail())
            .ok_or_else(|| format!("subscription for unknown output stream {stream_id}"))
    }

    pub(crate) async fn finish_invocation(&self) -> Result<(), String> {
        loop {
            let activity_changed = self.inner.activity.changed.notified();
            let imported_changed = self.inner.imported_changed.notified();
            let (imported, imported_settling) = {
                let imported = self
                    .inner
                    .imported
                    .lock()
                    .expect("live stream map mutex poisoned");
                let mut active = Vec::new();
                let mut settling = false;
                for (id, state) in &imported.states {
                    match state {
                        ImportedStreamState::Active(route)
                            if route.lifecycle.finished.load(Ordering::Acquire) =>
                        {
                            settling = true;
                        }
                        ImportedStreamState::Registering(_) => settling = true,
                        ImportedStreamState::Active(_) => active.push(*id),
                    }
                }
                (active, settling)
            };
            let (exported, exported_settling) = {
                let exported = self
                    .inner
                    .exported
                    .lock()
                    .expect("live stream map mutex poisoned");
                let mut active = Vec::new();
                let mut settling = false;
                for (id, route) in exported.iter() {
                    if route.lifecycle.finished.load(Ordering::Acquire) {
                        settling = true;
                    } else {
                        active.push(*id);
                    }
                }
                (active, settling)
            };
            if imported.is_empty() && exported.is_empty() {
                if !self.inner.activity.is_idle() || imported_settling || exported_settling {
                    tokio::select! {
                        _ = activity_changed => continue,
                        _ = imported_changed => continue,
                    }
                }
                return Ok(());
            }

            let details = format!(
                "live invocation terminated with open imported streams {imported:?} and open exported streams {exported:?}"
            );
            self.terminate_for_failure(&details).await;
            return Err(details);
        }
    }

    pub(crate) async fn terminate_for_failure(&self, details: &str) {
        let exported = self
            .inner
            .exported
            .lock()
            .expect("live stream map mutex poisoned")
            .values()
            .map(|route| route.publisher.clone())
            .collect::<Vec<_>>();
        for publisher in exported {
            let _ = publisher.publish_error(details.to_string()).await;
        }

        let imported = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned")
            .states
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in imported {
            self.cancel_imported(id, details).await;
        }

        self.wait_idle().await;
        *self
            .inner
            .failure
            .lock()
            .expect("live invocation failure mutex poisoned") = Some(details.to_string());
        self.cancel();
    }

    pub(crate) fn fail(&self, error: String) {
        *self
            .inner
            .failure
            .lock()
            .expect("live invocation failure mutex poisoned") = Some(error);
        self.cancel();
    }

    async fn send_outbound(&self, message: OutboundStreamMessage) -> bool {
        match (&self.inner.frames, message) {
            (SessionFrames::Requests(frames), OutboundStreamMessage::Request(request)) => {
                tokio::select! {
                    result = frames.send(InvocationRequest { request: Some(*request) }) => result.is_ok(),
                    _ = self.inner.cancelled.cancelled() => false,
                }
            }
            (SessionFrames::Responses(frames), OutboundStreamMessage::Response(response)) => {
                tokio::select! {
                    result = frames.send(InvocationResponse { response: Some(*response) }) => result.is_ok(),
                    _ = self.inner.cancelled.cancelled() => false,
                }
            }
            (SessionFrames::Requests(_), OutboundStreamMessage::Response(_))
            | (SessionFrames::Responses(_), OutboundStreamMessage::Request(_)) => {
                self.fail(
                    "live session attempted to send a message in the wrong direction".to_string(),
                );
                false
            }
        }
    }

    async fn rollback_imported_streams(&self, registrations: ImportedRegistrationBatch) {
        for id in registrations.stream_ids {
            self.cancel_imported(id, "recursive stream registration was rolled back")
                .await;
        }
    }

    fn rollback_imported_streams_silently(&self, registrations: ImportedRegistrationBatch) {
        for id in registrations.stream_ids {
            if let Some(route) = self.remove_imported(id) {
                route.publisher.close();
                route.lifecycle.finish();
            }
        }
    }

    async fn cancel_imported(&self, id: u64, details: &str) {
        let Some(route) = self.retain_cancelled_imported(id) else {
            return;
        };
        let mut acknowledgements = route.acknowledgements.lock().await;
        acknowledgements.consumer_closed = true;
        let offset = acknowledgements.next_offset;
        let role = match self.inner.side {
            SessionSide::Client => StreamCancelRole::OutputConsumer,
            SessionSide::Server => StreamCancelRole::InputConsumer,
        };
        let cancel = StreamCancel {
            stream_id: id,
            offset,
            role: role as i32,
            reason: StreamCancelReason::Cancelled as i32,
            details: Some(details.to_string()),
        };
        let message = match self.inner.side {
            SessionSide::Client => OutboundStreamMessage::Request(Box::new(
                invocation_request::Request::StreamCancel(cancel),
            )),
            SessionSide::Server => OutboundStreamMessage::Response(Box::new(
                invocation_response::Response::StreamCancel(cancel),
            )),
        };
        let _ = self.send_outbound(message).await;
        if self.inner.side == SessionSide::Client {
            self.cancel();
        }
    }

    fn remove_imported(&self, id: u64) -> Option<ImportedStreamRoute> {
        let state = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned")
            .states
            .remove(&id);
        self.inner.imported_changed.notify_waiters();
        state.map(|state| match state {
            ImportedStreamState::Registering(route) | ImportedStreamState::Active(route) => {
                route.completed.cancel();
                route
            }
        })
    }

    fn retain_cancelled_imported(&self, id: u64) -> Option<ImportedStreamRoute> {
        let mut imported = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned");
        let state = imported.states.remove(&id)?;
        let route = match state {
            ImportedStreamState::Registering(route) | ImportedStreamState::Active(route) => route,
        };
        route.completed.cancel();
        route.publisher.close();
        route.lifecycle.finish();
        imported.cancelled.insert(id, route.clone());
        drop(imported);
        self.inner.imported_changed.notify_waiters();
        Some(route)
    }

    fn remove_cancelled_imported(&self, id: u64) -> Option<ImportedStreamRoute> {
        let route = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned")
            .cancelled
            .remove(&id);
        self.inner.imported_changed.notify_waiters();
        route
    }

    fn cancel_exported(&self, id: u64) -> Result<(), String> {
        let route = self
            .inner
            .exported
            .lock()
            .expect("live stream map mutex poisoned")
            .remove(&id)
            .ok_or_else(|| format!("cancellation for unknown local stream {id}"))?;
        route.cancelled.cancel();
        Ok(())
    }

    fn commit_imported_streams(&self, registrations: ImportedRegistrationBatch) {
        if registrations.is_empty() {
            return;
        }
        let mut imported = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned");
        for id in registrations.stream_ids {
            let route = match imported.states.get(&id) {
                Some(ImportedStreamState::Registering(route)) => Some(route.clone()),
                _ => None,
            };
            if let Some(route) = route {
                imported
                    .states
                    .insert(id, ImportedStreamState::Active(route));
            }
        }
        drop(imported);
        self.inner.imported_changed.notify_waiters();
    }

    fn validate_remote_id(&self, id: u64) -> Result<(), String> {
        if id == 0 || id & 1 != self.inner.expected_remote_parity {
            Err(format!("invalid remote stream id {id}"))
        } else {
            Ok(())
        }
    }

    fn imported_route(&self, id: u64) -> Result<ImportedStreamRoute, String> {
        self.validate_remote_id(id)?;
        let imported = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned");
        imported
            .states
            .get(&id)
            .map(|state| match state {
                ImportedStreamState::Registering(route) | ImportedStreamState::Active(route) => {
                    route.clone()
                }
            })
            .or_else(|| imported.cancelled.get(&id).cloned())
            .ok_or_else(|| format!("item for unknown remote stream {id}"))
    }

    fn allocate_local_stream_id(&self) -> Result<u64, String> {
        loop {
            let current = self.inner.next_stream_id.load(Ordering::Acquire);
            if current == 0 {
                return Err("live stream ID space is exhausted".to_string());
            }
            let next = current.checked_add(2).unwrap_or(0);
            if self
                .inner
                .next_stream_id
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(current);
            }
        }
    }

    fn exported_end(&self, stream_id: u64, offset: u64) -> OutboundStreamMessage {
        match self.inner.side {
            SessionSide::Client => OutboundStreamMessage::Request(Box::new(
                invocation_request::Request::InputEnd(InputStreamEnd { stream_id, offset }),
            )),
            SessionSide::Server => OutboundStreamMessage::Response(Box::new(
                invocation_response::Response::OutputEnd(OutputStreamEnd { stream_id, offset }),
            )),
        }
    }

    fn exported_error(
        &self,
        stream_id: u64,
        offset: u64,
        details: String,
    ) -> OutboundStreamMessage {
        match self.inner.side {
            SessionSide::Client => OutboundStreamMessage::Request(Box::new(
                invocation_request::Request::StreamCancel(StreamCancel {
                    stream_id,
                    offset,
                    role: StreamCancelRole::InputProducer as i32,
                    reason: StreamCancelReason::Cancelled as i32,
                    details: Some(details),
                }),
            )),
            SessionSide::Server => OutboundStreamMessage::Response(Box::new(
                invocation_response::Response::OutputError(OutputStreamError {
                    stream_id,
                    offset,
                    details,
                }),
            )),
        }
    }

    fn spawn_exported(
        &self,
        id: u64,
        mut receiver: PrimaryLiveStreamSubscriber<SchemaValue>,
        lifecycle: Arc<SourceLifecycle>,
        publisher: LiveStreamPublisher<SchemaValue>,
    ) -> Result<(), String> {
        let cancelled = self.inner.cancelled.child_token();
        let activated = tokio_util::sync::CancellationToken::new();
        let announced = Arc::new(AtomicBool::new(false));
        let next_sent_offset = Arc::new(AtomicU64::new(0));
        let terminal_sent = Arc::new(AtomicBool::new(false));
        let (acknowledgements, mut acknowledgement_rx) = if self.inner.side == SessionSide::Client {
            let (sender, receiver) = mpsc::channel(1);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let mut exported = self
            .inner
            .exported
            .lock()
            .expect("live stream map mutex poisoned");
        match exported.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(ExportedStreamRoute {
                    lifecycle,
                    publisher,
                    cancelled: cancelled.clone(),
                    activated: activated.clone(),
                    acknowledgements,
                    announced: announced.clone(),
                    next_sent_offset: next_sent_offset.clone(),
                    terminal_sent: terminal_sent.clone(),
                });
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(format!("duplicate local stream id {id}"));
            }
        }
        drop(exported);
        let session = self.clone();
        let activity = self.inner.activity.start();
        tokio::spawn(async move {
            let _activity = activity;
            tokio::select! {
                _ = activated.cancelled() => {}
                _ = cancelled.cancelled() => return,
                _ = session.inner.cancelled.cancelled() => return,
            }
            loop {
                let event = tokio::select! {
                    event = receiver.recv() => event,
                    _ = cancelled.cancelled() => break,
                    _ = session.inner.cancelled.cancelled() => break,
                };
                let (message, registered_stream_ids, terminal, expected_ack, sent_offset) =
                    match event {
                        Ok(event) => {
                            match event.payload {
                                LiveStreamEventPayload::Item(value) => {
                                    match session.encode_with_registered_streams(&value) {
                                        Ok((value, registered_stream_ids)) => {
                                            let message = match session.inner.side {
                                        SessionSide::Client => OutboundStreamMessage::Request(Box::new(
                                            invocation_request::Request::InputItem(
                                                InputStreamItem {
                                                    stream_id: id,
                                                    sequence: event.offset,
                                                    payload: Some(
                                                        input_stream_item::Payload::Value(value),
                                                    ),
                                                },
                                            ),
                                        )),
                                        SessionSide::Server => OutboundStreamMessage::Response(Box::new(
                                            invocation_response::Response::OutputItem(
                                                OutputStreamItem {
                                                    stream_id: id,
                                                    offset: event.offset,
                                                    value: Some(value),
                                                },
                                            ),
                                        )),
                                    };
                                            (
                                                message,
                                                registered_stream_ids,
                                                false,
                                                (session.inner.side == SessionSide::Client)
                                                    .then_some((event.offset, 1)),
                                                Some(event.offset + 1),
                                            )
                                        }
                                        Err(error) => (
                                            session.exported_error(id, event.offset, error),
                                            Vec::new(),
                                            true,
                                            None,
                                            None,
                                        ),
                                    }
                                }
                                LiveStreamEventPayload::End => (
                                    session.exported_end(id, event.offset),
                                    Vec::new(),
                                    true,
                                    None,
                                    None,
                                ),
                                LiveStreamEventPayload::Error(error)
                                | LiveStreamEventPayload::Cancel(error) => (
                                    session.exported_error(id, event.offset, error),
                                    Vec::new(),
                                    true,
                                    None,
                                    None,
                                ),
                            }
                        }
                        Err(error) => (
                            session.exported_error(
                                id,
                                0,
                                format!("failed to receive live stream event: {error:?}"),
                            ),
                            Vec::new(),
                            true,
                            None,
                            None,
                        ),
                    };
                if !session.send_outbound(message).await {
                    session.discard_exported_streams(&registered_stream_ids);
                    break;
                }
                if let Some(sent_offset) = sent_offset {
                    next_sent_offset.store(sent_offset, Ordering::Release);
                }
                if terminal {
                    terminal_sent.store(true, Ordering::Release);
                }
                session.activate_exported_streams(&registered_stream_ids);
                if let Some((sequence, logical_item_count)) = expected_ack {
                    let ack = tokio::select! {
                        ack = acknowledgement_rx
                            .as_mut()
                            .expect("client stream has no acknowledgement receiver")
                            .recv() => ack,
                        _ = cancelled.cancelled() => None,
                        _ = session.inner.cancelled.cancelled() => None,
                    };
                    let Some(ack) = ack else {
                        session.discard_exported_streams(&registered_stream_ids);
                        break;
                    };
                    if ack.stream_id != id
                        || ack.sequence != sequence
                        || ack.logical_item_count != logical_item_count
                    {
                        session.discard_exported_streams(&registered_stream_ids);
                        session.fail(format!(
                            "input stream {id} received invalid acknowledgement ({}, {})",
                            ack.sequence, ack.logical_item_count
                        ));
                        break;
                    }
                }
                if terminal {
                    break;
                }
            }
            session
                .inner
                .exported
                .lock()
                .expect("live stream map mutex poisoned")
                .remove(&id);
        });
        Ok(())
    }

    fn spawn_imported(&self, id: u64, peer: LiveStreamPeer) -> Result<(), String> {
        if !self
            .inner
            .seen_remote_stream_ids
            .lock()
            .expect("live stream ID set mutex poisoned")
            .insert(id)
        {
            return Err(format!("duplicate remote stream id {id}"));
        }
        let completed = tokio_util::sync::CancellationToken::new();
        let previous = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned")
            .states
            .insert(
                id,
                ImportedStreamState::Registering(ImportedStreamRoute {
                    lifecycle: peer.lifecycle.clone(),
                    publisher: peer.publisher.clone(),
                    next_sequence: Arc::new(tokio::sync::Mutex::new(0)),
                    acknowledgements: Arc::new(tokio::sync::Mutex::new(
                        InputAcknowledgementState::default(),
                    )),
                    completed: completed.clone(),
                }),
            );
        debug_assert!(previous.is_none(), "new remote stream ID already imported");
        self.inner.imported_changed.notify_waiters();
        let session = self.clone();
        let activity = self.inner.activity.start();
        tokio::spawn(async move {
            let _activity = activity;
            tokio::select! {
                _ = completed.cancelled() => {}
                _ = session.inner.cancelled.cancelled() => {
                    peer.publisher.close();
                    peer.lifecycle.finish();
                }
                _ = peer.primary_dropped.notified() => {
                    session.cancel_imported(id, "stream consumer dropped its primary reader").await;
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod bus_tests {
    use super::*;
    use crate::durable_host::stream_transport::{
        LiveStreamEndpoint, LiveStreamPeer, input_stream_pair,
    };
    use golem_schema::schema::schema_value::{
        ResultValuePayload, UnionValuePayload, VariantValuePayload,
    };
    use test_r::test;
    use tokio_util::sync::CancellationToken;

    fn stream_id(value: &ProtoSchemaValue) -> u64 {
        match value.value.as_ref() {
            Some(proto_schema_value::Value::StreamReference(reference)) => reference.stream_id,
            other => panic!("expected stream id, got {other:?}"),
        }
    }

    fn stream_reference(id: u64) -> ProtoSchemaValue {
        ProtoSchemaValue {
            value: Some(proto_schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: id },
            )),
        }
    }

    fn stream_source(capacity: usize) -> (LiveStreamPeer, SchemaValue) {
        let cancellation = CancellationToken::new();
        let (peer, stream) = input_stream_pair(capacity, &cancellation).unwrap();
        (peer, SchemaValue::Stream(stream))
    }

    fn take_primary(
        value: SchemaValue,
    ) -> crate::durable_host::stream_bus::PrimaryLiveStreamSubscriber<SchemaValue> {
        let SchemaValue::Stream(stream) = value else {
            panic!("expected stream");
        };
        stream
            .take_host_endpoint::<LiveStreamEndpoint>()
            .unwrap()
            .activate()
    }

    fn count_streams(value: &SchemaValue) -> usize {
        match value {
            SchemaValue::Stream(_) => 1,
            SchemaValue::Record { fields } => fields.iter().map(count_streams).sum(),
            SchemaValue::Variant(value) => value
                .payload
                .as_deref()
                .map(count_streams)
                .unwrap_or_default(),
            SchemaValue::Tuple { elements }
            | SchemaValue::List { elements }
            | SchemaValue::FixedList { elements } => elements.iter().map(count_streams).sum(),
            SchemaValue::Map { entries } => entries
                .iter()
                .map(|(key, value)| count_streams(key) + count_streams(value))
                .sum(),
            SchemaValue::Option { inner } => {
                inner.as_deref().map(count_streams).unwrap_or_default()
            }
            SchemaValue::Result(value) => match value {
                ResultValuePayload::Ok { value } | ResultValuePayload::Err { value } => {
                    value.as_deref().map(count_streams).unwrap_or_default()
                }
            },
            SchemaValue::Union(value) => count_streams(&value.body),
            _ => 0,
        }
    }

    #[test]
    async fn recursive_composites_preserve_independent_streams() {
        let mut sources = Vec::new();
        let mut next_stream = || {
            let (source, stream) = stream_source(4);
            sources.push(source);
            stream
        };
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::Variant(VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(next_stream())),
                }),
                SchemaValue::Tuple {
                    elements: vec![next_stream()],
                },
                SchemaValue::List {
                    elements: vec![next_stream()],
                },
                SchemaValue::FixedList {
                    elements: vec![next_stream()],
                },
                SchemaValue::Map {
                    entries: vec![(next_stream(), next_stream())],
                },
                SchemaValue::Option {
                    inner: Some(Box::new(next_stream())),
                },
                SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(next_stream())),
                }),
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(next_stream())),
                }),
                SchemaValue::Union(UnionValuePayload {
                    tag: "stream".to_string(),
                    body: Box::new(next_stream()),
                }),
            ],
        };
        let (sender_frames, _sender_frame_rx) = mpsc::channel(32);
        let sender = LiveValueSession::new_client(sender_frames);
        let encoded = sender.encode(&value).unwrap();
        let (receiver_frames, _receiver_frame_rx) = mpsc::channel(32);
        let receiver = LiveValueSession::new_server(receiver_frames);

        let decoded = receiver.decode(encoded).await.unwrap();

        assert_eq!(count_streams(&decoded), 10);
        assert_eq!(sources.len(), 10);
        sender.cancel();
        receiver.cancel();
    }

    #[test]
    async fn stream_ids_are_affine_checked_and_session_local() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let receiver = LiveValueSession::new_client(frames);
        assert_eq!(
            receiver.decode(stream_reference(0)).await.unwrap_err(),
            "invalid remote stream id 0"
        );
        assert_eq!(
            receiver.decode(stream_reference(1)).await.unwrap_err(),
            "invalid remote stream id 1"
        );
        assert!(matches!(
            receiver.decode(stream_reference(2)).await.unwrap(),
            SchemaValue::Stream(_)
        ));
        assert_eq!(
            receiver.decode(stream_reference(2)).await.unwrap_err(),
            "duplicate remote stream id 2"
        );

        let (_source, stream) = stream_source(4);
        let SchemaValue::Stream(stream) = stream else {
            unreachable!()
        };
        let alias = stream.clone();
        let (frames, _frame_rx) = mpsc::channel(8);
        let sender = LiveValueSession::new_client(frames);
        sender.encode(&SchemaValue::Stream(stream)).unwrap();
        assert_eq!(
            sender.encode(&SchemaValue::Stream(alias)).unwrap_err(),
            "schema value stream was already transferred"
        );
        receiver.cancel();
        sender.cancel();
    }

    #[test]
    async fn recursive_registration_failures_roll_back_streams() {
        let (_source, stream) = stream_source(4);
        let SchemaValue::Stream(stream) = stream else {
            unreachable!()
        };
        let alias = stream.clone();
        let (frames, _frame_rx) = mpsc::channel(8);
        let sender = LiveValueSession::new_client(frames);
        assert!(
            sender
                .encode(&SchemaValue::Tuple {
                    elements: vec![SchemaValue::Stream(stream), SchemaValue::Stream(alias)],
                })
                .is_err()
        );
        sender.wait_idle().await;

        let (frames, mut frame_rx) = mpsc::channel(8);
        let receiver = LiveValueSession::new_client(frames);
        let error = receiver
            .decode(ProtoSchemaValue {
                value: Some(proto_schema_value::Value::TupleValue(TupleValue {
                    elements: vec![stream_reference(2), stream_reference(2)],
                })),
            })
            .await
            .unwrap_err();
        assert_eq!(error, "duplicate remote stream id 2");
        let cancel = frame_rx.recv().await.unwrap().request;
        assert!(matches!(
            cancel,
            Some(invocation_request::Request::StreamCancel(StreamCancel {
                stream_id: 2,
                role,
                ..
            })) if role == StreamCancelRole::OutputConsumer as i32
        ));
        receiver.wait_idle().await;
    }

    #[test]
    async fn rejected_start_rolls_back_streams_without_pre_accept_events() {
        let (frames, mut frame_rx) = mpsc::channel(8);
        let receiver = LiveValueSession::new_server(frames);

        let error = receiver
            .decode_start(ProtoSchemaValue {
                value: Some(proto_schema_value::Value::TupleValue(TupleValue {
                    elements: vec![stream_reference(1), stream_reference(1)],
                })),
            })
            .await
            .unwrap_err();

        assert_eq!(error, "duplicate remote stream id 1");
        assert!(frame_rx.try_recv().is_err());
        assert!(
            receiver
                .inner
                .imported
                .lock()
                .expect("live stream map mutex poisoned")
                .states
                .is_empty()
        );
        receiver.cancel();
    }

    #[test]
    fn local_stream_ids_are_not_reused_at_exhaustion() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_client(frames);
        let first_id = stream_id(&session.encode(&stream_source(1).1).unwrap());
        session
            .inner
            .next_stream_id
            .store(u64::MAX, Ordering::Release);
        let last_id = stream_id(&session.encode(&stream_source(1).1).unwrap());

        assert_ne!(first_id, last_id);
        assert_eq!(
            session.encode(&stream_source(1).1).unwrap_err(),
            "live stream ID space is exhausted"
        );
        session.cancel();
    }

    #[test]
    async fn first_output_can_arrive_before_the_session_registers_its_reader() {
        let (source, stream) = stream_source(1);
        assert_eq!(
            source.publisher.publish_item(SchemaValue::U32(7)).await,
            Ok(0)
        );
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new_server_with_capacity(frames, 1);
        let id = stream_id(&session.encode(&stream).unwrap());

        let Some(invocation_response::Response::OutputItem(item)) =
            frame_rx.recv().await.unwrap().response
        else {
            panic!("expected output item");
        };
        assert_eq!(item.stream_id, id);
        assert_eq!(item.offset, 0);
        assert_eq!(
            SchemaValue::try_from(item.value.unwrap()).unwrap(),
            SchemaValue::U32(7)
        );
        session.cancel();
    }

    #[test]
    async fn exported_stream_applies_bus_backpressure_and_preserves_order() {
        let (source, stream) = stream_source(1);
        let (frames, mut frame_rx) = mpsc::channel(1);
        let session = LiveValueSession::new_server_with_capacity(frames, 1);
        let id = stream_id(&session.encode(&stream).unwrap());
        source
            .publisher
            .publish_item(SchemaValue::String("first".to_string()))
            .await
            .unwrap();
        source
            .publisher
            .publish_item(SchemaValue::String("second".to_string()))
            .await
            .unwrap();
        source
            .publisher
            .publish_item(SchemaValue::String("third".to_string()))
            .await
            .unwrap();
        let fourth = tokio::spawn({
            let publisher = source.publisher.clone();
            async move {
                publisher
                    .publish_item(SchemaValue::String("fourth".to_string()))
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!fourth.is_finished());

        for (offset, expected) in ["first", "second", "third", "fourth"]
            .into_iter()
            .enumerate()
        {
            let Some(invocation_response::Response::OutputItem(item)) =
                frame_rx.recv().await.unwrap().response
            else {
                panic!("expected output item");
            };
            assert_eq!(item.stream_id, id);
            assert_eq!(item.offset, offset as u64);
            assert_eq!(
                SchemaValue::try_from(item.value.unwrap()).unwrap(),
                SchemaValue::String(expected.to_string())
            );
        }
        assert_eq!(fourth.await.unwrap(), Ok(3));
        session.cancel();
    }

    #[test]
    async fn output_consumer_cancellation_confirms_all_open_output_terminals() {
        let (first_source, first_stream) = stream_source(4);
        let (second_source, second_stream) = stream_source(4);
        let (frames, mut frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_server(frames);
        let encoded = session
            .encode(&SchemaValue::Tuple {
                elements: vec![first_stream, second_stream],
            })
            .unwrap();
        let proto_schema_value::Value::TupleValue(tuple) = encoded.value.unwrap() else {
            panic!("expected output stream tuple");
        };
        let first_id = stream_id(&tuple.elements[0]);
        let second_id = stream_id(&tuple.elements[1]);
        first_source
            .publisher
            .publish_item(SchemaValue::U32(1))
            .await
            .unwrap();
        second_source
            .publisher
            .publish_item(SchemaValue::U32(2))
            .await
            .unwrap();
        let mut item_streams = HashSet::new();
        while item_streams.len() < 2 {
            let frame = frame_rx.recv().await.unwrap();
            if let Some(invocation_response::Response::OutputItem(item)) = frame.response {
                item_streams.insert(item.stream_id);
            }
        }

        session
            .route_request(invocation_request::Request::StreamCancel(StreamCancel {
                stream_id: first_id,
                offset: 0,
                role: StreamCancelRole::OutputConsumer as i32,
                reason: StreamCancelReason::Cancelled as i32,
                details: Some("consumer stopped".to_string()),
            }))
            .await
            .unwrap();

        let mut terminals = HashMap::new();
        while terminals.len() < 2 {
            let frame = frame_rx.recv().await.unwrap();
            if let Some(invocation_response::Response::StreamCancel(cancel)) = frame.response {
                assert_eq!(cancel.role(), StreamCancelRole::OutputProducer);
                terminals.insert(cancel.stream_id, cancel.offset);
            }
        }
        assert_eq!(terminals.get(&first_id), Some(&0));
        assert_eq!(terminals.get(&second_id), Some(&1));
        session.wait_idle().await;
    }

    #[test]
    async fn auxiliary_output_subscription_starts_at_the_current_tail() {
        let (source, stream) = stream_source(4);
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new_server_with_capacity(frames, 4);
        let id = stream_id(&session.encode(&stream).unwrap());
        source
            .publisher
            .publish_item(SchemaValue::String("before".to_string()))
            .await
            .unwrap();
        let mut auxiliary = session.subscribe_output_tail(id).unwrap();
        source
            .publisher
            .publish_item(SchemaValue::String("after".to_string()))
            .await
            .unwrap();

        let event = auxiliary.recv().await.unwrap();

        assert_eq!(event.offset, 1);
        assert_eq!(
            event.payload,
            LiveStreamEventPayload::Item(SchemaValue::String("after".to_string()))
        );
        session.cancel();
    }

    #[test]
    async fn imported_stream_prefetches_into_the_bus_and_terminates_once() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_client_with_capacity(frames, 2);
        let value = session.decode(stream_reference(2)).await.unwrap();
        let mut primary = take_primary(value);

        session
            .route_response(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    stream_id: 2,
                    offset: 0,
                    value: Some(SchemaValue::U32(42).try_into().unwrap()),
                },
            ))
            .await
            .unwrap();
        let item = primary.recv().await.unwrap();
        assert_eq!(item.offset, 0);
        assert_eq!(
            item.payload,
            LiveStreamEventPayload::Item(SchemaValue::U32(42))
        );

        session
            .route_response(invocation_response::Response::OutputEnd(OutputStreamEnd {
                stream_id: 2,
                offset: 1,
            }))
            .await
            .unwrap();
        let end = primary.recv().await.unwrap();
        assert_eq!(end.offset, 1);
        assert_eq!(end.payload, LiveStreamEventPayload::End);
        assert_eq!(
            session
                .route_response(invocation_response::Response::OutputEnd(OutputStreamEnd {
                    stream_id: 2,
                    offset: 1,
                }))
                .await
                .unwrap_err(),
            "item for unknown remote stream 2"
        );
    }

    #[test]
    async fn packed_u8_admission_expands_offsets_and_acks_after_bus_acceptance() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_server_with_capacity(frames, 1);
        let value = session.decode(stream_reference(1)).await.unwrap();
        let mut primary = take_primary(value);

        let admission = tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .admit_input_item(InputStreamItem {
                        stream_id: 1,
                        sequence: 0,
                        payload: Some(input_stream_item::Payload::PackedU8(vec![7, 8, 9])),
                    })
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!admission.is_finished());

        for (offset, value) in [(0, 7), (1, 8), (2, 9)] {
            let event = primary.recv().await.unwrap();
            assert_eq!(event.offset, offset);
            assert_eq!(
                event.payload,
                LiveStreamEventPayload::Item(SchemaValue::U8(value))
            );
        }
        assert_eq!(
            admission.await.unwrap().unwrap(),
            InputItemAdmission::Acknowledged(InputStreamAck {
                stream_id: 1,
                sequence: 0,
                logical_item_count: 3,
            })
        );
        assert_eq!(
            session
                .admit_input_item(InputStreamItem {
                    stream_id: 1,
                    sequence: 2,
                    payload: Some(input_stream_item::Payload::Value(
                        SchemaValue::U8(10).try_into().unwrap(),
                    )),
                })
                .await
                .unwrap_err(),
            "input stream 1 expected sequence 3, got 2"
        );
        session.cancel();
    }

    #[test]
    async fn input_sequence_overflow_is_rejected_before_recursive_registration() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_server_with_capacity(frames, 1);
        let value = session.decode(stream_reference(1)).await.unwrap();
        let _primary = take_primary(value);
        let route = session.imported_route(1).unwrap();
        *route.next_sequence.lock().await = u64::MAX;

        assert_eq!(
            session
                .admit_input_item(InputStreamItem {
                    stream_id: 1,
                    sequence: u64::MAX,
                    payload: Some(input_stream_item::Payload::Value(stream_reference(3))),
                })
                .await
                .unwrap_err(),
            "input stream 1 sequence overflow"
        );
        assert!(
            !session
                .inner
                .imported
                .lock()
                .expect("live stream map mutex poisoned")
                .states
                .contains_key(&3)
        );
        assert!(
            !session
                .inner
                .seen_remote_stream_ids
                .lock()
                .expect("live stream ID set mutex poisoned")
                .contains(&3)
        );
        session.cancel();
    }

    #[test]
    async fn imported_stream_error_is_scoped_and_terminal() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_client(frames);
        let value = session.decode(stream_reference(2)).await.unwrap();
        let mut primary = take_primary(value);

        session
            .route_response(invocation_response::Response::OutputError(
                OutputStreamError {
                    stream_id: 2,
                    offset: 0,
                    details: "failed".to_string(),
                },
            ))
            .await
            .unwrap();

        assert_eq!(
            primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Error("failed".to_string())
        );
        assert!(!session.is_cancelled());
    }
}

#[cfg(test)]
mod bus_lifecycle_tests {
    use super::*;
    use crate::durable_host::stream_transport::{
        LiveStreamEndpoint, input_stream_pair, output_stream_pair,
    };
    use std::time::Duration;
    use test_r::{test, timeout};
    use tokio_util::sync::CancellationToken;

    fn stream_id(value: &ProtoSchemaValue) -> u64 {
        match value.value.as_ref() {
            Some(proto_schema_value::Value::StreamReference(reference)) => reference.stream_id,
            other => panic!("expected stream id, got {other:?}"),
        }
    }

    fn stream_reference(id: u64) -> ProtoSchemaValue {
        ProtoSchemaValue {
            value: Some(proto_schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: id },
            )),
        }
    }

    fn stream_source(capacity: usize) -> (LiveStreamPeer, SchemaValue) {
        let cancellation = CancellationToken::new();
        let (peer, stream) = input_stream_pair(capacity, &cancellation).unwrap();
        (peer, SchemaValue::Stream(stream))
    }

    fn take_primary(
        value: SchemaValue,
    ) -> crate::durable_host::stream_bus::PrimaryLiveStreamSubscriber<SchemaValue> {
        let SchemaValue::Stream(stream) = value else {
            panic!("expected stream");
        };
        stream
            .take_host_endpoint::<LiveStreamEndpoint>()
            .unwrap()
            .activate()
    }

    #[test]
    #[timeout("2s")]
    async fn dropping_one_imported_input_reader_cancels_only_that_stream() {
        let (frames, mut frame_rx) = mpsc::channel(16);
        let session = LiveValueSession::new_server(frames);
        let decoded = session
            .decode(ProtoSchemaValue {
                value: Some(proto_schema_value::Value::TupleValue(TupleValue {
                    elements: vec![stream_reference(1), stream_reference(3)],
                })),
            })
            .await
            .unwrap();
        let SchemaValue::Tuple { mut elements } = decoded else {
            panic!("expected tuple");
        };
        let second = elements.pop().unwrap();
        let first = elements.pop().unwrap();
        let first_endpoint = match first {
            SchemaValue::Stream(stream) => {
                stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap()
            }
            _ => unreachable!(),
        };
        let mut second_primary = take_primary(second);

        drop(first_endpoint);
        let Some(invocation_response::Response::StreamCancel(cancel)) =
            frame_rx.recv().await.unwrap().response
        else {
            panic!("expected first stream cancellation");
        };
        assert_eq!(cancel.stream_id, 1);
        assert_eq!(cancel.offset, 0);
        assert_eq!(cancel.role(), StreamCancelRole::InputConsumer);
        assert!(!session.is_cancelled());

        for sequence in 0..2 {
            session
                .route_request(invocation_request::Request::InputItem(InputStreamItem {
                    stream_id: 1,
                    sequence,
                    payload: Some(input_stream_item::Payload::Value(
                        SchemaValue::Bool(false).try_into().unwrap(),
                    )),
                }))
                .await
                .unwrap();
        }
        session
            .route_request(invocation_request::Request::InputEnd(InputStreamEnd {
                stream_id: 1,
                offset: 2,
            }))
            .await
            .unwrap();
        assert!(
            !session
                .inner
                .imported
                .lock()
                .expect("live stream map mutex poisoned")
                .cancelled
                .contains_key(&1)
        );
        assert!(frame_rx.try_recv().is_err());

        session
            .route_request(invocation_request::Request::InputItem(InputStreamItem {
                stream_id: 3,
                sequence: 0,
                payload: Some(input_stream_item::Payload::Value(
                    SchemaValue::Bool(true).try_into().unwrap(),
                )),
            }))
            .await
            .unwrap();
        assert_eq!(
            second_primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::Bool(true))
        );
        assert!(matches!(
            frame_rx.recv().await.unwrap().response,
            Some(invocation_response::Response::InputAck(InputStreamAck {
                stream_id: 3,
                sequence: 0,
                logical_item_count: 1,
            }))
        ));
        session
            .route_request(invocation_request::Request::InputEnd(InputStreamEnd {
                stream_id: 3,
                offset: 1,
            }))
            .await
            .unwrap();
        assert_eq!(
            second_primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::End
        );
        session.wait_idle().await;
    }

    #[test]
    async fn equal_remote_ids_in_independent_sessions_do_not_alias() {
        let (first_frames, _first_frame_rx) = mpsc::channel(8);
        let first = LiveValueSession::new_client(first_frames);
        let (second_frames, _second_frame_rx) = mpsc::channel(8);
        let second = LiveValueSession::new_client(second_frames);
        let mut first_primary = take_primary(first.decode(stream_reference(2)).await.unwrap());
        let mut second_primary = take_primary(second.decode(stream_reference(2)).await.unwrap());

        first
            .route_response(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    stream_id: 2,
                    offset: 0,
                    value: Some(SchemaValue::String("first".to_string()).try_into().unwrap()),
                },
            ))
            .await
            .unwrap();
        second
            .route_response(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    stream_id: 2,
                    offset: 0,
                    value: Some(
                        SchemaValue::String("second".to_string())
                            .try_into()
                            .unwrap(),
                    ),
                },
            ))
            .await
            .unwrap();

        assert_eq!(
            first_primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::String("first".to_string()))
        );
        assert_eq!(
            second_primary.recv().await.unwrap().payload,
            LiveStreamEventPayload::Item(SchemaValue::String("second".to_string()))
        );
        first.cancel();
        second.cancel();
    }

    #[test]
    async fn invocation_finish_rejects_open_streams_and_releases_readers() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_client(frames);
        let mut primary = take_primary(session.decode(stream_reference(2)).await.unwrap());

        let error = session.finish_invocation().await.unwrap_err();

        assert!(error.contains("open imported streams [2]"));
        assert_eq!(
            primary.recv().await.unwrap_err(),
            crate::durable_host::stream_bus::LiveStreamReceiveError::Closed
        );
        session.wait_idle().await;
    }

    #[test]
    async fn streams_discovered_in_items_get_independent_buses() {
        let (outer_source, outer_stream) = stream_source(4);
        let (nested_source, nested_stream) = stream_source(4);
        let (frames, mut frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new_server(frames);
        let outer_id = stream_id(&session.encode(&outer_stream).unwrap());
        outer_source
            .publisher
            .publish_item(SchemaValue::Option {
                inner: Some(Box::new(nested_stream)),
            })
            .await
            .unwrap();
        let Some(invocation_response::Response::OutputItem(item)) =
            frame_rx.recv().await.unwrap().response
        else {
            panic!("expected outer item");
        };
        let proto_schema_value::Value::OptionValue(option) = item.value.unwrap().value.unwrap()
        else {
            panic!("expected nested option");
        };
        let nested_id = stream_id(option.inner.as_deref().unwrap());
        assert_ne!(outer_id, nested_id);

        nested_source
            .publisher
            .publish_item(SchemaValue::String("nested".to_string()))
            .await
            .unwrap();
        let Some(invocation_response::Response::OutputItem(item)) =
            frame_rx.recv().await.unwrap().response
        else {
            panic!("expected nested item");
        };
        assert_eq!(item.stream_id, nested_id);
        assert_eq!(
            SchemaValue::try_from(item.value.unwrap()).unwrap(),
            SchemaValue::String("nested".to_string())
        );
        session.cancel();
    }

    #[test]
    async fn output_bus_error_becomes_one_stream_error_frame() {
        let (source, stream) = stream_source(4);
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new_server(frames);
        let id = stream_id(&session.encode(&stream).unwrap());
        source
            .publisher
            .publish_error("failed".to_string())
            .await
            .unwrap();

        assert!(matches!(
            frame_rx.recv().await.unwrap().response,
            Some(invocation_response::Response::OutputError(OutputStreamError {
                stream_id,
                offset: 0,
                details,
            })) if stream_id == id && details == "failed"
        ));
        session.wait_idle().await;
    }

    #[test]
    async fn output_primary_loss_cancels_the_invocation_tracker() {
        let cancellation = CancellationToken::new();
        let tracker = Arc::new(LiveStreamTracker::new(cancellation.clone(), 4));
        let (consumer, stream) = output_stream_pair(Some(tracker.clone()), 4).unwrap();
        assert_eq!(tracker.active.load(Ordering::Acquire), 1);

        drop(stream);

        assert!(cancellation.is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), tracker.wait_for_sources())
            .await
            .unwrap();
        drop(consumer);
    }

    #[test]
    async fn normal_output_drop_finishes_after_sending_stream_end() {
        let tracker = Arc::new(LiveStreamTracker::new(CancellationToken::new(), 4));
        let (consumer, stream) = output_stream_pair(Some(tracker.clone()), 4).unwrap();
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new_server(frames);
        let stream_id = stream_id(&session.encode(&SchemaValue::Stream(stream)).unwrap());

        drop(consumer);
        tracker.wait_for_sources().await;

        session
            .finish_invocation()
            .await
            .expect("a normal guest stream drop must wait for its end event");
        assert!(matches!(
            frame_rx.recv().await.unwrap().response,
            Some(invocation_response::Response::OutputEnd(OutputStreamEnd {
                stream_id: actual_stream_id,
                offset: 0,
            })) if actual_stream_id == stream_id
        ));
    }

    #[test]
    async fn cancellation_releases_an_exported_bus_waiter() {
        let (_source, stream) = stream_source(4);
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new_server(frames);
        session.encode(&stream).unwrap();

        session.cancel();

        tokio::time::timeout(Duration::from_secs(1), session.wait_idle())
            .await
            .unwrap();
    }

    #[test]
    async fn cancellation_releases_an_exported_stream_blocked_on_a_frame() {
        let (source, stream) = stream_source(1);
        let (frames, _frame_rx) = mpsc::channel(1);
        frames
            .send(InvocationResponse { response: None })
            .await
            .unwrap();
        let session = LiveValueSession::new_server_with_capacity(frames, 1);
        stream_id(&session.encode(&stream).unwrap());
        source
            .publisher
            .publish_item(SchemaValue::U32(42))
            .await
            .unwrap();
        tokio::task::yield_now().await;

        session.cancel();

        tokio::time::timeout(Duration::from_secs(1), session.wait_idle())
            .await
            .unwrap();
    }

    #[test]
    async fn unknown_stream_frames_are_rejected() {
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new_client(frames);
        assert_eq!(
            session
                .route_response(invocation_response::Response::OutputEnd(OutputStreamEnd {
                    stream_id: 2,
                    offset: 0,
                }))
                .await
                .unwrap_err(),
            "item for unknown remote stream 2"
        );
    }
}
