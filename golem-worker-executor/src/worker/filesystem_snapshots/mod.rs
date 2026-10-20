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
use crate::filesystem_snapshot::AgentSnapshots;
use crate::filesystem_snapshot::ChangeDetection as StoreChangeDetection;
use crate::model::SnapshotSource;
use crate::services::agent_filesystem::{
    CaptureError, CaptureOutcome, ChangeDetection, FilesystemCapture, InitialFilesCheck,
    InitialFilesRestore, RestoreError, RestoreTree, TreeMark, WholeCapture,
};
use crate::services::agent_filesystem_snapshots::{
    Admission, Admitted, AgentFilesystemSnapshots, Confirm, Confirmation, SavedUpdate,
    SnapshotsDisabled, StoreRestore, UpdateAdmitted, UploadNowError,
};
use crate::services::oplog::OplogError;
use crate::worker::snapshot_selection::SelectedBaseline;
use crate::worker::start_outcome::BaselineRole;
use crate::workerctx::WorkerCtx;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{FilesystemSnapshotName, OplogIndex, UpdateDescription};
use golem_common::model::oplog::{OplogEntry, RawSnapshotData};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{AuthoritativeSnapshotKind, UsableAutomaticSnapshot};
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::Weak;
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

/// Where the confirmed filesystem snapshot of a worker came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmedBaseline {
    /// A periodic snapshot record: a start restored it, or a confirmation confirmed it.
    Periodic,
    /// The record at the index that is or becomes the authoritative baseline: a manual-update
    /// record, or the record that a snapshot-assisted update selected. A start restored it.
    Authoritative(OplogIndex),
}

/// The last confirmed filesystem snapshot of a worker, with the mark of its tree. A snapshot
/// without a name is the record of a tree of initial files, which needs no confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfirmedFilesystemSnapshot {
    pub(crate) name: Option<FilesystemSnapshotName>,
    pub(crate) mark: TreeMark,
    pub(crate) baseline: ConfirmedBaseline,
}

/// The filesystem snapshots of the generation that runs, shared by the running worker, its
/// invocation loop and the confirmations of its uploads. It is empty before the start of a
/// generation and after its end. Every write that is not a start goes through the generation
/// check of [`confirmed_slot`], so a confirmation or a record of an ended generation changes
/// nothing.
#[derive(Clone, Debug, Default)]
pub(crate) struct SnapshotSlot(std::sync::Arc<std::sync::Mutex<Option<FilesystemSnapshotSlot>>>);

impl SnapshotSlot {
    /// Starts the snapshots of a generation whose baseline has `mark`. A start that restored a
    /// named snapshot gives it as `restored`.
    pub(crate) fn start(
        &self,
        mark: TreeMark,
        restored: Option<(FilesystemSnapshotName, ConfirmedBaseline)>,
    ) {
        *self.lock() = Some(FilesystemSnapshotSlot::at_start(mark, restored));
    }

    /// Ends the snapshots of the generation that runs.
    pub(crate) fn end(&self) {
        self.lock().take();
    }

    /// The mark of the baseline of the generation that runs, while one runs.
    pub(crate) fn generation(&self) -> Option<TreeMark> {
        self.lock().as_ref().map(|slot| slot.generation)
    }

    /// Whether a capture with `mark` is of the generation that runs.
    pub(crate) fn owns(&self, mark: TreeMark) -> bool {
        self.lock()
            .as_ref()
            .is_some_and(|slot| same_generation(slot, mark))
    }

    /// Records the confirmation of `name`, or the written record without a name of a tree of
    /// initial files when `name` is `None`, whose capture has `mark`. Only the generation that
    /// took the capture takes it.
    pub(crate) fn record(&self, name: Option<&FilesystemSnapshotName>, mark: TreeMark) {
        let mut slot = self.lock();
        if let Some(next) = slot
            .as_ref()
            .and_then(|current| confirmed_slot(current, name, mark))
        {
            *slot = Some(next);
        }
    }

