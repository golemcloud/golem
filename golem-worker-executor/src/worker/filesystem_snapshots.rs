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

//! Connects the filesystem snapshots of the service to a worker: the confirmed snapshot of the
//! worker, the record that a snapshot writes, the restore of a start, and the confirmer.
//!
//! The decisions are plain functions over values, so fast tests can check them.

use super::Worker;
use super::state_actor::ConfirmationReply;
use crate::filesystem_snapshot::ChangeDetection as StoreChangeDetection;
use crate::services::agent_filesystem::{
    ChangeDetection, InitialFilesRestore, RestoreError, RestoreTree, TreeMark,
};
use crate::services::agent_filesystem_snapshots::{ConfirmOutcome, ConfirmSnapshot, StoreRestore};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::UsableAutomaticSnapshot;
use golem_common::model::oplog::{FilesystemSnapshotName, OplogIndex};
use std::path::Path;
use std::sync::Weak;

/// Where the confirmed filesystem snapshot of a worker came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmedBaseline {
    /// A periodic snapshot record: a start restored it, or a confirmation confirmed it.
    Periodic,
    /// The manual-update record at the index, which a start restored.
    ManualUpdate(OplogIndex),
}

/// The last confirmed filesystem snapshot of a worker, with the mark of its tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfirmedFilesystemSnapshot {
    pub(crate) name: FilesystemSnapshotName,
    pub(crate) mark: TreeMark,
    pub(crate) baseline: ConfirmedBaseline,
}

/// What a worker knows about the filesystem snapshots of its current generation.
#[derive(Clone, Debug, Default)]
pub(crate) struct FilesystemSnapshotSlot {
    /// The mark of the baseline of the current generation, or `None` before the first start.
    generation: Option<TreeMark>,
    /// The last confirmed filesystem snapshot of the current generation.
    confirmed: Option<ConfirmedFilesystemSnapshot>,
}

impl FilesystemSnapshotSlot {
    /// The slot of a start with the baseline `mark`. A start that restored a named snapshot gives
    /// it as `restored`.
    pub(crate) fn at_start(
        mark: TreeMark,
        restored: Option<(FilesystemSnapshotName, ConfirmedBaseline)>,
    ) -> Self {
        Self {
            generation: Some(mark),
            confirmed: restored.map(|(name, baseline)| ConfirmedFilesystemSnapshot {
                name,
                mark,
                baseline,
            }),
        }
    }

    /// Records the confirmation of `name`, whose capture has `mark`. A confirmation of another
    /// generation changes nothing.
    pub(crate) fn confirm(&mut self, name: FilesystemSnapshotName, mark: TreeMark) {
        if self
            .generation
            .is_some_and(|generation| generation.same_generation(&mark))
        {
            self.confirmed = Some(ConfirmedFilesystemSnapshot {
                name,
                mark,
                baseline: ConfirmedBaseline::Periodic,
            });
        }
    }

    pub(crate) fn confirmed(&self) -> Option<&ConfirmedFilesystemSnapshot> {
        self.confirmed.as_ref()
    }

    /// Whether `mark` is a mark of the current generation.
    pub(crate) fn is_generation(&self, mark: &TreeMark) -> bool {
        self.generation
            .is_some_and(|generation| generation.same_generation(mark))
    }
}

/// Gives the confirmed snapshot that a capture compares with: the confirmed snapshot of the slot
/// while a start would restore its name now. `selected` is the automatic snapshot record that a
/// start selects now, and `last_manual_update` the index of the manual-update baseline.
pub(crate) fn since(
    confirmed: Option<&ConfirmedFilesystemSnapshot>,
    selected: Option<&UsableAutomaticSnapshot>,
    last_manual_update: Option<OplogIndex>,
) -> Option<ConfirmedFilesystemSnapshot> {
    let confirmed = confirmed?;
    let selected_now = match (selected, confirmed.baseline) {
        (Some(selected), _) => selected.filesystem_snapshot.as_ref() == Some(&confirmed.name),
        (None, ConfirmedBaseline::ManualUpdate(index)) => last_manual_update == Some(index),
        (None, ConfirmedBaseline::Periodic) => false,
    };
    selected_now.then(|| confirmed.clone())
}

