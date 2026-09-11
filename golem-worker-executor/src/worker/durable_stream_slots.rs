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
    CommittedProducerStreamEventPayloadV1, ExternalAppendOutcomeV1, ExternalProducerV1,
    StreamHandleReadResultV1,
};
use golem_api_grpc::proto::golem::schema::{SchemaValue as ProtoValue, schema_value};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AppendAccepted, AppendDuplicate, AppendEpochFenced, AppendSequenceGap,
    AppendToStreamSlotRequest, AppendToStreamSlotResponse, ReadStreamSlotRequest,
    ReadStreamSlotSuccess, StreamSlotItem, append_to_stream_slot_request,
    append_to_stream_slot_response, stream_slot_item,
};
use golem_common::model::durable_stream::{
    DurableStreamHandleV1, DurableStreamReadRequestV1, StreamHandleReadRequestV1,
    StreamItemsPayloadV1, StreamOffsetV1, StreamSessionKeyV1,
};
use golem_common::model::invocation_session_public::validate_durable_stream_session_id;
use golem_common::schema::{
    AgentMethodSchema, FieldSource, OutputSchema, SchemaGraph, SchemaType, SchemaValue,
};
use golem_schema::schema::fingerprint::schema_fingerprint_v1;
use golem_schema::schema::validation::validate_value;
use prost::Message;

struct Slot {
    session: StreamSessionKeyV1,
    graph: SchemaGraph,
    writable: bool,
    bytes: bool,
    source: SlotSource,
}

enum SlotSource {
    Stream(DurableStreamHandleV1),
    Value(Vec<u8>, StreamOffsetV1),
    Pending { finished: bool },
}

type SlotSchema = (SchemaType, Option<usize>, bool, bool);

/// Returns the element type, root record-field index, direction and whether the slot is a stream.
fn slot_schema(
    graph: &SchemaGraph,
    method: &AgentMethodSchema,
    name: &str,
) -> Result<Option<SlotSchema>, WorkerExecutorError> {
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
        if field.name == name {
            return match resolve(&field.schema)? {
                SchemaType::Stream {
                    inner: Some(element),
                    ..
                } => Ok(Some(((**element).clone(), Some(index), true, true))),
                _ => Ok(None),
            };
        }
    }
    let OutputSchema::Single(output) = &method.output_schema else {
        return Ok(None);
    };
    match resolve(output)? {
        SchemaType::Stream {
            inner: Some(element),
            ..
        } if name == "$result" => Ok(Some(((**element).clone(), None, false, true))),
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
                return Ok(Some(((**element).clone(), Some(index), false, true)));
            }
            if name == "$result"
                && !golem_common::schema::agent::contains_stream_in_graph(graph, output)
            {
                Ok(Some(((**output).clone(), None, false, false)))
            } else {
                Ok(None)
            }
        }
        _ if name == "$result"
            && !golem_common::schema::agent::contains_stream_in_graph(graph, output) =>
        {
            Ok(Some(((**output).clone(), None, false, false)))
        }
        _ => Ok(None),
    }
}

