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

impl DurableStreamStore {
    /// Loads a producer index from durable metadata, falling back to committed oplog records.
    pub(crate) async fn load_indexed_with_commit(
        oplog: Arc<dyn Oplog>,
        owner: OwnedAgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: Option<usize>,
        commit: DurableStreamCommit,
        service: Arc<dyn WorkerService>,
        mode: AgentMode,
    ) -> Result<Arc<Self>, StreamStoreError> {
        let live_join_capacity = live_join_capacity.unwrap_or(DEFAULT_LIVE_JOIN_BUFFER_SIZE);
        DurableLiveStreamBus::<CommittedProducerStreamEvent>::new(live_join_capacity)?;
        let (_, mut rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                mode,
                producer_fingerprint,
                vec![ProducerMetadataKey::Global, ProducerMetadataKey::Lineage],
            )
            .await
            .map_err(StreamStoreError::from)?;
        if rows.len() != 2 {
            return Err(StreamStoreError::CorruptHistory(
                "producer metadata lookup returned an incorrect row count".into(),
            ));
        }
        let mut index = ProducerStreamIndex::default();
        let Some(ProducerMetadataRow::Lineage(lineage)) = rows.pop().flatten() else {
            return Err(StreamStoreError::CorruptHistory(
                "producer metadata lookup did not return fork lineage".into(),
            ));
        };
        index.fork_lineage = lineage;
        if let Some(row) = rows.pop().flatten() {
            index
                .hydrate_metadata_row(ProducerMetadataKey::Global, row)
                .map_err(StreamStoreError::CorruptHistory)?;
        }
        index.loaded_metadata.insert(ProducerMetadataKey::Global);
        let producer = Self::from_index(
            oplog,
            owner.environment_id,
            owner.agent_id,
            producer_fingerprint,
            live_join_capacity,
            commit,
            index,
        )?;
        producer.set_control_metadata_provider(service, mode, producer_fingerprint);
        Ok(producer)
    }

    pub(in crate::durable_host::durable_stream) async fn index_for(
        &self,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        self.index_for_query(keys.into_iter().collect(), None, None)
            .await
    }

    pub(in crate::durable_host::durable_stream) async fn index_for_terminal(
        &self,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
        stream: StreamId,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.push(ProducerMetadataKey::Stream(stream));
        self.index_for_query(keys, Some(stream), None).await
    }

    pub(in crate::durable_host::durable_stream) async fn index_for_session_streams(
        &self,
        session: &StreamSessionKey,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        self.index_for_query(
            vec![ProducerMetadataKey::Session(session.clone())],
            None,
            Some(session.clone()),
        )
        .await
    }

    fn query_keys(
        &self,
        index: &ProducerStreamIndex,
        keys: &[ProducerMetadataKey],
        session: Option<&StreamSessionKey>,
    ) -> HashSet<ProducerMetadataKey> {
        let mut pending = keys.to_vec();
        pending.push(ProducerMetadataKey::Global);
        if let Some(session) = session {
            pending.extend(
                index
                    .session_stream_mappings
                    .get(session)
                    .into_iter()
                    .flatten()
                    .filter_map(|(source, _)| match source {
                        StreamRecordReference::Local(local_id) => qualify_local_stream(
                            *local_id,
                            self.environment_id,
                            &self.producer,
                            self.producer_fingerprint,
                        )
                        .ok()
                        .map(ProducerMetadataKey::Stream),
                        StreamRecordReference::Foreign(_) => None,
                    }),
            );
            pending.extend(
                index
                    .open_session_streams
                    .get(session)
                    .into_iter()
                    .flatten()
                    .map(|id| ProducerMetadataKey::Stream(*id)),
            );
        }
        let mut required = HashSet::new();
        while let Some(key) = pending.pop() {
            if !required.insert(key.clone()) {
                continue;
            }
            match key {
                ProducerMetadataKey::Batch(stream, _)
                | ProducerMetadataKey::Attachment(_, stream, _, _) => {
                    pending.push(ProducerMetadataKey::Stream(stream))
                }
                ProducerMetadataKey::Stream(stream) => pending.extend(
                    index
                        .stream_sessions
                        .get(&stream)
                        .cloned()
                        .map(ProducerMetadataKey::Session),
                ),
                ProducerMetadataKey::Coordinate(coordinate) => pending.extend(
                    index
                        .coordinates
                        .get(&coordinate)
                        .copied()
                        .map(ProducerMetadataKey::Stream),
                ),
                _ => {}
            }
        }
        required
    }