/// What a capture found, without the copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureFinding {
    Unchanged,
    InitialFiles,
    Captured(ChangeDetection),
}

/// The name that a periodic snapshot record gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PeriodicRecord {
    /// The record has no name. Nothing is uploaded.
    WithoutName,
    /// The record has the name of the admission, and the capture is uploaded with `parent`.
    Uploaded {
        parent: Option<(FilesystemSnapshotName, StoreChangeDetection)>,
    },
    /// The record has the confirmed name, and its confirmation record follows it at once.
    Reused(FilesystemSnapshotName),
}

/// Decides the record of a periodic snapshot from the finding of the capture and the confirmed
/// snapshot that the capture compared with.
pub(crate) fn plan_periodic_record(
    finding: CaptureFinding,
    since: Option<&ConfirmedFilesystemSnapshot>,
) -> PeriodicRecord {
    match (finding, since) {
        (CaptureFinding::Unchanged, Some(since)) => PeriodicRecord::Reused(since.name.clone()),
        (CaptureFinding::Unchanged, None) | (CaptureFinding::InitialFiles, _) => {
            PeriodicRecord::WithoutName
        }
        (CaptureFinding::Captured(ChangeDetection::SizeMtime), Some(since)) => {
            PeriodicRecord::Uploaded {
                parent: Some((since.name.clone(), StoreChangeDetection::SizeMtime)),
            }
        }
        (CaptureFinding::Captured(_), _) => PeriodicRecord::Uploaded { parent: None },
    }
}

/// The restore of a start: a filesystem snapshot of the store, or the initial files of the
/// source revision of a manual update without a name.
pub(crate) enum StartRestore {
    Store(StoreRestore),
    InitialFiles(InitialFilesRestore),
}

impl RestoreTree for StartRestore {
    async fn restore(self, into: &Path) -> Result<(), RestoreError> {
        match self {
            Self::Store(restore) => restore.restore(into).await,
            Self::InitialFiles(restore) => restore.restore(into).await,
        }
    }
}

/// The confirmer of one upload. It holds a weak handle to the worker, so an upload never keeps a
/// worker in memory, and the mark of the capture, which a confirmation gives to the slot.
pub(crate) struct WorkerConfirmer<Ctx: WorkerCtx> {
    worker: Weak<Worker<Ctx>>,
    mark: TreeMark,
}

