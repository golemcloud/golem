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
use crate::filesystem_snapshot::ChangeDetection as StoreChangeDetection;
use crate::services::agent_filesystem::{
    CaptureOutcome, ChangeDetection, FilesystemCapture, InitialFilesRestore, RestoreError,
    RestoreTree, TreeMark,
};
use crate::services::agent_filesystem_snapshots::{
    Admission, Confirm, ConfirmOutcome, SnapshotSkip, StoreRestore, UploadNowError,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::UsableAutomaticSnapshot;
use golem_common::model::oplog::{FilesystemSnapshotName, OplogIndex};
use std::path::Path;
use std::sync::Arc;
use std::sync::Weak;
use uuid::Uuid;

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

/// The slot of the generation that runs, shared by the running worker and its invocation loop.
/// It is empty before the start of a generation and after its end.
pub(crate) type SnapshotSlot = std::sync::Arc<std::sync::Mutex<Option<FilesystemSnapshotSlot>>>;

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
    /// No record is written: the capture found no change against a confirmed snapshot that it
    /// did not have.
    Skipped,
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
        (CaptureFinding::Unchanged, None) => PeriodicRecord::Skipped,
        (CaptureFinding::InitialFiles, _) => PeriodicRecord::WithoutName,
        (CaptureFinding::Captured(ChangeDetection::SizeMtime), Some(since)) => {
            PeriodicRecord::Uploaded {
                parent: Some((since.name.clone(), StoreChangeDetection::SizeMtime)),
            }
        }
        (CaptureFinding::Captured(_), _) => PeriodicRecord::Uploaded { parent: None },
    }
}

/// What a periodic snapshot writes: the name of its record, the name whose confirmation record
/// follows the record at once, and the upload that starts after the record commits.
pub(crate) struct PeriodicPlan {
    name: Option<FilesystemSnapshotName>,
    confirmed_at_once: Option<FilesystemSnapshotName>,
    upload: Option<PendingUpload>,
}

/// The upload of a periodic snapshot whose record is not written yet. It is consumed once, by
/// [`PeriodicPlan::submit`] or by [`PeriodicPlan::abandon`].
struct PendingUpload {
    admission: Admission,
    tree: FilesystemCapture,
    mark: TreeMark,
    parent: Option<(FilesystemSnapshotName, StoreChangeDetection)>,
}

impl PeriodicPlan {
    /// Plans the record of a periodic snapshot from the admission, the confirmed snapshot that
    /// the capture compared with, and the outcome of the capture, as [`plan_periodic_record`]
    /// decides. Without an admission the record has no name. Gives `None` when no record is
    /// written. The admission is dropped unless the record uploads the capture.
    pub(crate) fn new(
        capture: Option<(
            Admission,
            Option<ConfirmedFilesystemSnapshot>,
            CaptureOutcome,
        )>,
    ) -> Option<Self> {
        let without_name = Self {
            name: None,
            confirmed_at_once: None,
            upload: None,
        };
        let Some((admission, since, outcome)) = capture else {
            return Some(without_name);
        };
        let finding = match &outcome {
            CaptureOutcome::Unchanged => CaptureFinding::Unchanged,
            CaptureOutcome::InitialFiles => CaptureFinding::InitialFiles,
            CaptureOutcome::Captured { detection, .. } => CaptureFinding::Captured(*detection),
        };
        let record = plan_periodic_record(finding, since.as_ref());
        match outcome {
            CaptureOutcome::Captured { capture, mark, .. } => Some(Self {
                name: Some(admission.name().clone()),
                confirmed_at_once: None,
                upload: Some(PendingUpload {
                    admission,
                    tree: capture,
                    mark,
                    parent: match record {
                        PeriodicRecord::Uploaded { parent } => parent,
                        PeriodicRecord::Skipped
                        | PeriodicRecord::WithoutName
                        | PeriodicRecord::Reused(_) => None,
                    },
                }),
            }),
            CaptureOutcome::Unchanged | CaptureOutcome::InitialFiles => match record {
                PeriodicRecord::Skipped => None,
                PeriodicRecord::Reused(name) => Some(Self {
                    name: Some(name.clone()),
                    confirmed_at_once: Some(name),
                    upload: None,
                }),
                PeriodicRecord::WithoutName | PeriodicRecord::Uploaded { .. } => Some(without_name),
            },
        }
    }

