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
use crate::filesystem::errors::OperationContext;
use crate::services::agent_filesystem::{
    AccessError, FilesystemStorageError, InitialFileConflict, RestoreError,
};
use golem_common::model::component::ComponentId;
use golem_common::model::oplog::FilesystemSnapshotName;
use golem_common::model::{AssistedSelection, UsableAutomaticSnapshot};
use std::path::Path;
use test_r::test;

fn revision(value: u64) -> ComponentRevision {
    ComponentRevision::new(value).unwrap()
}

fn agent_id() -> AgentId {
    AgentId {
        component_id: ComponentId::new(),
        agent_id: "start-outcome".to_string(),
    }
}

fn head(kind: PendingUpdateKind, oplog_index: u64, admission_index: u64) -> PendingUpdateRef {
    PendingUpdateRef {
        timestamp: Timestamp::from(1_000),
        oplog_index: OplogIndex::from_u64(oplog_index),
        admission_index: OplogIndex::from_u64(admission_index),
        target_revision: revision(3),
        kind,
    }
}

fn assisted_kind(filesystem_snapshot: Option<FilesystemSnapshotName>) -> PendingUpdateKind {
    PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
        source_revision_start_index: OplogIndex::from_u64(4),
        snapshot: UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(7),
            component_revision: revision(2),
            filesystem_snapshot,
        },
    }))
}

fn assisted_head() -> PendingUpdateRef {
    head(assisted_kind(None), 12, 10)
}

fn manual_head() -> PendingUpdateRef {
    head(
        PendingUpdateKind::SnapshotBased {
            filesystem_snapshot: None,
        },
        12,
        9,
    )
}

fn automatic_head() -> PendingUpdateRef {
    head(PendingUpdateKind::Automatic, 12, 10)
}

#[test]
fn cancelled_compilation_keeps_the_update_pending() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let join_error = runtime.block_on(async {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.await.unwrap();
        // A blocking task can be cancelled only before it starts. Occupy the sole thread.
        let compilation = tokio::task::spawn_blocking(|| ());
        compilation.abort();
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        compilation.await.unwrap_err()
    });
    assert!(join_error.is_cancelled());
    let error = crate::services::component::compilation_join_error(join_error);
    let action = decide_now(
        &BaselineRole::InitialFiles,
        Some(&automatic_head()),
        RawStartError::TargetLoad(&error),
    );
    assert!(
        matches!(
            action,
            StartAction::Error(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Restart
            })
        ),
        "cancelled compilation must interrupt startup without failing the update: {action:?}"
    );
}

#[test]
async fn panicked_compilation_keeps_the_update_pending() {
    let join_error = tokio::task::spawn_blocking(|| panic!("compile task panic"))
        .await
        .unwrap_err();
    assert!(join_error.is_panic());
    let error = crate::services::component::compilation_join_error(join_error);
    let action = decide_now(
        &BaselineRole::InitialFiles,
        Some(&automatic_head()),
        RawStartError::TargetLoad(&error),
    );
    assert!(
        matches!(
            action,
            StartAction::Error(WorkerExecutorError::Runtime { .. })
        ),
        "a compiler panic must retry startup without failing the update: {action:?}"
    );
}

#[test]
fn target_load_executor_errors_preserve_every_pending_update() {
    let errors = [
        WorkerExecutorError::Interrupted {
            kind: InterruptKind::Restart,
        },
        WorkerExecutorError::Interrupted {
            kind: InterruptKind::Interrupt(Timestamp::from(1_000)),
        },
        WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(Timestamp::from(2_000)),
        },
        WorkerExecutorError::Interrupted {
            kind: InterruptKind::ShardLost,
        },
        WorkerExecutorError::RecoveryRequired {
            retry_from: Some(OplogIndex::from_u64(5)),
            details: "retry target load".to_string(),
        },
        WorkerExecutorError::runtime("compile task panic"),
    ];
    let automatic = automatic_head();
    let manual = manual_head();
    let assisted = assisted_head();
    let cases = [
        (BaselineRole::InitialFiles, automatic.clone()),
        (BaselineRole::ManualPromoted, automatic.clone()),
        (BaselineRole::AssistedPromoted, automatic),
        (
            BaselineRole::ManualPending(Arc::new(manual.clone())),
            manual,
        ),
        (
            BaselineRole::AssistedPending(Arc::new(assisted.clone())),
            assisted,
        ),
    ];
    for (role, head) in cases {
        for error in &errors {
            let action = decide_now(&role, Some(&head), RawStartError::TargetLoad(error));
            assert!(
                matches!(&action, StartAction::Error(passed) if passed == error),
                "executor failures must pass through for {role:?}: {action:?}"
            );
        }
    }
}

#[test]
fn metadata_runtime_failures_and_component_parse_failures_still_fail_updates() {
    let runtime = WorkerExecutorError::runtime("metadata could not be parsed");
    let parse = WorkerExecutorError::ComponentParseFailed {
        component_id: component_id(),
        component_revision: revision(3),
        reason: "invalid component".to_string(),
    };
    for problem in [
        RawStartError::TargetFetch(&runtime),
        RawStartError::TargetLoad(&parse),
    ] {
        assert!(matches!(
            decide_now(
                &BaselineRole::InitialFiles,
                Some(&automatic_head()),
                problem
            ),
            StartAction::FailUpdate { .. }
        ));
    }
}

