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
use std::collections::BTreeMap;

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

/// The regions and the update steps after `entries`, from the regions and the update queue of
/// `baseline`.
///
/// `Jump` and `Revert` entries add skipped regions. A successful snapshot-based update commits
/// the history up to and including its `PendingUpdate` entry, and a successful snapshot-assisted
/// automatic update commits the history up to and including its selected record, both read from
/// the paired queue element. The override is the history up to and including the `PendingUpdate`
/// entry of a snapshot-based queue head. Entries in a deleted region change only the manual admissions of the queue.
pub(crate) fn fold_regions(
    baseline: &AgentStatusRecord,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> RegionFold {
    fold(baseline, entries, None)
}

/// The skipped regions that a revert which drops `dropped` must respect. The regions of
/// snapshot-based and snapshot-assisted updates whose outcome is inside `dropped`, and the
/// override of a snapshot-based queue head inside `dropped`, are left out; jumps and earlier
/// reverts stay.
pub(crate) fn revert_validation_regions(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    dropped: &OplogRegion,
) -> DeletedRegions {
    fold(&AgentStatusRecord::default(), entries, Some(dropped)).skipped
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

fn fold(
    baseline: &AgentStatusRecord,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    ignored: Option<&OplogRegion>,
) -> RegionFold {
    let deleted = deleted_regions(baseline.deleted_regions.clone(), entries);
    let mut committed = baseline.skipped_regions.clone();
    if committed.is_overridden() {
        committed.drop_override();
    }
    let is_ignored = |index: OplogIndex| ignored.is_some_and(|region| region.contains(index));

    let (queue, skipped, steps) = entries.iter().fold(
        (
            UpdateQueue::of(baseline),
            DeletedRegionsBuilder::from_regions(committed.into_regions()),
            BTreeMap::new(),
        ),
        |(queue, mut skipped, mut steps), (index, entry)| {
            let is_deleted = deleted.is_in_deleted_region(*index);
            let (queue, step) = queue.after(*index, entry, is_deleted);
            if !is_deleted {
                match entry {
                    OplogEntry::Jump { jump, .. } => skipped.add(jump.clone()),
                    OplogEntry::Revert { dropped_region, .. } => {
                        skipped.add(dropped_region.clone())
                    }
                    _ => {}
                }
                if matches!(entry, OplogEntry::SuccessfulUpdate { .. })
                    && !is_ignored(*index)
                    && let Some(region) = step.ended().and_then(committed_update_region)
                {
                    skipped.add(region);
                }
            }
            if step != UpdateStep::Unchanged {
                steps.insert(*index, step);
            }
            (queue, skipped, steps)
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
/// including the record that a snapshot-assisted automatic update selected.
fn committed_update_region(applied: &PendingUpdateRef) -> Option<OplogRegion> {
    match &applied.kind {
        PendingUpdateKind::Automatic => None,
        PendingUpdateKind::SnapshotAssistedAutomatic(selection) => {
            Some(prefix_through(selection.snapshot.index))
        }
        PendingUpdateKind::SnapshotBased { .. } => Some(prefix_through(applied.oplog_index)),
    }
}

fn prefix_through(index: OplogIndex) -> OplogRegion {
    OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=index)
}