    /// The filesystem snapshot name of the record.
    pub(crate) fn name(&self) -> Option<FilesystemSnapshotName> {
        self.name.clone()
    }

    /// The name whose confirmation record follows the record at once, in one append.
    pub(crate) fn confirmed_at_once(&self) -> Option<FilesystemSnapshotName> {
        self.confirmed_at_once.clone()
    }

    /// Starts the upload of a written record. Its confirmation goes to `worker`.
    pub(crate) fn submit<Ctx: WorkerCtx>(self, worker: &Arc<Worker<Ctx>>) {
        if let Some(PendingUpload {
            admission,
            tree,
            mark,
            parent,
        }) = self.upload
        {
            admission.submit(
                tree.into(),
                parent,
                confirm_by(Arc::downgrade(worker), mark),
            );
        }
    }

    /// Drops the admission and discards the capture of a record that was not written.
    pub(crate) async fn abandon(self) {
        if let Some(PendingUpload {
            admission, tree, ..
        }) = self.upload
        {
            drop(admission);
            if let Err(error) = tree.discard().await {
                tracing::warn!("Failed to discard a filesystem capture: {error}");
            }
        }
    }
}

/// What a manual update does when a terminal interrupt stopped it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateStop {
    /// The shard is lost. Nothing is written, and the update stays pending for the new owner.
    WriteNothing,
    /// The update fails.
    FailUpdate,
}

/// What a manual update does when a terminal interrupt stopped it, with `lost_shard` telling
/// whether the shard of the agent is lost.
pub(crate) fn update_stop(lost_shard: bool) -> UpdateStop {
    if lost_shard {
        UpdateStop::WriteNothing
    } else {
        UpdateStop::FailUpdate
    }
}

/// Why the filesystem snapshot of a manual update was not uploaded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UpdateUploadFailure {
    /// A terminal interrupt stopped the upload.
    Interrupted,
    /// The upload failed, with the details of the failed update.
    Failed(String),
}

impl UpdateUploadFailure {
    /// The details of the failed update.
    pub(crate) fn details(self) -> String {
        match self {
            Self::Interrupted => {
                "the update was interrupted while it uploaded the filesystem snapshot".to_string()
            }
            Self::Failed(details) => details,
        }
    }
}

/// Classifies a failed upload of a manual update. A failure while a terminal interrupt waits
/// counts as interrupted, because the interrupt stops the save.
pub(crate) fn update_upload_failure(
    error: &UploadNowError,
    terminal_pending: bool,
) -> UpdateUploadFailure {
    if terminal_pending {
        UpdateUploadFailure::Interrupted
    } else {
        UpdateUploadFailure::Failed(format!(
            "failed to upload the filesystem snapshot for the update: {error}"
        ))
    }
}

/// The details of a manual update that fails because a terminal interrupt stopped the wait for a
/// running upload.
pub(crate) const UPDATE_WAIT_INTERRUPTED: &str = "the update was interrupted while it waited for an upload of a filesystem snapshot of the agent";

/// The details of a manual update that fails because its admission gave `skip`.
pub(crate) fn update_refused(skip: SnapshotSkip) -> String {
    format!("cannot take a filesystem snapshot for the update: {skip}")
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

/// The confirmation of one upload. It holds a weak handle to the worker, so an upload never keeps
/// a worker in memory, and the mark of the capture, which a confirmation gives to the slot. A
/// worker that is gone gives `Deferred`.
pub(crate) fn confirm_by<Ctx: WorkerCtx>(worker: Weak<Worker<Ctx>>, mark: TreeMark) -> Confirm {
    Box::new(move |name| {
        Box::pin(async move {
            match worker.upgrade() {
                Some(worker) => worker.confirm_as(name, Confirmer::Running(mark)).await,
                None => ConfirmOutcome::Deferred,
            }
        })
    })
}

/// Who writes a confirmation record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Confirmer {
    /// The running instance whose capture has this mark.
    Running(TreeMark),
    /// The start with this attempt, before it selects its record.
    Start(Uuid),
}

