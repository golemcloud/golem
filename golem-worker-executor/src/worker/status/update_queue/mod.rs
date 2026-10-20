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

//! The one pairing rule of component updates.
//!
//! An update has up to three entries: its admission (a `PendingUpdate` without an attempt index,
//! or the `PendingAgentInvocation` of a manual update), the strategy entry of an automatic update
//! (a `PendingUpdate` whose attempt index names the admission), and its outcome
//! (`SuccessfulUpdate` or `FailedUpdate`). The queue pairs them, so that one update counts once
//! in the status, the skipped regions, the pending invocations, the cut point and the fork.

use golem_common::base_model::OplogIndex;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{OplogEntry, OplogPayload, UpdateDescription};
use golem_common::model::{
    AgentInvocationPayload, PendingInvocationRef, PendingUpdateKind, PendingUpdateRef, Timestamp,
};
use golem_common::serialization::deserialize;
use std::collections::VecDeque;

/// A manual update invocation (a `PendingAgentInvocation` entry) that no `PendingUpdate` entry
/// and no outcome has paired yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManualAdmission {
    pub(crate) timestamp: Timestamp,
    /// The index of the `PendingAgentInvocation` entry.
    pub(crate) index: OplogIndex,
    pub(crate) target_revision: ComponentRevision,
}

/// Whether `update` is an automatic update whose strategy entry is not written yet: its kind is
/// `Automatic` and it is still at its admission entry.
pub(crate) fn is_unselected_automatic(update: &PendingUpdateRef) -> bool {
    update.kind == PendingUpdateKind::Automatic && update.oplog_index == update.admission_index
}

/// The pending updates of an agent and its unpaired manual update invocations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UpdateQueue {
    pending: VecDeque<PendingUpdateRef>,
    manual_admissions: VecDeque<ManualAdmission>,
}

/// What an entry did to the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateStep {
    Unchanged,
    /// A manual update invocation added a manual admission for this target revision.
    ManualAdmitted(ComponentRevision),
    /// A `PendingUpdate` added a queue element. `consumed` is the manual admission that a
    /// `SnapshotBased` entry with an attempt index paired.
    Admitted {
        consumed: Option<OplogIndex>,
    },
    /// A strategy entry refined the queue head.
    Selected,
    /// A `SuccessfulUpdate` applied this queue element.
    Succeeded(Option<PendingUpdateRef>),
    /// A `FailedUpdate` ended this queue element.
    FailedQueued(Option<PendingUpdateRef>),
    /// A `FailedUpdate` ended a manual update invocation that no `PendingUpdate` had paired.
    FailedAdmission(ManualAdmission),
    /// An entry in a deleted region (a `PendingUpdate` or a `FailedUpdate` with an attempt index)
    /// consumed the manual admission at this index.
    ConsumedInDeletedRegion(OplogIndex),
}

impl UpdateStep {
    /// The index of the manual admission that this step consumed: the admission that a
    /// `PendingUpdate` paired, that a `FailedUpdate` ended, or that an entry in a deleted region
    /// consumed.
    pub(crate) fn consumed_admission(&self) -> Option<OplogIndex> {
        match self {
            Self::Admitted {
                consumed: Some(index),
            }
            | Self::ConsumedInDeletedRegion(index) => Some(*index),
            Self::FailedAdmission(admission) => Some(admission.index),
            Self::Unchanged
            | Self::ManualAdmitted(_)
            | Self::Admitted { consumed: None }
            | Self::Selected
            | Self::Succeeded(_)
            | Self::FailedQueued(_) => None,
        }
    }

    /// The target revision of the manual update invocation that this step admitted.
    pub(crate) fn manual_admission_target(&self) -> Option<ComponentRevision> {
        match self {
            Self::ManualAdmitted(target_revision) => Some(*target_revision),
            Self::Unchanged
            | Self::Admitted { .. }
            | Self::Selected
            | Self::Succeeded(_)
            | Self::FailedQueued(_)
            | Self::FailedAdmission(_)
            | Self::ConsumedInDeletedRegion(_) => None,
        }
    }