#[test]
fn shared_memory_target_is_refused_without_a_snapshot_fault() {
    let bytes = wat::parse_str(
        "(component (core module $m (memory 1 1 shared)) (core instance (instantiate $m)))",
    )
    .unwrap();
    let metadata = golem_common::model::component_metadata::ComponentMetadata::analyse_component(
        &bytes,
        Vec::new(),
        Default::default(),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    assert!(metadata.has_shared_linear_memory());
    let reason = crate::services::component::component_support_error(&metadata)
        .expect("shared memory must fail metadata support validation");

    let engine = wasmtime::Engine::new(
        &golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache(),
    )
    .unwrap();
    let compilation_error = wasmtime::component::Component::from_binary(&engine, &bytes)
        .err()
        .expect("the production engine must reject shared memory");
    assert!(!compilation_error.to_string().is_empty());
    let automatic = automatic_head();
    let manual = manual_head();
    let assisted = assisted_head();
    let cases = [
        (BaselineRole::InitialFiles, automatic.clone()),
        (BaselineRole::ManualPromoted, automatic.clone()),
        (BaselineRole::AssistedPromoted, automatic),
        (
            BaselineRole::ManualPending(Arc::new(manual.clone())),
            manual,
        ),
        (
            BaselineRole::AssistedPending(Arc::new(assisted.clone())),
            assisted,
        ),
    ];
    for (role, head) in cases {
        let (entry, reject) = failed_entry(decide_now(
            &role,
            Some(&head),
            RawStartError::TargetUnsupported(reason),
        ));
        let (target, details, assisted, attempt, fault) = entry_fields(&entry);
        assert_eq!(target, revision(3));
        assert_eq!(attempt, Some(head.admission_index));
        assert_eq!(
            assisted,
            matches!(head.kind, PendingUpdateKind::SnapshotAssistedAutomatic(_))
                .then_some(assisted_details())
        );
        assert_eq!(fault, None);
        assert_eq!(reject, None);
        assert_eq!(
            details,
            "UPDATE_TARGET_REFUSED: The target revision uses WebAssembly threads, which Golem does not support"
        );
        assert!(matches!(
            decide(
                &role,
                Some(&head),
                RawStartError::TargetUnsupported(reason),
                &agent_id(),
                true
            ),
            StartAction::ShardLost
        ));
    }
}

#[test]
fn unsupported_target_preserves_a_non_thread_refusal_reason() {
    let reason = "The target revision requires a disabled Wasm feature";
    for (role, head) in [
        (BaselineRole::InitialFiles, automatic_head()),
        (
            BaselineRole::ManualPending(Arc::new(manual_head())),
            manual_head(),
        ),
        (
            BaselineRole::AssistedPending(Arc::new(assisted_head())),
            assisted_head(),
        ),
    ] {
        let (entry, reject) = failed_entry(decide_now(
            &role,
            Some(&head),
            RawStartError::TargetUnsupported(reason),
        ));
        let (_, details, _, attempt, fault) = entry_fields(&entry);
        assert_eq!(
            details,
            "UPDATE_TARGET_REFUSED: The target revision requires a disabled Wasm feature"
        );
        assert_eq!(attempt, Some(head.admission_index));
        assert_eq!(fault, None);
        assert_eq!(reject, None);
    }
}

#[test]
fn ordinary_instantiation_errors_are_not_target_refusals() {
    let error = WorkerExecutorError::worker_creation_failed(
        agent_id(),
        crate::services::linear_memory::SHARED_LINEAR_MEMORY_ERROR,
    );
    assert!(super::super::is_infrastructure_recovery_error(&error));
    let action = decide_now(
        &BaselineRole::InitialFiles,
        Some(&automatic_head()),
        RawStartError::Instantiation(&error),
    );
    assert!(
        matches!(
            action,
            StartAction::Error(WorkerExecutorError::AgentCreationFailed { .. })
        ),
        "only the typed unsupported-target outcome refuses an update: {action:?}"
    );
}

fn storage() -> FilesystemStorageError {
    FilesystemStorageError::from(OperationContext::verification("seed initial file", None))
}

fn restore(class: RestoreClass) -> FilesystemError {
    FilesystemError::Baseline(Box::new(RestoreError {
        class,
        source: anyhow::anyhow!("restore failed"),
    }))
}

fn refused_source() -> FilesystemError {
    FilesystemError::InitialFileRefused {
        refusal: BlobRefusal::AccessDenied,
        source: storage(),
    }
}

fn conflict() -> InitialFileConflict {
    InitialFileConflict::occupied(Path::new("e/f"))
}

fn component_id() -> ComponentId {
    ComponentId(uuid::Uuid::nil())
}

fn unavailable_service() -> WorkerExecutorError {
    WorkerExecutorError::ComponentServiceUnavailable {
        component_id: component_id(),
        component_revision: Some(revision(3)),
        reason: "transport".to_string(),
    }
}

fn refused_service() -> WorkerExecutorError {
    WorkerExecutorError::ComponentServiceRefused {
        component_id: component_id(),
        component_revision: Some(revision(3)),
        kind: ComponentServiceRefusal::Unauthorized,
        reason: "token".to_string(),
    }
}

fn not_found() -> WorkerExecutorError {
    WorkerExecutorError::ComponentNotFound {
        component_id: component_id(),
    }
}

/// The code of a cell, as the table of the design writes it. `P` stands for `C`, `Q` and `P`:
/// the error of the problem decides which (see the filesystem tests below).
fn code(cell: Cell) -> &'static str {
    match cell {
        Cell::Skip => "S",
        Cell::Reject => "R",
        Cell::Fail(Failure {
            code,
            fault,
            reject_selected,
        }) => match (code, fault, reject_selected) {
            (None, None, false) => "F",
            (Some(Code::SnapshotUnavailable), Some(SnapshotFault::Unavailable), true) => "Fu",
            (Some(Code::ManualSnapshotUnavailable), None, false) => "Fm",
            (Some(Code::SnapshotIncompatible), Some(SnapshotFault::Incompatible), false) => "Fi",
            (Some(Code::ReplayFailed), None, false) => "Fr",
            (Some(Code::RestoreNeedsFilesystemSnapshots), None, false) => "Fd",
            (Some(Code::SnapshotRestoreFailed | Code::SnapshotRestoreDiskFull), None, false) => {
                "Fx"
            }
            (Some(Code::TargetNotFound), None, false) => "Fn",
            (Some(Code::TargetRefused(_) | Code::TargetUnsupported), None, false) => "Fz",
            (Some(Code::BlobStorageRefused(_)), None, false) => "Fb",
            _ => "?",
        },
        Cell::Visible => "V",
        Cell::VisibleInvocation => "V2",
        Cell::Pass => "P",
        Cell::Retry => "D",
    }
}

/// The outcome table, cell by cell. Columns: Periodic, ManualPending, ManualPromoted,
/// InitialFiles, AutomaticPending (on a promoted manual baseline), AssistedPending,
/// AssistedPromoted.
///
/// Rows for causes that only some sites report: `Target(Other)`, `ManualLoadFailed` and
/// `ManualLoadExited` (all fail the update), `ManualLoadInterrupted` (which passes its
/// interrupt), `Restore(DiskFull)` (the cells of `Restore(Fixed)` with their own text),
/// `AfterReplay` (a filesystem failure after the replay, which passes in every column),
/// `BlobStorageRefused` (a refused initial-file source at the update point), and
/// `LoadLost` (a lost payload of the selected record, which only a snapshot-assisted attempt
/// reports).
#[test]
fn the_outcome_table() {
    use StartProblem as Problem;
    let columns = [
        Column::Periodic,
        Column::ManualPending,
        Column::ManualPromoted,
        Column::InitialFiles,
        Column::AutomaticPending(Base::ManualPromoted),
        Column::AssistedPending,
        Column::AssistedPromoted,
    ];
    let refused = FetchProblem::Refused(ComponentServiceRefusal::Unauthorized);
    let table: [(Problem, [&str; 7]); 36] = [
        (Problem::StaleSource, ["P", "P", "P", "P", "P", "F", "P"]),
        (Problem::TargetExecutor, ["P", "P", "P", "P", "P", "P", "P"]),
        (
            Problem::TargetUnsupported,
            ["P", "Fz", "P", "P", "Fz", "Fz", "P"],
        ),
        (
            Problem::Target(FetchProblem::Unavailable),
            ["P", "P", "P", "P", "P", "P", "P"],
        ),
        (
            Problem::Target(FetchProblem::NotFound),
            ["P", "Fn", "P", "P", "Fn", "Fn", "P"],
        ),
        (
            Problem::Target(refused),
            ["P", "Fz", "P", "P", "Fz", "Fz", "P"],
        ),
        (
            Problem::Target(FetchProblem::Other),
            ["P", "F", "P", "P", "F", "F", "P"],
        ),
        (Problem::ModeChange, ["P", "F", "P", "P", "F", "F", "P"]),
        (Problem::Disabled, ["P", "Fd", "V", "P", "V", "Fd", "V"]),
        (
            Problem::Restore(RestoreClass::Lost),
            ["S", "Fm", "V", "P", "V", "Fu", "V"],
        ),
        (
            Problem::Restore(RestoreClass::Fixed),
            ["S", "Fx", "V", "P", "V", "Fx", "V"],
        ),
        (
            Problem::Restore(RestoreClass::DiskFull),
            ["S", "Fx", "V", "P", "V", "Fx", "V"],
        ),
        (
            Problem::Restore(RestoreClass::Transient),
            ["S", "P", "P", "P", "P", "P", "P"],
        ),
        (
            Problem::RestoreConflict,
            ["P", "F", "P", "P", "P", "F", "P"],
        ),
        (Problem::Quota, ["P", "P", "P", "P", "P", "P", "P"]),
        (Problem::Reconstruction, ["P", "P", "P", "P", "P", "P", "P"]),
        (
            Problem::Instantiation { interrupted: true },
            ["P", "P", "P", "P", "P", "P", "P"],
        ),
        (
            Problem::Instantiation { interrupted: false },
            ["P", "P", "P", "P", "P", "F", "P"],
        ),
        (Problem::LoadRetry, ["D", "P", "D", "P", "D", "D", "D"]),
        (
            Problem::LoadUnavailable,
            ["P", "F", "V2", "P", "Fr", "F", "P"],
        ),
        (Problem::LoadLost, ["P", "P", "P", "P", "P", "Fu", "P"]),
        (Problem::LoadFailed, ["R", "F", "V2", "P", "Fr", "Fi", "V2"]),
        (
            Problem::ManualLoadFailed,
            ["P", "F", "P", "P", "P", "P", "P"],
        ),
        (
            Problem::ManualLoadInterrupted,
            ["P", "P", "P", "P", "P", "P", "P"],
        ),
        (
            Problem::ManualLoadExited,
            ["P", "F", "P", "P", "P", "P", "P"],
        ),
        (
            Problem::RecoveryRequired,
            ["P", "F", "P", "P", "P", "P", "P"],
        ),
        (Problem::Interrupted, ["P", "F", "P", "P", "Fr", "P", "P"]),
        (Problem::AgentFailed, ["P", "P", "P", "P", "Fr", "F", "P"]),
        (Problem::Divergence, ["R", "P", "P", "P", "Fr", "Fi", "P"]),
        (Problem::ReplayError, ["P", "P", "P", "P", "Fr", "F", "P"]),
        (
            Problem::UpdateState(FetchProblem::Unavailable),
            ["P", "P", "P", "P", "P", "P", "P"],
        ),
        (
            Problem::UpdateState(FetchProblem::NotFound),
            ["P", "Fn", "P", "P", "Fn", "Fn", "P"],
        ),
        (
            Problem::UpdateState(refused),
            ["P", "Fz", "P", "P", "Fz", "Fz", "P"],
        ),
        (
            Problem::UpdateState(FetchProblem::Other),
            ["P", "F", "P", "P", "F", "F", "P"],
        ),
        (Problem::AfterReplay, ["P", "P", "P", "P", "P", "P", "P"]),
        (
            Problem::BlobStorageRefused(BlobRefusal::AccessDenied),
            ["P", "Fb", "P", "P", "Fb", "Fb", "P"],
        ),
    ];
    table.iter().for_each(|(problem, expected)| {
        let actual = columns.map(|column| code(cell(column, *problem)));
        assert_eq!(&actual, expected, "{problem:?}");
    });
    // A pending plain automatic update on the other baselines takes their cells for the
    // problems of the baseline.
    [
        Problem::Disabled,
        Problem::Restore(RestoreClass::Lost),
        Problem::Restore(RestoreClass::Fixed),
        Problem::Restore(RestoreClass::DiskFull),
        Problem::Restore(RestoreClass::Transient),
        Problem::RestoreConflict,
        Problem::Quota,
        Problem::Reconstruction,
    ]
    .iter()
    .for_each(|problem| {
        [
            Base::ManualPromoted,
            Base::AssistedPromoted,
            Base::InitialFiles,
        ]
        .iter()
        .for_each(|base| {
            assert_eq!(
                cell(Column::AutomaticPending(*base), *problem),
                cell(base.column(), *problem),
                "{base:?} {problem:?}"
            );
        });
    });
}