/// What a confirmation sees of the instance of the worker, under the instance lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstanceView {
    /// The instance runs, as the top-level variant. `generation_matches` tells whether its
    /// generation is the generation of the confirmer.
    Running { generation_matches: bool },
    /// The instance waits for its permits with this start attempt.
    WaitingForPermit(Uuid),
    /// Any other state.
    Other,
}

/// Whether `who` may write a confirmation record now: only as the owner of the agent. A running
/// instance needs the generation of its capture. A start needs its own attempt and no pending
/// terminal interrupt. Both need an attached status, no retirement of the owner, and the
/// admission of the shard, which is asked last.
pub(crate) fn owner_gate(
    instance: InstanceView,
    who: &Confirmer,
    terminal_pending: bool,
    detached: bool,
    retiring: bool,
    admitted: impl FnOnce() -> bool,
) -> bool {
    let owner = match (instance, who) {
        (InstanceView::Running { generation_matches }, Confirmer::Running(_)) => generation_matches,
        (InstanceView::WaitingForPermit(attempt), Confirmer::Start(start)) => {
            attempt == *start && !terminal_pending
        }
        _ => false,
    };
    owner && !detached && !retiring && admitted()
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
            plan_periodic_record(CaptureFinding::Unchanged, None),
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
                PeriodicRecord::Skipped,
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
    async fn only_the_owner_of_the_agent_passes_the_gate_and_the_admission_is_asked_last() {
        let (mark, _) = marks().await;
        let attempt = Uuid::new_v4();
        let running = Confirmer::Running(mark);
        let start = Confirmer::Start(attempt);
        let matching = InstanceView::Running {
            generation_matches: true,
        };
        let gate = |instance, who: &Confirmer, terminal, detached, retiring, admitted: bool| {
            owner_gate(instance, who, terminal, detached, retiring, || admitted)
        };

        assert_eq!(
            [
                gate(matching, &running, false, false, false, true),
                gate(matching, &running, true, false, false, true),
                gate(
                    InstanceView::Running {
                        generation_matches: false
                    },
                    &running,
                    false,
                    false,
                    false,
                    true
                ),
                gate(matching, &start, false, false, false, true),
                gate(
                    InstanceView::WaitingForPermit(attempt),
                    &start,
                    false,
                    false,
                    false,
                    true
                ),
                gate(
                    InstanceView::WaitingForPermit(attempt),
                    &start,
                    true,
                    false,
                    false,
                    true
                ),
                gate(
                    InstanceView::WaitingForPermit(Uuid::new_v4()),
                    &start,
                    false,
                    false,
                    false,
                    true
                ),
                gate(
                    InstanceView::WaitingForPermit(attempt),
                    &running,
                    false,
                    false,
                    false,
                    true
                ),
                gate(InstanceView::Other, &running, false, false, false, true),
                gate(matching, &running, false, true, false, true),
                gate(matching, &running, false, false, true, true),
                gate(matching, &running, false, false, false, false),
            ],
            [
                true, true, false, false, true, false, false, false, false, false, false, false
            ]
        );
        let mut asked = false;
        owner_gate(InstanceView::Other, &running, false, false, false, || {
            asked = true;
            true
        });
        assert!(!asked);
    }

    #[test]
    async fn a_stopped_manual_update_writes_nothing_only_on_a_lost_shard() {
        assert_eq!(
            [update_stop(true), update_stop(false)],
            [UpdateStop::WriteNothing, UpdateStop::FailUpdate]
        );
    }

    #[test]
    async fn a_failed_update_upload_is_interrupted_while_a_terminal_interrupt_waits() {
        let store = UploadNowError::Store(crate::filesystem_snapshot::SnapshotStoreError::NotFound);
        assert_eq!(
            [
                update_upload_failure(&UploadNowError::Stopped, true),
                update_upload_failure(&store, true),
                update_upload_failure(&UploadNowError::Stopped, false),
                update_upload_failure(&store, false),
            ],
            [
                UpdateUploadFailure::Interrupted,
                UpdateUploadFailure::Interrupted,
                UpdateUploadFailure::Failed(
                    "failed to upload the filesystem snapshot for the update: the upload of the \
                     filesystem snapshot was stopped"
                        .to_string()
                ),
                UpdateUploadFailure::Failed(
                    "failed to upload the filesystem snapshot for the update: no complete \
                     filesystem snapshot has the name"
                        .to_string()
                ),
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