fn slot_handle(
    encoded: &[u8],
    field: Option<usize>,
    handles: &[DurableStreamHandleV1],
) -> Result<DurableStreamHandleV1, WorkerExecutorError> {
    let mut value = ProtoValue::decode(encoded)
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    if let Some(index) = field {
        let Some(schema_value::Value::RecordValue(record)) = value.value else {
            return Err(WorkerExecutorError::runtime(
                "persisted slot value is not a record",
            ));
        };
        value = record
            .fields
            .get(index)
            .cloned()
            .ok_or_else(|| WorkerExecutorError::runtime("persisted slot field is missing"))?;
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

impl<Ctx: WorkerCtx> Worker<Ctx> {
    async fn resolve_stream_slot(
        &self,
        session: &str,
        name: &str,
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
        let StreamSessionRecordV1::Prepared(prepared) =
            self.read_stream_session_record(prepared_index).await?
        else {
            return Err(WorkerExecutorError::runtime(
                "session prepared locator is invalid",
            ));
        };
        let descriptor = &prepared.attempt.invocation;
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
        let Some((element, field, writable, is_stream)) = slot_schema(&agent.schema, method, name)?
        else {
            return Ok(None);
        };
        let source = if writable {
            SlotSource::Stream(slot_handle(
                &descriptor.invocation_value,
                field,
                &descriptor.stream_handles,
            )?)
        } else if let Some(result_index) = status.invocation_result {
            let StreamSessionRecordV1::InvocationResult(result) =
                self.read_stream_session_record(result_index).await?
            else {
                return Err(WorkerExecutorError::runtime(
                    "session result locator is invalid",
                ));
            };
            if is_stream {
                SlotSource::Stream(slot_handle(&result.result, field, &result.output_streams)?)
            } else {
                SlotSource::Value(result.result, StreamOffsetV1::new(result_index, 0))
            }
        } else {
            SlotSource::Pending {
                finished: status.finished.is_some(),
            }
        };
        if let SlotSource::Stream(handle) = &source {
            let fingerprint = schema_fingerprint_v1(&agent.schema, Some(&element))
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
                .resolve_ref(&element)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?,
            SchemaType::U8 { .. }
        ) && is_stream;
        Ok(Some(Slot {
            session: prepared.attempt.session_key,
            graph: SchemaGraph {
                defs: agent.schema.defs.clone(),
                root: element,
            },
            writable,
            bytes,
            source,
        }))
    }

    pub(crate) async fn read_stream_slot(
        &self,
        request: ReadStreamSlotRequest,
    ) -> Result<Option<ReadStreamSlotSuccess>, DurableStreamReadError<WorkerExecutorError>> {
        let after = if request.from_offset.is_empty() {
            None
        } else {
            let bytes = request.from_offset.as_slice().try_into().map_err(|_| {
                WorkerExecutorError::invalid_request("offset must contain 24 bytes")
            })?;
            Some(
                StreamOffsetV1::from_bytes(bytes)
                    .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?,
            )
        };
        let producer = self.load_durable_stream_producer().await.map_err(|error| {
            DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        let notified = producer.session_records_changed().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let Some(mut slot) = producer
            .with_metadata_activity(self.resolve_stream_slot(&request.session, &request.slot))
            .await
            .map_err(|error| {
                DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
            })??
        else {
            return Ok(None);
        };
        if matches!(slot.source, SlotSource::Pending { finished: false })
            && request.max_items > 0
            && request.wait_millis > 0
        {
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(request.wait_millis.min(30_000)),
                notified,
            )
            .await;
            slot = producer
                .with_metadata_activity(self.resolve_stream_slot(&request.session, &request.slot))
                .await
                .map_err(|error| {
                    DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
                })??
                .ok_or_else(|| WorkerExecutorError::runtime("session disappeared during read"))?;
        }
        let mut response = ReadStreamSlotSuccess {
            items: Vec::new(),
            next_offset: request.from_offset.clone(),
            closed: false,
            cancelled: false,
            element_schema: Some(slot.graph.into()),
            content_type: if slot.bytes {
                "application/octet-stream"
            } else {
                "application/json"
            }
            .into(),
            up_to_date: true,
            head_offset: Vec::new(),
        };
        match slot.source {
            SlotSource::Stream(handle) => {
                let read = StreamHandleReadRequestV1 {
                    handle,
                    after,
                    max_items: request.max_items,
                    max_bytes: request.max_bytes,
                    wait_millis: request.wait_millis,
                };
                let bytes = self
                    .rpc()
                    .read_durable_stream_segment(
                        DurableStreamReadRequestV1::AuthorizedExport(Box::new(read)),
                        &AuthCtx::System,
                    )
                    .await
                    .map_err(|error| {
                        error.map_other(|error| WorkerExecutorError::runtime(error.to_string()))
                    })?;
                let read: StreamHandleReadResultV1 =
                    golem_common::serialization::deserialize(&bytes)
                        .map_err(WorkerExecutorError::runtime)?;
                response.next_offset = read
                    .next_offset
                    .map(|offset| offset.0.to_vec())
                    .unwrap_or_default();
                response.head_offset = read
                    .head_offset
                    .map(|offset| offset.0.to_vec())
                    .unwrap_or_default();
                response.closed = read.closed;
                response.cancelled = read.cancelled;
                response.up_to_date = read.next_offset >= read.head_offset;
                for event in read.events {
                    let content = match event.payload {
                        CommittedProducerStreamEventPayloadV1::Value(value) => {
                            Some(stream_slot_item::Content::Value(value))
                        }
                        CommittedProducerStreamEventPayloadV1::PackedU8(byte) => {
                            Some(stream_slot_item::Content::PackedU8(vec![byte]))
                        }
                        _ => None,
                    };
                    if let Some(content) = content {
                        response.items.push(StreamSlotItem {
                            offset: event.offset.0.to_vec(),
                            content: Some(content),
                        });
                    }
                }
            }
            SlotSource::Value(value, offset) => {
                response.head_offset = offset.0.to_vec();
                response.closed = true;
                if request.max_items > 0 && after.is_none_or(|after| after < offset) {
                    if value.len() as u64 > request.max_bytes {
                        return Err(WorkerExecutorError::invalid_request(
                            "slot value exceeds read byte limit",
                        )
                        .into());
                    }
                    response.next_offset = offset.0.to_vec();
                    response.items.push(StreamSlotItem {
                        offset: offset.0.to_vec(),
                        content: Some(stream_slot_item::Content::Value(value)),
                    });
                }
                response.up_to_date = response.next_offset >= response.head_offset;
            }
            SlotSource::Pending { finished } => {
                response.closed = finished;
                response.cancelled = finished;
            }
        }
        producer.ensure_healthy().map_err(|error| {
            DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        Ok(Some(response))
    }

    pub(crate) async fn append_to_stream_slot(
        &self,
        request: AppendToStreamSlotRequest,
    ) -> Result<AppendToStreamSlotResponse, WorkerExecutorError> {
        use append_to_stream_slot_response::Result as Outcome;
        let empty = || golem_api_grpc::proto::golem::common::Empty {};
        let producer = self.durable_stream_producer().await?;
        let Some(slot) = producer
            .with_metadata_activity(self.resolve_stream_slot(&request.session, &request.slot))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??
        else {
            return Ok(AppendToStreamSlotResponse {
                result: Some(Outcome::NotFound(empty())),
            });
        };
        if !slot.writable {
            return Err(WorkerExecutorError::invalid_request(
                "stream slot is read-only",
            ));
        }
        let SlotSource::Stream(handle) = slot.source else {
            return Err(WorkerExecutorError::runtime("input slot has no stream"));
        };
        let payload = match request.payload {
            Some(append_to_stream_slot_request::Payload::PackedU8(bytes)) if slot.bytes => {
                Some(StreamItemsPayloadV1::PackedU8(bytes))
            }
            Some(append_to_stream_slot_request::Payload::Values(values)) if !slot.bytes => {
                for encoded in &values.values {
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
                Some(StreamItemsPayloadV1::Values(values.values))
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
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
        let result = producer
            .append_external_input(
                &slot.session,
                handle.stream_id,
                payload,
                request.close,
                request.producer.map(|producer| ExternalProducerV1 {
                    id: producer.id,
                    epoch: producer.epoch,
                    sequence: producer.sequence,
                }),
            )
            .await
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
        Ok(AppendToStreamSlotResponse {
            result: Some(match result {
                ExternalAppendOutcomeV1::Accepted(offset) => Outcome::Accepted(AppendAccepted {
                    offset: offset.0.to_vec(),
                }),
                ExternalAppendOutcomeV1::Duplicate(offset) => Outcome::Duplicate(AppendDuplicate {
                    offset: offset.0.to_vec(),
                }),
                ExternalAppendOutcomeV1::EpochFenced(current_epoch) => {
                    Outcome::EpochFenced(AppendEpochFenced { current_epoch })
                }
                ExternalAppendOutcomeV1::SeqGap { expected, received } => {
                    Outcome::SequenceGap(AppendSequenceGap { expected, received })
                }
                ExternalAppendOutcomeV1::Closed => Outcome::Closed(empty()),
                ExternalAppendOutcomeV1::NotFound => Outcome::NotFound(empty()),
            }),
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
            slot_schema(&graph, &method, "input").unwrap(),
            Some((SchemaType::string(), Some(1), true, true))
        );
        assert_eq!(
            slot_schema(&graph, &method, "bytes").unwrap(),
            Some((SchemaType::u8(), Some(1), false, true))
        );
        assert_eq!(slot_schema(&graph, &method, "count").unwrap(), None);
        assert_eq!(slot_schema(&graph, &method, "$result").unwrap(), None);
        method.output_schema = OutputSchema::Single(Box::new(SchemaType::u64()));
        assert_eq!(
            slot_schema(&graph, &method, "$result").unwrap(),
            Some((SchemaType::u64(), None, false, false))
        );
        method.output_schema =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8()))));
        assert_eq!(
            slot_schema(&graph, &method, "$result").unwrap(),
            Some((SchemaType::u8(), None, false, true))
        );
    }

    #[test]
    fn canonical_slot_reference_is_not_a_transport_id_or_field_index() {
        use golem_common::model::durable_stream::{StreamId, StreamInvocationIdV1};
        let id = AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::from_u128(2)),
            agent_id: "source".into(),
        };
        let environment =
            golem_common::base_model::environment::EnvironmentId(uuid::Uuid::from_u128(3));
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(4));
        let source = StreamInvocationIdV1 {
            callee_environment_id: environment,
            callee: id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key: IdempotencyKey::new("source-session".into()),
        };
        let first = DurableStreamHandleV1 {
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
            slot_handle(
                &record.encode_to_vec(),
                Some(0),
                &[first.clone(), second.clone()]
            )
            .unwrap(),
            second
        );
        assert_eq!(
            slot_handle(
                &reference.encode_to_vec(),
                None,
                &[first.clone(), second.clone()]
            )
            .unwrap(),
            second
        );
        assert!(slot_handle(&record.encode_to_vec(), Some(0), &[first]).is_err());
    }
}