    fn key_ready(index: &ProducerStreamIndex, key: &ProducerMetadataKey) -> bool {
        index.loaded_metadata.contains(key)
            || match key {
                ProducerMetadataKey::Stream(stream) => index.streams.contains_key(stream),
                ProducerMetadataKey::Batch(stream, sequence) => {
                    index.streams.get(stream).is_some_and(|state| {
                        *sequence >= state.next_sequence || state.batches.contains_key(sequence)
                    })
                }
                ProducerMetadataKey::Position(stream, offset) => {
                    index.batch_positions.contains_key(&(*stream, *offset))
                }
                ProducerMetadataKey::ExternalProducerSequence(
                    session,
                    stream,
                    producer,
                    epoch,
                    sequence,
                ) => {
                    index.external_producer_offsets.contains_key(&(
                        session.clone(),
                        *stream,
                        producer.clone(),
                        *epoch,
                        *sequence,
                    )) || index
                        .external_producer_heads
                        .get(&(session.clone(), *stream, producer.clone()))
                        .is_some_and(|head| {
                            *epoch > head.epoch
                                || *epoch == head.epoch && *sequence >= head.next_sequence
                        })
                        || index.loaded_metadata.contains(
                            &ProducerMetadataKey::ExternalProducerHead(
                                session.clone(),
                                *stream,
                                producer.clone(),
                            ),
                        ) && !index.external_producer_heads.contains_key(&(
                            session.clone(),
                            *stream,
                            producer.clone(),
                        ))
                }
                _ => false,
            }
    }

    #[tracing::instrument(name = "durable_stream.metadata.query", level = "debug", skip_all)]
    async fn index_for_query(
        &self,
        keys: Vec<ProducerMetadataKey>,
        terminal: Option<StreamId>,
        session: Option<StreamSessionKey>,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        loop {
            self.ensure_healthy()?;
            if let Ok(index) = self.index.try_lock() {
                self.ensure_healthy()?;
                let required = self.query_keys(&index, &keys, session.as_ref());
                // A query's complete dependency set must fit even when it exceeds the cache budget.
                let budget = 128.max(required.len());
                let bounded = index.loaded_metadata.len() <= budget
                    && index.batch_positions.len() <= budget
                    && index.streams.len() <= budget;
                let metadata_ready = self.control_metadata_provider.get().is_none()
                    || index.complete_for_deletion
                    || bounded && required.iter().all(|key| Self::key_ready(&index, key));
                let terminal_ready = terminal.is_none_or(|id| {
                    index
                        .streams
                        .get(&id)
                        .is_none_or(|state| !state.terminal || state.terminal_event.is_some())
                });
                if metadata_ready && terminal_ready {
                    return Ok(index);
                }
            } else {
                // A store-polled waiter must not reserve the mutex permit. Enqueuing another
                // hydration task here can also keep competing callers permanently queued.
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                continue;
            }
            let producer = self
                .self_weak
                .upgrade()
                .expect("live producer has an owning Arc");
            // Never return a guard in the task result: a suspended store may stop polling
            // its JoinHandle indefinitely. The task owns and releases all hydration locks.
            self.tasks()
                .spawn_metadata(producer.hydrate_query(keys.clone(), terminal, session.clone()))
                .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
                .await
                .map_err(|error| StreamStoreError::Oplog(error.to_string()))??;
        }
    }