#[test]
fn the_column_follows_the_baseline_and_a_pending_plain_automatic_update() {
    let assisted = Arc::new(assisted_head());
    let manual = Arc::new(manual_head());
    let automatic = automatic_head();
    assert_eq!(
        [
            column(&BaselineRole::Periodic(OplogIndex::from_u64(10)), None),
            column(&BaselineRole::ManualPending(manual.clone()), Some(&manual)),
            column(&BaselineRole::ManualPromoted, None),
            column(&BaselineRole::InitialFiles, None),
            column(&BaselineRole::ManualPromoted, Some(&automatic)),
            column(&BaselineRole::AssistedPromoted, Some(&automatic)),
            column(&BaselineRole::InitialFiles, Some(&automatic)),
            column(
                &BaselineRole::AssistedPending(assisted.clone()),
                Some(&assisted)
            ),
            column(&BaselineRole::AssistedPromoted, None),
        ],
        [
            Column::Periodic,
            Column::ManualPending,
            Column::ManualPromoted,
            Column::InitialFiles,
            Column::AutomaticPending(Base::ManualPromoted),
            Column::AutomaticPending(Base::AssistedPromoted),
            Column::AutomaticPending(Base::InitialFiles),
            Column::AssistedPending,
            Column::AssistedPromoted,
        ]
        .map(Some)
    );
}

/// A baseline that a start never has with its head gives no column, and the start ends with
/// an error that names both instead of taking the cells of another column.
#[test]
fn a_baseline_that_cannot_go_with_its_head_has_no_column() {
    let assisted = assisted_head();
    let manual = manual_head();
    let automatic = automatic_head();
    let periodic = BaselineRole::Periodic(OplogIndex::from_u64(10));
    let lost = restore(RestoreClass::Lost);

    assert_eq!(
        [
            column(&periodic, Some(&automatic)),
            column(&periodic, Some(&manual)),
            column(&BaselineRole::ManualPromoted, Some(&assisted)),
            column(&BaselineRole::InitialFiles, Some(&manual)),
            column(&BaselineRole::AssistedPromoted, Some(&assisted)),
        ],
        [None, None, None, None, None]
    );
    assert!(matches!(
        decide_now(&periodic, Some(&automatic), RawStartError::Filesystem(&lost)),
        StartAction::Error(WorkerExecutorError::Runtime { details })
            if details.contains("cannot have the pending update")
    ));
}

/// The classifier, for every raw error kind.
#[test]
fn each_raw_error_has_its_problem() {
    use StartProblem as Problem;
    let found = SourceFound {
        revision: revision(2),
        start_index: OplogIndex::from_u64(4),
    };
    let runtime = WorkerExecutorError::runtime("boom");
    let recovery = WorkerExecutorError::RecoveryRequired {
        retry_from: None,
        details: "again".to_string(),
    };
    let interrupted = WorkerExecutorError::Interrupted {
        kind: InterruptKind::Restart,
    };
    let unexpected = WorkerExecutorError::UnexpectedOplogEntry {
        expected: "a".to_string(),
        got: "b".to_string(),
    };
    let previous_failed = WorkerExecutorError::PreviousInvocationFailed {
        error: AgentError::Unknown("trap".to_string()),
        stderr: String::new(),
    };
    let parse = WorkerExecutorError::ComponentParseFailed {
        component_id: component_id(),
        component_revision: revision(3),
        reason: "bad".to_string(),
    };
    let filesystem = [
        (
            FilesystemError::Access(AccessError::Revoked),
            Problem::Reconstruction,
        ),
        (FilesystemError::Sandbox(storage()), Problem::Reconstruction),
        (
            FilesystemError::InitialFileUnavailable(storage()),
            Problem::Reconstruction,
        ),
        (refused_source(), Problem::Reconstruction),
        (
            FilesystemError::InitialFileConflict(Box::new(conflict())),
            Problem::RestoreConflict,
        ),
        (FilesystemError::AgentQuota(storage()), Problem::Quota),
        (
            FilesystemError::PhysicalCapacity(storage()),
            Problem::Reconstruction,
        ),
        (
            restore(RestoreClass::Lost),
            Problem::Restore(RestoreClass::Lost),
        ),
        (
            restore(RestoreClass::Fixed),
            Problem::Restore(RestoreClass::Fixed),
        ),
        (
            restore(RestoreClass::Transient),
            Problem::Restore(RestoreClass::Transient),
        ),
        (FilesystemError::RuntimeInvalidated, Problem::Reconstruction),
    ];
    filesystem.iter().for_each(|(error, problem)| {
        assert_eq!(
            StartProblem::of(&RawStartError::Filesystem(error)),
            *problem,
            "{error}"
        );
    });
    let loads = [
        (
            SnapshotRecoveryResult::Retry(RetryDecision::Immediate),
            Problem::LoadRetry,
        ),
        (
            SnapshotRecoveryResult::Failed(runtime.clone()),
            Problem::LoadFailed,
        ),
        (
            SnapshotRecoveryResult::Unavailable(runtime.clone()),
            Problem::LoadUnavailable,
        ),
        (
            SnapshotRecoveryResult::Unavailable(recovery.clone()),
            Problem::RecoveryRequired,
        ),
        (
            SnapshotRecoveryResult::Unavailable(interrupted.clone()),
            Problem::Interrupted,
        ),
        (SnapshotRecoveryResult::NotAttempted, Problem::ReplayError),
    ];
    loads.iter().for_each(|(result, problem)| {
        assert_eq!(StartProblem::of(&RawStartError::Load(result)), *problem);
    });
    let replays = [
        (&runtime, false, Problem::ReplayError),
        (&runtime, true, Problem::Divergence),
        (&unexpected, false, Problem::Divergence),
        (&recovery, false, Problem::RecoveryRequired),
        (&interrupted, false, Problem::Interrupted),
        (&previous_failed, false, Problem::AgentFailed),
        (
            &WorkerExecutorError::PreviousInvocationExited,
            false,
            Problem::AgentFailed,
        ),
    ];
    replays.iter().for_each(|(error, diverged, problem)| {
        assert_eq!(
            StartProblem::of(&RawStartError::Replay {
                error,
                diverged: *diverged
            }),
            *problem,
            "{error} diverged: {diverged}"
        );
    });
    let fetches = [
        (unavailable_service(), FetchProblem::Unavailable),
        (not_found(), FetchProblem::NotFound),
        (
            refused_service(),
            FetchProblem::Refused(ComponentServiceRefusal::Unauthorized),
        ),
        (parse.clone(), FetchProblem::Other),
        (runtime.clone(), FetchProblem::Other),
    ];
    fetches.iter().for_each(|(error, fetch)| {
        assert_eq!(
            StartProblem::of(&RawStartError::TargetFetch(error)),
            Problem::Target(*fetch)
        );
        assert_eq!(
            StartProblem::of(&RawStartError::UpdateState(&UpdateStateError::Metadata(
                error.clone()
            ))),
            Problem::UpdateState(*fetch)
        );
    });
    let other_update_errors = [
        UpdateStateError::MissingAgentType("agent".to_string()),
        UpdateStateError::Config(runtime.clone()),
        UpdateStateError::WalletCards(runtime.clone()),
        UpdateStateError::InitialFiles(FilesystemError::InitialFileConflict(Box::new(conflict()))),
    ];
    other_update_errors.iter().for_each(|error| {
        assert_eq!(
            StartProblem::of(&RawStartError::UpdateState(error)),
            Problem::UpdateState(FetchProblem::Other)
        );
    });
    assert_eq!(
        [
            StartProblem::of(&RawStartError::StaleSource(&found)),
            StartProblem::of(&RawStartError::ModeChange("mode")),
            StartProblem::of(&RawStartError::Disabled),
            StartProblem::of(&RawStartError::Instantiation(&runtime)),
            StartProblem::of(&RawStartError::Instantiation(&interrupted)),
            StartProblem::of(&RawStartError::ManualLoad(&ManualLoadResult::Failed(
                "load".to_string()
            ))),
            StartProblem::of(&RawStartError::ManualLoad(&ManualLoadResult::Interrupted(
                InterruptKind::Restart
            ))),
            StartProblem::of(&RawStartError::ManualLoad(&ManualLoadResult::Exited)),
            StartProblem::of(&RawStartError::Finish(&FilesystemError::AgentQuota(
                storage()
            ))),
        ],
        [
            Problem::StaleSource,
            Problem::ModeChange,
            Problem::Disabled,
            Problem::Instantiation { interrupted: false },
            Problem::Instantiation { interrupted: true },
            Problem::ManualLoadFailed,
            Problem::ManualLoadInterrupted,
            Problem::ManualLoadExited,
            Problem::AfterReplay,
        ]
    );
}

