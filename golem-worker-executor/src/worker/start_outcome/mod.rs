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

//! The outcome of a start that failed: one table decides, for the baseline of the start and the
//! update at the head of its queue, what the start does with each failure, and builds every
//! failed update entry from the paired queue element.
//!
//! A failed update of a snapshot-assisted update carries the assisted details of its queue
//! element. Its `snapshot_fault` says whether the failure is about the selected record. A failure
//! with one of the causes below starts its details with a stable code.

use crate::durable_host::revision_update::UpdateStateError;
use crate::model::SnapshotReplayPurpose;
use crate::services::agent_filesystem::{Error as FilesystemError, RestoreClass};
use crate::services::agent_filesystem_snapshots::SnapshotsDisabled;
use crate::worker::RetryDecision;
use crate::worker::snapshot_selection::SourceFound;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    AgentError, FailedSnapshotAssistedUpdateDetails, OplogEntry, OplogIndex,
    SnapshotAssistedUpdateDetails, SnapshotFault,
};
use golem_common::model::{AgentId, PendingUpdateKind, PendingUpdateRef, Timestamp};
use golem_service_base::error::worker_executor::{
    ComponentServiceRefusal, InterruptKind, WorkerExecutorError,
};
use golem_service_base::storage::blob::BlobRefusal;
use std::sync::Arc;

/// The selected record of a snapshot-assisted update, or the record of a pending snapshot-based
/// manual update, is no longer in the store.
pub(crate) const UPDATE_SNAPSHOT_UNAVAILABLE: &str = "UPDATE_SNAPSHOT_UNAVAILABLE";
/// The target of a snapshot-assisted update could not load its selected record, or the history
/// after the record diverged.
pub(crate) const UPDATE_SNAPSHOT_INCOMPATIBLE: &str = "UPDATE_SNAPSHOT_INCOMPATIBLE";
/// The target of an automatic update could not replay the history of the agent.
pub(crate) const UPDATE_REPLAY_FAILED: &str = "UPDATE_REPLAY_FAILED";
/// The baseline of a pending update names a filesystem snapshot on an executor without
/// filesystem snapshots.
pub(crate) const UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS: &str =
    "UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS";
/// The filesystem snapshot of a pending update does not restore on this executor.
pub(crate) const UPDATE_SNAPSHOT_RESTORE_FAILED: &str = "UPDATE_SNAPSHOT_RESTORE_FAILED";
/// The target revision of an update does not exist.
pub(crate) const UPDATE_TARGET_NOT_FOUND: &str = "UPDATE_TARGET_NOT_FOUND";
/// The component service or this executor refused the target revision of an update.
pub(crate) const UPDATE_TARGET_REFUSED: &str = "UPDATE_TARGET_REFUSED";
/// The blob storage refused the executor a read of an initial-file source of the target revision
/// at the update point.
pub(crate) const UPDATE_BLOB_STORAGE_REFUSED: &str = "UPDATE_BLOB_STORAGE_REFUSED";

/// The baseline of a start: the column of the outcome table, without the head of a plain
/// automatic update, which [`decide`] takes on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BaselineRole {
    /// The periodic record at the index.
    Periodic(OplogIndex),
    /// The record of the snapshot-based manual update at the head of the queue.
    ManualPending(Arc<PendingUpdateRef>),
    /// The authoritative baseline of a successful snapshot-based manual update.
    ManualPromoted,
    /// The record that the snapshot-assisted update at the head of the queue selected.
    AssistedPending(Arc<PendingUpdateRef>),
    /// The authoritative baseline of a successful snapshot-assisted update.
    AssistedPromoted,
    /// No record.
    InitialFiles,
}

impl BaselineRole {
    /// What the replay of a start from this baseline is for, with `head` at the head of the
    /// queue. A pending plain automatic update replays the history after a promoted baseline or
    /// the whole history, and that replay is for the update.
    pub(crate) fn purpose(&self, head: Option<&PendingUpdateRef>) -> SnapshotReplayPurpose {
        match (self, head.map(|head| &head.kind)) {
            (Self::Periodic(_), _) => SnapshotReplayPurpose::PeriodicRecovery,
            (Self::AssistedPending(_), _) => SnapshotReplayPurpose::AssistedUpdate,
            (
                Self::ManualPromoted | Self::AssistedPromoted | Self::InitialFiles,
                Some(PendingUpdateKind::Automatic),
            ) => SnapshotReplayPurpose::AutomaticUpdate,
            (
                Self::ManualPending(_)
                | Self::ManualPromoted
                | Self::AssistedPromoted
                | Self::InitialFiles,
                _,
            ) => SnapshotReplayPurpose::None,
        }
    }
}

/// What a load of an application snapshot gave.
pub(crate) enum SnapshotRecoveryResult {
    Success,
    NotAttempted,
    Failed(WorkerExecutorError),
    Unavailable(WorkerExecutorError),
    /// The payload of the application snapshot of the selected record of a snapshot-assisted
    /// update is missing or does not decode: the record is lost.
    Lost(WorkerExecutorError),
    Retry(RetryDecision),
}

