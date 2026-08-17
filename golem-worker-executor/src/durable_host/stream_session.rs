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
#[cfg(test)]
use crate::durable_host::stream_transport::LiveStreamTracker;
use crate::durable_host::stream_transport::{
    RelayPeer, RelayReceiver, SourceLifecycle, relay_endpoint_pair,
};
use golem_api_grpc::proto::golem::common::Empty;
use golem_api_grpc::proto::golem::schema::{
    FixedListValue, ListValue, MapEntry, MapValue, OptionValue, RecordValue, ResultValue,
    SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, TupleValue, UnionValue,
    VariantValue, result_value as proto_result_value, schema_value as proto_schema_value,
};
use golem_api_grpc::proto::golem::worker::{
    InvocationFrame as LiveInvocationFrame, StreamDemand as LiveStreamDemand,
    StreamDetach as LiveStreamDetach, StreamEnd as LiveStreamEnd, StreamError as LiveStreamError,
    StreamItem as LiveStreamItem, invocation_frame as live_invocation_frame,
};
use golem_schema::schema::SchemaValue;
#[cfg(test)]
use golem_schema::schema::SchemaValueStreamHandleRep;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

enum ImportedStreamEvent {
    Item(ImportedStreamItem),
    End,
    Error(String),
}

struct ImportedStreamItem {
    value: SchemaValue,
    registrations: ImportedRegistrationBatch,
}

#[must_use = "imported stream registrations must be committed or rolled back"]
#[derive(Default)]
struct ImportedRegistrationBatch {
    stream_ids: Vec<u64>,
}

impl ImportedRegistrationBatch {
    fn discharge_by_session_cancellation(self) {}

    fn is_empty(&self) -> bool {
        self.stream_ids.is_empty()
    }
}

#[derive(Clone)]
struct ImportedStreamRoute {
    events: mpsc::Sender<ImportedStreamEvent>,
    demand_outstanding: Arc<AtomicBool>,
    lifecycle: Arc<SourceLifecycle>,
}

enum ImportedStreamState {
    Registering(ImportedStreamRoute),
    Active(ImportedStreamRoute),
    DetachPending { response_outstanding: bool },
    DetachedAwaitingResponse,
}

#[derive(Default)]
struct ImportedStreams {
    states: HashMap<u64, ImportedStreamState>,
}

enum ExportedStreamCommand {
    Demand,
    Detach,
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
    frames: mpsc::Sender<LiveInvocationFrame>,
    exported: Mutex<HashMap<u64, mpsc::Sender<ExportedStreamCommand>>>,
    imported: Mutex<ImportedStreams>,
    imported_changed: tokio::sync::Notify,
    seen_remote_stream_ids: Mutex<HashSet<u64>>,
    activity: Arc<SessionActivity>,
    cancelled: tokio_util::sync::CancellationToken,
    failure: Mutex<Option<String>>,
}

/// Converts recursive live values to and from the session protocol and drives
/// one demand/item exchange per stream. The two peers allocate disjoint odd
/// and even stream IDs, so sibling streams cannot alias even when nested
/// streams are discovered in later items.
#[derive(Clone)]
pub(crate) struct LiveValueSession {
    inner: Arc<LiveValueSessionInner>,
}

