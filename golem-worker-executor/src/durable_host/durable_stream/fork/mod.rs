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
            && matches!(oplog.read(cut_index).await?, OplogEntry::Revert { .. })
            && let OplogEntry::StreamSession { record, .. } = oplog.read(cut_index.next()).await?
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
            let entries = oplog.read_exact(covered.next(), count).await?;
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
mod tests;
