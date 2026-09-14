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

use super::index::attachment_sort_key;
use super::*;

impl DurableStreamProducer {
    pub(crate) async fn commit_source_unavailable_overlay(
        &self,
        key: StreamAttachmentKey,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<bool, DurableStreamProducerError> {
        self.run_lifecycle(0, move |owner| async move {
            owner
                .commit_source_unavailable_overlay_owned(key, source_offset, consumer_read_ordinal)
                .await
        })
        .await
    }

    async fn commit_source_unavailable_overlay_owned(
        &self,
        key: StreamAttachmentKey,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<bool, DurableStreamProducerError> {
        if key.consumer_environment_id != self.environment_id
            || key.consumer != self.producer
            || key.expected_consumer_fingerprint != self.producer_fingerprint
        {
            return Err(DurableStreamProducerError::InvalidAttachmentState);
        }
        let mut index = self
            .index_for([ProducerMetadataKey::ConsumerHead(
                key.session_key.clone(),
                key.stream_id,
            )])
            .await?;
        let current = self.oplog.current_oplog_index().await;
        let mut source_offsets = Vec::new();
        let mut overlay = None;
        if current.is_defined() {
            for (_, entry) in self
                .oplog
                .read_exact(OplogIndex::INITIAL, current.as_u64())
                .await
            {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let record = self
                    .oplog
                    .download_payload(record)
                    .await
                    .map_err(DurableStreamProducerError::Oplog)?;
                match record {
                    StreamSessionRecord::ConsumerItemValue(record)
                        if record.session_key == key.session_key
                            && record.stream_id == key.stream_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "consumer value journal contains a read-ordinal gap".to_string(),
                            ));
                        }
                        for index in 0..record.logical_item_count() {
                            source_offsets.push(record.source_offset_at(index).ok_or_else(
                                || {
                                    DurableStreamProducerError::CorruptHistory(
                                        "packed-u8 consumer journal offset range is invalid"
                                            .to_string(),
                                    )
                                },
                            )?);
                        }
                    }
                    StreamSessionRecord::ConsumerTerminal(record)
                        if record.session_key == key.session_key
                            && record.stream_id == key.stream_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "consumer terminal journal contains a read-ordinal gap".to_string(),
                            ));
                        }
                        source_offsets.push(record.source_offset);
                    }
                    StreamSessionRecord::SourceUnavailable(record)
                        if record.key.session_key == key.session_key
                            && record.key.stream_id == key.stream_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "source-unavailable overlay contains a read-ordinal gap"
                                    .to_string(),
                            ));
                        }
                        match &overlay {
                            Some(existing) if existing != &record => {
                                return Err(DurableStreamProducerError::CorruptHistory(
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
            return if existing.key == key
                && existing.source_offset == source_offset
                && existing.consumer_read_ordinal == consumer_read_ordinal
            {
                Ok(true)
            } else {
                Err(DurableStreamProducerError::AttachmentConflict)
            };
        }
        if source_offsets.len() as u64 != consumer_read_ordinal {
            return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
        }
        let record = StreamSessionRecord::SourceUnavailable(StreamSourceUnavailableRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            key,
            source_offset,
            consumer_read_ordinal,
        });
        let entity_parent_start_index = index.session_entity_parent_start_index(
            crate::worker::stream_session_record_key(&record)
                .expect("source-unavailable record always identifies a session"),
        );
        index.apply_session_references(entity_parent_start_index, &record)?;
        index.apply_consumer_journal_record(&record)?;
        self.begin_durable_effect();
        self.oplog
            .add(OplogEntry::stream_session(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record)),
            ))
            .await;
        self.commit().await;
        self.notify_session_records_changed();
        Ok(false)
    }

    pub(crate) async fn consumer_source_unavailable(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<Option<StreamOffset>, DurableStreamProducerError> {
        if key.consumer_environment_id != self.environment_id
            || key.consumer != self.producer
            || key.expected_consumer_fingerprint != self.producer_fingerprint
            || key.consumer_invocation.callee_environment_id != self.environment_id
            || key.consumer_invocation.callee != self.producer
            || key.consumer_invocation.callee_fingerprint != self.producer_fingerprint
        {
            return Err(DurableStreamProducerError::InvalidAttachmentState);
        }
        let index = self
            .index_for([ProducerMetadataKey::ConsumerHead(
                key.session_key.clone(),
                key.stream_id,
            )])
            .await?;
        Ok(index
            .consumer_journals
            .get(&(key.session_key.clone(), key.stream_id))
            .and_then(|journal| journal.source_unavailable.as_ref())
            .and_then(|(recorded_key, offset)| (recorded_key == key).then_some(*offset)))
    }

    async fn persist_attachment_record(
        &self,
        record: StreamSessionRecord,
    ) -> Result<AttachmentApplyOutcome, DurableStreamProducerError> {
        self.run_lifecycle(0, move |owner| async move {
            owner.persist_attachment_record_owned(record).await
        })
        .await
    }

    async fn persist_attachment_record_owned(
        &self,
        record: StreamSessionRecord,
    ) -> Result<AttachmentApplyOutcome, DurableStreamProducerError> {
        if !record.has_supported_format() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "unsupported or malformed durable attachment record".to_string(),
            ));
        }
        let mut index = self
            .index_for(ProducerMetadataKey::session_record(&record))
            .await?;
        let stream_id = match &record {
            StreamSessionRecord::AttachmentPrepared(record) => record.key.stream_id,
            StreamSessionRecord::AttachmentActivated(record) => record.key.stream_id,
            StreamSessionRecord::AttachmentRenewed(record) => record.key.stream_id,
            StreamSessionRecord::AttachmentFinalized(record) => record.key.stream_id,
            _ => unreachable!("attachment persistence received a non-attachment record"),
        };
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let mut updated = index.clone();
        updated.apply_session_references(entity_parent_start_index, &record)?;
        let outcome = updated.apply_attachment_record(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        if outcome == AttachmentApplyOutcome::Changed {
            self.begin_durable_effect();
            self.oplog
                .add(OplogEntry::stream_session(
                    entity_parent_start_index,
                    OplogPayload::Inline(Box::new(record)),
                ))
                .await;
            self.commit().await;
            *index = updated;
        }
        drop(index);
        if outcome == AttachmentApplyOutcome::Changed {
            self.notify_session_records_changed();
        }
        Ok(outcome)
    }
}

