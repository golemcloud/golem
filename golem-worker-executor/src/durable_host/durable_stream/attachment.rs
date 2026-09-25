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

use super::index::{attachment_slot, attachment_sort_key};
use super::metadata::{IndexedAttachmentCandidate, IndexedAttachmentCandidateBatch};
use super::*;

impl DurableStreamStore {
    /// Commits deterministic source-unavailable state for unread consumer history.
    pub async fn commit_source_unavailable_overlay(
        &self,
        context: Option<&StreamWriteContext>,
        key: StreamAttachmentKey,
        reader_id: LocalStreamReaderId,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<bool, StreamStoreError> {
        self.run_lifecycle(context, 0, move |owner, context| async move {
            owner
                .commit_source_unavailable_overlay_owned(
                    &context,
                    key,
                    reader_id,
                    source_offset,
                    consumer_read_ordinal,
                )
                .await
        })
        .await
    }

    async fn commit_source_unavailable_overlay_owned(
        &self,
        context: &StreamWriteContext,
        key: StreamAttachmentKey,
        reader_id: LocalStreamReaderId,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<bool, StreamStoreError> {
        if key.consumer_environment_id != self.environment_id
            || key.consumer != self.producer
            || key.expected_consumer_fingerprint != self.producer_fingerprint
        {
            return Err(StreamStoreError::InvalidAttachmentState);
        }
        let mut metadata = SessionControlMetadata::default();
        self.refresh_control_metadata(&key.session_key, &mut metadata)
            .await
            .map_err(StreamStoreError::from)?;
        if !metadata.readers_for_attachment(&key).contains(&reader_id) {
            return Err(StreamStoreError::InvalidAttachmentState);
        }
        let introducing = self
            .oplog
            .read_exact(reader_id.introducing_oplog_index, 1)
            .await
            .into_iter()
            .next()
            .filter(|(index, _)| *index == reader_id.introducing_oplog_index)
            .ok_or_else(|| {
                StreamStoreError::CorruptHistory(
                    "consumer reader introducing record is missing".to_string(),
                )
            })?;
        let OplogEntry::StreamSession { record, .. } = introducing.1 else {
            return Err(StreamStoreError::CorruptHistory(
                "consumer reader was not introduced by a stream-session record".to_string(),
            ));
        };
        let introducing = self
            .oplog
            .download_payload(record)
            .await
            .map_err(StreamStoreError::Oplog)?;
        let session_key = crate::worker::stream_session_record_reference(&introducing, reader_id)
            .ok_or_else(|| {
            StreamStoreError::CorruptHistory(
                "consumer reader introducing record has no matching binding".to_string(),
            )
        })?;
        if session_key.qualify(
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        ) != key.session_key
        {
            return Err(StreamStoreError::InvalidAttachmentState);
        }
        let mut index = self
            .index_for([ProducerMetadataKey::ConsumerHead(
                key.session_key.clone(),
                reader_id,
            )])
            .await?;
        let current = self.oplog.current_oplog_index().await;
        let mut source_offsets = Vec::new();
        let mut overlay = None;
        if current.is_defined() {
            for (oplog_index, entry) in self
                .oplog
                .read_exact(OplogIndex::INITIAL, current.as_u64())
                .await
            {
                if self
                    .fork_lineage
                    .deleted_regions()
                    .is_in_deleted_region(oplog_index)
                {
                    continue;
                }
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let record = self
                    .oplog
                    .download_payload(record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                match record {
                    StreamSessionRecord::ConsumerItemValue(record)
                        if record.session_key.qualify(
                            key.consumer_environment_id,
                            &key.consumer,
                            key.expected_consumer_fingerprint,
                        ) == key.session_key
                            && record.reader_id == reader_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(StreamStoreError::CorruptHistory(
                                "consumer value journal contains a read-ordinal gap".to_string(),
                            ));
                        }
                        for index in 0..record.logical_item_count() {
                            source_offsets.push(record.source_offset_at(index).ok_or_else(
                                || {
                                    StreamStoreError::CorruptHistory(
                                        "packed-u8 consumer journal offset range is invalid"
                                            .to_string(),
                                    )
                                },
                            )?);
                        }
                    }
                    StreamSessionRecord::ConsumerTerminal(record)
                        if record.session_key.qualify(
                            key.consumer_environment_id,
                            &key.consumer,
                            key.expected_consumer_fingerprint,
                        ) == key.session_key
                            && record.reader_id == reader_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(StreamStoreError::CorruptHistory(
                                "consumer terminal journal contains a read-ordinal gap".to_string(),
                            ));
                        }
                        source_offsets.push(record.source_offset);
                    }
                    StreamSessionRecord::SourceUnavailable(record)
                        if record.session_key == session_key && record.reader_id == reader_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(StreamStoreError::CorruptHistory(
                                "source-unavailable overlay contains a read-ordinal gap"
                                    .to_string(),
                            ));
                        }
                        match &overlay {
                            Some(existing) if existing != &record => {
                                return Err(StreamStoreError::CorruptHistory(
                                    "conflicting source-unavailable overlays".to_string(),
                                ));
                            }
                            Some(_) => {}
                            None => overlay = Some(record),
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(existing) = overlay {
            return if existing.source_offset == source_offset
                && existing.consumer_read_ordinal == consumer_read_ordinal
            {
                Ok(true)
            } else {
                Err(StreamStoreError::AttachmentConflict)
            };
        }
        if source_offsets.len() as u64 != consumer_read_ordinal {
            return Err(StreamStoreError::ConsumerJournalAdvanced);
        }
        let record = StreamSessionRecord::SourceUnavailable(StreamSourceUnavailableRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key,
            reader_id,
            source_offset,
            consumer_read_ordinal,
        });
        let session_key = crate::worker::stream_session_record_key(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )
        .expect("source-unavailable record always identifies a session");
        let entity_parent_start_index = index.session_entity_parent_start_index(&session_key);
        index.apply_session_references(
            entity_parent_start_index,
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        context.begin_durable_effect();
        self.oplog
            .add(OplogEntry::stream_session(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record)),
            ))
            .await?;
        self.commit(context).await?;
        self.notify_session_records_changed(Some(context));
        Ok(false)
    }

