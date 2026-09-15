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

use super::index::{resource_exhausted_error_context, validate_terminal_sequence};
use super::*;

impl DurableStreamProducer {
    pub(super) async fn commit_resource_exhausted_terminal(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
    ) -> Result<(), DurableStreamProducerError> {
        let result = StreamEndResult::ErrorContext(resource_exhausted_error_context()?);
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let producer_fingerprint = self.producer_fingerprint;
        self.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::End(
                    entity_parent_start_index,
                    StreamEndRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        sequence,
                        offset: StreamOffset::new(oplog_index, 0),
                        authored_by: StreamTerminalAuthor::Protocol,
                        result,
                    },
                )]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("resource exhaustion terminal batch returned no oplog entry");
        let OplogEntry::StreamEnd {
            entity_parent_start_index,
            record,
            ..
        } = entry
        else {
            unreachable!("resource exhaustion terminal builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_end(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let publication = self.enqueue_events(stream_id, vec![event], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.wait_for_publication(publication).await?;
        self.finish_durable_effect();
        Ok(())
    }

    /// Commits the stream's single end terminal before publishing it to live readers.
    pub(crate) async fn end(
        &self,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResult,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, DurableStreamProducerError> {
        let memory = match &result {
            StreamEndResult::ErrorContext(bytes) => bytes.len(),
            _ => 0,
        };
        self.run_owned(memory, move |owner| async move {
            owner
                .end_authored(stream_id, sequence, result, StreamTerminalAuthor::Guest)
                .await
        })
        .await
    }

    async fn end_authored(
        &self,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResult,
        authored_by: StreamTerminalAuthor,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, DurableStreamProducerError> {
        let index = self.index_for_terminal([], stream_id).await?;
        self.end_authored_locked(index, stream_id, sequence, result, authored_by)
            .await
    }

    #[tracing::instrument(
        name = "durable_stream.end",
        skip_all,
        fields(stream_id = %stream_id, sequence)
    )]
    async fn end_authored_locked(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResult,
        authored_by: StreamTerminalAuthor,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, DurableStreamProducerError> {
        match replay_terminal(
            &index,
            stream_id,
            sequence,
            &CommittedProducerStreamEventPayload::End(result.clone()),
            authored_by,
        )? {
            TerminalReplayDecision::Append => {}
            TerminalReplayDecision::Replayed(event) => {
                let offset = event.offset;
                drop(index);
                self.publish_repair(stream_id, vec![event]).await?;
                crate::metrics::durable_stream::record_producer_operation("end", true);
                tracing::debug!(
                    stream_id = %stream_id,
                    sequence,
                    durable_offset = %offset,
                    replayed = true,
                    "Durable stream terminal resolved"
                );
                return Ok(ProducerWriteOutcome {
                    value: offset,
                    replayed: true,
                });
            }
            TerminalReplayDecision::Fenced(event) => {
                let error = DurableStreamProducerError::FencedByTerminal(event.payload.clone());
                drop(index);
                self.publish_repair(stream_id, vec![event]).await?;
                return Err(error);
            }
        }
        index.ensure_producer_write_allowed()?;
        let stream = index
            .streams
            .get(&stream_id)
            .expect("terminal replay validated the stream");
        if stream.terminal {
            let terminal = stream
                .terminal_event
                .as_ref()
                .expect("terminal stream has no terminal event")
                .clone();
            let error = fenced_by_terminal(stream);
            drop(index);
            self.publish_repair(stream_id, vec![terminal]).await?;
            return Err(error);
        }
        validate_new_terminal(&index, stream_id, sequence)?;
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let producer_fingerprint = self.producer_fingerprint;
        self.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::End(
                    entity_parent_start_index,
                    StreamEndRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        sequence,
                        offset: StreamOffset::new(oplog_index, 0),
                        authored_by,
                        result,
                    },
                )]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream end batch returned no oplog entry");
        let OplogEntry::StreamEnd {
            entity_parent_start_index,
            record,
            ..
        } = entry
        else {
            unreachable!("stream end builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_end(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let offset = event.offset;
        let publication = self.enqueue_events(stream_id, vec![event], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.wait_for_publication(publication).await?;
        crate::metrics::durable_stream::record_producer_operation("end", false);
        tracing::debug!(
            stream_id = %stream_id,
            sequence,
            durable_offset = %offset,
            replayed = false,
            "Durable stream terminal committed"
        );
        Ok(ProducerWriteOutcome {
            value: offset,
            replayed: false,
        })
    }

    #[tracing::instrument(
        name = "durable_stream.cancel",
        skip_all,
        fields(stream_id = %stream_id, sequence, role = ?role, reason = ?reason)
    )]
    async fn commit_cancel_locked(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    ) -> Result<PendingCommittedCancellation, DurableStreamProducerError> {
        let payload = CommittedProducerStreamEventPayload::Cancel {
            role,
            reason,
            details: details.clone(),
        };
        match replay_terminal(
            &index,
            stream_id,
            sequence,
            &payload,
            StreamTerminalAuthor::Protocol,
        )? {
            TerminalReplayDecision::Append => {}
            TerminalReplayDecision::Replayed(event) => {
                let offset = event.offset;
                let publication = self.enqueue_events(stream_id, vec![event.clone()], true)?;
                drop(index);
                self.cancel_source(stream_id);
                return Ok(PendingCommittedCancellation {
                    stream_id,
                    sequence,
                    role,
                    reason,
                    publication,
                    event,
                    outcome: Ok(ProducerWriteOutcome {
                        value: offset,
                        replayed: true,
                    }),
                });
            }
            TerminalReplayDecision::Fenced(event) => {
                let error = DurableStreamProducerError::FencedByTerminal(event.payload.clone());
                let publication = self.enqueue_events(stream_id, vec![event.clone()], true)?;
                drop(index);
                self.cancel_source(stream_id);
                return Ok(PendingCommittedCancellation {
                    stream_id,
                    sequence,
                    role,
                    reason,
                    publication,
                    event,
                    outcome: Err(error),
                });
            }
        }
        index.ensure_producer_write_allowed()?;
        validate_new_terminal(&index, stream_id, sequence)?;
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let producer_fingerprint = self.producer_fingerprint;
        self.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Cancel(
                    entity_parent_start_index,
                    StreamCancelRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        sequence,
                        offset: StreamOffset::new(oplog_index, 0),
                        authored_by: StreamTerminalAuthor::Protocol,
                        role,
                        reason,
                        details,
                    },
                )]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream cancellation batch returned no oplog entry");
        let OplogEntry::StreamCancel {
            entity_parent_start_index,
            record,
            ..
        } = entry
        else {
            unreachable!("stream cancellation builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_cancel(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let offset = event.offset;
        let publication = self.enqueue_events(stream_id, vec![event.clone()], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.cancel_source(stream_id);
        Ok(PendingCommittedCancellation {
            stream_id,
            sequence,
            role,
            reason,
            publication,
            event,
            outcome: Ok(ProducerWriteOutcome {
                value: offset,
                replayed: false,
            }),
        })
    }

    /// Publishes an already committed cancellation and waits for postcommit ordering.
    pub(crate) async fn publish_committed_cancellation(
        &self,
        pending: PendingCommittedCancellation,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, DurableStreamProducerError> {
        let offset = pending.event.offset;
        self.wait_for_publication(pending.publication).await?;
        if let Ok(outcome) = &pending.outcome {
            crate::metrics::durable_stream::record_producer_operation("cancel", outcome.replayed);
            tracing::debug!(
                stream_id = %pending.stream_id,
                sequence = pending.sequence,
                durable_offset = %offset,
                role = ?pending.role,
                reason = ?pending.reason,
                replayed = outcome.replayed,
                "Durable stream cancellation committed"
            );
        }
        pending.outcome
    }

    /// Commits cancellation for an open stream without yet completing live publication.
    pub(crate) async fn commit_cancel_open(
        &self,
        stream_id: StreamId,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    ) -> Result<Option<PendingCommittedCancellation>, DurableStreamProducerError> {
        self.run_lifecycle(
            details.as_ref().map_or(0, String::len),
            move |owner| async move {
                owner
                    .commit_cancel_open_owned(stream_id, role, reason, details)
                    .await
            },
        )
        .await
    }

    async fn commit_cancel_open_owned(
        &self,
        stream_id: StreamId,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    ) -> Result<Option<PendingCommittedCancellation>, DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        if stream.terminal {
            drop(index);
            self.cancel_source(stream_id);
            return Ok(None);
        }
        let sequence = stream.next_sequence;
        self.commit_cancel_locked(index, stream_id, sequence, role, reason, details)
            .await
            .map(Some)
    }

    /// Commits and publishes cancellation for an open stream exactly once.
    pub(crate) async fn cancel_open(
        self: &Arc<Self>,
        stream_id: StreamId,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    ) -> Result<(), DurableStreamProducerError> {
        self.run_lifecycle(
            details.as_ref().map_or(0, String::len),
            move |owner| async move {
                owner
                    .cancel_open_owned(stream_id, role, reason, details)
                    .await
            },
        )
        .await
    }

    async fn cancel_open_owned(
        &self,
        stream_id: StreamId,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    ) -> Result<(), DurableStreamProducerError> {
        if let Some(pending) = self
            .commit_cancel_open(stream_id, role, reason, details)
            .await?
        {
            self.publish_committed_cancellation(pending).await?;
        }
        Ok(())
    }

    /// Installs a disposable signal used to stop active source work after durable cancellation.
    pub(crate) fn register_source_cancellation(
        &self,
        stream_id: StreamId,
        cancellation: CancellationToken,
    ) -> u64 {
        let registration_id = self
            .next_source_cancellation_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("durable stream source cancellation registration IDs exhausted");
        let replaced = self
            .source_cancellations
            .write()
            .expect("durable stream source cancellation lock poisoned")
            .insert(stream_id, (registration_id, cancellation));
        if let Some((_, replaced)) = replaced {
            replaced.cancel();
        }
        registration_id
    }

    /// Removes the signal only if it still belongs to the registering source task.
    pub(crate) fn unregister_source_cancellation(&self, stream_id: StreamId, registration_id: u64) {
        let mut registrations = self
            .source_cancellations
            .write()
            .expect("durable stream source cancellation lock poisoned");
        if registrations
            .get(&stream_id)
            .is_some_and(|(current_id, _)| *current_id == registration_id)
        {
            registrations.remove(&stream_id);
        }
    }

    pub(super) fn cancel_source(&self, stream_id: StreamId) {
        if let Some(cancellation) = self
            .source_cancellations
            .read()
            .expect("durable stream source cancellation lock poisoned")
            .get(&stream_id)
        {
            cancellation.1.cancel();
        }
    }

    /// Appends a protocol-authored end to one stream unless it already has a terminal.
    pub(crate) async fn end_open(
        &self,
        stream_id: StreamId,
        result: StreamEndResult,
    ) -> Result<(), DurableStreamProducerError> {
        let memory = match &result {
            StreamEndResult::ErrorContext(bytes) => bytes.len(),
            _ => 0,
        };
        self.run_lifecycle(memory, move |owner| async move {
            owner.end_open_owned(stream_id, result).await
        })
        .await
    }

    async fn end_open_owned(
        &self,
        stream_id: StreamId,
        result: StreamEndResult,
    ) -> Result<(), DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        if stream.terminal {
            return Ok(());
        }
        let sequence = stream.next_sequence;
        self.end_authored_locked(
            index,
            stream_id,
            sequence,
            result,
            StreamTerminalAuthor::Protocol,
        )
        .await?;
        Ok(())
    }
}