/// An oplog that does not read ends the start with its error from every step of the start
/// that reads it, for every baseline and head: no record is skipped or rejected, and no
/// update fails, because each of them would start the agent from another history.
#[test]
fn an_unreadable_oplog_fails_the_start_in_every_column() {
    let unreadable = WorkerExecutorError::oplog_failed(
        agent_id(),
        "corrupt oplog data from archive layer 1: failed to decode compressed oplog chunk",
    );
    let failed = SnapshotRecoveryResult::Failed(unreadable.clone());
    let unavailable = SnapshotRecoveryResult::Unavailable(unreadable.clone());
    let lost = SnapshotRecoveryResult::Lost(unreadable.clone());
    let errors = [
        RawStartError::Replay {
            error: &unreadable,
            diverged: false,
        },
        RawStartError::Replay {
            error: &unreadable,
            diverged: true,
        },
        RawStartError::Load(&failed),
        RawStartError::Load(&unavailable),
        RawStartError::Load(&lost),
        RawStartError::Instantiation(&unreadable),
        RawStartError::TargetLoad(&unreadable),
        RawStartError::TargetFetch(&unreadable),
    ];
    let assisted = Arc::new(assisted_head());
    let manual = Arc::new(manual_head());
    let automatic = automatic_head();
    let starts = [
        (BaselineRole::Periodic(OplogIndex::from_u64(10)), None),
        (BaselineRole::ManualPending(manual.clone()), Some(&*manual)),
        (BaselineRole::ManualPromoted, None),
        (BaselineRole::InitialFiles, None),
        (BaselineRole::ManualPromoted, Some(&automatic)),
        (BaselineRole::AssistedPromoted, Some(&automatic)),
        (BaselineRole::InitialFiles, Some(&automatic)),
        (
            BaselineRole::AssistedPending(assisted.clone()),
            Some(&*assisted),
        ),
        (BaselineRole::AssistedPromoted, None),
    ];
    for (role, head) in &starts {
        for (index, error) in errors.iter().enumerate() {
            let action = decide_now(role, *head, *error);
            assert!(
                matches!(&action, StartAction::Error(error) if *error == unreadable),
                "{role:?} with error {index}: {action:?}"
            );
        }
    }
}

fn decide_now(
    role: &BaselineRole,
    head: Option<&PendingUpdateRef>,
    error: RawStartError<'_>,
) -> StartAction {
    decide(role, head, error, &agent_id(), false)
}

fn failed_entry(action: StartAction) -> (OplogEntry, Option<OplogIndex>) {
    match action {
        StartAction::FailUpdate { entry, reject } => (entry, reject),
        other => panic!("expected a failed update, got {other:?}"),
    }
}

fn entry_fields(
    entry: &OplogEntry,
) -> (
    ComponentRevision,
    String,
    Option<FailedSnapshotAssistedUpdateDetails>,
    Option<OplogIndex>,
    Option<SnapshotFault>,
) {
    match entry {
        OplogEntry::FailedUpdate {
            target_revision,
            details,
            snapshot_assisted_details,
            update_attempt_index,
            snapshot_fault,
            ..
        } => (
            *target_revision,
            details.clone().unwrap_or_default(),
            snapshot_assisted_details.clone(),
            *update_attempt_index,
            *snapshot_fault,
        ),
        other => panic!("expected a FailedUpdate entry, got {other:?}"),
    }
}

fn assisted_details() -> FailedSnapshotAssistedUpdateDetails {
    FailedSnapshotAssistedUpdateDetails {
        pending_update_index: OplogIndex::from_u64(12),
        source_component_revision: revision(2),
        source_revision_start_index: OplogIndex::from_u64(4),
        snapshot_index: OplogIndex::from_u64(7),
    }
}

/// Every failed update of a snapshot-assisted update carries the assisted details of its
/// queue element and its admission as the attempt index; a lost record is rejected, and only
/// the causes about the record have a fault. Each coded failure starts with its code.
#[test]
fn every_assisted_failure_carries_the_details_of_its_head() {
    let head = assisted_head();
    let role = BaselineRole::AssistedPending(Arc::new(head.clone()));
    let found = SourceFound {
        revision: revision(3),
        start_index: OplogIndex::from_u64(11),
    };
    let runtime = WorkerExecutorError::runtime("boom");
    let lost = restore(RestoreClass::Lost);
    let fixed = restore(RestoreClass::Fixed);
    let conflicting = FilesystemError::InitialFileConflict(Box::new(conflict()));
    let failed_load = SnapshotRecoveryResult::Failed(runtime.clone());
    let unavailable_load = SnapshotRecoveryResult::Unavailable(runtime.clone());
    let metadata = UpdateStateError::Metadata(not_found());
    let refused_target = refused_service();
    let other_update =
        UpdateStateError::InitialFiles(FilesystemError::InitialFileConflict(Box::new(conflict())));
    // (raw error, code at the start of the details, fault, rejected record)
    type Case<'a> = (
        RawStartError<'a>,
        Option<&'a str>,
        Option<SnapshotFault>,
        Option<u64>,
    );
    let cases: Vec<Case<'_>> = vec![
        (RawStartError::StaleSource(&found), None, None, None),
        (
            RawStartError::TargetFetch(&refused_target),
            Some(UPDATE_TARGET_REFUSED),
            None,
            None,
        ),
        (
            RawStartError::ModeChange("mode Ephemeral"),
            None,
            None,
            None,
        ),
        (
            RawStartError::Disabled,
            Some(UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS),
            None,
            None,
        ),
        (
            RawStartError::Filesystem(&lost),
            Some(UPDATE_SNAPSHOT_UNAVAILABLE),
            Some(SnapshotFault::Unavailable),
            Some(7),
        ),
        (
            RawStartError::Filesystem(&fixed),
            Some(UPDATE_SNAPSHOT_RESTORE_FAILED),
            None,
            None,
        ),
        (RawStartError::Filesystem(&conflicting), None, None, None),
        (RawStartError::Instantiation(&runtime), None, None, None),
        (RawStartError::Load(&unavailable_load), None, None, None),
        (
            RawStartError::Load(&failed_load),
            Some(UPDATE_SNAPSHOT_INCOMPATIBLE),
            Some(SnapshotFault::Incompatible),
            None,
        ),
        (
            RawStartError::Replay {
                error: &runtime,
                diverged: true,
            },
            Some(UPDATE_SNAPSHOT_INCOMPATIBLE),
            Some(SnapshotFault::Incompatible),
            None,
        ),
        (
            RawStartError::Replay {
                error: &runtime,
                diverged: false,
            },
            None,
            None,
            None,
        ),
        (
            RawStartError::UpdateState(&metadata),
            Some(UPDATE_TARGET_NOT_FOUND),
            None,
            None,
        ),
        (RawStartError::UpdateState(&other_update), None, None, None),
    ];
    cases
        .into_iter()
        .for_each(|(error, code, fault, rejected)| {
            let label = raw_text(&error);
            let (entry, reject) = failed_entry(decide_now(&role, Some(&head), error));
            let (target, details, assisted, attempt, entry_fault) = entry_fields(&entry);
            assert_eq!(target, revision(3), "{label}");
            assert_eq!(assisted, Some(assisted_details()), "{label}");
            assert_eq!(attempt, Some(OplogIndex::from_u64(10)), "{label}");
            assert_eq!(entry_fault, fault, "{label}");
            assert_eq!(reject, rejected.map(OplogIndex::from_u64), "{label}");
            match code {
                Some(code) => assert!(details.starts_with(&format!("{code}: ")), "{details}"),
                None => assert!(!details.starts_with("UPDATE_"), "{details}"),
            }
        });
}

