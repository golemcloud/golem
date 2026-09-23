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

use super::{ConsumerAttachmentStatus, qualify_local_stream};
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, LocalStreamReaderId, SessionStreamRole,
    StreamAttachmentKey, StreamBindingRecord, StreamCancelReason, StreamCancelRole,
    StreamConsumerCancelIntentRecord, StreamForkCutRecord, StreamRecordReference,
    StreamRegistrationInvocation, StreamSessionCancelRequestedRecord, StreamSessionKey,
    StreamSessionMappingRecord, StreamSessionRecord,
};
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentFingerprint, AgentId};
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
    pub binding: StreamBindingRecord,
    /// Pending durable cancellation intent.
    pub intent: StreamConsumerCancelIntentRecord,
}

/// Mapping additions and the horizon through which they were selected.
pub struct RecoveredMappings {
    /// Last examined oplog position, including records that added no mappings.
    pub covered_through: OplogIndex,
    /// Mappings established strictly after the caller's previous horizon.
    pub mappings: Vec<StreamBindingRecord>,
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
    pub existing_intents: HashSet<StreamRecordReference>,
}

#[derive(Clone, Default, desert_rust::BinaryCodec)]
#[cfg_attr(test, derive(Debug, PartialEq))]
/// Projection of one session's journal records through `covered_through`.
/// Local refreshes include the append buffer; persisted projections cover committed history.
pub struct SessionControlMetadata {
    covered_through: OplogIndex,
    recovery_slot: Option<u64>,
    prepared: Option<OplogIndex>,
    initial_attached: Option<OplogIndex>,
    malformed_record: bool,
    explicit_mappings: HashSet<StreamBindingRecord>,
    persisted_mappings: HashSet<StreamBindingRecord>,
    recoverable_mappings: Vec<(OplogIndex, StreamBindingRecord)>,
    acceptance_mappings: Vec<StreamBindingRecord>,
    reader_bindings: Vec<(LocalStreamReaderId, StreamBindingRecord)>,
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
    visible_mappings: HashSet<StreamBindingRecord>,
    topology_error: Option<String>,
    finalized_attachments:
        HashMap<(AttachmentId, golem_common::model::StreamId), StreamAttachmentKey>,
    closed_consumer_streams: HashSet<LocalStreamReaderId>,
    cancel_intents: HashMap<StreamRecordReference, StreamConsumerCancelIntentRecord>,
    applied_cancel_intents: HashSet<StreamConsumerCancelIntentRecord>,
    tombstoned_slots: HashMap<String, SessionStreamRole>,
    cancellation_requested: bool,
    consumer_record_counts: HashMap<LocalStreamReaderId, u64>,
    consumer_deleting: Option<golem_common::model::durable_stream::StreamConsumerDeletingRecord>,
}

