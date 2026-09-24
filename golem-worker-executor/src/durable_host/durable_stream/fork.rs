// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::{DurableStreamStore, ProducerStreamIndex, StreamStoreError};
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::worker_fork::lineage::StreamForkLineage;
use golem_common::base_model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle, LocalStreamId, StreamForkCutRecord,
    StreamId, StreamOffset, StreamRegistrationCoordinate, StreamSessionKey, StreamSessionRecord,
};
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::OplogRegion;
use golem_common::model::{AgentFingerprint, OplogIndex, OwnedAgentId};
use std::collections::HashSet;

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
        let mut index = Self::read_complete_index(
            oplog,
            source.0.environment_id,
            &source.0.agent_id,
            source.1,
            cut_index,
        )
        .await?;
        let selected_stream_id = selected
            .map(|(id, _)| {
                index
                    .registrations
                    .values()
                    .find(|registration| registration.handle.stream_id == id)
                    .map(|registration| LocalStreamId(registration.registration_oplog_index))
                    .ok_or(StreamStoreError::UnknownStream(id))
            })
            .transpose()?;
        let record = StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: request_hash.to_vec(),
            creation_fingerprint: target.1,
            export: None,
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
        };
        let retained_items = index.resolve_fork_prefix(&record)?;
        index.apply_fork_cut(&record, retained_items)?;
        StreamForkLineage::validate_fork_cut(oplog, record.clone())
            .await
            .map_err(StreamStoreError::CorruptHistory)?;
        Ok(record)
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
                let selected = self.runtime_stream_id(selected)?;
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
        let selected = record
            .selected_stream_id
            .map(|selected| self.runtime_stream_id(selected))
            .transpose()?;
        if selected.is_some_and(|selected| {
            self.finished_sessions
                .contains(&self.stream_sessions[&selected])
        }) {
            return Err(StreamStoreError::CorruptHistory(
                "selected stream cut retains its owning session completion".into(),
            ));
        }
        let selected_prefix_items = match (selected, selected_prefix_items) {
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
                                || (selected == Some(parent_stream_id)
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
        let old_streams: HashSet<_> = self.registrations.keys().copied().collect();
        self.registrations.retain(|id, _| retained.contains(id));
        self.entity_parent_start_indices
            .retain(|id, _| retained.contains(id));
        self.streams.retain(|id, _| retained.contains(id));
        self.stream_sessions.retain(|id, _| retained.contains(id));
        self.stream_roles.retain(|id, _| retained.contains(id));
        self.coordinates.retain(|_, id| retained.contains(id));
        self.local_stream_ids.retain(|_, id| retained.contains(id));

        if let Some(selected) = selected
            && let Some(state) = self.streams.get_mut(&selected)
        {
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
        // A retained prefix begins detached and with no inherited live producer or deletion state.
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
                if !retained.contains(stream) {
                    return None;
                }
                let mut value = *value;
                if selected == Some(*stream) {
                    let retained = selected_prefix_items.expect("selected stream has a prefix");
                    value.1 = value.1.min(retained.saturating_sub(value.0));
                    if value.1 == 0 {
                        return None;
                    }
                }
                Some(((*stream, *index), value))
            })
            .collect();

        Ok(ForkCutChanges {
            changed_streams: self.registrations.keys().copied().collect(),
            retired_streams: old_streams.difference(&retained).copied().collect(),
            changed_sessions: HashSet::new(),
            retired_sessions: HashSet::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::registration::{
        registered_stream, registration_record,
    };
    use crate::durable_host::durable_stream::{
        CommittedProducerStreamEventPayload, IndexedExternalProducer, ProducerRegistrationRequest,
        RegisteredStream,
        tests::{attachment_key, identity},
    };
    use crate::services::worker_fork::lineage::StreamForkLineage;
    use golem_common::base_model::durable_stream::{
        AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, LocalStreamId, LocalStreamReaderId,
        SessionStreamRole, StreamEndRecord, StreamEndResult, StreamItemsPayload, StreamItemsRecord,
        StreamRecordReference, StreamRegistrationInvocation, StreamRootKind,
        StreamSessionFinishedRecord, StreamSessionMapping, StreamSourceKind, StreamTerminalAuthor,
    };
    use golem_common::base_model::durable_stream::{
        ExternalProducerId, StreamAttachmentActivatedRecord, StreamAttachmentPreparedRecord,
        StreamBindingRecord, StreamCascadeDependentResult, StreamConsumerItemValueRecord,
        StreamConsumerTerminal, StreamConsumerTerminalRecord, StreamOffset,
        StreamSessionMappingUpdateRecord, StreamSessionRecord, StreamSourceUnavailableRecord,
    };
    use golem_common::base_model::{AgentFingerprint, AgentId, IdempotencyKey, OplogIndex};
    use golem_common::model::OwnedAgentId;
    use test_r::test;
    use uuid::Uuid;

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
        let root_local_id = LocalStreamId(oplog.current_oplog_index().await);
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
            .await
            .unwrap();
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
        let removed_local_id = LocalStreamId(oplog.current_oplog_index().await);
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
        let marker =
            DurableStreamOplogRecord::Session(None, Box::new(StreamSessionRecord::ForkCut(cut)))
                .into_inline_entry();
        let (_, marker_index) = oplog
            .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
            .await
            .unwrap();
        let rebuilt = DurableStreamStore::load(
            oplog.clone(),
            owner.environment_id,
            owner.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let continuation = rebuilt
            .materialize_binding(&StreamBindingRecord {
                transport_stream_id: 0,
                source: StreamRecordReference::Local(root_local_id),
                role: SessionStreamRole::Output,
            })
            .await
            .unwrap()
            .handle;
        assert_eq!(continuation.producer_generation, marker_index);
        let head = rebuilt.stream_head(&continuation).await.unwrap();
        assert_eq!(head.offset, retained.last().copied());
        assert!(!head.closed);
        assert!(!head.cancelled);
        assert!(rebuilt.stream_head(&root).await.is_err());
        assert!(
            rebuilt
                .materialize_binding(&StreamBindingRecord {
                    transport_stream_id: 1,
                    source: StreamRecordReference::Local(removed_local_id),
                    role: SessionStreamRole::Output,
                })
                .await
                .is_err()
        );
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

        let source_identity = identity();
        let handle = registered_stream(
            OplogIndex::from_u64(50),
            source_identity.environment_id,
            source_identity.agent_id.clone(),
            source_identity.fingerprint,
            super_registration(&source_identity.invocation, 0),
        )
        .handle;
        let key = attachment_key(&source_identity, handle.stream_id);
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
        consumer
            .append_session_record(
                None,
                StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: StreamRegistrationInvocation::Local(
                        key.session_key.idempotency_key.clone(),
                    ),
                    mapping: StreamBindingRecord {
                        transport_stream_id: 0,
                        source: StreamRecordReference::Foreign(handle),
                        role: SessionStreamRole::Input,
                    },
                }),
            )
            .await
            .unwrap();
        let reader_id = LocalStreamReaderId {
            introducing_oplog_index: oplog.current_oplog_index().await,
            binding_slot: 0,
        };
        let record = |ordinal, value| {
            StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                format_version: 1,
                session_key: StreamRegistrationInvocation::Local(
                    key.session_key.idempotency_key.clone(),
                ),
                reader_id,
                source_offset: StreamOffset::new(OplogIndex::from_u64(50), ordinal as u32),
                consumer_read_ordinal: ordinal,
                value,
                packed_u8: true,
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
            .await
            .unwrap();
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
                .commit_source_unavailable_overlay(None, key.clone(), reader_id, next, 2)
                .await
                .unwrap()
        );
        assert!(
            rebuilt
                .commit_source_unavailable_overlay(None, key, reader_id, next, 2)
                .await
                .unwrap()
        );
    }

    #[test]
    async fn fork_marker_preparation_discards_nested_suffix_and_prevalidates() {
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
        let root_local_id = LocalStreamId(oplog.current_oplog_index().await);
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
            assert_eq!(cut.creation_fingerprint, target_fingerprint);
            assert_eq!(cut.selected_stream_id, Some(root_local_id));
            let copied = Arc::new(TestOplog::default());
            for (_, entry) in history
                .read_exact(OplogIndex::INITIAL, horizon.as_u64())
                .await
            {
                copied.add(entry).await.unwrap();
            }
            copied
                .add(OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
                })
                .await
                .unwrap();
            let rebuilt = DurableStreamStore::read_complete_index(
                copied.as_ref(),
                target.environment_id,
                &target.agent_id,
                target_fingerprint,
                copied.current_oplog_index().await,
            )
            .await
            .unwrap();
            let continued_root = RegisteredStream::resolve(
                rebuilt
                    .registrations
                    .values()
                    .next()
                    .unwrap()
                    .record
                    .clone(),
                root_local_id.0,
                target.environment_id,
                &target.agent_id,
                target_fingerprint,
            )
            .unwrap()
            .issue(copied.current_oplog_index().await);
            assert_eq!(
                rebuilt.registrations.len(),
                1,
                "the nested stream introduced by the discarded item must not regain authority"
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
    async fn fork_marker_preparation_uses_exact_prefix_and_owner_relative_ids() {
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
        let root_local_id = LocalStreamId(oplog.current_oplog_index().await);
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
        assert_eq!(cut.creation_fingerprint, fingerprint);
        let copied = Arc::new(TestOplog::default());
        for (_, entry) in oplog
            .read_exact(OplogIndex::INITIAL, cut_index.as_u64())
            .await
        {
            copied.add(entry).await.unwrap();
        }
        copied
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            })
            .await
            .unwrap();
        let forked = DurableStreamStore::load(
            copied.clone(),
            target.environment_id,
            target.agent_id.clone(),
            fingerprint,
            None,
        )
        .await
        .unwrap();
        let continuation = forked
            .materialize_binding(&StreamBindingRecord {
                transport_stream_id: 0,
                source: StreamRecordReference::Local(root_local_id),
                role: SessionStreamRole::Output,
            })
            .await
            .unwrap()
            .handle;
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

        // Cutting before the parent's own marker still resolves the retained local registration.
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
        assert_eq!(before_marker.creation_fingerprint, next_fingerprint);
        assert_eq!(before_marker.selected_stream_id, Some(root_local_id));
        assert_eq!(before_marker.retained_through, first.first().copied());
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
        assert_eq!(after_marker.creation_fingerprint, next_fingerprint);

        // Reverting before the original fork marker retains the copied registration and items.
        use crate::services::oplog::DurableStreamOplogRecord;
        use golem_common::base_model::durable_stream::{
            ResumeAttemptDescriptor, StreamResumeOperation, StreamSessionResumeAttemptRecord,
        };
        let session = identity.invocation.clone();
        let mut previous_handle = continuation;
        for (accepted_epoch, expected_epoch) in [(37, 38), (40, 41)] {
            copied
                .add(
                    DurableStreamOplogRecord::Session(
                        None,
                        Box::new(StreamSessionRecord::ResumeAttempt(
                            StreamSessionResumeAttemptRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: session.idempotency_key.clone(),
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
                .await
                .unwrap();
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
            assert_eq!(reverted.creation_fingerprint, fingerprint);
            assert_eq!(reverted.epoch_floor, expected_epoch);
            let region = reverted.revert.clone().unwrap();
            let marker = DurableStreamOplogRecord::Session(
                None,
                Box::new(StreamSessionRecord::ForkCut(reverted)),
            )
            .into_inline_entry();
            let (_, marker_index) = copied
                .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
                .await
                .unwrap();
            let rebuilt = DurableStreamStore::load(
                copied.clone(),
                target.environment_id,
                target.agent_id.clone(),
                fingerprint,
                None,
            )
            .await
            .unwrap();
            let handle = rebuilt
                .materialize_binding(&StreamBindingRecord {
                    transport_stream_id: 0,
                    source: StreamRecordReference::Local(root_local_id),
                    role: SessionStreamRole::Output,
                })
                .await
                .unwrap()
                .handle;
            assert_eq!(handle.stream_id, previous_handle.stream_id);
            assert_eq!(handle.producer_generation, marker_index);
            assert!(rebuilt.stream_head(&previous_handle).await.is_err());
            previous_handle = handle.clone();
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
            .await
            .unwrap();
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
        assert_eq!(historical.creation_fingerprint, next_fingerprint);
        assert_eq!(historical.selected_stream_id, None);

        use golem_common::base_model::durable_stream::StreamSessionAttachedRecord;
        copied
            .add(
                DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session.idempotency_key.clone(),
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
            .await
            .unwrap();
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
        let first_cut = oplog.add(OplogEntry::no_op(None)).await.unwrap();
        let entry =
            |record| DurableStreamOplogRecord::Session(None, Box::new(record)).into_inline_entry();
        let attached = |epoch| {
            StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.idempotency_key.clone(),
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
            .await
            .unwrap();
        oplog.add(entry(attached(5))).await.unwrap();
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
        let region = cut.revert.clone().unwrap();
        let marker = entry(StreamSessionRecord::ForkCut(cut));
        oplog
            .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
            .await
            .unwrap();
        let second_cut = oplog
            .add(entry(StreamSessionRecord::Prepared(prepared(
                &identity.invocation,
            ))))
            .await
            .unwrap();
        oplog.add(entry(attached(1))).await.unwrap();
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
        assert_eq!(cut.epoch_floor, 7);
    }

    #[test]
    async fn fork_cut_validation_rejects_a_malformed_request_hash() {
        use crate::durable_host::durable_stream::tests::TestOplog;

        let identity = identity();
        let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let oplog = TestOplog::default();
        let cut_index = oplog.add(OplogEntry::no_op(None)).await.unwrap();
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
        oplog.add(OplogEntry::no_op(None)).await.unwrap();
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
        for round in 0..3 {
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
                        descriptor.stream_mappings = bindings;
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
            assert_eq!(attached.epoch, 1 + round);
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
            let region = cut.revert.clone().unwrap();
            let marker = crate::services::oplog::DurableStreamOplogRecord::Session(
                None,
                Box::new(StreamSessionRecord::ForkCut(cut)),
            )
            .into_inline_entry();
            oplog
                .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
                .await
                .unwrap();
        }
    }

    #[test]
    async fn fork_after_bare_revert_uses_the_fixed_horizon() {
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
            oplog.add(OplogEntry::no_op(None)).await.unwrap();
        }
        let horizon = oplog
            .add(OplogEntry::revert(OplogRegion::from_range(2..=3)))
            .await
            .unwrap();
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
        assert_eq!(cut.creation_fingerprint, fingerprint);
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
            .await
            .unwrap();
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
        assert_eq!(cut.creation_fingerprint, identity.fingerprint);
    }

    fn continuation_session(source: &StreamSessionKey, _suffix: &str) -> StreamSessionKey {
        source.clone()
    }

    fn register(
        index: &mut ProducerStreamIndex,
        oplog_index: u64,
        request: ProducerRegistrationRequest,
    ) -> DurableStreamHandle {
        let identity = identity();
        let oplog_index = OplogIndex::from_u64(oplog_index);
        let local_parent = match &request.coordinate {
            StreamRegistrationCoordinate::Nested {
                parent_stream_id, ..
            } => Some((
                *parent_stream_id,
                index.local_stream_id(*parent_stream_id).unwrap(),
            )),
            _ => None,
        };
        let record = registration_record(
            oplog_index,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            request,
            local_parent,
        );
        let handle = RegisteredStream::resolve(
            record.clone(),
            oplog_index,
            identity.environment_id,
            &identity.agent_id,
            identity.fingerprint,
        )
        .unwrap()
        .handle;
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

    fn continuation(source: &DurableStreamHandle, _suffix: &str) -> DurableStreamHandle {
        source.clone()
    }

    fn validate_marker(index: &ProducerStreamIndex, record: &StreamForkCutRecord) {
        let _ = index;
        crate::durable_host::durable_stream::session_state::SessionControlMetadata::default()
            .for_fork(record)
            .unwrap();
    }

    fn marker(
        _streams: Vec<(DurableStreamHandle, DurableStreamHandle)>,
        _sessions: Vec<(StreamSessionKey, StreamSessionKey)>,
        selected: Option<LocalStreamId>,
        retained_through: Option<golem_common::base_model::durable_stream::StreamOffset>,
    ) -> StreamForkCutRecord {
        StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            creation_fingerprint: AgentFingerprint(Uuid::from_u128(300)),
            export: None,
            cut_index: OplogIndex::from_u64(20),
            revert: None,
            epoch_floor: 1,
            selected_stream_id: selected,
            retained_through,
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
                source_invocation: StreamRegistrationInvocation::Local(
                    source_session.idempotency_key.clone(),
                ),
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
                    stream_id: LocalStreamId(OplogIndex::from_u64(2)),
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
                OplogIndex::NONE,
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
                source_invocation: StreamRegistrationInvocation::Local(
                    source_session.idempotency_key.clone(),
                ),
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
                    stream_id: LocalStreamId(OplogIndex::from_u64(2)),
                    first_sequence: 3,
                    nested_stream_ids: vec![StreamRecordReference::Local(LocalStreamId(
                        OplogIndex::from_u64(5),
                    ))],
                    newly_registered_stream_ids: vec![LocalStreamId(OplogIndex::from_u64(5))],
                    payload: StreamItemsPayload::Values(vec![vec![3]]),
                    offsets: vec![golem_common::base_model::durable_stream::StreamOffset::new(
                        OplogIndex::from_u64(6),
                        0,
                    )],
                },
                OplogIndex::NONE,
            )
            .unwrap();
        for (oplog_index, stream_id, sequence) in [
            (7, LocalStreamId(OplogIndex::from_u64(2)), 4),
            (8, LocalStreamId(OplogIndex::from_u64(5)), 0),
        ] {
            let event = index
                .apply_end(
                    OplogIndex::from_u64(oplog_index),
                    None,
                    StreamEndRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
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
            let runtime_stream_id = index.runtime_stream_id(stream_id).unwrap();
            index
                .streams
                .get_mut(&runtime_stream_id)
                .unwrap()
                .terminal_event = Some(event);
        }
        let mut completed = index.clone();
        completed
            .apply_finished(
                &StreamSessionFinishedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: StreamRegistrationInvocation::Local(
                        source_session.idempotency_key.clone(),
                    ),
                    result: Ok(()),
                },
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
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
            Some(index.local_stream_id(root.stream_id).unwrap()),
            Some(retained),
        );
        validate_marker(&index, &fork);
        assert!(completed.apply_fork_cut(&fork, Some(2)).is_err());
        index.apply_fork_cut(&fork, Some(2)).unwrap();
        assert!(index.registrations.contains_key(&root.stream_id));
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
                    stream_id: LocalStreamId(OplogIndex::from_u64(2)),
                    first_sequence: 2,
                    nested_stream_ids: Vec::new(),
                    newly_registered_stream_ids: Vec::new(),
                    payload: StreamItemsPayload::Values(vec![vec![4]]),
                    offsets: vec![golem_common::base_model::durable_stream::StreamOffset::new(
                        OplogIndex::from_u64(22),
                        0,
                    )],
                },
                OplogIndex::NONE,
            )
            .unwrap();
        assert_eq!(index.streams[&root_next.stream_id].next_sequence, 3);
    }

    #[test]
    fn fork_cut_preserves_unrelated_completed_stream_and_clears_live_state() {
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
            let local_stream_id = index.local_stream_id(stream_id).unwrap();
            let event = index
                .apply_end(
                    OplogIndex::from_u64(oplog_index),
                    None,
                    StreamEndRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id: local_stream_id,
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
            Some(index.local_stream_id(first.stream_id).unwrap()),
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
        assert!(index.registrations.contains_key(&first.stream_id));
        assert!(index.invocation_results.contains_key(&output_only_source));
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
        request.source_invocation = StreamRegistrationInvocation::Remote(remote.clone());
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
        for (binding_slot, session) in [&local, &remote].into_iter().enumerate() {
            let attribution = (session == &local).then_some(OplogIndex::from_u64(1));
            let session_reference = if session == &local {
                StreamRegistrationInvocation::Local(session.idempotency_key.clone())
            } else {
                StreamRegistrationInvocation::Remote(session.clone())
            };
            index
                .apply_session_attribution(session, attribution)
                .unwrap();
            index
                .apply_session_references(
                    attribution,
                    &StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_reference.clone(),
                        mapping: StreamBindingRecord {
                            transport_stream_id: 0,
                            source: StreamRecordReference::Foreign(foreign.clone()),
                            role: SessionStreamRole::Output,
                        },
                    }),
                    identity.environment_id,
                    &identity.agent_id,
                    identity.fingerprint,
                )
                .unwrap();
            index
                .invocation_results
                .insert(session.clone(), OplogIndex::from_u64(10));
            index
                .apply_consumer_journal_record(
                    &StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_reference,
                        reader_id: LocalStreamReaderId {
                            introducing_oplog_index: OplogIndex::from_u64(1),
                            binding_slot: binding_slot as u32,
                        },
                        source_offset: StreamOffset::new(OplogIndex::from_u64(8), 0),
                        consumer_read_ordinal: 0,
                        value: vec![4, 5, 6],
                        packed_u8: true,
                        recursive_mappings: vec![],
                    }),
                    identity.environment_id,
                    &identity.agent_id,
                    identity.fingerprint,
                )
                .unwrap();
        }
        index
            .apply_consumer_journal_record(
                &StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: StreamRegistrationInvocation::Local(local.idempotency_key.clone()),
                    reader_id: LocalStreamReaderId {
                        introducing_oplog_index: OplogIndex::from_u64(1),
                        binding_slot: 0,
                    },
                    source_offset: StreamOffset::new(OplogIndex::from_u64(9), 0),
                    consumer_read_ordinal: 3,
                    terminal: StreamConsumerTerminal::End(StreamEndResult::Ok),
                }),
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
            .unwrap();
        let unavailable = StreamOffset::new(OplogIndex::from_u64(10), 0);
        index
            .apply_consumer_journal_record(
                &StreamSessionRecord::SourceUnavailable(StreamSourceUnavailableRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: StreamRegistrationInvocation::Remote(remote.clone()),
                    reader_id: LocalStreamReaderId {
                        introducing_oplog_index: OplogIndex::from_u64(1),
                        binding_slot: 1,
                    },
                    source_offset: unavailable,
                    consumer_read_ordinal: 3,
                }),
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
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
        for session in [&target, &remote] {
            assert!(
                index.session_stream_mappings[session]
                    .iter()
                    .any(|(source, _)| source == &StreamRecordReference::Foreign(foreign.clone()))
            );
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
        let local_reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(1),
            binding_slot: 0,
        };
        let remote_reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(1),
            binding_slot: 1,
        };
        let journal = &index.consumer_journals[&(target, local_reader)];
        assert_eq!(journal.next_read_ordinal, 4);
        assert!(journal.terminal);
        let journal = &index.consumer_journals[&(remote, remote_reader)];
        assert_eq!(journal.next_read_ordinal, 3);
        assert_eq!(journal.source_unavailable, Some(unavailable));
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
        let handle = registered_stream(
            OplogIndex::from_u64(2),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            super_registration(&identity.invocation, 0),
        )
        .handle;
        let mut remote_handle = handle.clone();
        remote_handle.producer = remote.callee.clone();
        remote_handle.expected_producer_fingerprint = remote.callee_fingerprint;
        let reader_id = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(1),
            binding_slot: 0,
        };
        for session in [&local, &remote] {
            let mut key = attachment_key(&identity, handle.stream_id);
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
                .apply_session_references(
                    None,
                    &StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: if session == &local {
                            StreamRegistrationInvocation::Local(session.idempotency_key.clone())
                        } else {
                            StreamRegistrationInvocation::Remote(session.clone())
                        },
                        mapping: StreamBindingRecord {
                            transport_stream_id: 0,
                            source: StreamRecordReference::Foreign(remote_handle.clone()),
                            role: SessionStreamRole::Input,
                        },
                    }),
                    identity.environment_id,
                    &identity.agent_id,
                    identity.fingerprint,
                )
                .unwrap();
            index
                .apply_consumer_journal_record(
                    &StreamSessionRecord::SourceUnavailable(StreamSourceUnavailableRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: if session == &local {
                            StreamRegistrationInvocation::Local(session.idempotency_key.clone())
                        } else {
                            StreamRegistrationInvocation::Remote(session.clone())
                        },
                        reader_id,
                        source_offset: offset,
                        consumer_read_ordinal: 0,
                    }),
                    identity.environment_id,
                    &identity.agent_id,
                    identity.fingerprint,
                )
                .unwrap();
            let fork = marker(vec![], vec![(local.clone(), target.clone())], None, None);
            validate_marker(&index, &fork);
            index.apply_fork_cut(&fork, None).unwrap();
            assert!(index.attachments.is_empty());
            assert!(index.cascade_outbox.is_empty());
            let expected = key.clone();
            let producer = DurableStreamStore::from_index(
                Arc::new(TestOplog::default()),
                identity.environment_id,
                identity.agent_id.clone(),
                identity.fingerprint,
                8,
                Arc::new(|_| Box::pin(async {})),
                index,
            )
            .unwrap();
            assert_eq!(
                producer
                    .consumer_source_unavailable(&expected, reader_id)
                    .await
                    .unwrap(),
                Some(offset)
            );
        }
    }

    #[test]
    async fn fork_cut_full_reconstruction_qualifies_copied_records_and_retains_later_writes() {
        use crate::durable_host::durable_stream::{DurableStreamStore, tests::TestOplog};
        use crate::services::oplog::{Oplog, OplogOps};
        use crate::services::worker_fork::lineage::tests::prepared;
        use golem_common::model::{Timestamp, oplog::OplogEntry};

        let identity = identity();
        let target_agent = AgentId {
            component_id: identity.agent_id.component_id,
            agent_id: "fork-reconstruction".into(),
        };
        let target_fingerprint = AgentFingerprint(Uuid::from_u128(300));
        let mut target_session = identity.invocation.clone();
        target_session.callee = target_agent.clone();
        target_session.callee_fingerprint = target_fingerprint;
        let registration = registered_stream(
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
                Some(LocalStreamId(OplogIndex::from_u64(2))),
                Some(StreamOffset::new(OplogIndex::from_u64(3), sub_index)),
            );
            cut.cut_index = OplogIndex::from_u64(1024);
            let next = DurableStreamHandle {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: StreamId::derive(
                    identity.environment_id,
                    &target_agent,
                    target_fingerprint,
                    OplogIndex::from_u64(2),
                )
                .unwrap(),
                producer_environment_id: identity.environment_id,
                producer: target_agent.clone(),
                expected_producer_fingerprint: target_fingerprint,
                producer_generation: OplogIndex::NONE,
                source_invocation: target_session.clone(),
                component_revision: registration.handle.component_revision,
                element_schema_fingerprint: registration.handle.element_schema_fingerprint,
            };
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
                        record: oplog.upload_payload(&registration.record).await.unwrap(),
                    },
                    3 | 1026 => {
                        let first_sequence = if position == 3 { 0 } else { 2 };
                        let bytes = if position == 3 {
                            vec![5, 6, 7]
                        } else {
                            vec![8, 9]
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
                                    stream_id: LocalStreamId(OplogIndex::from_u64(2)),
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
                oplog.add(entry).await.unwrap();
            }
            let rebuilt = DurableStreamStore::read_complete_index(
                &oplog,
                identity.environment_id,
                &target_agent,
                target_fingerprint,
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
            assert!(rebuilt.registrations.contains_key(&next.stream_id));
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
            source_invocation: StreamRegistrationInvocation::Local(session.idempotency_key.clone()),
            entity_parent_start_index: None,
            component_revision: golem_common::base_model::component::ComponentRevision::INITIAL,
            element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([7; 32]),
            source_kind: StreamSourceKind::InvocationOutput,
            session_mapping: None,
        }
    }
}
