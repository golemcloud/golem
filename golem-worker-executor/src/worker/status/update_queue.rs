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
    AgentInvocationPayload, AgentStatusRecord, PendingUpdateKind, PendingUpdateRef, Timestamp,
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
    /// The queue of a fold that starts from `status`: its pending updates, and its pending
    /// invocations that carry a manual update target revision.
    pub(crate) fn of(status: &AgentStatusRecord) -> Self {
        Self {
            pending: status.pending_updates.clone(),
            manual_admissions: status
                .pending_invocations
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
                Some(target_revision) => {
                    self.manual_admissions.push_back(ManualAdmission {
                        timestamp: *timestamp,
                        index,
                        target_revision,
                    });
                    UpdateStep::ManualAdmitted(target_revision)
                }
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
/// Manual update payloads are tiny and always stored inline, so this never needs to download an
/// external payload: an `External` payload is by definition not a manual update.
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
mod tests {
    use super::*;
    use golem_common::model::oplog::FilesystemSnapshotName;
    use golem_common::model::{AssistedSelection, PendingInvocationRef, UsableAutomaticSnapshot};
    use test_r::test;

    fn idx(value: u64) -> OplogIndex {
        OplogIndex::from_u64(value)
    }

    fn revision(value: u64) -> ComponentRevision {
        ComponentRevision::new(value).unwrap()
    }

    fn automatic(target: u64) -> OplogEntry {
        OplogEntry::pending_update(
            UpdateDescription::Automatic {
                target_revision: revision(target),
            },
            None,
        )
    }

    fn strategy(target: u64, admission: u64, snapshot: u64) -> OplogEntry {
        OplogEntry::pending_update(
            UpdateDescription::SnapshotAssistedAutomatic {
                target_revision: revision(target),
                source_component_revision: revision(1),
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot_index: idx(snapshot),
                snapshot_revision: revision(1),
                filesystem_snapshot: Some(FilesystemSnapshotName::periodic()),
            },
            Some(idx(admission)),
        )
    }

    fn manual_invocation(target: u64) -> OplogEntry {
        OplogEntry::PendingAgentInvocation {
            timestamp: Timestamp::from(1_000),
            idempotency_key: golem_common::model::IdempotencyKey::fresh(),
            payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::ManualUpdate {
                target_revision: revision(target),
            })),
            trace_id: golem_common::model::invocation_context::TraceId::generate(),
            trace_states: Vec::new(),
            invocation_context: Vec::new(),
        }
    }

    fn manual_pending_update(target: u64, admission: Option<u64>) -> OplogEntry {
        OplogEntry::pending_update(
            UpdateDescription::SnapshotBased {
                target_revision: revision(target),
                payload: OplogPayload::Inline(Box::new(vec![])),
                mime_type: "application/octet-stream".to_string(),
                filesystem_snapshot: None,
            },
            admission.map(idx),
        )
    }

    fn failed(target: u64, attempt: Option<u64>) -> OplogEntry {
        OplogEntry::failed_update(revision(target), None, None, attempt.map(idx), None)
    }

    fn succeeded(target: u64) -> OplogEntry {
        OplogEntry::successful_update(revision(target), 10, None, Default::default(), None)
    }

    /// The queue and the steps after `entries`, from `queue`, with no entry deleted.
    fn fold(
        queue: UpdateQueue,
        entries: impl IntoIterator<Item = (u64, OplogEntry)>,
    ) -> (UpdateQueue, Vec<UpdateStep>) {
        entries
            .into_iter()
            .fold((queue, Vec::new()), |(queue, mut steps), (index, entry)| {
                let (queue, step) = queue.after(idx(index), &entry, false);
                steps.push(step);
                (queue, steps)
            })
    }

    fn admissions(queue: &UpdateQueue) -> Vec<OplogIndex> {
        queue.manual_admissions.iter().map(|a| a.index).collect()
    }

    #[test]
    fn a_strategy_entry_refines_an_unselected_automatic_head_and_adds_nothing() {
        let (queue, steps) = fold(
            UpdateQueue::default(),
            [(2, automatic(3)), (5, strategy(3, 2, 4))],
        );

        assert_eq!(
            steps,
            vec![
                UpdateStep::Admitted { consumed: None },
                UpdateStep::Selected
            ]
        );
        let pending = queue.into_open().0;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].admission_index, idx(2));
        assert_eq!(pending[0].oplog_index, idx(5));
        let PendingUpdateKind::SnapshotAssistedAutomatic(selection) = &pending[0].kind else {
            panic!("the head must carry the selection");
        };
        assert_eq!(selection.snapshot.index, idx(4));
        assert_eq!(selection.snapshot.component_revision, revision(1));
        assert!(selection.snapshot.filesystem_snapshot.is_some());
    }

    #[test]
    fn a_strategy_entry_does_not_refine_another_target_another_admission_or_a_selected_head() {
        let base = || fold(UpdateQueue::default(), [(2, automatic(3))]).0;
        let cases = [
            fold(base(), [(5, strategy(4, 2, 4))]),
            fold(base(), [(5, strategy(3, 7, 4))]),
            fold(base(), [(5, strategy(3, 2, 4)), (6, strategy(3, 2, 4))]),
        ];

        assert_eq!(
            cases
                .iter()
                .map(|(queue, steps)| (queue.pending.len(), steps.last().cloned()))
                .collect::<Vec<_>>(),
            vec![
                (1, Some(UpdateStep::Unchanged)),
                (1, Some(UpdateStep::Unchanged)),
                (1, Some(UpdateStep::Unchanged)),
            ]
        );
        assert_eq!(cases[0].0.pending[0].kind, PendingUpdateKind::Automatic);
        assert_eq!(cases[1].0.pending[0].kind, PendingUpdateKind::Automatic);
        assert_eq!(cases[2].0.pending[0].oplog_index, idx(5));
    }

    #[test]
    fn a_manual_pending_update_with_an_attempt_index_consumes_its_admission() {
        let (queue, steps) = fold(
            UpdateQueue::default(),
            [
                (2, manual_invocation(3)),
                (3, manual_invocation(4)),
                (4, manual_pending_update(4, Some(3))),
            ],
        );

        assert_eq!(
            steps.last(),
            Some(&UpdateStep::Admitted {
                consumed: Some(idx(3))
            })
        );
        assert_eq!(admissions(&queue), vec![idx(2)]);
        let pending = queue.into_open().0;
        assert_eq!(
            (pending[0].oplog_index, pending[0].admission_index),
            (idx(4), idx(3))
        );
    }

    #[test]
    fn a_failed_update_with_an_attempt_index_ends_its_head_else_its_admission_else_nothing() {
        let base = || {
            fold(
                UpdateQueue::default(),
                [(2, automatic(3)), (3, manual_invocation(4))],
            )
            .0
        };

        let (head, head_steps) = fold(base(), [(4, failed(3, Some(2)))]);
        let (admission, admission_steps) = fold(base(), [(4, failed(4, Some(3)))]);
        let (nothing, nothing_steps) = fold(base(), [(4, failed(9, Some(9)))]);

        assert!(matches!(
            head_steps.last(),
            Some(UpdateStep::FailedQueued(Some(update))) if update.admission_index == idx(2)
        ));
        assert!(head.pending.is_empty());
        assert_eq!(admissions(&head), vec![idx(3)]);

        assert_eq!(
            admission_steps.last(),
            Some(&UpdateStep::FailedAdmission(ManualAdmission {
                timestamp: Timestamp::from(1_000),
                index: idx(3),
                target_revision: revision(4),
            }))
        );
        assert_eq!(admission.pending.len(), 1);
        assert!(admissions(&admission).is_empty());

        assert_eq!(nothing_steps.last(), Some(&UpdateStep::FailedQueued(None)));
        assert_eq!(nothing.pending.len(), 1);
        assert_eq!(admissions(&nothing), vec![idx(3)]);
    }

    #[test]
    fn a_failed_update_with_an_attempt_index_never_ends_an_element_behind_the_head() {
        let (queue, steps) = fold(
            UpdateQueue::default(),
            [
                (2, automatic(3)),
                (3, manual_invocation(4)),
                (4, manual_pending_update(4, Some(3))),
                (5, failed(4, Some(3))),
            ],
        );

        assert_eq!(steps.last(), Some(&UpdateStep::FailedQueued(None)));
        assert_eq!(queue.into_open().0.len(), 2);
    }

    #[test]
    fn a_failed_update_without_an_attempt_index_ends_a_head_of_the_same_target_only() {
        let base = || fold(UpdateQueue::default(), [(2, automatic(3))]).0;

        let (same, same_steps) = fold(base(), [(3, failed(3, None))]);
        let (other, other_steps) = fold(base(), [(3, failed(4, None))]);

        assert!(matches!(
            same_steps.last(),
            Some(UpdateStep::FailedQueued(Some(_)))
        ));
        assert!(same.pending.is_empty());
        assert_eq!(other_steps.last(), Some(&UpdateStep::FailedQueued(None)));
        assert_eq!(other.pending.len(), 1);
    }

    #[test]
    fn a_successful_update_ends_the_head_whatever_its_target() {
        let (queue, steps) = fold(
            UpdateQueue::default(),
            [(2, automatic(3)), (3, automatic(4)), (4, succeeded(4))],
        );

        assert!(matches!(
            steps.last(),
            Some(UpdateStep::Succeeded(Some(update))) if update.target_revision == revision(3)
        ));
        assert_eq!(queue.into_open().0[0].target_revision, revision(4));
    }

    #[test]
    fn deleted_entries_change_only_the_manual_admissions() {
        let queue = fold(
            UpdateQueue::default(),
            [(2, automatic(3)), (3, manual_invocation(4))],
        )
        .0;
        let entries = [
            (4, manual_invocation(5)),
            (5, automatic(6)),
            (6, strategy(3, 2, 1)),
            (7, manual_pending_update(4, Some(3))),
            (8, failed(5, Some(4))),
            (9, failed(3, Some(2))),
            (10, succeeded(3)),
        ];
        let (queue, steps) =
            entries
                .into_iter()
                .fold((queue, Vec::new()), |(queue, mut steps), (index, entry)| {
                    let (queue, step) = queue.after(idx(index), &entry, true);
                    steps.push(step);
                    (queue, steps)
                });

        assert_eq!(
            steps,
            vec![
                UpdateStep::ManualAdmitted(revision(5)),
                UpdateStep::Unchanged,
                UpdateStep::Unchanged,
                UpdateStep::ConsumedInDeletedRegion(idx(3)),
                UpdateStep::ConsumedInDeletedRegion(idx(4)),
                UpdateStep::Unchanged,
                UpdateStep::Unchanged,
            ]
        );
        assert!(admissions(&queue).is_empty());
        let pending = queue.into_open().0;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, PendingUpdateKind::Automatic);
    }

    #[test]
    fn of_seeds_the_queue_and_the_manual_admissions_from_a_checkpoint_status() {
        let head = PendingUpdateRef {
            timestamp: Timestamp::from(2_000),
            oplog_index: idx(5),
            admission_index: idx(2),
            target_revision: revision(3),
            kind: PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot: UsableAutomaticSnapshot {
                    index: idx(4),
                    component_revision: revision(1),
                    filesystem_snapshot: None,
                },
            })),
        };
        let status = AgentStatusRecord {
            pending_updates: [head.clone()].into(),
            pending_invocations: vec![
                PendingInvocationRef {
                    timestamp: Timestamp::from(1_000),
                    oplog_index: idx(6),
                    idempotency_key: Some(golem_common::model::IdempotencyKey::fresh()),
                    manual_update_target_revision: None,
                },
                PendingInvocationRef {
                    timestamp: Timestamp::from(1_000),
                    oplog_index: idx(7),
                    idempotency_key: None,
                    manual_update_target_revision: Some(revision(4)),
                },
            ],
            ..AgentStatusRecord::default()
        };

        let queue = UpdateQueue::of(&status);
        assert_eq!(queue.head(), Some(&head));
        assert_eq!(admissions(&queue), vec![idx(7)]);

        let (queue, step) = queue.after(idx(8), &failed(4, Some(7)), false);
        assert!(
            matches!(step, UpdateStep::FailedAdmission(admission) if admission.index == idx(7))
        );
        let (_, step) = queue.after(idx(9), &succeeded(3), false);
        assert_eq!(step, UpdateStep::Succeeded(Some(head)));
    }

    /// Only an automatic admission that no strategy entry refined still needs its strategy.
    #[test]
    fn only_an_unrefined_automatic_admission_is_an_unselected_automatic_update() {
        let head = |kind: PendingUpdateKind, oplog_index: u64| PendingUpdateRef {
            timestamp: Timestamp::from(1_000),
            oplog_index: idx(oplog_index),
            admission_index: idx(10),
            target_revision: revision(3),
            kind,
        };
        let snapshot_based = || PendingUpdateKind::SnapshotBased {
            filesystem_snapshot: None,
        };
        let assisted = |filesystem_snapshot| {
            PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
                source_revision_start_index: idx(4),
                snapshot: UsableAutomaticSnapshot {
                    index: idx(7),
                    component_revision: revision(2),
                    filesystem_snapshot,
                },
            }))
        };
        assert_eq!(
            [
                is_unselected_automatic(&head(PendingUpdateKind::Automatic, 10)),
                is_unselected_automatic(&head(PendingUpdateKind::Automatic, 12)),
                is_unselected_automatic(&head(snapshot_based(), 10)),
                is_unselected_automatic(&head(snapshot_based(), 12)),
                is_unselected_automatic(&head(assisted(None), 12)),
                is_unselected_automatic(&head(
                    assisted(Some(FilesystemSnapshotName::periodic())),
                    12
                )),
            ],
            [true, false, false, false, false, false]
        );
    }
}
