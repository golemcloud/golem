// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::{DurableStreamStore, ProducerStreamIndex, StreamStoreError};
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::worker_fork::lineage::{StreamForkLineage, continuation_stream_id};
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle,
    StreamForkCutRecord, StreamForkSessionMapping, StreamForkStreamMapping, StreamId, StreamOffset,
    StreamRegistrationCoordinate, StreamSessionKey, StreamSessionRecord,
};
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::OplogRegion;
use golem_common::model::{AgentFingerprint, OplogIndex, OwnedAgentId};
use std::collections::{HashMap, HashSet};

impl DurableStreamStore {
    pub(crate) async fn export_fork_snapshot(
        &self,
        handle: &DurableStreamHandle,
    ) -> Result<super::ExportForkStreamSnapshot, StreamStoreError> {
        self.validate_handle(handle).await?;
        if !self.owns_handle_identity(handle) {
            return Err(StreamStoreError::InvalidHandle);
        }
        let horizon = self.oplog.current_oplog_index().await;
        let index = Self::read_complete_index(
            self.oplog.as_ref(),
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
            horizon,
        )
        .await?;
        let registration = index
            .registrations
            .get(&handle.stream_id)
            .ok_or(StreamStoreError::UnknownStream(handle.stream_id))?;
        let stream = index
            .streams
            .get(&handle.stream_id)
            .ok_or(StreamStoreError::UnknownStream(handle.stream_id))?;
        let mut positions = stream.batches.iter().peekable();
        let mut batches = Vec::new();
        while let Some((&first, &oplog_index)) = positions.next() {
            let end = positions
                .peek()
                .map_or(stream.next_sequence, |(next, _)| **next);
            let length = u32::try_from(end.saturating_sub(first))
                .map_err(|_| StreamStoreError::CounterOverflow)?;
            batches.push((oplog_index, length));
        }
        Ok(super::ExportForkStreamSnapshot {
            horizon,
            registration_index: registration.registration_oplog_index,
            batches,
            terminal: stream.terminal.then_some(stream.last_offset).flatten(),
        })
    }