    /// The queue element that this outcome ended, when the outcome paired with one.
    pub(crate) fn ended(&self) -> Option<&PendingUpdateRef> {
        match self {
            Self::Succeeded(Some(update)) | Self::FailedQueued(Some(update)) => Some(update),
            Self::Unchanged
            | Self::ManualAdmitted(_)
            | Self::Admitted { .. }
            | Self::Selected
            | Self::Succeeded(None)
            | Self::FailedQueued(None)
            | Self::FailedAdmission(_)
            | Self::ConsumedInDeletedRegion(_) => None,
        }
    }
}

impl UpdateQueue {
    /// The queue of a fold that starts from `pending` updates and from the `pending_invocations`
    /// that carry a manual update target revision.
    pub(crate) fn of(
        pending: VecDeque<PendingUpdateRef>,
        pending_invocations: &[PendingInvocationRef],
    ) -> Self {
        Self {
            pending,
            manual_admissions: pending_invocations
                .iter()
                .filter_map(|invocation| {
                    invocation
                        .manual_update_target_revision
                        .map(|target_revision| ManualAdmission {
                            timestamp: invocation.timestamp,
                            index: invocation.oplog_index,
                            target_revision,
                        })
                })
                .collect(),
        }
    }

    /// The queue after `entry` at `index`, and what the entry did to it. `deleted` tells whether
    /// `entry` is in a deleted region: such an entry changes only the manual admissions.
    ///
    /// - A manual update invocation adds a manual admission, and its step carries the target
    ///   revision, so a later reader of the step need not decode the invocation again.
    /// - A `PendingUpdate` without an attempt index adds a queue element.
    /// - A `SnapshotBased` `PendingUpdate` with an attempt index adds a queue element and pairs
    ///   the manual admission with that index.
    /// - Another `PendingUpdate` with an attempt index is a strategy entry: it refines the head
    ///   when the head is an unselected `Automatic` element with that admission index and the
    ///   same target, and changes nothing otherwise.
    /// - A `FailedUpdate` with an attempt index ends the head when the head has that admission
    ///   index, else the manual admission with that index. Without an attempt index it ends a
    ///   head with the same target.
    /// - A `SuccessfulUpdate` ends the head.
    pub(crate) fn after(
        mut self,
        index: OplogIndex,
        entry: &OplogEntry,
        deleted: bool,
    ) -> (Self, UpdateStep) {
        let step = match entry {
            OplogEntry::PendingAgentInvocation {
                timestamp, payload, ..
            } => match manual_update_target_revision_of(payload) {
                Some(target_revision) => self.admit(ManualAdmission {
                    timestamp: *timestamp,
                    index,
                    target_revision,
                }),
                None => UpdateStep::Unchanged,
            },
            OplogEntry::PendingUpdate {
                update_attempt_index: Some(attempt_index),
                ..
            }
            | OplogEntry::FailedUpdate {
                update_attempt_index: Some(attempt_index),
                ..
            } if deleted => self
                .take_manual_admission(*attempt_index)
                .map_or(UpdateStep::Unchanged, |admission| {
                    UpdateStep::ConsumedInDeletedRegion(admission.index)
                }),
            _ if deleted => UpdateStep::Unchanged,
            OplogEntry::PendingUpdate {
                timestamp,
                description,
                update_attempt_index,
            } => self.after_pending_update(index, *timestamp, description, *update_attempt_index),
            OplogEntry::FailedUpdate {
                target_revision,
                update_attempt_index,
                ..
            } => self.after_failed_update(*target_revision, *update_attempt_index),
            OplogEntry::SuccessfulUpdate { .. } => UpdateStep::Succeeded(self.pending.pop_front()),
            _ => UpdateStep::Unchanged,
        };
        (self, step)
    }

    /// The queue after the manual update invocation of `admission`, for a reader that has
    /// already classified the invocation, and the step that [`UpdateQueue::after`] gives for its
    /// entry.
    pub(crate) fn after_admission(mut self, admission: ManualAdmission) -> (Self, UpdateStep) {
        let step = self.admit(admission);
        (self, step)
    }

    /// The queue element that a start serves next.
    pub(crate) fn head(&self) -> Option<&PendingUpdateRef> {
        self.pending.front()
    }

    /// The pending updates, the head first, and the manual update invocations that no
    /// `PendingUpdate` and no outcome paired, in the order of their invocations.
    pub(crate) fn into_open(self) -> (VecDeque<PendingUpdateRef>, VecDeque<ManualAdmission>) {
        (self.pending, self.manual_admissions)
    }