/// A pending update whose failure the recovery path retries writes nothing, and a lost
/// shard writes nothing for any failed update.
#[test]
fn a_retried_failure_and_a_lost_shard_write_no_failed_update() {
    let head = assisted_head();
    let role = BaselineRole::AssistedPending(Arc::new(head.clone()));
    let transient = restore(RestoreClass::Transient);
    let quota = FilesystemError::AgentQuota(storage());
    let recovery = WorkerExecutorError::RecoveryRequired {
        retry_from: None,
        details: "again".to_string(),
    };
    let interrupted = WorkerExecutorError::Interrupted {
        kind: InterruptKind::Restart,
    };
    let unavailable = unavailable_service();
    let lost = restore(RestoreClass::Lost);

    assert!(matches!(
        decide_now(&role, Some(&head), RawStartError::Filesystem(&transient)),
        StartAction::Error(WorkerExecutorError::Runtime { .. })
    ));
    assert!(matches!(
        decide_now(&role, Some(&head), RawStartError::Filesystem(&quota)),
        StartAction::Error(WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(_)
        })
    ));
    assert!(matches!(
        decide_now(
            &role,
            Some(&head),
            RawStartError::Replay {
                error: &recovery,
                diverged: false
            }
        ),
        StartAction::Error(WorkerExecutorError::RecoveryRequired { .. })
    ));
    assert!(matches!(
        decide_now(
            &role,
            Some(&head),
            RawStartError::Instantiation(&interrupted)
        ),
        StartAction::Error(WorkerExecutorError::Interrupted { .. })
    ));
    assert!(matches!(
        decide_now(&role, Some(&head), RawStartError::TargetFetch(&unavailable)),
        StartAction::Error(WorkerExecutorError::ComponentServiceUnavailable { .. })
    ));
    assert!(matches!(
        decide(
            &role,
            Some(&head),
            RawStartError::Filesystem(&lost),
            &agent_id(),
            true
        ),
        StartAction::ShardLost
    ));
}

/// A transient fetch of the target retries the start of every pending update and writes no
/// failed update. A target that does not exist or that the registry refuses fails the update.
#[test]
fn a_transient_target_fetch_retries_every_pending_update() {
    let unavailable = unavailable_service();
    let metadata = UpdateStateError::Metadata(unavailable.clone());
    let assisted = assisted_head();
    let manual = manual_head();
    let automatic = automatic_head();
    let cases = [
        (
            BaselineRole::AssistedPending(Arc::new(assisted.clone())),
            assisted,
        ),
        (
            BaselineRole::ManualPending(Arc::new(manual.clone())),
            manual,
        ),
        (BaselineRole::ManualPromoted, automatic),
    ];
    cases.iter().for_each(|(role, head)| {
        assert!(matches!(
            decide_now(role, Some(head), RawStartError::TargetFetch(&unavailable)),
            StartAction::Error(WorkerExecutorError::ComponentServiceUnavailable { .. })
        ));
        assert!(matches!(
            decide_now(role, Some(head), RawStartError::UpdateState(&metadata)),
            StartAction::Error(WorkerExecutorError::ComponentServiceUnavailable { .. })
        ));
    });
    let missing = not_found();
    let (entry, _) = failed_entry(decide_now(
        &BaselineRole::ManualPromoted,
        Some(&automatic_head()),
        RawStartError::TargetFetch(&missing),
    ));
    let (_, details, assisted_details, attempt, _) = entry_fields(&entry);
    assert!(details.starts_with(&format!("{UPDATE_TARGET_NOT_FOUND}: ")));
    assert!(details.ends_with(&missing.to_string()));
    assert_eq!(
        (assisted_details, attempt),
        (None, Some(OplogIndex::from_u64(10)))
    );
}

/// The decision for every baseline, every error of the agent filesystem and both shard
/// states. When the restore of a pending manual update fails for good, the update fails and
/// the agent starts again on its source revision.
#[test]
fn a_pending_manual_update_whose_restore_fails_for_good_fails_and_other_baselines_keep_their_answer()
 {
    let manual = manual_head();
    let roles = |label: &str| match label {
        "initial" => BaselineRole::InitialFiles,
        "periodic" => BaselineRole::Periodic(OplogIndex::from_u64(10)),
        "manual-pending" => BaselineRole::ManualPending(Arc::new(manual.clone())),
        "manual-promoted" => BaselineRole::ManualPromoted,
        other => panic!("unknown role {other}"),
    };
    let errors = |label: &str| match label {
        "access" => FilesystemError::Access(AccessError::Revoked),
        "sandbox" => FilesystemError::Sandbox(storage()),
        "conflict" => FilesystemError::InitialFileConflict(Box::new(conflict())),
        "quota" => FilesystemError::AgentQuota(storage()),
        "capacity" => FilesystemError::PhysicalCapacity(storage()),
        "restore-transient" => restore(RestoreClass::Transient),
        "restore-fixed" => restore(RestoreClass::Fixed),
        "restore-lost" => restore(RestoreClass::Lost),
        "restore-disk-full" => restore(RestoreClass::DiskFull),
        "invalidated" => FilesystemError::RuntimeInvalidated,
        other => panic!("unknown error {other}"),
    };
    let outcome = |action: StartAction| match action {
        StartAction::SkipPeriodic(index) => format!("skip {index}"),
        StartAction::FailUpdate { entry, .. } => {
            let (_, details, _, attempt, _) = entry_fields(&entry);
            format!("record {attempt:?} {details}")
        }
        StartAction::ShardLost => "shard-lost".to_string(),
        StartAction::Error(WorkerExecutorError::FailedToResumeAgent { reason, .. }) => {
            format!("visibly {reason}")
        }
        StartAction::Error(WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(_),
        }) => "suspend".to_string(),
        StartAction::Error(WorkerExecutorError::Runtime { .. }) => "rebuild".to_string(),
        other => format!("{other:?}"),
    };
    let attempt = Some(OplogIndex::from_u64(9));
    let record = format!("record {attempt:?} {}", conflict());
    let visibly = format!(
        "visibly {}",
        WorkerExecutorError::invalid_request(restore(RestoreClass::Fixed).to_string())
    );
    let fixed = format!(
        "record {attempt:?} {}: {}",
        Code::SnapshotRestoreFailed.prefix(),
        restore(RestoreClass::Fixed)
    );
    let lost = format!(
        "record {attempt:?} {}: {}",
        Code::ManualSnapshotUnavailable.prefix(),
        restore(RestoreClass::Lost)
    );
    let disk_full = format!(
        "record {attempt:?} {}: {}",
        Code::SnapshotRestoreDiskFull.prefix(),
        restore(RestoreClass::DiskFull)
    );
    let rebuild = || "rebuild".to_string();
    let suspend = || "suspend".to_string();
    let skip = || "skip 10".to_string();
    // (role, error, without a lost shard, with a lost shard)
    let table = [
        ("initial", "access", rebuild(), rebuild()),
        ("initial", "sandbox", rebuild(), rebuild()),
        ("initial", "conflict", rebuild(), rebuild()),
        ("initial", "quota", suspend(), suspend()),
        ("initial", "capacity", rebuild(), rebuild()),
        ("initial", "restore-transient", rebuild(), rebuild()),
        ("initial", "restore-fixed", rebuild(), rebuild()),
        ("initial", "restore-lost", rebuild(), rebuild()),
        ("initial", "invalidated", rebuild(), rebuild()),
        ("periodic", "access", rebuild(), rebuild()),
        ("periodic", "sandbox", rebuild(), rebuild()),
        ("periodic", "conflict", rebuild(), rebuild()),
        ("periodic", "quota", suspend(), suspend()),
        ("periodic", "capacity", rebuild(), rebuild()),
        ("periodic", "restore-transient", skip(), skip()),
        ("periodic", "restore-fixed", skip(), skip()),
        ("periodic", "restore-lost", skip(), skip()),
        ("periodic", "restore-disk-full", skip(), skip()),
        ("periodic", "invalidated", rebuild(), rebuild()),
        ("manual-pending", "access", rebuild(), rebuild()),
        ("manual-pending", "sandbox", rebuild(), rebuild()),
        (
            "manual-pending",
            "conflict",
            record.clone(),
            "shard-lost".to_string(),
        ),
        ("manual-pending", "quota", suspend(), suspend()),
        ("manual-pending", "capacity", rebuild(), rebuild()),
        ("manual-pending", "restore-transient", rebuild(), rebuild()),
        (
            "manual-pending",
            "restore-fixed",
            fixed,
            "shard-lost".to_string(),
        ),
        (
            "manual-pending",
            "restore-lost",
            lost,
            "shard-lost".to_string(),
        ),
        (
            "manual-pending",
            "restore-disk-full",
            disk_full,
            "shard-lost".to_string(),
        ),
        ("manual-pending", "invalidated", rebuild(), rebuild()),
        ("manual-promoted", "access", rebuild(), rebuild()),
        ("manual-promoted", "sandbox", rebuild(), rebuild()),
        ("manual-promoted", "conflict", rebuild(), rebuild()),
        ("manual-promoted", "quota", suspend(), suspend()),
        ("manual-promoted", "capacity", rebuild(), rebuild()),
        ("manual-promoted", "restore-transient", rebuild(), rebuild()),
        (
            "manual-promoted",
            "restore-fixed",
            visibly.clone(),
            visibly.clone(),
        ),
        (
            "manual-promoted",
            "restore-disk-full",
            format!(
                "visibly {}",
                WorkerExecutorError::invalid_request(restore(RestoreClass::DiskFull).to_string())
            ),
            format!(
                "visibly {}",
                WorkerExecutorError::invalid_request(restore(RestoreClass::DiskFull).to_string())
            ),
        ),
        (
            "manual-promoted",
            "restore-lost",
            format!(
                "visibly {}",
                WorkerExecutorError::invalid_request(restore(RestoreClass::Lost).to_string())
            ),
            format!(
                "visibly {}",
                WorkerExecutorError::invalid_request(restore(RestoreClass::Lost).to_string())
            ),
        ),
        ("manual-promoted", "invalidated", rebuild(), rebuild()),
    ];

    table
        .into_iter()
        .for_each(|(role, error, without_lost_shard, with_lost_shard)| {
            let head = matches!(role, "manual-pending").then_some(&manual);
            let decide_with = |lost_shard| {
                outcome(decide(
                    &roles(role),
                    head,
                    RawStartError::Filesystem(&errors(error)),
                    &agent_id(),
                    lost_shard,
                ))
            };
            assert_eq!(
                (decide_with(false), decide_with(true)),
                (without_lost_shard, with_lost_shard),
                "{role} with {error}"
            );
        });
}