/// What the load of the application snapshot of a pending snapshot-based manual update gave,
/// when it did not load.
pub(crate) enum ManualLoadResult {
    /// The load failed with the details.
    Failed(String),
    /// The load was interrupted with the interrupt kind.
    Interrupted(InterruptKind),
    /// The guest exited during the load.
    Exited,
}

/// The raw failure of a start, as the place that found it has it.
#[derive(Clone, Copy)]
pub(crate) enum RawStartError<'a> {
    /// The source of a selected snapshot-assisted update does not hold.
    StaleSource(&'a SourceFound),
    /// The target revision of the pending update could not be fetched.
    TargetFetch(&'a WorkerExecutorError),
    /// The target component could not be loaded or compiled.
    TargetLoad(&'a WorkerExecutorError),
    /// The executor does not support the target revision; the text says why.
    TargetUnsupported(&'a str),
    /// The target revision changes the mode of the agent type; the text says how.
    ModeChange(&'a str),
    /// The baseline names a filesystem snapshot, and this executor keeps none.
    Disabled,
    /// The baseline did not materialize.
    Filesystem(&'a FilesystemError),
    /// The instance did not instantiate.
    Instantiation(&'a WorkerExecutorError),
    /// The application snapshot of the baseline did not load.
    Load(&'a SnapshotRecoveryResult),
    /// The application snapshot of a pending manual update did not load.
    ManualLoad(&'a ManualLoadResult),
    /// The replay failed. `diverged` tells whether the replay after an application snapshot
    /// diverged from the oplog with a typed divergence.
    Replay {
        error: &'a WorkerExecutorError,
        diverged: bool,
    },
    /// The update of the instance to the target revision failed.
    UpdateState(&'a UpdateStateError),
    /// The agent filesystem did not finish its reconstruction after the replay, when the update
    /// of the start is already decided.
    Finish(&'a FilesystemError),
}

/// What the start does with a failure.
#[derive(Debug)]
pub(crate) enum StartAction {
    /// Write `entry`, a failed update; reject the automatic snapshot record `reject` when set;
    /// and start again on the source revision.
    FailUpdate {
        entry: OplogEntry,
        reject: Option<OplogIndex>,
    },
    /// Skip the periodic record at the index for this start attempt, and start again.
    SkipPeriodic(OplogIndex),
    /// Reject the periodic record at the index, and start again.
    RejectPeriodic(OplogIndex),
    /// End the start with this error; the recovery path handles it.
    Error(WorkerExecutorError),
    /// Retry the start with this decision.
    Retry(RetryDecision),
    /// Write nothing; the update stays pending for the new owner of the shard.
    ShardLost,
}

/// Decides what a start does with `error`. `role` is the baseline of the start, `head` the update
/// at the head of its queue, `agent_id` the agent, and `lost_shard` whether this executor lost
/// the shard of the agent.
pub(crate) fn decide(
    role: &BaselineRole,
    head: Option<&PendingUpdateRef>,
    error: RawStartError<'_>,
    agent_id: &AgentId,
    lost_shard: bool,
) -> StartAction {
    let Some(column) = column(role, head) else {
        return StartAction::Error(WorkerExecutorError::runtime(format!(
            "a start from the baseline {role:?} cannot have the pending update {head:?}"
        )));
    };
    let problem = StartProblem::of(&error);
    match cell(column, problem) {
        Cell::Skip => periodic_index(role).map_or_else(
            || StartAction::Error(error.passed_through()),
            StartAction::SkipPeriodic,
        ),
        Cell::Reject => periodic_index(role).map_or_else(
            || StartAction::Error(error.passed_through()),
            StartAction::RejectPeriodic,
        ),
        Cell::Fail(failure) => match (lost_shard, update_of(role, head)) {
            (true, Some(_)) => StartAction::ShardLost,
            (false, Some(head)) => StartAction::FailUpdate {
                entry: failed_update_of(
                    head,
                    details(failure.code, &cause(column, head, &error)),
                    failure.fault,
                ),
                reject: failure
                    .reject_selected
                    .then(|| selected_index(head))
                    .flatten(),
            },
            (_, None) => StartAction::Error(error.passed_through()),
        },
        Cell::Visible => StartAction::Error(WorkerExecutorError::failed_to_resume_worker(
            agent_id.clone(),
            WorkerExecutorError::invalid_request(raw_text(&error)),
        )),
        Cell::VisibleInvocation => StartAction::Error(WorkerExecutorError::InvocationFailed {
            error: AgentError::InternalError(raw_text(&error)),
            stderr: String::new(),
        }),
        Cell::Pass => StartAction::Error(error.passed_through()),
        Cell::Retry => match error {
            RawStartError::Load(SnapshotRecoveryResult::Retry(decision)) => {
                StartAction::Retry(decision.clone())
            }
            _ => StartAction::Error(error.passed_through()),
        },
    }
}

/// The failed update of the manual update invocation at `admission_index`, which no
/// `PendingUpdate` entry paired.
pub(crate) fn failed_admission_of(
    target_revision: ComponentRevision,
    admission_index: OplogIndex,
    details: String,
) -> OplogEntry {
    OplogEntry::failed_update(
        target_revision,
        Some(details),
        None,
        Some(admission_index),
        None,
    )
}

/// The failed update that ends `head` without an error, such as a cancellation: it carries the
/// attempt index and the snapshot-assisted details of `head`, and no snapshot fault.
pub(crate) fn cancelled_update_of(head: &PendingUpdateRef, details: String) -> OplogEntry {
    failed_update_of(head, details, None)
}

/// The details of a successful update of `head`: its snapshot-assisted details, or `None` for
/// any other kind.
pub(crate) fn success_details_of(head: &PendingUpdateRef) -> Option<SnapshotAssistedUpdateDetails> {
    match &head.kind {
        PendingUpdateKind::SnapshotAssistedAutomatic(selection) => {
            Some(SnapshotAssistedUpdateDetails {
                pending_update_index: head.oplog_index,
                source_component_revision: selection.snapshot.component_revision,
                source_revision_start_index: selection.source_revision_start_index,
                snapshot_index: selection.snapshot.index,
            })
        }
        PendingUpdateKind::Automatic | PendingUpdateKind::SnapshotBased { .. } => None,
    }
}

/// The error of a start whose agent filesystem failed with `error`: a suspension when the quota
/// of the agent is full, else a runtime error, which the recovery path retries.
pub(crate) fn reconstruction_startup_error(error: &FilesystemError) -> WorkerExecutorError {
    match error {
        FilesystemError::AgentQuota(_) => WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(Timestamp::now_utc()),
        },
        error => WorkerExecutorError::runtime(error.to_string()),
    }
}

/// The error of a failed initial-file rule at the update point: a full quota suspends, an
/// unavailable initial-file source retries as a recovery, and every other error is a
/// reconstruction error ([`reconstruction_startup_error`]). A refused initial-file source reaches
/// this only in a column that fails no update, because every pending column fails the update
/// for it.
///
/// The update point can be inside a host call of a replayed invocation, where only a required
/// recovery stays off the guest's failure path. An unavailable source is transient: the install
/// loads every source before its first change, so the agent filesystem and the frozen attempt
/// stay as they were, and the same attempt runs again.
pub(crate) fn update_point_filesystem_error(error: &FilesystemError) -> WorkerExecutorError {
    match error {
        FilesystemError::InitialFileUnavailable(_) => {
            WorkerExecutorError::recovery_required(format!("the update point failed: {error}"))
        }
        error => reconstruction_startup_error(error),
    }
}

/// The error that ends a start at the update point for the passed error `error`. The update
/// stays pending and the start runs again. The update point can be inside a host call of a
/// replayed invocation, where only a required recovery stays off the guest's failure path, so the
/// cause that the outcome table passes because it is transient, an unavailable component service,
/// retries as a recovery. Every other error, such as the suspension of a full quota, stays as it
/// is.
pub(crate) fn update_point_error(error: WorkerExecutorError) -> WorkerExecutorError {
    match FetchProblem::of(&error) {
        FetchProblem::Unavailable => {
            WorkerExecutorError::recovery_required(format!("the update point failed: {error}"))
        }
        FetchProblem::NotFound | FetchProblem::Refused(_) | FetchProblem::Other => error,
    }
}

/// The column of the outcome table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Periodic,
    ManualPending,
    ManualPromoted,
    InitialFiles,
    /// A pending plain automatic update, whose baseline is the base column.
    AutomaticPending(Base),
    AssistedPending,
    AssistedPromoted,
}

/// The baseline of a start with a pending plain automatic update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    ManualPromoted,
    AssistedPromoted,
    InitialFiles,
}

impl Base {
    fn column(self) -> Column {
        match self {
            Self::ManualPromoted => Column::ManualPromoted,
            Self::AssistedPromoted => Column::AssistedPromoted,
            Self::InitialFiles => Column::InitialFiles,
        }
    }
}

/// The column of a start from `role` with the update `head` at the head of its queue.
///
/// `None` for a combination that a start never has: a snapshot-based or a snapshot-assisted head
/// always gives its pending baseline, and the selection gives no periodic record while any update
/// is pending.
fn column(role: &BaselineRole, head: Option<&PendingUpdateRef>) -> Option<Column> {
    match (role, head.map(|head| &head.kind)) {
        (BaselineRole::ManualPending(_), _) => Some(Column::ManualPending),
        (BaselineRole::AssistedPending(_), _) => Some(Column::AssistedPending),
        (
            _,
            Some(
                PendingUpdateKind::SnapshotBased { .. }
                | PendingUpdateKind::SnapshotAssistedAutomatic(_),
            ),
        )
        | (BaselineRole::Periodic(_), Some(PendingUpdateKind::Automatic)) => None,
        (BaselineRole::ManualPromoted, Some(PendingUpdateKind::Automatic)) => {
            Some(Column::AutomaticPending(Base::ManualPromoted))
        }
        (BaselineRole::AssistedPromoted, Some(PendingUpdateKind::Automatic)) => {
            Some(Column::AutomaticPending(Base::AssistedPromoted))
        }
        (BaselineRole::InitialFiles, Some(PendingUpdateKind::Automatic)) => {
            Some(Column::AutomaticPending(Base::InitialFiles))
        }
        (BaselineRole::Periodic(_), None) => Some(Column::Periodic),
        (BaselineRole::ManualPromoted, None) => Some(Column::ManualPromoted),
        (BaselineRole::AssistedPromoted, None) => Some(Column::AssistedPromoted),
        (BaselineRole::InitialFiles, None) => Some(Column::InitialFiles),
    }
}

/// Why a fetch of a component revision failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FetchProblem {
    /// The component service did not answer.
    Unavailable,
    /// The revision does not exist.
    NotFound,
    /// The component service refused the request.
    Refused(ComponentServiceRefusal),
    /// Any other failure.
    Other,
}

impl FetchProblem {
    pub(super) fn of(error: &WorkerExecutorError) -> Self {
        match error {
            WorkerExecutorError::ComponentServiceUnavailable { .. } => Self::Unavailable,
            WorkerExecutorError::ComponentNotFound { .. } => Self::NotFound,
            WorkerExecutorError::ComponentServiceRefused { kind, .. } => Self::Refused(*kind),
            _ => Self::Other,
        }
    }
}

/// The row of the outcome table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartProblem {
    StaleSource,
    Target(FetchProblem),
    TargetExecutor,
    TargetUnsupported,
    ModeChange,
    Disabled,
    Restore(RestoreClass),
    RestoreConflict,
    Quota,
    Reconstruction,
    Instantiation {
        interrupted: bool,
    },
    LoadRetry,
    LoadUnavailable,
    /// The payload of the selected record is lost; only a snapshot-assisted attempt reports it.
    LoadLost,
    LoadFailed,
    ManualLoadFailed,
    ManualLoadInterrupted,
    ManualLoadExited,
    RecoveryRequired,
    Interrupted,
    AgentFailed,
    Divergence,
    ReplayError,
    /// The metadata of the target could not be fetched at the update point; `Other` stands for
    /// every other source of the update of the instance, and for a conflict of the initial-file
    /// rule.
    UpdateState(FetchProblem),
    /// The agent filesystem failed after the replay.
    AfterReplay,
    /// The blob storage refused the executor a read of an initial-file source of the target at
    /// the update point.
    BlobStorageRefused(BlobRefusal),
    /// The oplog of the agent failed: it does not read or an archive layer did not store it. It is
    /// the row of every such error, whatever the
    /// step of the start that read the oplog.
    OplogFailed,
}

impl StartProblem {
    fn of(error: &RawStartError<'_>) -> Self {
        if error.oplog_failed() {
            return Self::OplogFailed;
        }
        match error {
            RawStartError::StaleSource(_) => Self::StaleSource,
            RawStartError::TargetFetch(error) => Self::Target(FetchProblem::of(error)),
            RawStartError::TargetLoad(error) => match error {
                WorkerExecutorError::Runtime { .. }
                | WorkerExecutorError::RecoveryRequired { .. }
                | WorkerExecutorError::Interrupted { .. } => Self::TargetExecutor,
                _ => Self::Target(FetchProblem::of(error)),
            },
            RawStartError::TargetUnsupported(_) => Self::TargetUnsupported,
            RawStartError::ModeChange(_) => Self::ModeChange,
            RawStartError::Disabled => Self::Disabled,
            RawStartError::Filesystem(error) => match error {
                FilesystemError::Baseline(error) => Self::Restore(error.class),
                FilesystemError::InitialFileConflict(_) => Self::RestoreConflict,
                FilesystemError::AgentQuota(_) => Self::Quota,
                FilesystemError::Access(_)
                | FilesystemError::Sandbox(_)
                | FilesystemError::InitialFileUnavailable(_)
                | FilesystemError::InitialFileRefused { .. }
                | FilesystemError::PhysicalCapacity(_)
                | FilesystemError::RuntimeInvalidated => Self::Reconstruction,
            },
            RawStartError::Finish(_) => Self::AfterReplay,
            RawStartError::Instantiation(error) => Self::Instantiation {
                interrupted: matches!(error, WorkerExecutorError::Interrupted { .. }),
            },
            RawStartError::Load(result) => match result {
                SnapshotRecoveryResult::Retry(_) => Self::LoadRetry,
                SnapshotRecoveryResult::Unavailable(error) => match error {
                    WorkerExecutorError::RecoveryRequired { .. } => Self::RecoveryRequired,
                    WorkerExecutorError::Interrupted { .. } => Self::Interrupted,
                    _ => Self::LoadUnavailable,
                },
                SnapshotRecoveryResult::Lost(_) => Self::LoadLost,
                SnapshotRecoveryResult::Failed(_) => Self::LoadFailed,
                SnapshotRecoveryResult::Success | SnapshotRecoveryResult::NotAttempted => {
                    Self::ReplayError
                }
            },
            RawStartError::ManualLoad(ManualLoadResult::Failed(_)) => Self::ManualLoadFailed,
            RawStartError::ManualLoad(ManualLoadResult::Interrupted(_)) => {
                Self::ManualLoadInterrupted
            }
            RawStartError::ManualLoad(ManualLoadResult::Exited) => Self::ManualLoadExited,
            RawStartError::Replay { error, diverged } => match error {
                WorkerExecutorError::UnexpectedOplogEntry { .. } => Self::Divergence,
                _ if *diverged => Self::Divergence,
                WorkerExecutorError::RecoveryRequired { .. } => Self::RecoveryRequired,
                WorkerExecutorError::Interrupted { .. } => Self::Interrupted,
                WorkerExecutorError::PreviousInvocationFailed { .. }
                | WorkerExecutorError::PreviousInvocationExited => Self::AgentFailed,
                _ => Self::ReplayError,
            },
            RawStartError::UpdateState(UpdateStateError::Metadata(error)) => {
                Self::UpdateState(FetchProblem::of(error))
            }
            // The initial-file rule at the update point fails the update for a conflict and for a
            // source that the blob storage refuses; the other filesystem errors retry the start,
            // as they do at the restore. The update point restores nothing, so a restore error
            // counts as a reconstruction error. `update_point_filesystem_error` gives the error of
            // each passed one.
            RawStartError::UpdateState(UpdateStateError::InitialFiles(error)) => match error {
                FilesystemError::InitialFileConflict(_) => Self::UpdateState(FetchProblem::Other),
                FilesystemError::InitialFileRefused { refusal, .. } => {
                    Self::BlobStorageRefused(*refusal)
                }
                FilesystemError::AgentQuota(_) => Self::Quota,
                FilesystemError::Access(_)
                | FilesystemError::Sandbox(_)
                | FilesystemError::InitialFileUnavailable(_)
                | FilesystemError::PhysicalCapacity(_)
                | FilesystemError::RuntimeInvalidated
                | FilesystemError::Baseline(_) => Self::Reconstruction,
            },
            RawStartError::UpdateState(
                UpdateStateError::MissingAgentType(_)
                | UpdateStateError::Config(_)
                | UpdateStateError::WalletCards(_),
            ) => Self::UpdateState(FetchProblem::Other),
        }
    }
}

/// A stable code of the details of a failed update, with the text that follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Code {
    /// The selected record of a snapshot-assisted update is lost.
    SnapshotUnavailable,
    /// The record of a pending manual update is lost.
    ManualSnapshotUnavailable,
    SnapshotIncompatible,
    ReplayFailed,
    RestoreNeedsFilesystemSnapshots,
    SnapshotRestoreFailed,
    /// The filesystem snapshot of a pending update does not restore because the local disk of
    /// the executor is full.
    SnapshotRestoreDiskFull,
    TargetNotFound,
    TargetRefused(ComponentServiceRefusal),
    TargetUnsupported,
    BlobStorageRefused(BlobRefusal),
}

impl Code {
    fn prefix(self) -> String {
        match self {
            Self::SnapshotUnavailable => format!(
                "{UPDATE_SNAPSHOT_UNAVAILABLE}: the snapshot that the update selected is no \
                 longer available; request the update again: Golem uses an earlier snapshot or \
                 replays the full history"
            ),
            Self::ManualSnapshotUnavailable => format!(
                "{UPDATE_SNAPSHOT_UNAVAILABLE}: the snapshot of the manual update is no longer \
                 available; request the manual update again; it takes a new snapshot"
            ),
            Self::SnapshotIncompatible => format!(
                "{UPDATE_SNAPSHOT_INCOMPATIBLE}: the new code could not load the selected \
                 snapshot or replay the history after it; requesting this automatic update again \
                 replays the full history; if the new code cannot replay the agent's history, \
                 use a manual snapshot-based update"
            ),
            Self::ReplayFailed => format!(
                "{UPDATE_REPLAY_FAILED}: the new code could not replay the agent's history; use a \
                 manual snapshot-based update"
            ),
            Self::RestoreNeedsFilesystemSnapshots => format!(
                "{UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS}: the update needs a filesystem \
                 snapshot, and filesystem snapshots are disabled on this executor; run the update \
                 on an executor with filesystem snapshots"
            ),
            Self::SnapshotRestoreFailed => format!(
                "{UPDATE_SNAPSHOT_RESTORE_FAILED}: the filesystem snapshot of the update could \
                 not be restored on this executor; request the update again"
            ),
            Self::SnapshotRestoreDiskFull => format!(
                "{UPDATE_SNAPSHOT_RESTORE_FAILED}: the executor's local disk is full; request the \
                 update again when it has space"
            ),
            Self::TargetNotFound => format!(
                "{UPDATE_TARGET_NOT_FOUND}: the target revision does not exist; request an \
                 update to an existing revision"
            ),
            Self::TargetRefused(kind) => format!(
                "{UPDATE_TARGET_REFUSED}: the component service refused the target revision \
                 ({kind}); check the executor's access to the component service and the target \
                 revision, then request the update again"
            ),
            Self::TargetUnsupported => UPDATE_TARGET_REFUSED.to_string(),
            Self::BlobStorageRefused(refusal) => format!(
                "{UPDATE_BLOB_STORAGE_REFUSED}: the executor cannot read blob storage \
                 ({refusal}); request the update again after the storage is fixed"
            ),
        }
    }
}

/// A cell that fails the update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Failure {
    code: Option<Code>,
    fault: Option<SnapshotFault>,
    /// Whether the selected record of the snapshot-assisted update is rejected.
    reject_selected: bool,
}

impl Failure {
    /// `F`: a failed update without a code.
    const PLAIN: Self = Self {
        code: None,
        fault: None,
        reject_selected: false,
    };

    /// A failed update with `code`.
    const fn coded(code: Code) -> Self {
        Self {
            code: Some(code),
            fault: None,
            reject_selected: false,
        }
    }
}

/// A cell of the outcome table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell {
    /// `S`: skip the periodic record for the start attempt.
    Skip,
    /// `R`: reject the periodic record.
    Reject,
    /// `F` and its coded forms.
    Fail(Failure),
    /// `V`: fail the start with a visible cause.
    Visible,
    /// `V2`: fail the start as a failed invocation.
    VisibleInvocation,
    /// `P`, `C` and `Q`: the error of the problem, which the recovery path handles: a
    /// reconstruction error is retried, a full quota suspends, and any other error passes as
    /// it is.
    Pass,
    /// `D`: the retry decision of the load.
    Retry,
}

/// The outcome table. A cell that cannot occur passes the error.
fn cell(column: Column, problem: StartProblem) -> Cell {
    use StartProblem as Problem;
    // An oplog that failed fails the start in every column. Skipping or rejecting the
    // record, or failing the update, would start the agent from another history.
    if problem == Problem::OplogFailed {
        return Cell::Pass;
    }
    let fail = Cell::Fail(Failure::PLAIN);
    let coded = |code| Cell::Fail(Failure::coded(code));
    let target = |fetch: FetchProblem| match fetch {
        FetchProblem::Unavailable => Cell::Pass,
        FetchProblem::NotFound => coded(Code::TargetNotFound),
        FetchProblem::Refused(kind) => coded(Code::TargetRefused(kind)),
        FetchProblem::Other => fail,
    };
    let replay_failed = coded(Code::ReplayFailed);
    match column {
        Column::Periodic => match problem {
            Problem::Restore(_) => Cell::Skip,
            Problem::LoadRetry => Cell::Retry,
            Problem::LoadFailed | Problem::Divergence => Cell::Reject,
            _ => Cell::Pass,
        },
        // A pending manual update loads its record through its own load, which reports a failed
        // load as `ManualLoadFailed`, an exited guest as `ManualLoadExited` and an interrupted
        // load as `ManualLoadInterrupted`, so the other load rows of this column are not reached.
        // An interrupted load passes its interrupt: the update stays pending, and the next start
        // loads the snapshot again.
        Column::ManualPending => match problem {
            Problem::Target(fetch) | Problem::UpdateState(fetch) => target(fetch),
            Problem::TargetUnsupported => coded(Code::TargetUnsupported),
            Problem::BlobStorageRefused(refusal) => coded(Code::BlobStorageRefused(refusal)),
            Problem::ModeChange
            | Problem::RestoreConflict
            | Problem::ManualLoadFailed
            | Problem::ManualLoadExited
            | Problem::LoadUnavailable
            | Problem::LoadFailed
            | Problem::RecoveryRequired
            | Problem::Interrupted => fail,
            Problem::Disabled => coded(Code::RestoreNeedsFilesystemSnapshots),
            Problem::Restore(RestoreClass::Lost) => coded(Code::ManualSnapshotUnavailable),
            Problem::Restore(RestoreClass::Fixed) => coded(Code::SnapshotRestoreFailed),
            Problem::Restore(RestoreClass::DiskFull) => coded(Code::SnapshotRestoreDiskFull),
            _ => Cell::Pass,
        },
        Column::ManualPromoted | Column::AssistedPromoted => match problem {
            Problem::Disabled
            | Problem::Restore(RestoreClass::Lost)
            | Problem::Restore(RestoreClass::Fixed)
            | Problem::Restore(RestoreClass::DiskFull) => Cell::Visible,
            Problem::LoadRetry => Cell::Retry,
            Problem::LoadUnavailable if column == Column::ManualPromoted => Cell::VisibleInvocation,
            Problem::LoadFailed => Cell::VisibleInvocation,
            _ => Cell::Pass,
        },
        Column::InitialFiles => Cell::Pass,
        Column::AutomaticPending(base) => match problem {
            Problem::Target(fetch) | Problem::UpdateState(fetch) => target(fetch),
            Problem::TargetUnsupported => coded(Code::TargetUnsupported),
            Problem::BlobStorageRefused(refusal) => coded(Code::BlobStorageRefused(refusal)),
            Problem::ModeChange => fail,
            Problem::Disabled
            | Problem::Restore(_)
            | Problem::RestoreConflict
            | Problem::Quota
            | Problem::Reconstruction => cell(base.column(), problem),
            Problem::LoadRetry => Cell::Retry,
            Problem::LoadUnavailable
            | Problem::LoadFailed
            | Problem::Interrupted
            | Problem::AgentFailed
            | Problem::Divergence
            | Problem::ReplayError => replay_failed,
            _ => Cell::Pass,
        },
        Column::AssistedPending => match problem {
            Problem::Target(fetch) | Problem::UpdateState(fetch) => target(fetch),
            Problem::TargetUnsupported => coded(Code::TargetUnsupported),
            Problem::BlobStorageRefused(refusal) => coded(Code::BlobStorageRefused(refusal)),
            Problem::StaleSource
            | Problem::ModeChange
            | Problem::RestoreConflict
            | Problem::Instantiation { interrupted: false }
            | Problem::LoadUnavailable
            | Problem::AgentFailed
            | Problem::ReplayError => fail,
            Problem::Disabled => coded(Code::RestoreNeedsFilesystemSnapshots),
            Problem::Restore(RestoreClass::Lost) | Problem::LoadLost => Cell::Fail(Failure {
                code: Some(Code::SnapshotUnavailable),
                fault: Some(SnapshotFault::Unavailable),
                reject_selected: true,
            }),
            Problem::Restore(RestoreClass::Fixed) => coded(Code::SnapshotRestoreFailed),
            Problem::Restore(RestoreClass::DiskFull) => coded(Code::SnapshotRestoreDiskFull),
            Problem::LoadFailed | Problem::Divergence => Cell::Fail(Failure {
                code: Some(Code::SnapshotIncompatible),
                fault: Some(SnapshotFault::Incompatible),
                reject_selected: false,
            }),
            Problem::LoadRetry => Cell::Retry,
            _ => Cell::Pass,
        },
    }
}

/// The index of the periodic record of `role`.
fn periodic_index(role: &BaselineRole) -> Option<OplogIndex> {
    match role {
        BaselineRole::Periodic(index) => Some(*index),
        _ => None,
    }
}

/// The update that a failure of a start from `role` with `head` fails: the head of a pending
/// baseline, else the pending update.
fn update_of<'a>(
    role: &'a BaselineRole,
    head: Option<&'a PendingUpdateRef>,
) -> Option<&'a PendingUpdateRef> {
    match role {
        BaselineRole::ManualPending(head) | BaselineRole::AssistedPending(head) => Some(head),
        _ => head,
    }
}

