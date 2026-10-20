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
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_common::model::durable_stream::StreamForkCutRecord;

struct Projection<'a> {
    index: ProducerStreamIndex,
    loaded: HashSet<ProducerMetadataKey>,
    new_sessions: HashSet<StreamSessionKey>,
    replacements: HashMap<ProducerMetadataKey, ProducerMetadataRow>,
    storage: &'a (dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
}

pub(crate) struct ForkControlProjection {
    pub(crate) sessions: HashSet<StreamSessionKey>,
}

#[derive(Default)]
pub(crate) struct ProducerMetadataProjection {
    pub(crate) rows: Vec<(String, Vec<u8>)>,
    pub(crate) fork: Option<ForkControlProjection>,
}

impl Projection<'_> {
    async fn load(&mut self, key: ProducerMetadataKey) -> Result<(), String> {
        if self.loaded.insert(key.clone()) {
            let row = self
                .storage
                .with_entity("worker", "producer_projection_read", "metadata")
                .get(self.namespace.clone(), &key.field()?)
                .await?;
            if let Some(row) = row {
                self.index.hydrate_metadata_row(key, row)?;
            } else if let ProducerMetadataKey::Session(session) = key {
                self.new_sessions.insert(session);
            }
        }
        Ok(())
    }

    async fn fork_cut(
        &mut self,
        cut: &StreamForkCutRecord,
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<ForkControlProjection, String> {
        for page in 0..self.index.session_count.div_ceil(SESSION_PAGE_SIZE) {
            self.load(ProducerMetadataKey::SessionPage(page)).await?;
            let sessions = self
                .index
                .session_pages
                .get(&page)
                .ok_or("producer session catalogue page is missing")?
                .clone();
            for session in sessions {
                self.load(ProducerMetadataKey::Session(session.clone()))
                    .await?;
                let streams = self
                    .index
                    .session_consumer_streams
                    .get(&session)
                    .cloned()
                    .unwrap_or_default();
                for stream in streams {
                    self.load(ProducerMetadataKey::ConsumerHead(session.clone(), stream))
                        .await?;
                }
            }
        }
        let local_ids = self
            .index
            .session_stream_mappings
            .values()
            .flatten()
            .filter_map(|(reference, _)| match reference {
                StreamRecordReference::Local(id) => Some(*id),
                StreamRecordReference::Foreign(_) => None,
            })
            .collect::<HashSet<_>>();
        for local_id in local_ids {
            let stream =
                qualify_local_stream(local_id, owner.environment_id, &owner.agent_id, fingerprint)
                    .map_err(|error| error.to_string())?;
            self.stream(stream).await?;
        }
        for page in 0..self
            .index
            .active_attachment_count
            .div_ceil(ATTACHMENT_PAGE_SIZE)
        {
            let key = ProducerMetadataKey::AttachmentPage(page);
            self.loaded.insert(key.clone());
            self.replacements
                .insert(key, ProducerMetadataRow::AttachmentPage(Vec::new()));
        }
        for coordinate in self.index.coordinates.keys() {
            self.loaded
                .insert(ProducerMetadataKey::Coordinate(coordinate.clone()));
        }
        let retained = match (cut.selected_stream_id, cut.retained_through) {
            (None, None) => None,
            (None, Some(_)) => return Err("fork retained offset has no selected stream".into()),
            (Some(_), None) => Some(0),
            (Some(stream), Some(offset)) => {
                let stream = *self
                    .index
                    .local_stream_ids
                    .get(&stream)
                    .ok_or("fork selected stream is not registered")?;
                let key = ProducerMetadataKey::Position(stream, offset.producer_oplog_index());
                let row = self
                    .storage
                    .with_entity("worker", "producer_projection_read", "position")
                    .get(self.namespace.clone(), &key.field()?)
                    .await?;
                let Some(ProducerMetadataRow::Position(first, count)) = row else {
                    return Err("fork retained offset is not an item of the selected stream".into());
                };
                let sub = u64::from(offset.sub_index());
                if sub >= count {
                    return Err("fork retained sub-offset exceeds its item batch".into());
                }
                Some(
                    first
                        .checked_add(sub)
                        .and_then(|n| n.checked_add(1))
                        .ok_or("fork retained item count overflow")?,
                )
            }
        };
        self.index
            .apply_fork_cut(cut, retained)
            .map_err(|error| error.to_string())?;
        // Locator rows are immutable history. Missing authority rows, unlike locators, must be
        // explicitly retired because the surrounding CAS only overwrites emitted fields.
        for key in &self.loaded {
            if matches!(
                key,
                ProducerMetadataKey::Stream(_)
                    | ProducerMetadataKey::Coordinate(_)
                    | ProducerMetadataKey::Session(_)
                    | ProducerMetadataKey::ConsumerHead(_, _)
            ) && self.index.metadata_row(key).is_none()
            {
                self.replacements
                    .insert(key.clone(), ProducerMetadataRow::Retired);
            }
        }
        for stream in self.index.streams.keys() {
            self.loaded.insert(ProducerMetadataKey::Stream(*stream));
        }
        for coordinate in self.index.coordinates.keys() {
            self.loaded
                .insert(ProducerMetadataKey::Coordinate(coordinate.clone()));
        }
        for (session, stream) in self.index.consumer_journals.keys() {
            self.loaded
                .insert(ProducerMetadataKey::ConsumerHead(session.clone(), *stream));
        }
        for session in self.index.session_entity_parent_start_indices.keys() {
            self.loaded
                .insert(ProducerMetadataKey::Session(session.clone()));
            self.new_sessions.insert(session.clone());
        }
        // Empty old page tails too: appending beyond a shortened catalogue must not reload them.
        for sessions in self.index.session_pages.values_mut() {
            sessions.clear();
        }
        self.index.session_count = 0;
        Ok(ForkControlProjection {
            sessions: self
                .index
                .session_entity_parent_start_indices
                .keys()
                .cloned()
                .collect(),
        })
    }

    async fn catalogue_new_sessions(&mut self) -> Result<(), String> {
        for session in std::mem::take(&mut self.new_sessions) {
            if !self
                .index
                .session_entity_parent_start_indices
                .contains_key(&session)
            {
                continue;
            }
            let page = self.index.session_count / SESSION_PAGE_SIZE;
            self.load(ProducerMetadataKey::SessionPage(page)).await?;
            self.index
                .session_pages
                .entry(page)
                .or_default()
                .push(session);
            self.index.session_count = self
                .index
                .session_count
                .checked_add(1)
                .ok_or("producer session catalogue overflow")?;
        }
        Ok(())
    }

    async fn catalogue_attachment(
        &mut self,
        attachment: AttachmentId,
        stream: StreamId,
        environment: EnvironmentId,
        consumer: AgentId,
    ) -> Result<(), String> {
        let key = (attachment, stream, environment, consumer.clone());
        self.load(ProducerMetadataKey::AttachmentPosition(
            attachment,
            stream,
            environment,
            consumer,
        ))
        .await?;
        let position = self.index.attachment_positions.get(&key).copied().flatten();
        let active = !matches!(
            self.index.attachments[&key].state,
            IndexedStreamAttachmentState::Finalized { .. }
        );
        match (active, position) {
            (true, None) => {
                let position = self.index.active_attachment_count;
                let page = position / ATTACHMENT_PAGE_SIZE;
                self.load(ProducerMetadataKey::AttachmentPage(page)).await?;
                self.index
                    .attachment_pages
                    .entry(page)
                    .or_default()
                    .push(key.clone());
                self.index.attachment_positions.insert(key, Some(position));
                self.index.active_attachment_count += 1;
            }
            (false, Some(position)) => {
                let last = self
                    .index
                    .active_attachment_count
                    .checked_sub(1)
                    .ok_or("active attachment catalogue is empty")?;
                if position > last {
                    return Err("active attachment position exceeds its catalogue".into());
                }
                self.load(ProducerMetadataKey::AttachmentPage(
                    position / ATTACHMENT_PAGE_SIZE,
                ))
                .await?;
                self.load(ProducerMetadataKey::AttachmentPage(
                    last / ATTACHMENT_PAGE_SIZE,
                ))
                .await?;
                let moved = self
                    .index
                    .attachment_pages
                    .get_mut(&(last / ATTACHMENT_PAGE_SIZE))
                    .and_then(Vec::pop)
                    .ok_or("active attachment catalogue page is missing")?;
                if position != last {
                    self.load(ProducerMetadataKey::AttachmentPosition(
                        moved.0,
                        moved.1,
                        moved.2,
                        moved.3.clone(),
                    ))
                    .await?;
                    *self
                        .index
                        .attachment_pages
                        .get_mut(&(position / ATTACHMENT_PAGE_SIZE))
                        .and_then(|page| page.get_mut((position % ATTACHMENT_PAGE_SIZE) as usize))
                        .ok_or("active attachment catalogue slot is missing")? = moved.clone();
                    self.index
                        .attachment_positions
                        .insert(moved, Some(position));
                }
                self.index.attachment_positions.insert(key, None);
                self.index.active_attachment_count = last;
            }
            _ => {}
        }
        Ok(())
    }

    async fn stream(&mut self, stream: StreamId) -> Result<(), String> {
        self.load(ProducerMetadataKey::Stream(stream)).await?;
        if let Some(session) = self.index.stream_sessions.get(&stream).cloned() {
            self.load(ProducerMetadataKey::Session(session)).await?;
        }
        Ok(())
    }

    async fn registration(
        &mut self,
        oplog_index: OplogIndex,
        author: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        record: &StreamRegisteredRecord,
    ) -> Result<(), String> {
        let registered = RegisteredStream::resolve(
            record.clone(),
            oplog_index,
            author.environment_id,
            &author.agent_id,
            fingerprint,
        )
        .map_err(|error| error.to_string())?;
        if let StreamRegistrationCoordinate::Nested {
            parent_stream_id, ..
        } = &registered.coordinate
        {
            self.stream(*parent_stream_id).await?;
        }
        self.load(ProducerMetadataKey::Coordinate(
            registered.coordinate.clone(),
        ))
        .await?;
        if let Some(existing) = self.index.coordinates.get(&registered.coordinate).copied() {
            self.stream(existing).await?;
        }
        let stream_id = StreamId::derive(
            author.environment_id,
            &author.agent_id,
            fingerprint,
            oplog_index,
        )
        .map_err(|error| error.to_string())?;
        self.stream(stream_id).await?;
        if let Some(session) = self
            .index
            .registration_session_key(&registered.coordinate, &registered.session_mapping)
        {
            self.load(ProducerMetadataKey::Session(session)).await?;
        }
        Ok(())
    }

    async fn session(
        &mut self,
        record: &StreamSessionRecord,
        author: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), String> {
        for key in ProducerMetadataKey::session_record(
            record,
            author.environment_id,
            &author.agent_id,
            fingerprint,
        ) {
            if let ProducerMetadataKey::Stream(stream) = key {
                self.stream(stream).await?;
            }
        }
        if let Some(session) = crate::worker::stream_session_record_key(
            record,
            author.environment_id,
            &author.agent_id,
            fingerprint,
        ) {
            self.load(ProducerMetadataKey::Session(session)).await?;
        }
        let attachment = match record {
            StreamSessionRecord::AttachmentPrepared(record) => Some(&record.key),
            StreamSessionRecord::AttachmentActivated(record) => Some(&record.key),
            StreamSessionRecord::AttachmentFinalized(record) => Some(&record.key),
            StreamSessionRecord::CascadeOutbox(record) => {
                self.load(ProducerMetadataKey::Cascade(Box::new(record.key.clone())))
                    .await?;
                Some(&record.key)
            }
            _ => None,
        };
        if let Some(key) = attachment {
            self.stream(key.stream_id).await?;
            self.load(ProducerMetadataKey::Attachment(
                key.attachment_id,
                key.stream_id,
                key.consumer_environment_id,
                key.consumer.clone(),
            ))
            .await?;
            self.load(ProducerMetadataKey::ActiveAttachmentCount(
                key.session_key.clone(),
                key.stream_id,
            ))
            .await?;
        }
        let head = match record {
            StreamSessionRecord::ConsumerItemValue(record) => Some((
                record
                    .session_key
                    .qualify(author.environment_id, &author.agent_id, fingerprint),
                record.reader_id,
            )),
            StreamSessionRecord::ConsumerTerminal(record) => Some((
                record
                    .session_key
                    .qualify(author.environment_id, &author.agent_id, fingerprint),
                record.reader_id,
            )),
            StreamSessionRecord::SourceUnavailable(record) => Some((
                record
                    .session_key
                    .qualify(author.environment_id, &author.agent_id, fingerprint),
                record.reader_id,
            )),
            _ => None,
        };
        if let Some((session, reader)) = head {
            self.load(ProducerMetadataKey::ConsumerHead(session, reader))
                .await?;
        }
        if let StreamSessionRecord::ExternalProducerState(record) = record {
            self.load(ProducerMetadataKey::ExternalProducerHead(
                record.session_key.clone(),
                record.stream_id,
                record.producer_id.clone(),
            ))
            .await?;
            self.loaded
                .insert(ProducerMetadataKey::ExternalProducerSequence(
                    record.session_key.clone(),
                    record.stream_id,
                    record.producer_id.clone(),
                    record.epoch,
                    record.sequence,
                ));
        }
        Ok(())
    }
}

