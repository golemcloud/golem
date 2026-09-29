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
use crate::services::agent_filesystem_snapshots::SnapshotsDisabled;
use crate::services::agent_filesystem_snapshots::{
    Admission, Confirm, ConfirmOutcome, SnapshotSkip, StoreRestore, UploadNowError,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    FilesystemSnapshotName, OplogIndex, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::{AgentId, Timestamp, UsableAutomaticSnapshot};
use golem_service_base::error::worker_executor::WorkerExecutorError;
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

/// The baseline that a start selected, with its restore.
pub(crate) struct StartBaseline {
    pub(crate) kind: BaselineKind,
    pub(crate) restore: Option<StartRestore>,
}

/// The record that the baseline of a start comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BaselineKind {
    /// No snapshot record: the initial files of the replay revision.
    InitialFiles,
    /// The automatic snapshot record at `index`.
    Periodic {
        index: OplogIndex,
        name: Option<FilesystemSnapshotName>,
    },
    /// The manual-update record at `index`, which is still pending when `pending` is true.
    ManualUpdate {
        index: OplogIndex,
        target_revision: ComponentRevision,
        pending: bool,
        name: Option<FilesystemSnapshotName>,
    },
}

impl BaselineKind {
    /// The named filesystem snapshot that the baseline restored.
    pub(crate) fn restored(&self) -> Option<(FilesystemSnapshotName, ConfirmedBaseline)> {
        match self {
            Self::InitialFiles => None,
            Self::Periodic { name, .. } => {
                name.clone().map(|name| (name, ConfirmedBaseline::Periodic))
            }
            Self::ManualUpdate { index, name, .. } => name
                .clone()
                .map(|name| (name, ConfirmedBaseline::ManualUpdate(*index))),
        }
    }
}

/// The component revision whose initial files a manual-update baseline without a name seeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceRevision {
    /// The current revision, while the update is pending.
    Current,
    /// The revision of the agent just before the update at this time.
    Before(Timestamp),
}

/// What a start does to get its baseline.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BaselineStep {
    /// The baseline is known. `restore` names the filesystem snapshot that the start restores
    /// from the store.
    Ready {
        kind: BaselineKind,
        restore: Option<FilesystemSnapshotName>,
    },
    /// A manual-update record without a name: the start needs the initial files of `source`.
    NeedsSourceFiles {
        kind: BaselineKind,
        source: SourceRevision,
    },
    /// The record names a filesystem snapshot, and this executor keeps none.
    Disabled { kind: BaselineKind },
}

/// Plans the baseline of a start from the selected automatic snapshot record, else from the
/// manual-update record with whether it is still pending, else from the initial files. `enabled`
/// tells whether this executor keeps filesystem snapshots.
pub(crate) fn plan_start_baseline(
    automatic: Option<&UsableAutomaticSnapshot>,
    manual: Option<(TimestampedUpdateDescription, bool)>,
    enabled: bool,
) -> BaselineStep {
    let named = |kind: BaselineKind, name: Option<FilesystemSnapshotName>| match name {
        Some(_) if !enabled => BaselineStep::Disabled { kind },
        restore => BaselineStep::Ready { kind, restore },
    };
    if let Some(snapshot) = automatic {
        return named(
            BaselineKind::Periodic {
                index: snapshot.index,
                name: snapshot.filesystem_snapshot.clone(),
            },
            snapshot.filesystem_snapshot.clone(),
        );
    }
    match manual {
        Some((
            TimestampedUpdateDescription {
                timestamp,
                oplog_index,
                description:
                    UpdateDescription::SnapshotBased {
                        target_revision,
                        filesystem_snapshot,
                        ..
                    },
            },
            pending,
        )) => {
            let kind = BaselineKind::ManualUpdate {
                index: oplog_index,
                target_revision,
                pending,
                name: filesystem_snapshot.clone(),
            };
            match filesystem_snapshot {
                Some(name) => named(kind, Some(name)),
                None => BaselineStep::NeedsSourceFiles {
                    kind,
                    source: if pending {
                        SourceRevision::Current
                    } else {
                        SourceRevision::Before(timestamp)
                    },
                },
            }
        }
        _ => BaselineStep::Ready {
            kind: BaselineKind::InitialFiles,
            restore: None,
        },
    }
}

/// The error of a start whose baseline names a filesystem snapshot on an executor that keeps
/// none. A manual-update baseline fails the start with a visible cause.
pub(crate) fn baseline_disabled_error(
    kind: &BaselineKind,
    agent_id: &AgentId,
) -> WorkerExecutorError {
    match kind {
        BaselineKind::ManualUpdate { .. } => WorkerExecutorError::failed_to_resume_worker(
            agent_id.clone(),
            WorkerExecutorError::invalid_request(SnapshotsDisabled.to_string()),
        ),
        BaselineKind::InitialFiles | BaselineKind::Periodic { .. } => {
            WorkerExecutorError::runtime(SnapshotsDisabled.to_string())
        }
    }
}