impl<Ctx: WorkerCtx> WorkerConfirmer<Ctx> {
    pub(crate) fn new(worker: Weak<Worker<Ctx>>, mark: TreeMark) -> Self {
        Self { worker, mark }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> ConfirmSnapshot for WorkerConfirmer<Ctx> {
    async fn confirm(&self, name: &FilesystemSnapshotName) -> ConfirmOutcome {
        let Some(worker) = self.worker.upgrade() else {
            return ConfirmOutcome::Deferred;
        };
        match worker
            .confirm_filesystem_snapshot(name.clone(), self.mark)
            .await
        {
            ConfirmationReply::Confirmed => {
                worker.record_confirmed_filesystem_snapshot(name.clone(), self.mark);
                ConfirmOutcome::Confirmed
            }
            ConfirmationReply::Superseded => ConfirmOutcome::Superseded,
            ConfirmationReply::Deferred => ConfirmOutcome::Deferred,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// Gives a mark of a new generation and a later mark of the same generation.
    async fn marks() -> (TreeMark, TreeMark) {
        crate::services::agent_filesystem::test_tree_marks().await
    }

    fn confirmed(name: &FilesystemSnapshotName, mark: TreeMark) -> ConfirmedFilesystemSnapshot {
        ConfirmedFilesystemSnapshot {
            name: name.clone(),
            mark,
            baseline: ConfirmedBaseline::Periodic,
        }
    }

    fn selected(name: Option<&FilesystemSnapshotName>) -> UsableAutomaticSnapshot {
        UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(10),
            component_revision: golem_common::model::component::ComponentRevision::INITIAL,
            filesystem_snapshot: name.cloned(),
        }
    }

    #[test]
    async fn the_capture_compares_with_the_confirmed_snapshot_only_while_a_start_selects_it() {
        let (mark, _) = marks().await;
        let name = FilesystemSnapshotName::periodic();
        let other = FilesystemSnapshotName::periodic();
        let periodic = confirmed(&name, mark);
        let manual = ConfirmedFilesystemSnapshot {
            baseline: ConfirmedBaseline::ManualUpdate(OplogIndex::from_u64(4)),
            ..periodic.clone()
        };

        let cases = [
            since(Some(&periodic), Some(&selected(Some(&name))), None).is_some(),
            since(Some(&periodic), Some(&selected(Some(&other))), None).is_some(),
            since(Some(&periodic), Some(&selected(None)), None).is_some(),
            since(Some(&periodic), None, None).is_some(),
            since(Some(&manual), None, Some(OplogIndex::from_u64(4))).is_some(),
            since(Some(&manual), None, Some(OplogIndex::from_u64(5))).is_some(),
            since(
                Some(&manual),
                Some(&selected(None)),
                Some(OplogIndex::from_u64(4)),
            )
            .is_some(),
            since(None, Some(&selected(Some(&name))), None).is_some(),
        ];

        assert_eq!(
            cases,
            [true, false, false, false, true, false, false, false]
        );
    }

    #[test]
    async fn the_record_of_a_periodic_snapshot_follows_the_finding_of_the_capture() {
        let (mark, _) = marks().await;
        let name = FilesystemSnapshotName::periodic();
        let since = confirmed(&name, mark);

        let cases = [
            plan_periodic_record(CaptureFinding::Unchanged, Some(&since)),
            plan_periodic_record(CaptureFinding::InitialFiles, Some(&since)),
            plan_periodic_record(CaptureFinding::InitialFiles, None),
            plan_periodic_record(
                CaptureFinding::Captured(ChangeDetection::SizeMtime),
                Some(&since),
            ),
            plan_periodic_record(
                CaptureFinding::Captured(ChangeDetection::Full),
                Some(&since),
            ),
            plan_periodic_record(CaptureFinding::Captured(ChangeDetection::Full), None),
        ];

        assert_eq!(
            cases,
            [
                PeriodicRecord::Reused(name.clone()),
                PeriodicRecord::WithoutName,
                PeriodicRecord::WithoutName,
                PeriodicRecord::Uploaded {
                    parent: Some((name, StoreChangeDetection::SizeMtime)),
                },
                PeriodicRecord::Uploaded { parent: None },
                PeriodicRecord::Uploaded { parent: None },
            ]
        );
    }

    #[test]
    async fn a_confirmation_of_another_generation_leaves_the_slot_unchanged() {
        let (first, later) = marks().await;
        let (other_generation, _) = marks().await;
        let restored = FilesystemSnapshotName::periodic();
        let confirmed_now = FilesystemSnapshotName::periodic();
        let mut slot = FilesystemSnapshotSlot::at_start(
            first,
            Some((restored.clone(), ConfirmedBaseline::Periodic)),
        );

        slot.confirm(FilesystemSnapshotName::periodic(), other_generation);
        let after_other = slot.confirmed().map(|confirmed| confirmed.name.clone());
        slot.confirm(confirmed_now.clone(), later);
        let after_own = slot.confirmed().cloned();

        assert_eq!(after_other, Some(restored));
        assert_eq!(after_own, Some(confirmed(&confirmed_now, later)));
    }
}