/// A restore into a full local disk or a full quota fails a pending manual or
/// snapshot-assisted update for good, as any other destination error does, and its details
/// say plainly that the disk is full: the code, the text and the cause.
#[test]
fn a_restore_into_a_full_disk_fails_a_pending_update_and_says_that_the_disk_is_full() {
    use crate::filesystem_snapshot::RestoreFailure;
    use crate::services::agent_filesystem_snapshots::restore_class;
    let manual = manual_head();
    let assisted = assisted_head();
    let failures = [libc::ENOSPC, libc::EDQUOT].map(|code| {
        let failure = RestoreFailure::Destination(std::io::Error::from_raw_os_error(code));
        FilesystemError::Baseline(Box::new(RestoreError {
            class: restore_class(&failure),
            source: anyhow::Error::new(failure),
        }))
    });
    let roles = [
        (
            BaselineRole::ManualPending(Arc::new(manual.clone())),
            manual,
        ),
        (
            BaselineRole::AssistedPending(Arc::new(assisted.clone())),
            assisted,
        ),
    ];

    failures.iter().for_each(|failure| {
        roles.iter().for_each(|(role, head)| {
            let (entry, reject) = failed_entry(decide_now(
                role,
                Some(head),
                RawStartError::Filesystem(failure),
            ));
            let (_, details, _, attempt, fault) = entry_fields(&entry);
            assert_eq!(
                details,
                format!(
                    "UPDATE_SNAPSHOT_RESTORE_FAILED: the executor's local disk is full; \
                         request the update again when it has space: {failure}"
                )
            );
            assert_eq!(
                (attempt, fault, reject),
                (Some(head.admission_index), None, None)
            );
        });
    });
}

/// The initial-file rule at the update point fails the update for a conflict and for an
/// initial-file source that the blob storage refuses. A full quota suspends the start, an
/// initial-file source that the blob storage cannot give now retries as a recovery, and every
/// other filesystem error, a failed download of an initial file included, retries the start;
/// none of them writes a failed update.
#[test]
fn the_initial_file_rule_at_the_update_point_fails_the_update_for_a_conflict_or_a_refused_source() {
    let assisted = assisted_head();
    let manual = manual_head();
    let automatic = automatic_head();
    let cases = [
        (
            BaselineRole::AssistedPending(Arc::new(assisted.clone())),
            assisted,
        ),
        (
            BaselineRole::ManualPending(Arc::new(manual.clone())),
            manual,
        ),
        (BaselineRole::InitialFiles, automatic.clone()),
        (BaselineRole::AssistedPromoted, automatic),
    ];
    let outcome = |action: StartAction| match action {
        StartAction::FailUpdate { .. } => "fail",
        StartAction::Error(WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(_),
        }) => "suspend",
        StartAction::Error(WorkerExecutorError::Runtime { .. }) => "retry",
        StartAction::Error(WorkerExecutorError::RecoveryRequired { .. }) => "recovery",
        _ => "other",
    };
    type MakeError = fn() -> FilesystemError;
    let errors: [(MakeError, &str); 8] = [
        (
            || FilesystemError::InitialFileConflict(Box::new(conflict())),
            "fail",
        ),
        (refused_source, "fail"),
        (|| FilesystemError::AgentQuota(storage()), "suspend"),
        (|| FilesystemError::Sandbox(storage()), "retry"),
        (
            || FilesystemError::InitialFileUnavailable(storage()),
            "recovery",
        ),
        (|| FilesystemError::PhysicalCapacity(storage()), "retry"),
        (|| FilesystemError::Access(AccessError::Revoked), "retry"),
        (|| FilesystemError::RuntimeInvalidated, "retry"),
    ];

    cases.iter().for_each(|(role, head)| {
        errors.iter().for_each(|(error, expected)| {
            let error = UpdateStateError::InitialFiles(error());
            assert_eq!(
                outcome(decide_now(
                    role,
                    Some(head),
                    RawStartError::UpdateState(&error)
                )),
                *expected,
                "{role:?} with {error}"
            );
        });
    });
}

/// An initial-file source that the blob storage refuses at the update point fails the update
/// with `UPDATE_BLOB_STORAGE_REFUSED`, the refusal and what to do next, then the cause. The
/// failed update carries the attempt index and no snapshot fault, and rejects no record: the
/// agent starts again on its source revision, as for a full local disk.
#[test]
fn a_refused_initial_file_source_at_the_update_point_fails_the_update_with_its_code() {
    let error = UpdateStateError::InitialFiles(refused_source());
    let head = automatic_head();

    let (entry, reject) = failed_entry(decide_now(
        &BaselineRole::InitialFiles,
        Some(&head),
        RawStartError::UpdateState(&error),
    ));
    let (_, details, assisted, attempt, fault) = entry_fields(&entry);

    assert_eq!(
        (details, assisted, attempt, fault, reject),
        (
            format!(
                "UPDATE_BLOB_STORAGE_REFUSED: the executor cannot read blob storage (access \
                     denied); request the update again after the storage is fixed: Applying \
                     worker update failed: {}",
                error
            ),
            None,
            Some(head.admission_index),
            None,
            None
        )
    );
}