    /// Returns the committed source-unavailable boundary for a consumer stream, if any.
    pub async fn consumer_source_unavailable(
        &self,
        key: &StreamAttachmentKey,
        reader_id: LocalStreamReaderId,
    ) -> Result<Option<StreamOffset>, StreamStoreError> {
        if key.consumer_environment_id != self.environment_id
            || key.consumer != self.producer
            || key.expected_consumer_fingerprint != self.producer_fingerprint
            || key.consumer_invocation.callee_environment_id != self.environment_id
            || key.consumer_invocation.callee != self.producer
            || key.consumer_invocation.callee_fingerprint != self.producer_fingerprint
        {
            return Err(StreamStoreError::InvalidAttachmentState);
        }
        let index = self
            .index_for([ProducerMetadataKey::ConsumerHead(
                key.session_key.clone(),
                reader_id,
            )])
            .await?;
        Ok(index
            .consumer_journals
            .get(&(key.session_key.clone(), reader_id))
            .and_then(|journal| journal.source_unavailable))
    }

    async fn persist_attachment_record(
        &self,
        record: StreamSessionRecord,
    ) -> Result<AttachmentApplyOutcome, StreamStoreError> {
        self.run_lifecycle(None, 0, move |owner, context| async move {
            owner
                .persist_attachment_record_owned(&context, record)
                .await
        })
        .await
    }

