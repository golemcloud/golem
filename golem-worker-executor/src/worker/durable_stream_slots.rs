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

use super::*;
use crate::durable_host::durable_stream::{
    CommittedProducerStreamEventPayload, ExternalAppendOutcome, ExternalProducer,
    StreamHandleReadResult, StreamStoreError,
};
use golem_api_grpc::proto::golem::schema::{SchemaValue as ProtoValue, schema_value};
use golem_common::model::durable_stream::{
    DurableStreamHandle, DurableStreamReadRequest, ExternalProducerId, StreamHandleReadRequest,
    StreamItemsPayload, StreamOffset, StreamSessionKey,
};
use golem_common::model::invocation_session_public::validate_durable_stream_session_id;
use golem_common::schema::{
    AgentMethodSchema, FieldSource, OutputSchema, SchemaGraph, SchemaType, SchemaValue,
};
use golem_schema::schema::fingerprint::schema_fingerprint_v1;
use golem_schema::schema::validation::validate_value;
use prost::Message;

/// Domain result of creating or replay-attaching a stream session.
pub(crate) struct CreateStreamSessionResult {
    pub(crate) session: String,
    pub(crate) replayed: bool,
    pub(crate) component_revision: ComponentRevision,
}

/// Domain request for reading one invocation stream slot.
pub(crate) struct ReadStreamSlotRequest {
    pub(crate) session: String,
    pub(crate) slot: String,
    pub(crate) from_offset: Option<StreamOffset>,
    pub(crate) max_items: u32,
    pub(crate) max_bytes: u64,
    pub(crate) wait_millis: u64,
    pub(crate) expected_method: String,
}

/// One domain item returned by a stream-slot read.
pub(crate) struct StreamSlotItem {
    pub(crate) offset: StreamOffset,
    pub(crate) content: StreamSlotItemContent,
}

/// Encoding selected by the slot's pinned element schema.
pub(crate) enum StreamSlotItemContent {
    Value(Vec<u8>),
    PackedU8(Vec<u8>),
}

/// Domain result of reading one invocation stream slot.
pub(crate) struct ReadStreamSlotResult {
    pub(crate) items: Vec<StreamSlotItem>,
    pub(crate) next_offset: Option<StreamOffset>,
    pub(crate) closed: bool,
    pub(crate) cancelled: bool,
    pub(crate) element_schema: SchemaGraph,
    pub(crate) content_type: &'static str,
    pub(crate) up_to_date: bool,
    pub(crate) head_offset: Option<StreamOffset>,
    pub(crate) stream_identity: String,
    pub(crate) slots: Vec<String>,
    pub(crate) tombstoned: bool,
    pub(crate) writable: bool,
}

/// Domain target for cancelling a session or tombstoning one export slot.
pub(crate) struct ExportStreamControlRequest {
    pub(crate) session: String,
    pub(crate) slot: Option<String>,
    pub(crate) expected_method: String,
}

/// Stable outcome of an export stream control operation.
pub(crate) enum ExportStreamControlResult {
    Applied,
    NotFound,
    Gone,
}

/// Domain payload accepted by an input stream slot.
pub(crate) enum AppendStreamSlotPayload {
    Values(Vec<Vec<u8>>),
    PackedU8(Vec<u8>),
}

/// Client producer coordinates for idempotent external appends.
pub(crate) struct StreamSlotProducer {
    pub(crate) id: String,
    pub(crate) epoch: u64,
    pub(crate) sequence: u64,
}

/// Domain request for appending to one input stream slot.
pub(crate) struct AppendToStreamSlotRequest {
    pub(crate) session: String,
    pub(crate) slot: String,
    pub(crate) payload: Option<AppendStreamSlotPayload>,
    pub(crate) close: bool,
    pub(crate) producer: Option<StreamSlotProducer>,
    pub(crate) expected_method: String,
}

/// Domain outcome of an input stream-slot append.
pub(crate) enum AppendToStreamSlotResult {
    Accepted(StreamOffset),
    Duplicate {
        offset: StreamOffset,
        highest_sequence: Option<u64>,
    },
    EpochFenced(u64),
    SequenceGap {
        expected: u64,
        received: u64,
    },
    Closed,
    NotFound,
    Gone,
    ReadOnly,
}

