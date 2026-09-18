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

use super::index::registration_coordinate_depth;
use super::*;

#[derive(Debug, Eq, PartialEq)]
/// Durable handles and the session record produced while materializing result streams.
pub struct ResultStreamRegistration {
    pub handles: Vec<DurableStreamHandle>,
    pub session_record: StreamSessionRecord,
}

impl DurableStreamStore {
    #[tracing::instrument(name = "durable_stream.register", skip_all)]
    /// Durably registers a stream before its handle can be exposed to a consumer.
    pub async fn register(
        &self,
        context: Option<&StreamWriteContext>,
        request: ProducerRegistrationRequest,
    ) -> Result<ProducerWriteOutcome<DurableStreamHandle>, StreamStoreError> {
        self.run_owned(context, 0, move |owner, context| async move {
            owner.register_owned(&context, request).await
        })
        .await
    }

    async fn register_owned(
        &self,
        context: &StreamWriteContext,
        request: ProducerRegistrationRequest,
    ) -> Result<ProducerWriteOutcome<DurableStreamHandle>, StreamStoreError> {
        if registration_coordinate_depth(&request.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH {
            crate::metrics::durable_stream::record_limit_violation("traversal_depth");
            return Err(StreamStoreError::TraversalDepthLimit);
        }
        let mut index = self
            .index_for(ProducerMetadataKey::registration(&request))
            .await?;
        if let Some(stream_id) = index.coordinates.get(&request.coordinate) {
            let existing = index
                .registrations
                .get(stream_id)
                .expect("coordinate index points at a missing registration");
            if registration_matches(existing, &request)
                && index.entity_parent_start_index(*stream_id)? == request.entity_parent_start_index
            {
                crate::metrics::durable_stream::record_producer_operation("register", true);
                tracing::debug!(
                    stream_id = %existing.handle.stream_id,
                    registration_oplog_index = existing.registration_oplog_index.as_u64(),
                    replayed = true,
                    "Durable stream registration resolved"
                );
                return Ok(ProducerWriteOutcome {
                    value: existing.handle.clone(),
                    replayed: true,
                });
            }
            return Err(StreamStoreError::RegistrationDivergence);
        }
        index.ensure_producer_write_allowed()?;
        if !matches!(
            &request.coordinate,
            StreamRegistrationCoordinate::Root { .. }
        ) {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        let session_key = index
            .registration_session_key(&request.coordinate, &request.session_mapping)
            .ok_or_else(|| match &request.coordinate {
                StreamRegistrationCoordinate::Nested {
                    parent_stream_id, ..
                } => StreamStoreError::UnknownStream(*parent_stream_id),
                StreamRegistrationCoordinate::Root { .. } => {
                    unreachable!("root registration always defines its session")
                }
            })?;
        if index.finished_sessions.contains(&session_key) {
            return Err(StreamStoreError::SessionFinished(session_key));
        }
        if index
            .session_stream_counts
            .get(&session_key)
            .copied()
            .unwrap_or_default()
            >= MAX_DURABLE_STREAMS_PER_SESSION
        {
            crate::metrics::durable_stream::record_limit_violation("streams_per_session");
            return Err(StreamStoreError::StreamLimit);
        }
        StreamId::derive(
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
            OplogIndex::INITIAL,
        )
        .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?;

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let entity_parent_start_index = request.entity_parent_start_index;
        let request_for_entry = request.clone();
        context.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Registered(
                    entity_parent_start_index,
                    Box::new(registration_record(
                        oplog_index,
                        environment_id,
                        producer,
                        producer_fingerprint,
                        request_for_entry,
                    )),
                )]
            }))
            .await
            .map_err(StreamStoreError::Oplog)?;
        self.commit(context).await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("registration batch returned no oplog entry");
        let OplogEntry::StreamRegistered { record, .. } = entry else {
            unreachable!("registration builder returned a different oplog entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(StreamStoreError::Oplog)?;
        index.apply_registration(
            oplog_index,
            entity_parent_start_index,
            record.clone(),
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        self.buses
            .write()
            .expect("durable stream bus map lock poisoned")
            .insert(
                record.handle.stream_id,
                Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
            );
        self.record_registered_streams(1);
        crate::metrics::durable_stream::record_producer_operation("register", false);
        tracing::debug!(
            stream_id = %record.handle.stream_id,
            registration_oplog_index = record.registration_oplog_index.as_u64(),
            replayed = false,
            "Durable stream registration committed"
        );
        Ok(ProducerWriteOutcome {
            value: record.handle,
            replayed: false,
        })
    }

    /// Registers or validates every stream discovered in an invocation result.
    pub async fn register_result_streams(
        &self,
        context: Option<&StreamWriteContext>,
        session_key: StreamSessionKey,
        result: Vec<u8>,
        outputs: Vec<ProducerOutputRegistration>,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Result<ResultStreamRegistration, StreamStoreError> {
        self.run_owned(
            context,
            result.len() * 2,
            move |owner, context| async move {
                owner
                    .register_result_streams_owned(
                        &context,
                        session_key,
                        result,
                        outputs,
                        entity_parent_start_index,
                    )
                    .await
            },
        )
        .await
    }

    async fn register_result_streams_owned(
        &self,
        context: &StreamWriteContext,
        session_key: StreamSessionKey,
        result: Vec<u8>,
        outputs: Vec<ProducerOutputRegistration>,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Result<ResultStreamRegistration, StreamStoreError> {
        let mut keys = vec![ProducerMetadataKey::Session(session_key.clone())];
        for output in &outputs {
            match &output.source {
                ProducerOutputSource::New(request) => {
                    keys.extend(ProducerMetadataKey::registration(request))
                }
                ProducerOutputSource::Existing(handle) => {
                    keys.push(ProducerMetadataKey::Reference(handle.stream_id));
                    if self.owns_handle_identity(handle) {
                        keys.push(ProducerMetadataKey::Stream(handle.stream_id));
                    }
                }
            }
        }
        let mut index = self.index_for(keys).await?;
        let result_offset = index.invocation_results.get(&session_key).copied();
        let result_session_key = session_key.clone();
        let mut cancellations = Vec::new();
        for (position, output) in outputs.iter().enumerate() {
            if let Some(epoch) = output.cancellation_epoch {
                if epoch == 0 {
                    return Err(StreamStoreError::InvalidAttachmentState);
                }
                let terminal = match &output.source {
                    ProducerOutputSource::New(_) => Some((0, entity_parent_start_index)),
                    ProducerOutputSource::Existing(handle) if self.owns_handle_identity(handle) => {
                        let stream = index
                            .streams
                            .get(&handle.stream_id)
                            .ok_or(StreamStoreError::UnknownStream(handle.stream_id))?;
                        if stream.terminal {
                            None
                        } else {
                            Some((
                                stream.next_sequence,
                                index.entity_parent_start_index(handle.stream_id)?,
                            ))
                        }
                    }
                    ProducerOutputSource::Existing(_) => None,
                };
                let applied_locally = matches!(&output.source, ProducerOutputSource::New(_))
                    || matches!(&output.source, ProducerOutputSource::Existing(handle) if self.owns_handle_identity(handle));
                cancellations.push((position, epoch, terminal, applied_locally));
            }
        }
        let requests = outputs
            .iter()
            .filter_map(|output| match &output.source {
                ProducerOutputSource::New(request) => Some(request.clone()),
                ProducerOutputSource::Existing(_) => None,
            })
            .collect::<Vec<_>>();
        let make_result = move |owned_handles: Vec<DurableStreamHandle>| {
            let mut owned_handles = owned_handles.into_iter();
            let stream_mappings = outputs
                .into_iter()
                .map(|output| {
                    let handle = match output.source {
                        ProducerOutputSource::New(_) => owned_handles
                            .next()
                            .expect("result allocation supplies one handle per new output"),
                        ProducerOutputSource::Existing(handle) => handle,
                    };
                    StreamSessionMappingRecord {
                        transport_stream_id: output.transport_stream_id,
                        handle,
                        role: SessionStreamRole::Output,
                    }
                })
                .collect::<Vec<_>>();
            StreamSessionRecord::InvocationResult(
                golem_common::base_model::durable_stream::StreamSessionInvocationResultRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key,
                    result,
                    output_streams: stream_mappings
                        .iter()
                        .map(|mapping| mapping.handle.clone())
                        .collect(),
                    stream_mappings,
                },
            )
        };
        index.ensure_producer_write_allowed()?;
        if requests.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            crate::metrics::durable_stream::record_limit_violation("streams_per_value");
            return Err(StreamStoreError::ValueStreamLimit);
        }
        let mut coordinates = HashSet::new();
        if requests.iter().any(|request| {
            request.entity_parent_start_index != entity_parent_start_index
                || !coordinates.insert(&request.coordinate)
        }) {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        if requests.is_empty()
            && let Some(result_offset) = result_offset
        {
            let expected = make_result(Vec::new());
            let StreamSessionRecord::InvocationResult(_) = &expected else {
                return Err(StreamStoreError::CorruptHistory(
                    "empty result registration did not build an invocation-result record"
                        .to_string(),
                ));
            };
            if !expected.has_supported_format() {
                return Err(StreamStoreError::CorruptHistory(
                    "result registration built a malformed invocation-result record".to_string(),
                ));
            }
            drop(index);
            let record = self.read_result_record(result_offset).await?;
            return if record == expected {
                Ok(ResultStreamRegistration {
                    handles: Vec::new(),
                    session_record: record,
                })
            } else {
                Err(StreamStoreError::RegistrationDivergence)
            };
        }
        let existing_handles = requests
            .iter()
            .map(|request| {
                index
                    .coordinates
                    .get(&request.coordinate)
                    .and_then(|stream_id| index.registrations.get(stream_id))
                    .filter(|registration| registration_matches(registration, request))
                    .map(|registration| registration.handle.clone())
            })
            .collect::<Vec<_>>();
        if existing_handles.iter().any(Option::is_some) {
            if existing_handles.iter().any(Option::is_none) {
                return Err(StreamStoreError::RegistrationDivergence);
            }
            let handles = existing_handles
                .into_iter()
                .map(Option::unwrap)
                .collect::<Vec<_>>();
            let expected = make_result(handles.clone());
            drop(index);
            if let Some(offset) = result_offset {
                let record = self.read_result_record(offset).await?;
                if record == expected {
                    crate::metrics::durable_stream::record_producer_operation(
                        "register_result",
                        true,
                    );
                    return Ok(ResultStreamRegistration {
                        handles,
                        session_record: record,
                    });
                }
            }
            return Err(StreamStoreError::RegistrationDivergence);
        }
        if index.finished_sessions.contains(&result_session_key) {
            return Err(StreamStoreError::SessionFinished(result_session_key));
        }
        let session_key = requests.first().and_then(|request| {
            index.registration_session_key(&request.coordinate, &request.session_mapping)
        });
        if let Some(session_key) = session_key {
            if index.finished_sessions.contains(&session_key) {
                return Err(StreamStoreError::SessionFinished(session_key));
            }
            let current = index
                .session_stream_counts
                .get(&session_key)
                .copied()
                .unwrap_or_default();
            if current
                .checked_add(requests.len())
                .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
            {
                crate::metrics::durable_stream::record_limit_violation("streams_per_session");
                return Err(StreamStoreError::StreamLimit);
            }
        }
        for request in &requests {
            if registration_coordinate_depth(&request.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH
            {
                crate::metrics::durable_stream::record_limit_violation("traversal_depth");
                return Err(StreamStoreError::TraversalDepthLimit);
            }
            if !matches!(
                request.coordinate,
                StreamRegistrationCoordinate::Root { .. }
            ) || index.coordinates.contains_key(&request.coordinate)
            {
                return Err(StreamStoreError::RegistrationDivergence);
            }
        }

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut result = Vec::with_capacity(requests.len() + 1);
                let mut handles = Vec::with_capacity(requests.len());
                for (sub_index, request) in requests.into_iter().enumerate() {
                    let oplog_index = OplogIndex::from_u64(
                        first_index.as_u64()
                            + u64::try_from(sub_index)
                                .expect("durable stream batch size fits in u64"),
                    );
                    let record = registration_record(
                        oplog_index,
                        environment_id,
                        producer.clone(),
                        producer_fingerprint,
                        request,
                    );
                    handles.push(record.handle.clone());
                    result.push(DurableStreamOplogRecord::Registered(
                        entity_parent_start_index,
                        Box::new(record),
                    ));
                }
                let session_record = make_result(handles);
                if !session_record.has_supported_format() {
                    return Vec::new();
                }
                let StreamSessionRecord::InvocationResult(invocation_result) = &session_record else {
                    unreachable!("result builder returns an invocation result");
                };
                let mut cancelled = HashSet::new();
                for (position, epoch, terminal, applied_locally) in cancellations {
                    let handle = &invocation_result.stream_mappings[position].handle;
                    if !cancelled.insert(handle.stream_id) { continue; }
                    result.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecord::ConsumerCancelIntent(
                            golem_common::base_model::durable_stream::StreamConsumerCancelIntentRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: invocation_result.session_key.clone(),
                                stream_id: handle.stream_id,
                                epoch,
                                role: StreamCancelRole::OutputConsumer,
                                reason: StreamCancelReason::Cancelled,
                                details: None,
                            }
                        )),
                    ));
                    if let Some((sequence, attribution)) = terminal {
                        let offset = StreamOffset::new(OplogIndex::from_u64(first_index.as_u64() + result.len() as u64), 0);
                        result.push(DurableStreamOplogRecord::Cancel(attribution, StreamCancelRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id: handle.stream_id,
                            producer_fingerprint,
                            sequence,
                            offset,
                            authored_by: StreamTerminalAuthor::Protocol,
                            role: StreamCancelRole::OutputConsumer,
                            reason: StreamCancelReason::Cancelled,
                            details: None,
                        }));
                    }
                    if applied_locally {
                        result.push(DurableStreamOplogRecord::Session(
                            entity_parent_start_index,
                            Box::new(StreamSessionRecord::ConsumerCancelApplied(
                                golem_common::base_model::durable_stream::StreamConsumerCancelAppliedRecord {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    intent: golem_common::base_model::durable_stream::StreamConsumerCancelIntentRecord {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        session_key: invocation_result.session_key.clone(),
                                        stream_id: handle.stream_id,
                                        epoch,
                                        role: StreamCancelRole::OutputConsumer,
                                        reason: StreamCancelReason::Cancelled,
                                        details: None,
                                    },
                                },
                            )),
                        ));
                    }
                }
                result.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(session_record),
                ));
                result
            }))
            .await
            .map_err(StreamStoreError::Oplog)?;
        self.commit(context).await;

        let mut handles = Vec::new();
        let mut session_record = None;
        let mut cancelled_count = 0;
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamRegistered {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    handles.push(record.handle.clone());
                    index.apply_registration(
                        oplog_index,
                        entity_parent_start_index,
                        record.clone(),
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?;
                    self.buses
                        .write()
                        .expect("durable stream bus map lock poisoned")
                        .insert(
                            record.handle.stream_id,
                            Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
                        );
                }
                OplogEntry::StreamSession {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    index.apply_session_references(entity_parent_start_index, &record)?;
                    index.apply_result_offset(oplog_index, &record);
                    if matches!(record, StreamSessionRecord::InvocationResult(_)) {
                        session_record = Some(record);
                    }
                }
                OplogEntry::StreamCancel {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    let event = index.apply_cancel(
                        oplog_index,
                        entity_parent_start_index,
                        record,
                        self.producer_fingerprint,
                    )?;
                    self.cancel_source(event.stream_id);
                    // The terminal dispatcher publishes without delaying result registration on a reader.
                    drop(self.enqueue_events(
                        Some(context),
                        event.stream_id,
                        vec![event],
                        false,
                    )?);
                    cancelled_count += 1;
                }
                _ => {
                    return Err(StreamStoreError::CorruptHistory(
                        "result registration batch contains an unexpected oplog entry".to_string(),
                    ));
                }
            }
        }
        self.record_registered_streams(handles.len());
        self.record_terminal_streams(cancelled_count);
        crate::metrics::durable_stream::record_producer_operation("register_result", false);
        Ok(ResultStreamRegistration {
            handles,
            session_record: session_record.ok_or_else(|| {
                StreamStoreError::CorruptHistory(
                    "result registration batch contains no session record".to_string(),
                )
            })?,
        })
    }

    /// Rejects registration reuse whose durable identity or schema differs.
    pub async fn validate_registration(
        &self,
        request: &ProducerRegistrationRequest,
    ) -> Result<DurableStreamHandle, StreamStoreError> {
        let index = self
            .index_for([ProducerMetadataKey::Coordinate(request.coordinate.clone())])
            .await?;
        let stream_id = index
            .coordinates
            .get(&request.coordinate)
            .ok_or(StreamStoreError::RegistrationDivergence)?;
        let registration = index.registrations.get(stream_id).ok_or_else(|| {
            StreamStoreError::CorruptHistory(
                "registration coordinate points at a missing registration".to_string(),
            )
        })?;
        if !registration_matches(registration, request) {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        Ok(registration.handle.clone())
    }

    /// Enforces the per-session limit before any additional registrations are appended.
    pub async fn validate_new_session_stream_count(
        &self,
        session_key: &StreamSessionKey,
        new_stream_count: usize,
    ) -> Result<(), StreamStoreError> {
        if new_stream_count > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            return Err(StreamStoreError::ValueStreamLimit);
        }
        let index = self
            .index_for([ProducerMetadataKey::Session(session_key.clone())])
            .await?;
        if new_stream_count != 0 && index.finished_sessions.contains(session_key) {
            return Err(StreamStoreError::SessionFinished(session_key.clone()));
        }
        let current = index
            .session_stream_counts
            .get(session_key)
            .copied()
            .unwrap_or_default();
        if current
            .checked_add(new_stream_count)
            .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
        {
            return Err(StreamStoreError::StreamLimit);
        }
        Ok(())
    }
}