    pub(super) async fn hydrate_query(
        self: Arc<Self>,
        keys: Vec<ProducerMetadataKey>,
        terminal: Option<StreamId>,
        session: Option<StreamSessionKey>,
    ) -> Result<(), StreamStoreError> {
        let mut index = self.index.lock().await;
        if !index.complete_for_deletion
            && self.control_metadata_provider.get().is_some()
            && (index.loaded_metadata.len() > 128
                || index.batch_positions.len() > 128
                || index.streams.len() > 128)
        {
            *index = ProducerStreamIndex::default();
            self.buses
                .write()
                .expect("durable stream bus map lock poisoned")
                .retain(|_, bus| Arc::strong_count(bus) > 1 || bus.has_pending_deferred_terminal());
        }
        self.load_index_keys(&mut index, keys.clone()).await?;
        let required = self.query_keys(&index, &keys, session.as_ref());
        self.load_index_keys(&mut index, required).await?;
        if let Some(stream) = terminal {
            self.load_terminal(&mut index, stream).await?;
        }
        Ok(())
    }

    pub(in crate::durable_host::durable_stream) async fn index_for_cleanup(
        &self,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        let mut index = self.index.lock().await;
        self.ensure_healthy()?;
        if self.control_metadata_provider.get().is_some() && !index.complete_for_deletion {
            let mut complete = Self::read_complete_index(
                self.oplog.as_ref(),
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
                self.oplog.current_oplog_index().await,
            )
            .await?;
            complete.complete_for_deletion = complete.deleting;
            for (stream, state) in &complete.streams {
                self.buses
                    .write()
                    .expect("durable stream bus map lock poisoned")
                    .entry(*stream)
                    .or_insert(Arc::new(DurableLiveStreamBus::from_committed_high_water(
                        self.live_join_capacity,
                        state.last_offset,
                    )?));
            }
            *index = complete;
        }
        Ok(index)
    }

    pub(in crate::durable_host::durable_stream) async fn load_index_keys(
        &self,
        index: &mut ProducerStreamIndex,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
    ) -> Result<(), StreamStoreError> {
        if index.complete_for_deletion {
            return Ok(());
        }
        let result = self.try_load_index_keys(index, keys).await;
        if result.is_err() {
            *index = ProducerStreamIndex::default();
            self.buses
                .write()
                .expect("durable stream bus map lock poisoned")
                .retain(|_, bus| Arc::strong_count(bus) > 1 || bus.has_pending_deferred_terminal());
        }
        result
    }

    async fn try_load_index_keys(
        &self,
        index: &mut ProducerStreamIndex,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
    ) -> Result<(), StreamStoreError> {
        let Some((service, mode, fingerprint)) = self.control_metadata_provider.get() else {
            return Ok(());
        };
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let mut pending: Vec<_> = keys.into_iter().collect();
        pending.push(ProducerMetadataKey::Global);
        while !pending.is_empty() {
            let mut visiting = std::mem::take(&mut pending);
            let mut seen = HashSet::new();
            let mut batch = Vec::new();
            while let Some(key) = visiting.pop() {
                if !seen.insert(key.clone()) {
                    continue;
                }
                let dependency = match &key {
                    ProducerMetadataKey::Batch(stream, _)
                    | ProducerMetadataKey::Attachment(_, stream, _, _) => {
                        Some(ProducerMetadataKey::Stream(*stream))
                    }
                    ProducerMetadataKey::Stream(stream) => index
                        .stream_sessions
                        .get(stream)
                        .cloned()
                        .map(ProducerMetadataKey::Session),
                    ProducerMetadataKey::Coordinate(coordinate) => index
                        .coordinates
                        .get(coordinate)
                        .copied()
                        .map(ProducerMetadataKey::Stream),
                    ProducerMetadataKey::ExternalProducerSequence(
                        session,
                        stream,
                        producer,
                        _,
                        _,
                    ) => Some(ProducerMetadataKey::ExternalProducerHead(
                        session.clone(),
                        *stream,
                        producer.clone(),
                    )),
                    _ => None,
                };
                if let Some(dependency) = dependency
                    && !Self::key_ready(index, &dependency)
                {
                    pending.push(key);
                    visiting.push(dependency);
                    continue;
                }
                if Self::key_ready(index, &key) {
                    continue;
                }
                if let ProducerMetadataKey::Position(stream, offset) = &key
                    && index.batch_positions.contains_key(&(*stream, *offset))
                {
                    index.loaded_metadata.insert(key);
                    continue;
                }
                batch.push(key);
            }
            if batch.is_empty() {
                continue;
            }
            let (_, rows) = service
                .lookup_durable_stream_producer_metadata(&owner, *mode, *fingerprint, batch.clone())
                .await
                .map_err(StreamStoreError::from)?;
            if rows.len() != batch.len() {
                return Err(StreamStoreError::CorruptHistory(
                    "producer metadata lookup returned an incorrect row count".into(),
                ));
            }
            for (key, row) in batch.into_iter().zip(rows) {
                if let Some(row) = row {
                    match &row {
                        ProducerMetadataRow::Coordinate(stream) => {
                            pending.push(ProducerMetadataKey::Stream(*stream));
                        }
                        ProducerMetadataRow::Stream(stream) => {
                            pending.push(ProducerMetadataKey::Session(stream.session_key.clone()));
                            self.buses
                                .write()
                                .expect("durable stream bus map lock poisoned")
                                .entry(stream.registration.handle.stream_id)
                                .or_insert(Arc::new(
                                    DurableLiveStreamBus::from_committed_high_water(
                                        self.live_join_capacity,
                                        stream.last_offset,
                                    )?,
                                ));
                        }
                        _ => {}
                    }
                    index
                        .hydrate_metadata_row(key.clone(), row)
                        .map_err(StreamStoreError::CorruptHistory)?;
                }
                index.loaded_metadata.insert(key);
            }
        }
        Ok(())
    }