#[async_trait]
impl StreamAttachmentControl for DurableStreamProducer {
    #[tracing::instrument(
        name = "durable_stream.attachment.prepare",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch)
    )]
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
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
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
        let lease_expires_at_millis = attachment_lease_expiry(now_millis)?;
        let outcome = self
            .persist_attachment_record(StreamSessionRecord::AttachmentActivated(
                StreamAttachmentActivatedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    activated_at_millis: now_millis,
                    lease_expires_at_millis,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "activate",
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

    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<StreamAttachmentView, DurableStreamProducerError> {
        let view = self.attachment_view(key).await?;
        if !matches!(view.state, StreamAttachmentState::Active) {
            return Err(DurableStreamProducerError::InvalidAttachmentState);
        }
        Ok(view)
    }

    #[tracing::instrument(
        name = "durable_stream.attachment.renew",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch)
    )]
    async fn renew_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
        let lease_expires_at_millis = attachment_lease_expiry(now_millis)?;
        let outcome = self
            .persist_attachment_record(StreamSessionRecord::AttachmentRenewed(
                StreamAttachmentRenewedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    renewed_at_millis: now_millis,
                    lease_expires_at_millis,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "renew",
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
        name = "durable_stream.attachment.finalize",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch, reason = ?reason)
    )]
    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKey,
        reason: StreamAttachmentFinalizationReason,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
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

impl DurableStreamProducer {
    pub(crate) async fn has_active_attachment(
        &self,
        session: &StreamSessionKey,
        handle: &DurableStreamHandle,
    ) -> Result<bool, DurableStreamProducerError> {
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
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        Ok(index
            .active_attachments_by_session_stream
            .get(&(session.clone(), handle.stream_id))
            .copied()
            .unwrap_or_default()
            > 0)
    }