/// A passed error at the update point: an unavailable component service and an unavailable
/// initial-file source retry as a recovery, a full quota suspends, and a permanent error
/// stays as it is.
#[test]
fn the_update_point_retries_only_a_transient_fetch_as_a_recovery() {
    let role = BaselineRole::InitialFiles;
    let head = automatic_head();
    let passed = |error: &UpdateStateError| match decide_now(
        &role,
        Some(&head),
        RawStartError::UpdateState(error),
    ) {
        StartAction::Error(error) => update_point_error(error),
        other => panic!("expected a passed error, got {other:?}"),
    };

    assert!(matches!(
        passed(&UpdateStateError::Metadata(unavailable_service())),
        WorkerExecutorError::RecoveryRequired { .. }
    ));
    assert!(matches!(
        passed(&UpdateStateError::InitialFiles(
            FilesystemError::InitialFileUnavailable(storage())
        )),
        WorkerExecutorError::RecoveryRequired { .. }
    ));
    assert!(matches!(
        passed(&UpdateStateError::InitialFiles(FilesystemError::Sandbox(
            storage()
        ))),
        WorkerExecutorError::Runtime { .. }
    ));
    assert!(matches!(
        passed(&UpdateStateError::InitialFiles(
            FilesystemError::AgentQuota(storage())
        )),
        WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(_)
        }
    ));
    assert!(matches!(
        update_point_error(not_found()),
        WorkerExecutorError::ComponentNotFound { .. }
    ));
    assert!(matches!(
        update_point_error(WorkerExecutorError::runtime("a sandbox failure")),
        WorkerExecutorError::Runtime { .. }
    ));
}

/// The answer for a baseline that names a filesystem snapshot on an executor without
/// filesystem snapshots: a promoted manual baseline fails the start visibly, and a pending
/// update fails the update.
#[test]
fn a_baseline_on_an_executor_without_snapshots_fails_a_pending_update_or_the_start() {
    let agent_id = agent_id();
    let manual = manual_head();
    let assisted = assisted_head();
    let promoted = decide(
        &BaselineRole::ManualPromoted,
        None,
        RawStartError::Disabled,
        &agent_id,
        false,
    );
    let periodic = decide(
        &BaselineRole::Periodic(OplogIndex::from_u64(10)),
        None,
        RawStartError::Disabled,
        &agent_id,
        false,
    );
    assert_eq!(
        format!("{promoted:?}"),
        format!(
            "{:?}",
            StartAction::Error(WorkerExecutorError::failed_to_resume_worker(
                agent_id.clone(),
                WorkerExecutorError::invalid_request(SnapshotsDisabled.to_string())
            ))
        )
    );
    assert_eq!(
        format!("{periodic:?}"),
        format!(
            "{:?}",
            StartAction::Error(WorkerExecutorError::runtime(SnapshotsDisabled.to_string()))
        )
    );
    [
        (
            BaselineRole::ManualPending(Arc::new(manual.clone())),
            manual,
        ),
        (
            BaselineRole::AssistedPending(Arc::new(assisted.clone())),
            assisted,
        ),
    ]
    .iter()
    .for_each(|(role, head)| {
        let (entry, reject) = failed_entry(decide(
            role,
            Some(head),
            RawStartError::Disabled,
            &agent_id,
            false,
        ));
        let (_, details, _, attempt, fault) = entry_fields(&entry);
        assert!(details.starts_with(&format!("{UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS}: ")));
        assert_eq!(
            (attempt, fault, reject),
            (Some(head.admission_index), None, None)
        );
    });
}