/// The selected record of a snapshot-assisted `head`.
fn selected_index(head: &PendingUpdateRef) -> Option<OplogIndex> {
    match &head.kind {
        PendingUpdateKind::SnapshotAssistedAutomatic(selection) => Some(selection.snapshot.index),
        PendingUpdateKind::Automatic | PendingUpdateKind::SnapshotBased { .. } => None,
    }
}

/// The failed update of `head`, with its admission as the attempt index and its
/// snapshot-assisted details.
fn failed_update_of(
    head: &PendingUpdateRef,
    details: String,
    snapshot_fault: Option<SnapshotFault>,
) -> OplogEntry {
    OplogEntry::failed_update(
        head.target_revision,
        Some(details),
        failed_details_of(head),
        Some(head.admission_index),
        snapshot_fault,
    )
}

/// The snapshot-assisted details of a failed update of `head`.
fn failed_details_of(head: &PendingUpdateRef) -> Option<FailedSnapshotAssistedUpdateDetails> {
    success_details_of(head).map(|details| FailedSnapshotAssistedUpdateDetails {
        pending_update_index: details.pending_update_index,
        source_component_revision: details.source_component_revision,
        source_revision_start_index: details.source_revision_start_index,
        snapshot_index: details.snapshot_index,
    })
}

/// The details of a failed update: the code and its text, then the cause.
fn details(code: Option<Code>, cause: &str) -> String {
    match code {
        Some(code) => format!("{}: {cause}", code.prefix()),
        None => cause.to_string(),
    }
}

