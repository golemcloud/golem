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
use crate::services::agent_filesystem::{
    CaptureOutcome, ChangeDetection, FilesystemCapture, InitialFilesRestore, RestoreError,
    RestoreTree, TreeMark, WholeCapture,
};
use crate::services::agent_filesystem_snapshots::{
    Admission, Admitted, AgentFilesystemSnapshots, Confirm, Confirmation, SavedUpdate,
    SnapshotsDisabled, StoreRestore, UpdateAdmitted, UploadNowError,
};
use crate::services::oplog::OplogError;
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    FilesystemSnapshotName, OplogIndex, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::oplog::{OplogEntry, RawSnapshotData};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{AgentId, UsableAutomaticSnapshot};
use golem_service_base::error::worker_executor::WorkerExecutorError;
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
    /// The manual-update record at the index, which a start restored.
    ManualUpdate(OplogIndex),
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
    /// record `selected` that a start selects now and the index `last_manual_update` of the
    /// manual-update baseline.
    pub(crate) fn since(
        &self,
        selected: Option<&UsableAutomaticSnapshot>,
        last_manual_update: Option<OplogIndex>,
    ) -> Option<ConfirmedFilesystemSnapshot> {
        since(
            self.lock()
                .as_ref()
                .and_then(FilesystemSnapshotSlot::confirmed),
            selected,
            last_manual_update,
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<FilesystemSnapshotSlot>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The baselines that a start of a worker has now: the automatic snapshot record that it
/// selects, and the index of the manual-update record of the status. A periodic capture
/// compares with the confirmed snapshot of its slot only while one of them restores it.
#[derive(Clone, Debug)]
pub(crate) struct StartBaselines {
    pub(crate) automatic: Option<UsableAutomaticSnapshot>,
    pub(crate) manual_update: Option<OplogIndex>,
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
/// without a name. `selected` is the automatic snapshot record that a start selects now, and
/// `last_manual_update` the index of the manual-update baseline.
fn since(
    confirmed: Option<&ConfirmedFilesystemSnapshot>,
    selected: Option<&UsableAutomaticSnapshot>,
    last_manual_update: Option<OplogIndex>,
) -> Option<ConfirmedFilesystemSnapshot> {
    let confirmed = confirmed?;
    let selected_now = match (selected, confirmed.baseline) {
        (Some(selected), _) => selected.filesystem_snapshot == confirmed.name,
        (None, ConfirmedBaseline::ManualUpdate(index)) => last_manual_update == Some(index),
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
    /// decides. Without an admission the record has no name. Gives `None` when no record is
    /// written. The admission is dropped unless the record uploads the capture.
    fn new(
        capture: Option<(
            Admission,
            Option<ConfirmedFilesystemSnapshot>,
            CaptureOutcome,
        )>,
    ) -> Option<Self> {
        let Some((admission, since, outcome)) = capture else {
            return Some(Self(PeriodicRecord::WithoutName));
        };
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
/// of the status, then the upload. An admission that the service refuses skips the snapshot,
/// and a disabled service gives a record without a name. A capture that fails writes no
/// record. A record that does not reach the oplog drops the admission and discards the capture
/// at one place. A written record of a tree of initial files gives its mark to the slot.
pub(crate) async fn periodic_snapshot<Host: PeriodicSnapshotHost>(
    host: &mut Host,
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
    mode: AgentMode,
) -> PeriodicResult<Host::Stop> {
    let admission = match snapshots.admit_periodic(agent, mode).await {
        Admitted::Upload(admission) => Some(admission),
        Admitted::WithoutName => None,
        Admitted::Skip(skip) => {
            tracing::debug!(reason = %skip, "Skipping periodic snapshot");
            return PeriodicResult::Continue;
        }
    };
    let snapshot = match host.snapshot_guest().await {
        Ok(snapshot) => snapshot,
        Err(stop) => return PeriodicResult::Guest(stop),
    };
    let capture = match admission {
        Some(admission) => {
            let since = host.since();
            match host
                .capture(
                    admission.capture_wait(),
                    since.as_ref().map(|since| since.mark),
                )
                .await
            {
                Some(outcome) => Some((admission, since, outcome)),
                None => return PeriodicResult::Continue,
            }
        }
        None => None,
    };
    let Some(plan) = PeriodicPlan::new(capture) else {
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
                PeriodicFailure::Write(error @ OplogError::Payload(_)) => {
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
/// owner. A disabled service gives a record without a name.
pub(crate) async fn update_snapshot<Host: UpdateSnapshotHost>(
    host: &mut Host,
    snapshots: &AgentFilesystemSnapshots,
    agent: &AgentSnapshots,
    mode: AgentMode,
) -> UpdateSnapshot<Host::Stop> {
    let admission = match snapshots.admit_update(agent, mode, host.terminal()).await {
        UpdateAdmitted::Upload(admission) => Some(admission),
        UpdateAdmitted::WithoutName => None,
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
    let Some(admission) = admission else {
        return UpdateSnapshot::Saved {
            snapshot,
            name: None,
            retention: None,
        };
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

/// What a start plans first.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StartPlan {
    /// The selected automatic snapshot record gives the baseline.
    Automatic(BaselineStep),
    /// No automatic snapshot record is selected: the start reads the manual-update record and
    /// plans with [`plan_manual_baseline`].
    ManualUpdate,
}

/// Plans the baseline of a start from the selected automatic snapshot record. Without one, the
/// start needs the manual-update record. `enabled` tells whether this executor keeps filesystem
/// snapshots.
pub(crate) fn plan_start_baseline(
    automatic: Option<&UsableAutomaticSnapshot>,
    enabled: bool,
) -> StartPlan {
    match automatic {
        Some(snapshot) => StartPlan::Automatic(named_baseline(
            BaselineKind::Periodic {
                index: snapshot.index,
                name: snapshot.filesystem_snapshot.clone(),
            },
            snapshot.filesystem_snapshot.clone(),
            enabled,
        )),
        None => StartPlan::ManualUpdate,
    }
}

/// The baseline `kind`, which restores the filesystem snapshot `name` when it has one.
fn named_baseline(
    kind: BaselineKind,
    name: Option<FilesystemSnapshotName>,
    enabled: bool,
) -> BaselineStep {
    match name {
        Some(_) if !enabled => BaselineStep::Disabled { kind },
        restore => BaselineStep::Ready { kind, restore },
    }
}

/// Plans the baseline of a start without an automatic snapshot record: from the manual-update
/// record with whether it is still pending, else from the initial files. `enabled` tells whether
/// this executor keeps filesystem snapshots.
pub(crate) fn plan_manual_baseline(
    manual: Option<(TimestampedUpdateDescription, bool)>,
    enabled: bool,
) -> BaselineStep {
    match manual {
        Some((
            TimestampedUpdateDescription {
                timestamp: _,
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
                Some(name) => named_baseline(kind, Some(name), enabled),
                None => BaselineStep::NeedsSourceFiles {
                    kind,
                    source: if pending {
                        SourceRevision::Current
                    } else {
                        SourceRevision::Before(oplog_index)
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
        message: Box<str>,
    },
    /// The same conflict on a lost shard. Nothing is written.
    ShardLost,
    /// A manual-update baseline that does not restore, and whose error allows no retry. The
    /// start fails with the message as a visible cause.
    FailVisibly(Box<str>),
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
                    message: conflict.to_string().into_boxed_str(),
                }
            }
        }
        (BaselineKind::ManualUpdate { .. }, Error::Baseline(error)) if !error.retryable => {
            BaselineFailure::FailVisibly(error.to_string().into_boxed_str())
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

/// The filesystem snapshot name that the record `entry` holds: the name of a snapshot record or
/// of a snapshot-based update record.
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
                },
            ..
        } => filesystem_snapshot.as_ref(),
        _ => None,
    }
}

/// The filesystem snapshot names that a revert of the region `dropped` makes unused, newest
/// first: the names of the snapshot records and of the snapshot-based update records in `entries`
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
mod tests {
    use super::*;
    use golem_common::model::Timestamp;
    use std::sync::Arc;
    use test_r::test;

    /// Gives a mark of a new generation and a later mark of the same generation.
    fn marks() -> (TreeMark, TreeMark) {
        crate::services::agent_filesystem::test_tree_marks()
    }

    fn confirmed(name: &FilesystemSnapshotName, mark: TreeMark) -> ConfirmedFilesystemSnapshot {
        ConfirmedFilesystemSnapshot {
            name: Some(name.clone()),
            mark,
            baseline: ConfirmedBaseline::Periodic,
        }
    }

    fn without_name(mark: TreeMark) -> ConfirmedFilesystemSnapshot {
        ConfirmedFilesystemSnapshot {
            name: None,
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
        let (mark, _) = marks();
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
    async fn a_snapshot_without_a_name_is_compared_with_only_while_a_start_selects_a_record_without_a_name()
     {
        let (mark, _) = marks();
        let written = without_name(mark);
        let name = FilesystemSnapshotName::periodic();

        let cases = [
            since(Some(&written), Some(&selected(None)), None),
            since(Some(&written), Some(&selected(Some(&name))), None),
            since(Some(&written), None, None),
        ];

        assert_eq!(cases, [Some(written), None, None]);
    }

    fn captured(detection: ChangeDetection) -> CaptureFinding<u8> {
        CaptureFinding::Captured { copy: 1, detection }
    }

    #[test]
    async fn the_record_of_a_periodic_snapshot_follows_the_finding_of_the_capture() {
        let (mark, _) = marks();
        let name = FilesystemSnapshotName::periodic();
        let since = confirmed(&name, mark);

        let initial = without_name(mark);

        let cases = [
            plan_periodic_record(CaptureFinding::Unchanged, Some(&since)),
            plan_periodic_record(CaptureFinding::Unchanged, Some(&initial)),
            plan_periodic_record(CaptureFinding::Unchanged, None),
            plan_periodic_record(CaptureFinding::InitialFiles { mark }, Some(&since)),
            plan_periodic_record(CaptureFinding::InitialFiles { mark }, None),
            plan_periodic_record(captured(ChangeDetection::SizeMtime), Some(&since)),
            plan_periodic_record(captured(ChangeDetection::SizeMtime), Some(&initial)),
            plan_periodic_record(captured(ChangeDetection::Full), Some(&since)),
            plan_periodic_record(captured(ChangeDetection::Full), None),
        ];

        assert_eq!(
            cases,
            [
                Some(PeriodicRecord::Reused(name.clone())),
                Some(PeriodicRecord::WithoutName),
                None,
                Some(PeriodicRecord::InitialFiles { mark }),
                Some(PeriodicRecord::InitialFiles { mark }),
                Some(PeriodicRecord::Own {
                    copy: 1,
                    parent: Some((name, StoreChangeDetection::SizeMtime)),
                }),
                Some(PeriodicRecord::Own {
                    copy: 1,
                    parent: None
                }),
                Some(PeriodicRecord::Own {
                    copy: 1,
                    parent: None
                }),
                Some(PeriodicRecord::Own {
                    copy: 1,
                    parent: None
                }),
            ]
        );
    }

    #[test]
    async fn only_the_owner_of_the_agent_passes_the_gate_before_the_admission() {
        let (mark, _) = marks();
        let attempt = Uuid::new_v4();
        let running = Confirmer::Running(mark);
        let start = Confirmer::Start(attempt);
        let matching = InstanceView::Running {
            generation_matches: true,
        };

        assert_eq!(
            [
                owner_gate(matching, &running, false, false, false),
                owner_gate(matching, &running, true, false, false),
                owner_gate(
                    InstanceView::Running {
                        generation_matches: false
                    },
                    &running,
                    false,
                    false,
                    false
                ),
                owner_gate(matching, &start, false, false, false),
                owner_gate(
                    InstanceView::WaitingForPermit(attempt),
                    &start,
                    false,
                    false,
                    false
                ),
                owner_gate(
                    InstanceView::WaitingForPermit(attempt),
                    &start,
                    true,
                    false,
                    false
                ),
                owner_gate(
                    InstanceView::WaitingForPermit(Uuid::new_v4()),
                    &start,
                    false,
                    false,
                    false
                ),
                owner_gate(
                    InstanceView::WaitingForPermit(attempt),
                    &running,
                    false,
                    false,
                    false
                ),
                owner_gate(InstanceView::Other, &running, false, false, false),
                owner_gate(matching, &running, false, true, false),
                owner_gate(matching, &running, false, false, true),
            ],
            [
                OwnerGate::NeedsAdmission,
                OwnerGate::NeedsAdmission,
                OwnerGate::Refused,
                OwnerGate::Refused,
                OwnerGate::NeedsAdmission,
                OwnerGate::Refused,
                OwnerGate::Refused,
                OwnerGate::Refused,
                OwnerGate::Refused,
                OwnerGate::Refused,
                OwnerGate::Refused,
            ]
        );
    }

    #[test]
    async fn the_admission_is_asked_once_and_only_when_the_gate_needs_it() {
        let asked = std::cell::Cell::new(0);
        let admission = |admitted| {
            let asked = &asked;
            move || {
                asked.set(asked.get() + 1);
                admitted
            }
        };

        let refused = admit_if_needed(OwnerGate::Refused, admission(true));
        let asked_after_refused = asked.get();
        let admitted = admit_if_needed(OwnerGate::NeedsAdmission, admission(true));
        let not_admitted = admit_if_needed(OwnerGate::NeedsAdmission, admission(false));

        assert_eq!(
            (
                refused,
                asked_after_refused,
                admitted,
                not_admitted,
                asked.get()
            ),
            (false, 0, true, false, 2)
        );
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
    async fn the_revision_before_an_update_record_follows_the_oplog_order_and_not_the_clocks() {
        let update =
            |millis: u64, revision: u64, index: u64| golem_common::model::SuccessfulUpdateRecord {
                timestamp: Timestamp::from(millis),
                target_revision: ComponentRevision::new(revision).unwrap(),
                oplog_index: OplogIndex::from_u64(index),
                filesystem_snapshot: None,
            };
        // The update to revision 3 applies the pending record at index 7 on an executor whose
        // clock is behind: its timestamp is earlier than the pending record, and its index is
        // later.
        let updates = [update(1_000, 2, 3), update(4_000, 3, 9)];

        assert_eq!(
            [
                revision_before(&updates, OplogIndex::from_u64(7)),
                revision_before(&updates, OplogIndex::from_u64(10)),
                revision_before(&updates, OplogIndex::from_u64(3)),
                revision_before(&[], OplogIndex::from_u64(7)),
            ],
            [
                Some(ComponentRevision::new(2).unwrap()),
                Some(ComponentRevision::new(3).unwrap()),
                None,
                None,
            ]
        );
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
                plan_start_baseline(Some(&automatic), true),
                plan_start_baseline(Some(&automatic), false),
                plan_start_baseline(Some(&automatic_without_name), false),
                plan_start_baseline(None, true),
            ],
            [
                StartPlan::Automatic(BaselineStep::Ready {
                    kind: periodic.clone(),
                    restore: Some(name.clone()),
                }),
                StartPlan::Automatic(BaselineStep::Disabled {
                    kind: automatic_kind
                }),
                StartPlan::Automatic(BaselineStep::Ready {
                    kind: BaselineKind::Periodic {
                        index: OplogIndex::from_u64(10),
                        name: None
                    },
                    restore: None,
                }),
                StartPlan::ManualUpdate,
            ]
        );
        assert_eq!(
            [
                plan_manual_baseline(Some(manual(Some(&update), false)), true),
                plan_manual_baseline(Some(manual(Some(&update), true)), false),
                plan_manual_baseline(Some(manual(None, true)), false),
                plan_manual_baseline(Some(manual(None, false)), true),
                plan_manual_baseline(Some(not_snapshot_based), true),
                plan_manual_baseline(None, true),
            ],
            [
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
                    source: SourceRevision::Before(OplogIndex::from_u64(7)),
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
                    message: conflict.to_string().into_boxed_str(),
                },
                BaselineFailure::ShardLost,
                BaselineFailure::Reconstruction,
                BaselineFailure::FailVisibly(restore(false).to_string().into_boxed_str()),
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

    /// A host that records its calls and answers as the test says.
    struct ScriptedHost {
        calls: std::sync::Mutex<Vec<Box<str>>>,
        guest: Option<Result<RawSnapshotData, &'static str>>,
        since: Option<ConfirmedFilesystemSnapshot>,
        capture: std::sync::Mutex<Option<CaptureOutcome>>,
        whole: std::sync::Mutex<Option<WholeCapture>>,
        entry_fails: bool,
        write_fails: bool,
        terminal: watch::Sender<bool>,
        lost_shard: bool,
    }

    impl ScriptedHost {
        fn new() -> Self {
            Self {
                calls: std::sync::Mutex::default(),
                guest: Some(Ok(RawSnapshotData {
                    data: vec![1],
                    mime_type: "application/octet-stream".to_string(),
                })),
                since: None,
                capture: std::sync::Mutex::default(),
                whole: std::sync::Mutex::default(),
                entry_fails: false,
                write_fails: false,
                terminal: watch::channel(false).0,
                lost_shard: false,
            }
        }

        fn call(&self, call: String) {
            self.calls.lock().unwrap().push(call.into_boxed_str());
        }

        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| call.to_string())
                .collect()
        }
    }

    fn named(name: Option<&FilesystemSnapshotName>) -> String {
        name.map_or_else(|| "none".to_string(), |name| name.as_str().to_string())
    }

    impl PeriodicSnapshotHost for ScriptedHost {
        type Stop = &'static str;

        async fn snapshot_guest(&mut self) -> Result<RawSnapshotData, &'static str> {
            self.call("snapshot_guest".to_string());
            self.guest.take().unwrap_or(Err("saved twice"))
        }

        fn since(&self) -> Option<ConfirmedFilesystemSnapshot> {
            self.call("since".to_string());
            self.since.clone()
        }

        async fn capture(
            &self,
            _wait: Duration,
            since: Option<TreeMark>,
        ) -> Option<CaptureOutcome> {
            self.call(format!("capture({})", since.is_some()));
            self.capture.lock().unwrap().take()
        }

        async fn entry(
            &self,
            _snapshot: RawSnapshotData,
            name: Option<FilesystemSnapshotName>,
        ) -> Result<OplogEntry, String> {
            self.call(format!("entry({})", named(name.as_ref())));
            if self.entry_fails {
                Err("no payload".to_string())
            } else {
                Ok(OplogEntry::interrupted())
            }
        }

        async fn write(
            &self,
            confirmed_at_once: Option<FilesystemSnapshotName>,
            _entry: OplogEntry,
        ) -> Result<(), OplogError> {
            self.call(format!("write({})", named(confirmed_at_once.as_ref())));
            if self.write_fails {
                Err(OplogError::Payload("refused".to_string()))
            } else {
                Ok(())
            }
        }

        fn confirm(&self, _mark: TreeMark) -> Confirm {
            self.call("confirm".to_string());
            Box::new(|_| Box::pin(async { Confirmation::Deferred }))
        }

        fn initial_files_written(&self, _mark: TreeMark) {
            self.call("initial_files_written".to_string());
        }
    }

    impl UpdateSnapshotHost for ScriptedHost {
        type Stop = &'static str;

        async fn snapshot_guest(&mut self) -> Result<RawSnapshotData, &'static str> {
            PeriodicSnapshotHost::snapshot_guest(self).await
        }

        async fn capture_whole(&self, _wait: Duration) -> Option<WholeCapture> {
            self.call("capture_whole".to_string());
            self.whole.lock().unwrap().take()
        }

        fn terminal(&self) -> watch::Receiver<bool> {
            self.terminal.subscribe()
        }

        fn lost_shard(&self) -> super::super::LostShard {
            watch::channel(
                self.lost_shard
                    .then_some(super::super::RetirementReason::ShardRevoked),
            )
            .1
        }
    }

    fn agent_snapshots(name: &str) -> AgentSnapshots {
        AgentSnapshots::agent(
            &golem_common::model::OwnedAgentId::new(
                golem_common::model::environment::EnvironmentId::new(),
                &AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: name.to_string(),
                },
            ),
            golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        )
    }

    /// An enabled service over an in-memory store, with the shutdown that keeps it running.
    fn enabled_service() -> (
        Arc<AgentFilesystemSnapshots>,
        crate::services::shutdown::Shutdown,
    ) {
        let shutdown = crate::services::shutdown::Shutdown::new();
        let snapshots = AgentFilesystemSnapshots::bind(
            &crate::services::golem_config::FilesystemSnapshotsConfig::default(),
            crate::services::agent_filesystem_snapshots::StoreSource::given(
                Arc::new(crate::filesystem_snapshot::InMemorySnapshotStore::new()),
                crate::services::golem_config::FilesystemSnapshotUploadConfig::default(),
            ),
            false,
            &shutdown,
        )
        .unwrap();
        (snapshots, shutdown)
    }

    /// Shuts down the service of `shutdown`, and waits until it stopped.
    async fn shut_down(shutdown: crate::services::shutdown::Shutdown) {
        shutdown.cancel();
        assert!(
            shutdown.wait_for_tracked(Duration::from_secs(10)).await,
            "the filesystem snapshot service did not stop"
        );
    }

    /// A service that keeps no filesystem snapshots, with the shutdown that keeps it running.
    fn disabled_service() -> (
        Arc<AgentFilesystemSnapshots>,
        crate::services::shutdown::Shutdown,
    ) {
        let shutdown = crate::services::shutdown::Shutdown::new();
        let snapshots = AgentFilesystemSnapshots::bind(
            &crate::services::golem_config::FilesystemSnapshotsConfig::default(),
            crate::services::agent_filesystem_snapshots::StoreSource::configured(
                Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new()),
                crate::services::agent_filesystem_snapshots::VolumeRoom::Unlimited,
            ),
            false,
            &shutdown,
        )
        .unwrap();
        (snapshots, shutdown)
    }

    fn outcome<Stop: std::fmt::Debug>(result: &PeriodicResult<Stop>) -> String {
        format!("{result:?}")
    }

    #[test]
    async fn a_periodic_snapshot_without_snapshots_writes_a_record_without_a_name() {
        let (disabled, disabled_shutdown) = disabled_service();
        let agent = agent_snapshots("periodic-disabled");
        let run = |host: ScriptedHost| {
            let disabled = disabled.as_ref();
            let agent = &agent;
            async move {
                let mut host = host;
                let result =
                    periodic_snapshot(&mut host, disabled, agent, AgentMode::Durable).await;
                (outcome(&result), host.calls())
            }
        };

        let written = run(ScriptedHost::new()).await;
        let guest_stop = run(ScriptedHost {
            guest: Some(Err("stop")),
            ..ScriptedHost::new()
        })
        .await;
        let no_entry = run(ScriptedHost {
            entry_fails: true,
            ..ScriptedHost::new()
        })
        .await;
        let not_written = run(ScriptedHost {
            write_fails: true,
            ..ScriptedHost::new()
        })
        .await;

        assert_eq!(
            written,
            (
                "Continue".to_string(),
                vec!["snapshot_guest", "entry(none)", "write(none)"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            )
        );
        assert_eq!(
            guest_stop,
            (
                "Guest(\"stop\")".to_string(),
                vec!["snapshot_guest".to_string()]
            )
        );
        assert_eq!(
            no_entry,
            (
                "NotWritten(Entry(\"no payload\"))".to_string(),
                vec!["snapshot_guest".to_string(), "entry(none)".to_string()]
            )
        );
        assert_eq!(
            not_written.0,
            "NotWritten(Write(Payload(\"refused\")))".to_string()
        );
        shut_down(disabled_shutdown).await;
    }

    #[test]
    async fn a_periodic_snapshot_admits_before_the_guest_saves_and_captures_after() {
        let (mark, _) = marks();
        let (snapshots, shutdown) = enabled_service();
        let agent = agent_snapshots("periodic-enabled");
        let since = confirmed(&FilesystemSnapshotName::periodic(), mark);

        let held = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .unwrap();
        let mut refused = ScriptedHost::new();
        let while_held =
            outcome(&periodic_snapshot(&mut refused, &snapshots, &agent, AgentMode::Durable).await);
        drop(held);
        let mut failed_capture = ScriptedHost::new();
        let capture_failed = outcome(
            &periodic_snapshot(&mut failed_capture, &snapshots, &agent, AgentMode::Durable).await,
        );
        let mut initial = ScriptedHost {
            capture: std::sync::Mutex::new(Some(CaptureOutcome::InitialFiles { mark })),
            ..ScriptedHost::new()
        };
        let initial_files =
            outcome(&periodic_snapshot(&mut initial, &snapshots, &agent, AgentMode::Durable).await);
        let mut unchanged = ScriptedHost {
            since: Some(since.clone()),
            capture: std::sync::Mutex::new(Some(CaptureOutcome::Unchanged)),
            ..ScriptedHost::new()
        };
        let reused = outcome(
            &periodic_snapshot(&mut unchanged, &snapshots, &agent, AgentMode::Durable).await,
        );
        let free_after = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .is_ok();

        let name = since.name.as_ref().unwrap().as_str().to_string();
        assert_eq!(
            (while_held, refused.calls()),
            ("Continue".to_string(), vec![])
        );
        assert_eq!(
            (capture_failed, failed_capture.calls()),
            (
                "Continue".to_string(),
                vec!["snapshot_guest", "since", "capture(false)"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            )
        );
        assert_eq!(
            (initial_files, initial.calls()),
            (
                "Continue".to_string(),
                vec![
                    "snapshot_guest",
                    "since",
                    "capture(false)",
                    "entry(none)",
                    "write(none)",
                    "initial_files_written"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            )
        );
        assert_eq!(
            (reused, unchanged.calls()),
            (
                "Continue".to_string(),
                vec![
                    "snapshot_guest".to_string(),
                    "since".to_string(),
                    "capture(true)".to_string(),
                    format!("entry({name})"),
                    format!("write({name})"),
                ]
            )
        );
        assert!(free_after);
        shut_down(shutdown).await;
    }

    fn update_outcome(result: &UpdateSnapshot<&'static str>) -> String {
        match result {
            UpdateSnapshot::Saved {
                name, retention, ..
            } => {
                format!("Saved({}, {})", named(name.as_ref()), retention.is_some())
            }
            UpdateSnapshot::Fail(details) => format!("Fail({details})"),
            UpdateSnapshot::WriteNothing => "WriteNothing".to_string(),
            UpdateSnapshot::Guest(stop) => format!("Guest({stop})"),
        }
    }

    #[test]
    async fn a_manual_update_snapshot_saves_the_guest_then_captures_the_whole_tree() {
        let (disabled, disabled_shutdown) = disabled_service();
        let (snapshots, shutdown) = enabled_service();
        let agent = agent_snapshots("update");

        let mut without = ScriptedHost::new();
        let without_snapshots = update_outcome(
            &update_snapshot(&mut without, &disabled, &agent, AgentMode::Durable).await,
        );
        let mut stopped = ScriptedHost {
            guest: Some(Err("stop")),
            ..ScriptedHost::new()
        };
        let guest_stop = update_outcome(
            &update_snapshot(&mut stopped, &snapshots, &agent, AgentMode::Durable).await,
        );
        let mut failed = ScriptedHost::new();
        let capture_failed = update_outcome(
            &update_snapshot(&mut failed, &snapshots, &agent, AgentMode::Durable).await,
        );
        let mut initial = ScriptedHost {
            whole: std::sync::Mutex::new(Some(WholeCapture::InitialFiles)),
            ..ScriptedHost::new()
        };
        let initial_files = update_outcome(
            &update_snapshot(&mut initial, &snapshots, &agent, AgentMode::Durable).await,
        );

        assert_eq!(
            (without_snapshots, without.calls()),
            (
                "Saved(none, false)".to_string(),
                vec!["snapshot_guest".to_string()]
            )
        );
        assert_eq!(guest_stop, "Guest(stop)");
        assert_eq!(
            (capture_failed, failed.calls()),
            (
                "Fail(failed to capture the agent filesystem for the update)".to_string(),
                vec!["snapshot_guest".to_string(), "capture_whole".to_string()]
            )
        );
        assert_eq!(initial_files, "Saved(none, false)");
        assert!(
            snapshots
                .admit_periodic(&agent, AgentMode::Durable)
                .await
                .is_ok()
        );
        shut_down(disabled_shutdown).await;
        shut_down(shutdown).await;
    }

    #[test]
    async fn an_interrupted_wait_of_a_manual_update_fails_it_or_writes_nothing_on_a_lost_shard() {
        let (snapshots, shutdown) = enabled_service();
        let agent = agent_snapshots("update-interrupted");
        let held = snapshots
            .admit_periodic(&agent, AgentMode::Durable)
            .await
            .unwrap();
        let run = |lost_shard| {
            let snapshots = &snapshots;
            let agent = &agent;
            async move {
                let mut host = ScriptedHost {
                    lost_shard,
                    ..ScriptedHost::new()
                };
                host.terminal.send_replace(true);
                let result = update_snapshot(&mut host, snapshots, agent, AgentMode::Durable).await;
                (update_outcome(&result), host.calls())
            }
        };

        let failed = run(false).await;
        let lost = run(true).await;
        drop(held);

        assert_eq!(
            failed,
            (
                "Fail(the update was interrupted while it waited for an upload of a filesystem \
                 snapshot of the agent)"
                    .to_string(),
                Vec::<String>::new()
            )
        );
        assert_eq!(lost, ("WriteNothing".to_string(), Vec::<String>::new()));
        shut_down(shutdown).await;
    }

    #[test]
    async fn an_upload_that_a_stop_ended_is_interrupted_and_a_failed_one_fails() {
        let store = UploadNowError::Store(crate::filesystem_snapshot::SaveError::NameInUse);
        let uploaded = |error: &UploadNowError, lost_shard: bool| {
            update_outcome(&failed_update_upload(error, lost_shard))
        };
        assert_eq!(
            [
                uploaded(&UploadNowError::Stopped, false),
                uploaded(&UploadNowError::Stopped, true),
                uploaded(&store, false),
                uploaded(&store, true),
                uploaded(&UploadNowError::NoSlot, false),
            ],
            [
                "Fail(the update was interrupted while it uploaded the filesystem snapshot)",
                "WriteNothing",
                "Fail(failed to upload the filesystem snapshot for the update: a filesystem \
                 snapshot already has the name)",
                "Fail(failed to upload the filesystem snapshot for the update: a filesystem \
                 snapshot already has the name)",
                "Fail(failed to upload the filesystem snapshot for the update: no slot of the \
                 uploads of filesystem snapshots was free in the wait)",
            ]
            .map(String::from)
        );
    }

    #[test]
    async fn a_confirmation_of_another_generation_leaves_the_slot_unchanged() {
        let (first, later) = marks();
        let (other_generation, _) = marks();
        let restored = FilesystemSnapshotName::periodic();
        let confirmed_now = FilesystemSnapshotName::periodic();
        let slot = FilesystemSnapshotSlot::at_start(
            first,
            Some((restored.clone(), ConfirmedBaseline::Periodic)),
        );

        let after_other = confirmed_slot(
            &slot,
            Some(&FilesystemSnapshotName::periodic()),
            other_generation,
        );
        let after_own = confirmed_slot(&slot, Some(&confirmed_now), later);

        assert!(after_other.is_none());
        assert_eq!(
            slot.confirmed().map(|confirmed| confirmed.name.clone()),
            Some(Some(restored))
        );
        assert_eq!(
            after_own
                .as_ref()
                .and_then(|slot| slot.confirmed().cloned()),
            Some(confirmed(&confirmed_now, later))
        );
    }

    #[test]
    async fn a_baseline_restored_the_name_of_its_record() {
        let name = FilesystemSnapshotName::periodic();
        let update = FilesystemSnapshotName::update();
        let periodic = |name: Option<&FilesystemSnapshotName>| BaselineKind::Periodic {
            index: OplogIndex::from_u64(10),
            name: name.cloned(),
        };

        assert_eq!(
            [
                BaselineKind::InitialFiles.restored(),
                periodic(Some(&name)).restored(),
                periodic(None).restored(),
                manual_kind(Some(&update), false).restored(),
                manual_kind(None, true).restored(),
            ],
            [
                None,
                Some((name, ConfirmedBaseline::Periodic)),
                None,
                Some((
                    update,
                    ConfirmedBaseline::ManualUpdate(OplogIndex::from_u64(7))
                )),
                None,
            ]
        );
    }

    #[test]
    async fn a_start_restore_of_a_snapshot_that_the_store_does_not_hold_fails() {
        let (snapshots, shutdown) = enabled_service();
        let agent = agent_snapshots("restore-missing");
        let restore = StartRestore::Store(
            snapshots
                .restore(&agent, &FilesystemSnapshotName::periodic())
                .unwrap(),
        );
        let into = tempfile::tempdir().unwrap();

        assert!(restore.restore(into.path()).await.is_err());
        shut_down(shutdown).await;
    }

    #[test]
    async fn only_a_service_with_a_store_is_enabled() {
        let (enabled, enabled_shutdown) = enabled_service();
        let (disabled, disabled_shutdown) = disabled_service();

        assert_eq!((enabled.is_enabled(), disabled.is_enabled()), (true, false));
        shut_down(enabled_shutdown).await;
        shut_down(disabled_shutdown).await;
    }

    #[test]
    async fn the_shared_slot_takes_a_record_only_from_the_generation_that_runs() {
        let (first, later) = marks();
        let (other, _) = marks();
        let name = FilesystemSnapshotName::periodic();
        let selected_name = selected(Some(&name));
        let slot = SnapshotSlot::default();

        slot.record(Some(&name), later);
        let before_start = slot.since(Some(&selected_name), None);
        slot.start(first, None);
        slot.record(Some(&name), other);
        let of_other_generation = slot.since(Some(&selected_name), None);
        slot.record(Some(&name), later);
        let of_own_generation = slot.since(Some(&selected_name), None);
        let owns = (slot.owns(later), slot.owns(other));
        slot.end();
        let ended = (slot.since(Some(&selected_name), None), slot.owns(later));

        assert_eq!(before_start, None);
        assert_eq!(of_other_generation, None);
        assert_eq!(of_own_generation, Some(confirmed(&name, later)));
        assert_eq!(owns, (true, false));
        assert_eq!(ended, (None, false));
    }

    #[test]
    async fn a_written_record_of_initial_files_sets_the_slot_without_a_name_only_in_its_generation()
    {
        let (first, later) = marks();
        let (other_generation, _) = marks();
        let restored = FilesystemSnapshotName::periodic();
        let slot = FilesystemSnapshotSlot::at_start(
            first,
            Some((restored.clone(), ConfirmedBaseline::Periodic)),
        );

        let after_other = confirmed_slot(&slot, None, other_generation);
        let after_own = confirmed_slot(&slot, None, later);

        assert!(after_other.is_none());
        assert_eq!(
            after_own
                .as_ref()
                .and_then(|slot| slot.confirmed().cloned()),
            Some(without_name(later))
        );
    }

    #[test]
    async fn an_unchanged_tree_of_initial_files_writes_a_record_without_a_name_and_confirms_nothing()
     {
        let (mark, _) = marks();
        let (snapshots, shutdown) = enabled_service();
        let agent = agent_snapshots("periodic-initial-files");
        let mut unchanged = ScriptedHost {
            since: Some(without_name(mark)),
            capture: std::sync::Mutex::new(Some(CaptureOutcome::Unchanged)),
            ..ScriptedHost::new()
        };
        let mut not_written = ScriptedHost {
            capture: std::sync::Mutex::new(Some(CaptureOutcome::InitialFiles { mark })),
            write_fails: true,
            ..ScriptedHost::new()
        };

        let unchanged_result = outcome(
            &periodic_snapshot(&mut unchanged, &snapshots, &agent, AgentMode::Durable).await,
        );
        let not_written_result = outcome(
            &periodic_snapshot(&mut not_written, &snapshots, &agent, AgentMode::Durable).await,
        );

        assert_eq!(
            (unchanged_result, unchanged.calls()),
            (
                "Continue".to_string(),
                [
                    "snapshot_guest",
                    "since",
                    "capture(true)",
                    "entry(none)",
                    "write(none)"
                ]
                .map(String::from)
                .to_vec()
            )
        );
        assert_eq!(
            (not_written_result, not_written.calls()),
            (
                "NotWritten(Write(Payload(\"refused\")))".to_string(),
                [
                    "snapshot_guest",
                    "since",
                    "capture(false)",
                    "entry(none)",
                    "write(none)"
                ]
                .map(String::from)
                .to_vec()
            )
        );
        shut_down(shutdown).await;
    }

    #[test]
    async fn a_written_periodic_record_that_uploads_its_capture_asks_for_its_confirmation() {
        let (mark, _) = marks();
        let (snapshots, shutdown) = enabled_service();
        let agent = agent_snapshots("periodic-upload");
        let scratch = crate::services::agent_filesystem::scratch_directory().await;
        let mut host = ScriptedHost {
            capture: std::sync::Mutex::new(Some(CaptureOutcome::Captured {
                capture: FilesystemCapture::empty_in(&scratch).await,
                mark,
                detection: ChangeDetection::Full,
            })),
            ..ScriptedHost::new()
        };

        let result =
            outcome(&periodic_snapshot(&mut host, &snapshots, &agent, AgentMode::Durable).await);

        assert_eq!(
            (result, host.calls().last().cloned()),
            ("Continue".to_string(), Some("confirm".to_string()))
        );
        shut_down(shutdown).await;
    }

    /// A store over an in-memory store that gives each save a time ten minutes after the time of
    /// the save before it, so that a retention sees snapshots older than its clock-skew margin,
    /// and that reports each deleted name.
    #[derive(Default)]
    struct SpacedStore {
        memory: crate::filesystem_snapshot::InMemorySnapshotStore,
        times: crate::filesystem_snapshot::SpacedTimes,
        deleted: watch::Sender<Vec<Box<str>>>,
    }

    #[async_trait::async_trait]
    impl crate::filesystem_snapshot::FilesystemSnapshotStore for SpacedStore {
        async fn save(
            &self,
            agent: &AgentSnapshots,
            name: &crate::filesystem_snapshot::SnapshotName,
            tree: &Path,
            parent: Option<(
                &crate::filesystem_snapshot::SnapshotName,
                StoreChangeDetection,
            )>,
            cancel: &tokio_util::sync::CancellationToken,
            slots: &dyn crate::filesystem_snapshot::RunSlots,
        ) -> Result<crate::filesystem_snapshot::SnapshotInfo, crate::filesystem_snapshot::SaveError>
        {
            let info = self
                .memory
                .save(agent, name, tree, parent, cancel, slots)
                .await?;
            Ok(self.times.timed(name, info))
        }

        async fn restore(
            &self,
            agent: &AgentSnapshots,
            name: &crate::filesystem_snapshot::SnapshotName,
            into: &Path,
            slots: &dyn crate::filesystem_snapshot::RunSlots,
        ) -> Result<
            crate::filesystem_snapshot::SnapshotInfo,
            crate::filesystem_snapshot::RestoreFailure,
        > {
            self.memory.restore(agent, name, into, slots).await
        }

        async fn stat(
            &self,
            agent: &AgentSnapshots,
            name: &crate::filesystem_snapshot::SnapshotName,
        ) -> Result<
            Option<crate::filesystem_snapshot::SnapshotInfo>,
            crate::filesystem_snapshot::ReadError,
        > {
            self.memory.stat(agent, name).await
        }

        async fn list(
            &self,
            agent: &AgentSnapshots,
            slots: &dyn crate::filesystem_snapshot::RunSlots,
        ) -> Result<
            Box<
                [(
                    crate::filesystem_snapshot::SnapshotName,
                    crate::filesystem_snapshot::SnapshotInfo,
                )],
            >,
            crate::filesystem_snapshot::CallError,
        > {
            Ok(self
                .memory
                .list(agent, slots)
                .await?
                .iter()
                .map(|(name, info)| (name.clone(), self.times.timed(name, *info)))
                .collect())
        }

        async fn delete(
            &self,
            agent: &AgentSnapshots,
            names: &[crate::filesystem_snapshot::SnapshotName],
            slots: &dyn crate::filesystem_snapshot::RunSlots,
        ) -> Result<(), crate::filesystem_snapshot::CallError> {
            self.memory.delete(agent, names, slots).await?;
            self.deleted.send_modify(|deleted| {
                deleted.extend(names.iter().map(|name| Box::from(name.as_str())))
            });
            Ok(())
        }

        async fn delete_all(
            &self,
            agent: &AgentSnapshots,
            slots: &dyn crate::filesystem_snapshot::RunSlots,
        ) -> Result<(), crate::filesystem_snapshot::CallError> {
            self.memory.delete_all(agent, slots).await
        }

        async fn copy_all(
            &self,
            from: &AgentSnapshots,
            to: &AgentSnapshots,
            slots: &dyn crate::filesystem_snapshot::RunSlots,
        ) -> Result<(), crate::filesystem_snapshot::CallError> {
            self.memory.copy_all(from, to, slots).await
        }
    }

    /// Saves `older` update snapshots, then a manual update whose retention keeps the older
    /// snapshots at the indexes `kept`, and gives the names that the retention deleted.
    async fn update_retention(older: usize, kept: &[usize]) -> (Vec<Box<str>>, Vec<Box<str>>) {
        let store = Arc::new(SpacedStore::default());
        let shutdown = crate::services::shutdown::Shutdown::new();
        let snapshots = AgentFilesystemSnapshots::bind(
            &crate::services::golem_config::FilesystemSnapshotsConfig::default(),
            crate::services::agent_filesystem_snapshots::StoreSource::given(
                store.clone(),
                crate::services::golem_config::FilesystemSnapshotUploadConfig::default(),
            ),
            false,
            &shutdown,
        )
        .unwrap();
        let agent = agent_snapshots("update-retention");
        let tree = tempfile::tempdir().unwrap();
        let older = (0..older)
            .map(|_| {
                crate::filesystem_snapshot::SnapshotName::new(
                    FilesystemSnapshotName::update().as_str(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let saved = futures::StreamExt::then(futures::stream::iter(&older), |name| {
            crate::filesystem_snapshot::FilesystemSnapshotStore::save(
                store.as_ref(),
                &agent,
                name,
                tree.path(),
                None,
                crate::filesystem_snapshot::never_cancelled(),
                &crate::filesystem_snapshot::Unlimited,
            )
        });
        futures::TryStreamExt::try_collect::<Vec<_>>(saved)
            .await
            .unwrap();
        let (mark, _) = marks();
        let scratch = crate::services::agent_filesystem::scratch_directory().await;
        let mut host = ScriptedHost {
            whole: std::sync::Mutex::new(Some(WholeCapture::Captured {
                capture: FilesystemCapture::empty_in(&scratch).await,
                mark,
            })),
            ..ScriptedHost::new()
        };
        let UpdateSnapshot::Saved {
            retention: Some(retention),
            ..
        } = update_snapshot(&mut host, &snapshots, &agent, AgentMode::Durable).await
        else {
            panic!("the manual update saves its snapshot with a retention");
        };
        let mut deleted = store.deleted.subscribe();

        retention.delete_older_snapshots(
            &kept
                .iter()
                .map(|index| {
                    older[*index]
                        .as_str()
                        .parse::<FilesystemSnapshotName>()
                        .unwrap()
                })
                .collect::<Box<[_]>>(),
        );
        let deleted = tokio::time::timeout(
            Duration::from_secs(10),
            deleted.wait_for(|deleted| !deleted.is_empty()),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(|deleted| deleted.clone())
        .unwrap_or_default();
        shut_down(shutdown).await;
        (
            deleted,
            older.iter().map(|name| Box::from(name.as_str())).collect(),
        )
    }

    #[test]
    async fn the_retention_of_a_saved_manual_update_deletes_the_update_snapshots_beyond_the_kept_count()
     {
        let (deleted, older) = update_retention(2, &[]).await;

        assert_eq!(deleted, vec![older[0].clone()]);
    }

    #[test]
    async fn update_retention_keeps_every_successful_update_name() {
        // The oldest name is kept, as the status of a successful update holds it; the next one is
        // beyond the count.
        let (deleted, older) = update_retention(3, &[0]).await;

        assert_eq!(deleted, vec![older[1].clone()]);
    }

    /// A snapshot record with the filesystem snapshot `name`.
    fn snapshot_record(name: Option<&FilesystemSnapshotName>) -> OplogEntry {
        OplogEntry::Snapshot {
            timestamp: golem_common::model::Timestamp::from(1_000),
            data: golem_common::model::oplog::OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
            active_cards: Vec::new(),
            wallet_generation: 0,
            filesystem_snapshot: name.cloned(),
        }
    }

    /// A snapshot-based update record with the filesystem snapshot `name`.
    fn update_record(name: &FilesystemSnapshotName) -> OplogEntry {
        OplogEntry::pending_update(UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::INITIAL,
            payload: golem_common::model::oplog::OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
            filesystem_snapshot: Some(name.clone()),
        })
    }

    fn region(start: u64, end: u64) -> golem_common::model::regions::OplogRegion {
        golem_common::model::regions::OplogRegion {
            start: OplogIndex::from_u64(start),
            end: OplogIndex::from_u64(end),
        }
    }

    fn oplog(
        records: Vec<(u64, OplogEntry)>,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        records
            .into_iter()
            .map(|(index, entry)| (OplogIndex::from_u64(index), entry))
            .collect()
    }

    #[test]
    fn names_inside_the_dropped_region_are_collected_newest_first() {
        let [p1, p2, p3] = [(); 3].map(|()| FilesystemSnapshotName::periodic());
        let entries = oplog(vec![
            (2, snapshot_record(Some(&p1))),
            (5, snapshot_record(Some(&p2))),
            (6, snapshot_record(None)),
            (8, snapshot_record(Some(&p3))),
        ]);

        assert_eq!(
            super::reverted_snapshot_names(
                &entries,
                &region(4, 9),
                &golem_common::model::regions::DeletedRegions::new()
            ),
            Box::from([p3, p2])
        );
    }

    #[test]
    fn a_snapshot_based_update_record_in_the_region_gives_its_name() {
        let u1 = FilesystemSnapshotName::update();
        let entries = oplog(vec![(5, update_record(&u1))]);

        assert_eq!(
            super::reverted_snapshot_names(
                &entries,
                &region(4, 9),
                &golem_common::model::regions::DeletedRegions::new()
            ),
            Box::from([u1])
        );
    }

    #[test]
    fn a_name_that_a_record_outside_the_region_uses_stays() {
        let [p1, p2] = [(); 2].map(|()| FilesystemSnapshotName::periodic());
        let entries = oplog(vec![
            (2, snapshot_record(Some(&p1))),
            (5, snapshot_record(Some(&p1))),
            (6, snapshot_record(Some(&p2))),
            (7, snapshot_record(Some(&p2))),
        ]);

        assert_eq!(
            super::reverted_snapshot_names(
                &entries,
                &region(4, 9),
                &golem_common::model::regions::DeletedRegions::new()
            ),
            Box::from([p2])
        );
    }

    #[test]
    fn records_in_an_earlier_deleted_region_do_not_keep_a_name() {
        let p1 = FilesystemSnapshotName::periodic();
        let entries = oplog(vec![
            (2, snapshot_record(Some(&p1))),
            (6, snapshot_record(Some(&p1))),
        ]);

        assert_eq!(
            super::reverted_snapshot_names(
                &entries,
                &region(5, 9),
                &golem_common::model::regions::DeletedRegions::from_regions([region(1, 3)])
            ),
            Box::from([p1])
        );
    }
}
