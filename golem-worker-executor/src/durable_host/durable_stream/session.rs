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
use super::registration::registration_record;
use super::*;

impl DurableStreamStore {
    /// Installs the runtime service used to project durable session control metadata.
    pub(crate) fn set_control_metadata_provider(
        &self,
        service: Arc<dyn WorkerService>,
        mode: AgentMode,
    ) {
        assert!(self.control_metadata_provider.set((service, mode)).is_ok());
    }

    /// Loads control metadata reconstructed from committed session records.
    pub(crate) async fn persisted_control_metadata(
        &self,
        key: &StreamSessionKey,
    ) -> Result<Option<SessionControlMetadata>, String> {
        self.ensure_healthy()?;
        let Some((service, mode)) = self.control_metadata_provider.get() else {
            return Ok(None);
        };
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(StreamStoreError::RecoveryRequired)?;
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        activity
            .scope(service.lookup_durable_stream_control_metadata(&owner, *mode, key))
            .await
            .map(Some)
    }

    /// Loads committed per-stream consumer ordinals and source offsets.
    pub(crate) async fn persisted_consumer_positions(
        &self,
        key: &StreamSessionKey,
        stream: StreamId,
    ) -> Result<Option<(OplogIndex, Vec<OplogIndex>)>, String> {
        self.ensure_healthy()?;
        let Some((service, mode)) = self.control_metadata_provider.get() else {
            return Ok(None);
        };
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(StreamStoreError::RecoveryRequired)?;
        activity
            .scope(async {
                let owner = OwnedAgentId::new(self.environment_id, &self.producer);
                let metadata = service
                    .lookup_durable_stream_control_metadata(&owner, *mode, key)
                    .await?;
                let count = metadata
                    .consumer_record_counts
                    .get(&stream)
                    .copied()
                    .unwrap_or_default();
                let page_size =
                    crate::services::stream_session_index::CONSUMER_JOURNAL_INDEX_PAGE_SIZE;
                let mut positions = Vec::new();
                for page in 0..count.div_ceil(page_size) {
                    let records = service
                        .read_durable_stream_consumer_page(&owner, key, stream, page)
                        .await?;
                    let needed = (count - page * page_size).min(page_size) as usize;
                    if records.len() < needed {
                        return Err(
                            "consumer journal index page is shorter than its coverage".into()
                        );
                    }
                    positions.extend_from_slice(&records[..needed]);
                }
                Ok(Some((metadata.covered_through, positions)))
            })
            .await
    }

    /// Appends a session fact under the producer's default attribution.
    pub(crate) async fn append_session_record(
        &self,
        context: Option<&StreamWriteContext>,
        record: StreamSessionRecord,
    ) -> Result<(), StreamStoreError> {
        self.append_session_record_attributed(context, None, record)
            .await
    }

    /// Appends a session fact with explicit entity ownership attribution.
    pub(crate) async fn append_session_record_attributed(
        &self,
        context: Option<&StreamWriteContext>,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamSessionRecord,
    ) -> Result<(), StreamStoreError> {
        let memory = golem_common::serialization::serialize(&record)
            .map_err(StreamStoreError::Oplog)?
            .len();
        self.run_owned(context, memory, move |owner, context| async move {
            owner
                .append_session_record_owned(&context, entity_parent_start_index, record)
                .await
        })
        .await
    }

    /// Serializes, commits, and indexes one session mutation.
    pub(crate) async fn append_session_record_owned(
        &self,
        context: &StreamWriteContext,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamSessionRecord,
    ) -> Result<(), StreamStoreError> {
        self.append_session_records_owned(context, entity_parent_start_index, vec![record])
            .await
    }