impl LiveValueSession {
    pub(crate) fn new(
        first_local_stream_id: u64,
        frames: mpsc::Sender<LiveInvocationFrame>,
    ) -> Self {
        debug_assert!(first_local_stream_id == 1 || first_local_stream_id == 2);
        Self {
            inner: Arc::new(LiveValueSessionInner {
                next_stream_id: std::sync::atomic::AtomicU64::new(first_local_stream_id),
                expected_remote_parity: 1 - (first_local_stream_id & 1),
                frames,
                exported: Mutex::new(HashMap::new()),
                imported: Mutex::new(ImportedStreams::default()),
                imported_changed: tokio::sync::Notify::new(),
                seen_remote_stream_ids: Mutex::new(HashSet::new()),
                activity: Arc::new(SessionActivity::default()),
                cancelled: tokio_util::sync::CancellationToken::new(),
                failure: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn encode(&self, value: &SchemaValue) -> Result<ProtoSchemaValue, String> {
        self.encode_with_registered_streams(value)
            .map(|(value, _)| value)
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
            exported.remove(id);
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
                let receiver = stream.take_host_endpoint::<RelayReceiver>()?;
                self.spawn_exported(id, receiver)?;
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
        let _activity = self.inner.activity.start();
        let (value, registrations) = self.decode_with_registered_streams(value);
        match value {
            Ok(value) => {
                self.commit_imported_streams(registrations);
                Ok(value)
            }
            Err(error) => {
                self.rollback_imported_streams(registrations).await;
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
                let (peer, stream) = relay_endpoint_pair(None);
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

    pub(crate) async fn route_stream_frame(
        &self,
        frame: live_invocation_frame::Frame,
    ) -> Result<bool, String> {
        let _activity = self.inner.activity.start();
        match frame {
            live_invocation_frame::Frame::Demand(demand) => {
                let sender = self
                    .inner
                    .exported
                    .lock()
                    .expect("live stream map mutex poisoned")
                    .get(&demand.stream_id)
                    .cloned()
                    .ok_or_else(|| format!("demand for unknown stream {}", demand.stream_id))?;
                let result = tokio::select! {
                    result = sender.send(ExportedStreamCommand::Demand) => result,
                    _ = self.inner.cancelled.cancelled() => return Ok(true),
                };
                result.map_err(|_| format!("stream {} is no longer readable", demand.stream_id))?;
                Ok(true)
            }
            live_invocation_frame::Frame::Detach(detach) => {
                let sender = self
                    .inner
                    .exported
                    .lock()
                    .expect("live stream map mutex poisoned")
                    .get(&detach.stream_id)
                    .cloned()
                    .ok_or_else(|| format!("detach for unknown stream {}", detach.stream_id))?;
                tokio::select! {
                    _ = sender.send(ExportedStreamCommand::Detach) => {}
                    _ = self.inner.cancelled.cancelled() => {}
                }
                self.inner
                    .exported
                    .lock()
                    .expect("live stream map mutex poisoned")
                    .remove(&detach.stream_id);
                Ok(true)
            }
            live_invocation_frame::Frame::Item(item) => {
                let stream_id = item.stream_id;
                let value = item
                    .value
                    .ok_or_else(|| format!("stream {stream_id} item has no value"))?;
                let route = self.take_imported_route(stream_id, false)?;
                let (value, registrations) = self.decode_with_registered_streams(value);
                let value = match value {
                    Ok(value) => value,
                    Err(error) => {
                        self.rollback_imported_streams(registrations).await;
                        return Err(error);
                    }
                };
                let item = ImportedStreamItem {
                    value,
                    registrations,
                };
                if let Some(route) = route {
                    if let Err(item) = self.send_imported_item(route, item).await {
                        self.rollback_imported_item(item).await;
                        self.ensure_imported_detached(stream_id).await;
                    }
                } else {
                    self.rollback_imported_item(item).await;
                }
                Ok(true)
            }
            live_invocation_frame::Frame::End(end) => {
                let route = self.take_imported_route(end.stream_id, true)?;
                if let Some(route) = route {
                    self.send_imported(route, ImportedStreamEvent::End).await;
                }
                Ok(true)
            }
            live_invocation_frame::Frame::StreamError(error) => {
                let route = self.take_imported_route(error.stream_id, true)?;
                if let Some(route) = route {
                    self.send_imported(route, ImportedStreamEvent::Error(error.details))
                        .await;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
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
        self.inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned")
            .states
            .retain(|_, state| matches!(state, ImportedStreamState::Active(_)));
        self.inner.imported_changed.notify_waiters();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.cancelled.is_cancelled()
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
                        ImportedStreamState::Registering(_)
                        | ImportedStreamState::DetachPending { .. }
                        | ImportedStreamState::DetachedAwaitingResponse => settling = true,
                        ImportedStreamState::Active(_) => active.push(*id),
                    }
                }
                (active, settling)
            };
            let exported = self
                .inner
                .exported
                .lock()
                .expect("live stream map mutex poisoned")
                .keys()
                .copied()
                .collect::<Vec<_>>();
            if imported.is_empty() && exported.is_empty() {
                if !self.inner.activity.is_idle() || imported_settling {
                    tokio::select! {
                        _ = activity_changed => continue,
                        _ = imported_changed => continue,
                        _ = self.inner.cancelled.cancelled() => return Ok(()),
                    }
                }
                self.cancel();
                return Ok(());
            }

            let details = format!(
                "live invocation terminated with open imported streams {imported:?} and open exported streams {exported:?}"
            );
            self.fail(details.clone());
            return Err(details);
        }
    }

    pub(crate) fn fail(&self, error: String) {
        *self
            .inner
            .failure
            .lock()
            .expect("live invocation failure mutex poisoned") = Some(error);
        self.cancel();
    }

    async fn send_frame(&self, frame: live_invocation_frame::Frame) -> bool {
        tokio::select! {
            result = self.inner.frames.send(LiveInvocationFrame { frame: Some(frame) }) => {
                result.is_ok()
            }
            _ = self.inner.cancelled.cancelled() => false,
        }
    }

    async fn rollback_imported_item(&self, item: ImportedStreamItem) {
        let ImportedStreamItem {
            value,
            registrations,
        } = item;
        self.rollback_imported_streams(registrations).await;
        drop(value);
    }

    async fn rollback_imported_streams(&self, registrations: ImportedRegistrationBatch) {
        if registrations.is_empty() {
            return;
        }
        for id in registrations.stream_ids {
            self.ensure_imported_detached(id).await;
        }
    }

    async fn ensure_imported_detached(&self, id: u64) {
        enum DetachStep {
            Send,
            Wait,
            Done,
        }

        loop {
            let imported_changed = self.inner.imported_changed.notified();
            let step = {
                let mut imported = self
                    .inner
                    .imported
                    .lock()
                    .expect("live stream map mutex poisoned");
                match imported.states.get(&id) {
                    Some(
                        ImportedStreamState::Registering(route)
                        | ImportedStreamState::Active(route),
                    ) => {
                        let response_outstanding =
                            route.demand_outstanding.swap(false, Ordering::AcqRel);
                        imported.states.insert(
                            id,
                            ImportedStreamState::DetachPending {
                                response_outstanding,
                            },
                        );
                        DetachStep::Send
                    }
                    Some(ImportedStreamState::DetachPending { .. }) => DetachStep::Wait,
                    Some(ImportedStreamState::DetachedAwaitingResponse) | None => DetachStep::Done,
                }
            };
            match step {
                DetachStep::Send => {
                    self.inner.imported_changed.notify_waiters();
                    let sent = self
                        .send_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                            stream_id: id,
                        }))
                        .await;
                    let mut imported = self
                        .inner
                        .imported
                        .lock()
                        .expect("live stream map mutex poisoned");
                    let response_outstanding = match imported.states.get(&id) {
                        Some(ImportedStreamState::DetachPending {
                            response_outstanding,
                        }) => Some(*response_outstanding),
                        _ => None,
                    };
                    match response_outstanding {
                        Some(true) if sent && !self.is_cancelled() => {
                            imported
                                .states
                                .insert(id, ImportedStreamState::DetachedAwaitingResponse);
                        }
                        Some(_) => {
                            imported.states.remove(&id);
                        }
                        None => {}
                    }
                    drop(imported);
                    self.inner.imported_changed.notify_waiters();
                    if !sent && !self.is_cancelled() {
                        self.cancel();
                    }
                    return;
                }
                DetachStep::Wait => {
                    tokio::select! {
                        _ = imported_changed => {}
                        _ = self.inner.cancelled.cancelled() => {
                            let mut imported = self
                                .inner
                                .imported
                                .lock()
                                .expect("live stream map mutex poisoned");
                            if matches!(
                                imported.states.get(&id),
                                Some(ImportedStreamState::DetachPending { .. })
                            ) {
                                imported.states.remove(&id);
                            }
                            drop(imported);
                            self.inner.imported_changed.notify_waiters();
                            return;
                        }
                    }
                }
                DetachStep::Done => return,
            }
        }
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

    async fn send_exported_terminal(
        &self,
        id: u64,
        frame: live_invocation_frame::Frame,
        command_rx: &mut mpsc::Receiver<ExportedStreamCommand>,
    ) -> bool {
        loop {
            tokio::select! {
                biased;
                command = command_rx.recv() => match command {
                    Some(ExportedStreamCommand::Demand) => {}
                    Some(ExportedStreamCommand::Detach) => {
                        return self.send_detached_response(id).await;
                    }
                    None => return false,
                },
                result = self.inner.frames.send(LiveInvocationFrame {
                    frame: Some(frame.clone()),
                }) => return result.is_ok(),
                _ = self.inner.cancelled.cancelled() => return false,
            }
        }
    }

    async fn send_detached_response(&self, id: u64) -> bool {
        self.send_frame(live_invocation_frame::Frame::End(LiveStreamEnd {
            stream_id: id,
        }))
        .await
    }

    fn validate_remote_id(&self, id: u64) -> Result<(), String> {
        if id == 0 || id & 1 != self.inner.expected_remote_parity {
            Err(format!("invalid remote stream id {id}"))
        } else {
            Ok(())
        }
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

    fn spawn_exported(&self, id: u64, mut receiver: RelayReceiver) -> Result<(), String> {
        let (commands, mut command_rx) = mpsc::channel(1);
        let mut exported = self
            .inner
            .exported
            .lock()
            .expect("live stream map mutex poisoned");
        match exported.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(commands);
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
            loop {
                let command = tokio::select! {
                    command = command_rx.recv() => command,
                    _ = session.inner.cancelled.cancelled() => None,
                };
                let Some(command) = command else {
                    break;
                };
                match command {
                    ExportedStreamCommand::Demand => {
                        let next = tokio::select! {
                            biased;
                            command = command_rx.recv() => match command {
                                Some(ExportedStreamCommand::Detach) => {
                                    session.send_detached_response(id).await;
                                    None
                                }
                                None => None,
                                Some(ExportedStreamCommand::Demand) => {
                                    session.send_exported_terminal(
                                        id,
                                        live_invocation_frame::Frame::StreamError(LiveStreamError {
                                            stream_id: id,
                                            details: "stream received overlapping demand".to_string(),
                                        }),
                                        &mut command_rx,
                                    ).await;
                                    None
                                }
                            },
                            next = receiver.next() => Some(next),
                            _ = session.inner.cancelled.cancelled() => break,
                        };
                        let Some(next) = next else {
                            break;
                        };
                        let (frame, registered_stream_ids) = match next {
                            Ok(Some(value)) => match session.encode_with_registered_streams(&value)
                            {
                                Ok((value, registered_stream_ids)) => (
                                    live_invocation_frame::Frame::Item(LiveStreamItem {
                                        stream_id: id,
                                        value: Some(value),
                                    }),
                                    registered_stream_ids,
                                ),
                                Err(error) => (
                                    live_invocation_frame::Frame::StreamError(LiveStreamError {
                                        stream_id: id,
                                        details: error,
                                    }),
                                    Vec::new(),
                                ),
                            },
                            Ok(None) => (
                                live_invocation_frame::Frame::End(LiveStreamEnd { stream_id: id }),
                                Vec::new(),
                            ),
                            Err(error) => (
                                live_invocation_frame::Frame::StreamError(LiveStreamError {
                                    stream_id: id,
                                    details: error,
                                }),
                                Vec::new(),
                            ),
                        };
                        let terminal = matches!(
                            frame,
                            live_invocation_frame::Frame::End(_)
                                | live_invocation_frame::Frame::StreamError(_)
                        );
                        let sent = tokio::select! {
                            biased;
                            command = command_rx.recv() => {
                                match command {
                                    Some(ExportedStreamCommand::Demand) => {
                                        session.discard_exported_streams(&registered_stream_ids);
                                        session.send_exported_terminal(
                                            id,
                                            live_invocation_frame::Frame::StreamError(LiveStreamError {
                                                stream_id: id,
                                                details: "stream received overlapping demand".to_string(),
                                            }),
                                            &mut command_rx,
                                        ).await;
                                    }
                                    Some(ExportedStreamCommand::Detach) => {
                                        session.discard_exported_streams(&registered_stream_ids);
                                        session.send_detached_response(id).await;
                                    }
                                    None => {}
                                }
                                false
                            },
                            result = session.inner.frames.send(LiveInvocationFrame {
                                frame: Some(frame),
                            }) => result.is_ok(),
                            _ = session.inner.cancelled.cancelled() => false,
                        };
                        if !sent {
                            session.discard_exported_streams(&registered_stream_ids);
                        }
                        if !sent || terminal {
                            break;
                        }
                    }
                    ExportedStreamCommand::Detach => break,
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

    fn spawn_imported(&self, id: u64, mut peer: RelayPeer) -> Result<(), String> {
        if !self
            .inner
            .seen_remote_stream_ids
            .lock()
            .expect("live stream ID set mutex poisoned")
            .insert(id)
        {
            return Err(format!("duplicate remote stream id {id}"));
        }
        let (events, mut event_rx) = mpsc::channel(1);
        let demand_outstanding = Arc::new(AtomicBool::new(false));
        let previous = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned")
            .states
            .insert(
                id,
                ImportedStreamState::Registering(ImportedStreamRoute {
                    events,
                    demand_outstanding: demand_outstanding.clone(),
                    lifecycle: peer.lifecycle.clone(),
                }),
            );
        debug_assert!(previous.is_none(), "new remote stream ID already imported");
        self.inner.imported_changed.notify_waiters();
        let session = self.clone();
        let activity = self.inner.activity.start();
        tokio::spawn(async move {
            let _activity = activity;
            loop {
                tokio::select! {
                    biased;
                    event = event_rx.recv() => match event {
                        Some(ImportedStreamEvent::Item(item)) => {
                            let permit = tokio::select! {
                                result = peer.item_tx.reserve() => result.ok(),
                                _ = session.inner.cancelled.cancelled() => None,
                            };
                            if let Some(permit) = permit.filter(|_| !session.is_cancelled()) {
                                let ImportedStreamItem {
                                    value,
                                    registrations,
                                } = item;
                                session.commit_imported_streams(registrations);
                                permit.send(Ok(value));
                            } else {
                                session.rollback_imported_item(item).await;
                                break;
                            }
                        }
                        Some(ImportedStreamEvent::Error(error)) => {
                            tokio::select! {
                                _ = peer.item_tx.send(Err(error)) => {}
                                _ = session.inner.cancelled.cancelled() => {}
                            }
                            break;
                        }
                        Some(ImportedStreamEvent::End) | None => break,
                    },
                    _ = session.inner.cancelled.cancelled() => {
                        let error = session
                            .inner
                            .failure
                            .lock()
                            .expect("live invocation failure mutex poisoned")
                            .clone()
                            .unwrap_or_else(|| "live streaming invocation was cancelled".to_string());
                        let _ = peer.item_tx.try_send(Err(error));
                        break;
                    },
                    demand = peer.demand_rx.recv() => match demand {
                        Some(()) => {
                            if demand_outstanding.swap(true, Ordering::AcqRel) {
                                let error = format!(
                                    "remote stream {id} received overlapping local demand"
                                );
                                session.fail(error.clone());
                                let _ = peer.item_tx.try_send(Err(error));
                                break;
                            }
                            if !session.send_frame(
                                live_invocation_frame::Frame::Demand(LiveStreamDemand {
                                    stream_id: id,
                                }),
                            ).await {
                                demand_outstanding.store(false, Ordering::Release);
                                break;
                            }
                        }
                        None => break,
                    },
                }
            }
            event_rx.close();
            while let Ok(event) = event_rx.try_recv() {
                if let ImportedStreamEvent::Item(item) = event {
                    if session.is_cancelled() {
                        let ImportedStreamItem {
                            value,
                            registrations,
                        } = item;
                        registrations.discharge_by_session_cancellation();
                        drop(value);
                    } else {
                        session.rollback_imported_item(item).await;
                    }
                }
            }
            session.ensure_imported_detached(id).await;
        });
        Ok(())
    }

    fn take_imported_route(
        &self,
        id: u64,
        terminal: bool,
    ) -> Result<Option<ImportedStreamRoute>, String> {
        self.validate_remote_id(id)?;
        let mut imported = self
            .inner
            .imported
            .lock()
            .expect("live stream map mutex poisoned");
        let result = match imported.states.get_mut(&id) {
            Some(ImportedStreamState::Registering(route) | ImportedStreamState::Active(route)) => {
                if !route.demand_outstanding.swap(false, Ordering::AcqRel) {
                    return Err(format!(
                        "frame for remote stream {id} received without outstanding demand"
                    ));
                }
                let detached = route.lifecycle.finished.load(Ordering::Acquire);
                let route = (!detached).then(|| route.clone());
                if terminal {
                    imported.states.remove(&id);
                }
                Ok(route)
            }
            Some(ImportedStreamState::DetachPending {
                response_outstanding,
            }) if *response_outstanding => {
                *response_outstanding = false;
                Ok(None)
            }
            Some(ImportedStreamState::DetachedAwaitingResponse) => {
                imported.states.remove(&id);
                Ok(None)
            }
            Some(ImportedStreamState::DetachPending { .. }) | None => {
                Err(format!("frame for unknown remote stream {id}"))
            }
        };
        drop(imported);
        self.inner.imported_changed.notify_waiters();
        result
    }

    async fn send_imported_item(
        &self,
        route: ImportedStreamRoute,
        item: ImportedStreamItem,
    ) -> Result<(), ImportedStreamItem> {
        let permit = tokio::select! {
            result = route.events.reserve() => result.ok(),
            _ = self.inner.cancelled.cancelled() => None,
        };
        if let Some(permit) = permit {
            permit.send(ImportedStreamEvent::Item(item));
            Ok(())
        } else {
            Err(item)
        }
    }

    async fn send_imported(&self, route: ImportedStreamRoute, event: ImportedStreamEvent) {
        tokio::select! {
            _ = route.events.send(event) => {}
            _ = self.inner.cancelled.cancelled() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::{test, timeout};

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

    fn stream_value() -> SchemaValue {
        let (_source, stream) = relay_endpoint_pair(None);
        SchemaValue::Stream(stream)
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
                golem_schema::schema::schema_value::ResultValuePayload::Ok { value }
                | golem_schema::schema::schema_value::ResultValuePayload::Err { value } => {
                    value.as_deref().map(count_streams).unwrap_or_default()
                }
            },
            SchemaValue::Union(value) => count_streams(&value.body),
            _ => 0,
        }
    }

    #[test]
    async fn recursive_composites_preserve_live_streams() {
        use golem_schema::schema::schema_value::{
            ResultValuePayload, UnionValuePayload, VariantValuePayload,
        };

        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::Variant(VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(stream_value())),
                }),
                SchemaValue::Tuple {
                    elements: vec![stream_value()],
                },
                SchemaValue::List {
                    elements: vec![stream_value()],
                },
                SchemaValue::FixedList {
                    elements: vec![stream_value()],
                },
                SchemaValue::Map {
                    entries: vec![(stream_value(), stream_value())],
                },
                SchemaValue::Option {
                    inner: Some(Box::new(stream_value())),
                },
                SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(stream_value())),
                }),
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(stream_value())),
                }),
                SchemaValue::Union(UnionValuePayload {
                    tag: "stream".to_string(),
                    body: Box::new(stream_value()),
                }),
            ],
        };
        let (sender_frames, _sender_frame_rx) = mpsc::channel(32);
        let sender = LiveValueSession::new(1, sender_frames);
        let encoded = sender.encode(&value).unwrap();
        let (receiver_frames, _receiver_frame_rx) = mpsc::channel(32);
        let receiver = LiveValueSession::new(2, receiver_frames);

        let decoded = receiver.decode(encoded).await.unwrap();

        assert_eq!(count_streams(&decoded), 10);
        sender.cancel();
        receiver.cancel();
    }

    #[test]
    async fn stream_ids_are_affine_and_validated_per_session() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let receiver = LiveValueSession::new(1, frames);

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

        let (_source, stream) = relay_endpoint_pair(None);
        let alias = stream.clone();
        let (frames, _frame_rx) = mpsc::channel(8);
        let sender = LiveValueSession::new(1, frames);
        sender.encode(&SchemaValue::Stream(stream)).unwrap();
        assert_eq!(
            sender.encode(&SchemaValue::Stream(alias)).unwrap_err(),
            "schema value stream was already transferred"
        );
        receiver.cancel();
        sender.cancel();
    }

    #[test]
    fn local_stream_id_exhaustion_must_not_reuse_an_active_id() {
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new(1, frames);

        let first_id = stream_id(&session.encode(&stream_value()).unwrap());
        session
            .inner
            .next_stream_id
            .store(u64::MAX, Ordering::Release);
        let last_id = stream_id(&session.encode(&stream_value()).unwrap());
        let error = session.encode(&stream_value()).unwrap_err();

        session.cancel();
        assert_ne!(first_id, last_id);
        assert_eq!(error, "live stream ID space is exhausted");
    }

    #[test]
    async fn failed_recursive_encode_releases_streams_registered_before_the_error() {
        let (_source, stream) = relay_endpoint_pair(None);
        let alias = stream.clone();
        let (frames, _frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new(1, frames);

        let result = session.encode(&SchemaValue::Tuple {
            elements: vec![SchemaValue::Stream(stream), SchemaValue::Stream(alias)],
        });

        assert!(result.is_err(), "aliasing a stream must be rejected");
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("a failed encode must not leave an exported stream active");
    }

    #[test]
    async fn failed_recursive_decode_releases_streams_registered_before_the_error() {
        let (frames, mut frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new(1, frames);
        let value = ProtoSchemaValue {
            value: Some(proto_schema_value::Value::TupleValue(TupleValue {
                elements: vec![stream_reference(2), stream_reference(2)],
            })),
        };

        let error = session.decode(value).await.unwrap_err();

        assert_eq!(error, "duplicate remote stream id 2");
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: 2
            }))
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("a failed decode must not leave an imported stream active");
    }

    #[test]
    async fn equal_stream_ids_in_different_sessions_do_not_alias() {
        let (first_frames, mut first_frame_rx) = mpsc::channel(8);
        let first = LiveValueSession::new(1, first_frames);
        let (second_frames, mut second_frame_rx) = mpsc::channel(8);
        let second = LiveValueSession::new(1, second_frames);
        let SchemaValue::Stream(first_stream) = first.decode(stream_reference(2)).await.unwrap()
        else {
            panic!("expected first stream");
        };
        let SchemaValue::Stream(second_stream) = second.decode(stream_reference(2)).await.unwrap()
        else {
            panic!("expected second stream");
        };
        let mut first_reader = first_stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let mut second_reader = second_stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let first_next = tokio::spawn(async move { first_reader.next().await });
        let second_next = tokio::spawn(async move { second_reader.next().await });
        assert!(matches!(
            first_frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));
        assert!(matches!(
            second_frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));

        first
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(SchemaValue::String("first".to_string()).try_into().unwrap()),
            }))
            .await
            .unwrap();
        second
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(
                    SchemaValue::String("second".to_string())
                        .try_into()
                        .unwrap(),
                ),
            }))
            .await
            .unwrap();

        assert_eq!(
            first_next.await.unwrap().unwrap(),
            Some(SchemaValue::String("first".to_string()))
        );
        assert_eq!(
            second_next.await.unwrap().unwrap(),
            Some(SchemaValue::String("second".to_string()))
        );
    }

    #[test]
    async fn frames_for_unknown_remote_streams_are_rejected() {
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);

        let error = session
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(SchemaValue::Bool(true).try_into().unwrap()),
            }))
            .await
            .unwrap_err();

        assert_eq!(error, "frame for unknown remote stream 2");

        for frame in [
            live_invocation_frame::Frame::End(LiveStreamEnd { stream_id: 2 }),
            live_invocation_frame::Frame::StreamError(LiveStreamError {
                stream_id: 2,
                details: "unknown".to_string(),
            }),
        ] {
            assert_eq!(
                session.route_stream_frame(frame).await.unwrap_err(),
                "frame for unknown remote stream 2"
            );
        }
    }

    #[test]
    async fn exported_streams_do_not_read_before_demand() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let encoded = session.encode(&SchemaValue::Stream(stream)).unwrap();
        let id = stream_id(&encoded);

        tokio::task::yield_now().await;
        assert!(source.demand_rx.try_recv().is_err());
        assert!(frame_rx.try_recv().is_err());

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));
        source
            .item_tx
            .send(Ok(SchemaValue::Bool(true)))
            .await
            .unwrap();

        let frame = frame_rx.recv().await.unwrap().frame.unwrap();
        let live_invocation_frame::Frame::Item(item) = frame else {
            panic!("expected item frame");
        };
        assert_eq!(item.stream_id, id);
        assert_eq!(
            session.decode(item.value.unwrap()).await.unwrap(),
            SchemaValue::Bool(true)
        );
    }

    #[test]
    async fn sibling_stream_demands_are_isolated() {
        let (mut first, first_stream) = relay_endpoint_pair(None);
        let (mut second, second_stream) = relay_endpoint_pair(None);
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let encoded = session
            .encode(&SchemaValue::Tuple {
                elements: vec![
                    SchemaValue::Stream(first_stream),
                    SchemaValue::Stream(second_stream),
                ],
            })
            .unwrap();
        let proto_schema_value::Value::TupleValue(tuple) = encoded.value.unwrap() else {
            panic!("expected tuple");
        };
        let first_id = stream_id(&tuple.elements[0]);
        let second_id = stream_id(&tuple.elements[1]);
        assert_ne!(first_id, second_id);

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: second_id,
            }))
            .await
            .unwrap();
        assert_eq!(second.demand_rx.recv().await, Some(()));
        assert!(first.demand_rx.try_recv().is_err());

        session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: first_id,
            }))
            .await
            .unwrap();
        session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: second_id,
            }))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .unwrap();
    }

    #[test]
    async fn imported_stream_preserves_demand_and_items() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let value = session.decode(stream_reference(2)).await.unwrap();
        let SchemaValue::Stream(stream) = value else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let next = tokio::spawn(async move { receiver.next().await });

        let frame = frame_rx.recv().await.unwrap().frame.unwrap();
        assert!(matches!(
            frame,
            live_invocation_frame::Frame::Demand(LiveStreamDemand { stream_id: 2 })
        ));
        session
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(SchemaValue::U32(42).try_into().unwrap()),
            }))
            .await
            .unwrap();

        assert_eq!(next.await.unwrap().unwrap(), Some(SchemaValue::U32(42)));
    }

    #[test]
    async fn imported_stream_rejects_item_before_demand() {
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(_stream) = session.decode(stream_reference(2)).await.unwrap()
        else {
            panic!("expected stream");
        };

        let result = session
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(SchemaValue::U32(42).try_into().unwrap()),
            }))
            .await;

        assert!(
            result.is_err(),
            "an item without outstanding demand must be rejected"
        );
    }

    #[test]
    async fn producer_errors_are_terminal_and_scoped_to_the_stream() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let encoded = session.encode(&SchemaValue::Stream(stream)).unwrap();
        let id = stream_id(&encoded);

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));
        source
            .item_tx
            .send(Err("producer failed".to_string()))
            .await
            .unwrap();

        let frame = frame_rx.recv().await.unwrap().frame.unwrap();
        assert!(matches!(
            frame,
            live_invocation_frame::Frame::StreamError(LiveStreamError {
                stream_id,
                details,
            }) if stream_id == id && details == "producer failed"
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .unwrap();
    }

    #[test]
    async fn overlapping_exported_demand_is_terminal_for_that_stream() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let id = stream_id(&session.encode(&SchemaValue::Stream(stream)).unwrap());

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));
        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: id,
            }))
            .await
            .unwrap();

        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::StreamError(LiveStreamError {
                stream_id,
                details,
            })) if stream_id == id && details == "stream received overlapping demand"
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("overlapping demand must terminate the exported relay");
    }

    #[test]
    async fn overlapping_imported_demand_fails_the_session() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();

        receiver.demand_tx.send(()).await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));
        receiver.demand_tx.send(()).await.unwrap();

        assert_eq!(
            receiver.item_rx.recv().await.unwrap().unwrap_err(),
            "remote stream 2 received overlapping local demand"
        );
        assert!(session.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("overlapping imported demand must terminate the session relay");
    }

    #[test]
    async fn invocation_terminal_requires_all_imported_streams_to_finish() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let next = tokio::spawn(async move { receiver.next().await });
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));

        let error = session.finish_invocation().await.unwrap_err();

        assert!(error.contains("open imported streams [2]"));
        assert_eq!(next.await.unwrap().unwrap_err(), error);
    }

    #[test]
    async fn premature_invocation_terminal_reaches_reader_before_first_demand() {
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();

        let error = session.finish_invocation().await.unwrap_err();
        session.wait_idle().await;
        receiver.demand_tx.closed().await;

        assert_eq!(receiver.next().await.unwrap_err(), error);
    }

    #[test]
    async fn imported_stream_reports_session_cancellation_instead_of_eof() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let next = tokio::spawn(async move { receiver.next().await });
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));

        session.cancel();

        let result = next.await.unwrap();
        assert!(
            result.is_err(),
            "semantic invocation cancellation must not be reported as clean EOF"
        );
    }

    #[test]
    async fn imported_stream_terminal_is_monotonic() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let next = tokio::spawn(async move { receiver.next().await });
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));

        session
            .route_stream_frame(live_invocation_frame::Frame::End(LiveStreamEnd {
                stream_id: 2,
            }))
            .await
            .unwrap();
        assert_eq!(
            session
                .route_stream_frame(live_invocation_frame::Frame::End(LiveStreamEnd {
                    stream_id: 2,
                }))
                .await
                .unwrap_err(),
            "frame for unknown remote stream 2"
        );
        session.finish_invocation().await.unwrap();
        assert_eq!(next.await.unwrap().unwrap(), None);
        assert_eq!(
            session.decode(stream_reference(2)).await.unwrap_err(),
            "duplicate remote stream id 2"
        );
    }

    #[test]
    async fn imported_error_terminal_cannot_be_repeated_or_replaced() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let next = tokio::spawn(async move { receiver.next().await });
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));

        session
            .route_stream_frame(live_invocation_frame::Frame::StreamError(LiveStreamError {
                stream_id: 2,
                details: "failed".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(next.await.unwrap().unwrap_err(), "failed");

        for frame in [
            live_invocation_frame::Frame::StreamError(LiveStreamError {
                stream_id: 2,
                details: "duplicate".to_string(),
            }),
            live_invocation_frame::Frame::End(LiveStreamEnd { stream_id: 2 }),
        ] {
            assert_eq!(
                session.route_stream_frame(frame).await.unwrap_err(),
                "frame for unknown remote stream 2"
            );
        }
    }

    #[test]
    async fn streams_discovered_in_items_get_independent_ids() {
        let (mut outer_source, outer_stream) = relay_endpoint_pair(None);
        let (mut nested_source, nested_stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new(1, frames);
        let outer_id = stream_id(&session.encode(&SchemaValue::Stream(outer_stream)).unwrap());

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: outer_id,
            }))
            .await
            .unwrap();
        assert_eq!(outer_source.demand_rx.recv().await, Some(()));
        outer_source
            .item_tx
            .send(Ok(SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::Stream(nested_stream))),
            }))
            .await
            .unwrap();

        let live_invocation_frame::Frame::Item(item) =
            frame_rx.recv().await.unwrap().frame.unwrap()
        else {
            panic!("expected item frame");
        };
        let proto_schema_value::Value::OptionValue(option) = item.value.unwrap().value.unwrap()
        else {
            panic!("expected option item");
        };
        let nested_id = stream_id(option.inner.as_deref().unwrap());
        assert_ne!(outer_id, nested_id);

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: nested_id,
            }))
            .await
            .unwrap();
        assert_eq!(nested_source.demand_rx.recv().await, Some(()));

        for id in [outer_id, nested_id] {
            session
                .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                    stream_id: id,
                }))
                .await
                .unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .unwrap();
    }

    #[test]
    async fn nested_stream_remains_live_after_its_outer_stream_ends() {
        let (mut outer_source, outer_stream) = relay_endpoint_pair(None);
        let (mut nested_source, nested_stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(8);
        let session = LiveValueSession::new(1, frames);
        let outer_id = stream_id(&session.encode(&SchemaValue::Stream(outer_stream)).unwrap());

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: outer_id,
            }))
            .await
            .unwrap();
        assert_eq!(outer_source.demand_rx.recv().await, Some(()));
        outer_source
            .item_tx
            .send(Ok(SchemaValue::Stream(nested_stream)))
            .await
            .unwrap();
        let Some(live_invocation_frame::Frame::Item(item)) = frame_rx.recv().await.unwrap().frame
        else {
            panic!("expected outer item");
        };
        let nested_id = stream_id(item.value.as_ref().unwrap());

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: outer_id,
            }))
            .await
            .unwrap();
        assert_eq!(outer_source.demand_rx.recv().await, Some(()));
        drop(outer_source.item_tx);
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::End(LiveStreamEnd { stream_id }))
                if stream_id == outer_id
        ));

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: nested_id,
            }))
            .await
            .unwrap();
        assert_eq!(nested_source.demand_rx.recv().await, Some(()));
        session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: nested_id,
            }))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("the nested stream should be independent of its terminated outer stream");
    }

    #[test]
    async fn detaching_one_source_does_not_finish_its_sibling() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let tracker = Arc::new(LiveStreamTracker::new(cancellation));
        let (_first_source, first_stream) = relay_endpoint_pair(Some(tracker.clone()));
        let (_second_source, second_stream) = relay_endpoint_pair(Some(tracker.clone()));
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let encoded = session
            .encode(&SchemaValue::Tuple {
                elements: vec![
                    SchemaValue::Stream(first_stream),
                    SchemaValue::Stream(second_stream),
                ],
            })
            .unwrap();
        let proto_schema_value::Value::TupleValue(tuple) = encoded.value.unwrap() else {
            panic!("expected tuple");
        };
        let first_id = stream_id(&tuple.elements[0]);
        let second_id = stream_id(&tuple.elements[1]);

        session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: first_id,
            }))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while tracker.active.load(Ordering::Acquire) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: second_id,
            }))
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tracker.wait_for_sources(),
        )
        .await
        .unwrap();
    }

    #[test]
    async fn dropping_an_imported_handle_detaches_its_remote_stream() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };

        drop(stream);

        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: 2
            }))
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("dropping an imported handle must release its relay task");
        session.finish_invocation().await.unwrap();
    }

    #[test]
    async fn detached_imported_stream_accepts_one_racing_response() {
        for response in 0..3 {
            let (frames, mut frame_rx) = mpsc::channel(4);
            let session = LiveValueSession::new(1, frames);
            let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap()
            else {
                panic!("expected stream");
            };
            let receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
            receiver.demand_tx.send(()).await.unwrap();
            assert!(matches!(
                frame_rx.recv().await.unwrap().frame,
                Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                    stream_id: 2
                }))
            ));

            drop(receiver);
            assert!(matches!(
                frame_rx.recv().await.unwrap().frame,
                Some(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                    stream_id: 2
                }))
            ));
            let idle_session = session.clone();
            let idle = tokio::spawn(async move { idle_session.wait_idle().await });
            tokio::task::yield_now().await;
            assert!(
                !idle.is_finished(),
                "the detached imported relay must await its outstanding response"
            );

            let frame = match response {
                0 => live_invocation_frame::Frame::Item(LiveStreamItem {
                    stream_id: 2,
                    value: Some(SchemaValue::U32(42).try_into().unwrap()),
                }),
                1 => live_invocation_frame::Frame::End(LiveStreamEnd { stream_id: 2 }),
                _ => live_invocation_frame::Frame::StreamError(LiveStreamError {
                    stream_id: 2,
                    details: "racing failure".to_string(),
                }),
            };
            session.route_stream_frame(frame.clone()).await.unwrap();
            idle.await.unwrap();
            assert_eq!(
                session.route_stream_frame(frame).await.unwrap_err(),
                "frame for unknown remote stream 2"
            );
            assert_eq!(
                session.decode(stream_reference(2)).await.unwrap_err(),
                "duplicate remote stream id 2"
            );
            session.finish_invocation().await.unwrap();
        }
    }

    #[test]
    #[timeout("2s")]
    async fn server_idle_must_wait_for_detached_response_with_nested_stream() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let tracker = Arc::new(LiveStreamTracker::new(cancellation));
        let (mut outer_source, outer_stream) = relay_endpoint_pair(Some(tracker.clone()));
        let (_nested_source, nested_stream) = relay_endpoint_pair(Some(tracker.clone()));

        let (client_frames, mut client_frame_rx) = mpsc::channel(8);
        let client = LiveValueSession::new(1, client_frames);
        let encoded_outer = client.encode(&SchemaValue::Stream(outer_stream)).unwrap();

        let (server_frames, mut server_frame_rx) = mpsc::channel(8);
        let server = LiveValueSession::new(2, server_frames);
        let SchemaValue::Stream(imported_outer) = server.decode(encoded_outer).await.unwrap()
        else {
            panic!("expected imported outer stream");
        };
        let receiver = imported_outer
            .take_host_endpoint::<RelayReceiver>()
            .unwrap();
        receiver.demand_tx.send(()).await.unwrap();
        let Some(live_invocation_frame::Frame::Demand(demand)) =
            server_frame_rx.recv().await.unwrap().frame
        else {
            panic!("expected outer demand");
        };
        client
            .route_stream_frame(live_invocation_frame::Frame::Demand(demand))
            .await
            .unwrap();
        assert_eq!(outer_source.demand_rx.recv().await, Some(()));

        outer_source
            .item_tx
            .send(Ok(SchemaValue::Stream(nested_stream)))
            .await
            .unwrap();
        let racing_item = client_frame_rx.recv().await.unwrap();

        drop(receiver);
        let Some(live_invocation_frame::Frame::Detach(detach)) =
            server_frame_rx.recv().await.unwrap().frame
        else {
            panic!("expected outer detach");
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), server.wait_idle())
                .await
                .is_err(),
            "server idle must wait for the response already outstanding when detach was sent"
        );
        client
            .route_stream_frame(live_invocation_frame::Frame::Detach(detach))
            .await
            .unwrap();

        server
            .route_stream_frame(racing_item.frame.unwrap())
            .await
            .unwrap();
        let Some(live_invocation_frame::Frame::Detach(nested_detach)) =
            server_frame_rx.recv().await.unwrap().frame
        else {
            panic!("expected nested detach");
        };
        client
            .route_stream_frame(live_invocation_frame::Frame::Detach(nested_detach))
            .await
            .unwrap();
        server.wait_idle().await;

        let result = client.finish_invocation().await;
        assert!(
            result.is_ok(),
            "the detached racing item must not leave its nested source attached: {result:?}"
        );
        tracker.wait_for_sources().await;
    }

    #[test]
    #[timeout("2s")]
    async fn detached_imported_item_must_rollback_undelivered_nested_streams() {
        let (frames, mut frame_rx) = mpsc::channel(1);
        let session = LiveValueSession::new(1, frames.clone());
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        receiver.demand_tx.send(()).await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));

        frames
            .send(LiveInvocationFrame { frame: None })
            .await
            .unwrap();
        drop(receiver);
        tokio::task::yield_now().await;

        let route_session = session.clone();
        let routing = tokio::spawn(async move {
            route_session
                .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                    stream_id: 2,
                    value: Some(stream_reference(4)),
                }))
                .await
        });
        let finish_session = session.clone();
        let finishing = tokio::spawn(async move { finish_session.finish_invocation().await });
        tokio::task::yield_now().await;
        assert!(
            !finishing.is_finished(),
            "invocation completion must wait for accepted detach frames"
        );

        frame_rx.recv().await.unwrap();
        let mut detached = HashSet::new();
        for _ in 0..2 {
            let frame = frame_rx
                .recv()
                .await
                .expect("both outer and nested detach frames must be sent")
                .frame
                .expect("detach frame must have a payload");
            let live_invocation_frame::Frame::Detach(detach) = frame else {
                panic!("expected detach frame, got {frame:?}");
            };
            detached.insert(detach.stream_id);
        }
        routing
            .await
            .unwrap()
            .expect("one response racing a valid detach should be accepted");
        assert_eq!(detached, HashSet::from([2, 4]));

        let result = finishing.await.unwrap();
        assert!(
            result.is_ok(),
            "a nested stream in an item that could not be delivered to the detached reader must be rolled back: {result:?}"
        );
    }

    #[test]
    #[timeout("2s")]
    async fn failed_imported_item_delivery_rolls_back_nested_and_detaches_outer() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        receiver.demand_tx.send(()).await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));
        receiver.item_rx.close();

        session
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(stream_reference(4)),
            }))
            .await
            .unwrap();

        let mut detached = HashSet::new();
        for _ in 0..2 {
            let frame = frame_rx.recv().await.unwrap().frame.unwrap();
            let live_invocation_frame::Frame::Detach(detach) = frame else {
                panic!("expected detach frame, got {frame:?}");
            };
            detached.insert(detach.stream_id);
        }
        assert_eq!(detached, HashSet::from([2, 4]));
        session.wait_idle().await;
        session.finish_invocation().await.unwrap();
    }

    #[test]
    #[timeout("2s")]
    async fn cancellation_discharges_a_blocked_nested_rollback() {
        let (frames, mut frame_rx) = mpsc::channel(1);
        let session = LiveValueSession::new(1, frames.clone());
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        receiver.demand_tx.send(()).await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));
        frames
            .send(LiveInvocationFrame { frame: None })
            .await
            .unwrap();
        drop(receiver);
        tokio::task::yield_now().await;

        let route_session = session.clone();
        let routing = tokio::spawn(async move {
            route_session
                .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                    stream_id: 2,
                    value: Some(stream_reference(4)),
                }))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if session
                    .inner
                    .seen_remote_stream_ids
                    .lock()
                    .expect("live stream ID set mutex poisoned")
                    .contains(&4)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the nested rollback should block on its detach frame");
        session.cancel();

        routing.await.unwrap().unwrap();
        session.wait_idle().await;
    }

    #[test]
    async fn detached_imported_item_still_validates_nested_stream_ids() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        receiver.demand_tx.send(()).await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: 2
            }))
        ));

        drop(receiver);
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: 2
            }))
        ));

        let result = session
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(stream_reference(3)),
            }))
            .await;

        assert!(
            result.is_err(),
            "wrong-parity nested stream IDs must remain invalid when an item races detach"
        );
        session.wait_idle().await;
    }

    #[test]
    async fn dropping_an_untransferred_stream_releases_its_tracker() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let tracker = Arc::new(LiveStreamTracker::new(cancellation));
        let (_source, stream) = relay_endpoint_pair(Some(tracker.clone()));
        assert_eq!(tracker.active.load(Ordering::Acquire), 1);

        drop(stream);

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tracker.wait_for_sources(),
        )
        .await
        .expect("dropping an untransferred stream must release its source tracker");
    }

    #[test]
    async fn dropping_a_store_resource_table_releases_its_stream_handle() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let tracker = Arc::new(LiveStreamTracker::new(cancellation));
        let (_source, stream) = relay_endpoint_pair(Some(tracker.clone()));
        let mut table = wasmtime::component::ResourceTable::new();
        table
            .push(SchemaValueStreamHandleRep::new(stream))
            .expect("the stream handle should fit in a new resource table");
        assert_eq!(tracker.active.load(Ordering::Acquire), 1);

        drop(table);

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tracker.wait_for_sources(),
        )
        .await
        .expect("discarding a Store must release stream handles still in its resource table");
    }

    #[test]
    async fn dropping_an_exported_source_after_demand_sends_end() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let id = stream_id(&session.encode(&SchemaValue::Stream(stream)).unwrap());
        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));

        drop(source.item_tx);

        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::End(LiveStreamEnd { stream_id }))
                if stream_id == id
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("dropping an exported source must terminate its relay");
    }

    #[test]
    async fn invocation_cancellation_releases_all_attached_sources() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let tracker = Arc::new(LiveStreamTracker::new(cancellation.clone()));
        let (_source, stream) = relay_endpoint_pair(Some(tracker.clone()));
        assert_eq!(tracker.active.load(Ordering::Acquire), 1);

        cancellation.cancel();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tracker.wait_for_sources(),
        )
        .await
        .unwrap();
        assert_eq!(tracker.active.load(Ordering::Acquire), 0);

        drop(stream);
        assert_eq!(tracker.active.load(Ordering::Acquire), 0);
    }

    #[test]
    async fn cancellation_releases_exported_stream_blocked_on_an_item() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, _frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let stream_id = stream_id(&session.encode(&SchemaValue::Stream(stream)).unwrap());
        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));

        session.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("cancellation must release a producer blocked waiting for its next item");
    }

    #[test]
    async fn cancellation_releases_exported_stream_blocked_sending_an_item() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, _frame_rx) = mpsc::channel(1);
        frames
            .send(LiveInvocationFrame { frame: None })
            .await
            .unwrap();
        let session = LiveValueSession::new(1, frames);
        let stream_id = stream_id(&session.encode(&SchemaValue::Stream(stream)).unwrap());
        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));
        source.item_tx.send(Ok(SchemaValue::U32(42))).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while source.item_tx.capacity() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the exported relay should consume the item before blocking on the frame channel");

        session.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("cancellation must release an exported stream blocked sending an item");
    }

    #[test]
    async fn detach_discards_nested_streams_from_an_undelivered_item() {
        let (mut outer_source, outer_stream) = relay_endpoint_pair(None);
        let (mut nested_source, nested_stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(1);
        frames
            .send(LiveInvocationFrame { frame: None })
            .await
            .unwrap();
        let session = LiveValueSession::new(1, frames);
        let outer_id = stream_id(&session.encode(&SchemaValue::Stream(outer_stream)).unwrap());
        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id: outer_id,
            }))
            .await
            .unwrap();
        assert_eq!(outer_source.demand_rx.recv().await, Some(()));
        outer_source
            .item_tx
            .send(Ok(SchemaValue::Stream(nested_stream)))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while outer_source.item_tx.capacity() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the outer relay should consume the nested item before blocking");

        session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id: outer_id,
            }))
            .await
            .unwrap();

        frame_rx.recv().await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::End(LiveStreamEnd {
                stream_id
            })) if stream_id == outer_id
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("detach must release the blocked outer and undisclosed nested relays");
        assert_eq!(nested_source.demand_rx.recv().await, None);
    }

    #[test]
    async fn detach_racing_a_blocked_terminal_frame_is_accepted() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(1);
        frames
            .send(LiveInvocationFrame { frame: None })
            .await
            .unwrap();
        let session = LiveValueSession::new(1, frames);
        let stream_id = stream_id(&session.encode(&SchemaValue::Stream(stream)).unwrap());
        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));

        drop(source.item_tx);
        tokio::task::yield_now().await;
        assert!(
            session
                .inner
                .exported
                .lock()
                .expect("live stream map mutex poisoned")
                .contains_key(&stream_id),
            "the exported route must remain addressable while its terminal frame is blocked"
        );

        let result = session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id,
            }))
            .await;

        assert!(
            result.is_ok(),
            "detach is valid until the peer can observe the terminal frame: {result:?}"
        );
        frame_rx.recv().await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::End(LiveStreamEnd {
                stream_id: id
            })) if id == stream_id
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("detach must interrupt a blocked terminal frame send");
    }

    #[test]
    async fn detach_must_interrupt_blocked_overlapping_demand_error() {
        let (mut source, stream) = relay_endpoint_pair(None);
        let (frames, mut frame_rx) = mpsc::channel(1);
        frames
            .send(LiveInvocationFrame { frame: None })
            .await
            .unwrap();
        let session = LiveValueSession::new(1, frames);
        let stream_id = stream_id(&session.encode(&SchemaValue::Stream(stream)).unwrap());
        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id,
            }))
            .await
            .unwrap();
        assert_eq!(source.demand_rx.recv().await, Some(()));
        source.item_tx.send(Ok(SchemaValue::U32(42))).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while source.item_tx.capacity() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the exported relay should block while sending the item frame");

        session
            .route_stream_frame(live_invocation_frame::Frame::Demand(LiveStreamDemand {
                stream_id,
            }))
            .await
            .unwrap();
        session
            .route_stream_frame(live_invocation_frame::Frame::Detach(LiveStreamDetach {
                stream_id,
            }))
            .await
            .unwrap();

        frame_rx.recv().await.unwrap();
        assert!(matches!(
            frame_rx.recv().await.unwrap().frame,
            Some(live_invocation_frame::Frame::End(LiveStreamEnd { stream_id: id }))
                | Some(live_invocation_frame::Frame::StreamError(LiveStreamError {
                    stream_id: id,
                    ..
                })) if id == stream_id
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("detach must interrupt the blocked overlapping-demand error frame");
    }

    #[test]
    async fn cancellation_releases_imported_stream_with_buffered_item() {
        let (frames, mut frame_rx) = mpsc::channel(4);
        let session = LiveValueSession::new(1, frames);
        let SchemaValue::Stream(stream) = session.decode(stream_reference(2)).await.unwrap() else {
            panic!("expected stream");
        };
        let mut receiver = stream.take_host_endpoint::<RelayReceiver>().unwrap();
        let mut next = Box::pin(receiver.next());

        let frame = tokio::select! {
            frame = frame_rx.recv() => frame.unwrap().frame.unwrap(),
            result = &mut next => panic!("read completed before receiving an item: {result:?}"),
        };
        assert!(matches!(
            frame,
            live_invocation_frame::Frame::Demand(LiveStreamDemand { stream_id: 2 })
        ));
        drop(next);

        session
            .route_stream_frame(live_invocation_frame::Frame::Item(LiveStreamItem {
                stream_id: 2,
                value: Some(SchemaValue::U32(42).try_into().unwrap()),
            }))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while receiver.item_rx.len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the delivered item should become buffered");

        session.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(1), session.wait_idle())
            .await
            .expect("cancellation must release an imported stream with a buffered item");
    }
}