    pub(in crate::durable_host::durable_stream) async fn indexed_attachment_candidates(
        &self,
        batch_size: usize,
    ) -> Result<Option<IndexedAttachmentCandidateBatch>, StreamStoreError> {
        let producer = self
            .self_weak
            .upgrade()
            .expect("live producer has an owning Arc");
        self.tasks()
            .spawn_metadata(async move {
                producer
                    .indexed_attachment_candidates_inner(batch_size)
                    .await
            })
            .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
            .await
            .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
    }

    pub(super) async fn indexed_attachment_candidates_inner(
        &self,
        batch_size: usize,
    ) -> Result<Option<IndexedAttachmentCandidateBatch>, StreamStoreError> {
        let Some((service, mode, fingerprint)) = self.control_metadata_provider.get() else {
            return Ok(None);
        };
        let _guard = self.index.lock().await;
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let (_, mut rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                *mode,
                *fingerprint,
                vec![ProducerMetadataKey::Global],
            )
            .await
            .map_err(StreamStoreError::from)?;
        let Some(ProducerMetadataRow::Global {
            active_attachments: count,
            deleting,
            ..
        }) = rows.pop().flatten()
        else {
            return Ok(Some(IndexedAttachmentCandidateBatch {
                deleting: false,
                candidates: Vec::new(),
            }));
        };
        if count == 0 || batch_size == 0 {
            return Ok(Some(IndexedAttachmentCandidateBatch {
                deleting,
                candidates: Vec::new(),
            }));
        }
        let start = self
            .reconciliation_cursor
            .fetch_add(batch_size, Ordering::Relaxed) as u64
            % count;
        let positions = (0..(batch_size as u64).min(count))
            .map(|n| (start + n) % count)
            .collect::<Vec<_>>();
        let pages = positions
            .iter()
            .map(|n| n / ATTACHMENT_PAGE_SIZE)
            .collect::<std::collections::BTreeSet<_>>();
        let (_, rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                *mode,
                *fingerprint,
                pages
                    .iter()
                    .map(|n| ProducerMetadataKey::AttachmentPage(*n))
                    .collect(),
            )
            .await
            .map_err(StreamStoreError::from)?;
        let mut catalogue = HashMap::new();
        for (page, row) in pages.into_iter().zip(rows) {
            let Some(ProducerMetadataRow::AttachmentPage(slots)) = row else {
                return Err(StreamStoreError::CorruptHistory(
                    "active attachment catalogue page is missing".into(),
                ));
            };
            catalogue.insert(page, slots);
        }
        let mut keys = Vec::new();
        for position in positions {
            let (attachment, stream, environment, consumer) = catalogue
                .get(&(position / ATTACHMENT_PAGE_SIZE))
                .and_then(|page| page.get((position % ATTACHMENT_PAGE_SIZE) as usize))
                .ok_or_else(|| {
                    StreamStoreError::CorruptHistory(
                        "active attachment catalogue slot is missing".into(),
                    )
                })?;
            keys.push(ProducerMetadataKey::Attachment(
                *attachment,
                *stream,
                *environment,
                consumer.clone(),
            ));
            keys.push(ProducerMetadataKey::Stream(*stream));
        }
        let (_, rows) = service
            .lookup_durable_stream_producer_metadata(&owner, *mode, *fingerprint, keys)
            .await
            .map_err(StreamStoreError::from)?;
        let mut candidates = Vec::new();
        for pair in rows.as_chunks::<2>().0 {
            let [
                Some(ProducerMetadataRow::Attachment(attachment)),
                Some(ProducerMetadataRow::Stream(stream)),
            ] = pair
            else {
                return Err(StreamStoreError::CorruptHistory(
                    "active attachment catalogue refers to missing metadata".into(),
                ));
            };
            candidates.push(IndexedAttachmentCandidate {
                attachment: attachment.as_ref().clone(),
                journal_summary: ProducerJournalSummary {
                    event_count: stream.next_sequence + u64::from(stream.terminal),
                    last_offset: stream.last_offset,
                    terminal: stream.terminal,
                },
            });
        }
        Ok(Some(IndexedAttachmentCandidateBatch {
            deleting,
            candidates,
        }))
    }

