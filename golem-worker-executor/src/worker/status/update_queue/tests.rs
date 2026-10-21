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
use golem_common::model::oplog::FilesystemSnapshotName;
use golem_common::model::{AgentStatusRecord, AssistedSelection, UsableAutomaticSnapshot};
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

    let queue = UpdateQueue::of(status.pending_updates, &status.pending_invocations);
    assert_eq!(queue.head(), Some(&head));
    assert_eq!(admissions(&queue), vec![idx(7)]);

    let (queue, step) = queue.after(idx(8), &failed(4, Some(7)), false);
    assert!(matches!(step, UpdateStep::FailedAdmission(admission) if admission.index == idx(7)));
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