    async fn persist_attachment_record_owned(
        &self,
        context: &StreamWriteContext,
        record: StreamSessionRecord,
    ) -> Result<AttachmentApplyOutcome, StreamStoreError> {
        if !record.has_supported_format() {
            return Err(StreamStoreError::CorruptHistory(
                "unsupported or malformed durable attachment record".to_string(),
            ));
        }
        let mut index = self
            .index_for(ProducerMetadataKey::session_record(
                &record,
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
            ))
            .await?;
        let key = match &record {
            StreamSessionRecord::AttachmentPrepared(record) => &record.key,
            StreamSessionRecord::AttachmentActivated(record) => &record.key,
            StreamSessionRecord::AttachmentFinalized(record) => &record.key,
            _ => unreachable!("attachment persistence received a non-attachment record"),
        };
        let stream_id = key.stream_id;
        let slot = attachment_slot(key);
        let was_reconcilable = index.attachments.get(&slot).is_some_and(|attachment| {
            !matches!(
                attachment.state,
                IndexedStreamAttachmentState::Finalized { .. }
            )
        });
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let mut updated = index.clone();
        updated.apply_session_references(
            entity_parent_start_index,
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        let outcome = updated.apply_attachment_record(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        let is_reconcilable = updated.attachments.get(&slot).is_some_and(|attachment| {
            !matches!(
                attachment.state,
                IndexedStreamAttachmentState::Finalized { .. }
            )
        });
        if outcome == AttachmentApplyOutcome::Changed {
            context.begin_durable_effect();
            self.oplog
                .add(OplogEntry::stream_session(
                    entity_parent_start_index,
                    OplogPayload::Inline(Box::new(record)),
                ))
                .await?;
            self.commit(context).await?;
            match (was_reconcilable, is_reconcilable) {
                (false, true) => {
                    self.reconcilable_attachment_count
                        .fetch_add(1, Ordering::Release);
                }
                (true, false) => {
                    let previous = self
                        .reconcilable_attachment_count
                        .fetch_sub(1, Ordering::Release);
                    assert!(previous != 0, "reconcilable attachment count underflow");
                }
                _ => {}
            }
            *index = updated;
        }
        drop(index);
        if outcome == AttachmentApplyOutcome::Changed {
            self.notify_session_records_changed(Some(context));
        }
        Ok(outcome)
    }
}

#[async_trait]
impl StreamAttachmentControl for DurableStreamStore {
    #[tracing::instrument(
        name = "durable_stream.attachment.prepare",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch)
    )]
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError> {
        let lease_expires_at_millis = attachment_lease_expiry(now_millis)?;
        let outcome = self
            .persist_attachment_record(StreamSessionRecord::AttachmentPrepared(
                StreamAttachmentPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    prepared_at_millis: now_millis,
                    lease_expires_at_millis,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "prepare",
            if replayed { "replayed" } else { "committed" },
        );
        crate::metrics::durable_stream::record_lease_remaining(
            lease_expires_at_millis.saturating_sub(now_millis),
        );
        Ok(ProducerWriteOutcome {
            value: self.attachment_view(&key).await?,
            replayed,
        })
    }

    #[tracing::instrument(
        name = "durable_stream.attachment.activate",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch)
    )]
    async fn activate_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError> {
        let outcome = self
            .persist_attachment_record(StreamSessionRecord::AttachmentActivated(
                StreamAttachmentActivatedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    activated_at_millis: now_millis,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "activate",
            if replayed { "replayed" } else { "committed" },
        );
        Ok(ProducerWriteOutcome {
            value: self.attachment_view(&key).await?,
            replayed,
        })
    }

    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<StreamAttachmentView, StreamStoreError> {
        let view = self.attachment_view(key).await?;
        if !matches!(view.state, StreamAttachmentState::Active) {
            return Err(StreamStoreError::InvalidAttachmentState);
        }
        Ok(view)
    }

    #[tracing::instrument(
        name = "durable_stream.attachment.finalize",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch, reason = ?reason)
    )]
    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKey,
        reason: StreamAttachmentFinalizationReason,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError> {
        let outcome = self
            .persist_attachment_record(StreamSessionRecord::AttachmentFinalized(
                StreamAttachmentFinalizedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    finalized_at_millis: now_millis,
                    reason,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "finalize",
            if replayed { "replayed" } else { "committed" },
        );
        Ok(ProducerWriteOutcome {
            value: self.attachment_view(&key).await?,
            replayed,
        })
    }

    #[cfg(test)]
    async fn inspect_attachments(&self) -> Vec<StreamAttachmentView> {
        self.index.lock().await.attachment_views()
    }
}

impl DurableStreamStore {
    /// Returns whether producer-side attachments need inspection on load or deletion.
    pub async fn has_reconcilable_attachments(&self) -> bool {
        self.reconcilable_attachment_count.load(Ordering::Acquire) != 0
    }