    pub(crate) async fn deletion_started(&self) -> bool {
        let index = self.index.lock().await;
        index.deleting || index.consumer_deleting
    }

    pub(crate) async fn deletion_diagnostics(
        &self,
    ) -> Result<StreamDeletionDiagnostics, DurableStreamProducerError> {
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

    pub(crate) async fn cascade_deletion(
        &self,
        now_millis: u64,
        probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
    ) -> Result<(), DurableStreamProducerError> {
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
                    let inspection = probe
                        .journal_inspection(&key)
                        .await?
                        .ok_or(DurableStreamProducerError::InvalidAttachmentState)?;
                    let producer_offsets = {
                        let index = self.index.lock().await;
                        index
                            .streams
                            .get(&key.stream_id)
                            .ok_or(DurableStreamProducerError::UnknownStream(key.stream_id))?
                            .offsets()
                    };
                    if inspection.source_offsets.len() > producer_offsets.len()
                        || producer_offsets[..inspection.source_offsets.len()]
                            != inspection.source_offsets
                    {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "consumer journal is not an exact prefix of producer history"
                                .to_string(),
                        ));
                    }
                    if inspection.source_offsets.len() == producer_offsets.len() {
                        StreamCascadeDependentResult::ConsumerJournalComplete
                    } else {
                        let first_unjournaled_offset =
                            producer_offsets[inspection.source_offsets.len()];
                        if let Some(existing) = inspection.source_unavailable {
                            if existing != first_unjournaled_offset {
                                return Err(DurableStreamProducerError::CorruptHistory(
                                    "source-unavailable overlay does not identify the first unjournaled producer position"
                                        .to_string(),
                                ));
                            }
                        } else {
                            probe
                                .commit_source_unavailable(
                                    &key,
                                    first_unjournaled_offset,
                                    inspection.source_offsets.len() as u64,
                                )
                                .await?;
                        }
                        StreamCascadeDependentResult::SourceUnavailable {
                            first_unjournaled_offset,
                        }
                    }
                }
            };
            self.commit_cascade_outbox(key, now_millis, result).await?;
        }
        let incomplete = self.index.lock().await.incomplete_cascade_dependents();
        if incomplete.is_empty() {
            Ok(())
        } else {
            Err(DurableStreamProducerError::DeletionBlocked(incomplete))
        }
    }

    pub(super) async fn commit_deletion_barrier(
        &self,
        now_millis: u64,
        require_no_dependents: bool,
    ) -> Result<(), DurableStreamProducerError> {
        self.run_lifecycle(0, move |owner| async move {
            owner
                .commit_deletion_barrier_owned(now_millis, require_no_dependents)
                .await
        })
        .await
    }

    async fn commit_deletion_barrier_owned(
        &self,
        now_millis: u64,
        require_no_dependents: bool,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index_for_cleanup().await?;
        if index.deleting {
            crate::metrics::durable_stream::record_producer_operation("deletion_barrier", true);
            return Ok(());
        }
        if require_no_dependents {
            let dependents = index.live_dependents();
            if !dependents.is_empty() {
                return Err(DurableStreamProducerError::DeletionBlocked(dependents));
            }
        }
        let mut open_streams = index
            .streams
            .iter()
            .filter_map(|(stream_id, stream)| {
                (!stream.terminal).then_some((
                    *stream_id,
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
        self.begin_durable_effect();
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
                            producer_fingerprint,
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
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
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
                        .map_err(DurableStreamProducerError::Oplog)?;
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
                        .map_err(DurableStreamProducerError::Oplog)?;
                    index.apply_deletion_record(
                        &record,
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?;
                }
                _ => {
                    return Err(DurableStreamProducerError::CorruptHistory(
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
            drop(self.enqueue_events(event.stream_id, vec![event], false)?);
        }
        self.record_terminal_streams(terminal_count);
        drop(index);
        crate::metrics::durable_stream::record_producer_operation("deletion_barrier", false);
        tracing::debug!(
            terminal_streams = terminal_count,
            "Durable stream producer deletion barrier committed"
        );
        self.notify_session_records_changed();
        Ok(())
    }

    async fn commit_cascade_outbox(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
        result: StreamCascadeDependentResult,
    ) -> Result<(), DurableStreamProducerError> {
        self.run_lifecycle(0, move |owner| async move {
            owner
                .commit_cascade_outbox_owned(key, now_millis, result)
                .await
        })
        .await
    }

    async fn commit_cascade_outbox_owned(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
        result: StreamCascadeDependentResult,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index.lock().await;
        self.ensure_healthy()?;
        if let Some(existing) = index.cascade_outbox.get(&key) {
            return if existing == &result {
                crate::metrics::durable_stream::record_cascade("replayed");
                Ok(())
            } else {
                Err(DurableStreamProducerError::CorruptHistory(
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
        self.begin_durable_effect();
        self.oplog
            .add(OplogEntry::stream_session(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record.clone())),
            ))
            .await;
        self.commit().await;
        index.apply_session_references(entity_parent_start_index, &record)?;
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
    ) -> Result<StreamAttachmentView, DurableStreamProducerError> {
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
        .ok_or(DurableStreamProducerError::InvalidAttachmentState)
    }

    pub(crate) async fn reconcile_attachments_configured(
        &self,
        now_millis: u64,
        renewal_target_millis: u64,
        batch_size: usize,
        probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
    ) -> Result<usize, DurableStreamProducerError> {
        self.ensure_healthy()?;
        let (deleting, candidates) =
            if let Some(candidates) = self.indexed_attachment_candidates(batch_size).await? {
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
                        (
                            attachment.clone(),
                            ProducerJournalSummary {
                                event_count: stream.next_sequence + u64::from(stream.terminal),
                                last_offset: stream.last_offset,
                                terminal: stream.terminal,
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                candidates.sort_by_key(|(attachment, _)| {
                    (
                        attachment.key.stream_id,
                        attachment.key.attachment_id,
                        attachment.key.epoch,
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
                (deleting, candidates)
            };
        let mut changed = 0;
        let mut first_error = None;
        for (attachment, producer_summary) in candidates {
            match &attachment.state {
                IndexedStreamAttachmentState::Prepared {
                    lease_expires_at_millis,
                    ..
                }
                | IndexedStreamAttachmentState::Active {
                    lease_expires_at_millis,
                    ..
                } => crate::metrics::durable_stream::record_lease_remaining(
                    lease_expires_at_millis.saturating_sub(now_millis),
                ),
                IndexedStreamAttachmentState::Finalized { .. } => {}
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
                (
                    IndexedStreamAttachmentState::Active {
                        activated_at_millis,
                        ..
                    },
                    ConsumerAttachmentStatus::Active,
                ) if now_millis.saturating_sub(activated_at_millis)
                    >= renewal_target_millis =>
                {
                    Some(ReconciliationAction::Renew)
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
                (true, Some(ReconciliationAction::Activate | ReconciliationAction::Renew)) => None,
                (_, action) => action,
            };
            let action_outcome = match &action {
                Some(ReconciliationAction::Activate) => "activated",
                Some(ReconciliationAction::Renew) => "renewed",
                Some(ReconciliationAction::Finalize(_)) => "finalized",
                None => "unchanged",
            };
            let replayed = match action {
                Some(ReconciliationAction::Activate) => self
                    .activate_attachment(attachment.key, now_millis)
                    .await
                    .map(|outcome| outcome.replayed),
                Some(ReconciliationAction::Renew) => self
                    .renew_attachment(attachment.key, now_millis)
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
    Renew,
    Finalize(StreamAttachmentFinalizationReason),
}

pub(super) fn attachment_lease_expiry(now_millis: u64) -> Result<u64, DurableStreamProducerError> {
    now_millis
        .checked_add(STREAM_ATTACHMENT_LEASE_TTL_MILLIS)
        .ok_or(DurableStreamProducerError::CounterOverflow)
}