/// What a start does when its baseline failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BaselineFailure {
    /// The filesystem snapshot of the automatic snapshot record at the index does not restore.
    /// The start skips the record and restarts.
    SkipPeriodic(OplogIndex),
    /// A conflict of the initial-file rule at the start of a pending manual update. The start
    /// records a failed update with the message and restarts on the current revision.
    RecordFailedUpdate {
        target: ComponentRevision,
        message: String,
    },
    /// The same conflict on a lost shard. Nothing is written.
    ShardLost,
    /// A manual-update baseline that does not restore, and whose error allows no retry. The
    /// start fails with the message as a visible cause.
    FailVisibly(String),
    /// Any other failure of the reconstruction.
    Reconstruction,
}

/// Classifies the failure `error` of the baseline `kind`. `lost_shard` tells whether the shard
/// of the agent is lost.
pub(crate) fn classify_baseline_failure(
    kind: &BaselineKind,
    error: &crate::services::agent_filesystem::Error,
    lost_shard: bool,
) -> BaselineFailure {
    use crate::services::agent_filesystem::Error;
    match (kind, error) {
        (BaselineKind::Periodic { index, .. }, Error::Baseline(_)) => {
            BaselineFailure::SkipPeriodic(*index)
        }
        (
            BaselineKind::ManualUpdate {
                target_revision,
                pending: true,
                ..
            },
            Error::InitialFileConflict(conflict),
        ) => {
            if lost_shard {
                BaselineFailure::ShardLost
            } else {
                BaselineFailure::RecordFailedUpdate {
                    target: *target_revision,
                    message: conflict.to_string(),
                }
            }
        }
        (BaselineKind::ManualUpdate { .. }, Error::Baseline(error)) if !error.retryable => {
            BaselineFailure::FailVisibly(error.to_string())
        }
        _ => BaselineFailure::Reconstruction,
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

    fn manual(
        name: Option<&FilesystemSnapshotName>,
        pending: bool,
    ) -> (TimestampedUpdateDescription, bool) {
        (
            TimestampedUpdateDescription {
                timestamp: Timestamp::from(1_000),
                oplog_index: OplogIndex::from_u64(7),
                description: UpdateDescription::SnapshotBased {
                    target_revision: ComponentRevision::new(3).unwrap(),
                    payload: golem_common::model::oplog::OplogPayload::Inline(Box::new(vec![])),
                    mime_type: "application/octet-stream".to_string(),
                    filesystem_snapshot: name.cloned(),
                },
            },
            pending,
        )
    }

    fn manual_kind(name: Option<&FilesystemSnapshotName>, pending: bool) -> BaselineKind {
        BaselineKind::ManualUpdate {
            index: OplogIndex::from_u64(7),
            target_revision: ComponentRevision::new(3).unwrap(),
            pending,
            name: name.cloned(),
        }
    }

    #[test]
    async fn a_start_plans_its_baseline_from_the_automatic_record_then_the_manual_update() {
        let name = FilesystemSnapshotName::periodic();
        let update = FilesystemSnapshotName::update();
        let periodic = BaselineKind::Periodic {
            index: OplogIndex::from_u64(10),
            name: Some(name.clone()),
        };
        let automatic = selected(Some(&name));
        let automatic_kind = BaselineKind::Periodic {
            index: OplogIndex::from_u64(10),
            name: Some(name.clone()),
        };
        let automatic_without_name = selected(None);
        let not_snapshot_based = (
            TimestampedUpdateDescription {
                timestamp: Timestamp::from(1_000),
                oplog_index: OplogIndex::from_u64(7),
                description: UpdateDescription::Automatic {
                    target_revision: ComponentRevision::new(3).unwrap(),
                },
            },
            true,
        );

        assert_eq!(
            [
                plan_start_baseline(Some(&automatic), Some(manual(Some(&update), true)), true),
                plan_start_baseline(Some(&automatic), None, false),
                plan_start_baseline(Some(&automatic_without_name), None, false),
                plan_start_baseline(None, Some(manual(Some(&update), false)), true),
                plan_start_baseline(None, Some(manual(Some(&update), true)), false),
                plan_start_baseline(None, Some(manual(None, true)), false),
                plan_start_baseline(None, Some(manual(None, false)), true),
                plan_start_baseline(None, Some(not_snapshot_based), true),
                plan_start_baseline(None, None, true),
            ],
            [
                BaselineStep::Ready {
                    kind: periodic.clone(),
                    restore: Some(name.clone()),
                },
                BaselineStep::Disabled {
                    kind: automatic_kind
                },
                BaselineStep::Ready {
                    kind: BaselineKind::Periodic {
                        index: OplogIndex::from_u64(10),
                        name: None
                    },
                    restore: None,
                },
                BaselineStep::Ready {
                    kind: manual_kind(Some(&update), false),
                    restore: Some(update.clone()),
                },
                BaselineStep::Disabled {
                    kind: manual_kind(Some(&update), true)
                },
                BaselineStep::NeedsSourceFiles {
                    kind: manual_kind(None, true),
                    source: SourceRevision::Current,
                },
                BaselineStep::NeedsSourceFiles {
                    kind: manual_kind(None, false),
                    source: SourceRevision::Before(Timestamp::from(1_000)),
                },
                BaselineStep::Ready {
                    kind: BaselineKind::InitialFiles,
                    restore: None
                },
                BaselineStep::Ready {
                    kind: BaselineKind::InitialFiles,
                    restore: None
                },
            ]
        );
    }

    #[test]
    async fn a_baseline_on_an_executor_without_snapshots_fails_visibly_only_for_a_manual_update() {
        let agent_id = AgentId {
            component_id: golem_common::model::component::ComponentId::new(),
            agent_id: "disabled".to_string(),
        };
        let periodic = baseline_disabled_error(
            &BaselineKind::Periodic {
                index: OplogIndex::from_u64(10),
                name: None,
            },
            &agent_id,
        );
        let manual = baseline_disabled_error(&manual_kind(None, true), &agent_id);

        assert_eq!(
            periodic,
            WorkerExecutorError::runtime(SnapshotsDisabled.to_string())
        );
        assert_eq!(
            manual,
            WorkerExecutorError::failed_to_resume_worker(
                agent_id,
                WorkerExecutorError::invalid_request(SnapshotsDisabled.to_string())
            )
        );
    }

    #[test]
    async fn a_failed_baseline_skips_a_periodic_record_or_fails_or_records_a_manual_update() {
        use crate::services::agent_filesystem::{Error, InitialFileConflict, RestoreError};
        let restore = |retryable| {
            Error::Baseline(Box::new(RestoreError {
                retryable,
                source: anyhow::anyhow!("restore failed"),
            }))
        };
        let conflict = InitialFileConflict::occupied(Path::new("e/f"));
        let conflicting = || Error::InitialFileConflict(Box::new(conflict.clone()));
        let periodic = BaselineKind::Periodic {
            index: OplogIndex::from_u64(10),
            name: None,
        };

        assert_eq!(
            [
                classify_baseline_failure(&periodic, &restore(true), false),
                classify_baseline_failure(&periodic, &conflicting(), false),
                classify_baseline_failure(&manual_kind(None, true), &conflicting(), false),
                classify_baseline_failure(&manual_kind(None, true), &conflicting(), true),
                classify_baseline_failure(&manual_kind(None, false), &conflicting(), false),
                classify_baseline_failure(&manual_kind(None, true), &restore(false), false),
                classify_baseline_failure(&manual_kind(None, true), &restore(true), false),
                classify_baseline_failure(&BaselineKind::InitialFiles, &restore(false), false),
            ],
            [
                BaselineFailure::SkipPeriodic(OplogIndex::from_u64(10)),
                BaselineFailure::Reconstruction,
                BaselineFailure::RecordFailedUpdate {
                    target: ComponentRevision::new(3).unwrap(),
                    message: conflict.to_string(),
                },
                BaselineFailure::ShardLost,
                BaselineFailure::Reconstruction,
                BaselineFailure::FailVisibly(restore(false).to_string()),
                BaselineFailure::Reconstruction,
                BaselineFailure::Reconstruction,
            ]
        );
    }

    #[test]
    async fn only_a_non_empty_set_of_read_only_initial_files_restores() {
        use golem_common::model::agent::AgentFileContentHash;
        use golem_common::model::component::{
            AgentFilePath, AgentFilePermissions, InitialAgentFile,
        };
        let file = |permissions| InitialAgentFile {
            content_hash: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
            path: AgentFilePath::from_abs_str("/file").unwrap(),
            permissions,
            size: 1,
        };
        assert_eq!(
            [
                InitialFilesRestore::of_read_only(Box::new([])).is_some(),
                InitialFilesRestore::of_read_only(Box::new([file(AgentFilePermissions::ReadOnly)]))
                    .is_some(),
                InitialFilesRestore::of_read_only(Box::new([
                    file(AgentFilePermissions::ReadOnly),
                    file(AgentFilePermissions::ReadWrite),
                ]))
                .is_some(),
            ],
            [false, true, false]
        );
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