    /// Counts committed source events beyond the consumer's last journaled offset.
    pub async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<usize, StreamStoreError> {
        let mut keys = vec![ProducerMetadataKey::Stream(handle.stream_id)];
        let owns_handle = self.owns_handle_identity(handle);
        if !owns_handle {
            keys.push(ProducerMetadataKey::Lineage);
        }
        if let Some(after) = after {
            keys.push(ProducerMetadataKey::Position(
                handle.stream_id,
                after.producer_oplog_index(),
            ));
        }
        let (stream, producer_generation, position) = if owns_handle {
            let index = self.index_for(keys).await?;
            let Some(ProducerMetadataRow::Stream(stream)) =
                index.metadata_row(&ProducerMetadataKey::Stream(handle.stream_id))
            else {
                return Err(StreamStoreError::InvalidHandle);
            };
            let position = after.and_then(|after| {
                index
                    .batch_positions
                    .get(&(handle.stream_id, after.producer_oplog_index()))
                    .copied()
            });
            (stream, self.generation(), position)
        } else {
            let (service, _, _) = self.control_metadata_provider.get().ok_or_else(|| {
                StreamStoreError::Oplog("producer metadata service is unavailable".into())
            })?;
            let owner = OwnedAgentId::new(handle.producer_environment_id, &handle.producer);
            let identity = service
                .resolve_agent_identity(&owner)
                .await
                .map_err(|err| StreamStoreError::Oplog(err.to_string()))?
                .ok_or(StreamStoreError::InvalidHandle)?;
            if identity.fingerprint != handle.expected_producer_fingerprint {
                crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
                return Err(StreamStoreError::InvalidHandle);
            }
            let (_, rows) = service
                .lookup_durable_stream_producer_metadata(
                    &owner,
                    identity.agent_mode,
                    handle.expected_producer_fingerprint,
                    keys,
                )
                .await
                .map_err(StreamStoreError::from)?;
            let mut rows = rows.into_iter();
            let Some(Some(ProducerMetadataRow::Stream(stream))) = rows.next() else {
                return Err(StreamStoreError::InvalidHandle);
            };
            let Some(Some(ProducerMetadataRow::Lineage(lineage))) = rows.next() else {
                return Err(StreamStoreError::InvalidHandle);
            };
            let producer_generation = lineage
                .cuts()
                .iter()
                .rev()
                .find_map(|(marker, cut)| cut.revert.is_some().then_some(*marker))
                .unwrap_or(OplogIndex::NONE);
            let position = match rows.next().flatten() {
                Some(ProducerMetadataRow::Position(first, count)) => Some((first, count)),
                _ => None,
            };
            (stream, producer_generation, position)
        };
        if !stream.registration.accepts(handle, producer_generation) {
            return Err(StreamStoreError::InvalidHandle);
        }
        let total = stream
            .next_sequence
            .checked_add(u64::from(stream.terminal))
            .ok_or(StreamStoreError::CounterOverflow)?;
        let consumed = match after {
            None => 0,
            Some(after) if stream.terminal && stream.last_offset == Some(after) => total,
            Some(after) => {
                let (first, count) = position.ok_or(StreamStoreError::CursorUnavailable)?;
                if u64::from(after.sub_index()) >= count
                    || stream.last_offset.is_none_or(|last| after > last)
                {
                    return Err(StreamStoreError::CursorUnavailable);
                }
                first
                    .checked_add(u64::from(after.sub_index()) + 1)
                    .ok_or(StreamStoreError::CounterOverflow)?
            }
        };
        usize::try_from(
            total
                .checked_sub(consumed)
                .ok_or(StreamStoreError::CursorUnavailable)?,
        )
        .map_err(|_| StreamStoreError::CounterOverflow)
    }