    /// The confirmed snapshot that a capture compares with now, as [`since`] decides from the
    /// periodic record `selected` that a start selects now and the index `authoritative` of the
    /// authoritative baseline.
    pub(crate) fn since(
        &self,
        selected: Option<&UsableAutomaticSnapshot>,
        authoritative: Option<OplogIndex>,
    ) -> Option<ConfirmedFilesystemSnapshot> {
        since(
            self.lock()
                .as_ref()
                .and_then(FilesystemSnapshotSlot::confirmed),
            selected,
            authoritative,
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<FilesystemSnapshotSlot>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The baselines that a start of a worker has now: the periodic record that it selects, and the
/// index of the authoritative baseline of the status. A periodic capture compares with the
/// confirmed snapshot of its slot only while one of them restores it.
#[derive(Clone, Debug)]
pub(crate) struct StartBaselines {
    pub(crate) periodic: Option<UsableAutomaticSnapshot>,
    pub(crate) authoritative: Option<OplogIndex>,
}

/// What a worker knows about the filesystem snapshots of its current generation.
#[derive(Clone, Debug)]
struct FilesystemSnapshotSlot {
    /// The mark of the baseline of the current generation.
    generation: TreeMark,
    /// The last confirmed filesystem snapshot of the current generation, or the last written
    /// record of a tree of initial files, which has no name.
    confirmed: Option<ConfirmedFilesystemSnapshot>,
}

impl FilesystemSnapshotSlot {
    /// The slot of a start with the baseline `mark`. A start that restored a named snapshot gives
    /// it as `restored`.
    fn at_start(
        mark: TreeMark,
        restored: Option<(FilesystemSnapshotName, ConfirmedBaseline)>,
    ) -> Self {
        Self {
            generation: mark,
            confirmed: restored.map(|(name, baseline)| ConfirmedFilesystemSnapshot {
                name: Some(name),
                mark,
                baseline,
            }),
        }
    }

    fn confirmed(&self) -> Option<&ConfirmedFilesystemSnapshot> {
        self.confirmed.as_ref()
    }
}

/// The slot after the confirmation of `name`, whose capture has `mark`, or `None` when the
/// capture is of another generation than `slot`. `name` is `None` for the written record of a
/// tree of initial files at `mark`. A confirmation or such a record counts only for the
/// generation that took its capture. The owner gate asks for the generation before the append,
/// and the append asks again when it sets the slot, because the loop starts a new generation
/// without the instance lock.
fn confirmed_slot(
    slot: &FilesystemSnapshotSlot,
    name: Option<&FilesystemSnapshotName>,
    mark: TreeMark,
) -> Option<FilesystemSnapshotSlot> {
    same_generation(slot, mark).then(|| FilesystemSnapshotSlot {
        generation: slot.generation,
        confirmed: Some(ConfirmedFilesystemSnapshot {
            name: name.cloned(),
            mark,
            baseline: ConfirmedBaseline::Periodic,
        }),
    })
}

/// Whether a capture with `mark` is of the generation of `slot`.
fn same_generation(slot: &FilesystemSnapshotSlot, mark: TreeMark) -> bool {
    slot.generation.same_generation(&mark)
}

/// Gives the confirmed snapshot that a capture compares with: the confirmed snapshot of the slot
/// while a start would restore its name now. A snapshot without a name matches a selected record
/// without a name. `selected` is the periodic record that a start selects now, and
/// `authoritative` the index of the authoritative baseline.
fn since(
    confirmed: Option<&ConfirmedFilesystemSnapshot>,
    selected: Option<&UsableAutomaticSnapshot>,
    authoritative: Option<OplogIndex>,
) -> Option<ConfirmedFilesystemSnapshot> {
    let confirmed = confirmed?;
    let selected_now = match (selected, confirmed.baseline) {
        (Some(selected), _) => selected.filesystem_snapshot == confirmed.name,
        (None, ConfirmedBaseline::Authoritative(index)) => authoritative == Some(index),
        (None, ConfirmedBaseline::Periodic) => false,
    };
    selected_now.then(|| confirmed.clone())
}

/// What a capture found. `Copy` is the copy that a capture made, which only a capture that
/// found a change has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureFinding<Copy> {
    Unchanged,
    InitialFiles {
        mark: TreeMark,
    },
    Captured {
        copy: Copy,
        detection: ChangeDetection,
    },
}

impl CaptureFinding<(FilesystemCapture, TreeMark)> {
    /// The finding of a capture outcome, with the copy and its mark.
    fn of(outcome: CaptureOutcome) -> Self {
        match outcome {
            CaptureOutcome::Unchanged => Self::Unchanged,
            CaptureOutcome::InitialFiles { mark } => Self::InitialFiles { mark },
            CaptureOutcome::Captured {
                capture,
                mark,
                detection,
            } => Self::Captured {
                copy: (capture, mark),
                detection,
            },
        }
    }
}

/// The record that a periodic snapshot writes. `Copy` is the copy that the record uploads.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PeriodicRecord<Copy> {
    /// The record has no name. Nothing is uploaded.
    WithoutName,
    /// The record has no name, and the tree at `mark` holds only initial files. Nothing is
    /// uploaded, and the slot keeps the mark once the record is written.
    InitialFiles { mark: TreeMark },
    /// The record has the name of the admission, and `copy` is uploaded with `parent`.
    Own {
        copy: Copy,
        parent: Option<(FilesystemSnapshotName, StoreChangeDetection)>,
    },
    /// The record has the confirmed name, and its confirmation record follows it at once.
    Reused(FilesystemSnapshotName),
}

/// Decides the record of a periodic snapshot from the finding of the capture and the confirmed
/// snapshot that the capture compared with. An unchanged tree reuses the name of that snapshot,
/// or writes a record without a name when that snapshot has none. Gives `None` when no record is
/// written: the capture found no change against a confirmed snapshot that it did not have.
fn plan_periodic_record<Copy>(
    finding: CaptureFinding<Copy>,
    since: Option<&ConfirmedFilesystemSnapshot>,
) -> Option<PeriodicRecord<Copy>> {
    let since_name = since.map(|since| since.name.as_ref());
    Some(match (finding, since_name) {
        (CaptureFinding::Unchanged, Some(Some(name))) => PeriodicRecord::Reused(name.clone()),
        (CaptureFinding::Unchanged, Some(None)) => PeriodicRecord::WithoutName,
        (CaptureFinding::Unchanged, None) => return None,
        (CaptureFinding::InitialFiles { mark }, _) => PeriodicRecord::InitialFiles { mark },
        (
            CaptureFinding::Captured {
                copy,
                detection: ChangeDetection::SizeMtime,
            },
            Some(Some(name)),
        ) => PeriodicRecord::Own {
            copy,
            parent: Some((name.clone(), StoreChangeDetection::SizeMtime)),
        },
        (CaptureFinding::Captured { copy, .. }, _) => PeriodicRecord::Own { copy, parent: None },
    })
}

impl<Copy> PeriodicRecord<Copy> {
    /// The same record with the copy that `with` makes of its copy.
    fn with_copy<Other>(self, with: impl FnOnce(Copy) -> Other) -> PeriodicRecord<Other> {
        match self {
            Self::WithoutName => PeriodicRecord::WithoutName,
            Self::InitialFiles { mark } => PeriodicRecord::InitialFiles { mark },
            Self::Own { copy, parent } => PeriodicRecord::Own {
                copy: with(copy),
                parent,
            },
            Self::Reused(name) => PeriodicRecord::Reused(name),
        }
    }
}

/// The record of a periodic snapshot on an executor without filesystem snapshots, from the check
/// of the tree: a record without a name when the tree holds only initial files, and no record
/// otherwise, because a start from the record would not get the files back.
fn checked_record<Copy>(check: InitialFilesCheck) -> Option<PeriodicRecord<Copy>> {
    match check {
        InitialFilesCheck::InitialFiles => Some(PeriodicRecord::WithoutName),
        InitialFilesCheck::Changed => None,
    }
}

/// What a periodic snapshot writes: the record that [`plan_periodic_record`] decides, with the
/// upload that starts after the record commits.
struct PeriodicPlan(PeriodicRecord<PendingUpload>);

/// The upload of a periodic snapshot whose record is not written yet. It is consumed once, by
/// [`PeriodicPlan::written`] or by [`PeriodicPlan::abandon`].
struct PendingUpload {
    admission: Admission,
    tree: FilesystemCapture,
    mark: TreeMark,
}

impl PeriodicPlan {
    /// Plans the record of a periodic snapshot from the admission, the confirmed snapshot that
    /// the capture compared with, and the outcome of the capture, as [`plan_periodic_record`]
    /// decides. Gives `None` when no record is written. The admission is dropped unless the record
    /// uploads the capture.
    fn captured(
        admission: Admission,
        since: Option<ConfirmedFilesystemSnapshot>,
        outcome: CaptureOutcome,
    ) -> Option<Self> {
        let record = plan_periodic_record(CaptureFinding::of(outcome), since.as_ref())?;
        Some(Self(record.with_copy(|(tree, mark)| PendingUpload {
            admission,
            tree,
            mark,
        })))
    }