    fn after_pending_update(
        &mut self,
        index: OplogIndex,
        timestamp: Timestamp,
        description: &UpdateDescription,
        update_attempt_index: Option<OplogIndex>,
    ) -> UpdateStep {
        let target_revision = *description.target_revision();
        let kind = PendingUpdateKind::of(description);
        match (update_attempt_index, description) {
            (None, _) => {
                self.pending.push_back(PendingUpdateRef {
                    timestamp,
                    oplog_index: index,
                    admission_index: index,
                    target_revision,
                    kind,
                });
                UpdateStep::Admitted { consumed: None }
            }
            (Some(admission_index), UpdateDescription::SnapshotBased { .. }) => {
                let consumed = self
                    .take_manual_admission(admission_index)
                    .map(|admission| admission.index);
                self.pending.push_back(PendingUpdateRef {
                    timestamp,
                    oplog_index: index,
                    admission_index,
                    target_revision,
                    kind,
                });
                UpdateStep::Admitted { consumed }
            }
            (Some(admission_index), _) => match self.pending.front_mut() {
                Some(head)
                    if head.admission_index == admission_index
                        && head.target_revision == target_revision
                        && is_unselected_automatic(head) =>
                {
                    head.oplog_index = index;
                    head.kind = kind;
                    UpdateStep::Selected
                }
                _ => UpdateStep::Unchanged,
            },
        }
    }

    fn after_failed_update(
        &mut self,
        target_revision: ComponentRevision,
        update_attempt_index: Option<OplogIndex>,
    ) -> UpdateStep {
        let ends_head = self
            .pending
            .front()
            .is_some_and(|head| match update_attempt_index {
                Some(attempt_index) => head.admission_index == attempt_index,
                None => head.target_revision == target_revision,
            });
        if ends_head {
            UpdateStep::FailedQueued(self.pending.pop_front())
        } else {
            update_attempt_index
                .and_then(|attempt_index| self.take_manual_admission(attempt_index))
                .map_or(UpdateStep::FailedQueued(None), UpdateStep::FailedAdmission)
        }
    }

    fn admit(&mut self, admission: ManualAdmission) -> UpdateStep {
        let target_revision = admission.target_revision;
        self.manual_admissions.push_back(admission);
        UpdateStep::ManualAdmitted(target_revision)
    }

    fn take_manual_admission(&mut self, index: OplogIndex) -> Option<ManualAdmission> {
        self.manual_admissions
            .iter()
            .position(|admission| admission.index == index)
            .and_then(|position| self.manual_admissions.remove(position))
    }
}

#[cfg(test)]
thread_local! {
    /// How many invocation payloads this thread decoded to classify them.
    pub(crate) static INVOCATION_PAYLOAD_DECODES: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// The target revision of a pending agent invocation payload when it is a manual update.
///
/// The producer of a manual update invocation keeps its payload in the oplog whatever the payload
/// limit, so this never needs to download an external payload: an `External` payload is not a
/// manual update.
pub(crate) fn manual_update_target_revision_of(
    payload: &OplogPayload<AgentInvocationPayload>,
) -> Option<ComponentRevision> {
    fn target_revision(payload: &AgentInvocationPayload) -> Option<ComponentRevision> {
        match payload {
            AgentInvocationPayload::ManualUpdate { target_revision } => Some(*target_revision),
            _ => None,
        }
    }

    match payload {
        OplogPayload::Inline(p) => target_revision(p),
        OplogPayload::SerializedInline {
            cached: Some(v), ..
        } => target_revision(v),
        OplogPayload::SerializedInline { bytes, .. } => {
            #[cfg(test)]
            INVOCATION_PAYLOAD_DECODES.with(|decodes| decodes.set(decodes.get() + 1));
            deserialize::<AgentInvocationPayload>(bytes)
                .map_err(|e| {
                    tracing::warn!("Failed to deserialize pending agent invocation payload: {e}");
                    e
                })
                .ok()
                .as_ref()
                .and_then(target_revision)
        }
        OplogPayload::External {
            cached: Some(v), ..
        } => target_revision(v),
        OplogPayload::External { .. } => None,
    }
}

#[cfg(test)]
mod tests;