    pub(in crate::durable_host::durable_stream) async fn load_terminal(
        &self,
        index: &mut ProducerStreamIndex,
        stream_id: StreamId,
    ) -> Result<(), StreamStoreError> {
        for (id, stream) in &mut index.streams {
            if *id != stream_id {
                stream.terminal_event = None;
            }
        }
        let stream = index
            .streams
            .get_mut(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        if !stream.terminal || stream.terminal_event.is_some() {
            return Ok(());
        }
        let offset = stream.last_offset.ok_or_else(|| {
            StreamStoreError::CorruptHistory(
                "terminal stream metadata has no durable offset".into(),
            )
        })?;
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let event = read_terminal_event(
            self.oplog.as_ref(),
            &self.fork_lineage,
            &owner,
            self.producer_fingerprint,
            stream_id,
            offset,
        )
        .await?;
        if event.producer_sequence != stream.next_sequence {
            return Err(StreamStoreError::CorruptHistory(
                "terminal metadata does not match its durable record".into(),
            ));
        }
        stream.terminal_event = Some(event);
        Ok(())
    }
}

pub(in crate::durable_host::durable_stream) async fn read_terminal_event(
    oplog: &dyn Oplog,
    _lineage: &StreamForkLineage,
    owner: &OwnedAgentId,
    owner_fingerprint: AgentFingerprint,
    stream_id: StreamId,
    offset: StreamOffset,
) -> Result<CommittedProducerStreamEvent, StreamStoreError> {
    let (id, sequence, recorded_offset, terminal_author, payload) =
        match oplog.read(offset.producer_oplog_index()).await? {
            OplogEntry::StreamEnd { record, .. } => {
                let record = oplog
                    .download_payload(record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                (
                    record.stream_id,
                    record.sequence,
                    record.offset,
                    record.authored_by,
                    CommittedProducerStreamEventPayload::End(record.result),
                )
            }
            OplogEntry::StreamCancel { record, .. } => {
                let record = oplog
                    .download_payload(record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                (
                    record.stream_id,
                    record.sequence,
                    record.offset,
                    record.authored_by,
                    CommittedProducerStreamEventPayload::Cancel {
                        role: record.role,
                        reason: record.reason,
                        details: record.details,
                    },
                )
            }
            _ => {
                return Err(StreamStoreError::CorruptHistory(
                    "terminal metadata points at a non-terminal record".into(),
                ));
            }
        };
    let id = qualify_local_stream(id, owner.environment_id, &owner.agent_id, owner_fingerprint)?;
    if id != stream_id || recorded_offset != offset {
        return Err(StreamStoreError::CorruptHistory(
            "terminal metadata does not match its durable record".into(),
        ));
    }
    Ok(CommittedProducerStreamEvent {
        stream_id,
        producer_sequence: sequence,
        offset,
        packed_u8_batch_end: None,
        terminal_author: Some(terminal_author),
        nested_handles: Vec::new(),
        nested_references: Vec::new(),
        payload,
    })
}
