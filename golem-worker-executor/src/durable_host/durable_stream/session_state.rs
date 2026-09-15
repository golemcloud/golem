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

//! Pure persisted projection of Stream Session journal state.
//!
//! The durable stream store owns this projection because it indexes durable records; the
//! `StreamSession` runtime consumes it without owning its persistence representation.

use super::ConsumerAttachmentStatus;
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DurableStreamHandle, SessionStreamRole, StreamAttachmentKey,
    StreamConsumerCancelIntentRecord, StreamSessionKey, StreamSessionMappingRecord,
    StreamSessionRecord,
};
use golem_common::model::oplog::OplogIndex;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default, desert_rust::BinaryCodec)]
/// Projection of one session's journal records through `covered_through`.
/// Local refreshes include the append buffer; persisted projections cover committed history.
pub struct SessionControlMetadata {
    pub(crate) covered_through: OplogIndex,
    pub(crate) recovery_slot: Option<u64>,
    pub(crate) prepared: Option<OplogIndex>,
    pub(crate) initial_attached: Option<OplogIndex>,
    pub(crate) malformed_record: bool,
    pub(crate) explicit_mappings: HashSet<(u64, DurableStreamHandle, SessionStreamRole)>,
    pub(crate) persisted_mappings: HashSet<(u64, DurableStreamHandle, SessionStreamRole)>,
    pub(crate) recoverable_mappings: Vec<(OplogIndex, StreamSessionMappingRecord)>,
    acceptance_mappings: Vec<StreamSessionMappingRecord>,
    pub(crate) caller_attempt: Option<AttemptId>,
    pub(crate) caller_attempt_conflict: bool,
    pub(crate) invocation_result: Option<OplogIndex>,
    pub(crate) finished: Option<OplogIndex>,
    pub(crate) root_outputs: Vec<u64>,
    pub(crate) topology_epoch: Option<u64>,
    pub(crate) topologies: HashMap<
        (
            AttachmentId,
            golem_common::model::StreamId,
            u64,
            SessionStreamRole,
        ),
        SessionTopologyMetadata,
    >,
    pub(crate) visible_mappings: HashSet<(u64, DurableStreamHandle, SessionStreamRole)>,
    pub(crate) topology_error: Option<String>,
    finalized_attachments:
        HashMap<(AttachmentId, golem_common::model::StreamId), StreamAttachmentKey>,
    pub(crate) closed_consumer_streams: HashSet<golem_common::model::StreamId>,
    pub(crate) cancel_intents:
        HashMap<golem_common::model::StreamId, StreamConsumerCancelIntentRecord>,
    pub(crate) applied_cancel_intents: HashSet<StreamConsumerCancelIntentRecord>,
    pub(crate) tombstoned_slots: HashMap<String, SessionStreamRole>,
    pub(crate) cancellation_requested: bool,
    pub(crate) consumer_record_counts: HashMap<golem_common::model::StreamId, u64>,
    pub(crate) consumer_deleting:
        Option<golem_common::model::durable_stream::StreamConsumerDeletingRecord>,
}

#[derive(Clone, desert_rust::BinaryCodec)]
/// Durable attachment topology projected from one stream session journal.
pub(crate) struct SessionTopologyMetadata {
    pub(crate) attachment: StreamAttachmentKey,
    pub(crate) mapping: StreamSessionMappingRecord,
    active: bool,
    prepared_index: Option<OplogIndex>,
    activated_index: Option<OplogIndex>,
    repeated_activation_index: Option<OplogIndex>,
}

impl SessionTopologyMetadata {
    /// Returns whether the topology has a matching durable activation record.
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }
}