/// The cause of a failed update of `head` in `column`.
fn cause(column: Column, head: &PendingUpdateRef, error: &RawStartError<'_>) -> String {
    match error {
        RawStartError::StaleSource(found) => match &head.kind {
            PendingUpdateKind::SnapshotAssistedAutomatic(selection)
                if found.revision == selection.snapshot.component_revision
                    && found.start_index == selection.source_revision_start_index =>
            {
                format!(
                    "Snapshot-assisted automatic update from revision {} to revision {} is not an \
                     upgrade",
                    selection.snapshot.component_revision, head.target_revision
                )
            }
            PendingUpdateKind::SnapshotAssistedAutomatic(selection) => format!(
                "Snapshot-assisted automatic update source became stale: expected revision {} at \
                 revision start index {}, found revision {} at revision start index {}",
                selection.snapshot.component_revision,
                selection.source_revision_start_index,
                found.revision,
                found.start_index
            ),
            PendingUpdateKind::Automatic | PendingUpdateKind::SnapshotBased { .. } => {
                "The source of the update does not hold".to_string()
            }
        },
        RawStartError::ModeChange(text) => text.to_string(),
        RawStartError::Instantiation(error) => format!(
            "Snapshot-assisted automatic update failed while instantiating the target: {error}"
        ),
        RawStartError::ManualLoad(ManualLoadResult::Failed(text)) => text.clone(),
        RawStartError::ManualLoad(ManualLoadResult::Exited) => {
            format!("Manual update failed to load snapshot: {}", raw_text(error))
        }
        RawStartError::UpdateState(error) => format!("Applying worker update failed: {error}"),
        RawStartError::Load(_) | RawStartError::Replay { .. } => match column {
            Column::AssistedPending => format!(
                "Snapshot-assisted automatic update failed after the snapshot at {}: {}",
                selected_index(head).map_or_else(|| "?".to_string(), |index| index.to_string()),
                raw_text(error)
            ),
            _ => format!("Automatic update failed: {}", raw_text(error)),
        },
        RawStartError::TargetFetch(_)
        | RawStartError::TargetLoad(_)
        | RawStartError::TargetUnsupported(_)
        | RawStartError::Disabled
        | RawStartError::Filesystem(_)
        | RawStartError::Finish(_)
        | RawStartError::ManualLoad(ManualLoadResult::Interrupted(_)) => raw_text(error),
    }
}