    /// Serializes and commits an ordered batch of session mutations.
    pub(crate) async fn append_session_records_owned(
        &self,
        context: &StreamWriteContext,
        entity_parent_start_index: Option<OplogIndex>,
        records: Vec<StreamSessionRecord>,
    ) -> Result<(), StreamStoreError> {
        if records.iter().any(|record| !record.has_supported_format()) {
            return Err(StreamStoreError::CorruptHistory(
                "unsupported or malformed durable Stream Session record".to_string(),
            ));
        }
        let mut index = self
            .index_for(records.iter().flat_map(ProducerMetadataKey::session_record))
            .await?;
        let mut staged = index.clone();
        for record in &records {
            if staged.deleting
                && !matches!(
                    record,
                    StreamSessionRecord::AttachmentFinalized(_)
                        | StreamSessionRecord::CascadeOutbox(_)
                        | StreamSessionRecord::ConsumerDeleting(_)
                        | StreamSessionRecord::SourceUnavailable(_)
                )
            {
                return Err(StreamStoreError::ProducerDeleting);
            }
            if staged.consumer_deleting
                && matches!(
                    record,
                    StreamSessionRecord::TopologyPrepared(_)
                        | StreamSessionRecord::TopologyActivated(_)
                )
            {
                return Err(StreamStoreError::ConsumerDeleting);
            }
            staged.apply_session_references(entity_parent_start_index, record)?;
            staged.apply_deletion_record(
                record,
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
            )?;
        }
        let result_keys = records
            .iter()
            .enumerate()
            .filter_map(|(position, record)| match record {
                StreamSessionRecord::InvocationResult(record) => {
                    Some((position, record.session_key.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |_| {
                records
                    .into_iter()
                    .map(|record| {
                        DurableStreamOplogRecord::Session(
                            entity_parent_start_index,
                            Box::new(record),
                        )
                    })
                    .collect()
            }))
            .await
            .map_err(StreamStoreError::Oplog)?;
        for (position, key) in result_keys {
            staged
                .invocation_results
                .entry(key)
                .or_insert(entries[position].0);
        }
        self.commit(context).await;
        *index = staged;
        drop(index);
        self.notify_session_records_changed(Some(context));
        Ok(())
    }

    /// Returns the process-local serialization lock for a durable session identity.
    pub(crate) fn session_lock(
        &self,
        session_key: &StreamSessionKey,
    ) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .session_locks
            .lock()
            .expect("durable stream session lock map poisoned");
        if locks.len() >= 128 {
            locks.retain(|_, lock| lock.strong_count() > 0);
        }
        if let Some(lock) = locks.get(session_key).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(session_key.clone(), Arc::downgrade(&lock));
        lock
    }

    /// Rejects new records once the durable session has reached a terminal state.
    pub(crate) async fn ensure_session_accepts_new_events(
        &self,
        session_key: &StreamSessionKey,
    ) -> Result<(), StreamStoreError> {
        if self
            .index_for([ProducerMetadataKey::Session(session_key.clone())])
            .await?
            .finished_sessions
            .contains(session_key)
        {
            Err(StreamStoreError::SessionFinished(session_key.clone()))
        } else {
            Ok(())
        }
    }

    /// Returns the notification used to recheck durable session state.
    pub(crate) fn session_records_changed(&self) -> &Notify {
        &self.session_records_changed
    }

    /// Wakes waiters after the corresponding session records have been committed.
    pub(crate) fn notify_session_records_changed(&self, context: Option<&StreamWriteContext>) {
        if let Some(context) = context {
            context.assert_owner(self);
            context.notify_session_records_changed();
        } else {
            self.session_records_changed.notify_waiters();
        }
    }

    /// Atomically persists root inputs, the session descriptor, and invocation attachment before acceptance.
    pub(crate) async fn prepare_session(
        &self,
        context: Option<&StreamWriteContext>,
        requests: Vec<(u64, ProducerRegistrationRequest)>,
        pending_invocation: OplogEntry,
        committed: oneshot::Sender<()>,
        make_prepared: impl FnOnce(Vec<(u64, DurableStreamHandle)>) -> StreamSessionPreparedRecord
        + Send
        + 'static,
    ) -> Result<StreamSessionPreparedRecord, StreamStoreError> {
        let memory = golem_common::serialization::serialize(&pending_invocation)
            .map_err(StreamStoreError::Oplog)?
            .len();
        self.run_owned(context, memory, move |owner, context| async move {
            owner
                .prepare_session_owned(
                    &context,
                    requests,
                    pending_invocation,
                    committed,
                    make_prepared,
                )
                .await
        })
        .await
    }

    async fn prepare_session_owned(
        &self,
        context: &StreamWriteContext,
        requests: Vec<(u64, ProducerRegistrationRequest)>,
        pending_invocation: OplogEntry,
        committed: oneshot::Sender<()>,
        make_prepared: impl FnOnce(Vec<(u64, DurableStreamHandle)>) -> StreamSessionPreparedRecord
        + Send
        + 'static,
    ) -> Result<StreamSessionPreparedRecord, StreamStoreError> {
        let mut index = self
            .index_for(
                requests
                    .iter()
                    .flat_map(|(_, request)| ProducerMetadataKey::registration(request))
                    .collect::<Vec<_>>(),
            )
            .await?;
        index.ensure_producer_write_allowed()?;
        if requests.len() > MAX_DURABLE_STREAMS_PER_SESSION {
            crate::metrics::durable_stream::record_limit_violation("streams_per_session");
            return Err(StreamStoreError::StreamLimit);
        }
        let entity_parent_start_index = requests
            .first()
            .and_then(|(_, request)| request.entity_parent_start_index);
        for (_, request) in &requests {
            if request.entity_parent_start_index != entity_parent_start_index {
                return Err(StreamStoreError::RegistrationDivergence);
            }
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
            if let Some(session_key) =
                index.registration_session_key(&request.coordinate, &request.session_mapping)
                && index.finished_sessions.contains(&session_key)
            {
                return Err(StreamStoreError::SessionFinished(session_key));
            }
        }

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let records = requests.clone();
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut result = Vec::with_capacity(records.len() + 3);
                let mut handles = Vec::with_capacity(records.len());
                for (sub_index, (transport_stream_id, request)) in records.into_iter().enumerate() {
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
                    handles.push((transport_stream_id, record.handle.clone()));
                    result.push(DurableStreamOplogRecord::Registered(
                        entity_parent_start_index,
                        record,
                    ));
                }
                let prepared = make_prepared(handles);
                let prepared_record = StreamSessionRecord::Prepared(prepared);
                if !prepared_record.has_supported_format() {
                    return Vec::new();
                }
                let pending_invocation_oplog_index = OplogIndex::from_u64(
                    first_index.as_u64()
                        + u64::try_from(result.len() + 1)
                            .expect("durable stream batch size fits in u64"),
                );
                let StreamSessionRecord::Prepared(prepared) = &prepared_record else {
                    unreachable!()
                };
                let attached = StreamSessionAttachedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: prepared.attempt.session_key.clone(),
                    attachment_id: prepared.attempt.attachment_id,
                    attempt_id: prepared.attempt.attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                };
                result.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(prepared_record),
                ));
                result.push(DurableStreamOplogRecord::InlineEntry(pending_invocation));
                result.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(StreamSessionRecord::Attached(attached)),
                ));
                result
            }))
            .await
            .map_err(StreamStoreError::Oplog)?;

        let mut prepared = None;
        let mut registrations = Vec::with_capacity(requests.len());
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
                    registrations.push((oplog_index, entity_parent_start_index, record));
                }
                OplogEntry::StreamSession { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    match record {
                        StreamSessionRecord::Prepared(record) => prepared = Some(record),
                        StreamSessionRecord::Attached(_) => {}
                        _ => {
                            return Err(StreamStoreError::CorruptHistory(
                                "preparation batch contains an unexpected session record"
                                    .to_string(),
                            ));
                        }
                    }
                }
                OplogEntry::PendingAgentInvocation { .. } => {}
                _ => {
                    return Err(StreamStoreError::CorruptHistory(
                        "preparation batch contains an unexpected oplog entry".to_string(),
                    ));
                }
            }
        }
        let prepared = prepared.ok_or_else(|| {
            StreamStoreError::CorruptHistory(
                "preparation batch contains no Prepared session record".to_string(),
            )
        })?;
        let mut updated_index = index.clone();
        let mut buses = Vec::with_capacity(registrations.len());
        for (oplog_index, entity_parent_start_index, record) in registrations {
            updated_index.apply_registration(
                oplog_index,
                entity_parent_start_index,
                record.clone(),
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
            )?;
            buses.push((
                record.handle.stream_id,
                Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
            ));
        }
        updated_index.apply_session_references(
            entity_parent_start_index,
            &StreamSessionRecord::Prepared(prepared.clone()),
        )?;

        self.commit_notifying(context, committed).await;
        *index = updated_index;
        self.buses
            .write()
            .expect("durable stream bus map lock poisoned")
            .extend(buses);
        self.record_registered_streams(requests.len());
        crate::metrics::durable_stream::record_producer_operation("prepare_session", false);
        tracing::debug!(
            attachment_id = %prepared.attempt.attachment_id.0,
            attempt_id = %prepared.attempt.attempt_id.0,
            epoch = 1_u64,
            registered_streams = requests.len(),
            "Durable Stream Session preparation committed"
        );
        Ok(prepared)
    }

    /// Returns whether a forwarded input still requires source history or a terminal.
    pub(crate) async fn has_open_forwarded_session_input(
        &self,
        session_key: &StreamSessionKey,
    ) -> Result<bool, StreamStoreError> {
        let index = self.index_for_session_streams(session_key).await?;
        let Some(mappings) = index.session_stream_mappings.get(session_key) else {
            return Ok(false);
        };
        let candidates = mappings
            .iter()
            .filter_map(|(handle, role)| {
                (*role == SessionStreamRole::Input
                    && mappings.contains(&(handle.clone(), SessionStreamRole::Output))
                    && self.owns_handle_identity(handle))
                .then_some(handle.stream_id)
            })
            .collect::<Vec<_>>();
        Ok(index.session_stream_mappings[session_key]
            .iter()
            .any(|(handle, role)| {
                *role == SessionStreamRole::Input
                    && candidates.contains(&handle.stream_id)
                    && index
                        .streams
                        .get(&handle.stream_id)
                        .is_some_and(|stream| !stream.terminal)
            }))
    }

    /// Records the session terminal after all required stream finalization is durable.
    pub(crate) async fn finish_session(
        &self,
        context: Option<&StreamWriteContext>,
        session_key: StreamSessionKey,
        entity_parent_start_index: Option<OplogIndex>,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReason,
    ) -> Result<(), StreamStoreError> {
        if self
            .index_for([ProducerMetadataKey::Session(session_key.clone())])
            .await?
            .finished_sessions
            .contains(&session_key)
        {
            return Ok(());
        }
        // Finalization can duplicate the error in every output terminal and Finished,
        // with additional copies held while the ordinary batch is serialized.
        let terminal_bytes = result
            .as_ref()
            .err()
            .map_or(0, |bytes| bytes.len())
            .max(b"output stream ended without a terminal".len());
        let memory = terminal_bytes
            .saturating_mul(MAX_DURABLE_STREAMS_PER_SESSION + 1)
            .saturating_mul(4);
        self.run_lifecycle(context, memory, move |owner, context| async move {
            owner
                .finish_session_owned(
                    &context,
                    session_key,
                    entity_parent_start_index,
                    result,
                    input_cancel_reason,
                )
                .await
        })
        .await
    }

    async fn finish_session_owned(
        &self,
        context: &StreamWriteContext,
        session_key: StreamSessionKey,
        entity_parent_start_index: Option<OplogIndex>,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReason,
    ) -> Result<(), StreamStoreError> {
        let mut index = self.index_for_session_streams(&session_key).await?;
        if index.finished_sessions.contains(&session_key) {
            return Ok(());
        }
        index.ensure_producer_write_allowed()?;
        let mut open_streams = index
            .stream_sessions
            .iter()
            .filter_map(|(stream_id, candidate_session)| {
                if candidate_session != &session_key {
                    return None;
                }
                let stream = index
                    .streams
                    .get(stream_id)
                    .expect("session stream index points at a missing stream");
                (!stream.terminal).then(|| {
                    (
                        *stream_id,
                        *index
                            .stream_roles
                            .get(stream_id)
                            .expect("session stream index points at a missing role"),
                        stream.next_sequence,
                        *index
                            .entity_parent_start_indices
                            .get(stream_id)
                            .expect("session stream index points at missing attribution"),
                    )
                })
            })
            .collect::<Vec<_>>();
        open_streams.sort_by_key(|(stream_id, _, _, _)| *stream_id);
        if open_streams
            .iter()
            .any(|(_, _, _, attribution)| *attribution != entity_parent_start_index)
        {
            return Err(StreamStoreError::CorruptHistory(
                "session stream attribution differs from its session".to_string(),
            ));
        }

        let producer_fingerprint = self.producer_fingerprint;
        let session_key_for_batch = session_key.clone();
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let terminals = open_streams.into_iter().enumerate().map(
                    |(position, (stream_id, role, sequence, stream_attribution))| {
                        let oplog_index =
                            OplogIndex::from_u64(first_index.as_u64() + position as u64);
                        match role {
                            SessionStreamRole::Input => DurableStreamOplogRecord::Cancel(
                                stream_attribution,
                                StreamCancelRecord {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    stream_id,
                                    producer_fingerprint,
                                    sequence,
                                    offset: StreamOffset::new(oplog_index, 0),
                                    authored_by: StreamTerminalAuthor::Protocol,
                                    role: StreamCancelRole::InputConsumer,
                                    reason: input_cancel_reason,
                                    details: Some(
                                        "invocation finished before consuming the complete input"
                                            .to_string(),
                                    ),
                                },
                            ),
                            SessionStreamRole::Output => {
                                let details = match &result {
                                    Ok(()) => b"output stream ended without a terminal".to_vec(),
                                    Err(details) => details.clone(),
                                };
                                DurableStreamOplogRecord::End(
                                    stream_attribution,
                                    StreamEndRecord {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        stream_id,
                                        producer_fingerprint,
                                        sequence,
                                        offset: StreamOffset::new(oplog_index, 0),
                                        authored_by: StreamTerminalAuthor::Protocol,
                                        result: StreamEndResult::ErrorContext(details),
                                    },
                                )
                            }
                        }
                    },
                );
                let mut records = terminals.collect::<Vec<_>>();
                records.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_key_for_batch,
                        result,
                    })),
                ));
                records
            }))
            .await
            .map_err(StreamStoreError::Oplog)?;
        self.commit(context).await;

        let mut terminal_events = Vec::new();
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamEnd {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    let event = index.apply_end(
                        oplog_index,
                        entity_parent_start_index,
                        record,
                        self.producer_fingerprint,
                    )?;
                    terminal_events.push(self.enqueue_events(
                        Some(context),
                        event.stream_id,
                        vec![event],
                        false,
                    )?);
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
                    terminal_events.push(self.enqueue_events(
                        Some(context),
                        event.stream_id,
                        vec![event],
                        false,
                    )?);
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
                    let StreamSessionRecord::Finished(record) = record else {
                        return Err(StreamStoreError::CorruptHistory(
                            "session finish batch contains an unexpected session record"
                                .to_string(),
                        ));
                    };
                    index.apply_finished(&record)?;
                }
                _ => {
                    return Err(StreamStoreError::CorruptHistory(
                        "session finish batch contains an unexpected oplog entry".to_string(),
                    ));
                }
            }
        }
        let terminal_count = terminal_events.len();
        self.record_terminal_streams(terminal_count);
        drop(index);
        for publication in terminal_events {
            self.wait_for_publication(Some(context), publication)
                .await?;
        }
        crate::metrics::durable_stream::record_producer_operation("finish_session", false);
        tracing::debug!(
            terminal_streams = terminal_count,
            "Durable Stream Session finish committed"
        );
        self.notify_session_records_changed(Some(context));
        Ok(())
    }
}