    /// The filesystem snapshot name of the record.
    fn name(&self) -> Option<FilesystemSnapshotName> {
        match &self.0 {
            PeriodicRecord::Own { copy, .. } => Some(copy.admission.name().clone()),
            PeriodicRecord::Reused(name) => Some(name.clone()),
            PeriodicRecord::WithoutName | PeriodicRecord::InitialFiles { .. } => None,
        }
    }

    /// The name whose confirmation record follows the record at once, in one append.
    fn confirmed_at_once(&self) -> Option<FilesystemSnapshotName> {
        match &self.0 {
            PeriodicRecord::Reused(name) => Some(name.clone()),
            PeriodicRecord::WithoutName
            | PeriodicRecord::InitialFiles { .. }
            | PeriodicRecord::Own { .. } => None,
        }
    }

    /// Finishes a written record on `host`: a record of a tree of initial files gives its mark to
    /// the slot, and a record with the name of the admission starts its upload.
    fn written<Host: PeriodicSnapshotHost>(self, host: &Host) {
        match self.0 {
            PeriodicRecord::InitialFiles { mark } => host.initial_files_written(mark),
            PeriodicRecord::Own {
                copy:
                    PendingUpload {
                        admission,
                        tree,
                        mark,
                    },
                parent,
            } => admission.submit(tree.into(), parent, host.confirm(mark)),
            PeriodicRecord::WithoutName | PeriodicRecord::Reused(_) => {}
        }
    }