    /// Validates producer identity and checks whether this session has an active attachment to the stream.
    pub async fn has_active_attachment(
        &self,
        session: &StreamSessionKey,
        handle: &DurableStreamHandle,
    ) -> Result<bool, StreamStoreError> {
        let index = self
            .index_for([
                ProducerMetadataKey::Stream(handle.stream_id),
                ProducerMetadataKey::ActiveAttachmentCount(session.clone(), handle.stream_id),
            ])
            .await?;
        if !index
            .registrations
            .get(&handle.stream_id)
            .is_some_and(|registration| {
                registration.handle.producer_environment_id == handle.producer_environment_id
                    && registration.handle.producer == handle.producer
                    && registration.handle.expected_producer_fingerprint
                        == handle.expected_producer_fingerprint
            })
        {
            return Err(StreamStoreError::InvalidHandle);
        }
        Ok(index
            .active_attachments_by_session_stream
            .get(&(session.clone(), handle.stream_id))
            .copied()
            .unwrap_or_default()
            > 0)
    }

    /// Returns whether the producer's durable deletion barrier has been recorded.
    pub async fn deletion_started(&self) -> bool {
        let index = self.index.lock().await;
        index.deleting || index.consumer_deleting
    }

    /// Returns attachment and cascade evidence that currently governs deletion.
    pub async fn deletion_diagnostics(
        &self,
    ) -> Result<StreamDeletionDiagnostics, StreamStoreError> {
        let index = self.index_for_cleanup().await?;
        let mut cascade_completed = index
            .cascade_outbox
            .iter()
            .map(|(key, result)| (key.clone(), result.clone()))
            .collect::<Vec<_>>();
        cascade_completed.sort_by(|(left, _), (right, _)| {
            attachment_sort_key(left).cmp(&attachment_sort_key(right))
        });
        Ok(StreamDeletionDiagnostics {
            deleting: index.deleting,
            attachments: index.attachment_views(),
            cascade_completed,
        })
    }

    /// Records deterministic loss for dependents before allowing cascaded deletion.
    pub async fn cascade_deletion(
        &self,
        now_millis: u64,
        probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
    ) -> Result<(), StreamStoreError> {
        self.commit_deletion_barrier(now_millis, false).await?;
        let dependents = self.index.lock().await.incomplete_cascade_dependents();
        for key in dependents {
            let status = probe.status(&key).await?;
            let result = match status {
                ConsumerAttachmentStatus::Deleting | ConsumerAttachmentStatus::Missing => {
                    StreamCascadeDependentResult::ConsumerDeleted
                }
                ConsumerAttachmentStatus::IncarnationMismatch
                | ConsumerAttachmentStatus::EpochMismatch => {
                    StreamCascadeDependentResult::ConsumerIncarnationChanged
                }
                ConsumerAttachmentStatus::Prepared | ConsumerAttachmentStatus::Active => {
                    let inspections = probe
                        .journal_inspection(&key)
                        .await?
                        .ok_or(StreamStoreError::InvalidAttachmentState)?;
                    let producer_offsets = {
                        let index = self.index.lock().await;
                        index
                            .streams
                            .get(&key.stream_id)
                            .ok_or(StreamStoreError::UnknownStream(key.stream_id))?
                            .offsets()
                    };
                    let mut first_missing = None;
                    for inspection in inspections {
                        if inspection.source_offsets.len() > producer_offsets.len()
                            || producer_offsets[..inspection.source_offsets.len()]
                                != inspection.source_offsets
                        {
                            return Err(StreamStoreError::CorruptHistory(
                                "consumer journal is not an exact prefix of producer history"
                                    .to_string(),
                            ));
                        }
                        if inspection.source_offsets.len() != producer_offsets.len() {
                            let first_unjournaled_offset =
                                producer_offsets[inspection.source_offsets.len()];
                            if let Some(existing) = inspection.source_unavailable {
                                if existing != first_unjournaled_offset {
                                    return Err(StreamStoreError::CorruptHistory(
                                    "source-unavailable overlay does not identify the first unjournaled producer position"
                                        .to_string(),
                                ));
                                }
                            } else {
                                probe
                                    .commit_source_unavailable(
                                        &key,
                                        inspection.reader_id,
                                        first_unjournaled_offset,
                                        inspection.source_offsets.len() as u64,
                                    )
                                    .await?;
                            }
                            first_missing = Some(
                                first_missing
                                    .map_or(first_unjournaled_offset, |offset: StreamOffset| {
                                        offset.min(first_unjournaled_offset)
                                    }),
                            );
                        }
                    }
                    match first_missing {
                        Some(first_unjournaled_offset) => {
                            StreamCascadeDependentResult::SourceUnavailable {
                                first_unjournaled_offset,
                            }
                        }
                        None => StreamCascadeDependentResult::ConsumerJournalComplete,
                    }
                }
            };
            self.commit_cascade_outbox(key, now_millis, result).await?;
        }
        let incomplete = self.index.lock().await.incomplete_cascade_dependents();
        if incomplete.is_empty() {
            Ok(())
        } else {
            Err(StreamStoreError::DeletionBlocked(incomplete))
        }
    }

