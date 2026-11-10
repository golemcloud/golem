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

//! The one fold of the deleted and skipped regions and of the update queue.
//!
//! The status fold, the checkpoint reader, the fork and the revert validation read their regions
//! from this fold, so they all pair the update entries with [`UpdateQueue::after`].

use super::update_queue::{UpdateQueue, UpdateStep};
use golem_common::base_model::OplogIndex;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{AgentStatusRecord, PendingUpdateKind, PendingUpdateRef};
use std::collections::{BTreeMap, VecDeque};

/// The regions and the update queue after a range of entries.
#[derive(Clone, Debug)]
pub(crate) struct RegionFold {
    /// The regions that a revert deleted.
    pub(crate) deleted: DeletedRegions,
    /// The committed skipped regions, with the override of a `SnapshotBased` queue head.
    pub(crate) skipped: DeletedRegions,
    /// The update queue after the range.
    pub(crate) queue: UpdateQueue,
    /// The step of each entry of the range whose step is not [`UpdateStep::Unchanged`].
    pub(crate) steps: BTreeMap<OplogIndex, UpdateStep>,
}

/// The regions and the update steps after `entries`, from the regions of `baseline` and the
/// update queue of `pending_updates` and of the pending invocations of `baseline`. The pending
/// updates of `baseline` are not read.
///
/// `Jump` and `Revert` entries add skipped regions. A successful snapshot-based update commits
/// the history up to and including its `PendingUpdate` entry, and a successful snapshot-assisted
/// automatic update commits the history up to and including its selected record, both read from
/// the paired queue element; either also commits the bootstrap range that its `SuccessfulUpdate`
/// entry names. A failed snapshot-based or snapshot-assisted update commits the history of its
/// attempt, between its `PendingUpdate` entry (the strategy entry of a snapshot-assisted update)
/// and its `FailedUpdate` entry. The override is the history up to and including the
/// `PendingUpdate` entry of a snapshot-based queue head. Entries in a deleted region change only
/// the manual admissions of the queue.
pub(crate) fn fold_regions_from(
    baseline: &AgentStatusRecord,
    pending_updates: VecDeque<PendingUpdateRef>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> RegionFold {
    fold(baseline, pending_updates, entries, None, FoldScope::Status)
}

/// The skipped regions after `entries`, from an empty status.
pub(crate) fn skipped_regions(entries: &BTreeMap<OplogIndex, OplogEntry>) -> DeletedRegions {
    fold_skipped_regions(entries, None)
}

/// The skipped regions that a revert which drops `dropped` must respect. The regions of
/// snapshot-based and snapshot-assisted updates whose outcome (successful or failed) is inside
/// `dropped`, and the override of a snapshot-based queue head inside `dropped`, are left out;
/// jumps and earlier reverts stay.
pub(crate) fn revert_validation_regions(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    dropped: &OplogRegion,
) -> DeletedRegions {
    fold_skipped_regions(entries, Some(dropped))
}

/// The skipped regions after `entries` from an empty status, without the regions that
/// [`fold`] leaves out for `ignored`. No skipped region depends on the manual admissions or on
/// the update steps, so the fold neither decodes the invocation payloads nor keeps the steps.
fn fold_skipped_regions(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    ignored: Option<&OplogRegion>,
) -> DeletedRegions {
    fold(
        &AgentStatusRecord::default(),
        VecDeque::new(),
        entries,
        ignored,
        FoldScope::Regions,
    )
    .skipped
}

/// The deleted regions of `initial` with the regions that the `Revert` entries of `entries` drop.
/// A `Revert` entry drops the history that ends right before it.
pub(crate) fn deleted_regions(
    initial: DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> DeletedRegions {
    entries
        .iter()
        .fold(
            DeletedRegionsBuilder::from_regions(initial.into_regions()),
            |mut builder, (index, entry)| {
                if let OplogEntry::Revert { dropped_region, .. } = entry {
                    debug_assert!(
                        dropped_region.end.next() == *index,
                        "the Revert entry at {index} drops {dropped_region}, which does not end right before it"
                    );
                    builder.add(dropped_region.clone());
                }
                builder
            },
        )
        .build()
}

/// What a fold gives besides the regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FoldScope {
    /// The queue with its manual admissions, and the update steps.
    Status,
    /// Nothing that the skipped regions do not need: the manual update invocations do not reach
    /// the queue, which then holds no manual admissions, and the fold keeps no update step.
    Regions,
}

/// The fold of `entries` in `scope`.
fn fold(
    baseline: &AgentStatusRecord,
    pending_updates: VecDeque<PendingUpdateRef>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    ignored: Option<&OplogRegion>,
    scope: FoldScope,
) -> RegionFold {
    let deleted = deleted_regions(baseline.deleted_regions.clone(), entries);
    let mut committed = baseline.skipped_regions.clone();
    if committed.is_overridden() {
        committed.drop_override();
    }
    let is_ignored = |index: OplogIndex| ignored.is_some_and(|region| region.contains(index));

    // The index of the latest outcome of a queue element. The attempts of the element at the
    // queue head begin only after the outcome of the element before it.
    let last_outcome = baseline
        .successful_updates
        .iter()
        .filter(|update| update.pending_update.is_some())
        .map(|update| update.oplog_index)
        .max();
    let (queue, skipped, steps, _) = entries.iter().fold(
        (
            UpdateQueue::of(pending_updates, &baseline.pending_invocations),
            DeletedRegionsBuilder::from_regions(committed.into_regions()),
            BTreeMap::new(),
            last_outcome,
        ),
        |(queue, mut skipped, mut steps, mut last_outcome), (index, entry)| {
            let is_deleted = deleted.is_in_deleted_region(*index);
            let (queue, step) = match entry {
                OplogEntry::PendingAgentInvocation { .. } if scope == FoldScope::Regions => {
                    (queue, UpdateStep::Unchanged)
                }
                _ => queue.after(*index, entry, is_deleted),
            };
            if !is_deleted {
                match entry {
                    OplogEntry::Jump { jump, .. } => skipped.add(jump.clone()),
                    OplogEntry::Revert { dropped_region, .. } => {
                        skipped.add(dropped_region.clone())
                    }
                    _ => {}
                }
                if !is_ignored(*index)
                    && let Some(ended) = step.ended()
                {
                    let regions = match entry {
                        OplogEntry::SuccessfulUpdate { bootstrap, .. } => {
                            committed_update_regions(ended, bootstrap.as_ref())
                        }
                        OplogEntry::FailedUpdate { .. } => {
                            failed_attempt_region(ended, last_outcome, *index)
                                .into_iter()
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    for region in regions {
                        skipped.add(region);
                    }
                }
            }
            if !is_deleted && step.ended().is_some() {
                last_outcome = Some(*index);
            }
            if scope == FoldScope::Status && step != UpdateStep::Unchanged {
                steps.insert(*index, step);
            }
            (queue, skipped, steps, last_outcome)
        },
    );

    let mut skipped = deleted
        .regions()
        .fold(skipped, |mut skipped, region| {
            skipped.add(region.clone());
            skipped
        })
        .build();
    if let Some(head) = queue.head()
        && matches!(head.kind, PendingUpdateKind::SnapshotBased { .. })
        && !is_ignored(head.oplog_index)
    {
        skipped.set_override(DeletedRegions::from_regions([prefix_through(
            head.oplog_index,
        )]));
    }

    RegionFold {
        deleted,
        skipped,
        queue,
        steps,
    }
}

/// The history that a successful update of `applied` makes skipped: the history up to and
/// including the `PendingUpdate` entry of a snapshot-based update, or the history up to and
/// including the record that a snapshot-assisted automatic update selected, and for both the
/// `bootstrap` range of the target.
fn committed_update_regions(
    applied: &PendingUpdateRef,
    bootstrap: Option<&OplogRegion>,
) -> Vec<OplogRegion> {
    let prefix = match &applied.kind {
        PendingUpdateKind::Automatic => return Vec::new(),
        PendingUpdateKind::SnapshotAssistedAutomatic(selection) => {
            prefix_through(selection.snapshot.index)
        }
        PendingUpdateKind::SnapshotBased { .. } => prefix_through(applied.oplog_index),
    };
    std::iter::once(prefix).chain(bootstrap.cloned()).collect()
}

/// The history that a failed update of `attempted`, whose `FailedUpdate` entry is at `failed`,
/// makes skipped: the entries that the attempts of a snapshot-based or snapshot-assisted update
/// recorded before `failed`, which belong to the target revision. The attempts begin after the
/// `PendingUpdate` entry of the element (the strategy entry of a snapshot-assisted update) and
/// after `last_outcome`, the outcome of the queue element before it, whose history stays.
fn failed_attempt_region(
    attempted: &PendingUpdateRef,
    last_outcome: Option<OplogIndex>,
    failed: OplogIndex,
) -> Option<OplogRegion> {
    let attempts_after = last_outcome.map_or(attempted.oplog_index, |outcome| {
        outcome.max(attempted.oplog_index)
    });
    match &attempted.kind {
        PendingUpdateKind::SnapshotBased { .. }
        | PendingUpdateKind::SnapshotAssistedAutomatic(_)
            if attempts_after.next() < failed =>
        {
            Some(OplogRegion::from_index_range(
                attempts_after.next()..=failed.previous(),
            ))
        }
        _ => None,
    }
}

fn prefix_through(index: OplogIndex) -> OplogRegion {
    OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=index)
}
