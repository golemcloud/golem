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
use crate::filesystem_snapshot::SnapshotScope;
use crate::services::agent_filesystem::{
    CaptureOutcome, ChangeDetection, FilesystemCapture, InitialFilesRestore, RestoreError,
    RestoreTree, TreeMark, WholeCapture,
};
use crate::services::agent_filesystem_snapshots::{
    Admission, AgentFilesystemSnapshots, Confirm, ConfirmOutcome, SavedUpdate, SnapshotSkip,
    SnapshotsDisabled, StoreRestore, UpdateRefusal, UploadNowError,
};
use crate::services::oplog::OplogError;
use crate::workerctx::WorkerCtx;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    FilesystemSnapshotName, OplogIndex, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::oplog::{OplogEntry, RawSnapshotData};
use golem_common::model::{AgentId, UsableAutomaticSnapshot};
use golem_service_base::error::worker_executor::WorkerExecutorError;
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
#[derive(Clone, Debug)]
pub(crate) struct FilesystemSnapshotSlot {
    /// The mark of the baseline of the current generation.
    generation: TreeMark,
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
            generation: mark,
            confirmed: restored.map(|(name, baseline)| ConfirmedFilesystemSnapshot {
                name,
                mark,
                baseline,
            }),
        }
    }

    /// Records the confirmation of `name`, whose capture has `mark`. A confirmation of another
    /// generation changes nothing: the loop starts a new generation without the instance lock,
    /// so the generation can change after the owner gate checked it.
    pub(crate) fn confirm(&mut self, name: FilesystemSnapshotName, mark: TreeMark) {
        if self.is_generation(&mark) {
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
        self.generation.same_generation(mark)
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

/// What a capture found. `Copy` is the copy that a capture made, which only a capture that
/// found a change has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureFinding<Copy> {
    Unchanged,
    InitialFiles,
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
            CaptureOutcome::InitialFiles => Self::InitialFiles,
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

/// The record of a periodic snapshot. `Copy` is the copy that the record uploads.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PeriodicRecord<Copy> {
    /// No record is written: the capture found no change against a confirmed snapshot that it
    /// did not have.
    Skipped,
    /// The record has no name. Nothing is uploaded.
    WithoutName,
    /// The record has the name of the admission, and `copy` is uploaded with `parent`.
    Own {
        copy: Copy,
        parent: Option<(FilesystemSnapshotName, StoreChangeDetection)>,
    },
    /// The record has the confirmed name, and its confirmation record follows it at once.
    Reused(FilesystemSnapshotName),
}

/// Decides the record of a periodic snapshot from the finding of the capture and the confirmed
/// snapshot that the capture compared with.
fn plan_periodic_record<Copy>(
    finding: CaptureFinding<Copy>,
    since: Option<&ConfirmedFilesystemSnapshot>,
) -> PeriodicRecord<Copy> {
    match (finding, since) {
        (CaptureFinding::Unchanged, Some(since)) => PeriodicRecord::Reused(since.name.clone()),
        (CaptureFinding::Unchanged, None) => PeriodicRecord::Skipped,
        (CaptureFinding::InitialFiles, _) => PeriodicRecord::WithoutName,
        (
            CaptureFinding::Captured {
                copy,
                detection: ChangeDetection::SizeMtime,
            },
            Some(since),
        ) => PeriodicRecord::Own {
            copy,
            parent: Some((since.name.clone(), StoreChangeDetection::SizeMtime)),
        },
        (CaptureFinding::Captured { copy, .. }, _) => PeriodicRecord::Own { copy, parent: None },
    }
}

/// What a periodic snapshot writes: the name of its record, the name whose confirmation record
/// follows the record at once, and the upload that starts after the record commits.
struct PeriodicPlan {
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
    fn new(
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
        match plan_periodic_record(CaptureFinding::of(outcome), since.as_ref()) {
            PeriodicRecord::Skipped => None,
            PeriodicRecord::WithoutName => Some(without_name),
            PeriodicRecord::Reused(name) => Some(Self {
                name: Some(name.clone()),
                confirmed_at_once: Some(name),
                upload: None,
            }),
            PeriodicRecord::Own {
                copy: (tree, mark),
                parent,
            } => Some(Self {
                name: Some(admission.name().clone()),
                confirmed_at_once: None,
                upload: Some(PendingUpload {
                    admission,
                    tree,
                    mark,
                    parent,
                }),
            }),
        }
    }

    /// The filesystem snapshot name of the record.
    fn name(&self) -> Option<FilesystemSnapshotName> {
        self.name.clone()
    }

    /// The name whose confirmation record follows the record at once, in one append.
    fn confirmed_at_once(&self) -> Option<FilesystemSnapshotName> {
        self.confirmed_at_once.clone()
    }

    /// Starts the upload of a written record. `confirm` gives the confirmation of the capture
    /// with the mark.
    fn submit(self, confirm: impl FnOnce(TreeMark) -> Confirm) {
        if let Some(PendingUpload {
            admission,
            tree,
            mark,
            parent,
        }) = self.upload
        {
            admission.submit(tree.into(), parent, confirm(mark));
        }
    }

    /// Drops the admission and discards the capture of a record that was not written.
    async fn abandon(self) {
        if let Some(PendingUpload {
            admission, tree, ..
        }) = self.upload
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
    fn save_guest(&mut self) -> impl Future<Output = Result<RawSnapshotData, Self::Stop>> + Send;
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

/// Takes a periodic snapshot of the agent of `scope`, in this order: admission, the
/// save hook of the guest, the capture, the record with the name, its commit and the checkpoint
/// of the status, then the upload. An admission that the service refuses skips the snapshot,
/// and a disabled service gives a record without a name. A capture that fails writes no
/// record. A record that does not reach the oplog drops the admission and discards the capture
/// at one place.
pub(crate) async fn periodic_snapshot<Host: PeriodicSnapshotHost>(
    host: &mut Host,
    snapshots: &AgentFilesystemSnapshots,
    scope: &SnapshotScope,
) -> PeriodicResult<Host::Stop> {
    let admission = match snapshots.admit_periodic(scope).await {
        Ok(admission) => Some(admission),
        Err(SnapshotSkip::Disabled) => None,
        Err(skip) => {
            tracing::debug!(reason = %skip, "Skipping periodic snapshot");
            return PeriodicResult::Continue;
        }
    };
    let snapshot = match host.save_guest().await {
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
            plan.submit(|mark| host.confirm(mark));
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
    fn save_guest(&mut self) -> impl Future<Output = Result<RawSnapshotData, Self::Stop>> + Send;
    /// Captures the whole tree, and gives `None` when the capture failed.
    fn capture_whole(&self, wait: Duration) -> impl Future<Output = Option<WholeCapture>> + Send;
    /// A receiver of whether a terminal interrupt waits for the agent.
    fn terminal(&self) -> watch::Receiver<bool>;
    /// Whether the shard of the agent is lost.
    fn lost_shard(&self) -> bool;
    /// The filesystem snapshot of the last successful manual update, which the retention of the
    /// update snapshot keeps.
    fn kept_baseline(&self) -> impl Future<Output = Option<FilesystemSnapshotName>> + Send;
}

/// The retention of a saved manual-update snapshot, with the snapshot that it keeps.
#[must_use = "the retention of a manual-update snapshot runs only after the update record commits"]
pub(crate) struct UpdateRetention {
    saved: SavedUpdate,
    kept: Option<FilesystemSnapshotName>,
}

impl UpdateRetention {
    /// Applies the retention in the background. Call it after the update record commits.
    pub(crate) fn retain(self) {
        self.saved.retain(self.kept.as_ref());
    }
}

/// How the snapshot part of a manual update ended.
pub(crate) enum UpdateSnapshot<Stop> {
    /// The store holds the filesystem snapshot `name`, or the tree holds only initial files and
    /// `name` is `None`. `retention` runs after the update record commits.
    Saved {
        snapshot: RawSnapshotData,
        name: Option<FilesystemSnapshotName>,
        retention: Option<UpdateRetention>,
    },
    /// The update fails with the details.
    Fail(String),
    /// The shard is lost. Nothing is written, and the update stays pending for the new owner.
    WriteNothing,
    /// The save hook of the guest ended the update.
    Guest(Stop),
}

/// Takes the snapshots of a manual update of the agent of `scope`, before the update record:
/// admission, the save hook of the guest, a capture of the whole tree, and the upload, which
/// the update waits for. An upload of a periodic snapshot of the agent can run at the
/// admission; the update waits for it once and asks again, so a frequent snapshot does not fail
/// the update. A terminal interrupt ends that wait or the upload and fails the update, except on
/// a lost shard: then nothing is written, and the update stays pending for the shard's new
/// owner. A disabled service gives a record without a name.
pub(crate) async fn update_snapshot<Host: UpdateSnapshotHost>(
    host: &mut Host,
    snapshots: &AgentFilesystemSnapshots,
    scope: &SnapshotScope,
) -> UpdateSnapshot<Host::Stop> {
    let admission = match snapshots.admit_update(scope, host.terminal()).await {
        Ok(admission) => Some(admission),
        Err(UpdateRefusal::Skip(SnapshotSkip::Disabled)) => None,
        Err(UpdateRefusal::Interrupted) => {
            return interrupted_update(UpdateInterruption::Wait, host.lost_shard());
        }
        Err(UpdateRefusal::Skip(skip)) => {
            return UpdateSnapshot::Fail(format!(
                "cannot take a filesystem snapshot for the update: {skip}"
            ));
        }
    };
    let snapshot = match host.save_guest().await {
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
    // A terminal interrupt stops the save, and the capture is discarded. An interrupted save
    // writes no record: a snapshot that its publish still leaves has no record, and nothing
    // selects it.
    let terminal = host.terminal();
    match admission.upload_now(tree.into(), terminal.clone()).await {
        Ok(saved) => UpdateSnapshot::Saved {
            snapshot,
            name: Some(name),
            retention: Some(UpdateRetention {
                saved,
                kept: host.kept_baseline().await,
            }),
        },
        Err(error) => failed_update_upload(&error, *terminal.borrow(), host.lost_shard()),
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

/// How a manual update whose upload failed with `error` ends. A failure while a terminal
/// interrupt waits counts as interrupted, because the interrupt stops the save.
fn failed_update_upload<Stop>(
    error: &UploadNowError,
    terminal_pending: bool,
    lost_shard: bool,
) -> UpdateSnapshot<Stop> {
    if terminal_pending {
        return interrupted_update(UpdateInterruption::Upload, lost_shard);
    }
    UpdateSnapshot::Fail(format!(
        "failed to upload the filesystem snapshot for the update: {error}"
    ))
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
                Some(name) => named(kind, Some(name)),
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
    use golem_common::model::Timestamp;
    use std::sync::Arc;
    use test_r::test;

    /// Gives a mark of a new generation and a later mark of the same generation.
    fn marks() -> (TreeMark, TreeMark) {
        crate::services::agent_filesystem::test_tree_marks()
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

    fn captured(detection: ChangeDetection) -> CaptureFinding<u8> {
        CaptureFinding::Captured { copy: 1, detection }
    }

    #[test]
    async fn the_record_of_a_periodic_snapshot_follows_the_finding_of_the_capture() {
        let (mark, _) = marks();
        let name = FilesystemSnapshotName::periodic();
        let since = confirmed(&name, mark);

        let cases = [
            plan_periodic_record(CaptureFinding::Unchanged, Some(&since)),
            plan_periodic_record(CaptureFinding::Unchanged, None),
            plan_periodic_record(CaptureFinding::InitialFiles, Some(&since)),
            plan_periodic_record(CaptureFinding::InitialFiles, None),
            plan_periodic_record(captured(ChangeDetection::SizeMtime), Some(&since)),
            plan_periodic_record(captured(ChangeDetection::Full), Some(&since)),
            plan_periodic_record(captured(ChangeDetection::Full), None),
        ];

        assert_eq!(
            cases,
            [
                PeriodicRecord::Reused(name.clone()),
                PeriodicRecord::Skipped,
                PeriodicRecord::WithoutName,
                PeriodicRecord::WithoutName,
                PeriodicRecord::Own {
                    copy: 1,
                    parent: Some((name, StoreChangeDetection::SizeMtime)),
                },
                PeriodicRecord::Own {
                    copy: 1,
                    parent: None
                },
                PeriodicRecord::Own {
                    copy: 1,
                    parent: None
                },
            ]
        );
    }

    #[test]
    async fn only_the_owner_of_the_agent_passes_the_gate_and_the_admission_is_asked_last() {
        let (mark, _) = marks();
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
    async fn the_revision_before_an_update_record_follows_the_oplog_order_and_not_the_clocks() {
        let update =
            |millis: u64, revision: u64, index: u64| golem_common::model::SuccessfulUpdateRecord {
                timestamp: Timestamp::from(millis),
                target_revision: ComponentRevision::new(revision).unwrap(),
                oplog_index: OplogIndex::from_u64(index),
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

    /// A host that records its calls and answers as the test says.
    struct ScriptedHost {
        calls: std::sync::Mutex<Vec<String>>,
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
            self.calls.lock().unwrap().push(call);
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn named(name: Option<&FilesystemSnapshotName>) -> String {
        name.map_or_else(|| "none".to_string(), |name| name.as_str().to_string())
    }

    impl PeriodicSnapshotHost for ScriptedHost {
        type Stop = &'static str;

        async fn save_guest(&mut self) -> Result<RawSnapshotData, &'static str> {
            self.call("save_guest".to_string());
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
            Box::new(|_| Box::pin(async { ConfirmOutcome::Deferred }))
        }
    }

    impl UpdateSnapshotHost for ScriptedHost {
        type Stop = &'static str;

        async fn save_guest(&mut self) -> Result<RawSnapshotData, &'static str> {
            PeriodicSnapshotHost::save_guest(self).await
        }

        async fn capture_whole(&self, _wait: Duration) -> Option<WholeCapture> {
            self.call("capture_whole".to_string());
            self.whole.lock().unwrap().take()
        }

        fn terminal(&self) -> watch::Receiver<bool> {
            self.terminal.subscribe()
        }

        fn lost_shard(&self) -> bool {
            self.lost_shard
        }

        async fn kept_baseline(&self) -> Option<FilesystemSnapshotName> {
            self.call("kept_baseline".to_string());
            None
        }
    }

    fn agent_scope(name: &str) -> SnapshotScope {
        SnapshotScope::agent(&golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId::new(),
            &AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: name.to_string(),
            },
        ))
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
                Arc::new(crate::filesystem_snapshot::InMemorySnapshotStore::default()),
                crate::services::golem_config::FilesystemSnapshotUploadConfig::default(),
            ),
            false,
            &shutdown,
        )
        .unwrap();
        (snapshots, shutdown)
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
        let (disabled, _disabled_shutdown) = disabled_service();
        let scope = agent_scope("periodic-disabled");
        let run = |host: ScriptedHost| {
            let disabled = disabled.as_ref();
            let scope = &scope;
            async move {
                let mut host = host;
                let result = periodic_snapshot(&mut host, disabled, scope).await;
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
                vec!["save_guest", "entry(none)", "write(none)"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            )
        );
        assert_eq!(
            guest_stop,
            (
                "Guest(\"stop\")".to_string(),
                vec!["save_guest".to_string()]
            )
        );
        assert_eq!(
            no_entry,
            (
                "NotWritten(Entry(\"no payload\"))".to_string(),
                vec!["save_guest".to_string(), "entry(none)".to_string()]
            )
        );
        assert_eq!(
            not_written.0,
            "NotWritten(Write(Payload(\"refused\")))".to_string()
        );
    }

    #[test]
    async fn a_periodic_snapshot_admits_before_the_guest_saves_and_captures_after() {
        let (mark, _) = marks();
        let (snapshots, _shutdown) = enabled_service();
        let scope = agent_scope("periodic-enabled");
        let since = confirmed(&FilesystemSnapshotName::periodic(), mark);

        let held = snapshots.admit_periodic(&scope).await.unwrap();
        let mut refused = ScriptedHost::new();
        let while_held = outcome(&periodic_snapshot(&mut refused, &snapshots, &scope).await);
        drop(held);
        let mut failed_capture = ScriptedHost::new();
        let capture_failed =
            outcome(&periodic_snapshot(&mut failed_capture, &snapshots, &scope).await);
        let mut initial = ScriptedHost {
            capture: std::sync::Mutex::new(Some(CaptureOutcome::InitialFiles)),
            ..ScriptedHost::new()
        };
        let initial_files = outcome(&periodic_snapshot(&mut initial, &snapshots, &scope).await);
        let mut unchanged = ScriptedHost {
            since: Some(since.clone()),
            capture: std::sync::Mutex::new(Some(CaptureOutcome::Unchanged)),
            ..ScriptedHost::new()
        };
        let reused = outcome(&periodic_snapshot(&mut unchanged, &snapshots, &scope).await);
        let free_after = snapshots.admit_periodic(&scope).await.is_ok();

        let name = since.name.as_str().to_string();
        assert_eq!(
            (while_held, refused.calls()),
            ("Continue".to_string(), vec![])
        );
        assert_eq!(
            (capture_failed, failed_capture.calls()),
            (
                "Continue".to_string(),
                vec!["save_guest", "since", "capture(false)"]
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
                    "save_guest",
                    "since",
                    "capture(false)",
                    "entry(none)",
                    "write(none)"
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
                    "save_guest".to_string(),
                    "since".to_string(),
                    "capture(true)".to_string(),
                    format!("entry({name})"),
                    format!("write({name})"),
                ]
            )
        );
        assert!(free_after);
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
        let (disabled, _disabled_shutdown) = disabled_service();
        let (snapshots, _shutdown) = enabled_service();
        let scope = agent_scope("update");

        let mut without = ScriptedHost::new();
        let without_snapshots =
            update_outcome(&update_snapshot(&mut without, &disabled, &scope).await);
        let mut stopped = ScriptedHost {
            guest: Some(Err("stop")),
            ..ScriptedHost::new()
        };
        let guest_stop = update_outcome(&update_snapshot(&mut stopped, &snapshots, &scope).await);
        let mut failed = ScriptedHost::new();
        let capture_failed =
            update_outcome(&update_snapshot(&mut failed, &snapshots, &scope).await);
        let mut initial = ScriptedHost {
            whole: std::sync::Mutex::new(Some(WholeCapture::InitialFiles)),
            ..ScriptedHost::new()
        };
        let initial_files =
            update_outcome(&update_snapshot(&mut initial, &snapshots, &scope).await);

        assert_eq!(
            (without_snapshots, without.calls()),
            (
                "Saved(none, false)".to_string(),
                vec!["save_guest".to_string()]
            )
        );
        assert_eq!(guest_stop, "Guest(stop)");
        assert_eq!(
            (capture_failed, failed.calls()),
            (
                "Fail(failed to capture the agent filesystem for the update)".to_string(),
                vec!["save_guest".to_string(), "capture_whole".to_string()]
            )
        );
        assert_eq!(initial_files, "Saved(none, false)");
        assert!(snapshots.admit_periodic(&scope).await.is_ok());
    }

    #[test]
    async fn an_interrupted_wait_of_a_manual_update_fails_it_or_writes_nothing_on_a_lost_shard() {
        let (snapshots, _shutdown) = enabled_service();
        let scope = agent_scope("update-interrupted");
        let held = snapshots.admit_periodic(&scope).await.unwrap();
        let run = |lost_shard| {
            let snapshots = &snapshots;
            let scope = &scope;
            async move {
                let mut host = ScriptedHost {
                    lost_shard,
                    ..ScriptedHost::new()
                };
                host.terminal.send_replace(true);
                let result = update_snapshot(&mut host, snapshots, scope).await;
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
    }

    #[test]
    async fn a_failed_update_upload_is_interrupted_while_a_terminal_interrupt_waits() {
        let store = UploadNowError::Store(crate::filesystem_snapshot::SnapshotStoreError::NotFound);
        let uploaded = |error: &UploadNowError, terminal: bool, lost_shard: bool| {
            update_outcome(&failed_update_upload(error, terminal, lost_shard))
        };
        assert_eq!(
            [
                uploaded(&UploadNowError::Stopped, true, false),
                uploaded(&store, true, false),
                uploaded(&store, true, true),
                uploaded(&UploadNowError::Stopped, false, false),
                uploaded(&store, false, true),
            ],
            [
                "Fail(the update was interrupted while it uploaded the filesystem snapshot)",
                "Fail(the update was interrupted while it uploaded the filesystem snapshot)",
                "WriteNothing",
                "Fail(failed to upload the filesystem snapshot for the update: the upload of the \
                 filesystem snapshot was stopped)",
                "Fail(failed to upload the filesystem snapshot for the update: no complete \
                 filesystem snapshot has the name)",
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