struct Slot {
    session: StreamSessionKey,
    name: String,
    slots: Vec<String>,
    graph: SchemaGraph,
    writable: bool,
    bytes: bool,
    source: SlotSource,
}

enum SlotSource {
    Stream(DurableStreamHandle),
    Value {
        encoded: Vec<u8>,
        offset: StreamOffset,
    },
    Pending {
        finished: bool,
    },
    Tombstoned,
}

#[derive(Debug, PartialEq)]
struct SlotSchema {
    element: SchemaType,
    field_index: Option<usize>,
    writable: bool,
    is_stream: bool,
}

fn append_error(error: StreamStoreError) -> WorkerExecutorError {
    match error {
        StreamStoreError::InvalidValueBatch
        | StreamStoreError::ItemTooLarge
        | StreamStoreError::InvalidPackedU8Batch
        | StreamStoreError::InvalidHandle
        | StreamStoreError::UnknownStream(_) => {
            WorkerExecutorError::invalid_request(error.to_string())
        }
        _ => WorkerExecutorError::runtime(error.to_string()),
    }
}

impl SlotSchema {
    /// Looks up a canonical input or output slot in the method's pinned schema.
    fn lookup(
        graph: &SchemaGraph,
        method: &AgentMethodSchema,
        name: &str,
    ) -> Result<Option<Self>, WorkerExecutorError> {
        let resolve = |ty| {
            graph
                .resolve_ref(ty)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
        };
        for (index, field) in method
            .input_schema
            .fields()
            .iter()
            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
            .enumerate()
        {
            if field.name == name
                && let SchemaType::Stream {
                    inner: Some(element),
                    ..
                } = resolve(&field.schema)?
            {
                return Ok(Some(Self {
                    element: (**element).clone(),
                    field_index: Some(index),
                    writable: true,
                    is_stream: true,
                }));
            }
        }
        let OutputSchema::Single(output) = &method.output_schema else {
            return Ok(None);
        };
        match resolve(output)? {
            SchemaType::Stream {
                inner: Some(element),
                ..
            } if name == "$result" => Ok(Some(Self {
                element: (**element).clone(),
                field_index: None,
                writable: false,
                is_stream: true,
            })),
            SchemaType::Record { fields, .. } => {
                if let Some((index, field)) = fields
                    .iter()
                    .enumerate()
                    .find(|(_, field)| field.name == name)
                    && let SchemaType::Stream {
                        inner: Some(element),
                        ..
                    } = resolve(&field.body)?
                {
                    return Ok(Some(Self {
                        element: (**element).clone(),
                        field_index: Some(index),
                        writable: false,
                        is_stream: true,
                    }));
                }
                if name == "$result"
                    && !golem_common::schema::agent::contains_stream_in_graph(graph, output)
                {
                    Ok(Some(Self {
                        element: (**output).clone(),
                        field_index: None,
                        writable: false,
                        is_stream: false,
                    }))
                } else {
                    Ok(None)
                }
            }
            _ if name == "$result"
                && !golem_common::schema::agent::contains_stream_in_graph(graph, output) =>
            {
                Ok(Some(Self {
                    element: (**output).clone(),
                    field_index: None,
                    writable: false,
                    is_stream: false,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Extracts this slot's canonical durable handle from a persisted value.
    fn extract_handle(
        &self,
        encoded: &[u8],
        handles: &[DurableStreamHandle],
    ) -> Result<DurableStreamHandle, WorkerExecutorError> {
        let mut value = ProtoValue::decode(encoded)
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        if let Some(index) = self.field_index {
            let Some(schema_value::Value::RecordValue(record)) = value.value else {
                return Err(WorkerExecutorError::runtime(
                    "persisted slot value is not a record",
                ));
            };
            value =
                record.fields.get(index).cloned().ok_or_else(|| {
                    WorkerExecutorError::runtime("persisted slot field is missing")
                })?;
        }
        let Some(schema_value::Value::StreamReference(reference)) = value.value else {
            return Err(WorkerExecutorError::runtime(
                "persisted slot is not a stream reference",
            ));
        };
        usize::try_from(reference.stream_id)
            .ok()
            .and_then(|index| handles.get(index))
            .cloned()
            .ok_or_else(|| WorkerExecutorError::runtime("persisted slot has no durable handle"))
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Resolves the pinned revision for a stream-slot session before transport decoding.
    pub(crate) async fn stream_session_revision(
        &self,
        key: &IdempotencyKey,
    ) -> Result<ComponentRevision, WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let pinned = producer
            .with_metadata_activity(self.prepared_stream_session(key))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??;
        if pinned.is_none() && self.agent_mode() != AgentMode::Ephemeral {
            Self::ensure_not_failed(
                &self.deps,
                &self.owned_agent_id,
                self.agent_mode(),
                self.get_last_known_status().await.as_ref(),
            )
            .await?;
        }
        Ok(match pinned {
            Some(prepared) => prepared.attempt.invocation.target_component_revision,
            None => self.get_last_known_status().await.component_revision,
        })
    }

    /// Accepts a fully domain-built streaming invocation and returns its stable session identity.
    pub(crate) async fn create_stream_session(
        self: &Arc<Self>,
        request: DurableStreamingInvocationRequest,
    ) -> Result<CreateStreamSessionResult, WorkerExecutorError> {
        let session = request.attempt.session_key.idempotency_key.value.clone();
        let component_revision = request.attempt.invocation.target_component_revision;
        let acceptance = self.accept_durable_stream_slot_invocation(request).await?;
        Ok(CreateStreamSessionResult {
            session,
            replayed: acceptance.replayed,
            component_revision,
        })
    }

    async fn resolve_stream_slot(
        &self,
        session: &str,
        name: &str,
        expected_method: Option<&str>,
    ) -> Result<Option<Slot>, WorkerExecutorError> {
        validate_durable_stream_session_id(session)
            .map_err(WorkerExecutorError::invalid_request)?;
        let Some(status) = self
            .durable_stream_session_status(&IdempotencyKey::new(session.to_string()))
            .await?
        else {
            return Ok(None);
        };
        let Some(prepared_index) = status.first_prepared else {
            return Ok(None);
        };
        let StreamSessionRecord::Prepared(prepared) =
            self.read_stream_session_record(prepared_index).await?
        else {
            return Err(WorkerExecutorError::runtime(
                "session prepared locator is invalid",
            ));
        };
        let descriptor = &prepared.attempt.invocation;
        if expected_method.is_some_and(|expected| expected != descriptor.method_name) {
            return Ok(None);
        }
        let component = self
            .component_service()
            .get_metadata(
                self.component_id(),
                Some(descriptor.target_component_revision),
            )
            .await?;
        let parsed = ParsedAgentId::parse(&self.agent_id().agent_id, &component.metadata)
            .map_err(WorkerExecutorError::invalid_request)?;
        let agent = component
            .metadata
            .find_agent_type_by_name_ref(&parsed.agent_type)
            .ok_or_else(|| WorkerExecutorError::runtime("persisted agent type is missing"))?;
        let method = agent
            .methods
            .iter()
            .find(|method| method.name == descriptor.method_name)
            .ok_or_else(|| WorkerExecutorError::runtime("persisted agent method is missing"))?;
        let mut candidates = method
            .input_schema
            .fields()
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        candidates.push("$result".into());
        if let OutputSchema::Single(output) = &method.output_schema
            && let SchemaType::Record { fields, .. } = agent
                .schema
                .resolve_ref(output)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        {
            candidates.extend(fields.iter().map(|field| field.name.clone()));
        }
        let mut slots = Vec::new();
        for candidate in candidates {
            if !slots.contains(&candidate)
                && SlotSchema::lookup(&agent.schema, method, &candidate)?.is_some()
            {
                slots.push(candidate);
            }
        }
        let name = if name.is_empty() {
            let Some(name) = slots.first() else {
                return Ok(None);
            };
            name.as_str()
        } else {
            name
        };
        let Some(schema) = SlotSchema::lookup(&agent.schema, method, name)? else {
            return Ok(None);
        };
        let source = if status.tombstoned_slots.contains(name) {
            SlotSource::Tombstoned
        } else if schema.writable {
            SlotSource::Stream(
                schema.extract_handle(&descriptor.invocation_value, &descriptor.stream_handles)?,
            )
        } else if let Some(result_index) = status.invocation_result {
            let StreamSessionRecord::InvocationResult(result) =
                self.read_stream_session_record(result_index).await?
            else {
                return Err(WorkerExecutorError::runtime(
                    "session result locator is invalid",
                ));
            };
            if schema.is_stream {
                SlotSource::Stream(schema.extract_handle(&result.result, &result.output_streams)?)
            } else {
                SlotSource::Value {
                    encoded: result.result,
                    offset: StreamOffset::new(result_index, 0),
                }
            }
        } else {
            SlotSource::Pending {
                finished: status.finished.is_some()
                    || (schema.is_stream && status.cancellation_requested),
            }
        };
        if let SlotSource::Stream(handle) = &source {
            let fingerprint = schema_fingerprint_v1(&agent.schema, Some(&schema.element))
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            if fingerprint != handle.element_schema_fingerprint {
                return Err(WorkerExecutorError::runtime(
                    "slot schema does not match durable handle",
                ));
            }
        }
        let bytes = matches!(
            agent
                .schema
                .resolve_ref(&schema.element)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?,
            SchemaType::U8 { .. }
        ) && schema.is_stream;
        Ok(Some(Slot {
            session: prepared.attempt.session_key,
            name: name.to_owned(),
            slots,
            graph: SchemaGraph {
                defs: agent.schema.defs.clone(),
                root: schema.element,
            },
            writable: schema.writable,
            bytes,
            source,
        }))
    }

    /// Reads data and metadata from a slot resolved against the session's pinned schema.
    pub(crate) async fn read_stream_slot(
        &self,
        request: ReadStreamSlotRequest,
    ) -> Result<Option<ReadStreamSlotResult>, DurableStreamReadError<WorkerExecutorError>> {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(request.wait_millis.min(30_000));
        let after = request.from_offset;
        let producer = self.load_durable_stream_producer().await.map_err(|error| {
            DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        let slot = loop {
            let notified = producer.session_records_changed().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let Some(slot) = producer
                .with_metadata_activity(self.resolve_stream_slot(
                    &request.session,
                    &request.slot,
                    Some(&request.expected_method),
                ))
                .await
                .map_err(|error| {
                    DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
                })??
            else {
                return Ok(None);
            };
            if !matches!(slot.source, SlotSource::Pending { finished: false })
                || request.max_items == 0
                || tokio::time::Instant::now() >= deadline
            {
                break slot;
            }
            let _ = tokio::time::timeout_at(deadline, notified).await;
        };
        let stream_id = match &slot.source {
            SlotSource::Stream(handle) => Some(handle.stream_id),
            SlotSource::Value { .. } | SlotSource::Pending { .. } | SlotSource::Tombstoned => None,
        };
        let identity = golem_common::serialization::serialize(&(
            slot.session.clone(),
            slot.name.clone(),
            stream_id,
        ))
        .map_err(WorkerExecutorError::runtime)?;
        let mut response = ReadStreamSlotResult {
            items: Vec::new(),
            next_offset: after,
            closed: false,
            cancelled: false,
            element_schema: slot.graph,
            content_type: if slot.bytes {
                "application/octet-stream"
            } else {
                "application/json"
            },
            up_to_date: true,
            head_offset: None,
            stream_identity: blake3::hash(&identity).to_hex().to_string(),
            slots: slot.slots,
            tombstoned: matches!(slot.source, SlotSource::Tombstoned),
            writable: slot.writable,
        };
        match slot.source {
            SlotSource::Stream(handle) => {
                let read = StreamHandleReadRequest {
                    handle,
                    after,
                    max_items: request.max_items,
                    max_bytes: request.max_bytes,
                    wait_millis: deadline
                        .saturating_duration_since(tokio::time::Instant::now())
                        .as_millis() as u64,
                };
                let bytes = self
                    .rpc()
                    .read_durable_stream_segment(
                        DurableStreamReadRequest::AuthorizedExport(Box::new(read)),
                        &AuthCtx::System,
                    )
                    .await
                    .map_err(|error| {
                        error.map_other(|error| WorkerExecutorError::runtime(error.to_string()))
                    })?;
                let read: StreamHandleReadResult = golem_common::serialization::deserialize(&bytes)
                    .map_err(WorkerExecutorError::runtime)?;
                response.next_offset = read.next_offset;
                response.head_offset = read.head_offset;
                response.closed = read.closed;
                response.cancelled = read.cancelled;
                response.up_to_date = read.next_offset >= read.head_offset;
                for event in read.events {
                    let content = match event.payload {
                        CommittedProducerStreamEventPayload::Value(value) => {
                            Some(StreamSlotItemContent::Value(value))
                        }
                        CommittedProducerStreamEventPayload::PackedU8(byte) => {
                            Some(StreamSlotItemContent::PackedU8(vec![byte]))
                        }
                        _ => None,
                    };
                    if let Some(content) = content {
                        response.items.push(StreamSlotItem {
                            offset: event.offset,
                            content,
                        });
                    }
                }
            }
            SlotSource::Value {
                encoded: value,
                offset,
            } => {
                response.head_offset = Some(offset);
                response.closed = true;
                if request.max_items > 0 && after.is_none_or(|after| after < offset) {
                    if value.len() as u64 > request.max_bytes {
                        return Err(WorkerExecutorError::invalid_request(
                            "slot value exceeds read byte limit",
                        )
                        .into());
                    }
                    response.next_offset = Some(offset);
                    response.items.push(StreamSlotItem {
                        offset,
                        content: StreamSlotItemContent::Value(value),
                    });
                }
                response.up_to_date = response.next_offset >= response.head_offset;
            }
            SlotSource::Pending { finished } => {
                response.closed = finished;
                response.cancelled = finished;
            }
            SlotSource::Tombstoned => {}
        }
        producer.ensure_healthy().map_err(|error| {
            DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        Ok(Some(response))
    }

    /// Cancels all session streams or tombstones one canonical export slot.
    pub(crate) async fn control_export_stream(
        self: &Arc<Self>,
        request: ExportStreamControlRequest,
    ) -> Result<ExportStreamControlResult, WorkerExecutorError> {
        validate_durable_stream_session_id(&request.session)
            .map_err(WorkerExecutorError::invalid_request)?;
        if request.expected_method.is_empty()
            || request
                .slot
                .as_ref()
                .is_some_and(|slot| slot.is_empty() || slot.starts_with("__ds"))
        {
            return Err(WorkerExecutorError::invalid_request(
                "invalid export stream control target",
            ));
        }
        let Some(prepared) = self
            .prepared_stream_session(&IdempotencyKey::new(request.session.clone()))
            .await?
        else {
            return Ok(ExportStreamControlResult::NotFound);
        };
        if prepared.attempt.invocation.method_name != request.expected_method {
            return Ok(ExportStreamControlResult::NotFound);
        }
        let producer = self.durable_stream_producer().await?;
        let streams = StreamSession::new(
            producer.clone(),
            self.oplog.clone(),
            prepared.attempt.session_key.clone(),
            prepared.stream_mappings.iter().cloned(),
        )
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        if let Some(name) = request.slot {
            let worker = self.clone();
            producer
                .run_admitted(None, 0, true, move |owner, admission| async move {
                    let lock = owner.session_lock(&prepared.attempt.session_key);
                    let guard = lock.lock_owned().await;
                    let Some(slot) = worker
                        .resolve_stream_slot(
                            &request.session,
                            &name,
                            Some(&request.expected_method),
                        )
                        .await
                        .map_err(|error| error.to_string())?
                    else {
                        return Ok::<_, String>(ExportStreamControlResult::NotFound);
                    };
                    let stream = match slot.source {
                        SlotSource::Tombstoned => return Ok(ExportStreamControlResult::Gone),
                        SlotSource::Stream(handle) => Some((
                            handle,
                            if slot.writable {
                                SessionStreamRole::Input
                            } else {
                                SessionStreamRole::Output
                            },
                        )),
                        SlotSource::Value { .. } | SlotSource::Pending { .. } => None,
                    };
                    let applied = streams
                        .tombstone_slot_owned(&admission, slot.name, stream, guard)
                        .await?;
                    Ok(if applied {
                        ExportStreamControlResult::Applied
                    } else {
                        ExportStreamControlResult::Gone
                    })
                })
                .await
                .map_err(WorkerExecutorError::runtime)
        } else {
            streams
                .cancel_session_streams()
                .await
                .map(|exists| {
                    if exists {
                        ExportStreamControlResult::Applied
                    } else {
                        ExportStreamControlResult::NotFound
                    }
                })
                .map_err(WorkerExecutorError::runtime)
        }
    }

    /// Validates and durably appends a batch to one writable stream slot.
    pub(crate) async fn append_to_stream_slot(
        &self,
        request: AppendToStreamSlotRequest,
    ) -> Result<AppendToStreamSlotResult, WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let Some(slot) = producer
            .with_metadata_activity(self.resolve_stream_slot(
                &request.session,
                &request.slot,
                Some(&request.expected_method),
            ))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??
        else {
            return Ok(AppendToStreamSlotResult::NotFound);
        };
        if matches!(slot.source, SlotSource::Tombstoned) {
            return Ok(AppendToStreamSlotResult::Gone);
        }
        if !slot.writable {
            return Ok(AppendToStreamSlotResult::ReadOnly);
        }
        let SlotSource::Stream(handle) = slot.source else {
            return Err(WorkerExecutorError::runtime("input slot has no stream"));
        };
        let payload = match request.payload {
            Some(AppendStreamSlotPayload::PackedU8(bytes)) if slot.bytes => {
                Some(StreamItemsPayload::PackedU8(bytes))
            }
            Some(AppendStreamSlotPayload::Values(values)) if !slot.bytes => {
                for encoded in &values {
                    let proto = ProtoValue::decode(encoded.as_slice())
                        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
                    let value: SchemaValue = proto
                        .try_into()
                        .map_err(WorkerExecutorError::invalid_request)?;
                    validate_value(&slot.graph, &slot.graph.root, &value).map_err(|errors| {
                        WorkerExecutorError::invalid_request(format!(
                            "invalid stream item: {errors:?}"
                        ))
                    })?;
                }
                Some(StreamItemsPayload::Values(values))
            }
            None => None,
            _ => {
                return Err(WorkerExecutorError::invalid_request(
                    "payload does not match stream content type",
                ));
            }
        };
        producer
            .validate_handle(&handle)
            .await
            .map_err(append_error)?;
        let result = producer
            .append_external_input(
                None,
                &slot.session,
                handle.stream_id,
                payload,
                request.close,
                request.producer.map(|producer| ExternalProducer {
                    id: ExternalProducerId::Client(producer.id),
                    epoch: producer.epoch,
                    sequence: producer.sequence,
                }),
            )
            .await
            .map_err(append_error)?;
        Ok(match result {
            ExternalAppendOutcome::Accepted(offset) => AppendToStreamSlotResult::Accepted(offset),
            ExternalAppendOutcome::Duplicate {
                offset,
                highest_sequence,
            } => AppendToStreamSlotResult::Duplicate {
                offset,
                highest_sequence,
            },
            ExternalAppendOutcome::EpochFenced(current_epoch) => {
                AppendToStreamSlotResult::EpochFenced(current_epoch)
            }
            ExternalAppendOutcome::SeqGap { expected, received } => {
                AppendToStreamSlotResult::SequenceGap { expected, received }
            }
            ExternalAppendOutcome::Closed => AppendToStreamSlotResult::Closed,
            ExternalAppendOutcome::NotFound => AppendToStreamSlotResult::NotFound,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::{InputSchema, NamedField, NamedFieldType};
    use test_r::test;

    #[test]
    fn named_slots_preserve_field_indices_and_scalar_results() {
        let graph = SchemaGraph::empty();
        let mut method = AgentMethodSchema {
            name: "run".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![
                NamedField::user_supplied("count", SchemaType::u32()),
                NamedField::user_supplied("input", SchemaType::stream(Some(SchemaType::string()))),
            ]),
            output_schema: OutputSchema::Single(Box::new(SchemaType::record(vec![
                NamedFieldType {
                    name: "numbers".into(),
                    body: SchemaType::stream(Some(SchemaType::u64())),
                    metadata: Default::default(),
                },
                NamedFieldType {
                    name: "bytes".into(),
                    body: SchemaType::stream(Some(SchemaType::u8())),
                    metadata: Default::default(),
                },
            ]))),
            http_endpoint: vec![],
            read_only: None,
        };
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "input").unwrap(),
            Some(SlotSchema {
                element: SchemaType::string(),
                field_index: Some(1),
                writable: true,
                is_stream: true
            })
        );
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "bytes").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(1),
                writable: false,
                is_stream: true
            })
        );
        method.input_schema = InputSchema::parameters(vec![NamedField::user_supplied(
            "bytes",
            SchemaType::string(),
        )]);
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "bytes").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(1),
                writable: false,
                is_stream: true
            })
        );
        assert_eq!(SlotSchema::lookup(&graph, &method, "count").unwrap(), None);
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "$result").unwrap(),
            None
        );
        method.output_schema = OutputSchema::Single(Box::new(SchemaType::u64()));
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "$result").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u64(),
                field_index: None,
                writable: false,
                is_stream: false
            })
        );
        method.output_schema =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8()))));
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "$result").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u8(),
                field_index: None,
                writable: false,
                is_stream: true
            })
        );
    }

    #[test]
    fn canonical_slot_reference_is_not_a_transport_id_or_field_index() {
        use golem_common::model::durable_stream::{StreamId, StreamInvocationId};
        let id = AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::from_u128(2)),
            agent_id: "source".into(),
        };
        let environment =
            golem_common::base_model::environment::EnvironmentId(uuid::Uuid::from_u128(3));
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(4));
        let source = StreamInvocationId {
            callee_environment_id: environment,
            callee: id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key: IdempotencyKey::new("source-session".into()),
        };
        let first = DurableStreamHandle {
            format_version: 1,
            stream_id: StreamId(uuid::Uuid::from_u128(101)),
            producer_environment_id: environment,
            producer: id,
            expected_producer_fingerprint: fingerprint,
            source_invocation: source,
            component_revision: golem_common::model::component::ComponentRevision::INITIAL,
            element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([0; 32]),
        };
        let mut second = first.clone();
        second.stream_id = StreamId(uuid::Uuid::from_u128(202));
        let reference = ProtoValue {
            value: Some(schema_value::Value::StreamReference(
                golem_api_grpc::proto::golem::schema::SchemaValueStreamReference { stream_id: 1 },
            )),
        };
        let record = ProtoValue {
            value: Some(schema_value::Value::RecordValue(
                golem_api_grpc::proto::golem::schema::RecordValue {
                    fields: vec![reference.clone()],
                },
            )),
        };
        assert_eq!(
            SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(0),
                writable: false,
                is_stream: true
            }
            .extract_handle(&record.encode_to_vec(), &[first.clone(), second.clone()])
            .unwrap(),
            second
        );
        assert_eq!(
            SlotSchema {
                element: SchemaType::u8(),
                field_index: None,
                writable: false,
                is_stream: true
            }
            .extract_handle(&reference.encode_to_vec(), &[first.clone(), second.clone()])
            .unwrap(),
            second
        );
        assert!(
            SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(0),
                writable: false,
                is_stream: true
            }
            .extract_handle(&record.encode_to_vec(), &[first])
            .is_err()
        );
    }
}