    pub(super) async fn commit_deletion_barrier(
        &self,
        now_millis: u64,
        require_no_dependents: bool,
    ) -> Result<(), StreamStoreError> {
        self.run_lifecycle(None, 0, move |owner, context| async move {
            owner
                .commit_deletion_barrier_owned(&context, now_millis, require_no_dependents)
                .await
        })
        .await
    }

    async fn commit_deletion_barrier_owned(
        &self,
        context: &StreamWriteContext,
        now_millis: u64,
        require_no_dependents: bool,
    ) -> Result<(), StreamStoreError> {
        let mut index = self.index_for_cleanup().await?;
        if index.deleting {
            crate::metrics::durable_stream::record_producer_operation("deletion_barrier", true);
            return Ok(());
        }
        if require_no_dependents {
            let dependents = index.live_dependents();
            if !dependents.is_empty() {
                return Err(StreamStoreError::DeletionBlocked(dependents));
            }
        }
        let mut open_streams = index
            .streams
            .iter()
            .filter_map(|(stream_id, stream)| {
                (!stream.terminal).then_some((
                    index
                        .local_stream_id(*stream_id)
                        .expect("open stream has a registration"),
                    stream.next_sequence,
                    *index
                        .entity_parent_start_indices
                        .get(stream_id)
                        .expect("stream index points at missing attribution"),
                ))
            })
            .collect::<Vec<_>>();
        open_streams.sort_by_key(|(stream_id, _, _)| *stream_id);
        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut records = Vec::with_capacity(open_streams.len() + 1);
                for (position, (stream_id, sequence, entity_parent_start_index)) in
                    open_streams.into_iter().enumerate()
                {
                    let oplog_index = OplogIndex::from_u64(first_index.as_u64() + position as u64);
                    records.push(DurableStreamOplogRecord::Cancel(
                        entity_parent_start_index,
                        StreamCancelRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id,
                            sequence,
                            offset: StreamOffset::new(oplog_index, 0),
                            authored_by: StreamTerminalAuthor::Protocol,
                            role: StreamCancelRole::System,
                            reason: StreamCancelReason::ProducerDeleting,
                            details: None,
                        },
                    ));
                }
                records.push(DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecord::ProducerDeleting(
                        StreamProducerDeletingRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            producer_environment_id: environment_id,
                            producer,
                            producer_fingerprint,
                            deleting_at_millis: now_millis,
                        },
                    )),
                ));
                records
            }))
            .await
            .map_err(StreamStoreError::from)?;
        self.commit(context).await?;
        let mut terminal_events = Vec::new();
        for (oplog_index, entry) in entries {
            match entry {
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
                    terminal_events.push(index.apply_cancel(
                        oplog_index,
                        entity_parent_start_index,
                        record,
                        self.producer_fingerprint,
                    )?);
                }
                OplogEntry::StreamSession { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    index.apply_deletion_record(
                        &record,
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?;
                }
                _ => {
                    return Err(StreamStoreError::CorruptHistory(
                        "durable deletion barrier batch contains an unexpected entry".to_string(),
                    ));
                }
            }
        }
        index.complete_for_deletion = true;
        let terminal_count = terminal_events.len();
        for event in terminal_events {
            self.cancel_source(event.stream_id);
            // The terminal dispatcher owns delivery. Deletion waits for the committed
            // barrier and cascade records, never for a live reader to make progress.
            drop(self.enqueue_events(Some(context), event.stream_id, vec![event], false)?);
        }
        self.record_terminal_streams(terminal_count);
        drop(index);
        crate::metrics::durable_stream::record_producer_operation("deletion_barrier", false);
        tracing::debug!(
            terminal_streams = terminal_count,
            "Durable stream producer deletion barrier committed"
        );
        self.notify_session_records_changed(Some(context));
        Ok(())
    }

    async fn commit_cascade_outbox(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
        result: StreamCascadeDependentResult,
    ) -> Result<(), StreamStoreError> {
        self.run_lifecycle(None, 0, move |owner, context| async move {
            owner
                .commit_cascade_outbox_owned(&context, key, now_millis, result)
                .await
        })
        .await
    }

    async fn commit_cascade_outbox_owned(
        &self,
        context: &StreamWriteContext,
        key: StreamAttachmentKey,
        now_millis: u64,
        result: StreamCascadeDependentResult,
    ) -> Result<(), StreamStoreError> {
        let mut index = self.index.lock().await;
        self.ensure_healthy()?;
        if let Some(existing) = index.cascade_outbox.get(&key) {
            return if existing == &result {
                crate::metrics::durable_stream::record_cascade("replayed");
                Ok(())
            } else {
                Err(StreamStoreError::CorruptHistory(
                    "conflicting durable cascade completion".to_string(),
                ))
            };
        }
        let outcome = match &result {
            StreamCascadeDependentResult::ConsumerDeleted => "consumer_deleted",
            StreamCascadeDependentResult::ConsumerIncarnationChanged => {
                "consumer_incarnation_changed"
            }
            StreamCascadeDependentResult::ConsumerJournalComplete => "journal_complete",
            StreamCascadeDependentResult::SourceUnavailable { .. } => "source_unavailable",
        };
        let attachment_id = key.attachment_id;
        let stream_id = key.stream_id;
        let epoch = key.epoch;
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let record = StreamSessionRecord::CascadeOutbox(StreamCascadeOutboxRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            key,
            completed_at_millis: now_millis,
            result,
        });
        context.begin_durable_effect();
        self.oplog
            .add(OplogEntry::stream_session(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record.clone())),
            ))
            .await?;
        self.commit(context).await?;
        index.apply_session_references(
            entity_parent_start_index,
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        index.apply_deletion_record(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        crate::metrics::durable_stream::record_cascade(outcome);
        tracing::debug!(
            attachment_id = %attachment_id.0,
            stream_id = %stream_id,
            epoch,
            outcome,
            "Durable stream cascade outbox committed"
        );
        Ok(())
    }

    pub(super) async fn attachment_view(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<StreamAttachmentView, StreamStoreError> {
        self.index_for([ProducerMetadataKey::Attachment(
            key.attachment_id,
            key.stream_id,
            key.consumer_environment_id,
            key.consumer.clone(),
        )])
        .await?
        .attachment_views()
        .into_iter()
        .find(|view| view.key == *key)
        .ok_or(StreamStoreError::InvalidAttachmentState)
    }

    /// Reconciles expired or abandoned attachments from durable consumer evidence.
    pub async fn reconcile_attachments_configured(
        &self,
        now_millis: u64,
        batch_size: usize,
        probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
    ) -> Result<usize, StreamStoreError> {
        self.ensure_healthy()?;
        let IndexedAttachmentCandidateBatch {
            deleting,
            candidates,
        } = if let Some(candidates) = self.indexed_attachment_candidates(batch_size).await? {
            candidates
        } else {
            let index = self.index.lock().await;
            let deleting = index.deleting;
            let mut candidates = index
                .attachments
                .values()
                .filter(|attachment| {
                    !matches!(
                        attachment.state,
                        IndexedStreamAttachmentState::Finalized { .. }
                    )
                })
                .map(|attachment| {
                    let stream = index
                        .streams
                        .get(&attachment.key.stream_id)
                        .expect("durable attachment index points at a missing producer stream");
                    IndexedAttachmentCandidate {
                        attachment: attachment.clone(),
                        journal_summary: ProducerJournalSummary {
                            event_count: stream.next_sequence + u64::from(stream.terminal),
                            last_offset: stream.last_offset,
                            terminal: stream.terminal,
                        },
                    }
                })
                .collect::<Vec<_>>();
            candidates.sort_by_key(|candidate| {
                (
                    candidate.attachment.key.stream_id,
                    candidate.attachment.key.attachment_id,
                    candidate.attachment.key.epoch,
                )
            });
            if !candidates.is_empty() {
                let start = self
                    .reconciliation_cursor
                    .fetch_add(batch_size, Ordering::Relaxed)
                    % candidates.len();
                candidates.rotate_left(start);
            }
            candidates.truncate(batch_size);
            IndexedAttachmentCandidateBatch {
                deleting,
                candidates,
            }
        };
        let mut changed = 0;
        let mut first_error = None;
        for candidate in candidates {
            let attachment = candidate.attachment;
            let producer_summary = candidate.journal_summary;
            match &attachment.state {
                IndexedStreamAttachmentState::Prepared {
                    lease_expires_at_millis,
                    ..
                } => crate::metrics::durable_stream::record_lease_remaining(
                    lease_expires_at_millis.saturating_sub(now_millis),
                ),
                IndexedStreamAttachmentState::Active { .. }
                | IndexedStreamAttachmentState::Finalized { .. } => {}
            }
            let status = match probe.status(&attachment.key).await {
                Ok(status) => status,
                Err(error) => {
                    crate::metrics::durable_stream::record_reconciliation("probe_error");
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            };
            let journal_complete = if producer_summary.terminal {
                match probe.journal_summary(&attachment.key).await {
                    Ok(Some(consumer)) => {
                        // Receive journals contain ordered, non-repeating producer offsets.
                        // Equal logical counts and the same terminal therefore imply completion.
                        !consumer.source_unavailable
                            && consumer.terminal
                            && consumer.event_count == producer_summary.event_count
                            && consumer.last_offset == producer_summary.last_offset
                    }
                    Ok(None) => false,
                    Err(error) => {
                        crate::metrics::durable_stream::record_reconciliation("journal_error");
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                        continue;
                    }
                }
            } else {
                false
            };
            let action = if journal_complete {
                Some(ReconciliationAction::Finalize(
                    StreamAttachmentFinalizationReason::ConsumerFinalized,
                ))
            } else {
                match (attachment.state, status) {
                (
                    IndexedStreamAttachmentState::Prepared { .. },
                    ConsumerAttachmentStatus::Active,
                ) => Some(ReconciliationAction::Activate),
                (
                    IndexedStreamAttachmentState::Prepared {
                        prepared_at_millis,
                        ..
                    },
                    ConsumerAttachmentStatus::Missing,
                ) if now_millis.saturating_sub(prepared_at_millis)
                    >= golem_common::base_model::durable_stream::STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS =>
                {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReason::PrepareAbandoned,
                    ))
                }
                (_, ConsumerAttachmentStatus::Deleting) => {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReason::ConsumerDeleted,
                    ))
                }
                (_, ConsumerAttachmentStatus::IncarnationMismatch) => {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReason::ConsumerIncarnationChanged,
                    ))
                }
                (_, ConsumerAttachmentStatus::EpochMismatch) => Some(
                    ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReason::Reconciled,
                    ),
                ),
                (IndexedStreamAttachmentState::Active { .. }, ConsumerAttachmentStatus::Missing) => {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReason::ConsumerDeleted,
                    ))
                }
                _ => None,
                }
            };
            let action = match (deleting, action) {
                (true, Some(ReconciliationAction::Activate)) => None,
                (_, action) => action,
            };
            let action_outcome = match &action {
                Some(ReconciliationAction::Activate) => "activated",
                Some(ReconciliationAction::Finalize(_)) => "finalized",
                None => "unchanged",
            };
            let replayed = match action {
                Some(ReconciliationAction::Activate) => self
                    .activate_attachment(attachment.key, now_millis)
                    .await
                    .map(|outcome| outcome.replayed),
                Some(ReconciliationAction::Finalize(reason)) => self
                    .finalize_attachment(attachment.key, reason, now_millis)
                    .await
                    .map(|outcome| outcome.replayed),
                None => Ok(true),
            };
            let replayed = match replayed {
                Ok(replayed) => replayed,
                Err(error) => {
                    crate::metrics::durable_stream::record_reconciliation("write_error");
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            };
            crate::metrics::durable_stream::record_reconciliation(if replayed {
                "replayed"
            } else {
                action_outcome
            });
            if !replayed {
                changed += 1;
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(changed),
        }
    }
}

enum ReconciliationAction {
    Activate,
    Finalize(StreamAttachmentFinalizationReason),
}

pub(super) fn attachment_lease_expiry(now_millis: u64) -> Result<u64, StreamStoreError> {
    now_millis
        .checked_add(STREAM_ATTACHMENT_LEASE_TTL_MILLIS)
        .ok_or(StreamStoreError::CounterOverflow)
}