impl SessionControlMetadata {
    /// Returns mappings that were durably established before session acceptance.
    pub(crate) fn acceptance_mappings(&self) -> Result<Vec<StreamSessionMappingRecord>, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        Ok(self.acceptance_mappings.clone())
    }

    /// Checks that cancellation and its exact topology mapping are both committed.
    pub(crate) fn has_committed_cancellation(
        &self,
        key: &StreamAttachmentKey,
        mapping: &StreamSessionMappingRecord,
        intent: &StreamConsumerCancelIntentRecord,
    ) -> Result<bool, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        let consumer_matches = key.consumer_invocation == key.session_key
            || self.topologies.values().any(|topology| {
                let mut current_key = key.clone();
                current_key.epoch = topology.attachment.epoch;
                topology.attachment == current_key && topology.mapping == *mapping
            });
        Ok(consumer_matches
            && intent.session_key == key.session_key
            && intent.stream_id == key.stream_id
            && intent.epoch == key.epoch
            && mapping.handle.stream_id == key.stream_id
            && self.cancel_intents.get(&key.stream_id) == Some(intent)
            && self.persisted_mappings.contains(&(
                mapping.transport_stream_id,
                mapping.handle.clone(),
                mapping.role,
            )))
    }

    /// Returns whether durable topology or cancellation work remains incomplete.
    pub(crate) fn needs_recovery(
        &self,
        owner: &golem_common::model::OwnedAgentId,
        key: &StreamSessionKey,
    ) -> bool {
        self.needs_topology_recovery(owner, key)
            || self
                .cancel_intents
                .values()
                .any(|intent| !self.applied_cancel_intents.contains(intent))
    }

    /// Returns whether any committed cancellation intent lacks its applied marker.
    pub(crate) fn has_cancellation_intents(&self) -> bool {
        self.cancel_intents
            .values()
            .any(|intent| !self.applied_cancel_intents.contains(intent))
    }

    /// Returns whether attachments must be reconstructed or finalized from the journal.
    pub(crate) fn needs_topology_recovery(
        &self,
        owner: &golem_common::model::OwnedAgentId,
        key: &StreamSessionKey,
    ) -> bool {
        self.malformed_record
            || self.topology_error.is_some()
            || (self.prepared.is_some() && self.finished.is_none())
            || self.topologies.values().any(|topology| {
                let local = key.callee_environment_id == owner.environment_id
                    && key.callee == owner.agent_id
                    && key.callee_fingerprint == topology.attachment.expected_consumer_fingerprint;
                !(local && self.finished.is_some())
                    && !self
                        .cancel_intents
                        .contains_key(&topology.attachment.stream_id)
                    && !self
                        .closed_consumer_streams
                        .contains(&topology.attachment.stream_id)
                    && self.finalized_attachments.get(&(
                        topology.attachment.attachment_id,
                        topology.attachment.stream_id,
                    )) != Some(&topology.attachment)
            })
    }

    /// Returns unresolved attachment mappings that recovery must process.
    pub(crate) fn recovery_topologies(
        &self,
        owner: &golem_common::model::OwnedAgentId,
        key: &StreamSessionKey,
    ) -> Result<Vec<(StreamAttachmentKey, StreamSessionMappingRecord)>, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        Ok(self
            .topologies
            .values()
            .filter(|topology| {
                let local = key.callee_environment_id == owner.environment_id
                    && key.callee == owner.agent_id
                    && key.callee_fingerprint == topology.attachment.expected_consumer_fingerprint;
                !(local && self.finished.is_some())
                    && !self
                        .cancel_intents
                        .contains_key(&topology.attachment.stream_id)
                    && !self
                        .closed_consumer_streams
                        .contains(&topology.attachment.stream_id)
                    && (!local
                        || self
                            .topology_epoch
                            .is_none_or(|epoch| epoch == topology.attachment.epoch))
                    && self.finalized_attachments.get(&(
                        topology.attachment.attachment_id,
                        topology.attachment.stream_id,
                    )) != Some(&topology.attachment)
            })
            .map(|topology| (topology.attachment.clone(), topology.mapping.clone()))
            .collect())
    }

    /// Folds the exact attachment slot into its durable prepared or active phase.
    pub(crate) fn topology_status(
        &self,
        attachment: &StreamAttachmentKey,
        expected_mapping: Option<&StreamSessionMappingRecord>,
    ) -> Result<ConsumerAttachmentStatus, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        let mut events = Vec::new();
        for topology in self
            .topologies
            .values()
            .filter(|topology| same_attachment_slot(&topology.attachment, attachment))
        {
            if topology.attachment.epoch < attachment.epoch {
                continue;
            }
            if topology.attachment != *attachment {
                return Ok(attachment_mismatch_status(&topology.attachment, attachment));
            }
            if expected_mapping.is_some_and(|mapping| mapping != &topology.mapping) {
                continue;
            }
            events.extend(topology.prepared_index.map(|index| (index, false)));
            events.extend(topology.activated_index.map(|index| (index, true)));
            events.extend(
                topology
                    .repeated_activation_index
                    .map(|index| (index, true)),
            );
        }
        events.sort_unstable_by_key(|(index, _)| *index);
        let mut state = ConsumerAttachmentStatus::Missing;
        for (_, active) in events {
            if !active {
                if state == ConsumerAttachmentStatus::Missing {
                    state = ConsumerAttachmentStatus::Prepared;
                }
            } else if !(expected_mapping.is_none() && state == ConsumerAttachmentStatus::Active) {
                if state != ConsumerAttachmentStatus::Prepared {
                    return Err("durable topology activation has no matching preparation".into());
                }
                state = ConsumerAttachmentStatus::Active;
            }
        }
        Ok(state)
    }

    /// Applies one record in oplog order to reconstructed session state.
    pub(crate) fn apply(
        &mut self,
        index: OplogIndex,
        key: &StreamSessionKey,
        record: &StreamSessionRecord,
    ) {
        self.malformed_record |= !record.has_supported_format();
        if let StreamSessionRecord::ConsumerDeleting(record) = record {
            self.consumer_deleting = Some(record.clone());
        }
        if let StreamSessionRecord::ConsumerCancelIntent(record) = record
            && &record.session_key == key
        {
            self.cancel_intents
                .entry(record.stream_id)
                .or_insert_with(|| record.clone());
        }
        if let StreamSessionRecord::ConsumerCancelApplied(record) = record
            && &record.intent.session_key == key
            && self.cancel_intents.get(&record.intent.stream_id) == Some(&record.intent)
        {
            self.applied_cancel_intents.insert(record.intent.clone());
        }
        if let StreamSessionRecord::Tombstoned(record) = record
            && &record.session_key == key
        {
            self.tombstoned_slots
                .entry(record.slot.clone())
                .or_insert(record.role);
        }
        if let StreamSessionRecord::CancelRequested(record) = record
            && &record.session_key == key
        {
            self.cancellation_requested = true;
        }
        if let StreamSessionRecord::Prepared(record) = record
            && &record.attempt.session_key == key
        {
            if self.prepared.is_some() {
                self.topology_error.get_or_insert_with(|| {
                    "durable Stream Session contains multiple Prepared records".into()
                });
            } else {
                self.prepared = Some(index);
            }
        }
        if let StreamSessionRecord::AttachmentFinalized(record) = record
            && &record.key.session_key == key
        {
            let slot = (record.key.attachment_id, record.key.stream_id);
            if self
                .finalized_attachments
                .get(&slot)
                .is_none_or(|old| old.epoch <= record.key.epoch)
            {
                self.finalized_attachments.insert(slot, record.key.clone());
            }
        }
        let consumer_stream = match record {
            StreamSessionRecord::ConsumerItemValue(record) if &record.session_key == key => {
                Some(record.stream_id)
            }
            StreamSessionRecord::ConsumerTerminal(record) if &record.session_key == key => {
                Some(record.stream_id)
            }
            StreamSessionRecord::SourceUnavailable(record) if &record.key.session_key == key => {
                Some(record.key.stream_id)
            }
            _ => None,
        };
        if let Some(stream) = consumer_stream {
            *self.consumer_record_counts.entry(stream).or_default() += 1;
            if matches!(
                record,
                StreamSessionRecord::ConsumerTerminal(_)
                    | StreamSessionRecord::SourceUnavailable(_)
            ) {
                self.closed_consumer_streams.insert(stream);
            }
        }
        match record {
            StreamSessionRecord::Attached(record) if &record.session_key == key => {
                if self.initial_attached.is_some() {
                    self.topology_error.get_or_insert_with(|| {
                        "durable Stream Session contains multiple Attached records".into()
                    });
                } else {
                    self.initial_attached = Some(index);
                }
                self.topology_epoch = Some(record.epoch);
            }
            StreamSessionRecord::ResumeAttempt(record) if &record.attempt.session_key == key => {
                self.topology_epoch = Some(record.accepted_epoch);
            }
            _ => {}
        }
        let topology = match record {
            StreamSessionRecord::TopologyPrepared(record) if &record.session_key == key => {
                Some((&record.attachment, &record.mapping, false))
            }
            StreamSessionRecord::TopologyActivated(record) if &record.session_key == key => {
                Some((&record.attachment, &record.mapping, true))
            }
            _ => None,
        };
        if let Some((attachment, mapping, active)) = topology {
            let slot = (
                attachment.attachment_id,
                attachment.stream_id,
                mapping.transport_stream_id,
                mapping.role,
            );
            match self.topologies.get_mut(&slot) {
                Some(existing)
                    if &existing.attachment != attachment || &existing.mapping != mapping =>
                {
                    if attachment.epoch > existing.attachment.epoch {
                        if active {
                            self.topology_error.get_or_insert_with(|| {
                                "durable topology activation has no matching preparation".into()
                            });
                        }
                        *existing = SessionTopologyMetadata {
                            attachment: attachment.clone(),
                            mapping: mapping.clone(),
                            active,
                            prepared_index: (!active).then_some(index),
                            activated_index: active.then_some(index),
                            repeated_activation_index: None,
                        };
                    } else {
                        self.topology_error.get_or_insert_with(|| {
                            "conflicting durable topology preparation or activation".into()
                        });
                    }
                }
                Some(existing) => {
                    existing.active |= active;
                    if active {
                        if existing.activated_index.is_some() {
                            existing.repeated_activation_index.get_or_insert(index);
                        } else {
                            existing.activated_index = Some(index);
                        }
                    } else {
                        existing.prepared_index.get_or_insert(index);
                    }
                }
                None => {
                    if active {
                        self.topology_error.get_or_insert_with(|| {
                            "durable topology activation has no matching preparation".into()
                        });
                    }
                    self.topologies.insert(
                        slot,
                        SessionTopologyMetadata {
                            attachment: attachment.clone(),
                            mapping: mapping.clone(),
                            active,
                            prepared_index: (!active).then_some(index),
                            activated_index: active.then_some(index),
                            repeated_activation_index: None,
                        },
                    );
                }
            }
        }
        let mappings: &[StreamSessionMappingRecord] = match record {
            StreamSessionRecord::CallerAttempt(record) if &record.session_key == key => {
                if self
                    .caller_attempt
                    .is_some_and(|attempt| attempt != record.attempt_id)
                {
                    self.caller_attempt_conflict = true;
                }
                self.caller_attempt.get_or_insert(record.attempt_id);
                &[]
            }
            StreamSessionRecord::Mapping(record) if &record.session_key == key => {
                self.explicit_mappings.insert((
                    record.mapping.transport_stream_id,
                    record.mapping.handle.clone(),
                    record.mapping.role,
                ));
                std::slice::from_ref(&record.mapping)
            }
            StreamSessionRecord::Prepared(record) if &record.attempt.session_key == key => {
                &record.stream_mappings
            }
            StreamSessionRecord::TopologyPrepared(record) if &record.session_key == key => {
                std::slice::from_ref(&record.mapping)
            }
            StreamSessionRecord::TopologyActivated(record) if &record.session_key == key => {
                std::slice::from_ref(&record.mapping)
            }
            StreamSessionRecord::ConsumerItemValue(record) if &record.session_key == key => {
                &record.recursive_mappings
            }
            StreamSessionRecord::InvocationResult(record) if &record.session_key == key => {
                self.invocation_result.get_or_insert(index);
                for mapping in &record.stream_mappings {
                    if mapping.role == SessionStreamRole::Output
                        && !self.root_outputs.contains(&mapping.transport_stream_id)
                    {
                        self.root_outputs.push(mapping.transport_stream_id);
                    }
                }
                &record.stream_mappings
            }
            StreamSessionRecord::Finished(record) if &record.session_key == key => {
                self.finished.get_or_insert(index);
                &[]
            }
            _ => &[],
        };
        for mapping in mappings {
            if !self.acceptance_mappings.contains(mapping) {
                self.acceptance_mappings.push(mapping.clone());
            }
        }
        if matches!(
            record,
            StreamSessionRecord::Prepared(_)
                | StreamSessionRecord::Mapping(_)
                | StreamSessionRecord::InvocationResult(_)
        ) {
            self.visible_mappings.extend(mappings.iter().map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            }));
        }
        if matches!(
            record,
            StreamSessionRecord::Mapping(_) | StreamSessionRecord::InvocationResult(_)
        ) {
            for mapping in mappings {
                if !self
                    .recoverable_mappings
                    .iter()
                    .any(|(_, existing)| existing == mapping)
                {
                    self.recoverable_mappings.push((index, mapping.clone()));
                }
            }
        }
        self.persisted_mappings
            .extend(mappings.iter().map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            }));
        self.covered_through = index;
    }
}

fn same_attachment_slot(left: &StreamAttachmentKey, right: &StreamAttachmentKey) -> bool {
    left.attachment_id == right.attachment_id
        && left.stream_id == right.stream_id
        && left.consumer_environment_id == right.consumer_environment_id
        && left.consumer == right.consumer
}

fn attachment_mismatch_status(
    persisted: &StreamAttachmentKey,
    supplied: &StreamAttachmentKey,
) -> ConsumerAttachmentStatus {
    let mut supplied_at_persisted_epoch = supplied.clone();
    supplied_at_persisted_epoch.epoch = persisted.epoch;
    if persisted == &supplied_at_persisted_epoch {
        ConsumerAttachmentStatus::EpochMismatch
    } else {
        ConsumerAttachmentStatus::IncarnationMismatch
    }
}
