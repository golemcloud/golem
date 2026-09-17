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
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle, SessionStreamRole,
    StreamAttachmentKey, StreamCancelReason, StreamCancelRole, StreamConsumerCancelIntentRecord,
    StreamSessionCancelRequestedRecord, StreamSessionKey, StreamSessionMappingRecord,
    StreamSessionRecord,
};
use golem_common::model::StreamId;
use golem_common::model::oplog::OplogIndex;
use std::collections::{HashMap, HashSet};

/// An unresolved attachment and the exact mapping required to recover it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryTopology {
    /// Durable attachment identity to reconstruct.
    pub attachment_key: StreamAttachmentKey,
    /// Exact transport mapping associated with the attachment.
    pub mapping: StreamSessionMappingRecord,
}

/// Pure cancellation work selected from the folded session journal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancellationWork {
    /// Exact persisted mapping whose stream must be cancelled.
    pub mapping: StreamSessionMappingRecord,
    /// Pending durable cancellation intent.
    pub intent: StreamConsumerCancelIntentRecord,
}

/// Mapping additions and the horizon through which they were selected.
pub struct RecoveredMappings {
    /// Last examined oplog position, including records that added no mappings.
    pub covered_through: OplogIndex,
    /// Mappings established strictly after the caller's previous horizon.
    pub mappings: Vec<StreamSessionMappingRecord>,
}

/// Folded cancellation facts needed before registering invocation outputs.
pub struct ResultMaterializationState {
    /// Whether the invocation result has not yet been journaled.
    pub first_result: bool,
    /// Whether new output streams must inherit whole-session cancellation.
    pub cancel_session: bool,
    /// Output slots deleted before their result streams were materialized.
    pub deleted_outputs: HashSet<String>,
    /// Streams that already have a durable cancellation intent.
    pub existing_intents: HashSet<StreamId>,
}

#[derive(Clone, Default, desert_rust::BinaryCodec)]
/// Projection of one session's journal records through `covered_through`.
/// Local refreshes include the append buffer; persisted projections cover committed history.
pub struct SessionControlMetadata {
    covered_through: OplogIndex,
    recovery_slot: Option<u64>,
    prepared: Option<OplogIndex>,
    initial_attached: Option<OplogIndex>,
    malformed_record: bool,
    explicit_mappings: HashSet<(u64, DurableStreamHandle, SessionStreamRole)>,
    persisted_mappings: HashSet<(u64, DurableStreamHandle, SessionStreamRole)>,
    recoverable_mappings: Vec<(OplogIndex, StreamSessionMappingRecord)>,
    acceptance_mappings: Vec<StreamSessionMappingRecord>,
    caller_attempt: Option<AttemptId>,
    caller_attempt_conflict: bool,
    invocation_result: Option<OplogIndex>,
    finished: Option<OplogIndex>,
    root_outputs: Vec<u64>,
    topology_epoch: Option<u64>,
    topologies: HashMap<
        (
            AttachmentId,
            golem_common::model::StreamId,
            u64,
            SessionStreamRole,
        ),
        SessionTopologyMetadata,
    >,
    visible_mappings: HashSet<(u64, DurableStreamHandle, SessionStreamRole)>,
    topology_error: Option<String>,
    finalized_attachments:
        HashMap<(AttachmentId, golem_common::model::StreamId), StreamAttachmentKey>,
    closed_consumer_streams: HashSet<golem_common::model::StreamId>,
    cancel_intents: HashMap<golem_common::model::StreamId, StreamConsumerCancelIntentRecord>,
    applied_cancel_intents: HashSet<StreamConsumerCancelIntentRecord>,
    tombstoned_slots: HashMap<String, SessionStreamRole>,
    cancellation_requested: bool,
    consumer_record_counts: HashMap<golem_common::model::StreamId, u64>,
    consumer_deleting: Option<golem_common::model::durable_stream::StreamConsumerDeletingRecord>,
}

#[derive(Clone, desert_rust::BinaryCodec)]
/// Durable attachment topology projected from one stream session journal.
struct SessionTopologyMetadata {
    attachment: StreamAttachmentKey,
    mapping: StreamSessionMappingRecord,
    active: bool,
    prepared_index: Option<OplogIndex>,
    activated_index: Option<OplogIndex>,
    repeated_activation_index: Option<OplogIndex>,
}