    /// Captures only the retained prefix, even when the source has advanced or is itself a fork.
    /// A fork appends its marker immediately after the copied prefix; a self-revert atomically
    /// appends Revert and the marker after the discarded physical tail.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_fork_cut(
        oplog: &dyn Oplog,
        source: (&OwnedAgentId, AgentFingerprint),
        target: (&OwnedAgentId, AgentFingerprint),
        horizon: OplogIndex,
        cut_index: OplogIndex,
        selected: Option<(StreamId, Option<StreamOffset>)>,
        request_hash: [u8; 32],
        revert: bool,
    ) -> Result<StreamForkCutRecord, StreamStoreError> {
        if cut_index < OplogIndex::INITIAL || cut_index > horizon {
            return Err(StreamStoreError::InvalidOffset(
                "fork cut is outside the source oplog".into(),
            ));
        }
        if revert && (source != target || selected.is_some() || cut_index == horizon) {
            return Err(StreamStoreError::InvalidOffset(
                "revert requires an earlier prefix of the same agent without a stream sub-offset"
                    .into(),
            ));
        }
        if cut_index < horizon
            && matches!(oplog.read(cut_index).await, OplogEntry::Revert { .. })
            && let OplogEntry::StreamSession { record, .. } = oplog.read(cut_index.next()).await
            && matches!(oplog.download_payload(record).await.map_err(StreamStoreError::Oplog)?,
                StreamSessionRecord::ForkCut(cut) if cut.revert.is_some())
        {
            return Err(StreamStoreError::InvalidOffset(
                "cut separates a Revert entry from its stream fork marker".into(),
            ));
        }
        let lineage = StreamForkLineage::load_at_horizon(oplog, source.0, source.1, horizon)
            .await
            .map_err(StreamStoreError::CorruptHistory)?;
        if revert && lineage.deleted_regions().is_in_deleted_region(cut_index) {
            return Err(StreamStoreError::InvalidOffset(
                "revert must retain a visible prefix of the same agent".into(),
            ));
        }
        if crate::worker::cut_point::streaming_acceptance_spans_cut(oplog, cut_index, horizon)
            .await
            .map_err(StreamStoreError::CorruptHistory)?
        {
            return Err(StreamStoreError::InvalidOffset(
                "cut splits a streaming invocation acceptance batch".into(),
            ));
        }
        let (author, fingerprint) = lineage
            .historical_author_at(oplog, cut_index, horizon, source.0, source.1)
            .await
            .map_err(StreamStoreError::CorruptHistory)?;
        let mut index = Self::read_complete_index(
            oplog,
            author.environment_id,
            &author.agent_id,
            fingerprint,
            cut_index,
        )
        .await?;
        let selected_stream_id = selected
            .map(|(id, _)| {
                index
                    .registrations
                    .values()
                    .find(|registration| {
                        lineage.continuation_handle(&registration.handle).stream_id == id
                    })
                    .map(|registration| registration.handle.stream_id)
                    .ok_or(StreamStoreError::UnknownStream(id))
            })
            .transpose()?;
        let mut record = StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: request_hash.to_vec(),
            export: None,
            source_environment_id: author.environment_id,
            source: author.agent_id.clone(),
            source_fingerprint: fingerprint,
            target_environment_id: target.0.environment_id,
            target: target.0.agent_id.clone(),
            target_fingerprint: target.1,
            cut_index,
            revert: revert.then_some(OplogRegion {
                start: cut_index.next(),
                end: horizon,
            }),
            epoch_floor: if revert {
                let (from, floor) = lineage
                    .cuts()
                    .last()
                    .map_or((OplogIndex::NONE, 1), |(index, cut)| {
                        (*index, cut.epoch_floor)
                    });
                Self::max_issued_epoch(oplog, from, horizon)
                    .await?
                    .max(floor)
                    .checked_add(1)
                    .ok_or(StreamStoreError::CounterOverflow)?
            } else {
                1
            },
            selected_stream_id,
            retained_through: selected.and_then(|(_, offset)| offset),
            streams: vec![],
            sessions: vec![],
        };
        for source in index
            .session_entity_parent_start_indices
            .keys()
            .filter(|key| {
                key.callee_environment_id == author.environment_id
                    && key.callee == author.agent_id
                    && key.callee_fingerprint == fingerprint
            })
        {
            let mut continuation = source.clone();
            continuation.callee_environment_id = target.0.environment_id;
            continuation.callee = target.0.agent_id.clone();
            continuation.callee_fingerprint = target.1;
            record.sessions.push(StreamForkSessionMapping {
                source: source.clone(),
                continuation,
                continuation_attempt_id: AttemptId::fresh(),
            });
        }
        record.sessions.sort_by(|a, b| {
            a.source
                .idempotency_key
                .value
                .cmp(&b.source.idempotency_key.value)
        });
        let mut handles: HashMap<_, _> = index
            .fork_lineage
            .cuts()
            .last()
            .map(|(_, cut)| {
                cut.streams
                    .iter()
                    .map(|mapping| (mapping.continuation.stream_id, mapping.continuation.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for registration in index.registrations.values() {
            handles.insert(registration.handle.stream_id, registration.handle.clone());
        }
        for source in handles.values() {
            let mut continuation = source.clone();
            continuation.stream_id = continuation_stream_id(&record, source.stream_id)
                .map_err(StreamStoreError::CorruptHistory)?;
            continuation.producer_environment_id = target.0.environment_id;
            continuation.producer = target.0.agent_id.clone();
            continuation.expected_producer_fingerprint = target.1;
            if let Some(session) = record
                .sessions
                .iter()
                .find(|mapping| mapping.source == source.source_invocation)
            {
                continuation.source_invocation = session.continuation.clone();
            }
            record.streams.push(StreamForkStreamMapping {
                source: source.clone(),
                continuation,
            });
        }
        record
            .streams
            .sort_by_key(|mapping| mapping.source.stream_id.0);
        let retained_items = index.resolve_fork_prefix(&record)?;
        index.apply_fork_cut(&record, retained_items)?;
        StreamForkLineage::validate_fork_cut(oplog, record.clone())
            .await
            .map_err(StreamStoreError::CorruptHistory)?;
        Ok(record)
    }

    pub(crate) async fn initial_session_epoch(
        &self,
        key: &StreamSessionKey,
    ) -> Result<u64, StreamStoreError> {
        let floor = self.attachment_epoch_floor();
        if self.fork_lineage.cuts().last().is_some_and(|(_, cut)| {
            cut.sessions
                .iter()
                .any(|mapping| &mapping.continuation == key)
        }) {
            floor
                .checked_add(1)
                .ok_or(StreamStoreError::CounterOverflow)
        } else {
            Ok(floor)
        }
    }

    pub(crate) fn attachment_epoch_floor(&self) -> u64 {
        self.fork_lineage
            .cuts()
            .last()
            .map_or(1, |(_, cut)| cut.epoch_floor)
    }

    /// Consumer-issued epochs in physical history remain fences even when their records
    /// are deleted. Producer-side attachment records belong to other consumers' epoch spaces.
    async fn max_issued_epoch(
        oplog: &dyn Oplog,
        from: OplogIndex,
        horizon: OplogIndex,
    ) -> Result<u64, StreamStoreError> {
        let mut max_epoch = 0;
        let mut covered = from;
        while covered < horizon {
            let count = (horizon.as_u64() - covered.as_u64()).min(1024);
            let entries = oplog.read_exact(covered.next(), count).await;
            if entries.len() as u64 != count {
                return Err(StreamStoreError::CorruptHistory(
                    "missing oplog entries while reading stream epochs".into(),
                ));
            }
            for (index, entry) in entries {
                if index != covered.next() {
                    return Err(StreamStoreError::CorruptHistory(
                        "noncontiguous oplog while reading stream epochs".into(),
                    ));
                }
                covered = index;
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let record = oplog
                    .download_payload(record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                let epoch = match record {
                    StreamSessionRecord::Attached(record) => record.epoch,
                    StreamSessionRecord::ResumeAttempt(record) => record.accepted_epoch,
                    StreamSessionRecord::ForkCut(cut) => cut.epoch_floor,
                    StreamSessionRecord::TopologyPrepared(record) => record.attachment.epoch,
                    StreamSessionRecord::TopologyActivated(record) => record.attachment.epoch,
                    StreamSessionRecord::ConsumerCancelIntent(record) => record.epoch,
                    _ => 0,
                };
                max_epoch = max_epoch.max(epoch);
            }
        }
        Ok(max_epoch)
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct ForkCutChanges {
    pub(super) changed_streams: HashSet<StreamId>,
    pub(super) retired_streams: HashSet<StreamId>,
    pub(super) changed_sessions: HashSet<StreamSessionKey>,
    pub(super) retired_sessions: HashSet<StreamSessionKey>,
}

impl ProducerStreamIndex {
    pub(super) fn resolve_fork_prefix(
        &self,
        record: &StreamForkCutRecord,
    ) -> Result<Option<u64>, StreamStoreError> {
        match (record.selected_stream_id, record.retained_through) {
            (None, None) => Ok(None),
            (None, Some(_)) => Err(StreamStoreError::CorruptHistory(
                "fork marker has a retained offset without a selected stream".into(),
            )),
            (Some(_), None) => Ok(Some(0)),
            (Some(selected), Some(offset)) => {
                let (first, count) = self
                    .batch_positions
                    .get(&(selected, offset.producer_oplog_index()))
                    .copied()
                    .ok_or_else(|| {
                        StreamStoreError::CorruptHistory(
                            "fork retained offset is not an item of the selected stream".into(),
                        )
                    })?;
                let sub_index = u64::from(offset.sub_index());
                if sub_index >= count {
                    return Err(StreamStoreError::CorruptHistory(
                        "fork retained sub-offset exceeds its item batch".into(),
                    ));
                }
                first
                    .checked_add(sub_index)
                    .and_then(|value| value.checked_add(1))
                    .map(Some)
                    .ok_or(StreamStoreError::CounterOverflow)
            }
        }
    }

    /// Applies an already validated fork marker to the state accumulated before that marker.
    /// `selected_prefix_items` is the number of selected-stream items retained by the resolved cut.
    pub(super) fn apply_fork_cut(
        &mut self,
        record: &StreamForkCutRecord,
        selected_prefix_items: Option<u64>,
    ) -> Result<ForkCutChanges, StreamStoreError> {
        if let Some(selected) = record.selected_stream_id
            && !self.registrations.contains_key(&selected)
        {
            return Err(StreamStoreError::CorruptHistory(
                "fork selected stream has no active registration at its cut".into(),
            ));
        }
        if record.selected_stream_id.is_some_and(|selected| {
            self.finished_sessions
                .contains(&self.stream_sessions[&selected])
        }) {
            return Err(StreamStoreError::CorruptHistory(
                "selected stream cut retains its owning session completion".into(),
            ));
        }
        let stream_map: HashMap<_, _> = record
            .streams
            .iter()
            .map(|mapping| (mapping.source.stream_id, mapping.continuation.clone()))
            .collect();
        let session_map: HashMap<_, _> = record
            .sessions
            .iter()
            .map(|mapping| (mapping.source.clone(), mapping.continuation.clone()))
            .collect();

        let selected_prefix_items = match (record.selected_stream_id, selected_prefix_items) {
            (Some(_), Some(count)) => Some(count),
            (None, None) => None,
            _ => {
                return Err(StreamStoreError::CorruptHistory(
                    "fork selected-prefix resolution does not match its marker".into(),
                ));
            }
        };

        // A nested registration is authoritative only if its owning parent item survived. Iterate
        // to a fixed point so descendants of a discarded child are removed as well.
        let mut retained: HashSet<_> = self.registrations.keys().copied().collect();
        loop {
            let discarded = retained
                .iter()
                .copied()
                .filter(|stream_id| {
                    let Some(registration) = self.registrations.get(stream_id) else {
                        return true;
                    };
                    match registration.coordinate {
                        StreamRegistrationCoordinate::Root { .. } => false,
                        StreamRegistrationCoordinate::Nested {
                            parent_stream_id,
                            parent_producer_sequence,
                            ..
                        } => {
                            !retained.contains(&parent_stream_id)
                                || (record.selected_stream_id == Some(parent_stream_id)
                                    && parent_producer_sequence
                                        >= selected_prefix_items
                                            .expect("selected stream has a prefix"))
                        }
                    }
                })
                .collect::<Vec<_>>();
            if discarded.is_empty() {
                break;
            }
            retained.retain(|stream_id| !discarded.contains(stream_id));
        }
        retained.retain(|id| stream_map.contains_key(id));

        let old_streams: HashSet<_> = self.registrations.keys().copied().collect();
        let old_sessions: HashSet<_> = self
            .session_entity_parent_start_indices
            .keys()
            .cloned()
            .collect();
        let remap_session = |key: &StreamSessionKey| session_map.get(key).unwrap_or(key).clone();
        let remap_handle = |handle: &DurableStreamHandle| {
            if old_streams.contains(&handle.stream_id) && !retained.contains(&handle.stream_id) {
                None
            } else {
                Some(stream_map.get(&handle.stream_id).unwrap_or(handle).clone())
            }
        };

        let mut registrations = HashMap::new();
        let mut entity_parents = HashMap::new();
        let mut streams = HashMap::new();
        let mut stream_sessions = HashMap::new();
        let mut stream_roles = HashMap::new();
        let mut coordinates = HashMap::new();
        for source_id in retained.iter().copied() {
            let continuation = stream_map[&source_id].clone();
            let continuation_id = continuation.stream_id;
            let mut registration = self.registrations[&source_id].clone();
            registration.handle = continuation.clone();
            if let Some(mapping) = &mut registration.session_mapping {
                mapping.session_key = remap_session(&mapping.session_key);
                mapping.attachment_id = AttachmentId::primary(
                    mapping.session_key.callee_environment_id,
                    &mapping.session_key.callee,
                    &mapping.session_key.idempotency_key,
                )
                .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?;
            }
            registration.coordinate = match registration.coordinate {
                StreamRegistrationCoordinate::Root {
                    invocation_id,
                    root_kind,
                    recursive_value_path,
                } => StreamRegistrationCoordinate::Root {
                    invocation_id: session_map
                        .get(&invocation_id)
                        .cloned()
                        .unwrap_or(invocation_id),
                    root_kind,
                    recursive_value_path,
                },
                StreamRegistrationCoordinate::Nested {
                    parent_stream_id,
                    parent_producer_sequence,
                    recursive_value_path,
                } => StreamRegistrationCoordinate::Nested {
                    parent_stream_id: stream_map[&parent_stream_id].stream_id,
                    parent_producer_sequence,
                    recursive_value_path,
                },
            };
            let mut state = self.streams[&source_id].clone();
            if let Some(event) = &mut state.terminal_event {
                event.stream_id = continuation_id;
            }
            if record.selected_stream_id == Some(source_id) {
                let count = selected_prefix_items.expect("selected stream has a prefix");
                state.next_sequence = count;
                state.batches.retain(|first, _| *first < count);
                if count == 0 {
                    state.first_sequence = None;
                }
                state.terminal = false;
                state.terminal_event = None;
                state.last_offset = record.retained_through;
                state.last_item_offset = record.retained_through;
            }
            let source_session = &self.stream_sessions[&source_id];
            let continuation_session = session_map
                .get(source_session)
                .cloned()
                .unwrap_or_else(|| source_session.clone());
            coordinates.insert(registration.coordinate.clone(), continuation_id);
            registrations.insert(continuation_id, registration);
            entity_parents.insert(
                continuation_id,
                self.entity_parent_start_indices[&source_id],
            );
            streams.insert(continuation_id, state);
            stream_sessions.insert(continuation_id, continuation_session);
            stream_roles.insert(continuation_id, self.stream_roles[&source_id]);
            self.fork_aliases.insert(source_id, continuation);
        }

        self.registrations = registrations;
        self.entity_parent_start_indices = entity_parents;
        self.streams = streams;
        self.stream_sessions = stream_sessions;
        self.stream_roles = stream_roles;
        self.coordinates = coordinates;

        self.referenced_handles = self
            .referenced_handles
            .iter()
            .filter_map(|(_, (handle, sessions))| {
                let handle = remap_handle(handle)?;
                let sessions = sessions.iter().map(&remap_session).collect();
                Some((handle.stream_id, (handle, sessions)))
            })
            .collect();
        self.session_stream_mappings = self
            .session_stream_mappings
            .iter()
            .map(|(session, mappings)| {
                let session = remap_session(session);
                let mappings = mappings
                    .iter()
                    .filter_map(|(handle, role)| remap_handle(handle).map(|handle| (handle, *role)))
                    .collect::<HashSet<_>>();
                (session, mappings)
            })
            .collect();
        self.session_stream_counts = self
            .session_stream_mappings
            .iter()
            .map(|(key, mappings)| (key.clone(), mappings.len()))
            .collect();
        self.session_entity_parent_start_indices = self
            .session_entity_parent_start_indices
            .iter()
            .map(|(key, value)| (remap_session(key), *value))
            .collect();
        self.open_session_streams.clear();
        self.open_streams = 0;
        for (stream_id, state) in &self.streams {
            if !state.terminal {
                self.open_streams += 1;
                self.open_session_streams
                    .entry(self.stream_sessions[stream_id].clone())
                    .or_default()
                    .insert(*stream_id);
            }
        }
        self.invocation_results = self
            .invocation_results
            .iter()
            .map(|(key, value)| (remap_session(key), *value))
            .collect();
        self.finished_sessions = self.finished_sessions.iter().map(&remap_session).collect();
        self.consumer_journals = self
            .consumer_journals
            .iter()
            .filter_map(|((session, stream), journal)| {
                if old_streams.contains(stream) && !retained.contains(stream) {
                    return None;
                }
                Some((
                    (
                        remap_session(session),
                        stream_map
                            .get(stream)
                            .map_or(*stream, |handle| handle.stream_id),
                    ),
                    journal.clone(),
                ))
            })
            .collect();
        self.session_consumer_streams.clear();
        for (session, stream) in self.consumer_journals.keys() {
            self.session_consumer_streams
                .entry(session.clone())
                .or_default()
                .insert(*stream);
        }
        for journal in self.consumer_journals.values_mut() {
            let Some((key, _)) = &mut journal.source_unavailable else {
                continue;
            };
            crate::services::worker_fork::lineage::project_attachment_key(record, key)
                .map_err(StreamStoreError::CorruptHistory)?;
        }

        // Fork continuations begin detached and with no inherited remote producer or deletion state.
        self.attachments.clear();
        self.active_attachments_by_session_stream.clear();
        self.active_attachment_count = 0;
        self.attachment_pages.clear();
        self.attachment_positions.clear();
        self.cascade_outbox.clear();
        self.external_producer_heads.clear();
        self.external_producer_offsets.clear();
        self.deleting = false;
        self.consumer_deleting = false;
        self.complete_for_deletion = false;
        self.batch_positions = self
            .batch_positions
            .iter()
            .filter_map(|((stream, index), value)| {
                let continuation = stream_map.get(stream)?;
                if !retained.contains(stream) {
                    return None;
                }
                let mut value = *value;
                if record.selected_stream_id == Some(*stream) {
                    let retained = selected_prefix_items.expect("selected stream has a prefix");
                    value.1 = value.1.min(retained.saturating_sub(value.0));
                    if value.1 == 0 {
                        return None;
                    }
                }
                Some(((continuation.stream_id, *index), value))
            })
            .collect();

        Ok(ForkCutChanges {
            changed_streams: self.registrations.keys().copied().collect(),
            retired_streams: old_streams,
            changed_sessions: self
                .session_entity_parent_start_indices
                .keys()
                .cloned()
                .collect(),
            retired_sessions: old_sessions
                .into_iter()
                .filter(|key| !self.session_entity_parent_start_indices.contains_key(key))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::registration::registration_record;
    use crate::durable_host::durable_stream::{
        CommittedProducerStreamEventPayload, IndexedExternalProducer, ProducerRegistrationRequest,
        tests::{attachment_key, identity},
    };
    use crate::services::worker_fork::lineage::{StreamForkLineage, continuation_stream_id};
    use golem_common::base_model::durable_stream::{
        AttemptId, DURABLE_STREAM_FORMAT_VERSION, SessionStreamRole, StreamEndRecord,
        StreamEndResult, StreamForkSessionMapping, StreamForkStreamMapping, StreamItemsPayload,
        StreamItemsRecord, StreamRootKind, StreamSessionFinishedRecord, StreamSessionMapping,
        StreamSourceKind, StreamTerminalAuthor,
    };
    use golem_common::base_model::durable_stream::{
        ExternalProducerId, StreamAttachmentActivatedRecord, StreamAttachmentPreparedRecord,
        StreamCascadeDependentResult, StreamConsumerItemValueRecord, StreamConsumerTerminal,
        StreamConsumerTerminalRecord, StreamOffset, StreamSessionMappingRecord,
        StreamSessionMappingUpdateRecord, StreamSessionRecord, StreamSourceUnavailableRecord,
    };
    use golem_common::base_model::{AgentFingerprint, AgentId, IdempotencyKey, OplogIndex};
    use golem_common::model::OwnedAgentId;
    use test_r::test;
    use uuid::Uuid;

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    async fn complete_revert_fold_retains_jump_items_and_discards_suffix_streams() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::oplog::DurableStreamOplogRecord;
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::model::{oplog::OplogEntry, regions::OplogRegion};
        use std::sync::Arc;

        let identity = identity();
        let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamStore::load(
            oplog.clone(),
            owner.environment_id,
            owner.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        producer
            .append_session_record(
                None,
                StreamSessionRecord::Prepared(prepared(&identity.invocation)),
            )
            .await
            .unwrap();
        let root = producer
            .register(None, super_registration(&identity.invocation, 0))
            .await
            .unwrap()
            .value;
        let retained = producer
            .write_items(
                None,
                root.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![17, 43]),
            )
            .await
            .unwrap()
            .value;
        let batch = oplog.current_oplog_index().await;
        let cut_index = oplog
            .add(OplogEntry::jump(
                None,
                OplogRegion {
                    start: batch,
                    end: batch,
                },
            ))
            .await;
        producer
            .write_items(
                None,
                root.stream_id,
                2,
                StreamItemsPayload::PackedU8(vec![99]),
            )
            .await
            .unwrap();
        let removed = producer
            .register(None, super_registration(&identity.invocation, 1))
            .await
            .unwrap()
            .value;
        let end = oplog.current_oplog_index().await;
        let cut = DurableStreamStore::prepare_fork_cut(
            oplog.as_ref(),
            (&owner, identity.fingerprint),
            (&owner, identity.fingerprint),
            end,
            cut_index,
            None,
            [0; 32],
            true,
        )
        .await
        .unwrap();
        let region = OplogRegion {
            start: cut_index.next(),
            end,
        };
        let continuation = cut.streams[0].continuation.clone();
        let marker =
            DurableStreamOplogRecord::Session(None, Box::new(StreamSessionRecord::ForkCut(cut)))
                .into_inline_entry();
        oplog
            .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
            .await;
        let rebuilt = DurableStreamStore::load(
            oplog.clone(),
            owner.environment_id,
            owner.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let head = rebuilt.stream_head(&continuation).await.unwrap();
        assert_eq!(head.offset, retained.last().copied());
        assert!(!head.closed);
        assert!(!head.cancelled);
        assert!(rebuilt.stream_head(&removed).await.is_err());
        let appended = rebuilt
            .write_items(
                None,
                continuation.stream_id,
                2,
                StreamItemsPayload::PackedU8(vec![71]),
            )
            .await
            .unwrap();
        assert!(appended.value[0].producer_oplog_index() > end);
        assert_eq!(
            rebuilt.stream_head(&continuation).await.unwrap().offset,
            appended.value.last().copied()
        );
    }

    #[test]
    async fn reverted_consumer_tail_does_not_advance_source_unavailable_overlay() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::oplog::DurableStreamOplogRecord;
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::model::{oplog::OplogEntry, regions::OplogRegion};
        use std::sync::Arc;

        let key = attachment_key(&identity(), StreamId(Uuid::new_v4()));
        let owner = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let fingerprint = key.expected_consumer_fingerprint;
        let oplog = Arc::new(TestOplog::default());
        let consumer = DurableStreamStore::load(
            oplog.clone(),
            owner.environment_id,
            owner.agent_id.clone(),
            fingerprint,
            None,
        )
        .await
        .unwrap();
        consumer
            .append_session_record(
                None,
                StreamSessionRecord::Prepared(prepared(&key.session_key)),
            )
            .await
            .unwrap();
        let record = |ordinal, value| {
            StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                format_version: 1,
                session_key: key.session_key.clone(),
                stream_id: key.stream_id,
                source_offset: StreamOffset::new(OplogIndex::from_u64(50), ordinal as u32),
                consumer_read_ordinal: ordinal,
                value,
                packed_u8: true,
                recursive_handles: vec![],
                recursive_mappings: vec![],
            })
        };
        consumer
            .append_session_record(None, record(0, vec![17, 43]))
            .await
            .unwrap();
        let cut_index = oplog.current_oplog_index().await;
        consumer
            .append_session_record(None, record(2, vec![99]))
            .await
            .unwrap();
        let end = oplog.current_oplog_index().await;
        let cut = DurableStreamStore::prepare_fork_cut(
            oplog.as_ref(),
            (&owner, fingerprint),
            (&owner, fingerprint),
            end,
            cut_index,
            None,
            [0; 32],
            true,
        )
        .await
        .unwrap();
        let region = OplogRegion {
            start: cut_index.next(),
            end,
        };
        let marker =
            DurableStreamOplogRecord::Session(None, Box::new(StreamSessionRecord::ForkCut(cut)))
                .into_inline_entry();
        oplog
            .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
            .await;
        let rebuilt = DurableStreamStore::load(
            oplog.clone(),
            owner.environment_id,
            owner.agent_id.clone(),
            fingerprint,
            None,
        )
        .await
        .unwrap();
        let next = StreamOffset::new(OplogIndex::from_u64(50), 2);
        assert!(
            !rebuilt
                .commit_source_unavailable_overlay(None, key.clone(), next, 2)
                .await
                .unwrap()
        );
        assert!(
            rebuilt
                .commit_source_unavailable_overlay(None, key, next, 2)
                .await
                .unwrap()
        );
    }

    #[test]
    async fn fork_marker_preparation_retains_discarded_child_provenance_and_prevalidates() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::model::{
            Timestamp,
            oplog::{OplogEntry, OplogPayload},
        };
        use std::sync::Arc;

        let identity = identity();
        let source = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamStore::load(
            oplog.clone(),
            source.environment_id,
            source.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        producer
            .append_session_record(
                None,
                StreamSessionRecord::Prepared(prepared(&identity.invocation)),
            )
            .await
            .unwrap();
        let root = producer
            .register(None, super_registration(&identity.invocation, 0))
            .await
            .unwrap()
            .value;
        let retained = producer
            .write_items(
                None,
                root.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1]]),
            )
            .await
            .unwrap()
            .value[0];
        let mut nested = super_registration(&identity.invocation, 1);
        nested.source_kind = StreamSourceKind::Nested;
        nested.coordinate = StreamRegistrationCoordinate::Nested {
            parent_stream_id: root.stream_id,
            parent_producer_sequence: 1,
            recursive_value_path: vec![],
        };
        producer
            .write_items_with_nested(
                None,
                root.stream_id,
                1,
                StreamItemsPayload::Values(vec![vec![2]]),
                vec![nested],
            )
            .await
            .unwrap();
        let mut owner = source;
        let mut fingerprint = identity.fingerprint;
        let mut history = oplog;
        let mut selected = Some((root.stream_id, Some(retained)));
        for number in 0..2 {
            let target = OwnedAgentId::new(
                identity.environment_id,
                &AgentId {
                    component_id: identity.agent_id.component_id,
                    agent_id: format!("fork-{number}"),
                },
            );
            let target_fingerprint = AgentFingerprint(Uuid::from_u128(600 + number));
            let horizon = history.current_oplog_index().await;
            let cut = DurableStreamStore::prepare_fork_cut(
                history.as_ref(),
                (&owner, fingerprint),
                (&target, target_fingerprint),
                horizon,
                horizon,
                selected,
                [0; 32],
                false,
            )
            .await
            .unwrap();
            assert_eq!(
                cut.streams.len(),
                2,
                "discarded child still has lineage provenance"
            );
            let continued_root = cut
                .streams
                .iter()
                .find(|mapping| mapping.source.stream_id == selected.unwrap().0)
                .unwrap()
                .continuation
                .clone();
            let copied = Arc::new(TestOplog::default());
            for (_, entry) in history
                .read_exact(OplogIndex::INITIAL, horizon.as_u64())
                .await
            {
                copied.add(entry).await;
            }
            copied
                .add(OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
                })
                .await;
            let rebuilt = DurableStreamStore::read_complete_index(
                copied.as_ref(),
                target.environment_id,
                &target.agent_id,
                target_fingerprint,
                copied.current_oplog_index().await,
            )
            .await
            .unwrap();
            assert_eq!(
                rebuilt.registrations.len(),
                1,
                "discarded child must not regain authority"
            );
            assert_eq!(rebuilt.streams[&continued_root.stream_id].next_sequence, 1);
            for invalid in [
                StreamOffset::new(retained.producer_oplog_index(), 1),
                StreamOffset::new(horizon.next(), 0),
            ] {
                assert!(
                    DurableStreamStore::prepare_fork_cut(
                        history.as_ref(),
                        (&owner, fingerprint),
                        (&target, target_fingerprint),
                        horizon,
                        horizon,
                        Some((selected.unwrap().0, Some(invalid))),
                        [0; 32],
                        false,
                    )
                    .await
                    .is_err()
                );
                assert_eq!(history.current_oplog_index().await, horizon);
            }
            selected = Some((continued_root.stream_id, Some(retained)));
            history = copied;
            owner = target;
            fingerprint = target_fingerprint;
        }
    }

    #[test]
    async fn fork_marker_preparation_uses_exact_prefix_and_historical_author() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::model::Timestamp;
        use golem_common::model::oplog::{OplogEntry, OplogPayload};
        use std::sync::Arc;

        let identity = identity();
        let source = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamStore::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        producer
            .append_session_record(
                None,
                StreamSessionRecord::Prepared(prepared(&identity.invocation)),
            )
            .await
            .unwrap();
        let root = producer
            .register(None, super_registration(&identity.invocation, 0))
            .await
            .unwrap()
            .value;
        let first = producer
            .write_items(
                None,
                root.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![17, 43]),
            )
            .await
            .unwrap()
            .value;
        let cut_index = oplog.current_oplog_index().await;
        producer
            .write_items(
                None,
                root.stream_id,
                2,
                StreamItemsPayload::PackedU8(vec![99]),
            )
            .await
            .unwrap();
        let mut later_session = identity.invocation.clone();
        later_session.idempotency_key = IdempotencyKey::new("later".into());
        producer
            .append_session_record(
                None,
                StreamSessionRecord::Prepared(prepared(&later_session)),
            )
            .await
            .unwrap();
        producer
            .register(None, super_registration(&later_session, 1))
            .await
            .unwrap();

        let target = OwnedAgentId::new(
            identity.environment_id,
            &AgentId {
                component_id: identity.agent_id.component_id,
                agent_id: "fork".into(),
            },
        );
        let fingerprint = AgentFingerprint(Uuid::from_u128(420));
        let cut = DurableStreamStore::prepare_fork_cut(
            oplog.as_ref(),
            (&source, identity.fingerprint),
            (&target, fingerprint),
            oplog.current_oplog_index().await,
            cut_index,
            None,
            [0; 32],
            false,
        )
        .await
        .unwrap();
        assert_eq!(cut.source, source.agent_id);
        assert_eq!(cut.streams.len(), 1);
        assert_eq!(cut.streams[0].source, root);
        assert_eq!(cut.sessions.len(), 1);
        assert_eq!(cut.sessions[0].source, identity.invocation);
        let continuation = cut.streams[0].continuation.clone();
        let copied = Arc::new(TestOplog::default());
        for (_, entry) in oplog
            .read_exact(OplogIndex::INITIAL, cut_index.as_u64())
            .await
        {
            copied.add(entry).await;
        }
        copied
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            })
            .await;
        let forked = DurableStreamStore::load(
            copied.clone(),
            target.environment_id,
            target.agent_id.clone(),
            fingerprint,
            None,
        )
        .await
        .unwrap();
        let head = forked.stream_head(&continuation).await.unwrap();
        assert_eq!(head.offset, first.last().copied());
        assert!(!head.closed);
        assert!(!head.cancelled);
        assert_eq!(
            producer
                .input_high_water(root.stream_id)
                .await
                .unwrap()
                .unwrap()
                .highest_contiguous_sequence,
            2
        );

        // Cutting before the parent's own marker retains the original author's identity.
        let next = OwnedAgentId::new(
            identity.environment_id,
            &AgentId {
                component_id: identity.agent_id.component_id,
                agent_id: "next-fork".into(),
            },
        );
        let next_fingerprint = AgentFingerprint(Uuid::from_u128(421));
        let before_marker = DurableStreamStore::prepare_fork_cut(
            copied.as_ref(),
            (&target, fingerprint),
            (&next, next_fingerprint),
            copied.current_oplog_index().await,
            cut_index,
            Some((continuation.stream_id, first.first().copied())),
            [0; 32],
            false,
        )
        .await
        .unwrap();
        assert_eq!(before_marker.source, source.agent_id);
        assert_eq!(before_marker.source_fingerprint, identity.fingerprint);
        assert_eq!(before_marker.selected_stream_id, Some(root.stream_id));
        assert_eq!(before_marker.retained_through, first.first().copied());
        assert_eq!(before_marker.sessions[0].source, identity.invocation);
        assert_ne!(
            before_marker.streams[0].continuation.stream_id,
            continuation.stream_id
        );
        let after_marker = DurableStreamStore::prepare_fork_cut(
            copied.as_ref(),
            (&target, fingerprint),
            (&next, next_fingerprint),
            copied.current_oplog_index().await,
            copied.current_oplog_index().await,
            None,
            [0; 32],
            false,
        )
        .await
        .unwrap();
        assert_eq!(after_marker.source, target.agent_id);
        assert_eq!(after_marker.streams[0].source, continuation);

        // Reverting before the original fork marker retains A's records in F's oplog.
        use crate::services::oplog::DurableStreamOplogRecord;
        use golem_common::base_model::durable_stream::{
            ResumeAttemptDescriptor, StreamResumeOperation, StreamSessionResumeAttemptRecord,
        };
        let session = after_marker.sessions[0].source.clone();
        let mut previous_id = continuation.stream_id;
        for (accepted_epoch, expected_epoch) in [(37, 38), (40, 41)] {
            copied
                .add(
                    DurableStreamOplogRecord::Session(
                        None,
                        Box::new(StreamSessionRecord::ResumeAttempt(
                            StreamSessionResumeAttemptRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                attempt: ResumeAttemptDescriptor {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    operation: StreamResumeOperation::Resume,
                                    session_key: session.clone(),
                                    attachment_id: AttachmentId::primary(
                                        target.environment_id,
                                        &target.agent_id,
                                        &session.idempotency_key,
                                    )
                                    .unwrap(),
                                    expected_callee_fingerprint: fingerprint,
                                    attempt_id: AttemptId::fresh(),
                                    expected_epoch: accepted_epoch - 1,
                                    effective_identity: vec![],
                                    cursors: vec![],
                                    live_join_buffer_events: 1,
                                },
                                accepted_epoch,
                            },
                        )),
                    )
                    .into_inline_entry(),
                )
                .await;
            let horizon = copied.current_oplog_index().await;
            let reverted = DurableStreamStore::prepare_fork_cut(
                copied.as_ref(),
                (&target, fingerprint),
                (&target, fingerprint),
                horizon,
                cut_index,
                None,
                [0; 32],
                true,
            )
            .await
            .unwrap();
            assert_eq!(reverted.source, source.agent_id);
            assert_eq!(reverted.target, target.agent_id);
            assert_eq!(reverted.epoch_floor, expected_epoch);
            let handle = reverted.streams[0].continuation.clone();
            assert_ne!(handle.stream_id, previous_id);
            previous_id = handle.stream_id;
            let region = reverted.revert.clone().unwrap();
            let marker = DurableStreamOplogRecord::Session(
                None,
                Box::new(StreamSessionRecord::ForkCut(reverted)),
            )
            .into_inline_entry();
            copied
                .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
                .await;
            let rebuilt = DurableStreamStore::load(
                copied.clone(),
                target.environment_id,
                target.agent_id.clone(),
                fingerprint,
                None,
            )
            .await
            .unwrap();
            let head = rebuilt.stream_head(&handle).await.unwrap();
            assert_eq!(head.offset, first.last().copied());
            assert!(!head.closed);
            assert!(!head.cancelled);

            let new_horizon = copied.current_oplog_index().await;
            for invalid_cut in [horizon, horizon.next(), new_horizon] {
                assert!(matches!(
                    DurableStreamStore::prepare_fork_cut(
                        copied.as_ref(),
                        (&target, fingerprint),
                        (&target, fingerprint),
                        new_horizon,
                        invalid_cut,
                        None,
                        [0; 32],
                        true,
                    )
                    .await,
                    Err(StreamStoreError::InvalidOffset(_))
                ));
                assert_eq!(copied.current_oplog_index().await, new_horizon);
            }
            let next_revert = DurableStreamStore::prepare_fork_cut(
                copied.as_ref(),
                (&target, fingerprint),
                (&target, fingerprint),
                new_horizon,
                cut_index,
                None,
                [0; 32],
                true,
            )
            .await
            .unwrap();
            assert_eq!(next_revert.epoch_floor, expected_epoch + 1);
        }

        let earlier = DurableStreamStore::prepare_fork_cut(
            copied.as_ref(),
            (&target, fingerprint),
            (&target, fingerprint),
            copied.current_oplog_index().await,
            cut_index.previous(),
            None,
            [0; 32],
            true,
        )
        .await
        .unwrap();
        let region = earlier.revert.clone().unwrap();
        let marker = DurableStreamOplogRecord::Session(
            None,
            Box::new(StreamSessionRecord::ForkCut(earlier)),
        )
        .into_inline_entry();
        copied
            .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
            .await;
        let historical = DurableStreamStore::prepare_fork_cut(
            copied.as_ref(),
            (&target, fingerprint),
            (&next, next_fingerprint),
            copied.current_oplog_index().await,
            cut_index,
            None,
            [0; 32],
            false,
        )
        .await
        .unwrap();
        assert_eq!(historical.source, source.agent_id);
        assert_eq!(historical.streams[0].source, root);

        use golem_common::base_model::durable_stream::StreamSessionAttachedRecord;
        copied
            .add(
                DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session.clone(),
                        attachment_id: AttachmentId::primary(
                            target.environment_id,
                            &target.agent_id,
                            &session.idempotency_key,
                        )
                        .unwrap(),
                        attempt_id: AttemptId::fresh(),
                        epoch: u64::MAX,
                        pending_invocation_oplog_index: OplogIndex::INITIAL,
                    })),
                )
                .into_inline_entry(),
            )
            .await;
        let horizon = copied.current_oplog_index().await;
        assert!(matches!(
            DurableStreamStore::prepare_fork_cut(
                copied.as_ref(),
                (&target, fingerprint),
                (&target, fingerprint),
                horizon,
                cut_index.previous(),
                None,
                [0; 32],
                true,
            )
            .await,
            Err(StreamStoreError::CounterOverflow)
        ));
        assert_eq!(copied.current_oplog_index().await, horizon);
    }

    #[test]
    async fn revert_epoch_fences_a_deleted_session_prepared_again_under_the_same_key() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::oplog::DurableStreamOplogRecord;
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::base_model::durable_stream::StreamSessionAttachedRecord;

        let identity = identity();
        let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let oplog = TestOplog::default();
        let first_cut = oplog.add(OplogEntry::no_op(None)).await;
        let entry =
            |record| DurableStreamOplogRecord::Session(None, Box::new(record)).into_inline_entry();
        let attached = |epoch| {
            StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                attachment_id: AttachmentId::primary(
                    owner.environment_id,
                    &owner.agent_id,
                    &identity.invocation.idempotency_key,
                )
                .unwrap(),
                attempt_id: AttemptId::fresh(),
                epoch,
                pending_invocation_oplog_index: OplogIndex::INITIAL,
            })
        };
        oplog
            .add(entry(StreamSessionRecord::Prepared(prepared(
                &identity.invocation,
            ))))
            .await;
        oplog.add(entry(attached(5))).await;
        let cut = DurableStreamStore::prepare_fork_cut(
            &oplog,
            (&owner, identity.fingerprint),
            (&owner, identity.fingerprint),
            oplog.current_oplog_index().await,
            first_cut,
            None,
            [0; 32],
            true,
        )
        .await
        .unwrap();
        assert!(cut.sessions.is_empty());
        let region = cut.revert.clone().unwrap();
        let marker = entry(StreamSessionRecord::ForkCut(cut));
        oplog
            .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
            .await;
        let second_cut = oplog
            .add(entry(StreamSessionRecord::Prepared(prepared(
                &identity.invocation,
            ))))
            .await;
        oplog.add(entry(attached(1))).await;
        let cut = DurableStreamStore::prepare_fork_cut(
            &oplog,
            (&owner, identity.fingerprint),
            (&owner, identity.fingerprint),
            oplog.current_oplog_index().await,
            second_cut,
            None,
            [0; 32],
            true,
        )
        .await
        .unwrap();
        assert_eq!(cut.sessions.len(), 1);
        assert_eq!(cut.epoch_floor, 7);
    }

    #[test]
    async fn fork_cut_validation_rejects_a_malformed_request_hash() {
        use crate::durable_host::durable_stream::tests::TestOplog;

        let identity = identity();
        let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let oplog = TestOplog::default();
        let cut_index = oplog.add(OplogEntry::no_op(None)).await;
        let mut cut = DurableStreamStore::prepare_fork_cut(
            &oplog,
            (&owner, identity.fingerprint),
            (&owner, identity.fingerprint),
            cut_index,
            cut_index,
            None,
            [0; 32],
            false,
        )
        .await
        .unwrap();

        cut.request_hash.clear();
        assert!(
            StreamForkLineage::validate_fork_cut(&oplog, cut)
                .await
                .is_err(),
            "the persisted request hash must remain a 32-byte digest"
        );
    }

    #[test]
    async fn streaming_acceptance_cuts_preserve_the_whole_batch() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::model::AgentInvocationPayload;
        use golem_common::model::invocation_context::TraceId;
        use golem_common::model::oplog::OplogPayload;
        use std::sync::Arc;

        let identity = identity();
        let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let oplog = Arc::new(TestOplog::default());
        oplog.add(OplogEntry::no_op(None)).await;
        let descriptor = prepared(&identity.invocation);
        let requests: Vec<_> = (0..2)
            .map(|path| {
                let mut request = super_registration(&identity.invocation, path);
                if let StreamRegistrationCoordinate::Root { root_kind, .. } =
                    &mut request.coordinate
                {
                    *root_kind = StreamRootKind::MethodInput;
                }
                request.source_kind = StreamSourceKind::ExternalInlineInput;
                request.session_mapping = Some(StreamSessionMapping {
                    session_key: identity.invocation.clone(),
                    attachment_id: descriptor.attempt.attachment_id,
                    role: SessionStreamRole::Input,
                });
                (u64::from(path), request)
            })
            .collect();
        let pending = OplogEntry::pending_agent_invocation(
            identity.invocation.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            vec![],
            vec![],
        );
        for expected_epoch in 1..=3 {
            let producer = DurableStreamStore::load(
                oplog.clone(),
                owner.environment_id,
                owner.agent_id.clone(),
                identity.fingerprint,
                None,
            )
            .await
            .unwrap();
            let (committed, _) = tokio::sync::oneshot::channel();
            let mut descriptor = descriptor.clone();
            producer
                .prepare_session(
                    None,
                    requests.clone(),
                    vec![],
                    pending.clone(),
                    committed,
                    move |bindings| {
                        descriptor.attempt.invocation.stream_handles =
                            bindings.iter().map(|(_, h)| h.clone()).collect();
                        descriptor.stream_mappings = bindings
                            .into_iter()
                            .map(|(transport_stream_id, handle)| StreamSessionMappingRecord {
                                transport_stream_id,
                                handle,
                                role: SessionStreamRole::Input,
                            })
                            .collect();
                        descriptor
                    },
                )
                .await
                .unwrap();
            let horizon = oplog.current_oplog_index().await;
            let OplogEntry::StreamSession { record, .. } = oplog.read(horizon).await else {
                panic!("missing Attached")
            };
            let StreamSessionRecord::Attached(attached) =
                oplog.download_payload(record).await.unwrap()
            else {
                panic!("missing Attached")
            };
            assert_eq!(attached.epoch, expected_epoch);
            for cut in (horizon.as_u64() - 5)..=horizon.as_u64() {
                assert_eq!(
                    crate::worker::cut_point::streaming_acceptance_spans_cut(
                        oplog.as_ref(),
                        OplogIndex::from_u64(cut),
                        horizon,
                    )
                    .await
                    .unwrap(),
                    cut > horizon.as_u64() - 5 && cut < horizon.as_u64(),
                    "cut at {cut}"
                );
            }
            let cut = DurableStreamStore::prepare_fork_cut(
                oplog.as_ref(),
                (&owner, identity.fingerprint),
                (&owner, identity.fingerprint),
                horizon,
                OplogIndex::INITIAL,
                None,
                [0; 32],
                true,
            )
            .await
            .unwrap();
            assert!(cut.sessions.is_empty());
            let region = cut.revert.clone().unwrap();
            let marker = crate::services::oplog::DurableStreamOplogRecord::Session(
                None,
                Box::new(StreamSessionRecord::ForkCut(cut)),
            )
            .into_inline_entry();
            oplog
                .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
                .await;
        }
    }

    #[test]
    async fn fork_after_bare_revert_uses_the_fixed_horizon_and_original_author() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::oplog::DurableStreamOplogRecord;

        let identity = identity();
        let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let target = OwnedAgentId::new(
            owner.environment_id,
            &AgentId {
                component_id: owner.agent_id.component_id,
                agent_id: "bare-revert-fork".into(),
            },
        );
        let fingerprint = AgentFingerprint(Uuid::from_u128(422));
        let oplog = TestOplog::default();
        for _ in 0..3 {
            oplog.add(OplogEntry::no_op(None)).await;
        }
        let horizon = oplog
            .add(OplogEntry::revert(OplogRegion::from_range(2..=3)))
            .await;
        let old_cut = OplogIndex::from_u64(2);
        let cut = DurableStreamStore::prepare_fork_cut(
            &oplog,
            (&owner, identity.fingerprint),
            (&target, fingerprint),
            horizon,
            old_cut,
            None,
            [0; 32],
            false,
        )
        .await
        .unwrap();
        assert_eq!(cut.source, owner.agent_id);
        let cut = DurableStreamStore::prepare_fork_cut(
            &oplog,
            (&owner, identity.fingerprint),
            (&target, fingerprint),
            horizon,
            horizon,
            None,
            [0; 32],
            false,
        )
        .await
        .unwrap();
        oplog
            .add(
                DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecord::ForkCut(cut)),
                )
                .into_inline_entry(),
            )
            .await;
        let cut = DurableStreamStore::prepare_fork_cut(
            &oplog,
            (&target, fingerprint),
            (&owner, identity.fingerprint),
            oplog.current_oplog_index().await,
            old_cut,
            None,
            [0; 32],
            false,
        )
        .await
        .unwrap();
        assert_eq!(cut.source, owner.agent_id);
        assert_eq!(cut.source_fingerprint, identity.fingerprint);
    }

    fn continuation_session(source: &StreamSessionKey, suffix: &str) -> StreamSessionKey {
        let mut result = source.clone();
        result.callee.agent_id = format!("fork-{suffix}");
        result.callee_fingerprint = AgentFingerprint(Uuid::from_u128(300));
        result
    }

    fn register(
        index: &mut ProducerStreamIndex,
        oplog_index: u64,
        request: ProducerRegistrationRequest,
    ) -> DurableStreamHandle {
        let identity = identity();
        let oplog_index = OplogIndex::from_u64(oplog_index);
        let record = registration_record(
            oplog_index,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            request,
        );
        let handle = record.handle.clone();
        index
            .apply_registration(
                oplog_index,
                None,
                record,
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
            .unwrap();
        handle
    }

    fn continuation(source: &DurableStreamHandle, suffix: &str) -> DurableStreamHandle {
        let mut result = source.clone();
        let target = AgentId {
            component_id: source.producer.component_id,
            agent_id: format!("fork-{suffix}"),
        };
        let mut cut = marker(Vec::new(), Vec::new(), None, None);
        cut.target = target.clone();
        result.stream_id = continuation_stream_id(&cut, source.stream_id).unwrap();
        result.producer = target.clone();
        result.expected_producer_fingerprint = AgentFingerprint(Uuid::from_u128(300));
        if result.source_invocation.callee_environment_id == cut.source_environment_id
            && result.source_invocation.callee == cut.source
            && result.source_invocation.callee_fingerprint == cut.source_fingerprint
        {
            result.source_invocation.callee = target;
            result.source_invocation.callee_fingerprint = cut.target_fingerprint;
        }
        result
    }

    fn validate_marker(index: &ProducerStreamIndex, record: &StreamForkCutRecord) {
        let registrations = index
            .registrations
            .values()
            .cloned()
            .map(|registration| (registration.registration_oplog_index, registration))
            .collect::<Vec<_>>();
        let owner = OwnedAgentId::new(record.target_environment_id, &record.target);
        StreamForkLineage::validate(
            vec![(record.cut_index.next(), record.clone())],
            &registrations,
            &owner,
            record.target_fingerprint,
        )
        .unwrap();
        assert!(
            record.sessions.iter().all(|mapping| mapping
                .continuation_attempt_id
                .0
                .get_version_num()
                == 4)
        );
    }

    fn marker(
        streams: Vec<(DurableStreamHandle, DurableStreamHandle)>,
        sessions: Vec<(StreamSessionKey, StreamSessionKey)>,
        selected: Option<StreamId>,
        retained_through: Option<golem_common::base_model::durable_stream::StreamOffset>,
    ) -> StreamForkCutRecord {
        let identity = identity();
        StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            export: None,
            source_environment_id: identity.environment_id,
            source: identity.agent_id.clone(),
            source_fingerprint: identity.fingerprint,
            target_environment_id: identity.environment_id,
            target: sessions
                .first()
                .map(|(_, continuation)| continuation.callee.clone())
                .unwrap_or_else(|| AgentId {
                    component_id: identity.agent_id.component_id,
                    agent_id: "fork-default".to_string(),
                }),
            target_fingerprint: AgentFingerprint(Uuid::from_u128(300)),
            cut_index: OplogIndex::from_u64(20),
            revert: None,
            epoch_floor: 1,
            selected_stream_id: selected,
            retained_through,
            streams: streams
                .into_iter()
                .map(|(source, continuation)| StreamForkStreamMapping {
                    source,
                    continuation,
                })
                .collect(),
            sessions: sessions
                .into_iter()
                .map(|(source, continuation)| StreamForkSessionMapping {
                    source,
                    continuation,
                    continuation_attempt_id: AttemptId::fresh(),
                })
                .collect(),
        }
    }

    #[test]
    fn fork_cut_is_marker_time_partial_transition_and_discards_nested_suffix() {
        let identity = identity();
        let source_session = identity.invocation.clone();
        let target_session = continuation_session(&source_session, "fork");
        let mut index = ProducerStreamIndex::default();
        let root = register(
            &mut index,
            2,
            ProducerRegistrationRequest {
                coordinate: StreamRegistrationCoordinate::Root {
                    invocation_id: source_session.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: source_session.clone(),
                entity_parent_start_index: None,
                component_revision: golem_common::base_model::component::ComponentRevision::INITIAL,
                element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKind::InvocationOutput,
                session_mapping: Some(StreamSessionMapping {
                    session_key: source_session.clone(),
                    attachment_id: AttachmentId::primary(
                        source_session.callee_environment_id,
                        &source_session.callee,
                        &source_session.idempotency_key,
                    )
                    .unwrap(),
                    role: SessionStreamRole::Output,
                }),
            },
        );
        index
            .apply_items(
                OplogIndex::from_u64(3),
                None,
                StreamItemsRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id: root.stream_id,
                    producer_fingerprint: identity.fingerprint,
                    first_sequence: 0,
                    nested_stream_ids: Vec::new(),
                    newly_registered_stream_ids: Vec::new(),
                    payload: StreamItemsPayload::PackedU8(vec![10, 11, 12]),
                    offsets: (0..3)
                        .map(|sub| {
                            golem_common::base_model::durable_stream::StreamOffset::new(
                                OplogIndex::from_u64(3),
                                sub,
                            )
                        })
                        .collect(),
                },
                identity.fingerprint,
            )
            .unwrap();
        let child = register(
            &mut index,
            5,
            ProducerRegistrationRequest {
                coordinate: StreamRegistrationCoordinate::Nested {
                    parent_stream_id: root.stream_id,
                    parent_producer_sequence: 3,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: source_session.clone(),
                entity_parent_start_index: None,
                component_revision: root.component_revision,
                element_schema_fingerprint: root.element_schema_fingerprint,
                source_kind: StreamSourceKind::Nested,
                session_mapping: None,
            },
        );
        index
            .apply_items(
                OplogIndex::from_u64(6),
                None,
                StreamItemsRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id: root.stream_id,
                    producer_fingerprint: identity.fingerprint,
                    first_sequence: 3,
                    nested_stream_ids: vec![child.stream_id],
                    newly_registered_stream_ids: vec![child.stream_id],
                    payload: StreamItemsPayload::Values(vec![vec![3]]),
                    offsets: vec![golem_common::base_model::durable_stream::StreamOffset::new(
                        OplogIndex::from_u64(6),
                        0,
                    )],
                },
                identity.fingerprint,
            )
            .unwrap();
        for (oplog_index, stream_id, sequence) in [(7, root.stream_id, 4), (8, child.stream_id, 0)]
        {
            let event = index
                .apply_end(
                    OplogIndex::from_u64(oplog_index),
                    None,
                    StreamEndRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint: identity.fingerprint,
                        sequence,
                        offset: golem_common::base_model::durable_stream::StreamOffset::new(
                            OplogIndex::from_u64(oplog_index),
                            0,
                        ),
                        result: StreamEndResult::Ok,
                        authored_by: StreamTerminalAuthor::Protocol,
                    },
                    identity.fingerprint,
                )
                .unwrap();
            index.streams.get_mut(&stream_id).unwrap().terminal_event = Some(event);
        }
        let mut completed = index.clone();
        completed
            .apply_finished(&StreamSessionFinishedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: source_session.clone(),
                result: Ok(()),
            })
            .unwrap();
        let root_next = continuation(&root, "fork");
        let child_next = continuation(&child, "fork");
        let retained =
            golem_common::base_model::durable_stream::StreamOffset::new(OplogIndex::from_u64(3), 1);
        let fork = marker(
            vec![
                (root.clone(), root_next.clone()),
                (child, child_next.clone()),
            ],
            vec![(source_session, target_session.clone())],
            Some(root.stream_id),
            Some(retained),
        );
        validate_marker(&index, &fork);
        assert!(completed.apply_fork_cut(&fork, Some(2)).is_err());
        let original_registrations = index
            .registrations
            .values()
            .cloned()
            .map(|registration| (registration.registration_oplog_index, registration))
            .collect::<Vec<_>>();
        index.apply_fork_cut(&fork, Some(2)).unwrap();
        assert!(!index.registrations.contains_key(&root.stream_id));
        assert!(index.registrations.contains_key(&root_next.stream_id));
        assert!(!index.registrations.contains_key(&child_next.stream_id));
        assert_eq!(index.streams[&root_next.stream_id].next_sequence, 2);
        assert_eq!(
            index.batch_positions[&(root_next.stream_id, OplogIndex::from_u64(3))],
            (0, 2)
        );
        assert!(!index.finished_sessions.contains(&target_session));
        let registration_mapping = index.registrations[&root_next.stream_id]
            .session_mapping
            .as_ref()
            .unwrap();
        assert_eq!(registration_mapping.session_key, target_session);
        assert_eq!(
            registration_mapping.attachment_id,
            AttachmentId::primary(
                target_session.callee_environment_id,
                &target_session.callee,
                &target_session.idempotency_key,
            )
            .unwrap()
        );
        assert_eq!(
            index.streams[&root_next.stream_id].last_item_offset,
            Some(retained)
        );

        // This is applied after the marker and therefore survives; projecting the cut globally
        // after the complete fold would incorrectly discard it.
        index
            .apply_items(
                OplogIndex::from_u64(22),
                None,
                StreamItemsRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id: root_next.stream_id,
                    producer_fingerprint: AgentFingerprint(Uuid::from_u128(300)),
                    first_sequence: 2,
                    nested_stream_ids: Vec::new(),
                    newly_registered_stream_ids: Vec::new(),
                    payload: StreamItemsPayload::Values(vec![vec![4]]),
                    offsets: vec![golem_common::base_model::durable_stream::StreamOffset::new(
                        OplogIndex::from_u64(22),
                        0,
                    )],
                },
                AgentFingerprint(Uuid::from_u128(300)),
            )
            .unwrap();
        assert_eq!(index.streams[&root_next.stream_id].next_sequence, 3);

        // A discarded child's mapping is retained provenance, not selectable authority.
        let mut second = fork.clone();
        second.source = fork.target.clone();
        second.source_fingerprint = fork.target_fingerprint;
        second.target.agent_id = "second-fork".into();
        second.target_fingerprint = AgentFingerprint(Uuid::from_u128(301));
        second.cut_index = OplogIndex::from_u64(22);
        second.selected_stream_id = Some(child_next.stream_id);
        second.retained_through = None;
        second.sessions = fork
            .sessions
            .iter()
            .map(|mapping| {
                let mut continuation = mapping.continuation.clone();
                continuation.callee = second.target.clone();
                continuation.callee_fingerprint = second.target_fingerprint;
                StreamForkSessionMapping {
                    source: mapping.continuation.clone(),
                    continuation,
                    continuation_attempt_id: AttemptId::fresh(),
                }
            })
            .collect();
        second.streams = fork
            .streams
            .iter()
            .map(|mapping| {
                let source = mapping.continuation.clone();
                let mut continuation = source.clone();
                continuation.stream_id = continuation_stream_id(&second, source.stream_id).unwrap();
                continuation.producer = second.target.clone();
                continuation.expected_producer_fingerprint = second.target_fingerprint;
                continuation.source_invocation = second.sessions[0].continuation.clone();
                StreamForkStreamMapping {
                    source,
                    continuation,
                }
            })
            .collect();
        StreamForkLineage::validate(
            vec![
                (fork.cut_index.next(), fork),
                (second.cut_index.next(), second.clone()),
            ],
            &original_registrations,
            &OwnedAgentId::new(second.target_environment_id, &second.target),
            second.target_fingerprint,
        )
        .unwrap();
        assert!(matches!(
            index.apply_fork_cut(&second, Some(0)),
            Err(StreamStoreError::CorruptHistory(_))
        ));
        assert!(index.registrations.contains_key(&root_next.stream_id));
    }

    #[test]
    fn fork_cut_preserves_other_terminal_and_maps_output_only_session() {
        let identity = identity();
        let source_session = identity.invocation.clone();
        let target_session = continuation_session(&source_session, "fork-output-only");
        let mut output_only_source = source_session.clone();
        output_only_source.idempotency_key = IdempotencyKey::new("output-only".to_string());
        let mut output_only_target = output_only_source.clone();
        output_only_target.callee = target_session.callee.clone();
        output_only_target.callee_fingerprint = target_session.callee_fingerprint;
        let mut index = ProducerStreamIndex::default();
        let first = register(&mut index, 5, super_registration(&source_session, 0));
        let second = register(&mut index, 6, super_registration(&source_session, 1));
        for (oplog_index, stream_id) in [(7, first.stream_id), (8, second.stream_id)] {
            let event = index
                .apply_end(
                    OplogIndex::from_u64(oplog_index),
                    None,
                    StreamEndRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint: identity.fingerprint,
                        sequence: 0,
                        offset: golem_common::base_model::durable_stream::StreamOffset::new(
                            OplogIndex::from_u64(oplog_index),
                            0,
                        ),
                        result: StreamEndResult::Ok,
                        authored_by: StreamTerminalAuthor::Protocol,
                    },
                    identity.fingerprint,
                )
                .unwrap();
            index.streams.get_mut(&stream_id).unwrap().terminal_event = Some(event);
        }
        index
            .session_entity_parent_start_indices
            .insert(output_only_source.clone(), None);
        index
            .invocation_results
            .insert(output_only_source.clone(), OplogIndex::from_u64(9));
        index.deleting = true;
        index.consumer_deleting = true;
        let first_next = continuation(&first, "fork-output-only");
        let second_next = continuation(&second, "fork-output-only");
        let fork = marker(
            vec![
                (first.clone(), first_next.clone()),
                (second, second_next.clone()),
            ],
            vec![
                (source_session, target_session),
                (output_only_source.clone(), output_only_target.clone()),
            ],
            Some(first.stream_id),
            None,
        );
        validate_marker(&index, &fork);
        index.apply_fork_cut(&fork, Some(0)).unwrap();
        assert!(index.streams[&second_next.stream_id].terminal);
        let terminal = index.streams[&second_next.stream_id]
            .terminal_event
            .as_ref()
            .unwrap();
        assert_eq!(terminal.stream_id, second_next.stream_id);
        assert_eq!(
            terminal.offset,
            golem_common::base_model::durable_stream::StreamOffset::new(OplogIndex::from_u64(8), 0)
        );
        assert_eq!(
            terminal.payload,
            CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
        );
        assert!(!index.streams[&first_next.stream_id].terminal);
        assert!(!index.registrations.contains_key(&first.stream_id));
        assert_eq!(index.fork_aliases[&first.stream_id], first_next);
        assert!(index.invocation_results.contains_key(&output_only_target));
        assert!(!index.deleting && !index.consumer_deleting);
        assert!(index.attachments.is_empty() && index.cascade_outbox.is_empty());
    }

    #[test]
    fn fork_cut_preserves_foreign_dependency_journals_and_clears_live_authority() {
        let identity = identity();
        let local = identity.invocation.clone();
        let target = continuation_session(&local, "dependencies");
        let mut remote = local.clone();
        remote.callee.agent_id = "remote".into();
        remote.callee_fingerprint = AgentFingerprint(Uuid::from_u128(700));
        let mut index = ProducerStreamIndex::default();
        let mut request = super_registration(&remote, 0);
        request.source_kind = StreamSourceKind::AgentHostedInput;
        request.coordinate = StreamRegistrationCoordinate::Root {
            invocation_id: remote.clone(),
            root_kind: StreamRootKind::MethodInput,
            recursive_value_path: vec![],
        };
        let input = register(&mut index, 2, request);
        let input_next = continuation(&input, "dependencies");
        let mut foreign = input.clone();
        foreign.producer = remote.callee.clone();
        foreign.expected_producer_fingerprint = remote.callee_fingerprint;
        foreign.stream_id = StreamId::derive(
            remote.callee_environment_id,
            &remote.callee,
            remote.callee_fingerprint,
            OplogIndex::from_u64(7),
        )
        .unwrap();
        for session in [&local, &remote] {
            let attribution = (session == &local).then_some(OplogIndex::from_u64(1));
            index
                .apply_session_attribution(session, attribution)
                .unwrap();
            index
                .apply_session_references(
                    attribution,
                    &StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session.clone(),
                        mapping: StreamSessionMappingRecord {
                            transport_stream_id: 0,
                            handle: foreign.clone(),
                            role: SessionStreamRole::Output,
                        },
                    }),
                )
                .unwrap();
            index
                .invocation_results
                .insert(session.clone(), OplogIndex::from_u64(10));
            index
                .apply_consumer_journal_record(&StreamSessionRecord::ConsumerItemValue(
                    StreamConsumerItemValueRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session.clone(),
                        stream_id: foreign.stream_id,
                        source_offset: StreamOffset::new(OplogIndex::from_u64(8), 0),
                        consumer_read_ordinal: 0,
                        value: vec![4, 5, 6],
                        packed_u8: true,
                        recursive_handles: vec![],
                        recursive_mappings: vec![],
                    },
                ))
                .unwrap();
        }
        index
            .apply_consumer_journal_record(&StreamSessionRecord::ConsumerTerminal(
                StreamConsumerTerminalRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: local.clone(),
                    stream_id: foreign.stream_id,
                    source_offset: StreamOffset::new(OplogIndex::from_u64(9), 0),
                    consumer_read_ordinal: 3,
                    terminal: StreamConsumerTerminal::End(StreamEndResult::Ok),
                },
            ))
            .unwrap();
        let mut unavailable_key = attachment_key(&identity, foreign.stream_id);
        unavailable_key.session_key = remote.clone();
        unavailable_key.attachment_id = AttachmentId::primary(
            remote.callee_environment_id,
            &remote.callee,
            &remote.idempotency_key,
        )
        .unwrap();
        // Neither endpoint belongs to the forked agent, so this epoch remains unchanged.
        unavailable_key.epoch = 7;
        let unavailable = StreamOffset::new(OplogIndex::from_u64(10), 0);
        index
            .apply_consumer_journal_record(&StreamSessionRecord::SourceUnavailable(
                StreamSourceUnavailableRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: unavailable_key.clone(),
                    source_offset: unavailable,
                    consumer_read_ordinal: 3,
                },
            ))
            .unwrap();

        let key = attachment_key(&identity, input.stream_id);
        index
            .apply_attachment_record(
                &StreamSessionRecord::AttachmentPrepared(StreamAttachmentPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    prepared_at_millis: 1,
                    lease_expires_at_millis: 100,
                }),
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
            .unwrap();
        index
            .apply_attachment_record(
                &StreamSessionRecord::AttachmentActivated(StreamAttachmentActivatedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    activated_at_millis: 2,
                    lease_expires_at_millis: 100,
                }),
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
            .unwrap();
        let attachment_tuple = (
            key.attachment_id,
            key.stream_id,
            key.consumer_environment_id,
            key.consumer.clone(),
        );
        index
            .attachment_pages
            .insert(0, vec![attachment_tuple.clone()]);
        index.attachment_positions.insert(attachment_tuple, Some(0));
        index.active_attachment_count = 1;
        index
            .cascade_outbox
            .insert(key, StreamCascadeDependentResult::ConsumerJournalComplete);
        let producer_id = ExternalProducerId::Client("source-writer".into());
        index.external_producer_heads.insert(
            (remote.clone(), input.stream_id, producer_id.clone()),
            IndexedExternalProducer {
                epoch: 2,
                last_sequence: 0,
                next_sequence: 1,
                last_offset: unavailable,
            },
        );
        index.external_producer_offsets.insert(
            (remote.clone(), input.stream_id, producer_id, 2, 0),
            unavailable,
        );
        assert!(!index.attachments.is_empty());
        assert!(index.active_attachment_count > 0);
        let fork = marker(
            vec![(input.clone(), input_next.clone())],
            vec![(local.clone(), target.clone())],
            None,
            None,
        );
        validate_marker(&index, &fork);
        index.apply_fork_cut(&fork, None).unwrap();
        assert_eq!(index.stream_sessions[&input_next.stream_id], remote);
        assert_eq!(
            index.registrations[&input_next.stream_id]
                .handle
                .source_invocation,
            remote
        );
        assert_eq!(index.referenced_handles[&foreign.stream_id].0, foreign);
        assert_eq!(
            index.referenced_handles[&foreign.stream_id].1,
            HashSet::from([target.clone(), remote.clone()])
        );
        for session in [&target, &remote] {
            assert_eq!(
                index.session_stream_counts[session],
                if session == &target { 1 } else { 2 }
            );
            assert_eq!(index.invocation_results[session], OplogIndex::from_u64(10));
            assert_eq!(
                index.session_entity_parent_start_indices[session],
                (session == &target).then_some(OplogIndex::from_u64(1))
            );
        }
        let journal = &index.consumer_journals[&(target, foreign.stream_id)];
        assert_eq!(journal.next_read_ordinal, 4);
        assert!(journal.terminal);
        let journal = &index.consumer_journals[&(remote, foreign.stream_id)];
        assert_eq!(journal.next_read_ordinal, 3);
        assert_eq!(
            journal.source_unavailable,
            Some((unavailable_key, unavailable))
        );
        assert!(index.attachments.is_empty());
        assert!(index.active_attachments_by_session_stream.is_empty());
        assert_eq!(index.active_attachment_count, 0);
        assert!(index.attachment_pages.is_empty());
        assert!(index.attachment_positions.is_empty());
        assert!(index.cascade_outbox.is_empty());
        assert!(index.external_producer_heads.is_empty());
        assert!(index.external_producer_offsets.is_empty());
    }

    #[test]
    async fn fork_cut_source_unavailable_remains_queryable_by_continuation_consumer() {
        use crate::durable_host::durable_stream::{DurableStreamStore, tests::TestOplog};
        use std::sync::Arc;
        let identity = identity();
        let local = identity.invocation.clone();
        let target = continuation_session(&local, "unavailable");
        let mut remote = local.clone();
        remote.callee.agent_id = "remote".into();
        remote.callee_fingerprint = AgentFingerprint(Uuid::from_u128(701));
        for session in [&local, &remote] {
            let mut key = attachment_key(&identity, StreamId(Uuid::from_u128(702)));
            key.producer = remote.callee.clone();
            key.expected_producer_fingerprint = remote.callee_fingerprint;
            key.consumer_environment_id = identity.environment_id;
            key.consumer = identity.agent_id.clone();
            key.expected_consumer_fingerprint = identity.fingerprint;
            key.consumer_invocation = local.clone();
            key.session_key = session.clone();
            key.epoch = 4;
            key.attachment_id = AttachmentId::primary(
                session.callee_environment_id,
                &session.callee,
                &session.idempotency_key,
            )
            .unwrap();
            let offset = StreamOffset::new(OplogIndex::from_u64(8), 3);
            let mut index = ProducerStreamIndex::default();
            index.apply_session_attribution(&local, None).unwrap();
            index
                .apply_consumer_journal_record(&StreamSessionRecord::SourceUnavailable(
                    StreamSourceUnavailableRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        key: key.clone(),
                        source_offset: offset,
                        consumer_read_ordinal: 0,
                    },
                ))
                .unwrap();
            let fork = marker(vec![], vec![(local.clone(), target.clone())], None, None);
            validate_marker(&index, &fork);
            index.apply_fork_cut(&fork, None).unwrap();
            assert!(index.attachments.is_empty());
            assert!(index.cascade_outbox.is_empty());
            let mut expected = key.clone();
            expected.consumer = target.callee.clone();
            expected.expected_consumer_fingerprint = target.callee_fingerprint;
            expected.consumer_invocation = target.clone();
            expected.epoch = 1;
            if session == &local {
                expected.session_key = target.clone();
                expected.attachment_id = AttachmentId::primary(
                    target.callee_environment_id,
                    &target.callee,
                    &target.idempotency_key,
                )
                .unwrap();
            }
            let producer = DurableStreamStore::from_index(
                Arc::new(TestOplog::default()),
                identity.environment_id,
                target.callee.clone(),
                target.callee_fingerprint,
                8,
                Arc::new(|_| Box::pin(async {})),
                index,
            )
            .unwrap();
            assert_eq!(
                producer
                    .consumer_source_unavailable(&expected)
                    .await
                    .unwrap(),
                Some(offset)
            );
            assert!(producer.consumer_source_unavailable(&key).await.is_err());
        }
    }

    #[test]
    async fn fork_cut_full_reconstruction_uses_historical_authors_and_retains_later_writes() {
        use crate::durable_host::durable_stream::{DurableStreamStore, tests::TestOplog};
        use crate::services::oplog::{Oplog, OplogOps};
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::model::{Timestamp, oplog::OplogEntry};

        let identity = identity();
        let target_session = continuation_session(&identity.invocation, "reconstruction");
        let registration = registration_record(
            OplogIndex::from_u64(2),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            super_registration(&identity.invocation, 0),
        );
        for sub_index in [1, 3] {
            let mut cut = marker(
                vec![],
                vec![(identity.invocation.clone(), target_session.clone())],
                Some(registration.handle.stream_id),
                Some(StreamOffset::new(OplogIndex::from_u64(3), sub_index)),
            );
            cut.cut_index = OplogIndex::from_u64(1024);
            let mut next = continuation(&registration.handle, "reconstruction");
            next.stream_id = continuation_stream_id(&cut, registration.handle.stream_id).unwrap();
            cut.streams.push(StreamForkStreamMapping {
                source: registration.handle.clone(),
                continuation: next.clone(),
            });
            let oplog = TestOplog::default();
            for position in 1..=1026 {
                let timestamp = Timestamp::now_utc();
                let entry = match position {
                    1 => OplogEntry::StreamSession {
                        timestamp,
                        entity_parent_start_index: None,
                        record: oplog
                            .upload_payload(&StreamSessionRecord::Prepared(prepared(
                                &identity.invocation,
                            )))
                            .await
                            .unwrap(),
                    },
                    2 => OplogEntry::StreamRegistered {
                        timestamp,
                        entity_parent_start_index: None,
                        record: oplog.upload_payload(&registration).await.unwrap(),
                    },
                    3 | 1026 => {
                        let (stream, fingerprint, first_sequence, bytes) = if position == 3 {
                            (
                                registration.handle.stream_id,
                                identity.fingerprint,
                                0,
                                vec![5, 6, 7],
                            )
                        } else {
                            (
                                next.stream_id,
                                next.expected_producer_fingerprint,
                                2,
                                vec![8, 9],
                            )
                        };
                        let offsets = (0..bytes.len())
                            .map(|index| {
                                StreamOffset::new(OplogIndex::from_u64(position), index as u32)
                            })
                            .collect();
                        OplogEntry::StreamItems {
                            timestamp,
                            entity_parent_start_index: None,
                            record: oplog
                                .upload_payload(&StreamItemsRecord {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    stream_id: stream,
                                    producer_fingerprint: fingerprint,
                                    first_sequence,
                                    nested_stream_ids: vec![],
                                    newly_registered_stream_ids: vec![],
                                    payload: StreamItemsPayload::PackedU8(bytes),
                                    offsets,
                                })
                                .await
                                .unwrap(),
                        }
                    }
                    1025 => OplogEntry::StreamSession {
                        timestamp,
                        entity_parent_start_index: None,
                        record: oplog
                            .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                            .await
                            .unwrap(),
                    },
                    _ => OplogEntry::NoOp {
                        timestamp,
                        entity_parent_start_index: None,
                    },
                };
                oplog.add(entry).await;
            }
            let rebuilt = DurableStreamStore::read_complete_index(
                &oplog,
                identity.environment_id,
                &target_session.callee,
                target_session.callee_fingerprint,
                oplog.current_oplog_index().await,
            )
            .await;
            if sub_index == 3 {
                assert!(matches!(rebuilt, Err(StreamStoreError::CorruptHistory(_))));
                continue;
            }
            let rebuilt = rebuilt.unwrap();
            assert!(
                !rebuilt
                    .registrations
                    .contains_key(&registration.handle.stream_id)
            );
            assert_eq!(rebuilt.streams[&next.stream_id].next_sequence, 4);
            assert_eq!(
                rebuilt.batch_positions[&(next.stream_id, OplogIndex::from_u64(3))],
                (0, 2)
            );
            assert_eq!(
                rebuilt.batch_positions[&(next.stream_id, OplogIndex::from_u64(1026))],
                (2, 2)
            );
            assert_eq!(
                rebuilt.streams[&next.stream_id].last_item_offset,
                Some(StreamOffset::new(OplogIndex::from_u64(1026), 1))
            );
        }
    }

    fn super_registration(session: &StreamSessionKey, path: u32) -> ProducerRegistrationRequest {
        ProducerRegistrationRequest {
            coordinate: StreamRegistrationCoordinate::Root {
                invocation_id: session.clone(),
                root_kind: StreamRootKind::MethodResult,
                recursive_value_path: vec![
                    golem_common::base_model::durable_stream::StreamValuePathStep::TupleElement(
                        path,
                    ),
                ],
            },
            source_invocation: session.clone(),
            entity_parent_start_index: None,
            component_revision: golem_common::base_model::component::ComponentRevision::INITIAL,
            element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([7; 32]),
            source_kind: StreamSourceKind::InvocationOutput,
            session_mapping: None,
        }
    }
}