/// The text of the raw error.
fn raw_text(error: &RawStartError<'_>) -> String {
    match error {
        RawStartError::StaleSource(found) => format!(
            "the source revision {} at revision start index {} does not hold",
            found.revision, found.start_index
        ),
        RawStartError::TargetFetch(error)
        | RawStartError::TargetLoad(error)
        | RawStartError::Instantiation(error)
        | RawStartError::Replay { error, .. } => error.to_string(),
        RawStartError::TargetUnsupported(reason) => reason.to_string(),
        RawStartError::ModeChange(text) => text.to_string(),
        RawStartError::Disabled => SnapshotsDisabled.to_string(),
        RawStartError::Filesystem(error) | RawStartError::Finish(error) => error.to_string(),
        RawStartError::Load(result) => match result {
            SnapshotRecoveryResult::Failed(error)
            | SnapshotRecoveryResult::Unavailable(error)
            | SnapshotRecoveryResult::Lost(error) => error.to_string(),
            SnapshotRecoveryResult::NotAttempted => {
                "the update did not attempt its required snapshot".to_string()
            }
            SnapshotRecoveryResult::Success | SnapshotRecoveryResult::Retry(_) => {
                "the snapshot load gave no error".to_string()
            }
        },
        RawStartError::ManualLoad(ManualLoadResult::Failed(text)) => text.clone(),
        RawStartError::ManualLoad(ManualLoadResult::Interrupted(_)) => {
            "the snapshot load was interrupted".to_string()
        }
        RawStartError::ManualLoad(ManualLoadResult::Exited) => {
            "the agent exited during the snapshot load".to_string()
        }
        RawStartError::UpdateState(error) => error.to_string(),
    }
}