impl SessionControlMetadata {
    /// Returns the last oplog index covered by this projection.
    pub fn covered_through(&self) -> OplogIndex {
        self.covered_through
    }
    /// Records the examined horizon, including spans containing no records for this session.
    pub fn advance_coverage(&mut self, index: OplogIndex) {
        self.covered_through = index;
    }
    /// Returns whether persisted index coverage has been loaded.
    pub fn is_loaded(&self) -> bool {
        self.covered_through.is_defined()
    }
    /// Rejects a projection containing an unsupported record format.
    pub fn ensure_valid(&self) -> Result<(), String> {
        if self.malformed_record {
            Err("unsupported or malformed durable Stream Session record version".into())
        } else {
            Ok(())
        }
    }
    /// Restores envelope-owned index state without changing serialized projection layout.
    pub fn restore_index_state(
        &mut self,
        covered_through: OplogIndex,
        consumer_deleting: Option<
            golem_common::model::durable_stream::StreamConsumerDeletingRecord,
        >,
    ) {
        self.covered_through = covered_through;
        self.consumer_deleting = consumer_deleting;
    }
    /// Returns the recovery catalogue slot assigned to this session.
    pub fn recovery_slot(&self) -> Option<u64> {
        self.recovery_slot
    }
    /// Assigns or clears this session's recovery catalogue slot.
    pub fn assign_recovery_slot(&mut self, slot: Option<u64>) {
        self.recovery_slot = slot;
    }
    /// Returns the durable Prepared record position.
    pub fn prepared_position(&self) -> Option<OplogIndex> {
        self.prepared
    }
    /// Returns the initial Attached record position.
    pub fn attached_position(&self) -> Option<OplogIndex> {
        self.initial_attached
    }
    /// Returns the invocation result record position.
    pub fn result_position(&self) -> Option<OplogIndex> {
        self.invocation_result
    }
    /// Returns the session terminal record position.
    pub fn finished_position(&self) -> Option<OplogIndex> {
        self.finished
    }
    /// Returns the currently folded attachment epoch.
    pub fn topology_epoch(&self) -> Option<u64> {
        self.topology_epoch
    }
    /// Returns the unique folded caller attempt, rejecting conflicting records.
    pub fn caller_attempt_id(&self) -> Result<Option<AttemptId>, String> {
        if self.caller_attempt_conflict {
            Err("conflicting caller attempt IDs are persisted for the Stream Session".into())
        } else {
            Ok(self.caller_attempt)
        }
    }
    /// Returns whether this exact mapping has an explicit Mapping record.
    pub fn has_explicit_mapping(&self, mapping: &StreamSessionMappingRecord) -> bool {
        self.explicit_mappings.contains(&(
            mapping.transport_stream_id,
            mapping.handle.clone(),
            mapping.role,
        ))
    }
    /// Returns whether this exact mapping is established by any persisted session record.
    pub fn has_persisted_mapping(&self, mapping: &StreamSessionMappingRecord) -> bool {
        self.persisted_mappings.contains(&(
            mapping.transport_stream_id,
            mapping.handle.clone(),
            mapping.role,
        ))
    }
    /// Selects recoverable mappings newer than the supplied runtime horizon.
    pub fn recoverable_mappings_after(&self, covered: OplogIndex) -> RecoveredMappings {
        RecoveredMappings {
            covered_through: self.covered_through,
            mappings: self
                .recoverable_mappings
                .iter()
                .filter(|(index, _)| *index > covered)
                .map(|(_, mapping)| mapping.clone())
                .collect(),
        }
    }
    /// Checks whether the stream has a terminal and validates its exact persisted mapping.
    pub fn has_consumer_terminal(
        &self,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<bool, String> {
        if !self
            .closed_consumer_streams
            .contains(&mapping.handle.stream_id)
        {
            return Ok(false);
        }
        self.ensure_valid()?;
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        if !self.persisted_mappings.contains(&(
            mapping.transport_stream_id,
            mapping.handle.clone(),
            mapping.role,
        )) {
            return Err("closed consumer stream has no matching persisted mapping".into());
        }
        Ok(true)
    }
    /// Returns the first durable cancellation intent for a stream.
    pub fn cancellation_intent(
        &self,
        stream: golem_common::model::StreamId,
    ) -> Option<&StreamConsumerCancelIntentRecord> {
        self.cancel_intents.get(&stream)
    }
    /// Returns whether the exact cancellation intent has an applied receipt.
    pub fn is_cancellation_applied(&self, intent: &StreamConsumerCancelIntentRecord) -> bool {
        self.applied_cancel_intents.contains(intent)
    }
    /// Returns whether this session has durable Prepared authority.
    pub fn is_prepared(&self) -> bool {
        self.prepared.is_some()
    }
    /// Returns whether whole-session cancellation was durably requested.
    pub fn cancellation_requested(&self) -> bool {
        self.cancellation_requested
    }
    /// Returns whether the named result slot is tombstoned.
    pub fn is_slot_tombstoned(&self, slot: &str) -> bool {
        self.tombstoned_slots.contains_key(slot)
    }
    /// Checks for a persisted mapping with the exact handle identity and role.
    pub fn has_persisted_handle(
        &self,
        handle: &DurableStreamHandle,
        role: SessionStreamRole,
    ) -> bool {
        self.persisted_mappings
            .iter()
            .any(|(_, saved, saved_role)| saved == handle && *saved_role == role)
    }
    /// Checks whether whole-session cancellation can be requested.
    pub fn can_request_cancellation(&self) -> Result<bool, String> {
        if self.prepared.is_none() {
            return Ok(false);
        }
        if self.malformed_record || self.topology_error.is_some() {
            return Err("cannot cancel a malformed durable stream session".into());
        }
        Ok(true)
    }
    /// Builds the durable records needed to request cancellation of every open stream.
    pub fn cancellation_records(
        &self,
        epoch: u64,
        key: &StreamSessionKey,
    ) -> Result<Option<Vec<StreamSessionRecord>>, String> {
        if !self.can_request_cancellation()? {
            return Ok(None);
        }
        let mut records = Vec::new();
        if !self.cancellation_requested {
            records.push(StreamSessionRecord::CancelRequested(
                StreamSessionCancelRequestedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: key.clone(),
                },
            ));
        }
        let mut cancelled = self.cancel_intents.keys().copied().collect::<HashSet<_>>();
        for (_, handle, role) in &self.persisted_mappings {
            if cancelled.insert(handle.stream_id) {
                records.push(StreamSessionRecord::ConsumerCancelIntent(
                    StreamConsumerCancelIntentRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: key.clone(),
                        stream_id: handle.stream_id,
                        epoch,
                        role: match role {
                            SessionStreamRole::Input => StreamCancelRole::InputProducer,
                            SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
                        },
                        reason: StreamCancelReason::Cancelled,
                        details: None,
                    },
                ));
            }
        }
        Ok(Some(records))
    }
    /// Returns the folded facts needed while materializing an invocation result.
    pub fn materialization_state(&self) -> ResultMaterializationState {
        ResultMaterializationState {
            first_result: self.invocation_result.is_none(),
            cancel_session: self.cancellation_requested,
            deleted_outputs: self
                .tombstoned_slots
                .iter()
                .filter(|(_, role)| **role == SessionStreamRole::Output)
                .map(|(slot, _)| slot.clone())
                .collect(),
            existing_intents: self.cancel_intents.keys().copied().collect(),
        }
    }
    /// Validates that every prepared topology is active and visible.
    pub fn validate_topology_complete(&self) -> Result<(), String> {
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        for topology in self.topologies.values() {
            if !topology.active {
                return Err("durable session has prepared but inactive foreign topology".into());
            }
            let mapping = &topology.mapping;
            if !self.visible_mappings.contains(&(
                mapping.transport_stream_id,
                mapping.handle.clone(),
                mapping.role,
            )) {
                return Err(
                    "durable session has activated foreign topology without a visible mapping"
                        .into(),
                );
            }
        }
        Ok(())
    }
    /// Returns transport IDs classified as root outputs.
    pub fn root_outputs(&self) -> &[u64] {
        &self.root_outputs
    }
    /// Returns the number of indexed consumer records for a stream.
    pub fn consumer_record_count(&self, stream: golem_common::model::StreamId) -> u64 {
        self.consumer_record_counts
            .get(&stream)
            .copied()
            .unwrap_or_default()
    }
    /// Returns the durable consumer deletion fence, if any.
    pub fn consumer_deleting(
        &self,
    ) -> Option<&golem_common::model::durable_stream::StreamConsumerDeletingRecord> {
        self.consumer_deleting.as_ref()
    }

    /// Counts exact persisted mappings for cross-module projection assertions.
    #[cfg(test)]
    pub(crate) fn persisted_mapping_count(&self) -> usize {
        self.persisted_mappings.len()
    }

    /// Counts topology slots for cross-module projection assertions.
    #[cfg(test)]
    pub(crate) fn topology_count(&self) -> usize {
        self.topologies.len()
    }

    /// Checks that folding recorded no topology conflict.
    #[cfg(test)]
    pub(crate) fn topology_is_valid(&self) -> bool {
        self.topology_error.is_none()
    }

    /// Exposes the complete tombstone set for exact projection assertions.
    #[cfg(test)]
    pub(crate) fn tombstoned_slots(&self) -> &HashMap<String, SessionStreamRole> {
        &self.tombstoned_slots
    }

    /// Counts retained cancellation intents, including those already applied.
    #[cfg(test)]
    pub(crate) fn cancellation_intent_count(&self) -> usize {
        self.cancel_intents.len()
    }

    /// Counts exact cancellation receipts retained by the projection.
    #[cfg(test)]
    pub(crate) fn applied_cancellation_count(&self) -> usize {
        self.applied_cancel_intents.len()
    }

    /// Constructs an epoch mismatch without rewriting persisted topology records.
    #[cfg(test)]
    pub(crate) fn set_topology_epoch_for_test(&mut self, epoch: u64) {
        self.topology_epoch = Some(epoch);
    }

    /// Removes intent authority to exercise rejection of an incomplete projection.
    #[cfg(test)]
    pub(crate) fn clear_cancellation_intents_for_test(&mut self) {
        self.cancel_intents.clear();
    }

    /// Constructs a finished projection without an invocation's journal setup.
    #[cfg(test)]
    pub(crate) fn set_finished_for_test(&mut self, index: OplogIndex) {
        self.finished = Some(index);
    }

    /// Selects unresolved intents with their persisted mappings; iteration order is unspecified.
    pub fn pending_cancellations(
        &self,
    ) -> impl Iterator<Item = Result<CancellationWork, String>> + '_ {
        self.cancel_intents
            .values()
            .filter(|intent| !self.applied_cancel_intents.contains(*intent))
            .map(|intent| {
                let (transport_stream_id, handle, role) = self
                    .persisted_mappings
                    .iter()
                    .find(|(_, handle, _)| handle.stream_id == intent.stream_id)
                    .ok_or("durable cancellation intent has no persisted stream mapping")?;
                Ok(CancellationWork {
                    mapping: StreamSessionMappingRecord {
                        transport_stream_id: *transport_stream_id,
                        handle: handle.clone(),
                        role: *role,
                    },
                    intent: intent.clone(),
                })
            })
    }

    /// Resolves consumer invocation authority from a mapping's recorded topology.
    pub fn consumer_invocation(
        &self,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<&StreamSessionKey, String> {
        self.topologies
            .values()
            .find(|topology| &topology.mapping == mapping)
            .map(|topology| &topology.attachment.consumer_invocation)
            .ok_or_else(|| "foreign cancellation has no consumer invocation authority".into())
    }
    /// Returns mappings that were durably established before session acceptance.
    pub fn acceptance_mappings(&self) -> Result<Vec<StreamSessionMappingRecord>, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        Ok(self.acceptance_mappings.clone())
    }

    /// Checks that cancellation and its exact topology mapping are both committed.
    pub fn has_committed_cancellation(
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
    pub fn needs_recovery(
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
    pub fn has_cancellation_intents(&self) -> bool {
        self.cancel_intents
            .values()
            .any(|intent| !self.applied_cancel_intents.contains(intent))
    }

    /// Returns whether attachments must be reconstructed or finalized from the journal.
    pub fn needs_topology_recovery(
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
    pub fn recovery_topologies(
        &self,
        owner: &golem_common::model::OwnedAgentId,
        key: &StreamSessionKey,
    ) -> Result<Vec<RecoveryTopology>, String> {
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
            .map(|topology| RecoveryTopology {
                attachment_key: topology.attachment.clone(),
                mapping: topology.mapping.clone(),
            })
            .collect())
    }

    /// Folds the exact attachment slot into its durable prepared or active phase.
    pub fn topology_status(
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
    pub fn apply(
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
                if let Some(stdout) = record.tool_stdout
                    && !self.root_outputs.contains(&stdout)
                {
                    self.root_outputs.push(stdout);
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::tests::identity;
    use golem_common::base_model::durable_stream::{
        StreamConsumerCancelAppliedRecord, StreamSessionMappingUpdateRecord,
    };
    use golem_common::model::component::ComponentRevision;
    use golem_schema::schema::SchemaFingerprintV1;
    use test_r::test;
    use uuid::Uuid;

    fn mapping() -> StreamSessionMappingRecord {
        let owner = identity();
        StreamSessionMappingRecord {
            transport_stream_id: 91,
            role: SessionStreamRole::Output,
            handle: DurableStreamHandle {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: StreamId(Uuid::from_u128(17)),
                producer_environment_id: owner.environment_id,
                producer: owner.agent_id,
                expected_producer_fingerprint: owner.fingerprint,
                source_invocation: owner.invocation,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
            },
        }
    }

    #[test]
    fn terminal_evidence_validates_exact_mapping_only_after_a_terminal() {
        let mapping = mapping();
        // An incomplete projection can lack terminal evidence without claiming validity.
        let mut state = SessionControlMetadata {
            malformed_record: true,
            ..Default::default()
        };
        assert!(!state.has_consumer_terminal(&mapping).unwrap());
        state
            .closed_consumer_streams
            .insert(mapping.handle.stream_id);
        assert!(state.has_consumer_terminal(&mapping).is_err());
        state.malformed_record = false;
        assert!(state.has_consumer_terminal(&mapping).is_err());
        state.persisted_mappings.insert((
            mapping.transport_stream_id,
            mapping.handle.clone(),
            mapping.role,
        ));
        assert!(state.has_consumer_terminal(&mapping).unwrap());
        let mut wrong = mapping.clone();
        wrong.transport_stream_id = 7;
        assert!(state.has_consumer_terminal(&wrong).is_err());
        wrong = mapping.clone();
        wrong.role = SessionStreamRole::Input;
        assert!(state.has_consumer_terminal(&wrong).is_err());
        wrong = mapping.clone();
        wrong.handle.element_schema_fingerprint = SchemaFingerprintV1([8; 32]);
        assert!(state.has_consumer_terminal(&wrong).is_err());
        state.topology_error = Some("invalid topology".into());
        assert!(state.has_consumer_terminal(&mapping).is_err());
    }

    #[test]
    fn pending_cancellation_requires_mapping_and_exact_applied_receipt() {
        let mapping = mapping();
        let key = identity().invocation;
        let intent = StreamConsumerCancelIntentRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: key.clone(),
            stream_id: mapping.handle.stream_id,
            epoch: 3,
            role: StreamCancelRole::OutputConsumer,
            reason: StreamCancelReason::GuestDrop,
            details: Some("closed by consumer".into()),
        };
        let mut state = SessionControlMetadata::default();
        state.apply(
            OplogIndex::from_u64(1),
            &key,
            &StreamSessionRecord::ConsumerCancelIntent(intent.clone()),
        );
        assert!(state.pending_cancellations().next().unwrap().is_err());
        state.apply(
            OplogIndex::from_u64(2),
            &key,
            &StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: key.clone(),
                mapping: mapping.clone(),
            }),
        );
        let expected = vec![CancellationWork {
            mapping,
            intent: intent.clone(),
        }];
        assert_eq!(
            state
                .pending_cancellations()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            expected
        );
        let mut stale = intent.clone();
        stale.epoch = 2;
        state.apply(
            OplogIndex::from_u64(3),
            &key,
            &StreamSessionRecord::ConsumerCancelApplied(StreamConsumerCancelAppliedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                intent: stale,
            }),
        );
        assert_eq!(
            state
                .pending_cancellations()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            expected
        );
        state.apply(
            OplogIndex::from_u64(4),
            &key,
            &StreamSessionRecord::ConsumerCancelApplied(StreamConsumerCancelAppliedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                intent,
            }),
        );
        assert!(state.pending_cancellations().next().is_none());
    }

    #[test]
    fn recovered_mappings_exclude_the_old_horizon_and_report_full_coverage() {
        let key = identity().invocation;
        let first = mapping();
        let mut second = first.clone();
        second.transport_stream_id = 7;
        let mut state = SessionControlMetadata::default();
        for (index, mapping) in [(4, first), (9, second.clone())] {
            state.apply(
                OplogIndex::from_u64(index),
                &key,
                &StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: key.clone(),
                    mapping,
                }),
            );
        }
        state.advance_coverage(OplogIndex::from_u64(12));
        let recovered = state.recoverable_mappings_after(OplogIndex::from_u64(4));
        assert_eq!(recovered.covered_through, OplogIndex::from_u64(12));
        assert_eq!(recovered.mappings, vec![second]);
        let recovered = state.recoverable_mappings_after(OplogIndex::from_u64(9));
        assert_eq!(recovered.covered_through, OplogIndex::from_u64(12));
        assert!(recovered.mappings.is_empty());
    }
}