#[derive(Clone, desert_rust::BinaryCodec)]
#[cfg_attr(test, derive(Debug, PartialEq))]
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
    pub fn has_explicit_mapping(&self, binding: &StreamBindingRecord) -> bool {
        self.explicit_mappings.contains(binding)
    }
    /// Returns whether this exact mapping is established by any persisted session record.
    pub fn has_persisted_mapping(&self, binding: &StreamBindingRecord) -> bool {
        self.persisted_mappings.contains(binding)
    }
    /// Resolves a wire mapping to its original local binding, independent of later evidence.
    pub fn reader_id(&self, binding: &StreamBindingRecord) -> Result<LocalStreamReaderId, String> {
        self.reader_bindings
            .iter()
            .find(|(_, candidate)| candidate == binding)
            .map(|(id, _)| *id)
            .ok_or_else(|| "stream reader has no durable binding".to_string())
    }

    /// Looks up the source recorded when a reader was introduced.
    pub fn reader_binding(&self, id: LocalStreamReaderId) -> Result<&StreamBindingRecord, String> {
        self.reader_bindings
            .iter()
            .find(|(reader, _)| *reader == id)
            .map(|(_, binding)| binding)
            .ok_or_else(|| "consumer observation refers to an unknown reader binding".to_string())
    }

    /// Returns every local reader bound to the qualified source attachment.
    pub fn readers_for_attachment(&self, key: &StreamAttachmentKey) -> Vec<LocalStreamReaderId> {
        self.reader_bindings
            .iter()
            .filter_map(|(id, binding)| match &binding.source {
                StreamRecordReference::Foreign(handle)
                    if handle.stream_id == key.stream_id
                        && handle.producer_environment_id == key.producer_environment_id
                        && handle.producer == key.producer
                        && handle.expected_producer_fingerprint
                            == key.expected_producer_fingerprint =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect()
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
    pub fn has_consumer_terminal(&self, binding: &StreamBindingRecord) -> Result<bool, String> {
        let reader_id = self.reader_id(binding)?;
        if !self.closed_consumer_streams.contains(&reader_id) {
            return Ok(false);
        }
        self.ensure_valid()?;
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        if !self.persisted_mappings.contains(binding) {
            return Err("closed consumer stream has no matching persisted mapping".into());
        }
        Ok(true)
    }
    /// Returns the first durable cancellation intent for a stream.
    pub fn cancellation_intent(
        &self,
        source: &StreamRecordReference,
    ) -> Option<&StreamConsumerCancelIntentRecord> {
        self.cancel_intents.get(source)
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
    pub fn has_persisted_source(
        &self,
        source: &StreamRecordReference,
        role: SessionStreamRole,
    ) -> bool {
        self.persisted_mappings
            .iter()
            .any(|binding| &binding.source == source && binding.role == role)
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
        session_reference: &StreamRegistrationInvocation,
    ) -> Result<Option<Vec<StreamSessionRecord>>, String> {
        if !self.can_request_cancellation()? {
            return Ok(None);
        }
        let mut records = Vec::new();
        if !self.cancellation_requested {
            records.push(StreamSessionRecord::CancelRequested(
                StreamSessionCancelRequestedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: session_reference.clone(),
                },
            ));
        }
        let mut cancelled = self.cancel_intents.keys().cloned().collect::<HashSet<_>>();
        for binding in &self.persisted_mappings {
            if cancelled.insert(binding.source.clone()) {
                records.push(StreamSessionRecord::ConsumerCancelIntent(
                    StreamConsumerCancelIntentRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_reference.clone(),
                        consumer_invocation: session_reference.idempotency_key().clone(),
                        source: binding.source.clone(),
                        epoch,
                        role: match binding.role {
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
            existing_intents: self.cancel_intents.keys().cloned().collect(),
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
            if !self
                .visible_mappings
                .contains(&StreamBindingRecord::foreign(mapping))
            {
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
    pub fn consumer_record_count(&self, reader: LocalStreamReaderId) -> u64 {
        self.consumer_record_counts
            .get(&reader)
            .copied()
            .unwrap_or_default()
    }
    /// Returns consumer journal counts for fork-prefix indexing.
    #[cfg(test)]
    pub(crate) fn consumer_record_counts(&self) -> &HashMap<LocalStreamReaderId, u64> {
        &self.consumer_record_counts
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
                let binding = self
                    .persisted_mappings
                    .iter()
                    .find(|binding| binding.source == intent.source)
                    .ok_or("durable cancellation intent has no persisted stream mapping")?;
                Ok(CancellationWork {
                    binding: binding.clone(),
                    intent: intent.clone(),
                })
            })
    }

    /// Returns mappings that were durably established before session acceptance.
    pub fn acceptance_mappings(&self) -> Result<Vec<StreamBindingRecord>, String> {
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
        let consumer_matches = key.consumer_invocation
            == StreamRegistrationInvocation::Local(intent.consumer_invocation.clone()).qualify(
                key.consumer_environment_id,
                &key.consumer,
                key.expected_consumer_fingerprint,
            );
        let source_matches = match &intent.source {
            StreamRecordReference::Local(id) => {
                mapping.handle.producer_environment_id == key.consumer_environment_id
                    && mapping.handle.producer == key.consumer
                    && mapping.handle.expected_producer_fingerprint
                        == key.expected_consumer_fingerprint
                    && qualify_local_stream(
                        *id,
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    )
                    .is_ok_and(|stream_id| stream_id == mapping.handle.stream_id)
            }
            StreamRecordReference::Foreign(handle) => handle == &mapping.handle,
        };
        Ok(consumer_matches
            && intent.session_key.qualify(
                key.consumer_environment_id,
                &key.consumer,
                key.expected_consumer_fingerprint,
            ) == key.session_key
            && source_matches
            && intent.epoch == key.epoch
            && mapping.handle.stream_id == key.stream_id
            && self
                .cancel_intents
                .get(&intent.source)
                .is_some_and(|persisted| {
                    persisted.session_key.qualify(
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    ) == key.session_key
                        && persisted.format_version == intent.format_version
                        && persisted.consumer_invocation == intent.consumer_invocation
                        && persisted.epoch == intent.epoch
                        && persisted.role == intent.role
                        && persisted.reason == intent.reason
                        && persisted.details == intent.details
                })
            && self.persisted_mappings.contains(&StreamBindingRecord {
                transport_stream_id: mapping.transport_stream_id,
                source: intent.source.clone(),
                role: mapping.role,
            }))
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
                        .contains_key(&StreamRecordReference::Foreign(
                            topology.mapping.handle.clone(),
                        ))
                    && !self
                        .reader_id(&StreamBindingRecord::foreign(&topology.mapping))
                        .is_ok_and(|reader| self.closed_consumer_streams.contains(&reader))
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
                        .contains_key(&StreamRecordReference::Foreign(
                            topology.mapping.handle.clone(),
                        ))
                    && !self
                        .reader_id(&StreamBindingRecord::foreign(&topology.mapping))
                        .is_ok_and(|reader| self.closed_consumer_streams.contains(&reader))
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
        owner_environment_id: EnvironmentId,
        owner: &AgentId,
        owner_fingerprint: AgentFingerprint,
    ) {
        let record_matches = crate::worker::stream_session_record_key(
            record,
            owner_environment_id,
            owner,
            owner_fingerprint,
        )
        .as_ref()
            == Some(key);
        self.malformed_record |= !record.has_supported_format();
        if let StreamSessionRecord::ConsumerDeleting(record) = record {
            self.consumer_deleting = Some(record.clone());
        }
        if let StreamSessionRecord::ConsumerCancelIntent(record) = record
            && record_matches
        {
            self.cancel_intents
                .entry(record.source.clone())
                .or_insert_with(|| record.clone());
        }
        if let StreamSessionRecord::ConsumerCancelApplied(record) = record
            && record_matches
            && self.cancel_intents.get(&record.intent.source) == Some(&record.intent)
        {
            self.applied_cancel_intents.insert(record.intent.clone());
        }
        if let StreamSessionRecord::Tombstoned(record) = record
            && record_matches
        {
            self.tombstoned_slots
                .entry(record.slot.clone())
                .or_insert(record.role);
        }
        if let StreamSessionRecord::CancelRequested(_) = record
            && record_matches
        {
            self.cancellation_requested = true;
        }
        if let StreamSessionRecord::Prepared(_) = record
            && record_matches
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
            && record_matches
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
        let consumer_reader = match record {
            StreamSessionRecord::ConsumerItemValue(record) if record_matches => {
                Some(record.reader_id)
            }
            StreamSessionRecord::ConsumerTerminal(record) if record_matches => {
                Some(record.reader_id)
            }
            StreamSessionRecord::SourceUnavailable(record) if record_matches => {
                Some(record.reader_id)
            }
            _ => None,
        };
        if let Some(reader) = consumer_reader {
            *self.consumer_record_counts.entry(reader).or_default() += 1;
            if matches!(
                record,
                StreamSessionRecord::ConsumerTerminal(_)
                    | StreamSessionRecord::SourceUnavailable(_)
            ) {
                self.closed_consumer_streams.insert(reader);
            }
        }
        match record {
            StreamSessionRecord::Attached(record) if record_matches => {
                if self.initial_attached.is_some() {
                    self.topology_error.get_or_insert_with(|| {
                        "durable Stream Session contains multiple Attached records".into()
                    });
                } else {
                    self.initial_attached = Some(index);
                }
                self.topology_epoch = Some(record.epoch);
            }
            StreamSessionRecord::ResumeAttempt(record) if record_matches => {
                self.topology_epoch = Some(record.accepted_epoch);
            }
            _ => {}
        }
        let topology = match record {
            StreamSessionRecord::TopologyPrepared(record) if record_matches => {
                Some((&record.attachment, &record.mapping, false))
            }
            StreamSessionRecord::TopologyActivated(record) if record_matches => {
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
        let bindings: Vec<StreamBindingRecord> = match record {
            StreamSessionRecord::CallerAttempt(record) if record_matches => {
                if self
                    .caller_attempt
                    .is_some_and(|attempt| attempt != record.attempt_id)
                {
                    self.caller_attempt_conflict = true;
                }
                self.caller_attempt.get_or_insert(record.attempt_id);
                vec![]
            }
            StreamSessionRecord::Mapping(record) if record_matches => {
                self.explicit_mappings.insert(record.mapping.clone());
                vec![record.mapping.clone()]
            }
            StreamSessionRecord::Prepared(record) if record_matches => {
                for mapping in &record.stream_mappings {
                    if mapping.role == SessionStreamRole::Output
                        && !self.root_outputs.contains(&mapping.transport_stream_id)
                    {
                        self.root_outputs.push(mapping.transport_stream_id);
                    }
                }
                record.stream_mappings.clone()
            }
            StreamSessionRecord::TopologyPrepared(record) if record_matches => {
                vec![StreamBindingRecord::foreign(&record.mapping)]
            }
            StreamSessionRecord::TopologyActivated(record) if record_matches => {
                vec![StreamBindingRecord::foreign(&record.mapping)]
            }
            StreamSessionRecord::ConsumerItemValue(record) if record_matches => {
                record.recursive_mappings.clone()
            }
            StreamSessionRecord::InvocationResult(record) if record_matches => {
                self.invocation_result.get_or_insert(index);
                for mapping in &record.stream_mappings {
                    if mapping.role == SessionStreamRole::Output
                        && !self.root_outputs.contains(&mapping.transport_stream_id)
                    {
                        self.root_outputs.push(mapping.transport_stream_id);
                    }
                }
                record.stream_mappings.clone()
            }
            StreamSessionRecord::Finished(record) if record_matches => {
                self.finished.get_or_insert(index);
                vec![]
            }
            _ => vec![],
        };
        if matches!(
            record,
            StreamSessionRecord::Prepared(_)
                | StreamSessionRecord::Mapping(_)
                | StreamSessionRecord::InvocationResult(_)
                | StreamSessionRecord::ConsumerItemValue(_)
        ) {
            for (slot, binding) in bindings.iter().enumerate() {
                if self.reader_id(binding).is_ok() {
                    continue;
                }
                self.reader_bindings.push((
                    LocalStreamReaderId {
                        introducing_oplog_index: index,
                        binding_slot: slot as u32,
                    },
                    binding.clone(),
                ));
            }
        }
        for binding in &bindings {
            if !self.acceptance_mappings.contains(binding) {
                self.acceptance_mappings.push(binding.clone());
            }
        }
        if matches!(
            record,
            StreamSessionRecord::Prepared(_)
                | StreamSessionRecord::Mapping(_)
                | StreamSessionRecord::InvocationResult(_)
                | StreamSessionRecord::ConsumerItemValue(_)
        ) {
            self.visible_mappings.extend(bindings.iter().cloned());
        }
        if matches!(
            record,
            StreamSessionRecord::Prepared(_)
                | StreamSessionRecord::Mapping(_)
                | StreamSessionRecord::InvocationResult(_)
                | StreamSessionRecord::ConsumerItemValue(_)
        ) {
            for binding in &bindings {
                if !self
                    .recoverable_mappings
                    .iter()
                    .any(|(_, existing)| existing == binding)
                {
                    self.recoverable_mappings.push((index, binding.clone()));
                }
            }
        }
        self.persisted_mappings.extend(bindings);
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

impl SessionControlMetadata {
    /// Projects retained consumer observations, not live producer attachment authority.
    pub(crate) fn for_fork(&self, cut: &StreamForkCutRecord) -> Result<Self, String> {
        let mut result = self.clone();
        result.covered_through = cut
            .revert
            .as_ref()
            .map_or(cut.cut_index.next(), |region| region.end.next().next());
        result.recovery_slot = None;
        if result.topology_epoch.is_some() {
            result.topology_epoch = Some(cut.epoch_floor);
        }
        result.topologies.clear();
        result.finalized_attachments.clear();
        result.consumer_deleting = None;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::tests::identity;
    use golem_common::base_model::durable_stream::{
        DurableStreamHandle, StreamCallerAttemptRecord, StreamConsumerCancelAppliedRecord,
        StreamSessionMappingUpdateRecord,
    };
    use golem_common::model::StreamId;
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
                producer_generation: OplogIndex::NONE,
                source_invocation: owner.invocation,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
            },
        }
    }

    fn binding(mapping: &StreamSessionMappingRecord) -> StreamBindingRecord {
        StreamBindingRecord::foreign(mapping)
    }

    fn cut() -> StreamForkCutRecord {
        StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            creation_fingerprint: identity().fingerprint,
            export: None,
            cut_index: OplogIndex::from_u64(20),
            revert: None,
            epoch_floor: 1,
            selected_stream_id: None,
            retained_through: None,
        }
    }

    fn mapping_for(session: &StreamSessionKey, id: u128) -> StreamSessionMappingRecord {
        StreamSessionMappingRecord {
            transport_stream_id: id as u64,
            role: SessionStreamRole::Output,
            handle: DurableStreamHandle {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: StreamId(Uuid::from_u128(id)),
                producer_environment_id: session.callee_environment_id,
                producer: session.callee.clone(),
                expected_producer_fingerprint: session.callee_fingerprint,
                producer_generation: OplogIndex::NONE,
                source_invocation: session.clone(),
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
            },
        }
    }

    #[test]
    fn repeated_binding_evidence_preserves_one_reader_per_binding() {
        use golem_common::base_model::durable_stream::StreamSessionInvocationResultRecord;

        let owner = identity();
        let key = owner.invocation.clone();
        let first = binding(&mapping());
        let mut second = first.clone();
        second.transport_stream_id += 1;
        let third = binding(&mapping_for(&key, 18));
        let reference = StreamRegistrationInvocation::Local(key.idempotency_key.clone());
        let mut state = SessionControlMetadata::default();
        let records = [
            StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: reference.clone(),
                mapping: first.clone(),
            }),
            StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: reference.clone(),
                result: Vec::new(),
                stream_mappings: vec![first.clone(), third.clone()],
            }),
            StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: reference,
                mapping: second.clone(),
            }),
        ];
        for (position, record) in records.iter().enumerate() {
            assert!(record.has_supported_format());
            state.apply(
                OplogIndex::from_u64(position as u64 + 10),
                &key,
                record,
                owner.environment_id,
                &owner.agent_id,
                owner.fingerprint,
            );
        }
        state.ensure_valid().unwrap();
        let first_reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(10),
            binding_slot: 0,
        };
        let second_reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(12),
            binding_slot: 0,
        };
        let third_reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(11),
            binding_slot: 1,
        };
        assert_eq!(state.reader_id(&first).unwrap(), first_reader);
        assert_eq!(state.reader_id(&second).unwrap(), second_reader);
        assert_eq!(state.reader_id(&third).unwrap(), third_reader);
        assert_eq!(
            state.reader_bindings,
            vec![
                (first_reader, first),
                (third_reader, third),
                (second_reader, second),
            ]
        );
    }

    #[test]
    fn local_session_records_are_qualified_against_the_oplog_owner() {
        let real_owner = identity();
        let key = real_owner.invocation.clone();
        let local = StreamSessionRecord::CallerAttempt(StreamCallerAttemptRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: StreamRegistrationInvocation::Local(key.idempotency_key.clone()),
            attempt_id: AttemptId::fresh(),
        });
        let mut matching = SessionControlMetadata::default();
        matching.apply(
            OplogIndex::from_u64(1),
            &key,
            &local,
            real_owner.environment_id,
            &real_owner.agent_id,
            real_owner.fingerprint,
        );
        assert!(matching.caller_attempt.is_some());

        let remote_environment_id = real_owner.environment_id;
        let mut remote_agent_id = real_owner.agent_id.clone();
        remote_agent_id.agent_id = "remote".into();
        let remote_fingerprint = real_owner.fingerprint;
        let mut mismatching = SessionControlMetadata::default();
        mismatching.apply(
            OplogIndex::from_u64(1),
            &key,
            &local,
            remote_environment_id,
            &remote_agent_id,
            remote_fingerprint,
        );
        assert!(mismatching.caller_attempt.is_none());

        let explicitly_remote = StreamSessionRecord::CallerAttempt(StreamCallerAttemptRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: StreamRegistrationInvocation::Remote(key.clone()),
            attempt_id: AttemptId::fresh(),
        });
        let mut remote = SessionControlMetadata::default();
        remote.apply(
            OplogIndex::from_u64(1),
            &key,
            &explicitly_remote,
            remote_environment_id,
            &remote_agent_id,
            remote_fingerprint,
        );
        assert!(remote.caller_attempt.is_some());
        let StreamSessionRecord::CallerAttempt(record) = explicitly_remote else {
            unreachable!()
        };
        assert!(matches!(
            record.session_key,
            StreamRegistrationInvocation::Remote(_)
        ));
    }

    #[test]
    fn local_fork_keeps_owner_relative_observations_and_discards_runtime_state() {
        let cut = cut();
        let source = identity().invocation;
        let first = mapping_for(&source, 50);
        let discarded = mapping_for(&source, 51);
        let mut old = SessionControlMetadata {
            prepared: Some(OplogIndex::from_u64(2)),
            initial_attached: Some(OplogIndex::from_u64(3)),
            finished: Some(OplogIndex::from_u64(19)),
            caller_attempt: Some(AttemptId::fresh()),
            caller_attempt_conflict: true,
            topology_epoch: Some(3),
            recovery_slot: Some(7),
            cancellation_requested: true,
            ..Default::default()
        };
        old.acceptance_mappings = vec![binding(&first), binding(&discarded)];
        let first_reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(2),
            binding_slot: 0,
        };
        let discarded_reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(2),
            binding_slot: 1,
        };
        old.reader_bindings = vec![
            (first_reader, binding(&first)),
            (discarded_reader, binding(&discarded)),
        ];
        old.consumer_record_counts = HashMap::from([(first_reader, 7), (discarded_reader, 9)]);
        old.closed_consumer_streams.insert(first_reader);
        old.tombstoned_slots
            .insert("$result".into(), SessionStreamRole::Output);
        let intent = StreamConsumerCancelIntentRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: StreamRegistrationInvocation::Local(source.idempotency_key.clone()),
            consumer_invocation: source.idempotency_key.clone(),
            source: binding(&first).source,
            epoch: 3,
            role: StreamCancelRole::OutputConsumer,
            reason: StreamCancelReason::GuestDrop,
            details: None,
        };
        old.cancel_intents
            .insert(intent.source.clone(), intent.clone());
        old.applied_cancel_intents.insert(intent);
        let new = old.for_fork(&cut).unwrap();
        assert_eq!(new.prepared, old.prepared);
        assert_eq!(new.initial_attached, old.initial_attached);
        assert_eq!(new.finished, old.finished);
        assert!(new.recovery_slot.is_none());
        assert_eq!(new.caller_attempt_conflict, old.caller_attempt_conflict);
        assert_eq!(new.caller_attempt, old.caller_attempt);
        assert_eq!(new.topology_epoch, Some(cut.epoch_floor));
        assert!(new.topologies.is_empty());
        assert!(new.finalized_attachments.is_empty());
        assert_eq!(new.consumer_record_counts, old.consumer_record_counts);
        assert_eq!(new.closed_consumer_streams, old.closed_consumer_streams);
        assert_eq!(new.acceptance_mappings, old.acceptance_mappings);
        assert_eq!(new.reader_bindings, old.reader_bindings);
        assert_eq!(new.tombstoned_slots, old.tombstoned_slots);
        assert_eq!(new.cancellation_requested, old.cancellation_requested);
        assert_eq!(new.cancel_intents, old.cancel_intents);
        assert_eq!(new.applied_cancel_intents, old.applied_cancel_intents);
        assert_eq!(old.consumer_record_counts.len(), 2);
    }

    #[test]
    fn fork_retains_remote_cancellation_authority_for_pending_intent() {
        let owner = identity();
        let source = owner.invocation.clone();
        let mapping = mapping_for(&source, 52);
        let mut attachment = crate::durable_host::durable_stream::tests::attachment_key(
            &owner,
            mapping.handle.stream_id,
        );
        attachment.session_key = source.clone();
        attachment.attachment_id = AttachmentId::primary(
            source.callee_environment_id,
            &source.callee,
            &source.idempotency_key,
        )
        .unwrap();
        attachment.epoch = 3;
        attachment.consumer_invocation.idempotency_key =
            golem_common::model::IdempotencyKey::new("consumer-call".into());
        let intent = StreamConsumerCancelIntentRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: StreamRegistrationInvocation::Remote(source.clone()),
            consumer_invocation: attachment.consumer_invocation.idempotency_key.clone(),
            source: binding(&mapping).source,
            epoch: 3,
            role: StreamCancelRole::OutputConsumer,
            reason: StreamCancelReason::GuestDrop,
            details: None,
        };
        let mut old = SessionControlMetadata::default();
        old.persisted_mappings.insert(binding(&mapping));
        old.cancel_intents
            .insert(intent.source.clone(), intent.clone());
        old.topologies.insert(
            (
                attachment.attachment_id,
                attachment.stream_id,
                attachment.epoch,
                mapping.role,
            ),
            SessionTopologyMetadata {
                attachment: attachment.clone(),
                mapping: mapping.clone(),
                active: true,
                prepared_index: Some(OplogIndex::from_u64(4)),
                activated_index: Some(OplogIndex::from_u64(5)),
                repeated_activation_index: None,
            },
        );

        let forked = old.for_fork(&cut()).unwrap();
        let pending = forked
            .pending_cancellations()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert!(forked.topologies.is_empty());
        attachment.consumer.agent_id = "forked-consumer".into();
        attachment.expected_consumer_fingerprint = AgentFingerprint::new();
        attachment.consumer_invocation =
            StreamRegistrationInvocation::Local(intent.consumer_invocation.clone()).qualify(
                attachment.consumer_environment_id,
                &attachment.consumer,
                attachment.expected_consumer_fingerprint,
            );
        assert!(
            forked
                .has_committed_cancellation(&attachment, &mapping, &intent)
                .unwrap()
        );
        attachment.consumer_invocation.idempotency_key =
            golem_common::model::IdempotencyKey::new("other-call".into());
        assert!(
            !forked
                .has_committed_cancellation(&attachment, &mapping, &intent)
                .unwrap()
        );
    }

    #[test]
    fn terminal_evidence_validates_exact_mapping_only_after_a_terminal() {
        let mapping = mapping();
        // An incomplete projection can lack terminal evidence without claiming validity.
        let mut state = SessionControlMetadata {
            malformed_record: true,
            ..Default::default()
        };
        let reader = LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(2),
            binding_slot: 0,
        };
        state.reader_bindings.push((reader, binding(&mapping)));
        assert!(!state.has_consumer_terminal(&binding(&mapping)).unwrap());
        state.closed_consumer_streams.insert(reader);
        assert!(state.has_consumer_terminal(&binding(&mapping)).is_err());
        state.malformed_record = false;
        assert!(state.has_consumer_terminal(&binding(&mapping)).is_err());
        state.persisted_mappings.insert(binding(&mapping));
        assert!(state.has_consumer_terminal(&binding(&mapping)).unwrap());
        let mut wrong = mapping.clone();
        wrong.transport_stream_id = 7;
        state.reader_bindings.push((
            LocalStreamReaderId {
                introducing_oplog_index: OplogIndex::from_u64(3),
                binding_slot: 0,
            },
            binding(&wrong),
        ));
        state.closed_consumer_streams.insert(LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(3),
            binding_slot: 0,
        });
        assert!(state.has_consumer_terminal(&binding(&wrong)).is_err());
        wrong = mapping.clone();
        wrong.role = SessionStreamRole::Input;
        state.reader_bindings.push((
            LocalStreamReaderId {
                introducing_oplog_index: OplogIndex::from_u64(4),
                binding_slot: 0,
            },
            binding(&wrong),
        ));
        state.closed_consumer_streams.insert(LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(4),
            binding_slot: 0,
        });
        assert!(state.has_consumer_terminal(&binding(&wrong)).is_err());
        wrong = mapping.clone();
        wrong.handle.element_schema_fingerprint = SchemaFingerprintV1([8; 32]);
        state.reader_bindings.push((
            LocalStreamReaderId {
                introducing_oplog_index: OplogIndex::from_u64(5),
                binding_slot: 0,
            },
            binding(&wrong),
        ));
        state.closed_consumer_streams.insert(LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(5),
            binding_slot: 0,
        });
        assert!(state.has_consumer_terminal(&binding(&wrong)).is_err());
        state.topology_error = Some("invalid topology".into());
        assert!(state.has_consumer_terminal(&binding(&mapping)).is_err());
    }

    #[test]
    fn pending_cancellation_requires_mapping_and_exact_applied_receipt() {
        let mapping = mapping();
        let owner = identity();
        let key = identity().invocation;
        let intent = StreamConsumerCancelIntentRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: StreamRegistrationInvocation::Local(key.idempotency_key.clone()),
            consumer_invocation: key.idempotency_key.clone(),
            source: StreamRecordReference::Foreign(mapping.handle.clone()),
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
            owner.environment_id,
            &owner.agent_id,
            owner.fingerprint,
        );
        assert!(state.pending_cancellations().next().unwrap().is_err());
        state.apply(
            OplogIndex::from_u64(2),
            &key,
            &StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: StreamRegistrationInvocation::Local(key.idempotency_key.clone()),
                mapping: binding(&mapping),
            }),
            owner.environment_id,
            &owner.agent_id,
            owner.fingerprint,
        );
        let expected = vec![CancellationWork {
            binding: binding(&mapping),
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
            owner.environment_id,
            &owner.agent_id,
            owner.fingerprint,
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
            owner.environment_id,
            &owner.agent_id,
            owner.fingerprint,
        );
        assert!(state.pending_cancellations().next().is_none());
    }

    #[test]
    fn recovered_mappings_exclude_the_old_horizon_and_report_full_coverage() {
        let owner = identity();
        let key = owner.invocation.clone();
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
                    session_key: StreamRegistrationInvocation::Local(key.idempotency_key.clone()),
                    mapping: binding(&mapping),
                }),
                owner.environment_id,
                &owner.agent_id,
                owner.fingerprint,
            );
        }
        state.advance_coverage(OplogIndex::from_u64(12));
        let recovered = state.recoverable_mappings_after(OplogIndex::from_u64(4));
        assert_eq!(recovered.covered_through, OplogIndex::from_u64(12));
        assert_eq!(recovered.mappings, vec![binding(&second)]);
        let recovered = state.recoverable_mappings_after(OplogIndex::from_u64(9));
        assert_eq!(recovered.covered_through, OplogIndex::from_u64(12));
        assert!(recovered.mappings.is_empty());
    }
}