/// Rebuilds metadata rows from committed producer history without changing that history.
pub(crate) async fn project_producer_metadata(
    storage: &(dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
    oplog: &dyn OplogService,
    owner: &OwnedAgentId,
    mode: AgentMode,
    fingerprint: AgentFingerprint,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    lineage: &StreamForkLineage,
) -> Result<ProducerMetadataProjection, String> {
    let mut projection = Projection {
        index: ProducerStreamIndex::default(),
        loaded: HashSet::new(),
        new_sessions: HashSet::new(),
        replacements: HashMap::new(),
        storage,
        namespace,
    };
    projection.load(ProducerMetadataKey::Global).await?;
    let mut pending = Vec::new();
    let mut fork = None;
    for (index, entry) in entries {
        if !pending.is_empty()
            && !matches!(
                entry,
                OplogEntry::StreamRegistered { .. } | OplogEntry::StreamItems { .. }
            )
        {
            return Err("nested registration batch is missing its enclosing item".into());
        }
        match entry {
            OplogEntry::StreamRegistered {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection
                    .registration(*index, owner, fingerprint, &record)
                    .await?;
                if matches!(
                    record.coordinate,
                    StreamRegistrationRecordCoordinate::Nested { .. }
                ) {
                    pending.push((*index, *entity_parent_start_index, record));
                } else {
                    if !pending.is_empty() {
                        return Err(
                            "nested registration batch is missing its enclosing item".into()
                        );
                    }
                    projection
                        .index
                        .apply_registration(
                            *index,
                            *entity_parent_start_index,
                            record,
                            owner.environment_id,
                            &owner.agent_id,
                            fingerprint,
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
            OplogEntry::StreamItems {
                entity_parent_start_index,
                record,
                ..
            } => {
                let mut record = oplog.download_payload(owner, mode, record.clone()).await?;
                lineage.project_item_batch(*index, &mut record)?;
                let stream_id = qualify_local_stream(
                    record.stream_id,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                )
                .map_err(|error| error.to_string())?;
                projection.stream(stream_id).await?;
                for reference in &record.nested_stream_ids {
                    let stream = materialize_stream_reference(
                        reference,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                    projection.load(ProducerMetadataKey::Stream(stream)).await?;
                }
                projection
                    .loaded
                    .insert(ProducerMetadataKey::Batch(stream_id, record.first_sequence));
                projection
                    .loaded
                    .insert(ProducerMetadataKey::Position(stream_id, *index));
                projection
                    .index
                    .apply_item_batch(
                        *index,
                        *entity_parent_start_index,
                        std::mem::take(&mut pending),
                        record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                        lineage
                            .cuts()
                            .iter()
                            .rev()
                            .find_map(|(marker, cut)| cut.revert.is_some().then_some(*marker))
                            .unwrap_or(OplogIndex::NONE),
                    )
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamEnd {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                let stream_id = qualify_local_stream(
                    record.stream_id,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                )
                .map_err(|error| error.to_string())?;
                projection.stream(stream_id).await?;
                projection
                    .index
                    .apply_end(*index, *entity_parent_start_index, record, fingerprint)
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamCancel {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                let stream_id = qualify_local_stream(
                    record.stream_id,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                )
                .map_err(|error| error.to_string())?;
                projection.stream(stream_id).await?;
                projection
                    .index
                    .apply_cancel(*index, *entity_parent_start_index, record, fingerprint)
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamSession {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                if let Some(original_index) = record.reobserves() {
                    if original_index >= *index {
                        return Err(
                            "consumer re-observation must reference an earlier record".into()
                        );
                    }
                    let original = oplog
                        .read_exact(owner, mode, original_index, 1)
                        .await
                        .map_err(|error| error.to_string())?;
                    let Some(OplogEntry::StreamSession {
                        record: original, ..
                    }) = original.get(&original_index)
                    else {
                        return Err(
                            "consumer re-observation has no original consumer record".into()
                        );
                    };
                    let original = oplog
                        .download_payload(owner, mode, original.clone())
                        .await?;
                    if !record.has_supported_format() || !record.matches_reobserved(&original) {
                        return Err(
                            "consumer re-observation differs from its original input".into()
                        );
                    }
                }
                if let StreamSessionRecord::ForkCut(cut) = &record {
                    if entries.len() != 1 {
                        return Err("fork marker must occupy its own projection transaction".into());
                    }
                    if lineage
                        .cuts()
                        .iter()
                        .find(|(position, _)| position == index)
                        .map(|(_, record)| record)
                        != Some(cut)
                    {
                        return Err("fork marker differs from the validated lineage".into());
                    }
                    fork = Some(projection.fork_cut(cut, owner, fingerprint).await?);
                    continue;
                }
                if lineage.resets_control(*index, &record) {
                    continue;
                }
                projection.session(&record, owner, fingerprint).await?;
                projection
                    .index
                    .apply_session_references(
                        *entity_parent_start_index,
                        &record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                projection.index.apply_result_offset(
                    *index,
                    &record,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                );
                if let StreamSessionRecord::ExternalProducerState(value) = &record {
                    projection.index.apply_external_producer_state(value);
                }
                projection
                    .index
                    .apply_deletion_record(
                        &record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                projection
                    .index
                    .apply_attachment_record(
                        &record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                for key in ProducerMetadataKey::session_record(
                    &record,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                ) {
                    if let ProducerMetadataKey::Attachment(
                        attachment,
                        stream,
                        environment,
                        consumer,
                    ) = key
                    {
                        projection
                            .catalogue_attachment(attachment, stream, environment, consumer)
                            .await?;
                    }
                }
                if let StreamSessionRecord::Finished(record) = &record {
                    projection
                        .index
                        .apply_finished(record, owner.environment_id, &owner.agent_id, fingerprint)
                        .map_err(|error| error.to_string())?;
                }
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err("producer projection chunk splits a nested registration batch".into());
    }
    projection.catalogue_new_sessions().await?;
    let rows = projection
        .loaded
        .into_iter()
        .filter_map(|key| {
            projection
                .replacements
                .remove(&key)
                .or_else(|| projection.index.metadata_row(&key))
                .map(|row| (key, row))
        })
        .map(|(key, row)| Ok((key.field()?, serialize(&row)?)))
        .collect::<Result<_, String>>()?;
    Ok(ProducerMetadataProjection { rows, fork })
}