enum TerminalReplayDecision {
    Append,
    Replayed(CommittedProducerStreamEvent),
    Fenced(CommittedProducerStreamEvent),
}

fn replay_terminal(
    index: &ProducerStreamIndex,
    stream_id: StreamId,
    sequence: u64,
    expected_payload: &CommittedProducerStreamEventPayload,
    expected_author: StreamTerminalAuthor,
) -> Result<TerminalReplayDecision, DurableStreamProducerError> {
    let stream = index
        .streams
        .get(&stream_id)
        .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
    if let Some(event) = &stream.terminal_event
        && event.producer_sequence == sequence
    {
        if event.is_terminal()
            && event.terminal_author == Some(expected_author)
            && &event.payload == expected_payload
        {
            return Ok(TerminalReplayDecision::Replayed(event.clone()));
        }
        if event.is_terminal()
            && event.terminal_author == Some(StreamTerminalAuthor::Protocol)
            && expected_author == StreamTerminalAuthor::Guest
        {
            return Ok(TerminalReplayDecision::Fenced(event.clone()));
        }
        return Err(DurableStreamProducerError::EventConflict);
    }
    if sequence < stream.next_sequence {
        return Err(DurableStreamProducerError::EventConflict);
    }
    Ok(TerminalReplayDecision::Append)
}

fn validate_new_terminal(
    index: &ProducerStreamIndex,
    stream_id: StreamId,
    sequence: u64,
) -> Result<(), DurableStreamProducerError> {
    let stream = index
        .streams
        .get(&stream_id)
        .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
    validate_terminal_sequence(stream, stream_id, sequence)
}

pub(super) fn fenced_by_terminal(stream: &IndexedProducerStream) -> DurableStreamProducerError {
    DurableStreamProducerError::FencedByTerminal(
        stream
            .terminal_event
            .as_ref()
            .expect("terminal stream has no terminal event")
            .payload
            .clone(),
    )
}