    /// Drops the admission and discards the capture of a record that was not written.
    async fn abandon(self) {
        if let PeriodicRecord::Own {
            copy: PendingUpload {
                admission, tree, ..
            },
            ..
        } = self.0
        {
            drop(admission);
            if let Err(error) = tree.discard().await {
                tracing::warn!(error = %error, "Failed to discard a filesystem capture");
            }
        }
    }
}

/// Why a periodic snapshot record did not reach the oplog.
#[derive(Debug)]
pub(crate) enum PeriodicFailure {
    /// The payload of the record was not made, with the details.
    Entry(String),
    /// The append or the commit of the record failed.
    Write(OplogError),
}

/// What a periodic snapshot needs from the invocation loop of the agent.
pub(crate) trait PeriodicSnapshotHost {
    /// What ends the snapshot when the save hook of the guest does not give a snapshot.
    type Stop;
    /// Runs the save hook of the guest.
    fn snapshot_guest(
        &mut self,
    ) -> impl Future<Output = Result<RawSnapshotData, Self::Stop>> + Send;
    /// The confirmed snapshot that a capture compares with now.
    fn since(&self) -> Option<ConfirmedFilesystemSnapshot>;
    /// Captures the tree against the mark `since`, and gives `None` when the capture failed.
    fn capture(
        &self,
        wait: Duration,
        since: Option<TreeMark>,
    ) -> impl Future<Output = Option<CaptureOutcome>> + Send;
    /// Checks whether the tree holds only initial files, and gives the error of a check that
    /// failed.
    fn check_initial_files(
        &self,
        wait: Duration,
    ) -> impl Future<Output = Result<InitialFilesCheck, CaptureError>> + Send;
    /// Makes the record of `snapshot` with the filesystem snapshot `name`, or gives the details
    /// when its payload is not made.
    fn entry(
        &self,
        snapshot: RawSnapshotData,
        name: Option<FilesystemSnapshotName>,
    ) -> impl Future<Output = Result<OplogEntry, String>> + Send;
    /// Appends `entry`, with the confirmation record of `confirmed_at_once` in the same append,
    /// commits and checkpoints the status.
    fn write(
        &self,
        confirmed_at_once: Option<FilesystemSnapshotName>,
        entry: OplogEntry,
    ) -> impl Future<Output = Result<(), OplogError>> + Send;
    /// The confirmation of an upload whose capture has `mark`.
    fn confirm(&self, mark: TreeMark) -> Confirm;
    /// Keeps the mark of a tree of initial files whose record without a name is written, so the
    /// next capture compares with it.
    fn initial_files_written(&self, mark: TreeMark);
}

/// How a periodic snapshot ended.
#[derive(Debug)]
pub(crate) enum PeriodicResult<Stop> {
    /// The loop continues: the record is written and its upload runs, or the snapshot was
    /// skipped.
    Continue,
    /// The save hook of the guest ended the snapshot.
    Guest(Stop),
    /// The record did not reach the oplog. The capture was discarded.
    NotWritten(PeriodicFailure),
}

/// Takes a periodic snapshot of the agent `agent`, in this order: admission, the
/// save hook of the guest, the capture, the record with the name, its commit and the checkpoint
/// of the status, then the upload. An admission that the service refuses skips the snapshot. A
/// disabled service checks the tree in place of the capture: a tree of initial files gives a
/// record without a name, and any other tree, or a check that fails, writes no record. A capture
/// that fails writes no record. A record that does not reach the oplog drops the admission and
/// discards the capture at one place. A written record of a tree of initial files gives its mark
/// to the slot.
pub(crate) async fn periodic_snapshot<Host: PeriodicSnapshotHost>(
    host: &mut Host,
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
) -> PeriodicResult<Host::Stop> {
    let admitted = match snapshots.admit_periodic(agent).await {
        Admitted::Upload(admission) => Admit::Upload(admission),
        Admitted::InitialFilesOnly { capture_wait } => Admit::InitialFilesOnly(capture_wait),
        Admitted::Skip(skip) => {
            tracing::debug!(reason = %skip, "Skipping periodic snapshot");
            return PeriodicResult::Continue;
        }
    };
    let snapshot = match host.snapshot_guest().await {
        Ok(snapshot) => snapshot,
        Err(stop) => return PeriodicResult::Guest(stop),
    };
    let plan = match admitted {
        Admit::Upload(admission) => {
            let since = host.since();
            match host
                .capture(
                    admission.capture_wait(),
                    since.as_ref().map(|since| since.mark),
                )
                .await
            {
                Some(outcome) => PeriodicPlan::captured(admission, since, outcome),
                None => return PeriodicResult::Continue,
            }
        }
        Admit::InitialFilesOnly(wait) => {
            let checked = host.check_initial_files(wait).await;
            if matches!(checked, Ok(InitialFilesCheck::Changed)) {
                tracing::debug!(
                    "Skipping periodic snapshot: the files of the agent differ from its initial \
                     files, and filesystem snapshots are disabled on this executor"
                );
            }
            checked_record(checked_tree(checked)).map(PeriodicPlan)
        }
    };
    let Some(plan) = plan else {
        return PeriodicResult::Continue;
    };
    let written = match host.entry(snapshot, plan.name()).await {
        Ok(entry) => host
            .write(plan.confirmed_at_once(), entry)
            .await
            .map_err(PeriodicFailure::Write),
        Err(details) => Err(PeriodicFailure::Entry(details)),
    };
    match written {
        Ok(()) => {
            plan.written(host);
            PeriodicResult::Continue
        }
        Err(failure) => {
            match &failure {
                PeriodicFailure::Entry(details) => {
                    tracing::warn!(error = %details, "Failed to make the periodic snapshot record")
                }
                // A refused write is a move of the shard of the agent to a new owner.
                PeriodicFailure::Write(error @ OplogError::Fenced(_)) => tracing::debug!(
                    error = %error,
                    "The periodic snapshot record was not written: the shard of the agent moved"
                ),
                PeriodicFailure::Write(
                    error @ (OplogError::Payload(_)
                    | OplogError::Maintenance(_)
                    | OplogError::ArchiveWrite(_)
                    | OplogError::Unreadable(_)),
                ) => {
                    tracing::warn!(error = %error, "Failed to append the periodic snapshot record")
                }
            }
            plan.abandon().await;
            PeriodicResult::NotWritten(failure)
        }
    }
}

/// What the filesystem part of a manual update needs from the invocation loop of the agent.
pub(crate) trait UpdateSnapshotHost {
    /// What ends the update when the save hook of the guest does not give a snapshot.
    type Stop;
    /// Runs the save hook of the guest.
    fn snapshot_guest(
        &mut self,
    ) -> impl Future<Output = Result<RawSnapshotData, Self::Stop>> + Send;
    /// Captures the whole tree, and gives `None` when the capture failed.
    fn capture_whole(&self, wait: Duration) -> impl Future<Output = Option<WholeCapture>> + Send;
    /// Checks whether the tree holds only initial files, and gives the error of a check that
    /// failed.
    fn check_initial_files(
        &self,
        wait: Duration,
    ) -> impl Future<Output = Result<InitialFilesCheck, CaptureError>> + Send;
    /// A receiver of whether a terminal interrupt waits for the agent.
    fn terminal(&self) -> watch::Receiver<bool>;
    /// A receiver of why the shard of the agent moved to another executor, once it has.
    fn lost_shard(&self) -> super::LostShard;
}

/// How the snapshot part of a manual update ended.
pub(crate) enum UpdateSnapshot<Stop> {
    /// The store holds the filesystem snapshot `name`, or the tree holds only initial files and
    /// `name` is `None`. `retention` runs after the update record commits.
    Saved {
        snapshot: RawSnapshotData,
        name: Option<FilesystemSnapshotName>,
        retention: Option<Box<SavedUpdate>>,
    },
    /// The update fails with the details.
    Fail(String),
    /// The shard is lost. Nothing is written, and the update stays pending for the new owner.
    WriteNothing,
    /// The save hook of the guest ended the update.
    Guest(Stop),
}

/// Takes the snapshots of a manual update of the agent `agent`, before the update record:
/// admission, the save hook of the guest, a capture of the whole tree, and the upload, which
/// the update waits for. An upload of a periodic snapshot of the agent can run at the
/// admission; the update waits for it once and asks again, so a frequent snapshot does not fail
/// the update. A terminal interrupt ends that wait or the upload and fails the update, except on
/// a lost shard: then nothing is written, and the update stays pending for the shard's new
/// owner. A disabled service checks the tree in place of the capture: a tree of initial files gives
/// a record without a name, and any other tree, or a check that fails, fails the update.
pub(crate) async fn update_snapshot<Host: UpdateSnapshotHost>(
    host: &mut Host,
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
) -> UpdateSnapshot<Host::Stop> {
    let admission = match snapshots.admit_update(agent, host.terminal()).await {
        UpdateAdmitted::Upload(admission) => Admit::Upload(admission),
        UpdateAdmitted::InitialFilesOnly { capture_wait } => Admit::InitialFilesOnly(capture_wait),
        UpdateAdmitted::Interrupted => {
            return interrupted_update(
                UpdateInterruption::Wait,
                host.lost_shard().borrow().is_some(),
            );
        }
        UpdateAdmitted::Skip(skip) => {
            return UpdateSnapshot::Fail(format!(
                "cannot take a filesystem snapshot for the update: {skip}"
            ));
        }
    };
    let snapshot = match host.snapshot_guest().await {
        Ok(snapshot) => snapshot,
        Err(stop) => return UpdateSnapshot::Guest(stop),
    };
    let admission = match admission {
        Admit::Upload(admission) => admission,
        Admit::InitialFilesOnly(wait) => {
            return checked_update(host.check_initial_files(wait).await, snapshot);
        }
    };
    let tree = match host.capture_whole(admission.capture_wait()).await {
        None => {
            return UpdateSnapshot::Fail(
                "failed to capture the agent filesystem for the update".to_string(),
            );
        }
        Some(WholeCapture::InitialFiles) => {
            return UpdateSnapshot::Saved {
                snapshot,
                name: None,
                retention: None,
            };
        }
        Some(WholeCapture::Captured { capture, .. }) => capture,
    };
    let name = admission.name().clone();
    // A terminal interrupt stops the upload, and the capture is discarded after the save returned.
    // An interrupted save writes no record: a snapshot that its publish still leaves has no
    // record, and nothing selects it. A lost shard also cancels the save before its publish.
    let terminal = host.terminal();
    match admission
        .upload_now(tree.into(), terminal.clone(), host.lost_shard())
        .await
    {
        Ok(saved) => UpdateSnapshot::Saved {
            snapshot,
            name: Some(name),
            retention: Some(Box::new(saved)),
        },
        Err(error) => failed_update_upload(&error, host.lost_shard().borrow().is_some()),
    }
}

/// What an admission gave, when it gave a snapshot.
enum Admit {
    /// The upload of a snapshot with a new name.
    Upload(Admission),
    /// A record without a name, only when the check of the tree finds only initial files. The
    /// check waits up to this time for open file calls.
    InitialFilesOnly(Duration),
}

/// A manual update on an executor without filesystem snapshots cannot take a snapshot, because
/// the tree of the agent holds more than its initial files.
pub(crate) const UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS: &str = "UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS";

/// The details of a failed update with [`UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS`]: the code, then its
/// text.
fn needs_filesystem_snapshots() -> String {
    format!(
        "{UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS}: cannot take a snapshot for the update: the files of \
         the agent differ from its initial files, and filesystem snapshots are disabled on this \
         executor"
    )
}

/// Why a manual update on an executor without filesystem snapshots fails when the check of the
/// tree failed. The cause of the failure follows it.
const UPDATE_CHECK_FAILED: &str = "failed to check the agent filesystem for the update";

/// The tree that a check found, where a check that failed counts as a changed tree: a start can
/// trust a record without a name only when the check found a tree of initial files.
fn checked_tree(check: Result<InitialFilesCheck, CaptureError>) -> InitialFilesCheck {
    check.unwrap_or(InitialFilesCheck::Changed)
}

/// How a manual update on an executor without filesystem snapshots ends after the check of the
/// tree: a tree of initial files gives a record without a name with `snapshot`, and any other
/// tree, or a check that failed, fails the update.
fn checked_update<Stop>(
    check: Result<InitialFilesCheck, CaptureError>,
    snapshot: RawSnapshotData,
) -> UpdateSnapshot<Stop> {
    match check {
        Ok(InitialFilesCheck::InitialFiles) => UpdateSnapshot::Saved {
            snapshot,
            name: None,
            retention: None,
        },
        Ok(InitialFilesCheck::Changed) => UpdateSnapshot::Fail(needs_filesystem_snapshots()),
        Err(error) => UpdateSnapshot::Fail(format!("{UPDATE_CHECK_FAILED}: {error}")),
    }
}

/// Where a terminal interrupt stopped a manual update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateInterruption {
    /// In the wait for a running upload of the agent.
    Wait,
    /// In the upload of the update snapshot.
    Upload,
}