pub(super) fn registration_record(
    oplog_index: OplogIndex,
    environment_id: EnvironmentId,
    producer: AgentId,
    producer_fingerprint: AgentFingerprint,
    request: ProducerRegistrationRequest,
) -> StreamRegisteredRecord {
    let stream_id = StreamId::derive(environment_id, &producer, producer_fingerprint, oplog_index)
        .expect("producer identity was validated before reserving the registration index");
    StreamRegisteredRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        coordinate: request.coordinate,
        registration_oplog_index: oplog_index,
        handle: DurableStreamHandle {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id,
            producer_environment_id: environment_id,
            producer,
            expected_producer_fingerprint: producer_fingerprint,
            source_invocation: request.source_invocation,
            component_revision: request.component_revision,
            element_schema_fingerprint: request.element_schema_fingerprint,
        },
        source_kind: request.source_kind,
        session_mapping: request.session_mapping,
    }
}

pub(super) fn registration_matches(
    record: &StreamRegisteredRecord,
    request: &ProducerRegistrationRequest,
) -> bool {
    record.coordinate == request.coordinate
        && record.handle.source_invocation == request.source_invocation
        && record.handle.component_revision == request.component_revision
        && record.handle.element_schema_fingerprint == request.element_schema_fingerprint
        && record.source_kind == request.source_kind
        && record.session_mapping == request.session_mapping
}