/// The instantiation failure for every queue head kind: only a
/// snapshot-assisted head fails its update, and an interruption fails none.
#[test]
fn an_instantiation_failure_fails_only_a_pending_assisted_update() {
    let runtime = WorkerExecutorError::runtime("target initializer trapped");
    let interrupted = WorkerExecutorError::Interrupted {
        kind: InterruptKind::Restart,
    };
    let heads = [
        (BaselineRole::InitialFiles, automatic_head()),
        (
            BaselineRole::ManualPending(Arc::new(manual_head())),
            manual_head(),
        ),
        (
            BaselineRole::AssistedPending(Arc::new(assisted_head())),
            assisted_head(),
        ),
        (
            BaselineRole::AssistedPending(Arc::new(head(
                assisted_kind(Some(FilesystemSnapshotName::periodic())),
                12,
                10,
            ))),
            head(
                assisted_kind(Some(FilesystemSnapshotName::periodic())),
                12,
                10,
            ),
        ),
    ];
    let fails = heads
        .iter()
        .map(|(role, head)| {
            matches!(
                decide_now(role, Some(head), RawStartError::Instantiation(&runtime)),
                StartAction::FailUpdate { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(fails, vec![false, false, true, true]);
    assert!(heads.iter().all(|(role, head)| matches!(
        decide_now(role, Some(head), RawStartError::Instantiation(&interrupted)),
        StartAction::Error(WorkerExecutorError::Interrupted { .. })
    )));
    let (entry, _) = failed_entry(decide_now(
        &heads[2].0,
        Some(&heads[2].1),
        RawStartError::Instantiation(&runtime),
    ));
    let (target, details, assisted, attempt, _) = entry_fields(&entry);
    assert_eq!(
        (target, details, assisted, attempt),
        (
            revision(3),
            "Snapshot-assisted automatic update failed while instantiating the target: \
                 Runtime error: target initializer trapped"
                .to_string(),
            Some(assisted_details()),
            Some(OplogIndex::from_u64(10)),
        )
    );
}

/// Characterizes the details of a failed and a successful update for every queue head kind:
/// only a snapshot-assisted head has assisted details, with the source start index.
#[test]
fn the_update_details_of_every_head_kind() {
    let success = SnapshotAssistedUpdateDetails {
        pending_update_index: OplogIndex::from_u64(12),
        source_component_revision: revision(2),
        source_revision_start_index: OplogIndex::from_u64(4),
        snapshot_index: OplogIndex::from_u64(7),
    };
    assert_eq!(
        [
            success_details_of(&automatic_head()),
            success_details_of(&manual_head()),
            success_details_of(&assisted_head()),
            success_details_of(&head(
                assisted_kind(Some(FilesystemSnapshotName::periodic())),
                12,
                10
            )),
        ],
        [None, None, Some(success.clone()), Some(success)]
    );
    assert_eq!(
        [
            failed_details_of(&automatic_head()),
            failed_details_of(&manual_head()),
            failed_details_of(&assisted_head()),
        ],
        [None, None, Some(assisted_details())]
    );
}

#[test]
fn a_failed_manual_admission_names_its_invocation() {
    let entry = failed_admission_of(
        revision(3),
        OplogIndex::from_u64(9),
        "cannot take a snapshot".to_string(),
    );
    assert_eq!(
        entry_fields(&entry),
        (
            revision(3),
            "cannot take a snapshot".to_string(),
            None,
            Some(OplogIndex::from_u64(9)),
            None
        )
    );
}

/// The failed details of a stale or downgrading assisted head name what the status has.
#[test]
fn a_stale_assisted_head_fails_with_what_the_status_has() {
    let head = assisted_head();
    let role = BaselineRole::AssistedPending(Arc::new(head.clone()));
    let stale = SourceFound {
        revision: revision(3),
        start_index: OplogIndex::from_u64(11),
    };
    let source = SourceFound {
        revision: revision(2),
        start_index: OplogIndex::from_u64(4),
    };
    let details = |found| {
        entry_fields(
            &failed_entry(decide_now(
                &role,
                Some(&head),
                RawStartError::StaleSource(found),
            ))
            .0,
        )
        .1
    };
    let restarted = SourceFound {
        revision: revision(2),
        start_index: OplogIndex::from_u64(11),
    };
    let moved = SourceFound {
        revision: revision(3),
        start_index: OplogIndex::from_u64(4),
    };
    assert_eq!(
        [
            details(&stale),
            details(&source),
            details(&restarted),
            details(&moved)
        ],
        [
            "Snapshot-assisted automatic update source became stale: expected revision 2 at \
                 revision start index 4, found revision 3 at revision start index 11"
                .to_string(),
            "Snapshot-assisted automatic update from revision 2 to revision 3 is not an \
                 upgrade"
                .to_string(),
            "Snapshot-assisted automatic update source became stale: expected revision 2 at \
                 revision start index 4, found revision 2 at revision start index 11"
                .to_string(),
            "Snapshot-assisted automatic update source became stale: expected revision 2 at \
                 revision start index 4, found revision 3 at revision start index 4"
                .to_string(),
        ]
    );
}

/// A failure of a start from a pending baseline fails the update of that baseline, whatever
/// head the caller passes, and the details of a failed load name the selected record.
#[test]
fn a_pending_baseline_fails_its_own_update_and_a_failed_load_names_its_record() {
    let runtime = WorkerExecutorError::runtime("boom");
    let failed_load = SnapshotRecoveryResult::Failed(runtime);
    let lost = restore(RestoreClass::Lost);
    let assisted = assisted_head();
    let manual = manual_head();
    let fields = |role: &BaselineRole, head: Option<&PendingUpdateRef>, error| {
        entry_fields(&failed_entry(decide_now(role, head, error)).0)
    };

    let (_, details, assisted_details_of_entry, attempt, _) = fields(
        &BaselineRole::AssistedPending(Arc::new(assisted.clone())),
        None,
        RawStartError::Load(&failed_load),
    );
    assert_eq!(
        (assisted_details_of_entry, attempt),
        (Some(assisted_details()), Some(OplogIndex::from_u64(10)))
    );
    assert!(
        details.ends_with(
            "Snapshot-assisted automatic update failed after the snapshot at 7: Runtime \
                 error: boom"
        ),
        "{details}"
    );

    let (_, _, manual_details, attempt, _) = fields(
        &BaselineRole::ManualPending(Arc::new(manual)),
        Some(&assisted),
        RawStartError::Filesystem(&lost),
    );
    assert_eq!(
        (manual_details, attempt),
        (None, Some(OplogIndex::from_u64(9)))
    );
}

/// The start error for every error of the agent filesystem: an agent
/// quota suspends the start, and every other error fails it with its message.
#[test]
fn reconstruction_startup_error_for_every_filesystem_error() {
    let others = [
        FilesystemError::Access(AccessError::Revoked),
        FilesystemError::Sandbox(storage()),
        FilesystemError::InitialFileUnavailable(storage()),
        FilesystemError::InitialFileConflict(Box::new(conflict())),
        FilesystemError::PhysicalCapacity(storage()),
        restore(RestoreClass::Transient),
        restore(RestoreClass::Fixed),
        restore(RestoreClass::Lost),
        FilesystemError::RuntimeInvalidated,
    ];

    assert!(matches!(
        reconstruction_startup_error(&FilesystemError::AgentQuota(storage())),
        WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(_)
        }
    ));
    others.iter().for_each(|error| {
        assert_eq!(
            reconstruction_startup_error(error),
            WorkerExecutorError::runtime(error.to_string())
        );
    });
}

#[test]
fn a_load_retry_keeps_its_decision_and_a_periodic_record_is_skipped_or_rejected() {
    let periodic = BaselineRole::Periodic(OplogIndex::from_u64(10));
    let retry = SnapshotRecoveryResult::Retry(RetryDecision::Immediate);
    let failed = SnapshotRecoveryResult::Failed(WorkerExecutorError::runtime("bad"));
    let runtime = WorkerExecutorError::runtime("trap");
    assert!(matches!(
        decide_now(&periodic, None, RawStartError::Load(&retry)),
        StartAction::Retry(RetryDecision::Immediate)
    ));
    assert!(matches!(
        decide_now(&periodic, None, RawStartError::Load(&failed)),
        StartAction::RejectPeriodic(index) if index == OplogIndex::from_u64(10)
    ));
    assert!(matches!(
        decide_now(
            &periodic,
            None,
            RawStartError::Replay {
                error: &runtime,
                diverged: true
            }
        ),
        StartAction::RejectPeriodic(index) if index == OplogIndex::from_u64(10)
    ));
    assert!(matches!(
        decide_now(
            &BaselineRole::ManualPromoted,
            None,
            RawStartError::Load(&failed)
        ),
        StartAction::Error(WorkerExecutorError::InvocationFailed { .. })
    ));
    let unavailable = SnapshotRecoveryResult::Unavailable(runtime.clone());
    assert!(matches!(
        decide_now(
            &BaselineRole::AssistedPromoted,
            None,
            RawStartError::Load(&unavailable)
        ),
        StartAction::Error(WorkerExecutorError::Runtime { .. })
    ));
}

/// An interrupted load of a pending manual update ends the start with its interrupt, so the
/// update stays pending and the next start loads the snapshot again; on a lost shard the new
/// owner does. A guest exit during the load fails the update and names the exit.
#[test]
fn an_interrupted_manual_load_keeps_the_update_pending_and_an_exit_fails_it() {
    let manual = manual_head();
    let role = BaselineRole::ManualPending(Arc::new(manual.clone()));
    [
        InterruptKind::Interrupt(Timestamp::from(5)),
        InterruptKind::Suspend(Timestamp::from(5)),
        InterruptKind::Restart,
        InterruptKind::Jump,
        InterruptKind::ShardLost,
    ]
    .into_iter()
    .for_each(|kind| {
        [false, true].into_iter().for_each(|lost_shard| {
            let action = decide(
                &role,
                Some(&manual),
                RawStartError::ManualLoad(&ManualLoadResult::Interrupted(kind)),
                &agent_id(),
                lost_shard,
            );
            assert!(
                matches!(
                    &action,
                    StartAction::Error(WorkerExecutorError::Interrupted { kind: actual })
                        if *actual == kind
                ),
                "{kind:?} {lost_shard}: {action:?}"
            );
        });
    });

    let (target, details, assisted, attempt, fault) = entry_fields(
        &failed_entry(decide_now(
            &role,
            Some(&manual),
            RawStartError::ManualLoad(&ManualLoadResult::Exited),
        ))
        .0,
    );
    assert_eq!(
        (target, assisted, attempt, fault),
        (revision(3), None, Some(OplogIndex::from_u64(9)), None)
    );
    assert_eq!(
        details,
        "Manual update failed to load snapshot: the agent exited during the snapshot load"
    );
    assert!(matches!(
        decide(
            &role,
            Some(&manual),
            RawStartError::ManualLoad(&ManualLoadResult::Exited),
            &agent_id(),
            true,
        ),
        StartAction::ShardLost
    ));
}

#[test]
fn a_full_replay_failure_of_an_automatic_update_names_the_replay_code() {
    let runtime = WorkerExecutorError::runtime("boom");
    let (entry, _) = failed_entry(decide_now(
        &BaselineRole::InitialFiles,
        Some(&automatic_head()),
        RawStartError::Replay {
            error: &runtime,
            diverged: false,
        },
    ));
    let (_, details, assisted, attempt, fault) = entry_fields(&entry);
    assert!(details.starts_with(&format!("{UPDATE_REPLAY_FAILED}: ")));
    assert!(details.ends_with("Automatic update failed: Runtime error: boom"));
    assert_eq!(
        (assisted, attempt, fault),
        (None, Some(OplogIndex::from_u64(10)), None)
    );
}

#[test]
fn the_replay_purpose_of_each_baseline_and_head() {
    let automatic = automatic_head();
    let manual = manual_head();
    assert_eq!(
        [
            BaselineRole::Periodic(OplogIndex::from_u64(10)).purpose(None),
            BaselineRole::AssistedPending(Arc::new(assisted_head()))
                .purpose(Some(&assisted_head())),
            BaselineRole::AssistedPromoted.purpose(None),
            BaselineRole::ManualPending(Arc::new(manual_head())).purpose(Some(&manual)),
            BaselineRole::ManualPromoted.purpose(None),
            BaselineRole::InitialFiles.purpose(None),
            BaselineRole::InitialFiles.purpose(Some(&automatic)),
            BaselineRole::AssistedPromoted.purpose(Some(&automatic)),
            BaselineRole::ManualPromoted.purpose(Some(&automatic)),
            BaselineRole::ManualPromoted.purpose(Some(&manual)),
        ],
        [
            SnapshotReplayPurpose::PeriodicRecovery,
            SnapshotReplayPurpose::AssistedUpdate,
            SnapshotReplayPurpose::None,
            SnapshotReplayPurpose::None,
            SnapshotReplayPurpose::None,
            SnapshotReplayPurpose::None,
            SnapshotReplayPurpose::AutomaticUpdate,
            SnapshotReplayPurpose::AutomaticUpdate,
            SnapshotReplayPurpose::AutomaticUpdate,
            SnapshotReplayPurpose::None,
        ]
    );
}