impl RawStartError<'_> {
    /// The executor error that the raw error carries as it is: the error of a target fetch, of a
    /// target load, of an instantiation or of the replay, and the error of a load of the
    /// application snapshot that did not load.
    fn carried_error(&self) -> Option<&WorkerExecutorError> {
        match self {
            RawStartError::TargetFetch(error)
            | RawStartError::TargetLoad(error)
            | RawStartError::Instantiation(error)
            | RawStartError::Replay { error, .. } => Some(*error),
            RawStartError::Load(
                SnapshotRecoveryResult::Failed(error)
                | SnapshotRecoveryResult::Unavailable(error)
                | SnapshotRecoveryResult::Lost(error),
            ) => Some(error),
            _ => None,
        }
    }

    /// Whether the start failed because the oplog of the agent failed.
    fn oplog_failed(&self) -> bool {
        matches!(
            self.carried_error(),
            Some(WorkerExecutorError::OplogFailed { .. })
        )
    }

    /// The error of the problem as it is, for the recovery path: a filesystem failure as a
    /// reconstruction error or a suspension, a disabled baseline as a runtime error, and every
    /// other raw error unchanged.
    pub(crate) fn passed_through(&self) -> WorkerExecutorError {
        if let Some(error) = self.carried_error() {
            return error.clone();
        }
        match self {
            RawStartError::Filesystem(error) | RawStartError::Finish(error) => {
                reconstruction_startup_error(error)
            }
            RawStartError::UpdateState(error) => error.to_worker_executor_error(),
            RawStartError::ManualLoad(ManualLoadResult::Interrupted(kind)) => {
                WorkerExecutorError::Interrupted { kind: *kind }
            }
            RawStartError::TargetFetch(_)
            | RawStartError::TargetLoad(_)
            | RawStartError::Instantiation(_)
            | RawStartError::Replay { .. }
            | RawStartError::StaleSource(_)
            | RawStartError::TargetUnsupported(_)
            | RawStartError::ModeChange(_)
            | RawStartError::Disabled
            | RawStartError::Load(_)
            | RawStartError::ManualLoad(ManualLoadResult::Failed(_) | ManualLoadResult::Exited) => {
                WorkerExecutorError::runtime(raw_text(self))
            }
        }
    }
}

#[cfg(test)]
mod tests;
