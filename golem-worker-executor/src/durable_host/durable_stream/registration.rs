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
use golem_common::model::InvocationId;
use golem_common::model::durable_stream::StreamSessionRemoteResultRecord;
use golem_common::model::oplog::ReturnedValue;

#[derive(Debug, PartialEq)]
/// Durable handles and the invocation return produced while materializing result streams.
pub struct ResultStreamRegistration {
    pub handles: Vec<DurableStreamHandle>,
    pub returned_value: ReturnedValue,
    pub oplog_index: OplogIndex,
}

impl DurableStreamStore {
    pub(super) fn validate_registration_owner(
        &self,
        request: &ProducerRegistrationRequest,
    ) -> Result<(), StreamStoreError> {
        if matches!(
            request.source_invocation,
            StreamRegistrationInvocation::Local(_)
        ) && let StreamRegistrationCoordinate::Root { invocation_id, .. } = &request.coordinate
            && *invocation_id != self.qualify_session(&request.source_invocation)
        {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        if let Some(mapping) = &request.session_mapping {
            let session = self.qualify_session(&request.source_invocation);
            if mapping.session_key != session
                || !AttachmentId::primary(
                    session.callee_environment_id,
                    &session.callee,
                    &session.idempotency_key,
                )
                .is_ok_and(|attachment| attachment == mapping.attachment_id)
            {
                return Err(StreamStoreError::RegistrationDivergence);
            }
        }
        Ok(())
    }

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
        self.validate_registration_owner(&request)?;
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
                    value: existing.issue(self.generation()),
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
        let environment_id = self.environment_id;
        let producer = self.producer;
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
                        None,
                    )),
                )]
            }))
            .await
            .map_err(StreamStoreError::from)?;
        self.commit(context).await?;
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
        let registered = index
            .registrations
            .get(&StreamId::derive(
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
                oplog_index,
            ))
            .expect("applied registration is missing")
            .clone();
        self.buses
            .write()
            .expect("durable stream bus map lock poisoned")
            .insert(
                registered.handle.stream_id,
                Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
            );
        self.record_registered_streams(1);
        crate::metrics::durable_stream::record_producer_operation("register", false);
        tracing::debug!(
            stream_id = %registered.handle.stream_id,
            registration_oplog_index = registered.registration_oplog_index.as_u64(),
            replayed = false,
            "Durable stream registration committed"
        );
        Ok(ProducerWriteOutcome {
            value: registered.issue(self.generation()),
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
                        None,
                    )
                    .await
            },
        )
        .await
    }

    pub async fn register_invocation_return(
        &self,
        context: Option<&StreamWriteContext>,
        session_key: StreamSessionKey,
        result: Vec<u8>,
        outputs: Vec<ProducerOutputRegistration>,
        entity_parent_start_index: Option<OplogIndex>,
        method_name: Option<String>,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<ResultStreamRegistration, StreamStoreError> {
        if entity_parent_start_index.is_some() {
            return Err(StreamStoreError::CorruptHistory(
                "an agent invocation return cannot have entity attribution".into(),
            ));
        }
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
                        Some((method_name, component_revision)),
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
        invocation: Option<(
            Option<String>,
            golem_common::model::component::ComponentRevision,
        )>,
    ) -> Result<ResultStreamRegistration, StreamStoreError> {
        let mut keys = vec![ProducerMetadataKey::Session(session_key.clone())];
        for output in &outputs {
            match &output.source {
                ProducerOutputSource::New(request) => {
                    self.validate_registration_owner(request)?;
                    keys.extend(ProducerMetadataKey::registration(request));
                }
                ProducerOutputSource::Existing(handle) => {
                    if self.owns_handle_identity(handle) {
                        keys.push(ProducerMetadataKey::Stream(handle.stream_id));
                    }
                }
                ProducerOutputSource::Registered(handle, _) => {
                    keys.push(ProducerMetadataKey::Stream(handle.stream_id));
                }
            }
        }
        let mut index = self.index_for(keys).await?;
        if invocation.is_some()
            && index
                .session_entity_parent_start_index(&session_key)
                .is_some()
        {
            return Err(StreamStoreError::CorruptHistory(
                "an agent invocation return cannot have entity attribution".into(),
            ));
        }
        let invalid_existing_handle = outputs.iter().any(|output| {
            matches!(&output.source, ProducerOutputSource::Existing(handle) | ProducerOutputSource::Registered(handle, _)
                if self.owns_handle_identity(handle)
                && index
                    .registrations
                    .get(&handle.stream_id)
                    .is_none_or(|registration| !registration.accepts(handle, self.generation())))
                || matches!(&output.source, ProducerOutputSource::Registered(handle, source)
                    if !self.owns_handle_identity(handle)
                        || index.registrations.get(&handle.stream_id).is_none_or(|registration| {
                            source != &StreamRecordReference::Local(LocalStreamId(
                                registration.registration_oplog_index,
                            ))
                        }))
        });
        let result_offset = index.invocation_results.get(&session_key).copied();
        let result_session_key = session_key.clone();
        let result_session_reference =
            StreamRegistrationInvocation::Local(session_key.idempotency_key);
        if outputs
            .iter()
            .any(|output| output.cancellation_epoch == Some(0))
        {
            return Err(StreamStoreError::InvalidAttachmentState);
        }
        let mut requests = outputs
            .iter()
            .filter_map(|output| match &output.source {
                ProducerOutputSource::New(request) => Some(request.clone()),
                ProducerOutputSource::Existing(_) | ProducerOutputSource::Registered(_, _) => None,
            })
            .collect::<Vec<_>>();
        let make_result =
            move |outputs: Vec<ProducerOutputRegistration>,
                  owned_sources: Vec<StreamRecordReference>| {
                let mut owned_sources = owned_sources.into_iter();
                let stream_mappings = outputs
                    .into_iter()
                    .map(|output| {
                        let source = match output.source {
                            ProducerOutputSource::New(_) => owned_sources
                                .next()
                                .expect("result allocation supplies one source per new output"),
                            ProducerOutputSource::Existing(handle) => {
                                StreamRecordReference::Foreign(handle)
                            }
                            ProducerOutputSource::Registered(_, source) => source,
                        };
                        StreamBindingRecord {
                            transport_stream_id: output.transport_stream_id,
                            source,
                            role: output.role,
                        }
                    })
                    .collect::<Vec<_>>();
                ReturnedValue {
                    value: result,
                    stream_bindings: stream_mappings,
                }
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
            let expected = make_result(outputs, Vec::new());
            drop(index);
            let record = self
                .validate_result_registration(
                    result_offset,
                    result_session_key.idempotency_key,
                    &expected,
                    &invocation,
                )
                .await?;
            return if record == expected {
                Ok(ResultStreamRegistration {
                    handles: Vec::new(),
                    returned_value: record,
                    oplog_index: result_offset,
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
                    .map(|registration| registration.issue(self.generation()))
            })
            .collect::<Vec<_>>();
        let mut registered_handles = Vec::new();
        if existing_handles.iter().any(Option::is_some) {
            if existing_handles.iter().any(Option::is_none) {
                return Err(StreamStoreError::RegistrationDivergence);
            }
            let handles = existing_handles
                .into_iter()
                .map(Option::unwrap)
                .collect::<Vec<_>>();
            if let Some(offset) = result_offset {
                let sources = handles
                    .iter()
                    .map(|handle| {
                        let registration = index.registrations.get(&handle.stream_id).unwrap();
                        StreamRecordReference::Local(LocalStreamId(
                            registration.registration_oplog_index,
                        ))
                    })
                    .collect();
                let expected = make_result(outputs, sources);
                drop(index);
                let record = self
                    .validate_result_registration(
                        offset,
                        result_session_key.idempotency_key,
                        &expected,
                        &invocation,
                    )
                    .await?;
                if record == expected {
                    crate::metrics::durable_stream::record_producer_operation(
                        "register_result",
                        true,
                    );
                    return Ok(ResultStreamRegistration {
                        handles,
                        returned_value: record,
                        oplog_index: offset,
                    });
                }
                return Err(StreamStoreError::RegistrationDivergence);
            }
            registered_handles = handles;
        }
        // Exact committed replay preserves foreign handles without granting live authority.
        if invalid_existing_handle {
            return Err(StreamStoreError::InvalidHandle);
        }
        let mut cancellations = Vec::new();
        for (position, output) in outputs.iter().enumerate() {
            if let Some(epoch) = output.cancellation_epoch {
                let terminal = match &output.source {
                    ProducerOutputSource::New(request) => {
                        if let Some(stream_id) = index.coordinates.get(&request.coordinate) {
                            let stream = index
                                .streams
                                .get(stream_id)
                                .ok_or(StreamStoreError::UnknownStream(*stream_id))?;
                            if stream.terminal {
                                None
                            } else {
                                Some((
                                    stream.next_sequence,
                                    index.entity_parent_start_index(*stream_id)?,
                                ))
                            }
                        } else {
                            Some((0, entity_parent_start_index))
                        }
                    }
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
                    ProducerOutputSource::Registered(handle, _) => {
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
                };
                let applied_locally = matches!(&output.source, ProducerOutputSource::New(_))
                    || matches!(&output.source, ProducerOutputSource::Existing(handle) if self.owns_handle_identity(handle))
                    || matches!(&output.source, ProducerOutputSource::Registered(_, _));
                cancellations.push((position, epoch, terminal, applied_locally));
            }
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
                .checked_add(requests.len() - registered_handles.len())
                .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
            {
                crate::metrics::durable_stream::record_limit_violation("streams_per_session");
                return Err(StreamStoreError::StreamLimit);
            }
        }
        if !registered_handles.is_empty() {
            requests.clear();
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
        let producer = self.producer;
        let producer_fingerprint = self.producer_fingerprint;
        let mut handle_local_ids = index
            .registrations
            .iter()
            .map(|(id, registration)| (*id, LocalStreamId(registration.registration_oplog_index)))
            .collect::<HashMap<_, _>>();
        let existing_handles = registered_handles.clone();
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut result = Vec::with_capacity(requests.len() + 1);
                let mut handles = existing_handles;
                for (sub_index, request) in requests.into_iter().enumerate() {
                    let oplog_index = OplogIndex::from_u64(
                        first_index.as_u64()
                            + u64::try_from(sub_index)
                                .expect("durable stream batch size fits in u64"),
                    );
                    let record = registration_record(
                        oplog_index,
                        environment_id,
                        producer,
                        producer_fingerprint,
                        request,
                        None,
                    );
                    let handle = RegisteredStream::resolve(
                        record.clone(),
                        oplog_index,
                        environment_id,
                        &producer,
                        producer_fingerprint,
                    )
                    .expect("producer identity was validated")
                    .handle;
                    handle_local_ids.insert(handle.stream_id, LocalStreamId(oplog_index));
                    handles.push(handle);
                    result.push(DurableStreamOplogRecord::Registered(
                        entity_parent_start_index,
                        Box::new(record),
                    ));
                }
                let returned_value = make_result(
                    outputs,
                    handles
                        .iter()
                        .map(|handle| {
                            StreamRecordReference::Local(
                                *handle_local_ids
                                    .get(&handle.stream_id)
                                    .expect("new registration has a local id"),
                            )
                        })
                        .collect(),
                );
                let mut cancelled = HashSet::new();
                for (position, epoch, terminal, applied_locally) in cancellations {
                    let source = returned_value.stream_bindings[position].source.clone();
                    if !cancelled.insert(source.clone()) { continue; }
                    result.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecord::ConsumerCancelIntent(
                            golem_common::base_model::durable_stream::StreamConsumerCancelIntentRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: result_session_reference.clone(),
                                consumer_invocation: *result_session_reference.idempotency_key(),
                                source: source.clone(),
                                epoch,
                                cancellation: StreamCancellationRequest {
                                    direction: SessionStreamRole::Output,
                                    side: StreamEndpointSide::Consumer,
                                    authored_by: StreamTerminalAuthor::Host,
                                    terminal: StreamTerminal::Failed(StreamError::Interrupted("output slot deleted".into())),
                                },
                            }
                        )),
                    ));
                    if let Some((sequence, attribution)) = terminal {
                        let offset = StreamOffset::new(OplogIndex::from_u64(first_index.as_u64() + result.len() as u64), 0);
                        result.push(DurableStreamOplogRecord::Terminal(attribution, StreamTerminalRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id: match &source {
                                StreamRecordReference::Local(local_id) => *local_id,
                                StreamRecordReference::Foreign(handle) => handle_local_ids[&handle.stream_id],
                            },
                            sequence,
                            offset,
                            authored_by: StreamTerminalAuthor::Host,
                            origin: StreamTerminalOrigin::Control,
                            terminal: StreamTerminal::Failed(StreamError::Interrupted("output slot deleted".into())),
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
                                        session_key: result_session_reference.clone(),
                                        consumer_invocation: *result_session_reference.idempotency_key(),
                                        source,
                                        epoch,
                                        cancellation: StreamCancellationRequest {
                                            direction: SessionStreamRole::Output,
                                            side: StreamEndpointSide::Consumer,
                                            authored_by: StreamTerminalAuthor::Host,
                                            terminal: StreamTerminal::Failed(StreamError::Interrupted("output slot deleted".into())),
                                        },
                                    },
                                },
                            )),
                        ));
                    }
                }
                result.push(match invocation {
                    Some((method_name, component_revision)) => DurableStreamOplogRecord::InvocationReturned {
                        invocation_id: *result_session_reference.idempotency_key(),
                        value: returned_value,
                        method_name,
                        component_revision,
                    },
                    None => DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecord::RemoteResult(StreamSessionRemoteResultRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: result_session_reference,
                            result: returned_value.value,
                            stream_mappings: returned_value.stream_bindings,
                        })),
                    ),
                });
                result
            }))
            .await
            .map_err(StreamStoreError::from)?;
        self.commit(context).await?;

        let registered_count = registered_handles.len();
        let mut handles = registered_handles;
        let mut invocation_returned = None;
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
                    index.apply_registration(
                        oplog_index,
                        entity_parent_start_index,
                        record.clone(),
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?;
                    let registered = index
                        .registrations
                        .values()
                        .find(|value| value.registration_oplog_index == oplog_index)
                        .expect("applied registration is missing");
                    handles.push(registered.issue(self.generation()));
                    self.buses
                        .write()
                        .expect("durable stream bus map lock poisoned")
                        .insert(
                            registered.handle.stream_id,
                            Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
                        );
                }
                OplogEntry::AgentInvocationReturned {
                    invocation_id,
                    result,
                    ..
                } => {
                    let value = self
                        .oplog
                        .download_payload(result)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    index
                        .invocation_results
                        .insert(result_session_key.clone(), oplog_index);
                    index.apply_returned_bindings(&result_session_key, &value.stream_bindings)?;
                    if invocation_id != result_session_key.idempotency_key {
                        return Err(StreamStoreError::RegistrationDivergence);
                    }
                    invocation_returned = Some((oplog_index, value));
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
                    index.apply_session_references(
                        entity_parent_start_index,
                        &record,
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?;
                    index.apply_result_offset(
                        oplog_index,
                        &record,
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    );
                    if let StreamSessionRecord::RemoteResult(record) = record {
                        invocation_returned = Some((
                            oplog_index,
                            ReturnedValue {
                                value: record.result,
                                stream_bindings: record.stream_mappings,
                            },
                        ));
                    }
                }
                OplogEntry::StreamTerminal {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    let event = index.apply_terminal(
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
        self.record_registered_streams(handles.len() - registered_count);
        self.record_terminal_streams(cancelled_count);
        crate::metrics::durable_stream::record_producer_operation("register_result", false);
        let (oplog_index, returned_value) = invocation_returned.ok_or_else(|| {
            StreamStoreError::CorruptHistory(
                "result registration batch contains no invocation return".to_string(),
            )
        })?;
        Ok(ResultStreamRegistration {
            handles,
            returned_value,
            oplog_index,
        })
    }

    async fn validate_result_registration(
        &self,
        index: OplogIndex,
        invocation_id: InvocationId,
        expected: &ReturnedValue,
        invocation: &Option<(
            Option<String>,
            golem_common::model::component::ComponentRevision,
        )>,
    ) -> Result<ReturnedValue, StreamStoreError> {
        if invocation.is_none() {
            let OplogEntry::StreamSession { record, .. } = self.oplog.read(index).await? else {
                return Err(StreamStoreError::RegistrationDivergence);
            };
            let StreamSessionRecord::RemoteResult(record) = self
                .oplog
                .download_payload(record)
                .await
                .map_err(StreamStoreError::Oplog)?
            else {
                return Err(StreamStoreError::RegistrationDivergence);
            };
            if record.format_version != DURABLE_STREAM_FORMAT_VERSION
                || record.session_key != StreamRegistrationInvocation::Local(invocation_id)
                || record.result != expected.value
                || record.stream_mappings != expected.stream_bindings
            {
                return Err(StreamStoreError::RegistrationDivergence);
            }
            return Ok(ReturnedValue {
                value: record.result,
                stream_bindings: record.stream_mappings,
            });
        }
        let OplogEntry::AgentInvocationReturned {
            invocation_id: recorded_id,
            result,
            method_name: recorded_method,
            component_revision: recorded_revision,
            ..
        } = self.oplog.read(index).await?
        else {
            return Err(StreamStoreError::RegistrationDivergence);
        };
        if recorded_id != invocation_id
            || invocation.as_ref() != Some(&(recorded_method, recorded_revision))
        {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        let recorded = self
            .oplog
            .download_payload(result)
            .await
            .map_err(StreamStoreError::Oplog)?;
        if &recorded != expected {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        Ok(recorded)
    }

    pub async fn read_invocation_returned(
        &self,
        index: OplogIndex,
        invocation_id: InvocationId,
    ) -> Result<ReturnedValue, StreamStoreError> {
        match self
            .oplog
            .read(index)
            .await
            .map_err(StreamStoreError::from)?
        {
            OplogEntry::AgentInvocationReturned {
                invocation_id: recorded,
                result,
                ..
            } if recorded == invocation_id => self
                .oplog
                .download_payload(result)
                .await
                .map_err(StreamStoreError::Oplog),
            _ => Err(StreamStoreError::RegistrationDivergence),
        }
    }

    pub async fn read_invocation_result_value(
        &self,
        index: OplogIndex,
        invocation_id: InvocationId,
    ) -> Result<ReturnedValue, StreamStoreError> {
        use golem_common::model::AgentInvocationResult;
        use golem_common::model::oplog::FinishedResult;
        use prost::Message;

        match self.oplog.read(index).await? {
            OplogEntry::AgentInvocationReturned {
                invocation_id: recorded,
                result,
                ..
            } if recorded == invocation_id => self
                .oplog
                .download_payload(result)
                .await
                .map_err(StreamStoreError::Oplog),
            OplogEntry::AgentInvocationFinished {
                invocation_id: recorded,
                result,
                ..
            } if recorded == invocation_id => match result {
                FinishedResult::Returned(returned) if returned < index => {
                    self.read_invocation_returned(returned, invocation_id).await
                }
                FinishedResult::Payload(payload) => {
                    let result = self
                        .oplog
                        .download_payload(payload)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    let AgentInvocationResult::AgentMethod { output } = result else {
                        return Err(StreamStoreError::CorruptHistory(
                            "session result is not a method result".into(),
                        ));
                    };
                    let value = golem_common::schema::streaming::encode_recursive_stream_value(
                        &output,
                        |_, _| Err("inline invocation result contains a stream".into()),
                    )
                    .map_err(StreamStoreError::CorruptHistory)?
                    .encode_to_vec();
                    Ok(ReturnedValue {
                        value,
                        stream_bindings: Vec::new(),
                    })
                }
                _ => Err(StreamStoreError::RegistrationDivergence),
            },
            _ => Err(StreamStoreError::RegistrationDivergence),
        }
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
        Ok(registration.issue(self.generation()))
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
    _oplog_index: OplogIndex,
    environment_id: EnvironmentId,
    producer: AgentId,
    producer_fingerprint: AgentFingerprint,
    request: ProducerRegistrationRequest,
    local_parent: Option<(StreamId, LocalStreamId)>,
) -> StreamRegisteredRecord {
    let source_invocation = request.source_invocation;
    let coordinate = match request.coordinate {
        StreamRegistrationCoordinate::Root {
            invocation_id,
            root_kind,
            recursive_value_path,
        } => {
            let invocation = match &source_invocation {
                StreamRegistrationInvocation::Local(key) => {
                    assert!(
                        invocation_id.callee_environment_id == environment_id
                            && invocation_id.callee == producer
                            && invocation_id.callee_fingerprint == producer_fingerprint
                            && invocation_id.idempotency_key == *key,
                        "local stream registration coordinate must identify the registering owner"
                    );
                    StreamRegistrationInvocation::Local(*key)
                }
                StreamRegistrationInvocation::Remote(_) => {
                    StreamRegistrationInvocation::Remote(invocation_id)
                }
            };
            StreamRegistrationRecordCoordinate::Root {
                invocation,
                root_kind,
                recursive_value_path,
            }
        }
        StreamRegistrationCoordinate::Nested {
            parent_stream_id,
            parent_producer_sequence,
            recursive_value_path,
        } => StreamRegistrationRecordCoordinate::Nested {
            parent_stream: if let Some((local_stream_id, local_id)) = local_parent
                && local_stream_id == parent_stream_id
            {
                StreamRecordReference::Local(local_id)
            } else {
                panic!("nested registrations require an oplog-local producer parent")
            },
            parent_producer_sequence,
            recursive_value_path,
        },
    };
    StreamRegisteredRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        coordinate,
        source_invocation,
        component_revision: request.component_revision,
        element_schema_fingerprint: request.element_schema_fingerprint,
        source_kind: request.source_kind,
        session_role: request.session_mapping.map(|mapping| mapping.role),
    }
}

#[cfg(test)]
pub(super) fn registered_stream(
    oplog_index: OplogIndex,
    environment_id: EnvironmentId,
    producer: AgentId,
    producer_fingerprint: AgentFingerprint,
    request: ProducerRegistrationRequest,
) -> RegisteredStream {
    RegisteredStream::resolve(
        registration_record(
            oplog_index,
            environment_id,
            producer,
            producer_fingerprint,
            request,
            None,
        ),
        oplog_index,
        environment_id,
        &producer,
        producer_fingerprint,
    )
    .expect("producer identity was validated")
}

pub(super) fn registration_matches(
    registration: &RegisteredStream,
    request: &ProducerRegistrationRequest,
) -> bool {
    registration.coordinate == request.coordinate
        && registration.record.source_invocation == request.source_invocation
        && registration.handle.component_revision == request.component_revision
        && registration.handle.element_schema_fingerprint == request.element_schema_fingerprint
        && registration.record.source_kind == request.source_kind
        && registration.session_mapping == request.session_mapping
}
