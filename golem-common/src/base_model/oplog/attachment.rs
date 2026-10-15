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

use crate::base_model::durable_stream::StreamConsumerObservation;
use crate::base_model::regions::DeletedRegions;
use crate::base_model::{IdempotencyKey, OplogIndex};
use golem_schema_derive::{FromSchema, IntoSchema};
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included};

/// Owner-relative identity. Explicit constructors own their Start; implicit
/// endpoints use the entity Start and a fixed slot. Moving an endpoint preserves it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolAttachmentId {
    pub creation_start: OplogIndex,
    pub slot: ToolAttachmentSlot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolAttachmentSlot {
    Stdin,
    Stdout,
    Stderr,
    Output,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ByteStreamFailure {
    Cancelled,
    Abandoned,
    ResourceExhausted,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ByteStreamCloseCause {
    Finished,
    Failed(ByteStreamFailure),
    ConsumerCancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamWriteError {
    Closed(ByteStreamCloseCause),
    ConcurrentOperation,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolAttachmentTerminal {
    Application(ByteStreamCloseCause),
    /// Resource exhaustion selected by the host, not a guest fail request.
    HostResourceExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AcceptedAttachmentWriteEnd(pub OplogIndex);

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolAttachmentReadOutcome {
    TypedChunk(AcceptedAttachmentWriteEnd),
    RawBytes {
        end: AcceptedAttachmentWriteEnd,
        offset: u64,
        length: u64,
    },
    Eof {
        terminal_index: OplogIndex,
    },
    Error {
        terminal_index: OplogIndex,
        failure: ByteStreamFailure,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolAttachmentReadObservation {
    pub attachment: ToolAttachmentId,
    /// The invocation that introduced the consumer binding, retained across method calls.
    pub logical_invocation: IdempotencyKey,
    pub observation: StreamConsumerObservation,
    pub ordinal: u64,
    /// Native read attempt at this physical boundary. Zero cancellations leave gaps.
    /// A re-observation can have a different attempt while retaining the logical value ordinal.
    pub read_attempt_ordinal: u64,
    pub outcome: ToolAttachmentReadOutcome,
    pub reobserves: Option<OplogIndex>,
}

impl ToolAttachmentReadObservation {
    /// Checks payload ownership without fetching an external body or source write.
    pub fn validate_payload_presence(
        &self,
        present: bool,
    ) -> Result<(), ToolAttachmentHistoryError> {
        let owns_payload = self.reobserves.is_none()
            && matches!(
                self.outcome,
                ToolAttachmentReadOutcome::TypedChunk(_)
                    | ToolAttachmentReadOutcome::RawBytes { .. }
            );
        if owns_payload == present {
            Ok(())
        } else {
            Err(ToolAttachmentHistoryError::InvalidEffect)
        }
    }
}

/// Only effective semantic transitions are recorded. Executor teardown is not
/// a transition and must not select a terminal or discard unread bytes.
#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolAttachmentControl {
    TerminalSelected(ToolAttachmentTerminal),
    ReaderDiscarded,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ToolAttachmentRecord {
    Read(ToolAttachmentReadObservation),
    Control {
        attachment: ToolAttachmentId,
        transition: ToolAttachmentControl,
    },
}

/// Metadata only: producer Ends contain no byte payloads. Non-accepted Ends
/// have no accepted length. Retention concerns acceptance, not acknowledgment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolAttachmentWriteMetadata {
    pub attachment: ToolAttachmentId,
    pub start: OplogIndex,
    pub accepted_length: Option<u64>,
    pub retained: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolAttachmentControlMetadata {
    pub attachment: ToolAttachmentId,
    /// Present for an effect carried by a producer End, absent for a standalone control.
    pub producer_start: Option<OplogIndex>,
    /// The entity scope owning a standalone control, not a producer host-call Start.
    pub entity_parent_start: Option<OplogIndex>,
    pub transition: ToolAttachmentControl,
    pub retained: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolAttachmentHistoryError {
    InvalidChronology,
    MissingAcceptance,
    WrongAttachment,
    NotAccepted,
    RemovedAcceptance,
    InvalidRange,
    MissingObservation,
    DifferentObservation,
    DuplicateObservation,
    InvalidEffect,
}

/// A disposable metadata index, including removed observations for logical
/// re-observation checks. Removing an observation never removes its acceptance.
#[derive(Clone, Debug, Default)]
pub struct ToolAttachmentHistory {
    pub writes: BTreeMap<OplogIndex, ToolAttachmentWriteMetadata>,
    pub observations: BTreeMap<OplogIndex, ToolAttachmentReadObservation>,
    /// Effective transitions, from producer Ends or explicit control records.
    pub controls: BTreeMap<OplogIndex, ToolAttachmentControlMetadata>,
}

impl ToolAttachmentHistory {
    /// A removed source does not constrain the replacement branch's logical input.
    /// Missing metadata is not proof of removal and remains a validation error.
    pub fn source_was_removed(&self, read: &ToolAttachmentReadObservation) -> bool {
        match read.outcome {
            ToolAttachmentReadOutcome::TypedChunk(end)
            | ToolAttachmentReadOutcome::RawBytes { end, .. } => {
                self.writes.get(&end.0).is_some_and(|write| !write.retained)
            }
            ToolAttachmentReadOutcome::Eof { terminal_index }
            | ToolAttachmentReadOutcome::Error { terminal_index, .. } => self
                .controls
                .get(&terminal_index)
                .is_some_and(|control| !control.retained),
        }
    }

    /// Applies the owner's authoritative skipped regions to the derived index.
    /// Removed observations remain available for logical re-observation checks.
    pub fn retain_history(&mut self, deleted: &DeletedRegions) {
        for (end, write) in &mut self.writes {
            write.retained = !deleted.is_in_deleted_region(*end)
                && !deleted.is_in_deleted_region(write.start)
                && !deleted.is_in_deleted_region(write.attachment.creation_start);
        }
        for (index, control) in &mut self.controls {
            control.retained = !deleted.is_in_deleted_region(*index)
                && !control
                    .producer_start
                    .is_some_and(|start| deleted.is_in_deleted_region(start))
                && !control
                    .entity_parent_start
                    .is_some_and(|start| deleted.is_in_deleted_region(start))
                && !deleted.is_in_deleted_region(control.attachment.creation_start);
        }
    }

    /// Validates the physical observation set once for a recovery horizon.
    pub fn validate_observations(
        &self,
        deleted: &DeletedRegions,
    ) -> Result<(), ToolAttachmentHistoryError> {
        let mut boundaries = BTreeMap::new();
        let mut originals = BTreeMap::new();
        for (index, read) in &self.observations {
            if deleted.is_in_deleted_region(*index) && self.source_was_removed(read) {
                continue;
            }
            let key = (
                read.attachment,
                read.logical_invocation.value.as_str(),
                read.ordinal,
            );
            if read.reobserves.is_none() && originals.insert(key, index).is_some() {
                return Err(ToolAttachmentHistoryError::DuplicateObservation);
            }
            self.validate_read(*index, read)?;
            if deleted.is_in_deleted_region(*index) {
                continue;
            }
            if boundaries.insert(key, index).is_some() {
                return Err(ToolAttachmentHistoryError::DuplicateObservation);
            }
        }
        Ok(())
    }

    /// Validates resolved consumer bytes without storing them in the metadata index.
    /// An accepted producer End supplies the length, never the body.
    pub fn validate_read_payload(
        &self,
        index: OplogIndex,
        read: &ToolAttachmentReadObservation,
        payload: Option<&[u8]>,
    ) -> Result<(), ToolAttachmentHistoryError> {
        self.validate_read(index, read)?;
        read.validate_payload_presence(payload.is_some())?;
        let expected = if read.reobserves.is_some() {
            None
        } else {
            match &read.outcome {
                ToolAttachmentReadOutcome::TypedChunk(end) => self.writes[&end.0].accepted_length,
                ToolAttachmentReadOutcome::RawBytes { length, .. } => Some(*length),
                ToolAttachmentReadOutcome::Eof { .. } | ToolAttachmentReadOutcome::Error { .. } => {
                    None
                }
            }
        };
        match (expected, payload) {
            (Some(length), Some(bytes)) if u64::try_from(bytes.len()) == Ok(length) => Ok(()),
            (None, None) => Ok(()),
            (Some(_), Some(_)) => Err(ToolAttachmentHistoryError::InvalidRange),
            _ => Err(ToolAttachmentHistoryError::InvalidEffect),
        }
    }

    pub fn validate_read(
        &self,
        index: OplogIndex,
        read: &ToolAttachmentReadObservation,
    ) -> Result<(), ToolAttachmentHistoryError> {
        use ToolAttachmentHistoryError::*;
        if !read.attachment.creation_start.is_defined() || read.attachment.creation_start >= index {
            return Err(InvalidChronology);
        }
        if let StreamConsumerObservation::Guest {
            parent_start_index: Some(parent),
        } = read.observation
            && (!parent.is_defined() || parent >= index)
        {
            return Err(InvalidChronology);
        }
        let source = match &read.outcome {
            ToolAttachmentReadOutcome::TypedChunk(end) => Some((*end, None)),
            ToolAttachmentReadOutcome::RawBytes {
                end,
                offset,
                length,
            } => Some((*end, Some((*offset, *length)))),
            _ => None,
        };
        if let Some((end, range)) = source {
            if end.0 >= index {
                return Err(InvalidChronology);
            }
            let write = self.writes.get(&end.0).ok_or(MissingAcceptance)?;
            if write.attachment != read.attachment {
                return Err(WrongAttachment);
            }
            if write.start <= write.attachment.creation_start || write.start >= end.0 {
                return Err(InvalidChronology);
            }
            if !write.retained {
                return Err(RemovedAcceptance);
            }
            let size = write
                .accepted_length
                .filter(|size| *size > 0)
                .ok_or(NotAccepted)?;
            if let Some((offset, length)) = range {
                if length == 0 || offset.checked_add(length).is_none_or(|end| end > size) {
                    return Err(InvalidRange);
                }
            }
        }
        let terminal_index = match &read.outcome {
            ToolAttachmentReadOutcome::Eof { terminal_index }
            | ToolAttachmentReadOutcome::Error { terminal_index, .. } => Some(*terminal_index),
            _ => None,
        };
        if let Some(terminal_index) = terminal_index {
            let control = self
                .controls
                .get(&terminal_index)
                .ok_or(MissingAcceptance)?;
            if control.attachment != read.attachment {
                return Err(WrongAttachment);
            }
            if terminal_index <= read.attachment.creation_start
                || terminal_index >= index
                || control.producer_start.is_some_and(|start| {
                    start <= read.attachment.creation_start || start >= terminal_index
                })
                || control
                    .entity_parent_start
                    .is_some_and(|start| !start.is_defined() || start >= terminal_index)
            {
                return Err(InvalidChronology);
            }
            if !control.retained {
                return Err(RemovedAcceptance);
            }
            if let ToolAttachmentReadOutcome::Error { failure, .. } = &read.outcome {
                let matches = match &control.transition {
                    ToolAttachmentControl::TerminalSelected(
                        ToolAttachmentTerminal::Application(ByteStreamCloseCause::Failed(selected)),
                    ) => selected == failure,
                    ToolAttachmentControl::TerminalSelected(
                        ToolAttachmentTerminal::HostResourceExhausted,
                    ) => *failure == ByteStreamFailure::ResourceExhausted,
                    _ => false,
                };
                if !matches {
                    return Err(InvalidEffect);
                }
            }
        }
        let mut previous_index = index;
        let mut previous_read = read;
        while let Some(original) = previous_read.reobserves {
            if !original.is_defined() || original >= previous_index {
                return Err(InvalidChronology);
            }
            let old = self.observations.get(&original).ok_or(MissingObservation)?;
            if previous_read.attachment != old.attachment
                || previous_read.logical_invocation != old.logical_invocation
                || previous_read.ordinal != old.ordinal
                || previous_read.outcome != old.outcome
            {
                return Err(DifferentObservation);
            }
            if old.attachment.creation_start >= original
                || source.is_some_and(|(end, _)| end.0 >= original)
                || terminal_index.is_some_and(|index| index >= original)
            {
                return Err(InvalidChronology);
            }
            if let StreamConsumerObservation::Guest {
                parent_start_index: Some(parent),
            } = old.observation
                && (!parent.is_defined() || parent >= original)
            {
                return Err(InvalidChronology);
            }
            previous_index = original;
            previous_read = old;
        }
        Ok(())
    }
}

/// An unread suffix of one accepted write. It contains no payload bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolAttachmentUnreadRange {
    pub offset: u64,
    pub length: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolAttachmentProjectedState {
    pub unread: BTreeMap<OplogIndex, ToolAttachmentUnreadRange>,
    pub terminal: Option<ToolAttachmentTerminal>,
    pub terminal_index: Option<OplogIndex>,
    pub reader_discarded: bool,
    pub error_observed: bool,
}

/// Incremental projection of crossed effects, not a scan-ahead publication cache.
/// Construct a new projection after changing the owner's deleted regions.
#[derive(Default)]
pub struct ToolAttachmentProjection {
    frontier: OplogIndex,
    attachments: BTreeMap<ToolAttachmentId, ToolAttachmentProjectedState>,
}

impl ToolAttachmentProjection {
    pub fn attachment(&self, id: &ToolAttachmentId) -> Option<&ToolAttachmentProjectedState> {
        self.attachments.get(id)
    }

    /// Apply only newly crossed records. On error the owner must discard the
    /// projection; partially applied corrupt history must never reach a guest.
    pub fn advance_to(
        &mut self,
        frontier: OplogIndex,
        history: &ToolAttachmentHistory,
        deleted: &DeletedRegions,
    ) -> Result<(), ToolAttachmentHistoryError> {
        use ToolAttachmentHistoryError::*;
        if frontier < self.frontier {
            return Err(InvalidChronology);
        }
        if frontier == self.frontier {
            return Ok(());
        }
        enum Effect<'a> {
            Write(&'a ToolAttachmentWriteMetadata),
            Read(&'a ToolAttachmentReadObservation),
            Control(&'a ToolAttachmentControlMetadata),
        }
        let range = (Excluded(self.frontier), Included(frontier));
        let mut effects: Vec<_> = history
            .writes
            .range(range)
            .map(|(index, write)| (*index, Effect::Write(write)))
            .chain(
                history
                    .observations
                    .range(range)
                    .map(|(index, read)| (*index, Effect::Read(read))),
            )
            .chain(
                history
                    .controls
                    .range(range)
                    .map(|(index, control)| (*index, Effect::Control(control))),
            )
            .collect();
        effects.sort_by_key(|(index, _)| *index);
        for (index, effect) in effects {
            if deleted.is_in_deleted_region(index) {
                continue;
            }
            match effect {
                Effect::Write(write) => {
                    if !write.retained
                        || deleted.is_in_deleted_region(write.start)
                        || deleted.is_in_deleted_region(write.attachment.creation_start)
                    {
                        continue;
                    }
                    if !write.attachment.creation_start.is_defined()
                        || write.start <= write.attachment.creation_start
                        || write.start >= index
                    {
                        return Err(InvalidChronology);
                    }
                    if let Some(length) = write.accepted_length {
                        let state = self.attachments.entry(write.attachment).or_default();
                        if length == 0 || state.terminal.is_some() || state.reader_discarded {
                            return Err(InvalidEffect);
                        }
                        state
                            .unread
                            .insert(index, ToolAttachmentUnreadRange { offset: 0, length });
                    }
                }
                Effect::Read(read) => {
                    history.validate_read(index, read)?;
                    if deleted.is_in_deleted_region(read.attachment.creation_start) {
                        return Err(InvalidEffect);
                    }
                    let state = self.attachments.entry(read.attachment).or_default();
                    match &read.outcome {
                        ToolAttachmentReadOutcome::TypedChunk(end) => {
                            let Some((next, unread)) = state.unread.first_key_value() else {
                                return Err(MissingAcceptance);
                            };
                            if *next != end.0 || unread.offset != 0 {
                                return Err(InvalidRange);
                            }
                            state.unread.pop_first();
                        }
                        ToolAttachmentReadOutcome::RawBytes {
                            end,
                            offset,
                            length,
                        } => {
                            let Some(mut entry) = state.unread.first_entry() else {
                                return Err(MissingAcceptance);
                            };
                            let next = *entry.key();
                            let unread = entry.get_mut();
                            if next != end.0 || unread.offset != *offset || *length > unread.length
                            {
                                return Err(InvalidRange);
                            }
                            unread.offset += length;
                            unread.length -= length;
                            if unread.length == 0 {
                                entry.remove();
                            }
                        }
                        ToolAttachmentReadOutcome::Error {
                            terminal_index,
                            failure,
                        } => {
                            let matches = match &state.terminal {
                                Some(ToolAttachmentTerminal::Application(
                                    ByteStreamCloseCause::Failed(selected),
                                )) => selected == failure,
                                Some(ToolAttachmentTerminal::HostResourceExhausted) => {
                                    *failure == ByteStreamFailure::ResourceExhausted
                                }
                                _ => false,
                            };
                            if !state.unread.is_empty()
                                || !matches
                                || state.error_observed
                                || state.terminal_index != Some(*terminal_index)
                            {
                                return Err(InvalidEffect);
                            }
                            state.error_observed = true;
                        }
                        ToolAttachmentReadOutcome::Eof { terminal_index } => {
                            let finished = matches!(
                                state.terminal,
                                Some(ToolAttachmentTerminal::Application(
                                    ByteStreamCloseCause::Finished
                                        | ByteStreamCloseCause::ConsumerCancelled
                                ))
                            );
                            if !state.unread.is_empty()
                                || !(finished || state.error_observed)
                                || state.terminal_index != Some(*terminal_index)
                            {
                                return Err(InvalidEffect);
                            }
                        }
                    }
                }
                Effect::Control(control) => {
                    if !control.retained {
                        continue;
                    }
                    let id = control.attachment;
                    if let Some(parent) = control.entity_parent_start {
                        if !parent.is_defined() || parent >= index {
                            return Err(InvalidChronology);
                        }
                        if deleted.is_in_deleted_region(parent) {
                            continue;
                        }
                    }
                    if let Some(start) = control.producer_start {
                        if start <= id.creation_start || start >= index {
                            return Err(InvalidChronology);
                        }
                        if deleted.is_in_deleted_region(start) {
                            continue;
                        }
                    }
                    if !id.creation_start.is_defined()
                        || id.creation_start >= index
                        || deleted.is_in_deleted_region(id.creation_start)
                    {
                        return Err(InvalidChronology);
                    }
                    let state = self.attachments.entry(id).or_default();
                    match &control.transition {
                        ToolAttachmentControl::TerminalSelected(terminal) => {
                            if state.terminal.is_some() {
                                return Err(InvalidEffect);
                            }
                            state.terminal = Some(terminal.clone());
                            state.terminal_index = Some(index);
                            if matches!(
                                terminal,
                                ToolAttachmentTerminal::Application(
                                    ByteStreamCloseCause::ConsumerCancelled
                                )
                            ) {
                                state.unread.clear();
                            }
                        }
                        ToolAttachmentControl::ReaderDiscarded => {
                            if state.reader_discarded {
                                return Err(InvalidEffect);
                            }
                            state.reader_discarded = true;
                            state.unread.clear();
                            if state.terminal.is_none() {
                                state.terminal_index = Some(index);
                            }
                            state
                                .terminal
                                .get_or_insert(ToolAttachmentTerminal::Application(
                                    ByteStreamCloseCause::ConsumerCancelled,
                                ));
                        }
                    }
                }
            }
        }
        self.frontier = frontier;
        Ok(())
    }
}