/// How a manual update that a terminal interrupt stopped ends: on a lost shard nothing is
/// written, and the update stays pending for the new owner; otherwise the update fails.
fn interrupted_update<Stop>(
    interruption: UpdateInterruption,
    lost_shard: bool,
) -> UpdateSnapshot<Stop> {
    if lost_shard {
        return UpdateSnapshot::WriteNothing;
    }
    UpdateSnapshot::Fail(
        match interruption {
            UpdateInterruption::Wait => {
                "the update was interrupted while it waited for an upload of a filesystem \
                 snapshot of the agent"
            }
            UpdateInterruption::Upload => {
                "the update was interrupted while it uploaded the filesystem snapshot"
            }
        }
        .to_string(),
    )
}

/// How a manual update whose upload failed with `error` ends. An upload that a stop ended gives
/// `Stopped`, also when its save failed after the stop, and counts as interrupted.
fn failed_update_upload<Stop>(error: &UploadNowError, lost_shard: bool) -> UpdateSnapshot<Stop> {
    match error {
        UploadNowError::Stopped => interrupted_update(UpdateInterruption::Upload, lost_shard),
        UploadNowError::Store(_) | UploadNowError::SaveRunning | UploadNowError::NoSlot => {
            UpdateSnapshot::Fail(format!(
                "failed to upload the filesystem snapshot for the update: {error}"
            ))
        }
    }
}

/// The component revision whose initial files a manual-update baseline without a name seeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceRevision {
    /// The current revision, while the update is pending.
    Current,
    /// The revision of the agent just before the update record at this oplog index.
    Before(OplogIndex),
}

/// The target of the last successful update in `successful_updates` whose entry comes before the
/// oplog index `before`, or `None` when no update comes before it. The order is the order of the
/// oplog, not of the timestamps, which different executors give with their own clocks.
pub(crate) fn revision_before(
    successful_updates: &[golem_common::model::SuccessfulUpdateRecord],
    before: OplogIndex,
) -> Option<ComponentRevision> {
    successful_updates
        .iter()
        .filter(|update| update.oplog_index < before)
        .max_by_key(|update| update.oplog_index)
        .map(|update| update.target_revision)
}

/// The plan of the baseline of a start: where its tree comes from, and the replay inputs. One
/// plan gives both, so a start loads an application snapshot only from the record whose files it
/// restores, and skips exactly the history before that record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartPlan {
    pub(crate) source: TreeSource,
    pub(crate) replay: ReplayBaseline,
}

/// Where the tree of a start comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeSource {
    /// The initial files of the replay revision; no restore.
    InitialFiles,
    /// The filesystem snapshot of this name in the store of the incarnation.
    Store(FilesystemSnapshotName),
    /// The initial files of another revision, followed by the initial-file rule: a manual-update
    /// record without a name.
    SourceFiles(SourceRevision),
}

/// The replay inputs of the baseline of a start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplayBaseline {
    /// The record whose application snapshot the start uses, with its source.
    pub(crate) snapshot: Option<(OplogIndex, SnapshotSource)>,
    /// `Some(index)` makes the start skip the history `1..=index`; `None` keeps only the skipped
    /// regions of the status.
    pub(crate) skip_through: Option<OplogIndex>,
    /// The column of the start outcome table.
    pub(crate) role: BaselineRole,
}

impl ReplayBaseline {
    /// The replay inputs of a start without a record: the initial files and the whole history.
    pub(crate) fn initial_files() -> Self {
        Self {
            snapshot: None,
            skip_through: None,
            role: BaselineRole::InitialFiles,
        }
    }

    /// The skipped regions of the start: `status_regions` with the history before the record
    /// skipped, or without an override when the record is not pending.
    pub(crate) fn skipped_regions(&self, status_regions: &DeletedRegions) -> DeletedRegions {
        let mut regions = status_regions.clone();
        match self.skip_through {
            Some(index) => regions.set_override(
                DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
                    OplogIndex::INITIAL.next()..=index,
                )])
                .build(),
            ),
            None if regions.is_overridden() => regions.drop_override(),
            None => {}
        }
        regions
    }
}

impl StartPlan {
    /// The named filesystem snapshot that the start restores, with the baseline that it is: a
    /// periodic record, or the record that is or becomes the authoritative baseline.
    pub(crate) fn restored(&self) -> Option<(FilesystemSnapshotName, ConfirmedBaseline)> {
        let TreeSource::Store(name) = &self.source else {
            return None;
        };
        let baseline = match &self.replay.role {
            BaselineRole::Periodic(_) => ConfirmedBaseline::Periodic,
            BaselineRole::ManualPending(head) => ConfirmedBaseline::Authoritative(head.oplog_index),
            BaselineRole::ManualPromoted
            | BaselineRole::AssistedPending(_)
            | BaselineRole::AssistedPromoted
            | BaselineRole::InitialFiles => {
                ConfirmedBaseline::Authoritative(self.replay.snapshot.map(|(index, _)| index)?)
            }
        };
        Some((name.clone(), baseline))
    }
}

