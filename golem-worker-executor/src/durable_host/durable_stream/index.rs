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

impl IndexedProducerStream {
    pub(super) fn offsets(&self) -> Vec<StreamOffset> {
        let mut offsets = Vec::new();
        let mut batches = self.batches.iter().peekable();
        while let Some((&first_sequence, &index)) = batches.next() {
            let end = batches
                .peek()
                .map_or(self.next_sequence, |(sequence, _)| **sequence);
            offsets.extend(
                (0..end - first_sequence)
                    .map(|sub_index| StreamOffset::new(index, sub_index as u32)),
            );
        }
        if self.terminal {
            offsets.extend(self.last_offset);
        }
        offsets
    }
}

impl ProducerStreamIndex {
    pub(super) fn apply_external_producer_state(
        &mut self,
        record: &StreamExternalProducerStateRecord,
    ) {
        let base = (
            record.session_key.clone(),
            record.stream_id,
            record.producer_id.clone(),
        );
        self.external_producer_heads.insert(
            base.clone(),
            IndexedExternalProducer {
                epoch: record.epoch,
                last_sequence: record.sequence,
                next_sequence: record.next_sequence,
                last_offset: record.resulting_offset,
            },
        );
        self.external_producer_offsets.insert(
            (base.0, base.1, base.2, record.epoch, record.sequence),
            record.resulting_offset,
        );
    }

    pub(super) fn entity_parent_start_index(
        &self,
        stream_id: StreamId,
    ) -> Result<Option<OplogIndex>, StreamStoreError> {
        self.entity_parent_start_indices
            .get(&stream_id)
            .copied()
            .ok_or(StreamStoreError::UnknownStream(stream_id))
    }

    pub(super) fn session_entity_parent_start_index(
        &self,
        session_key: &StreamSessionKey,
    ) -> Option<OplogIndex> {
        self.session_entity_parent_start_indices
            .get(session_key)
            .copied()
            .flatten()
    }

