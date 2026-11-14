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

use super::index::validate_terminal_sequence;
use super::*;

impl DurableStreamStore {
    pub(crate) async fn cancel_unbound_rpc_input(
        self: &Arc<Self>,
        session: &StreamSessionKey,
        handle: &DurableStreamHandle,
    ) -> Result<(), StreamStoreError> {
        if !self.owns_handle_identity(handle) {
            return Ok(());
        }
        let index = self
            .index_for([ProducerMetadataKey::Stream(handle.stream_id)])
            .await?;
        let registration = index
            .registrations
            .get(&handle.stream_id)
            .ok_or(StreamStoreError::UnknownStream(handle.stream_id))?;
        if !registration.accepts(handle, self.generation()) {
            return Err(StreamStoreError::InvalidHandle);
        }
        let owned_input = registration.source_kind == StreamSourceKind::AgentHostedInput
            && registration.handle.source_invocation == *session;
        drop(index);
        if owned_input {
            self.cancel_open(
                None,
                handle.stream_id,
                StreamTerminalAuthor::Host,
                StreamTerminal::Failed(StreamError::Interrupted("unbound RPC input".into())),
            )
            .await?;
            self.notify_session_records_changed(None);
        }
        Ok(())
    }

    pub(super) async fn commit_resource_exhausted_terminal(
        &self,
        context: &StreamWriteContext,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
    ) -> Result<(), StreamStoreError> {
        let terminal = StreamTerminal::Failed(StreamError::ResourceExhausted(
            "stream resource limit exceeded".into(),
        ));
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let local_stream_id = index.local_stream_id(stream_id)?;
        context.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Terminal(
                    entity_parent_start_index,
                    StreamTerminalRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id: local_stream_id,
                        sequence,
                        offset: StreamOffset::new(oplog_index, 0),
                        authored_by: StreamTerminalAuthor::Host,
                        origin: StreamTerminalOrigin::Control,
                        terminal,
                    },
                )]
            }))
            .await
            .map_err(StreamStoreError::from)?;
        self.commit(context).await?;
        let (oplog_index, entry) = entries
            .pop()
            .expect("resource exhaustion terminal batch returned no oplog entry");
        let OplogEntry::StreamTerminal {
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
            .map_err(StreamStoreError::Oplog)?;
        let event = index.apply_terminal(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let publication = self.enqueue_events(Some(context), stream_id, vec![event], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.wait_for_publication(Some(context), publication)
            .await?;
        context.finish_durable_effect();
        Ok(())
    }

    /// Commits the stream's single end terminal before publishing it to live readers.
    pub async fn end(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        sequence: u64,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, StreamStoreError> {
        let memory = terminal_retained_bytes(&terminal)?;
        self.run_owned(context, memory, move |owner, context| async move {
            owner
                .end_authored(&context, stream_id, sequence, terminal, authored_by)
                .await
        })
        .await
    }

    async fn end_authored(
        &self,
        context: &StreamWriteContext,
        stream_id: StreamId,
        sequence: u64,
        terminal: StreamTerminal,
        authored_by: StreamTerminalAuthor,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, StreamStoreError> {
        let index = self.index_for_terminal([], stream_id).await?;
        self.end_authored_locked(
            context,
            index,
            stream_id,
            sequence,
            terminal,
            authored_by,
            StreamTerminalOrigin::Source,
        )
        .await
    }

    #[tracing::instrument(
        name = "durable_stream.end",
        skip_all,
        fields(stream_id = %stream_id, sequence)
    )]
    async fn end_authored_locked(
        &self,
        context: &StreamWriteContext,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
        terminal: StreamTerminal,
        authored_by: StreamTerminalAuthor,
        origin: StreamTerminalOrigin,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, StreamStoreError> {
        match replay_terminal(
            &index,
            stream_id,
            sequence,
            &CommittedProducerStreamEventPayload::Terminal(terminal.clone()),
            authored_by,
            origin,
        )? {
            TerminalReplayDecision::Append => {}
            TerminalReplayDecision::Replayed(event) => {
                let offset = event.offset;
                drop(index);
                self.publish_repair(Some(context), stream_id, vec![event])
                    .await?;
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
                let error = StreamStoreError::FencedByTerminal(Box::new(event.clone()));
                drop(index);
                self.publish_repair(Some(context), stream_id, vec![event])
                    .await?;
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
            self.publish_repair(Some(context), stream_id, vec![terminal])
                .await?;
            return Err(error);
        }
        validate_new_terminal(&index, stream_id, sequence)?;
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let local_stream_id = index.local_stream_id(stream_id)?;
        context.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Terminal(
                    entity_parent_start_index,
                    StreamTerminalRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id: local_stream_id,
                        sequence,
                        offset: StreamOffset::new(oplog_index, 0),
                        authored_by,
                        origin,
                        terminal,
                    },
                )]
            }))
            .await
            .map_err(StreamStoreError::from)?;
        self.commit(context).await?;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream end batch returned no oplog entry");
        let OplogEntry::StreamTerminal {
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
            .map_err(StreamStoreError::Oplog)?;
        let event = index.apply_terminal(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let offset = event.offset;
        let publication = self.enqueue_events(Some(context), stream_id, vec![event], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.wait_for_publication(Some(context), publication)
            .await?;
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
        fields(stream_id = %stream_id, sequence)
    )]
    async fn commit_cancel_locked(
        &self,
        context: &StreamWriteContext,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
    ) -> Result<PendingCommittedCancellation, StreamStoreError> {
        reject_finished_cancellation(&terminal)?;
        let payload = CommittedProducerStreamEventPayload::Terminal(terminal.clone());
        match replay_terminal(
            &index,
            stream_id,
            sequence,
            &payload,
            authored_by,
            StreamTerminalOrigin::Control,
        )? {
            TerminalReplayDecision::Append => {}
            TerminalReplayDecision::Replayed(event) => {
                let offset = event.offset;
                let publication =
                    self.enqueue_events(Some(context), stream_id, vec![event.clone()], true)?;
                drop(index);
                if event.terminal_origin == Some(StreamTerminalOrigin::Control) {
                    self.cancel_source(stream_id);
                }
                return Ok(PendingCommittedCancellation {
                    stream_id,
                    sequence,
                    publication,
                    event,
                    outcome: Ok(ProducerWriteOutcome {
                        value: offset,
                        replayed: true,
                    }),
                });
            }
            TerminalReplayDecision::Fenced(event) => {
                let error = StreamStoreError::FencedByTerminal(Box::new(event.clone()));
                let publication =
                    self.enqueue_events(Some(context), stream_id, vec![event.clone()], true)?;
                drop(index);
                if event.terminal_origin == Some(StreamTerminalOrigin::Control) {
                    self.cancel_source(stream_id);
                }
                return Ok(PendingCommittedCancellation {
                    stream_id,
                    sequence,
                    publication,
                    event,
                    outcome: Err(error),
                });
            }
        }
        index.ensure_producer_write_allowed()?;
        validate_new_terminal(&index, stream_id, sequence)?;
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let local_stream_id = index.local_stream_id(stream_id)?;
        context.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Terminal(
                    entity_parent_start_index,
                    StreamTerminalRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id: local_stream_id,
                        sequence,
                        offset: StreamOffset::new(oplog_index, 0),
                        authored_by,
                        origin: StreamTerminalOrigin::Control,
                        terminal,
                    },
                )]
            }))
            .await
            .map_err(StreamStoreError::from)?;
        self.commit(context).await?;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream cancellation batch returned no oplog entry");
        let OplogEntry::StreamTerminal {
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
            .map_err(StreamStoreError::Oplog)?;
        let event = index.apply_terminal(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let offset = event.offset;
        let publication =
            self.enqueue_events(Some(context), stream_id, vec![event.clone()], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.cancel_source(stream_id);
        Ok(PendingCommittedCancellation {
            stream_id,
            sequence,
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
        context: Option<&StreamWriteContext>,
        pending: PendingCommittedCancellation,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, StreamStoreError> {
        let offset = pending.event.offset;
        self.wait_for_publication(context, pending.publication)
            .await?;
        if let Ok(outcome) = &pending.outcome {
            crate::metrics::durable_stream::record_producer_operation("cancel", outcome.replayed);
            tracing::debug!(
                stream_id = %pending.stream_id,
                sequence = pending.sequence,
                durable_offset = %offset,
                replayed = outcome.replayed,
                "Durable stream cancellation committed"
            );
        }
        pending.outcome
    }

    /// Commits cancellation for an open stream without yet completing live publication.
    pub(crate) async fn commit_cancel_open(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
    ) -> Result<Option<PendingCommittedCancellation>, StreamStoreError> {
        reject_finished_cancellation(&terminal)?;
        self.run_lifecycle(
            context,
            terminal_retained_bytes(&terminal)?,
            move |owner, context| async move {
                owner
                    .commit_cancel_open_owned(&context, stream_id, authored_by, terminal)
                    .await
            },
        )
        .await
    }

    async fn commit_cancel_open_owned(
        &self,
        context: &StreamWriteContext,
        stream_id: StreamId,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
    ) -> Result<Option<PendingCommittedCancellation>, StreamStoreError> {
        let index = self.index_for_terminal([], stream_id).await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        if stream.terminal {
            let control = stream
                .terminal_event
                .as_ref()
                .is_some_and(|event| event.terminal_origin == Some(StreamTerminalOrigin::Control));
            drop(index);
            if control {
                self.cancel_source(stream_id);
            }
            return Ok(None);
        }
        let sequence = stream.next_sequence;
        self.commit_cancel_locked(context, index, stream_id, sequence, authored_by, terminal)
            .await
            .map(Some)
    }

    /// Commits and publishes cancellation for an open stream exactly once.
    pub async fn cancel_open(
        self: &Arc<Self>,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
    ) -> Result<(), StreamStoreError> {
        reject_finished_cancellation(&terminal)?;
        self.run_lifecycle(
            context,
            terminal_retained_bytes(&terminal)?,
            move |owner, context| async move {
                owner
                    .cancel_open_owned(&context, stream_id, authored_by, terminal)
                    .await
            },
        )
        .await
    }

    async fn cancel_open_owned(
        &self,
        context: &StreamWriteContext,
        stream_id: StreamId,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
    ) -> Result<(), StreamStoreError> {
        if let Some(pending) = self
            .commit_cancel_open(Some(context), stream_id, authored_by, terminal)
            .await?
        {
            self.publish_committed_cancellation(Some(context), pending)
                .await?;
        }
        Ok(())
    }

    /// Commits and publishes a cancellation at the supplied producer sequence.
    #[cfg(test)]
    pub(crate) async fn cancel(
        &self,
        stream_id: StreamId,
        sequence: u64,
        authored_by: StreamTerminalAuthor,
        terminal: StreamTerminal,
    ) -> Result<ProducerWriteOutcome<StreamOffset>, StreamStoreError> {
        reject_finished_cancellation(&terminal)?;
        self.run_owned(
            None,
            terminal_retained_bytes(&terminal)?,
            move |owner, context| async move {
                let index = owner.index_for_terminal([], stream_id).await?;
                let pending = owner
                    .commit_cancel_locked(
                        &context,
                        index,
                        stream_id,
                        sequence,
                        authored_by,
                        terminal,
                    )
                    .await?;
                owner
                    .publish_committed_cancellation(Some(&context), pending)
                    .await
            },
        )
        .await
    }

    /// Installs a disposable signal used to stop active source work after durable cancellation.
    pub fn register_source_cancellation(
        &self,
        stream_id: StreamId,
        cancellation: CancellationToken,
    ) -> u64 {
        let registration_id = self
            .next_source_cancellation_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
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
    pub fn unregister_source_cancellation(&self, stream_id: StreamId, registration_id: u64) {
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

    /// Appends a terminal to one stream unless it already has a terminal.
    pub async fn end_open(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        authored_by: StreamTerminalAuthor,
        origin: StreamTerminalOrigin,
        terminal: StreamTerminal,
    ) -> Result<(), StreamStoreError> {
        let memory = terminal_retained_bytes(&terminal)?;
        self.run_lifecycle(context, memory, move |owner, context| async move {
            owner
                .end_open_owned(&context, stream_id, authored_by, origin, terminal)
                .await
        })
        .await
    }

    async fn end_open_owned(
        &self,
        context: &StreamWriteContext,
        stream_id: StreamId,
        authored_by: StreamTerminalAuthor,
        origin: StreamTerminalOrigin,
        terminal: StreamTerminal,
    ) -> Result<(), StreamStoreError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        if stream.terminal {
            return Ok(());
        }
        let sequence = stream.next_sequence;
        self.end_authored_locked(
            context,
            index,
            stream_id,
            sequence,
            terminal,
            authored_by,
            origin,
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
    expected_origin: StreamTerminalOrigin,
) -> Result<TerminalReplayDecision, StreamStoreError> {
    let stream = index
        .streams
        .get(&stream_id)
        .ok_or(StreamStoreError::UnknownStream(stream_id))?;
    if let Some(event) = &stream.terminal_event {
        if event.is_terminal()
            && event.producer_sequence == sequence
            && event.terminal_author == Some(expected_author)
            && event.terminal_origin == Some(expected_origin)
            && &event.payload == expected_payload
        {
            return Ok(TerminalReplayDecision::Replayed(event.clone()));
        }
        return Ok(TerminalReplayDecision::Fenced(event.clone()));
    }
    if sequence < stream.next_sequence {
        return Err(StreamStoreError::EventConflict);
    }
    Ok(TerminalReplayDecision::Append)
}

fn validate_new_terminal(
    index: &ProducerStreamIndex,
    stream_id: StreamId,
    sequence: u64,
) -> Result<(), StreamStoreError> {
    let stream = index
        .streams
        .get(&stream_id)
        .ok_or(StreamStoreError::UnknownStream(stream_id))?;
    validate_terminal_sequence(stream, stream_id, sequence)
}

pub(super) fn fenced_by_terminal(stream: &IndexedProducerStream) -> StreamStoreError {
    StreamStoreError::FencedByTerminal(Box::new(
        stream
            .terminal_event
            .as_ref()
            .expect("terminal stream has no terminal event")
            .clone(),
    ))
}

pub(super) fn terminal_retained_bytes(
    terminal: &StreamTerminal,
) -> Result<usize, StreamStoreError> {
    golem_common::serialization::serialize(terminal)
        .map(|bytes| bytes.len())
        .map_err(StreamStoreError::Oplog)
}

fn reject_finished_cancellation(terminal: &StreamTerminal) -> Result<(), StreamStoreError> {
    if matches!(terminal, StreamTerminal::Finished) {
        return Err(StreamStoreError::EventConflict);
    }
    Ok(())
}