/// Plans the baseline of a start from `selected`. `manual_name` is the filesystem snapshot name
/// of the manual-update record when `selected` is one; `enabled` tells whether this executor
/// keeps filesystem snapshots. A record that names a filesystem snapshot gives `SnapshotsDisabled`
/// on an executor without them.
pub(crate) fn plan_start(
    selected: &SelectedBaseline,
    manual_name: Option<&FilesystemSnapshotName>,
    enabled: bool,
) -> Result<StartPlan, SnapshotsDisabled> {
    let named = |name: Option<&FilesystemSnapshotName>| match name {
        Some(_) if !enabled => Err(SnapshotsDisabled),
        Some(name) => Ok(TreeSource::Store(name.clone())),
        None => Ok(TreeSource::InitialFiles),
    };
    let manual = |source: SourceRevision| match manual_name {
        Some(name) => named(Some(name)),
        None => Ok(TreeSource::SourceFiles(source)),
    };
    let role = selected.role();
    Ok(match selected {
        SelectedBaseline::Periodic(snapshot) => StartPlan {
            source: named(snapshot.filesystem_snapshot.as_ref())?,
            replay: ReplayBaseline {
                snapshot: Some((snapshot.index, SnapshotSource::Automatic)),
                skip_through: Some(snapshot.index),
                role,
            },
        },
        SelectedBaseline::AssistedPending { snapshot, .. } => StartPlan {
            source: named(snapshot.filesystem_snapshot.as_ref())?,
            replay: ReplayBaseline {
                snapshot: Some((snapshot.index, SnapshotSource::SnapshotAssistedAutomatic)),
                skip_through: Some(snapshot.index),
                role,
            },
        },
        SelectedBaseline::AssistedPromoted { index, name } => StartPlan {
            source: named(name.as_ref())?,
            replay: ReplayBaseline {
                snapshot: Some((*index, SnapshotSource::SnapshotAssistedAutomatic)),
                skip_through: None,
                role,
            },
        },
        SelectedBaseline::ManualPending { head, previous } => StartPlan {
            source: manual(SourceRevision::Current)?,
            replay: ReplayBaseline {
                snapshot: previous.as_ref().map(|previous| {
                    (
                        previous.index,
                        match previous.kind {
                            AuthoritativeSnapshotKind::ManualUpdate => SnapshotSource::ManualUpdate,
                            AuthoritativeSnapshotKind::SnapshotAssistedAutomatic { .. } => {
                                SnapshotSource::SnapshotAssistedAutomatic
                            }
                        },
                    )
                }),
                skip_through: Some(head.oplog_index),
                role,
            },
        },
        SelectedBaseline::ManualPromoted { index } => StartPlan {
            source: manual(SourceRevision::Before(*index))?,
            replay: ReplayBaseline {
                snapshot: Some((*index, SnapshotSource::ManualUpdate)),
                skip_through: None,
                role,
            },
        },
        SelectedBaseline::InitialFiles => StartPlan {
            source: TreeSource::InitialFiles,
            replay: ReplayBaseline::initial_files(),
        },
    })
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

    fn gives_saved_times(&self) -> bool {
        match self {
            Self::Store(restore) => restore.gives_saved_times(),
            Self::InitialFiles(restore) => restore.gives_saved_times(),
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
                None => Confirmation::Deferred,
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

/// What the owner gate says before the admission of the shard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerGate {
    /// The confirmer may not write a confirmation record. Nobody asks the admission.
    Refused,
    /// The confirmer may write the confirmation record when the shard still admits the agent.
    /// The caller asks the admission last.
    NeedsAdmission,
}

/// Whether a confirmer that the owner gate gave `gate` may write its confirmation record. It
/// asks `admission` only when the gate needs it, and at most once, after every other condition.
pub(crate) fn admit_if_needed(gate: OwnerGate, admission: impl FnOnce() -> bool) -> bool {
    gate == OwnerGate::NeedsAdmission && admission()
}

/// Whether `who` may write a confirmation record now, before the admission of the shard: only
/// as the owner of the agent. A running instance needs the generation of its capture. A start
/// needs its own attempt and no pending terminal interrupt. Both need an attached status and no
/// retirement of the owner.
pub(crate) fn owner_gate(
    instance: InstanceView,
    who: &Confirmer,
    terminal_pending: bool,
    detached: bool,
    retiring: bool,
) -> OwnerGate {
    let owner = match (instance, who) {
        (InstanceView::Running { generation_matches }, Confirmer::Running(_)) => generation_matches,
        (InstanceView::WaitingForPermit(attempt), Confirmer::Start(start)) => {
            attempt == *start && !terminal_pending
        }
        _ => false,
    };
    if owner && !detached && !retiring {
        OwnerGate::NeedsAdmission
    } else {
        OwnerGate::Refused
    }
}

/// The filesystem snapshot name that the record `entry` holds: the name of a snapshot record, of
/// a snapshot-based update record, or of the record that a snapshot-assisted update selected. The
/// name of a snapshot-assisted update is always the name of a snapshot record before it.
fn record_name(entry: &OplogEntry) -> Option<&FilesystemSnapshotName> {
    match entry {
        OplogEntry::Snapshot {
            filesystem_snapshot,
            ..
        } => filesystem_snapshot.as_ref(),
        OplogEntry::PendingUpdate {
            description:
                UpdateDescription::SnapshotBased {
                    filesystem_snapshot,
                    ..
                }
                | UpdateDescription::SnapshotAssistedAutomatic {
                    filesystem_snapshot,
                    ..
                },
            ..
        } => filesystem_snapshot.as_ref(),
        _ => None,
    }
}

/// The filesystem snapshot names that a revert of the region `dropped` makes unused, newest
/// first: the names of the snapshot records and of the update records in `entries`
/// inside `dropped`, without the names that such a record outside `dropped` and outside the
/// regions `deleted` uses. A record outside the region can use an older name again, and after the
/// revert that record can be a baseline again.
pub(crate) fn reverted_snapshot_names(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    dropped: &OplogRegion,
    deleted: &DeletedRegions,
) -> Box<[FilesystemSnapshotName]> {
    let live = entries
        .iter()
        .filter(|(index, _)| !dropped.contains(**index) && !deleted.is_in_deleted_region(**index))
        .filter_map(|(_, entry)| record_name(entry))
        .collect::<HashSet<_>>();
    entries
        .range(dropped.start..=dropped.end)
        .rev()
        .filter_map(|(_, entry)| record_name(entry))
        .filter(|name| !live.contains(name))
        .fold(
            (HashSet::new(), Vec::new()),
            |(mut seen, mut names), name| {
                if seen.insert(name) {
                    names.push(name.clone());
                }
                (seen, names)
            },
        )
        .1
        .into_boxed_slice()
}

#[cfg(test)]
mod tests;

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Writes the confirmation record of the filesystem snapshot `name` as `who`, when
    /// [`filesystem_snapshots::owner_gate`] lets it: it reads the detached flag without a lock,
    /// takes the owned instance lock, applies the gate, and sends one status job that carries the
    /// guard. A confirmation of the running instance that gives `Confirmed` records the name in
    /// the slot of the instance under the same guard. So a stop waits for at most the status jobs
    /// already queued plus one confirm transaction. A `Confirmed` answer carries the filesystem
    /// snapshot names that the agent still uses (`snapshot_selection::names_in_use`), from the
    /// status that the worker holds after the append returned. A status job that folds an entry after the append, such as the
    /// `SuccessfulUpdate` of an automatic update, can change those names before the read.
    pub(crate) async fn confirm_as(
        self: &Arc<Self>,
        name: FilesystemSnapshotName,
        who: filesystem_snapshots::Confirmer,
    ) -> agent_filesystem_snapshots::Confirmation {
        if self.last_known_status_detached.load(Ordering::Acquire) {
            return agent_filesystem_snapshots::Confirmation::Deferred;
        }
        let instance_guard = self.instance.clone().lock_owned().await;
        let instance = match (&*instance_guard, &who) {
            (WorkerInstance::Running(running), filesystem_snapshots::Confirmer::Running(mark)) => {
                filesystem_snapshots::InstanceView::Running {
                    generation_matches: running.filesystem_snapshot_slot.owns(*mark),
                }
            }
            (WorkerInstance::Running(_), filesystem_snapshots::Confirmer::Start(_)) => {
                filesystem_snapshots::InstanceView::Running {
                    generation_matches: false,
                }
            }
            (WorkerInstance::WaitingForPermit(waiting), _) => {
                filesystem_snapshots::InstanceView::WaitingForPermit(waiting.start_attempt)
            }
            _ => filesystem_snapshots::InstanceView::Other,
        };
        let gate = filesystem_snapshots::owner_gate(
            instance,
            &who,
            self.interrupt_signal.terminal_pending(),
            self.last_known_status_detached.load(Ordering::Acquire),
            self.owner_retirement_requested.is_cancelled(),
        );
        // The admission reads the clock and the shard assignment, so it is asked last, and only
        // when the gate needs it.
        let owner = filesystem_snapshots::admit_if_needed(gate, || {
            self.shard_service()
                .check_admission(&self.owned_agent_id.agent_id)
                .is_ok()
        });
        if !owner {
            return agent_filesystem_snapshots::Confirmation::Deferred;
        }
        let on_confirmed: state_actor::OnConfirmed = match who {
            filesystem_snapshots::Confirmer::Running(mark) => {
                let name = name.clone();
                Box::new(move |instance| {
                    if let WorkerInstance::Running(running) = instance {
                        running.filesystem_snapshot_slot.record(Some(&name), mark);
                    }
                })
            }
            filesystem_snapshots::Confirmer::Start(_) => Box::new(|_| {}),
        };
        match self
            .state_actor
            .append_confirmation(name, instance_guard, on_confirmed)
            .await
        {
            agent_filesystem_snapshots::ConfirmOutcome::Confirmed => {
                agent_filesystem_snapshots::Confirmation::Confirmed {
                    kept: snapshot_selection::names_in_use(&self.last_known_status.load()),
                }
            }
            agent_filesystem_snapshots::ConfirmOutcome::Superseded => {
                agent_filesystem_snapshots::Confirmation::Superseded
            }
            agent_filesystem_snapshots::ConfirmOutcome::Deferred => {
                agent_filesystem_snapshots::Confirmation::Deferred
            }
        }
    }
    /// Confirms the filesystem snapshot of the last automatic snapshot record before a start, when
    /// the start would select that record if it were confirmed.
    ///
    /// The service waits for an upload of the snapshot on this executor and asks the store, as
    /// [`agent_filesystem_snapshots::AgentFilesystemSnapshots::prepare_start`] says. When the store
    /// holds the whole snapshot, the start writes the confirmation record as the owner of the
    /// agent: while the instance waits for its permits with this start attempt, no terminal
    /// interrupt waits, the status is attached, no retirement of the owner is requested, and the
    /// shard admits the agent. This start holds no slot, no memory and no lock while it waits.
    pub(in crate::worker) async fn confirm_filesystem_snapshot_before_start(
        self: &Arc<Self>,
        start_attempt: Uuid,
    ) {
        let status = self.last_known_status.load_full();
        let Some(name) = self.selection_in_memory(&status).candidate else {
            return;
        };
        let agent_snapshots = crate::filesystem_snapshot::AgentSnapshots::agent(
            &self.owned_agent_id,
            self.initial_worker_metadata.fingerprint,
        );
        if self
            .agent_filesystem_snapshots()
            .prepare_start(&agent_snapshots, &name, self.terminal_interrupt())
            .await
            != agent_filesystem_snapshots::StartCheck::Stored
        {
            return;
        }
        let outcome = self
            .confirm_as(name, filesystem_snapshots::Confirmer::Start(start_attempt))
            .await;
        debug!(?outcome, "Confirmed a filesystem snapshot before a start");
    }
    /// The name of the newest record whose upload a loaded agent waits for before it ends its
    /// generation to start the automatic update at the head of its queue: the record that the
    /// update selects once confirmed. `None` when no unselected automatic update is at the head or
    /// its record is confirmed.
    pub(crate) fn upload_before_an_update(&self) -> Option<FilesystemSnapshotName> {
        snapshot_selection::upload_before_an_automatic_update(
            &self.last_known_status.load(),
            self.filesystem_snapshots_enabled(),
            || {
                self.snapshot_exclusions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            },
        )
    }
    /// Waits, before a loaded agent ends the generation with `mark` to start the automatic update
    /// at the head of its queue, for the upload of the record `name` that
    /// [`Self::upload_before_an_update`] gives, as a start of an unloaded agent does before it
    /// takes its permits. The upload confirms its record as the generation that took it. When the
    /// store holds the whole snapshot, the generation with `mark` confirms it too, which changes
    /// nothing when the upload already did. The wait ends at `confirmation_wait` or at a terminal
    /// interrupt; the invocation loop that awaits the call also ends it at a requested retirement
    /// or a stop. When no upload of the record runs, the call asks the store once, for at most
    /// `store_check_limit`.
    pub(crate) async fn confirm_filesystem_snapshot_before_an_update(
        self: &Arc<Self>,
        name: FilesystemSnapshotName,
        mark: crate::services::agent_filesystem::TreeMark,
    ) {
        let agent_snapshots = crate::filesystem_snapshot::AgentSnapshots::agent(
            &self.owned_agent_id,
            self.initial_worker_metadata.fingerprint,
        );
        if self
            .agent_filesystem_snapshots()
            .prepare_start(&agent_snapshots, &name, self.terminal_interrupt())
            .await
            != agent_filesystem_snapshots::StartCheck::Stored
        {
            return;
        }
        let outcome = self
            .confirm_as(name, filesystem_snapshots::Confirmer::Running(mark))
            .await;
        debug!(
            ?outcome,
            "Confirmed a filesystem snapshot before an automatic update"
        );
    }
    /// The baselines of a start now: the periodic record that it selects under the exclusions of
    /// this incarnation in memory, and the index of the authoritative baseline of the status.
    pub(crate) fn start_baselines_now(&self) -> filesystem_snapshots::StartBaselines {
        let status = self.last_known_status.load();
        let enabled = self.filesystem_snapshots_enabled();
        filesystem_snapshots::StartBaselines {
            periodic: self.read_exclusions(|exclusions| {
                snapshot_selection::periodic_baseline(&status, exclusions, enabled)
            }),
            authoritative: status
                .authoritative_snapshot
                .as_ref()
                .map(|snapshot| snapshot.index),
        }
    }
}

impl StartFilesystem {
    /// Plans the baseline of `selection` for a start of `status` with the hydrated pending update
    /// `head`, as [`filesystem_snapshots::plan_start`] does, and loads what the restore needs: the
    /// manual-update record, the initial files of its source revision, the restore of the store.
    pub(in crate::worker) async fn load_and_plan<Ctx: WorkerCtx>(
        parent: &Arc<Worker<Ctx>>,
        status: &AgentStatusRecord,
        selection: StartSelection,
        head: Option<&HydratedUpdate>,
    ) -> Result<Self, StartFilesystemError> {
        let manual_name = match &selection.baseline {
            snapshot_selection::SelectedBaseline::ManualPending { head: pending, .. } => {
                match head.map(|head| &head.description) {
                    Some(UpdateDescription::SnapshotBased {
                        filesystem_snapshot,
                        ..
                    }) => filesystem_snapshot.clone(),
                    _ => {
                        return Err(StartFilesystemError::Other(WorkerExecutorError::unknown(
                            format!(
                                "Expected a snapshot-based PendingUpdate oplog entry at index {}",
                                pending.oplog_index
                            ),
                        )));
                    }
                }
            }
            snapshot_selection::SelectedBaseline::ManualPromoted { index } => {
                match parent.oplog.read(*index).await? {
                    OplogEntry::PendingUpdate {
                        description:
                            UpdateDescription::SnapshotBased {
                                filesystem_snapshot,
                                ..
                            },
                        ..
                    } => filesystem_snapshot,
                    other => {
                        return Err(StartFilesystemError::Other(WorkerExecutorError::unknown(
                            format!(
                                "Expected a snapshot-based PendingUpdate oplog entry at index {index}, but found {other:?}"
                            ),
                        )));
                    }
                }
            }
            _ => None,
        };
        let snapshots = parent.agent_filesystem_snapshots();
        let plan = filesystem_snapshots::plan_start(
            &selection.baseline,
            manual_name.as_ref(),
            snapshots.is_enabled(),
        )
        .map_err(|_| StartFilesystemError::Disabled)?;
        let restore = match &plan.source {
            filesystem_snapshots::TreeSource::InitialFiles => None,
            filesystem_snapshots::TreeSource::Store(name) => {
                let agent_snapshots = crate::filesystem_snapshot::AgentSnapshots::agent(
                    &parent.owned_agent_id,
                    parent.initial_worker_metadata.fingerprint,
                );
                Some(filesystem_snapshots::StartRestore::Store(
                    snapshots
                        .restore(&agent_snapshots, name)
                        .map_err(|_| StartFilesystemError::Disabled)?,
                ))
            }
            filesystem_snapshots::TreeSource::SourceFiles(source) => {
                let source_revision = match source {
                    filesystem_snapshots::SourceRevision::Current => status.component_revision,
                    filesystem_snapshots::SourceRevision::Before(index) => {
                        RunningWorker::revision_before(parent, status, *index).await?
                    }
                };
                let source_files = RunningWorker::initial_files_of(parent, source_revision)
                    .await
                    .map_err(StartFilesystemError::Other)?;
                crate::services::agent_filesystem::InitialFilesRestore::of_read_only(source_files)
                    .map(filesystem_snapshots::StartRestore::InitialFiles)
            }
        };
        Ok(Self {
            selection,
            plan,
            restore,
        })
    }

    /// The replay inputs of the plan.
    pub(in crate::worker) fn replay(&self) -> &filesystem_snapshots::ReplayBaseline {
        &self.plan.replay
    }

    /// The component revision at the start of the replay.
    pub(in crate::worker) fn replay_revision(&self) -> ComponentRevision {
        self.selection.replay_revision
    }

    /// Materializes the baseline into `reconstructing` with the initial files `prepared`. On
    /// success it starts the filesystem snapshots of the generation in `slot`, with the mark of
    /// the tree before any call runs and the named snapshot that it restored, and gives the
    /// filesystem with the replay inputs of the plan.
    pub(in crate::worker) async fn materialize(
        self,
        reconstructing: ReconstructingFilesystem,
        prepared: crate::services::agent_filesystem::PreparedInitialFiles,
        slot: &filesystem_snapshots::SnapshotSlot,
    ) -> Result<
        (
            ReconstructingFilesystem,
            filesystem_snapshots::ReplayBaseline,
        ),
        StartFilesystemFailure,
    > {
        let restored = self.plan.restored();
        let replay = self.plan.replay;
        match materialize_baseline(reconstructing, prepared, self.restore).await {
            Ok(reconstructing) => {
                slot.start(tree_mark(&reconstructing), restored);
                Ok((reconstructing, replay))
            }
            Err(failure) => Err(StartFilesystemFailure {
                filesystem: failure.filesystem,
                error: failure.source,
                replay,
            }),
        }
    }
}

impl From<UnreadableOplog> for StartFilesystemError {
    fn from(error: UnreadableOplog) -> Self {
        Self::Other(error.into())
    }
}