    pub(super) fn apply_session_attribution(
        &mut self,
        session_key: &StreamSessionKey,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Result<(), StreamStoreError> {
        match self.session_entity_parent_start_indices.get(session_key) {
            Some(existing) if *existing != entity_parent_start_index => {
                Err(StreamStoreError::CorruptHistory(
                    "durable stream session contains conflicting entity attribution".to_string(),
                ))
            }
            Some(_) => Ok(()),
            None => {
                self.session_entity_parent_start_indices
                    .insert(session_key.clone(), entity_parent_start_index);
                Ok(())
            }
        }
    }

    pub(super) fn ensure_producer_write_allowed(&self) -> Result<(), StreamStoreError> {
        if self.deleting {
            Err(StreamStoreError::ProducerDeleting)
        } else {
            Ok(())
        }
    }

    pub(super) fn apply_deletion_record(
        &mut self,
        record: &StreamSessionRecord,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<(), StreamStoreError> {
        match record {
            StreamSessionRecord::ProducerDeleting(record) => {
                if record.producer_environment_id != environment_id
                    || record.producer != *producer
                    || record.producer_fingerprint != producer_fingerprint
                {
                    return Err(StreamStoreError::CorruptHistory(
                        "durable stream deletion barrier identifies another producer incarnation"
                            .to_string(),
                    ));
                }
                self.deleting = true;
            }
            StreamSessionRecord::ConsumerDeleting(record) => {
                if record.consumer_environment_id != environment_id
                    || record.consumer != *producer
                    || record.consumer_fingerprint != producer_fingerprint
                {
                    return Err(StreamStoreError::CorruptHistory(
                        "durable stream consumer deletion intent identifies another consumer incarnation"
                            .to_string(),
                    ));
                }
                self.consumer_deleting = true;
            }
            StreamSessionRecord::CascadeOutbox(record) => {
                self.validate_attachment_key(
                    &record.key,
                    environment_id,
                    producer,
                    producer_fingerprint,
                )?;
                match self.cascade_outbox.get(&record.key) {
                    Some(existing) if existing != &record.result => {
                        return Err(StreamStoreError::CorruptHistory(
                            "conflicting durable stream cascade outbox result".to_string(),
                        ));
                    }
                    Some(_) => {}
                    None => {
                        self.cascade_outbox
                            .insert(record.key.clone(), record.result.clone());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn apply_result_offset(&mut self, index: OplogIndex, record: &StreamSessionRecord) {
        if let StreamSessionRecord::InvocationResult(record) = record {
            self.invocation_results
                .entry(record.session_key.clone())
                .or_insert(index);
        }
    }

    pub(super) fn apply_session_references(
        &mut self,
        entity_parent_start_index: Option<OplogIndex>,
        record: &StreamSessionRecord,
    ) -> Result<(), StreamStoreError> {
        if self.consumer_deleting
            && matches!(
                record,
                StreamSessionRecord::TopologyPrepared(_)
                    | StreamSessionRecord::TopologyActivated(_)
            )
        {
            return Err(StreamStoreError::ConsumerDeleting);
        }
        if let Some(session_key) = crate::worker::stream_session_record_key(record) {
            self.apply_session_attribution(session_key, entity_parent_start_index)?;
        }
        self.apply_consumer_journal_record(record)?;
        let (session_key, mappings): (&StreamSessionKey, &[StreamSessionMappingRecord]) =
            match record {
                StreamSessionRecord::Mapping(record) => {
                    (&record.session_key, std::slice::from_ref(&record.mapping))
                }
                StreamSessionRecord::InvocationResult(record) => {
                    (&record.session_key, &record.stream_mappings)
                }
                StreamSessionRecord::Prepared(record) => {
                    (&record.attempt.session_key, &record.stream_mappings)
                }
                _ => return Ok(()),
            };
        if matches!(
            record,
            StreamSessionRecord::Prepared(_) | StreamSessionRecord::InvocationResult(_)
        ) && mappings.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE
        {
            return Err(StreamStoreError::ValueStreamLimit);
        }
        let existing_mappings = self.session_stream_mappings.get(session_key);
        let mut new_mappings = HashSet::new();
        for mapping in mappings {
            if mapping.handle.format_version != DURABLE_STREAM_FORMAT_VERSION {
                return Err(StreamStoreError::InvalidHandle);
            }
            if let Some((existing, _)) = self.referenced_handles.get(&mapping.handle.stream_id)
                && existing != &mapping.handle
            {
                return Err(StreamStoreError::CorruptHistory(
                    "durable stream mapping relabels an existing stream handle".to_string(),
                ));
            }
            let identity = (mapping.handle.clone(), mapping.role);
            if existing_mappings.is_none_or(|existing| !existing.contains(&identity)) {
                new_mappings.insert(identity);
            }
        }
        let current = self
            .session_stream_counts
            .get(session_key)
            .copied()
            .unwrap_or_default();
        if current
            .checked_add(new_mappings.len())
            .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
        {
            return Err(StreamStoreError::StreamLimit);
        }
        let new_mapping_count = new_mappings.len();
        for mapping in mappings {
            match self.referenced_handles.get_mut(&mapping.handle.stream_id) {
                Some((existing, _)) if existing != &mapping.handle => {
                    unreachable!("stream handle conflict was validated before index mutation")
                }
                Some((_, sessions)) => {
                    sessions.insert(session_key.clone());
                }
                None => {
                    self.referenced_handles.insert(
                        mapping.handle.stream_id,
                        (mapping.handle.clone(), HashSet::from([session_key.clone()])),
                    );
                }
            }
        }
        self.session_stream_mappings
            .entry(session_key.clone())
            .or_default()
            .extend(new_mappings);
        self.session_stream_counts.insert(
            session_key.clone(),
            current
                .checked_add(new_mapping_count)
                .expect("validated durable session stream count cannot overflow"),
        );
        Ok(())
    }

    pub(super) fn apply_consumer_journal_record(
        &mut self,
        record: &StreamSessionRecord,
    ) -> Result<(), StreamStoreError> {
        if matches!(
            record,
            StreamSessionRecord::ConsumerItemValue(_)
                | StreamSessionRecord::ConsumerTerminal(_)
                | StreamSessionRecord::SourceUnavailable(_)
        ) && !record.has_supported_format()
        {
            return Err(StreamStoreError::CorruptHistory(
                "unsupported or malformed durable consumer journal record".to_string(),
            ));
        }
        let (session_key, stream_id, ordinal, item_count, terminal) = match record {
            StreamSessionRecord::ConsumerItemValue(record) => (
                &record.session_key,
                record.stream_id,
                record.consumer_read_ordinal,
                record.logical_item_count(),
                false,
            ),
            StreamSessionRecord::ConsumerTerminal(record) => (
                &record.session_key,
                record.stream_id,
                record.consumer_read_ordinal,
                1,
                true,
            ),
            StreamSessionRecord::SourceUnavailable(record) => {
                let journal = self
                    .consumer_journals
                    .get(&(record.key.session_key.clone(), record.key.stream_id));
                if let Some(journal) = journal
                    && let Some((existing_key, existing_offset)) = &journal.source_unavailable
                {
                    return if existing_key == &record.key
                        && existing_offset == &record.source_offset
                        && record.consumer_read_ordinal == journal.next_read_ordinal
                    {
                        Ok(())
                    } else {
                        Err(StreamStoreError::AttachmentConflict)
                    };
                }
                if journal.is_some_and(|journal| journal.terminal)
                    || record.consumer_read_ordinal
                        != journal.map_or(0, |journal| journal.next_read_ordinal)
                {
                    return Err(StreamStoreError::ConsumerJournalAdvanced);
                }
                let journal = self
                    .consumer_journals
                    .entry((record.key.session_key.clone(), record.key.stream_id))
                    .or_default();
                journal.source_unavailable = Some((record.key.clone(), record.source_offset));
                self.session_consumer_streams
                    .entry(record.key.session_key.clone())
                    .or_default()
                    .insert(record.key.stream_id);
                return Ok(());
            }
            _ => return Ok(()),
        };
        let key = (session_key.clone(), stream_id);
        let journal = self.consumer_journals.get(&key);
        if journal.is_some_and(|journal| journal.terminal || journal.source_unavailable.is_some())
            || ordinal != journal.map_or(0, |journal| journal.next_read_ordinal)
        {
            return Err(StreamStoreError::ConsumerJournalAdvanced);
        }
        let next_read_ordinal = ordinal
            .checked_add(u64::try_from(item_count).map_err(|_| StreamStoreError::CounterOverflow)?)
            .ok_or(StreamStoreError::CounterOverflow)?;
        let last_source_offset = match record {
            StreamSessionRecord::ConsumerItemValue(record) => record
                .logical_item_count()
                .checked_sub(1)
                .and_then(|index| record.source_offset_at(index))
                .ok_or_else(|| {
                    StreamStoreError::CorruptHistory(
                        "consumer journal source offset range is empty or invalid".to_string(),
                    )
                })?,
            StreamSessionRecord::ConsumerTerminal(record) => record.source_offset,
            _ => unreachable!(),
        };
        let journal = self.consumer_journals.entry(key).or_default();
        journal.next_read_ordinal = next_read_ordinal;
        journal.last_source_offset = Some(last_source_offset);
        journal.terminal = terminal;
        self.session_consumer_streams
            .entry(session_key.clone())
            .or_default()
            .insert(stream_id);
        Ok(())
    }

    pub(super) fn registration_session_key(
        &self,
        coordinate: &StreamRegistrationCoordinate,
        mapping: &Option<StreamSessionMapping>,
    ) -> Option<StreamSessionKey> {
        if let Some(mapping) = mapping {
            return Some(mapping.session_key.clone());
        }
        match coordinate {
            StreamRegistrationCoordinate::Root { invocation_id, .. } => Some(invocation_id.clone()),
            StreamRegistrationCoordinate::Nested {
                parent_stream_id, ..
            } => self.stream_sessions.get(parent_stream_id).cloned(),
        }
    }

    pub(super) fn apply_registration(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamRegisteredRecord,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<(), StreamStoreError> {
        validate_version(record.format_version)?;
        if registration_coordinate_depth(&record.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH {
            return Err(StreamStoreError::CorruptHistory(
                "stream registration coordinate exceeds the traversal-depth limit".to_string(),
            ));
        }
        if record.registration_oplog_index != oplog_index
            || record.handle.format_version != DURABLE_STREAM_FORMAT_VERSION
            || record.handle.stream_id
                != StreamId::derive(environment_id, producer, producer_fingerprint, oplog_index)
                    .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?
            || record.handle.producer_environment_id != environment_id
            || record.handle.producer != *producer
            || record.handle.expected_producer_fingerprint != producer_fingerprint
        {
            return Err(StreamStoreError::InvalidHandle);
        }
        if let Some(existing_id) = self.coordinates.get(&record.coordinate) {
            let existing = self
                .registrations
                .get(existing_id)
                .expect("coordinate index points at a missing stream registration");
            let existing_entity_parent_start_index = self
                .entity_parent_start_indices
                .get(existing_id)
                .copied()
                .flatten();
            return if existing == &record
                && existing_entity_parent_start_index == entity_parent_start_index
            {
                Ok(())
            } else {
                Err(StreamStoreError::RegistrationDivergence)
            };
        }
        if self.registrations.contains_key(&record.handle.stream_id) {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        let session_key = self
            .registration_session_key(&record.coordinate, &record.session_mapping)
            .ok_or_else(|| {
                StreamStoreError::CorruptHistory(
                    "nested stream registration references an unknown parent stream".to_string(),
                )
            })?;
        if let StreamRegistrationCoordinate::Nested {
            parent_stream_id, ..
        } = &record.coordinate
            && self
                .entity_parent_start_indices
                .get(parent_stream_id)
                .copied()
                .flatten()
                != entity_parent_start_index
        {
            return Err(StreamStoreError::CorruptHistory(
                "nested stream attribution differs from its parent stream".to_string(),
            ));
        }
        if self.finished_sessions.contains(&session_key) {
            return Err(StreamStoreError::CorruptHistory(
                "stream registration follows its session Finished record".to_string(),
            ));
        }
        let role = match (&record.coordinate, &record.session_mapping) {
            (_, Some(mapping)) => mapping.role,
            (
                StreamRegistrationCoordinate::Nested {
                    parent_stream_id, ..
                },
                None,
            ) => *self.stream_roles.get(parent_stream_id).ok_or_else(|| {
                StreamStoreError::CorruptHistory(
                    "nested stream registration references a parent without a session role"
                        .to_string(),
                )
            })?,
            (
                StreamRegistrationCoordinate::Root {
                    root_kind: golem_common::base_model::durable_stream::StreamRootKind::MethodInput,
                    ..
                },
                None,
            ) => SessionStreamRole::Input,
            (
                StreamRegistrationCoordinate::Root {
                    root_kind:
                        golem_common::base_model::durable_stream::StreamRootKind::MethodResult,
                    ..
                },
                None,
            ) => SessionStreamRole::Output,
        };
        let mapping_identity = (record.handle.clone(), role);
        let new_session_mapping = self
            .session_stream_mappings
            .get(&session_key)
            .is_none_or(|mappings| !mappings.contains(&mapping_identity));
        if new_session_mapping
            && self
                .session_stream_counts
                .get(&session_key)
                .copied()
                .unwrap_or_default()
                >= MAX_DURABLE_STREAMS_PER_SESSION
        {
            return Err(StreamStoreError::StreamLimit);
        }
        self.apply_session_attribution(&session_key, entity_parent_start_index)?;
        self.coordinates
            .insert(record.coordinate.clone(), record.handle.stream_id);
        self.entity_parent_start_indices
            .insert(record.handle.stream_id, entity_parent_start_index);
        self.streams
            .insert(record.handle.stream_id, IndexedProducerStream::default());
        self.stream_sessions
            .insert(record.handle.stream_id, session_key.clone());
        self.stream_roles.insert(record.handle.stream_id, role);
        self.open_session_streams
            .entry(session_key.clone())
            .or_default()
            .insert(record.handle.stream_id);
        if new_session_mapping {
            self.session_stream_mappings
                .entry(session_key.clone())
                .or_default()
                .insert(mapping_identity);
            *self.session_stream_counts.entry(session_key).or_default() += 1;
        }
        self.registrations.insert(record.handle.stream_id, record);
        self.open_streams += 1;
        Ok(())
    }

    pub(super) fn apply_item_batch(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        pending_registrations: Vec<(OplogIndex, Option<OplogIndex>, StreamRegisteredRecord)>,
        record: StreamItemsRecord,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        if pending_registrations.len() != record.newly_registered_stream_ids.len() {
            return Err(StreamStoreError::CorruptHistory(
                "nested registrations are not claimed by their enclosing item batch".to_string(),
            ));
        }
        if pending_registrations.is_empty() {
            return self.apply_items(
                oplog_index,
                entity_parent_start_index,
                record,
                producer_fingerprint,
            );
        }
        let registration_start = oplog_index
            .as_u64()
            .checked_sub(pending_registrations.len() as u64)
            .ok_or_else(|| {
                StreamStoreError::CorruptHistory(
                    "nested registrations precede the beginning of the oplog".to_string(),
                )
            })?;
        let logical_item_count = record.payload.logical_item_count() as u64;
        for (position, ((registration_index, _, registration), expected_stream_id)) in
            pending_registrations
                .iter()
                .zip(&record.newly_registered_stream_ids)
                .enumerate()
        {
            let expected_index = OplogIndex::from_u64(registration_start + position as u64);
            if *registration_index != expected_index
                || registration.handle.stream_id != *expected_stream_id
                || !nested_coordinate_matches_item(
                    &registration.coordinate,
                    record.stream_id,
                    record.first_sequence,
                    logical_item_count,
                )
            {
                return Err(StreamStoreError::CorruptHistory(
                    "nested registration batch does not match its enclosing stream item"
                        .to_string(),
                ));
            }
        }
        let mut updated = self.clone();
        for (registration_index, entity_parent_start_index, registration) in pending_registrations {
            updated.apply_registration(
                registration_index,
                entity_parent_start_index,
                registration,
                environment_id,
                producer,
                producer_fingerprint,
            )?;
        }
        let events = updated.apply_items(
            oplog_index,
            entity_parent_start_index,
            record,
            producer_fingerprint,
        )?;
        *self = updated;
        Ok(events)
    }

    pub(super) fn apply_items(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamItemsRecord,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint {
            return Err(StreamStoreError::InvalidHandle);
        }
        if self.entity_parent_start_index(record.stream_id)? != entity_parent_start_index {
            return Err(StreamStoreError::CorruptHistory(
                "stream item attribution differs from its registration".to_string(),
            ));
        }
        validate_items_payload(&record.payload)?;
        let session_key = self
            .stream_sessions
            .get(&record.stream_id)
            .ok_or(StreamStoreError::UnknownStream(record.stream_id))?;
        if self.finished_sessions.contains(session_key) {
            return Err(StreamStoreError::CorruptHistory(
                "stream item follows its session Finished record".to_string(),
            ));
        }
        let logical_item_count = record.payload.logical_item_count() as u64;
        if matches!(record.payload, StreamItemsPayload::PackedU8(_))
            && !record.nested_stream_ids.is_empty()
        {
            return Err(StreamStoreError::CorruptHistory(
                "packed-u8 stream items cannot contain nested streams".to_string(),
            ));
        }
        for nested_stream_id in &record.nested_stream_ids {
            if let Some(registration) = self.registrations.get(nested_stream_id) {
                if !nested_coordinate_matches_item(
                    &registration.coordinate,
                    record.stream_id,
                    record.first_sequence,
                    logical_item_count,
                ) && self.referenced_handles.get(nested_stream_id).is_none_or(
                    |(_, referenced_sessions)| !referenced_sessions.contains(session_key),
                ) {
                    return Err(StreamStoreError::CorruptHistory(
                        "nested stream registration does not identify its enclosing stream item"
                            .to_string(),
                    ));
                }
            } else if self
                .referenced_handles
                .get(nested_stream_id)
                .is_none_or(|(_, referenced_sessions)| !referenced_sessions.contains(session_key))
            {
                return Err(StreamStoreError::CorruptHistory(
                    "stream item batch references an unknown durable stream handle".to_string(),
                ));
            }
        }
        let nested_handles = record
            .nested_stream_ids
            .iter()
            .map(|stream_id| {
                self.registrations
                    .get(stream_id)
                    .map(|registration| registration.handle.clone())
                    .or_else(|| {
                        self.referenced_handles
                            .get(stream_id)
                            .map(|(handle, _)| handle.clone())
                    })
                    .expect("validated nested durable stream handle is missing")
            })
            .collect::<Vec<_>>();
        let registration_start = oplog_index
            .as_u64()
            .checked_sub(record.newly_registered_stream_ids.len() as u64)
            .ok_or_else(|| {
                StreamStoreError::CorruptHistory(
                    "nested registrations precede the beginning of the oplog".to_string(),
                )
            })?;
        let nested_ids: HashSet<_> = record.nested_stream_ids.iter().copied().collect();
        if nested_ids.len() != record.nested_stream_ids.len() {
            return Err(StreamStoreError::CorruptHistory(
                "stream item batch contains duplicate nested stream ownership".to_string(),
            ));
        }
        let mut newly_registered_ids =
            HashSet::with_capacity(record.newly_registered_stream_ids.len());
        for (position, stream_id) in record.newly_registered_stream_ids.iter().enumerate() {
            if !newly_registered_ids.insert(*stream_id) || !nested_ids.contains(stream_id) {
                return Err(StreamStoreError::CorruptHistory(
                    "stream item batch has an invalid newly registered stream list".to_string(),
                ));
            }
            let expected_index = OplogIndex::from_u64(registration_start + position as u64);
            if self
                .registrations
                .get(stream_id)
                .is_none_or(|registration| registration.registration_oplog_index != expected_index)
            {
                return Err(StreamStoreError::CorruptHistory(
                    "stream item batch does not follow its declared nested registrations"
                        .to_string(),
                ));
            }
        }
        let stream = self
            .streams
            .get_mut(&record.stream_id)
            .ok_or(StreamStoreError::UnknownStream(record.stream_id))?;
        if stream.terminal {
            return Err(StreamStoreError::AlreadyTerminal(record.stream_id));
        }
        if record.first_sequence != stream.next_sequence {
            return Err(StreamStoreError::SequenceGap {
                expected: stream.next_sequence,
                actual: record.first_sequence,
            });
        }
        let payloads = logical_payloads(&record.payload);
        if payloads.len() != record.offsets.len() {
            return Err(StreamStoreError::CorruptHistory(
                "item count does not match offset count".to_string(),
            ));
        }
        let packed_u8_batch_end = if matches!(&record.payload, StreamItemsPayload::PackedU8(_)) {
            record.offsets.last().copied()
        } else {
            None
        };
        let mut events = Vec::with_capacity(payloads.len());
        for (sub_index, (payload, offset)) in payloads
            .into_iter()
            .zip(record.offsets.iter().copied())
            .enumerate()
        {
            if offset != StreamOffset::new(oplog_index, sub_index as u32) {
                return Err(StreamStoreError::CorruptHistory(
                    "stream item offset does not match its producer oplog position".to_string(),
                ));
            }
            let sequence = record
                .first_sequence
                .checked_add(sub_index as u64)
                .ok_or(StreamStoreError::CounterOverflow)?;
            let event = CommittedProducerStreamEvent {
                stream_id: record.stream_id,
                producer_sequence: sequence,
                offset,
                packed_u8_batch_end,
                terminal_author: None,
                nested_handles: nested_handles.clone(),
                payload,
            };
            events.push(event);
        }
        stream.first_sequence.get_or_insert(record.first_sequence);
        stream.next_sequence = stream
            .next_sequence
            .checked_add(events.len() as u64)
            .ok_or(StreamStoreError::CounterOverflow)?;
        stream.last_offset = record.offsets.last().copied();
        stream.last_item_offset = stream.last_offset;
        stream.batches.insert(record.first_sequence, oplog_index);
        self.batch_positions.insert(
            (record.stream_id, oplog_index),
            (record.first_sequence, events.len() as u64),
        );
        Ok(events)
    }

    pub(super) fn apply_end(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamEndRecord,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<CommittedProducerStreamEvent, StreamStoreError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint
            || record.offset != StreamOffset::new(oplog_index, 0)
        {
            return Err(StreamStoreError::InvalidHandle);
        }
        if self.entity_parent_start_index(record.stream_id)? != entity_parent_start_index {
            return Err(StreamStoreError::CorruptHistory(
                "stream end attribution differs from its registration".to_string(),
            ));
        }
        let stream = self
            .streams
            .get_mut(&record.stream_id)
            .ok_or(StreamStoreError::UnknownStream(record.stream_id))?;
        validate_terminal_sequence(stream, record.stream_id, record.sequence)?;
        let event = CommittedProducerStreamEvent {
            stream_id: record.stream_id,
            producer_sequence: record.sequence,
            offset: record.offset,
            packed_u8_batch_end: None,
            terminal_author: Some(record.authored_by),
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayload::End(record.result),
        };
        stream.first_sequence.get_or_insert(record.sequence);
        stream.last_offset = Some(record.offset);
        stream.terminal = true;
        self.open_streams -= 1;
        if let Some(session) = self.stream_sessions.get(&record.stream_id)
            && let Some(streams) = self.open_session_streams.get_mut(session)
        {
            streams.remove(&record.stream_id);
        }
        Ok(event)
    }

    pub(super) fn apply_finished(
        &mut self,
        record: &StreamSessionFinishedRecord,
    ) -> Result<(), StreamStoreError> {
        validate_version(record.format_version)?;
        if self.finished_sessions.contains(&record.session_key) {
            return Ok(());
        }
        if self
            .open_session_streams
            .get(&record.session_key)
            .is_some_and(|streams| !streams.is_empty())
        {
            return Err(StreamStoreError::CorruptHistory(
                "session Finished record precedes a materialized stream terminal".to_string(),
            ));
        }
        self.finished_sessions.insert(record.session_key.clone());
        Ok(())
    }

    pub(super) fn apply_cancel(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamCancelRecord,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<CommittedProducerStreamEvent, StreamStoreError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint
            || record.offset != StreamOffset::new(oplog_index, 0)
        {
            return Err(StreamStoreError::InvalidHandle);
        }
        if self.entity_parent_start_index(record.stream_id)? != entity_parent_start_index {
            return Err(StreamStoreError::CorruptHistory(
                "stream cancellation attribution differs from its registration".to_string(),
            ));
        }
        let stream = self
            .streams
            .get_mut(&record.stream_id)
            .ok_or(StreamStoreError::UnknownStream(record.stream_id))?;
        validate_terminal_sequence(stream, record.stream_id, record.sequence)?;
        let event = CommittedProducerStreamEvent {
            stream_id: record.stream_id,
            producer_sequence: record.sequence,
            offset: record.offset,
            packed_u8_batch_end: None,
            terminal_author: Some(record.authored_by),
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayload::Cancel {
                role: record.role,
                reason: record.reason,
                details: record.details,
            },
        };
        stream.first_sequence.get_or_insert(record.sequence);
        stream.last_offset = Some(record.offset);
        stream.terminal = true;
        self.open_streams -= 1;
        if let Some(session) = self.stream_sessions.get(&record.stream_id)
            && let Some(streams) = self.open_session_streams.get_mut(session)
        {
            streams.remove(&record.stream_id);
        }
        Ok(event)
    }

    pub(super) fn apply_attachment_record(
        &mut self,
        record: &StreamSessionRecord,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<AttachmentApplyOutcome, StreamStoreError> {
        let (key, state) = match record {
            StreamSessionRecord::AttachmentPrepared(record) => {
                validate_version(record.format_version)?;
                if record.lease_expires_at_millis <= record.prepared_at_millis {
                    return Err(StreamStoreError::AttachmentConflict);
                }
                (
                    &record.key,
                    IndexedStreamAttachmentState::Prepared {
                        prepared_at_millis: record.prepared_at_millis,
                        lease_expires_at_millis: record.lease_expires_at_millis,
                    },
                )
            }
            StreamSessionRecord::AttachmentActivated(record) => {
                validate_version(record.format_version)?;
                if record.lease_expires_at_millis <= record.activated_at_millis {
                    return Err(StreamStoreError::AttachmentConflict);
                }
                (
                    &record.key,
                    IndexedStreamAttachmentState::Active {
                        activated_at_millis: record.activated_at_millis,
                        lease_expires_at_millis: record.lease_expires_at_millis,
                    },
                )
            }
            StreamSessionRecord::AttachmentRenewed(record) => {
                validate_version(record.format_version)?;
                if record.lease_expires_at_millis <= record.renewed_at_millis {
                    return Err(StreamStoreError::AttachmentConflict);
                }
                (
                    &record.key,
                    IndexedStreamAttachmentState::Active {
                        activated_at_millis: record.renewed_at_millis,
                        lease_expires_at_millis: record.lease_expires_at_millis,
                    },
                )
            }
            StreamSessionRecord::AttachmentFinalized(record) => {
                validate_version(record.format_version)?;
                (
                    &record.key,
                    IndexedStreamAttachmentState::Finalized {
                        finalized_at_millis: record.finalized_at_millis,
                        reason: record.reason,
                    },
                )
            }
            _ => return Ok(AttachmentApplyOutcome::Replayed),
        };
        self.validate_attachment_key(key, environment_id, producer, producer_fingerprint)?;
        let slot = attachment_slot(key);
        let existing = self.attachments.get(&slot);
        let was_active = existing.is_some_and(|attachment| {
            matches!(
                attachment.state,
                IndexedStreamAttachmentState::Active { .. }
            )
        });

        if self.deleting
            && matches!(
                record,
                StreamSessionRecord::AttachmentPrepared(_)
                    | StreamSessionRecord::AttachmentActivated(_)
                    | StreamSessionRecord::AttachmentRenewed(_)
            )
        {
            return Err(StreamStoreError::ProducerDeleting);
        }

        if matches!(record, StreamSessionRecord::AttachmentPrepared(_)) {
            return match existing {
                None => {
                    self.attachments.insert(
                        slot,
                        IndexedStreamAttachment {
                            key: key.clone(),
                            state,
                        },
                    );
                    Ok(AttachmentApplyOutcome::Changed)
                }
                Some(existing) => {
                    if key.epoch > existing.key.epoch
                        && attachment_identity_matches_except_epoch(&existing.key, key)
                    {
                        if was_active {
                            let count = self
                                .active_attachments_by_session_stream
                                .entry((key.session_key.clone(), key.stream_id))
                                .or_default();
                            *count = count.checked_sub(1).ok_or_else(|| {
                                StreamStoreError::CorruptHistory(
                                    "active attachment session count underflow".into(),
                                )
                            })?;
                        }
                        self.attachments.insert(
                            slot,
                            IndexedStreamAttachment {
                                key: key.clone(),
                                state,
                            },
                        );
                        Ok(AttachmentApplyOutcome::Changed)
                    } else {
                        validate_attachment_epoch(existing, key)?;
                        match existing.state {
                            IndexedStreamAttachmentState::Prepared { .. }
                            | IndexedStreamAttachmentState::Active { .. }
                                if existing.key == *key =>
                            {
                                Ok(AttachmentApplyOutcome::Replayed)
                            }
                            _ => Err(StreamStoreError::InvalidAttachmentState),
                        }
                    }
                }
            };
        }

        let existing = existing.ok_or(StreamStoreError::InvalidAttachmentState)?;
        validate_attachment_epoch(existing, key)?;
        if existing.key != *key {
            return Err(StreamStoreError::AttachmentConflict);
        }

        let outcome = match record {
            StreamSessionRecord::AttachmentActivated(_) => match existing.state {
                IndexedStreamAttachmentState::Prepared { .. } => AttachmentApplyOutcome::Changed,
                IndexedStreamAttachmentState::Active { .. } => AttachmentApplyOutcome::Replayed,
                IndexedStreamAttachmentState::Finalized { .. } => {
                    return Err(StreamStoreError::InvalidAttachmentState);
                }
            },
            StreamSessionRecord::AttachmentRenewed(record) => match existing.state {
                IndexedStreamAttachmentState::Active {
                    lease_expires_at_millis,
                    ..
                } if record.lease_expires_at_millis > lease_expires_at_millis => {
                    AttachmentApplyOutcome::Changed
                }
                IndexedStreamAttachmentState::Active { .. } => AttachmentApplyOutcome::Replayed,
                _ => return Err(StreamStoreError::InvalidAttachmentState),
            },
            StreamSessionRecord::AttachmentFinalized(_) => match existing.state {
                IndexedStreamAttachmentState::Prepared { .. }
                | IndexedStreamAttachmentState::Active { .. } => AttachmentApplyOutcome::Changed,
                IndexedStreamAttachmentState::Finalized { .. } => AttachmentApplyOutcome::Replayed,
            },
            _ => unreachable!("non-attachment records returned before state application"),
        };
        if outcome == AttachmentApplyOutcome::Changed {
            let is_active = matches!(state, IndexedStreamAttachmentState::Active { .. });
            if was_active != is_active {
                let count_key = (key.session_key.clone(), key.stream_id);
                let count = self
                    .active_attachments_by_session_stream
                    .entry(count_key)
                    .or_default();
                if is_active {
                    *count = count
                        .checked_add(1)
                        .ok_or(StreamStoreError::CounterOverflow)?;
                } else {
                    *count = count.checked_sub(1).ok_or_else(|| {
                        StreamStoreError::CorruptHistory(
                            "active attachment session count underflow".into(),
                        )
                    })?;
                }
            }
            self.attachments.insert(
                slot,
                IndexedStreamAttachment {
                    key: key.clone(),
                    state,
                },
            );
        }
        Ok(outcome)
    }

    pub(super) fn validate_attachment_key(
        &self,
        key: &StreamAttachmentKey,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<(), StreamStoreError> {
        let registration = self
            .registrations
            .get(&key.stream_id)
            .ok_or(StreamStoreError::UnknownStream(key.stream_id))?;
        if key.producer_environment_id != environment_id
            || key.producer != *producer
            || key.expected_producer_fingerprint != producer_fingerprint
            || registration.handle.producer_environment_id != key.producer_environment_id
            || registration.handle.producer != key.producer
            || registration.handle.expected_producer_fingerprint
                != key.expected_producer_fingerprint
            || key.consumer_invocation.callee_environment_id != key.consumer_environment_id
            || key.consumer_invocation.callee != key.consumer
            || key.consumer_invocation.callee_fingerprint != key.expected_consumer_fingerprint
            || AttachmentId::primary(
                key.session_key.callee_environment_id,
                &key.session_key.callee,
                &key.session_key.idempotency_key,
            )
            .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?
                != key.attachment_id
        {
            return Err(StreamStoreError::InvalidHandle);
        }
        Ok(())
    }

    pub(super) fn attachment_views(&self) -> Vec<StreamAttachmentView> {
        let mut views = self
            .attachments
            .values()
            .map(|attachment| {
                let (state, lease_expires_at_millis) = match attachment.state {
                    IndexedStreamAttachmentState::Prepared {
                        lease_expires_at_millis,
                        ..
                    } => (
                        StreamAttachmentState::Prepared,
                        Some(lease_expires_at_millis),
                    ),
                    IndexedStreamAttachmentState::Active {
                        lease_expires_at_millis,
                        ..
                    } => (StreamAttachmentState::Active, Some(lease_expires_at_millis)),
                    IndexedStreamAttachmentState::Finalized { reason, .. } => {
                        (StreamAttachmentState::Finalized(reason), None)
                    }
                };
                StreamAttachmentView {
                    key: attachment.key.clone(),
                    state,
                    lease_expires_at_millis,
                }
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| {
            attachment_sort_key(&left.key).cmp(&attachment_sort_key(&right.key))
        });
        views
    }

    pub(super) fn live_dependents(&self) -> Vec<StreamAttachmentKey> {
        let mut dependents = self
            .attachments
            .values()
            .filter_map(|attachment| {
                (!matches!(
                    attachment.state,
                    IndexedStreamAttachmentState::Finalized { .. }
                ))
                .then_some(attachment.key.clone())
            })
            .collect::<Vec<_>>();
        dependents
            .sort_by(|left, right| attachment_sort_key(left).cmp(&attachment_sort_key(right)));
        dependents
    }

    pub(super) fn incomplete_cascade_dependents(&self) -> Vec<StreamAttachmentKey> {
        self.live_dependents()
            .into_iter()
            .filter(|key| !self.cascade_outbox.contains_key(key))
            .collect()
    }
}

pub(super) fn attachment_slot(
    key: &StreamAttachmentKey,
) -> (AttachmentId, StreamId, EnvironmentId, AgentId) {
    (
        key.attachment_id,
        key.stream_id,
        key.consumer_environment_id,
        key.consumer.clone(),
    )
}

pub(super) fn attachment_sort_key(
    key: &StreamAttachmentKey,
) -> (StreamId, AttachmentId, EnvironmentId, &AgentId, u64) {
    (
        key.stream_id,
        key.attachment_id,
        key.consumer_environment_id,
        &key.consumer,
        key.epoch,
    )
}

pub(super) fn validate_attachment_epoch(
    existing: &IndexedStreamAttachment,
    requested: &StreamAttachmentKey,
) -> Result<(), StreamStoreError> {
    if requested.epoch < existing.key.epoch {
        Err(StreamStoreError::StaleEpoch {
            current: existing.key.epoch,
            actual: requested.epoch,
        })
    } else if requested.epoch > existing.key.epoch {
        Err(StreamStoreError::InvalidEpoch {
            current: existing.key.epoch,
            actual: requested.epoch,
        })
    } else {
        Ok(())
    }
}

fn attachment_identity_matches_except_epoch(
    existing: &StreamAttachmentKey,
    requested: &StreamAttachmentKey,
) -> bool {
    existing.attachment_id == requested.attachment_id
        && existing.stream_id == requested.stream_id
        && existing.session_key == requested.session_key
        && existing.producer_environment_id == requested.producer_environment_id
        && existing.producer == requested.producer
        && existing.expected_producer_fingerprint == requested.expected_producer_fingerprint
        && existing.consumer_environment_id == requested.consumer_environment_id
        && existing.consumer == requested.consumer
        && existing.expected_consumer_fingerprint == requested.expected_consumer_fingerprint
        && existing.consumer_invocation == requested.consumer_invocation
}

pub(super) fn validate_terminal_sequence(
    stream: &IndexedProducerStream,
    stream_id: StreamId,
    sequence: u64,
) -> Result<(), StreamStoreError> {
    if stream.terminal {
        return Err(StreamStoreError::AlreadyTerminal(stream_id));
    }
    if sequence != stream.next_sequence {
        return Err(StreamStoreError::SequenceGap {
            expected: stream.next_sequence,
            actual: sequence,
        });
    }
    Ok(())
}

pub(super) fn validate_version(version: u8) -> Result<(), StreamStoreError> {
    if version == DURABLE_STREAM_FORMAT_VERSION {
        Ok(())
    } else {
        Err(StreamStoreError::UnsupportedVersion(version))
    }
}

pub(super) fn validate_items_payload(payload: &StreamItemsPayload) -> Result<(), StreamStoreError> {
    match payload {
        StreamItemsPayload::Values(values) if values.len() != 1 => {
            crate::metrics::durable_stream::record_limit_violation("value_batch_items");
            Err(StreamStoreError::InvalidValueBatch)
        }
        StreamItemsPayload::Values(values)
            if values
                .first()
                .is_some_and(|value| value.len() > MAX_DURABLE_STREAM_ITEM_SIZE) =>
        {
            crate::metrics::durable_stream::record_limit_violation("item_size");
            Err(StreamStoreError::ItemTooLarge)
        }
        StreamItemsPayload::PackedU8(bytes)
            if bytes.is_empty() || bytes.len() > MAX_PACKED_U8_STREAM_ITEM_SIZE =>
        {
            crate::metrics::durable_stream::record_limit_violation("packed_u8_batch_size");
            Err(StreamStoreError::InvalidPackedU8Batch)
        }
        _ => Ok(()),
    }
}

pub(super) fn logical_payloads(
    payload: &StreamItemsPayload,
) -> impl ExactSizeIterator<Item = CommittedProducerStreamEventPayload> + '_ {
    (0..payload.logical_item_count()).map(|index| match payload {
        StreamItemsPayload::Values(values) => {
            CommittedProducerStreamEventPayload::Value(values[index].clone())
        }
        StreamItemsPayload::PackedU8(bytes) => {
            CommittedProducerStreamEventPayload::PackedU8(bytes[index])
        }
    })
}

pub(super) fn registration_coordinate_depth(coordinate: &StreamRegistrationCoordinate) -> usize {
    match coordinate {
        StreamRegistrationCoordinate::Root {
            recursive_value_path,
            ..
        }
        | StreamRegistrationCoordinate::Nested {
            recursive_value_path,
            ..
        } => recursive_value_path.len(),
    }
}

pub(super) fn nested_coordinate_matches_item(
    coordinate: &StreamRegistrationCoordinate,
    stream_id: StreamId,
    first_sequence: u64,
    logical_item_count: u64,
) -> bool {
    matches!(
        coordinate,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id,
            parent_producer_sequence,
            ..
        } if *parent_stream_id == stream_id
            && parent_producer_sequence
                .checked_sub(first_sequence)
                .is_some_and(|relative_sequence| relative_sequence < logical_item_count)
    )
}

pub(super) fn resource_exhausted_error_context() -> Result<Vec<u8>, StreamStoreError> {
    let error = AgentError::CustomError(TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::string()),
        SchemaValue::String("ResourceExhausted".to_string()),
    ));
    golem_common::serialization::serialize(&error).map_err(StreamStoreError::Oplog)
}
