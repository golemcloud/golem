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
use bytes::Bytes;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::{AgentOwnerPattern, EmptyOwnerPattern};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AgentCardHolder, AgentClass, AgentPermissionMonomorphizationContext, AgentResourcePattern,
    AgentVerb, Card, ClassPermissionPattern, ClassPermissionTarget, GrantSurface,
    NetworkResourcePattern, NetworkVerb, PermissionPattern, PermissionTarget, PolymorphicCard,
    PortPattern,
};
use golem_common::model::component::ComponentName;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::{PendingInvocationRef, PendingUpdateKind, PendingUpdateRef};
use http_body::Frame;
use std::collections::HashSet;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use test_r::test;

/// A payload of an application snapshot that does not download skips a periodic record for
/// the start attempt. A snapshot-assisted attempt whose payload store fails retries with the
/// same record through the recovery path: the outcome table writes no failed update and
/// rejects nothing for it. A snapshot-assisted attempt whose payload is missing or does not
/// decode fails the update as a lost record and rejects the record.
#[test]
fn a_payload_that_does_not_download_retries_an_assisted_attempt_only_when_its_store_failed() {
    use golem_common::model::{
        AssistedSelection, PendingUpdateKind, PendingUpdateRef, UsableAutomaticSnapshot,
    };
    let head = PendingUpdateRef {
        timestamp: Timestamp::from(1_000),
        oplog_index: OplogIndex::from_u64(12),
        admission_index: OplogIndex::from_u64(10),
        target_revision: ComponentRevision::new(3).unwrap(),
        kind: PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
            source_revision_start_index: OplogIndex::from_u64(4),
            snapshot: UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(7),
                component_revision: ComponentRevision::new(2).unwrap(),
                filesystem_snapshot: None,
            },
        })),
    };
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId::new(),
        agent_id: "payload".to_string(),
    };
    let decide = |result: &SnapshotRecoveryResult| {
        start_outcome::decide(
            &start_outcome::BaselineRole::AssistedPending(Arc::new(head.clone())),
            Some(&head),
            start_outcome::RawStartError::Load(result),
            &agent_id,
            false,
        )
    };
    let unavailable = payload_download_failure(
        SnapshotReplayPurpose::AssistedUpdate,
        false,
        "blob store unavailable".to_string(),
    );
    let lost = payload_download_failure(
        SnapshotReplayPurpose::AssistedUpdate,
        true,
        "referenced oplog payload is missing".to_string(),
    );

    [false, true].iter().for_each(|lost| {
        assert!(matches!(
            payload_download_failure(
                SnapshotReplayPurpose::PeriodicRecovery,
                *lost,
                "down".to_string()
            ),
            SnapshotRecoveryResult::Retry(RetryDecision::Immediate)
        ));
        assert!(matches!(
            payload_download_failure(SnapshotReplayPurpose::None, *lost, "down".to_string()),
            SnapshotRecoveryResult::Unavailable(WorkerExecutorError::Runtime { .. })
        ));
    });
    assert!(matches!(
        payload_download_failure(
            SnapshotReplayPurpose::AutomaticUpdate,
            false,
            "blob store unavailable".to_string()
        ),
        SnapshotRecoveryResult::Unavailable(WorkerExecutorError::RecoveryRequired { .. })
    ));
    assert!(matches!(
        payload_download_failure(
            SnapshotReplayPurpose::AutomaticUpdate,
            true,
            "referenced oplog payload is missing".to_string()
        ),
        SnapshotRecoveryResult::Unavailable(WorkerExecutorError::Runtime { .. })
    ));
    assert!(matches!(
        unavailable,
        SnapshotRecoveryResult::Unavailable(WorkerExecutorError::RecoveryRequired { .. })
    ));
    assert!(matches!(
        decide(&unavailable),
        start_outcome::StartAction::Error(WorkerExecutorError::RecoveryRequired { .. })
    ));
    match decide(&lost) {
        start_outcome::StartAction::FailUpdate {
            entry:
                OplogEntry::FailedUpdate {
                    details,
                    snapshot_assisted_details: Some(assisted),
                    snapshot_fault,
                    ..
                },
            reject,
        } => {
            assert!(
                details.as_deref().is_some_and(
                    |details| details.starts_with(start_outcome::UPDATE_SNAPSHOT_UNAVAILABLE)
                ),
                "{details:?}"
            );
            assert_eq!(assisted.snapshot_index, OplogIndex::from_u64(7));
            assert_eq!(
                snapshot_fault,
                Some(golem_common::model::oplog::SnapshotFault::Unavailable)
            );
            assert_eq!(reject, Some(OplogIndex::from_u64(7)));
        }
        other => panic!("a lost payload fails the update, got {other:?}"),
    }
}

/// An out-of-memory trap while the application snapshot loads retries the start with its
/// permits reacquired, as during any invocation, in a snapshot-assisted attempt and in the
/// start of a plain automatic update after a promoted baseline: the outcome table writes no
/// failed update for it. An interrupt keeps its fixed decision; any other trap does not retry.
#[test]
fn an_out_of_memory_trap_during_the_load_of_an_application_snapshot_retries_the_start() {
    let failed = |error: AgentError| InvokeResult::Failed {
        consumed_fuel: 0,
        error,
        timed_out: false,
        retry_from: OplogIndex::INITIAL,
        semantic_trap_retry_override: None,
    };
    let retry = |result: &InvokeResult| {
        DurableWorkerCtx::<crate::workerctx::default::Context>::load_retry(result)
    };
    let pending = |kind: PendingUpdateKind| PendingUpdateRef {
        timestamp: Timestamp::from(1_000),
        oplog_index: OplogIndex::from_u64(12),
        admission_index: OplogIndex::from_u64(10),
        target_revision: ComponentRevision::new(3).unwrap(),
        kind,
    };
    let assisted = pending(PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(
        golem_common::model::AssistedSelection {
            source_revision_start_index: OplogIndex::from_u64(4),
            snapshot: golem_common::model::UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(7),
                component_revision: ComponentRevision::new(2).unwrap(),
                filesystem_snapshot: None,
            },
        },
    )));
    let automatic = pending(PendingUpdateKind::Automatic);
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId::new(),
        agent_id: "load".to_string(),
    };
    let out_of_memory = SnapshotRecoveryResult::Retry(
        retry(&failed(AgentError::OutOfMemory)).expect("an out-of-memory trap retries"),
    );
    let decide = |role: start_outcome::BaselineRole, head: &PendingUpdateRef| {
        start_outcome::decide(
            &role,
            Some(head),
            start_outcome::RawStartError::Load(&out_of_memory),
            &agent_id,
            false,
        )
    };

    assert_eq!(
        [
            retry(&failed(AgentError::OutOfMemory)),
            retry(&failed(AgentError::InternalError("diverged".to_string()))),
            retry(&failed(AgentError::Unknown("transient".to_string()))),
            retry(&InvokeResult::Interrupted {
                consumed_fuel: 0,
                interrupt_kind: InterruptKind::Restart,
            }),
            retry(&InvokeResult::Exited { consumed_fuel: 0 }),
        ],
        [
            Some(RetryDecision::ReacquirePermits),
            None,
            None,
            Some(RetryDecision::Immediate),
            None,
        ]
    );
    assert!(matches!(
        decide(
            start_outcome::BaselineRole::AssistedPending(Arc::new(assisted.clone())),
            &assisted
        ),
        start_outcome::StartAction::Retry(RetryDecision::ReacquirePermits)
    ));
    [
        start_outcome::BaselineRole::AssistedPromoted,
        start_outcome::BaselineRole::ManualPromoted,
    ]
    .into_iter()
    .for_each(|role| {
        assert!(matches!(
            decide(role, &automatic),
            start_outcome::StartAction::Retry(RetryDecision::ReacquirePermits)
        ))
    });
}

/// An interrupt that ends a start, for example the restart of a revert while the start after
/// a failed update replays, keeps its form, so the invocation loop handles it as an interrupted
/// start and writes no recovery failure. A failure of the previous invocation keeps its form
/// too; every other error is a failure to resume the agent.
#[test]
fn a_prepare_failure_keeps_an_interrupt_and_a_previous_invocation_failure() {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "prepare".to_string(),
    };
    let interrupt = WorkerExecutorError::Interrupted {
        kind: InterruptKind::Restart,
    };
    let previous = WorkerExecutorError::PreviousInvocationFailed {
        error: AgentError::Unknown("trap".to_string()),
        stderr: String::new(),
    };
    let runtime = WorkerExecutorError::runtime("boom");
    assert_eq!(
        [
            prepare_failure(&agent_id, interrupt.clone()),
            prepare_failure(&agent_id, previous.clone()),
            prepare_failure(&agent_id, WorkerExecutorError::PreviousInvocationExited),
            prepare_failure(&agent_id, runtime.clone()),
        ],
        [
            interrupt,
            previous,
            WorkerExecutorError::PreviousInvocationExited,
            WorkerExecutorError::failed_to_resume_worker(agent_id.clone(), runtime),
        ]
    );
}

#[test]
fn replay_recovery_branch_preserves_owner_and_ephemeral_priority() {
    let result = Err(WorkerExecutorError::recovery_required(
        "payload backend unavailable",
    ));
    assert!(selected_recovery_error(None, AgentMode::Durable, &result).is_some());
    assert!(
        selected_recovery_error(Some(&lifecycle_winner()), AgentMode::Durable, &result).is_none()
    );
    assert!(selected_recovery_error(None, AgentMode::Ephemeral, &result).is_none());
}

fn lifecycle_winner() -> crate::durable_host::tool::operation::OwnerFailureWinner {
    crate::durable_host::tool::operation::OwnerFailureWinner::Lifecycle(InterruptKind::Interrupt(
        Timestamp::now_utc(),
    ))
}

/// A snapshot record whose storage did not answer is a recovery the start retries, not a
/// snapshot that failed to load; a snapshot record that does not read for good is the failed
/// oplog.
#[test]
fn a_snapshot_record_that_is_unavailable_is_a_recovery_not_a_failed_load() {
    use crate::services::oplog::{OplogReadError, OplogReadSource};

    let agent_id = recovered_agent().agent_id();
    let unavailable = snapshot_entry_read_failure(UnreadableOplog::new(
        &agent_id,
        OplogReadError::source_unavailable(OplogReadSource::Archive(1), "outage"),
    ));
    assert!(matches!(
        unavailable,
        SnapshotRecoveryResult::Unavailable(WorkerExecutorError::RecoveryRequired { .. })
    ));
    let corrupt = snapshot_entry_read_failure(UnreadableOplog::new(
        &agent_id,
        OplogReadError::corruption(OplogReadSource::Archive(1), "bad chunk"),
    ));
    assert!(matches!(
        corrupt,
        SnapshotRecoveryResult::Failed(WorkerExecutorError::OplogFailed { .. })
    ));
}

/// A tool failure that wins the owner arbitration as an infrastructure error keeps that
/// classification: a reconstruction that is required, or an oplog that failed, is selected
/// whatever the invocation itself returned. An ordinary infrastructure winner, a trap and a
/// lifecycle winner keep their own handling.
#[test]
fn a_winning_infrastructure_owner_failure_is_the_recovery_error() {
    use crate::durable_host::tool::operation::OwnerFailureWinner;

    let result = Err(WorkerExecutorError::runtime("guest trapped"));
    let recovery = OwnerFailureWinner::Infrastructure(WorkerExecutorError::recovery_required(
        "archive storage unavailable",
    ));
    assert_eq!(
        selected_recovery_error(Some(&recovery), AgentMode::Durable, &result),
        Some(&WorkerExecutorError::recovery_required(
            "archive storage unavailable"
        ))
    );
    let ordinary = OwnerFailureWinner::Infrastructure(WorkerExecutorError::runtime("host"));
    assert!(selected_recovery_error(Some(&ordinary), AgentMode::Durable, &result).is_none());
    assert!(selected_recovery_error(Some(&recovery), AgentMode::Ephemeral, &result).is_none());
    let recovery_result = Err(WorkerExecutorError::recovery_required("payload"));
    assert!(
        selected_recovery_error(
            Some(&lifecycle_winner()),
            AgentMode::Durable,
            &recovery_result
        )
        .is_none()
    );
}

/// An oplog that does not read ends the replay with its error instead of failing the guest.
#[test]
fn replay_recovery_branch_selects_an_unreadable_oplog() {
    let result = Err(WorkerExecutorError::oplog_failed(
        AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "unreadable".to_string(),
        },
        "corrupt oplog data from archive layer 1",
    ));
    assert_eq!(
        selected_recovery_error(None, AgentMode::Durable, &result),
        result.as_ref().err()
    );
    assert!(
        selected_recovery_error(Some(&lifecycle_winner()), AgentMode::Durable, &result).is_none()
    );
}

/// A refused write of the failure of an invocation takes the classification of its agent
/// error. A failed archive write, or an oplog that does not read for good, fails the invocation
/// with its cause. An oplog storage that did not answer is a recovery, so the invocation stays
/// pending. A fence publishes nothing, because the new owner of the shard runs the invocation.
#[test]
fn a_refused_invocation_failure_takes_the_classification_of_its_agent_error() {
    use crate::services::oplog::{
        OplogError, OplogFence, OplogReadError, OplogReadSource, UnreadableOplog,
    };

    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "terminal-entry".to_string(),
    };
    assert!(matches!(
        terminal_entry_refusal(&OplogError::ArchiveWrite(
            crate::services::oplog::ArchiveWriteFailure {
                agent_id: agent_id.clone(),
                level: 0,
                details: "injected archive failure".to_string(),
            }
        )),
        TerminalEntryRefusal::OplogFailed(WorkerExecutorError::OplogFailed { .. })
    ));
    assert!(matches!(
        terminal_entry_refusal(&OplogError::Unreadable(UnreadableOplog {
            agent_id: agent_id.clone(),
            error: OplogReadError::Gap {
                start: OplogIndex::INITIAL,
                end: OplogIndex::from_u64(2),
            },
        })),
        TerminalEntryRefusal::OplogFailed(WorkerExecutorError::OplogFailed { .. })
    ));
    assert!(matches!(
        terminal_entry_refusal(&OplogError::Unreadable(UnreadableOplog {
            agent_id: agent_id.clone(),
            error: OplogReadError::source_unavailable(
                OplogReadSource::Archive(0),
                "injected archive outage",
            ),
        })),
        TerminalEntryRefusal::RecoveryRequired(WorkerExecutorError::RecoveryRequired { .. })
    ));
    assert!(matches!(
        terminal_entry_refusal(&OplogError::Fenced(OplogFence {
            agent_id,
            expected_epoch: golem_common::model::ShardEpoch(1),
            actual_epoch: Some(golem_common::model::ShardEpoch(2)),
        })),
        TerminalEntryRefusal::Unpublished
    ));
    assert!(matches!(
        terminal_entry_refusal(&OplogError::Payload("payload".to_string())),
        TerminalEntryRefusal::Unpublished
    ));
}

#[test]
fn replay_recovery_branch_rejects_ordinary_host_failure() {
    let result = Err(WorkerExecutorError::runtime("host task failed"));
    assert!(selected_recovery_error(None, AgentMode::Durable, &result).is_none());
}

#[test]
fn ephemeral_restart_interrupts_become_invocation_failures() {
    let start_index = OplogIndex::from_u64(37);
    for (kind, message) in [
        (
            InterruptKind::Jump,
            "ephemeral agent cannot be restarted in process (Jump)",
        ),
        (
            InterruptKind::Restart,
            "ephemeral agent cannot be restarted in process (Restart)",
        ),
    ] {
        let trap = TrapType::Interrupt(kind);
        assert!(ephemeral_terminal_trap(AgentMode::Durable, &trap, start_index).is_none());
        let converted = ephemeral_terminal_trap(AgentMode::Ephemeral, &trap, start_index)
            .expect("ephemeral restart is terminal");
        assert!(matches!(&converted,
            TrapType::Error { error: AgentError::InternalError(error), retry_from,
                semantic_trap_retry_override: None }
            if error == message && *retry_from == start_index));
        assert!(matches!(converted.as_golem_error(""),
            Some(WorkerExecutorError::InvocationFailed { error: AgentError::InternalError(error), .. })
            if error == message));
    }
    for trap in [
        TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::now_utc())),
        TrapType::Interrupt(InterruptKind::Suspend(Timestamp::now_utc())),
        TrapType::Interrupt(InterruptKind::ShardLost),
        TrapType::Exit,
        TrapType::Error {
            error: AgentError::OutOfMemory,
            retry_from: start_index,
            semantic_trap_retry_override: None,
        },
    ] {
        assert!(ephemeral_terminal_trap(AgentMode::Ephemeral, &trap, start_index).is_none());
    }
}

#[test]
fn ephemeral_active_suspend_becomes_an_invocation_failure() {
    let start_index = OplogIndex::from_u64(37);
    let trap = TrapType::Interrupt(InterruptKind::Suspend(Timestamp::now_utc()));
    assert!(ephemeral_suspend_trap(AgentMode::Durable, &trap, start_index).is_none());
    let converted = ephemeral_suspend_trap(AgentMode::Ephemeral, &trap, start_index)
        .expect("ephemeral active suspension is terminal");
    assert!(matches!(&converted,
        TrapType::Error {
            error: AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError { reason }),
            retry_from,
            semantic_trap_retry_override: None,
        } if reason == "suspended while an invocation was active" && *retry_from == start_index));
    assert_eq!(
        DurableWorkerCtx::<crate::workerctx::default::Context>::fixed_decision_for_trap_type(
            &converted
        ),
        Some(RetryDecision::None)
    );
    for trap in [
        TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::now_utc())),
        TrapType::Interrupt(InterruptKind::ShardLost),
        TrapType::Interrupt(InterruptKind::Jump),
        TrapType::Interrupt(InterruptKind::Restart),
        TrapType::Exit,
    ] {
        assert!(ephemeral_suspend_trap(AgentMode::Ephemeral, &trap, start_index).is_none());
    }
}

#[test]
fn ephemeral_restart_decisions_are_terminal() {
    for (decision, ephemeral) in [
        (RetryDecision::Immediate, RetryDecision::None),
        (
            RetryDecision::Delayed(Duration::from_millis(37)),
            RetryDecision::None,
        ),
        (RetryDecision::ReacquirePermits, RetryDecision::None),
        (RetryDecision::None, RetryDecision::None),
        (
            RetryDecision::TryStop(Timestamp::from(1234)),
            RetryDecision::TryStop(Timestamp::from(1234)),
        ),
    ] {
        assert_eq!(
            ephemeral_terminal_decision(AgentMode::Durable, decision.clone()),
            decision
        );
        assert_eq!(
            ephemeral_terminal_decision(AgentMode::Ephemeral, decision),
            ephemeral
        );
    }
}

#[test]
fn ephemeral_replay_is_only_for_typed_owner_initialization() {
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};

    let agent = ResolvedOwnerContext::Agent(Box::new(ParsedAgentId::new(
        AgentTypeName("counter".to_string()),
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String("meaningful-constructor-input".to_string()),
        ),
        None,
    )));
    assert!(should_replay_invocation(AgentMode::Ephemeral, &agent, 0));
    assert!(!should_replay_invocation(AgentMode::Ephemeral, &agent, 1));
    for owner in [
        ResolvedOwnerContext::ComponentWorker,
        ResolvedOwnerContext::ComponentBaseline,
    ] {
        for replayed in [0, 1, 3] {
            assert!(!should_replay_invocation(
                AgentMode::Ephemeral,
                &owner,
                replayed
            ));
        }
    }
    for owner in [agent, ResolvedOwnerContext::ComponentWorker] {
        for replayed in [0, 1, 3] {
            assert!(should_replay_invocation(
                AgentMode::Durable,
                &owner,
                replayed
            ));
        }
    }
}

#[test]
fn component_file_node_reports_the_write_permission_of_a_file() {
    use crate::services::agent_filesystem as agent_fs;
    let file = |read_only| agent_fs::Attributes {
        kind: agent_fs::ObjectKind::File,
        link_count: 1,
        size: 7,
        accessed: None,
        modified: None,
        read_only,
    };

    let read_only = component_file_node(PathBuf::from("dir/read-only.txt"), file(true)).unwrap();
    let writable = component_file_node(PathBuf::from("writable.txt"), file(false)).unwrap();

    assert_eq!(
        read_only,
        ComponentFileSystemNode {
            name: "read-only.txt".to_string(),
            last_modified: SystemTime::UNIX_EPOCH,
            details: ComponentFileSystemNodeDetails::File {
                permissions: golem_common::model::component::AgentFilePermissions::ReadOnly,
                size: 7,
            },
        }
    );
    assert_eq!(
        writable.details,
        ComponentFileSystemNodeDetails::File {
            permissions: golem_common::model::component::AgentFilePermissions::ReadWrite,
            size: 7,
        }
    );
}

#[test]
fn entity_store_liveness_is_scoped_to_its_invocation_mode() {
    assert!(!store_is_live(None, false, false));
    assert!(store_is_live(None, false, true));

    let completed = Some(InvocationExecutionMode::ReplayingCompleted);
    assert!(!store_is_live(completed, false, false));
    assert!(!store_is_live(completed, false, true));
    assert!(!store_is_live(completed, true, true));

    let incomplete = Some(InvocationExecutionMode::ReplayingIncomplete);
    assert!(!store_is_live(incomplete, false, false));
    assert!(!store_is_live(incomplete, false, true));
    assert!(store_is_live(incomplete, true, true));
    assert!(store_is_live(
        Some(InvocationExecutionMode::Live),
        true,
        false
    ));
}

#[test]
async fn pending_replay_to_live_is_fail_closed_until_finished() {
    let memory = replay_usage_tracker();
    let _body = memory.begin_guest_body();
    let local_live_tail = Arc::new(AtomicBool::new(false));
    let pending = PendingReplayToLive {
        replay_target: OplogIndex::from_u64(42),
        role: ReplayToLiveRole::NonPrimary,
        replaying_incomplete_entity: false,
        tool_entity: false,
        tool_operation: None,
        local_live_tail: local_live_tail.clone(),
        linear_memory: memory.clone(),
    };

    assert_eq!(pending.replay_target(), OplogIndex::from_u64(42));
    assert!(!local_live_tail.load(Ordering::Acquire));
    assert!(!memory.usage_is_live());
    assert_eq!(
        pending.finish_at(None).await.unwrap(),
        FinishReplayToLive::Live
    );
    assert!(local_live_tail.load(Ordering::Acquire));
    assert!(memory.usage_is_live());

    let dropped_tail = Arc::new(AtomicBool::new(false));
    let dropped_memory = replay_usage_tracker();
    let _dropped_body = dropped_memory.begin_guest_body();
    drop(PendingReplayToLive {
        replay_target: OplogIndex::from_u64(43),
        role: ReplayToLiveRole::NonPrimary,
        replaying_incomplete_entity: false,
        tool_entity: false,
        tool_operation: None,
        local_live_tail: dropped_tail.clone(),
        linear_memory: dropped_memory.clone(),
    });
    assert!(!dropped_tail.load(Ordering::Acquire));
    assert!(!dropped_memory.usage_is_live());
}

pub(super) fn replay_usage_tracker() -> LinearMemoryTracker {
    LinearMemoryTracker::new_with_metering(
        0,
        0,
        AgentMode::Durable,
        true,
        Arc::new(AtomicResourceEntry::new(
            0,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            1,
        )),
        Arc::new(Mutex::new(MemoryGrant::inert(0))),
        false,
    )
}

#[test]
fn operator_authorized_invocation_guard_clears_mode_when_dropped() {
    let active = Arc::new(AtomicBool::new(false));
    let guard = OperatorAuthorizedOplogProcessorInvocationGuard::enter(&active);
    assert!(active.load(Ordering::Acquire));
    drop(guard);
    assert!(!active.load(Ordering::Acquire));
}

fn network_permission(host: &str) -> PermissionTarget {
    PermissionTarget::Network(ClassPermissionTarget {
        verb: Some(NetworkVerb::Connect),
        owner: EmptyOwnerPattern,
        resource: NetworkResourcePattern::host_port(host.to_string(), PortPattern::single(443)),
    })
}

#[test]
fn effective_surface_authorization_preserves_algebra_and_batch_atomicity() {
    let first = network_permission("first.example.com");
    let second = network_permission("second.example.com");
    let unrelated = network_permission("unrelated.example.com");
    let lower_or_upper_and = golem_common::model::card::EffectiveSurface {
        source_card_ids: Vec::new(),
        lower: vec![
            GrantSurface {
                positive: vec![first.clone()],
                negative: Vec::new(),
            },
            GrantSurface {
                positive: vec![second.clone()],
                negative: Vec::new(),
            },
        ],
        upper: vec![
            GrantSurface {
                positive: vec![first.clone(), second.clone()],
                negative: Vec::new(),
            },
            GrantSurface {
                positive: vec![first.clone(), second.clone(), unrelated],
                negative: Vec::new(),
            },
        ],
    };

    assert!(
        authorize_effective_surface(&lower_or_upper_and, &[first.clone(), second.clone()]).is_ok()
    );

    let mut denied = lower_or_upper_and;
    denied.upper[1].negative.push(second.clone());
    assert!(authorize_effective_surface(&denied, std::slice::from_ref(&first)).is_ok());
    let error = authorize_effective_surface(&denied, &[first, second]);
    assert!(matches!(
        error,
        Err(AuthorizationError::PermissionNotAllowed(_))
    ));
}

#[test]
fn authority_fast_path_requires_an_open_current_unexpired_snapshot() {
    let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let later = now + chrono::Duration::nanoseconds(1);

    assert!(authority_snapshot_is_current_at(
        true,
        true,
        7,
        7,
        Some(later),
        now,
    ));
    assert!(!authority_snapshot_is_current_at(
        false,
        true,
        7,
        7,
        Some(later),
        now,
    ));
    assert!(!authority_snapshot_is_current_at(
        true,
        false,
        7,
        7,
        Some(later),
        now,
    ));
    assert!(!authority_snapshot_is_current_at(
        true,
        true,
        6,
        7,
        Some(later),
        now,
    ));
    assert!(!authority_snapshot_is_current_at(
        true,
        true,
        7,
        7,
        Some(now),
        now,
    ));
}

#[test]
fn generation_recheck_invalidates_in_progress_allow_and_deny() {
    let allowed = network_permission("allowed.example.com");
    let denied = network_permission("denied.example.com");
    let surface = golem_common::model::card::EffectiveSurface {
        source_card_ids: Vec::new(),
        lower: vec![GrantSurface {
            positive: vec![allowed.clone()],
            negative: Vec::new(),
        }],
        upper: Vec::new(),
    };
    let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();

    assert!(authorize_effective_surface(&surface, &[allowed]).is_ok());
    assert!(authorize_effective_surface(&surface, &[denied]).is_err());
    assert!(authority_snapshot_is_current_at(
        true, true, 11, 11, None, now,
    ));
    assert!(
        !authority_snapshot_is_current_at(true, true, 11, 12, None, now),
        "a newly published revocation must invalidate an in-progress allow"
    );
    assert!(
        !authority_snapshot_is_current_at(true, true, 11, 12, None, now),
        "a newly published installation must invalidate an in-progress denial"
    );
}

fn authorize_boundary_for_test(
    initialized: bool,
    authority_open: bool,
    processed_generation: &mut u64,
    published_generation: u64,
    expiration: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    slow_refreshes: &mut usize,
    surface: &golem_common::model::card::EffectiveSurface,
    target: &PermissionTarget,
) -> Result<(), AuthorizationError> {
    if authority_snapshot_is_current_at(
        initialized,
        authority_open,
        *processed_generation,
        published_generation,
        expiration,
        now,
    ) {
        return authorize_effective_surface(surface, std::slice::from_ref(target));
    }
    *slow_refreshes += 1;
    if !authority_open {
        return Err(AuthorizationError::PermissionNotAllowed(Box::new(
            target.clone(),
        )));
    }
    *processed_generation = published_generation;
    authorize_effective_surface(surface, std::slice::from_ref(target))
}

fn allowing_surface(target: PermissionTarget) -> golem_common::model::card::EffectiveSurface {
    golem_common::model::card::EffectiveSurface {
        source_card_ids: Vec::new(),
        lower: vec![GrantSurface {
            positive: vec![target],
            negative: Vec::new(),
        }],
        upper: Vec::new(),
    }
}

#[test]
fn unchanged_authority_generation_uses_no_io_fast_path() {
    let target = network_permission("fast.example.com");
    let surface = allowing_surface(target.clone());
    let mut processed = 9;
    let mut slow_refreshes = 0;
    authorize_boundary_for_test(
        true,
        true,
        &mut processed,
        9,
        None,
        Utc::now(),
        &mut slow_refreshes,
        &surface,
        &target,
    )
    .unwrap();
    assert_eq!(slow_refreshes, 0);
}

#[test]
fn authority_event_burst_causes_exactly_one_slow_refresh() {
    let target = network_permission("burst.example.com");
    let surface = allowing_surface(target.clone());
    let mut processed = 4;
    let mut slow_refreshes = 0;
    for _ in 0..2 {
        authorize_boundary_for_test(
            true,
            true,
            &mut processed,
            8,
            None,
            Utc::now(),
            &mut slow_refreshes,
            &surface,
            &target,
        )
        .unwrap();
    }
    assert_eq!(slow_refreshes, 1);
    assert_eq!(processed, 8);
}

#[test]
fn closed_authority_cannot_allow() {
    let target = network_permission("closed.example.com");
    let surface = allowing_surface(target.clone());
    let mut processed = 3;
    let mut slow_refreshes = 0;
    assert!(
        authorize_boundary_for_test(
            true,
            false,
            &mut processed,
            3,
            None,
            Utc::now(),
            &mut slow_refreshes,
            &surface,
            &target,
        )
        .is_err()
    );
}

#[test]
fn expiration_is_visible_at_first_due_live_boundary() {
    let target = network_permission("expired.example.com");
    let surface = allowing_surface(target.clone());
    let due = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut processed = 5;
    let mut slow_refreshes = 0;
    authorize_boundary_for_test(
        true,
        true,
        &mut processed,
        5,
        Some(due),
        due,
        &mut slow_refreshes,
        &surface,
        &target,
    )
    .unwrap();
    assert_eq!(slow_refreshes, 1, "the boundary at expires_at must refresh");
}

#[test]
#[should_panic(expected = "live permission authorization must not run during replay")]
fn replay_cannot_invoke_authorization_helper() {
    assert_live_authorization(false);
}
use test_r::timeout;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn aggregate_memory_growth_over_worker_limit_is_rejected() {
    let now = Instant::now();
    let tracker = LinearMemoryTracker::new(
        60,
        60,
        AgentMode::Durable,
        false,
        Arc::new(AtomicResourceEntry::new(0, 100, 0, 0, 0)),
        Arc::new(Mutex::new(MemoryGrant::inert(60))),
        now,
    );
    tracker.reconcile_at(60, now);

    let second_memory_growth = tracker.prepare_unshared_growth(0, 60);

    assert_eq!(
        validate_unshared_memory_growth(second_memory_growth, 100, 60, Some(100)),
        None,
        "two individually valid 60-byte memories must not bypass the 100-byte aggregate cap"
    );
}

#[test]
fn snapshot_boundary_all_clear_has_no_blocker() {
    assert_eq!(SnapshotBoundaryConditions::default().blocker(), None);
}

#[test]
fn snapshot_boundary_each_condition_blocks_alone() {
    let cases = [
        (
            SnapshotBoundaryConditions {
                replaying: true,
                ..Default::default()
            },
            SnapshotBoundaryBlocker::Replaying,
        ),
        (
            SnapshotBoundaryConditions {
                snapshotting: true,
                ..Default::default()
            },
            SnapshotBoundaryBlocker::Snapshotting,
        ),
        (
            SnapshotBoundaryConditions {
                in_flight_host_call: true,
                ..Default::default()
            },
            SnapshotBoundaryBlocker::InFlightHostCall,
        ),
        (
            SnapshotBoundaryConditions {
                open_tool_attachment: true,
                ..Default::default()
            },
            SnapshotBoundaryBlocker::OpenToolAttachment,
        ),
    ];
    for (conditions, expected) in cases {
        assert_eq!(
            conditions.blocker(),
            Some(expected),
            "single blocking condition {conditions:?} must be reported as {expected:?}"
        );
    }
}

#[test]
fn snapshot_boundary_any_condition_combination_blocks() {
    // Exhaustive truth table over the four conditions: a snapshot is admitted iff every
    // condition is clear, and the reported blocker is always one of the set conditions.
    for bits in 0u32..16 {
        let conditions = SnapshotBoundaryConditions {
            replaying: bits & 1 != 0,
            in_flight_host_call: bits & 2 != 0,
            snapshotting: bits & 4 != 0,
            open_tool_attachment: bits & 8 != 0,
        };
        let blocker = conditions.blocker();
        assert_eq!(
            blocker.is_none(),
            bits == 0,
            "snapshot must be admitted iff no condition is set; conditions: {conditions:?}"
        );
        if let Some(blocker) = blocker {
            let named_condition_is_set = match blocker {
                SnapshotBoundaryBlocker::Replaying => conditions.replaying,
                SnapshotBoundaryBlocker::Snapshotting => conditions.snapshotting,
                SnapshotBoundaryBlocker::InFlightHostCall => conditions.in_flight_host_call,
                SnapshotBoundaryBlocker::OpenToolAttachment => conditions.open_tool_attachment,
            };
            assert!(
                named_condition_is_set,
                "reported blocker {blocker:?} must name a set condition; conditions: {conditions:?}"
            );
        }
    }
}

#[test]
fn checkpoint_boundary_truth_table() {
    // Exhaustive truth table over the three blocking conditions: a mid-invocation status
    // checkpoint is admitted iff every condition is clear.
    for bits in 0u32..8 {
        let replaying = bits & 1 != 0;
        let in_flight_host_call = bits & 2 != 0;
        let snapshotting = bits & 4 != 0;
        assert_eq!(
            PrivateDurableWorkerState::clean_checkpoint_boundary(
                replaying,
                in_flight_host_call,
                snapshotting,
            ),
            bits == 0,
            "checkpoint must be admitted iff no condition is set; replaying: {replaying}, \
             in_flight_host_call: {in_flight_host_call}, \
             snapshotting: {snapshotting}"
        );
    }
}

#[test]
fn snapshot_boundary_is_checkpoint_boundary_without_live_resources() {
    // The documented sync invariant between the two predicates: `blocker() == None` is
    // equivalent to a clean checkpoint without in-flight calls or open tool attachments.
    for bits in 0u32..16 {
        let conditions = SnapshotBoundaryConditions {
            replaying: bits & 1 != 0,
            in_flight_host_call: bits & 2 != 0,
            snapshotting: bits & 4 != 0,
            open_tool_attachment: bits & 8 != 0,
        };
        let at_checkpoint_boundary = PrivateDurableWorkerState::clean_checkpoint_boundary(
            conditions.replaying,
            conditions.in_flight_host_call,
            conditions.snapshotting,
        );
        assert_eq!(
            conditions.blocker().is_none(),
            at_checkpoint_boundary
                && !conditions.in_flight_host_call
                && !conditions.open_tool_attachment,
            "snapshot admission requires a checkpoint without live calls or attachments; \
             conditions: {conditions:?}"
        );
    }
}

fn permission_id_test_inputs() -> (OwnedAgentId, IdempotencyKey) {
    (
        OwnedAgentId {
            environment_id: EnvironmentId(
                Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap(),
            ),
            agent_id: AgentId {
                component_id: ComponentId(
                    Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").unwrap(),
                ),
                agent_id: "cart/primary".to_string(),
            },
        },
        IdempotencyKey::new("checkout-invocation".to_string()),
    )
}

fn invocation_scope_card(root_card_ids: Vec<CardId>) -> ScopeCard {
    ScopeCard {
        scope_card_id: CardId(Uuid::from_u128(100)),
        root_card_ids,
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        expires_at: None,
    }
}

#[test]
fn unrelated_revocation_preserves_invocation_scope_and_handles() {
    let scope_root = CardId(Uuid::from_u128(1));
    let unrelated = CardId(Uuid::from_u128(2));
    let mut scope_card = Some(invocation_scope_card(vec![scope_root]));
    let mut handles = HashMap::from([(7, vec![scope_root])]);

    assert_eq!(
        remove_invocation_scope_for_revoked_roots(&mut scope_card, &mut handles, &[unrelated],),
        (false, HashSet::new())
    );
    assert!(scope_card.is_some());
    assert_eq!(handles, HashMap::from([(7, vec![scope_root])]));
}

#[test]
fn matching_revocation_removes_scope_and_dependent_handles_only() {
    let revoked_root = CardId(Uuid::from_u128(1));
    let unrelated_root = CardId(Uuid::from_u128(2));
    let mut scope_card = Some(invocation_scope_card(vec![revoked_root]));
    let mut handles = HashMap::from([
        (7, vec![revoked_root]),
        (11, vec![unrelated_root]),
        (13, vec![revoked_root, unrelated_root]),
    ]);

    assert_eq!(
        remove_invocation_scope_for_revoked_roots(&mut scope_card, &mut handles, &[revoked_root],),
        (true, HashSet::from([7, 13]))
    );
    assert!(scope_card.is_none());
    assert_eq!(handles, HashMap::from([(11, vec![unrelated_root])]));
}

#[test]
fn inherited_scope_roots_are_checked_once_and_unknown_roots_are_errors() {
    let first = CardId(Uuid::from_u128(1));
    let second = CardId(Uuid::from_u128(2));
    let live = |card_id| CardState::Live(Box::new(concrete_card(card_id)));
    let unchecked =
        || InheritedInvocationScope::unchecked(invocation_scope_card(vec![first, second]));

    let mut scope = unchecked();
    assert_eq!(scope.unchecked_root_ids(), Some(vec![first, second]));
    assert!(
        scope
            .check_roots(&HashMap::from([
                (first, live(first)),
                (second, live(second))
            ]))
            .unwrap()
    );
    assert_eq!(
        scope
            .live_roots()
            .map(|roots| roots.keys().copied().collect::<Vec<_>>()),
        Some(vec![first, second])
    );
    assert_eq!(scope.unchecked_root_ids(), None);
    assert!(scope.check_roots(&HashMap::new()).unwrap());

    let mut scope = unchecked();
    assert!(
        !scope
            .check_roots(&HashMap::from([
                (first, live(first)),
                (second, CardState::Revoked)
            ]))
            .unwrap()
    );

    let mut scope = unchecked();
    assert!(
        scope
            .check_roots(&HashMap::from([
                (first, live(first)),
                (second, CardState::Unknown)
            ]))
            .is_err()
    );
    let mut scope = unchecked();
    assert!(
        scope
            .check_roots(&HashMap::from([(first, live(first))]))
            .is_err()
    );
}

#[test]
fn inherited_scope_detects_revoked_and_expired_roots() {
    let root = CardId(Uuid::from_u128(1));
    let unrelated = CardId(Uuid::from_u128(2));
    let now = Utc::now();
    let scope = InheritedInvocationScope {
        card: invocation_scope_card(vec![root]),
        roots: Some(BTreeMap::from([(
            root,
            concrete_card_expiring_at(root, Some(now + chrono::Duration::seconds(60))),
        )])),
    };

    assert!(!scope.roots_include(&HashSet::from([unrelated])));
    assert!(scope.roots_include(&HashSet::from([root])));
    assert!(!scope.roots_expired(now));
    assert!(scope.roots_expired(now + chrono::Duration::seconds(60)));
    assert!(
        !InheritedInvocationScope::unchecked(invocation_scope_card(vec![root]))
            .roots_expired(now + chrono::Duration::seconds(60))
    );
}

#[test]
fn invocation_end_clears_scope_card_and_all_invocation_handles() {
    let mut scope_card = Some(invocation_scope_card(vec![CardId(Uuid::from_u128(1))]));
    let mut handles = HashMap::from([
        (7, vec![CardId(Uuid::from_u128(1))]),
        (11, vec![CardId(Uuid::from_u128(2))]),
    ]);

    let (scope_changed, removed_handles) =
        clear_invocation_scope_state(&mut scope_card, &mut handles);

    assert!(scope_changed);
    assert!(scope_card.is_none());
    assert!(handles.is_empty());
    assert_eq!(removed_handles, HashSet::from([7, 11]));
}

#[test]
fn permission_id_derivation_has_golden_vectors() {
    let (owned_agent_id, invocation_key) = permission_id_test_inputs();
    let oplog_index = OplogIndex::from_u64(42);

    let card_id = derive_permission_uuid(
        DERIVED_CARD_ID_CONTEXT,
        &owned_agent_id,
        &invocation_key,
        oplog_index,
    );
    let transfer_id = derive_permission_uuid(
        TRANSFER_ID_CONTEXT,
        &owned_agent_id,
        &invocation_key,
        oplog_index,
    );
    let installed_child_id = derive_permission_uuid(
        INSTALLED_CHILD_CARD_ID_CONTEXT,
        &owned_agent_id,
        &invocation_key,
        oplog_index,
    );
    let scope_card_id = derive_permission_uuid_for_sequence(
        SCOPE_CARD_ID_CONTEXT,
        &owned_agent_id,
        &invocation_key,
        42,
    );

    assert_eq!(
        card_id,
        Uuid::parse_str("00000000-002a-7cdd-b83d-c9e9f32d880c").unwrap()
    );
    assert_eq!(
        transfer_id,
        Uuid::parse_str("00000000-002a-7646-b8aa-b84e348196f6").unwrap()
    );
    assert_eq!(
        installed_child_id,
        Uuid::parse_str("00000000-002a-7607-aaa4-0b04c840169d").unwrap()
    );
    assert_eq!(
        scope_card_id,
        Uuid::parse_str("00000000-002a-7680-bbdb-7f78bd28fc43").unwrap()
    );
    assert_eq!(card_id.as_bytes()[6] >> 4, 7);
    assert_eq!(card_id.as_bytes()[8] >> 6, 2);
    assert_eq!(scope_card_id.as_bytes()[6] >> 4, 7);
    assert_eq!(scope_card_id.as_bytes()[8] >> 6, 2);
}

#[test]
fn permission_card_ids_follow_oplog_order() {
    let (owned_agent_id, invocation_key) = permission_id_test_inputs();
    let derive = |index| {
        derive_permission_uuid(
            DERIVED_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            OplogIndex::from_u64(index),
        )
    };

    assert!(derive(41) < derive(42));
    assert!(derive(42) < derive(43));
}

#[test]
fn scope_card_ids_follow_the_invocation_local_ordinal() {
    let (owned_agent_id, invocation_key) = permission_id_test_inputs();
    let derive = |ordinal| {
        derive_permission_uuid_for_sequence(
            SCOPE_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            ordinal,
        )
    };

    assert!(derive(0) < derive(1));
    assert!(derive(1) < derive(2));
    assert_ne!(derive(0), derive(1));
}

#[test]
fn permission_id_derivation_accepts_the_full_oplog_index_domain() {
    let (owned_agent_id, invocation_key) = permission_id_test_inputs();

    derive_permission_uuid(
        DERIVED_CARD_ID_CONTEXT,
        &owned_agent_id,
        &invocation_key,
        OplogIndex::from_u64(1_u64 << 48),
    );
}

#[test]
fn permission_ids_remain_distinct_after_timestamp_saturation() {
    let (owned_agent_id, invocation_key) = permission_id_test_inputs();
    let ids = [UUID_V7_MAX_TIMESTAMP, 1_u64 << 48, u64::MAX].map(|index| {
        derive_permission_uuid(
            DERIVED_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            OplogIndex::from_u64(index),
        )
    });

    assert_eq!(ids.into_iter().collect::<HashSet<_>>().len(), 3);
}

#[test]
fn permission_id_domains_are_distinct() {
    let (owned_agent_id, invocation_key) = permission_id_test_inputs();
    let oplog_index = OplogIndex::from_u64(42);
    let ids = [
        derive_permission_uuid(
            DERIVED_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index,
        ),
        derive_permission_uuid(
            TRANSFER_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index,
        ),
        derive_permission_uuid(
            INSTALLED_CHILD_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index,
        ),
        derive_permission_uuid_for_sequence(
            SCOPE_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index.as_u64(),
        ),
    ];

    assert_eq!(ids.into_iter().collect::<HashSet<_>>().len(), 4);
}

#[test]
fn permission_ids_do_not_collide_across_workers() {
    let (base_agent, invocation_key) = permission_id_test_inputs();
    let different_environment = OwnedAgentId {
        environment_id: EnvironmentId(
            Uuid::parse_str("10112233-4455-6677-8899-aabbccddeeff").unwrap(),
        ),
        ..base_agent.clone()
    };
    let different_component = OwnedAgentId {
        agent_id: AgentId {
            component_id: ComponentId(
                Uuid::parse_str("efeeddcc-bbaa-9988-7766-554433221100").unwrap(),
            ),
            ..base_agent.agent_id.clone()
        },
        ..base_agent.clone()
    };
    let different_name = OwnedAgentId {
        agent_id: AgentId {
            agent_id: "cart/secondary".to_string(),
            ..base_agent.agent_id.clone()
        },
        ..base_agent.clone()
    };
    let oplog_index = OplogIndex::from_u64(42);
    let ids = [
        base_agent,
        different_environment,
        different_component,
        different_name,
    ]
    .map(|owned_agent_id| {
        derive_permission_uuid(
            DERIVED_CARD_ID_CONTEXT,
            &owned_agent_id,
            &invocation_key,
            oplog_index,
        )
    });

    assert_eq!(ids.into_iter().collect::<HashSet<_>>().len(), 4);
}

fn concrete_card(card_id: CardId) -> StoredCard {
    Card {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
    .into()
}

fn concrete_card_expiring_at(card_id: CardId, expires_at: Option<DateTime<Utc>>) -> StoredCard {
    let mut card = concrete_card(card_id);
    match &mut card {
        StoredCard::Concrete(card) => card.expires_at = expires_at,
        StoredCard::Polymorphic(_) => unreachable!(),
    }
    card
}

fn agent_permission_pattern(
    owner: &str,
    recipient: &str,
    verb: Option<AgentVerb>,
) -> PermissionPattern {
    PermissionPattern::Agent(ClassPermissionPattern::<AgentClass> {
        verb,
        owner: AgentOwnerPattern::parse(owner).unwrap(),
        recipient: RecipientPattern::parse(recipient).unwrap(),
        resource: AgentResourcePattern::Any,
    })
}

fn agent_permission_card(
    card_id: CardId,
    owner: &str,
    recipient: &str,
    lower_verbs: Vec<Option<AgentVerb>>,
    upper_verbs: Vec<Option<AgentVerb>>,
) -> StoredCard {
    Card {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: lower_verbs
            .into_iter()
            .map(|verb| agent_permission_pattern(owner, recipient, verb))
            .collect(),
        lower_negative: Vec::new(),
        upper_positive: upper_verbs
            .into_iter()
            .map(|verb| agent_permission_pattern(owner, recipient, verb))
            .collect(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
    .into()
}

fn agent_permission_test_context() -> AgentPermissionMonomorphizationContext {
    AgentPermissionMonomorphizationContext {
        account: AccountEmail::from("owner@example.com"),
        application: ApplicationName::try_from("shop").unwrap(),
        environment: EnvironmentName::try_from("prod").unwrap(),
        component: ComponentName("cart-svc".to_string()),
        agent_name: "Cart(alice)".to_string(),
        owner: golem_common::model::card::recipient::RecipientOwnerContext::AgentType(
            AgentTypeName("Cart".to_string()),
        ),
    }
}

fn agent_permission_target(holder: &str, verb: AgentVerb) -> PermissionTarget {
    PermissionTarget::Agent(ClassPermissionTarget::<AgentClass> {
        verb: Some(verb),
        owner: AgentOwnerPattern::parse(holder).unwrap(),
        resource: AgentResourcePattern::Any,
    })
}

fn wallet_cards(cards: impl IntoIterator<Item = (CardId, StoredCard)>) -> WalletCards {
    cards
        .into_iter()
        .map(|(id, card)| (id, Arc::new(card)))
        .collect()
}

#[test]
fn wallet_membership_helpers_bump_once_per_effective_change() {
    let card_id = CardId::new();
    let card = concrete_card(card_id);
    let mut wallet = WalletCards::new();
    let mut generation = 10;

    assert!(add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
    assert_eq!(generation, 11);
    assert!(!add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
    assert_eq!(generation, 11);

    let mut replacement = card.clone();
    match &mut replacement {
        StoredCard::Concrete(card) => card.system_card = true,
        StoredCard::Polymorphic(_) => unreachable!(),
    }
    assert!(add_wallet_card(&mut wallet, &mut generation, replacement).is_err());
    assert_eq!(generation, 11);

    assert!(!remove_wallet_card(&mut wallet, &mut generation, CardId::new()).unwrap());
    assert_eq!(generation, 11);
    assert!(remove_wallet_card(&mut wallet, &mut generation, card_id).unwrap());
    assert_eq!(generation, 12);
}

#[test]
fn wallet_cascade_removal_bumps_once_for_all_effective_removals() {
    let first = concrete_card(CardId::new());
    let second = concrete_card(CardId::new());
    let retained = concrete_card(CardId::new());
    let mut wallet = wallet_cards([
        (first.card_id(), first.clone()),
        (second.card_id(), second.clone()),
        (retained.card_id(), retained.clone()),
    ]);
    let mut generation = 10;

    assert!(
        remove_wallet_cards(
            &mut wallet,
            &mut generation,
            &[first.card_id(), second.card_id(), CardId::new()],
        )
        .unwrap()
    );
    assert_eq!(generation, 11);
    assert_eq!(wallet, wallet_cards([(retained.card_id(), retained)]));

    assert!(
        !remove_wallet_cards(
            &mut wallet,
            &mut generation,
            &[first.card_id(), second.card_id()],
        )
        .unwrap()
    );
    assert_eq!(generation, 11);
}

#[test]
fn wallet_cascade_removal_is_atomic_when_generation_is_exhausted() {
    let card = concrete_card(CardId::new());
    let original_wallet = wallet_cards([(card.card_id(), card.clone())]);
    let mut wallet = original_wallet.clone();
    let mut generation = u64::MAX;

    assert!(remove_wallet_cards(&mut wallet, &mut generation, &[card.card_id()]).is_err());
    assert_eq!(wallet, original_wallet);
    assert_eq!(generation, u64::MAX);
}

#[test]
fn wallet_cascade_preserves_independent_floor_releases_ceiling_and_replays_identically() {
    let holder = "owner@example.com/shop/prod/cart-svc/Cart(alice)";
    let recipient = "owner@example.com/shop/prod/cart-svc/Cart";
    let context = agent_permission_test_context();
    let view = agent_permission_target(holder, AgentVerb::View);
    let invoke = agent_permission_target(holder, AgentVerb::Invoke);
    let broad_floor =
        agent_permission_card(CardId::new(), holder, recipient, vec![None], Vec::new());
    let independent_view_floor = agent_permission_card(
        CardId::new(),
        holder,
        recipient,
        vec![Some(AgentVerb::View)],
        Vec::new(),
    );
    let view_ceiling = agent_permission_card(
        CardId::new(),
        holder,
        recipient,
        Vec::new(),
        vec![Some(AgentVerb::View)],
    );
    let original_wallet = wallet_cards([
        (broad_floor.card_id(), broad_floor.clone()),
        (
            independent_view_floor.card_id(),
            independent_view_floor.clone(),
        ),
        (view_ceiling.card_id(), view_ceiling.clone()),
    ]);
    let original_surface = golem_common::model::card::agent_effective_surface_from_wallet(
        &context,
        original_wallet.values().map(Arc::as_ref),
    );
    assert!(original_surface.authorize(&view).unwrap());
    assert!(!original_surface.authorize(&invoke).unwrap());

    let mut live_wallet = original_wallet.clone();
    let mut live_generation = 12;
    assert!(
        remove_wallet_cards(
            &mut live_wallet,
            &mut live_generation,
            &[broad_floor.card_id()],
        )
        .unwrap()
    );
    let live_surface = golem_common::model::card::agent_effective_surface_from_wallet(
        &context,
        live_wallet.values().map(Arc::as_ref),
    );
    assert_eq!(live_generation, 13);
    assert!(live_surface.authorize(&view).unwrap());
    assert!(!live_surface.authorize(&invoke).unwrap());

    let mut replayed_wallet = original_wallet.clone();
    let mut replayed_generation = 12;
    assert!(
        remove_wallet_cards(
            &mut replayed_wallet,
            &mut replayed_generation,
            &[broad_floor.card_id()],
        )
        .unwrap()
    );
    adopt_recorded_wallet_generation(&mut replayed_generation, live_generation).unwrap();
    let replayed_surface = golem_common::model::card::agent_effective_surface_from_wallet(
        &context,
        replayed_wallet.values().map(Arc::as_ref),
    );
    assert_eq!(replayed_wallet, live_wallet);
    assert_eq!(replayed_generation, live_generation);
    assert_eq!(
        replayed_surface.source_card_ids,
        live_surface.source_card_ids
    );
    assert_eq!(
        replayed_surface.authorize(&view).unwrap(),
        live_surface.authorize(&view).unwrap()
    );
    assert_eq!(
        replayed_surface.authorize(&invoke).unwrap(),
        live_surface.authorize(&invoke).unwrap()
    );

    let mut released_wallet = original_wallet;
    let mut released_generation = 12;
    assert!(
        remove_wallet_cards(
            &mut released_wallet,
            &mut released_generation,
            &[view_ceiling.card_id()],
        )
        .unwrap()
    );
    let released_surface = golem_common::model::card::agent_effective_surface_from_wallet(
        &context,
        released_wallet.values().map(Arc::as_ref),
    );
    assert_eq!(released_generation, 13);
    assert!(released_surface.authorize(&view).unwrap());
    assert!(released_surface.authorize(&invoke).unwrap());
}

#[test]
fn wallet_generation_distinguishes_aba_membership_cycles() {
    let card = concrete_card(CardId::new());
    let mut wallet = WalletCards::new();
    let mut generation = 0;

    assert!(add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
    let first_contents = wallet.clone();
    let first_generation = generation;

    assert!(remove_wallet_card(&mut wallet, &mut generation, card.card_id()).unwrap());
    assert!(add_wallet_card(&mut wallet, &mut generation, card).unwrap());

    assert_eq!(wallet, first_contents);
    assert_eq!(first_generation, 1);
    assert_eq!(generation, 3);
}

#[test]
fn duplicate_wallet_delivery_does_not_bump_generation() {
    let card = concrete_card(CardId::new());
    let mut wallet = WalletCards::new();
    let mut generation = 40;

    assert!(add_wallet_card(&mut wallet, &mut generation, card.clone()).unwrap());
    assert!(!add_wallet_card(&mut wallet, &mut generation, card).unwrap());

    assert_eq!(wallet.len(), 1);
    assert_eq!(generation, 41);
}

#[test]
fn expiry_generation_outcome_is_stable_when_replayed() {
    let card = concrete_card(CardId::new());
    let card_id = card.card_id();
    let mut wallet = wallet_cards([(card_id, card)]);
    let mut generation = 7;

    assert!(remove_wallet_card(&mut wallet, &mut generation, card_id).unwrap());
    let recorded_expiry_generation = generation;

    assert!(!remove_wallet_card(&mut wallet, &mut generation, card_id).unwrap());
    adopt_recorded_wallet_generation(&mut generation, recorded_expiry_generation).unwrap();

    assert!(wallet.is_empty());
    assert_eq!(generation, 8);
}

#[test]
fn expiry_boundary_is_inclusive_and_preserves_later_cards() {
    let boundary = DateTime::from_timestamp(1_700_000_000, 123).unwrap();
    let elapsed = concrete_card_expiring_at(
        CardId::new(),
        Some(boundary - chrono::Duration::nanoseconds(1)),
    );
    let at_boundary = concrete_card_expiring_at(CardId::new(), Some(boundary));
    let later = concrete_card_expiring_at(
        CardId::new(),
        Some(boundary + chrono::Duration::nanoseconds(1)),
    );
    let indefinite = concrete_card(CardId::new());
    let wallet = wallet_cards([
        (elapsed.card_id(), elapsed.clone()),
        (at_boundary.card_id(), at_boundary.clone()),
        (later.card_id(), later),
        (indefinite.card_id(), indefinite),
    ]);

    assert_eq!(
        expired_wallet_card_ids_at(wallet.values().map(Arc::as_ref), boundary)
            .into_iter()
            .collect::<HashSet<_>>(),
        HashSet::from([elapsed.card_id(), at_boundary.card_id()])
    );
}

#[test]
fn recorded_expiry_events_replay_identically_with_an_earlier_clock() {
    let boundary = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let replay_clock = boundary - chrono::Duration::hours(1);
    let elapsed = concrete_card_expiring_at(
        CardId::new(),
        Some(boundary - chrono::Duration::nanoseconds(1)),
    );
    let at_boundary = concrete_card_expiring_at(CardId::new(), Some(boundary));
    let later =
        concrete_card_expiring_at(CardId::new(), Some(boundary + chrono::Duration::hours(1)));
    let original_wallet = wallet_cards([
        (elapsed.card_id(), elapsed),
        (at_boundary.card_id(), at_boundary),
        (later.card_id(), later),
    ]);

    let mut live_wallet = original_wallet.clone();
    let mut live_generation = 7;
    let mut recorded_expiries = Vec::new();
    for card_id in expired_wallet_card_ids_at(live_wallet.values().map(Arc::as_ref), boundary) {
        assert!(remove_wallet_card(&mut live_wallet, &mut live_generation, card_id).unwrap());
        recorded_expiries.push((card_id, live_generation));
    }

    let mut replayed_wallet = original_wallet;
    let mut replayed_generation = 7;
    assert!(
        expired_wallet_card_ids_at(replayed_wallet.values().map(Arc::as_ref), replay_clock)
            .is_empty()
    );
    for (card_id, recorded_generation) in recorded_expiries {
        assert!(
            remove_wallet_card(&mut replayed_wallet, &mut replayed_generation, card_id).unwrap()
        );
        adopt_recorded_wallet_generation(&mut replayed_generation, recorded_generation).unwrap();
    }

    assert_eq!(replayed_wallet, live_wallet);
    assert_eq!(replayed_generation, live_generation);
    assert_eq!(live_wallet.len(), 1);
    assert_eq!(live_generation, 9);
}

#[test]
fn conflicting_payload_for_existing_card_id_is_rejected_without_generation_change() {
    let card_id = CardId::new();
    let card = concrete_card(card_id);
    let mut conflicting_payload = card.clone();
    match &mut conflicting_payload {
        StoredCard::Concrete(card) => card.system_card = true,
        StoredCard::Polymorphic(_) => unreachable!(),
    }
    let mut wallet = wallet_cards([(card_id, card.clone())]);
    let mut generation = 10;

    assert!(add_wallet_card(&mut wallet, &mut generation, conflicting_payload).is_err());

    assert_eq!(generation, 10);
    assert_eq!(wallet.get(&card_id).map(Arc::as_ref), Some(&card));
}

#[test]
fn wallet_replacement_bumps_for_each_membership_delta() {
    let retained = concrete_card(CardId::new());
    let mut conflicting_retained = retained.clone();
    match &mut conflicting_retained {
        StoredCard::Concrete(card) => card.system_card = true,
        StoredCard::Polymorphic(_) => unreachable!(),
    }
    let removed = concrete_card(CardId::new());
    let added = concrete_card(CardId::new());
    let mut wallet = wallet_cards([
        (retained.card_id(), retained.clone()),
        (removed.card_id(), removed),
    ]);
    let replacement = wallet_cards([
        (conflicting_retained.card_id(), conflicting_retained),
        (added.card_id(), added),
    ]);
    let mut generation = 20;

    assert!(replace_wallet_cards(&mut wallet, &mut generation, replacement).unwrap());
    assert_eq!(generation, 22);
    assert_eq!(wallet.len(), 2);
    assert_eq!(
        wallet.get(&retained.card_id()).map(Arc::as_ref),
        Some(&retained)
    );
}

#[test]
fn wallet_replacement_is_atomic_when_generation_would_overflow() {
    let removed = concrete_card(CardId::new());
    let added = concrete_card(CardId::new());
    let mut wallet = wallet_cards([(removed.card_id(), removed)]);
    let original_wallet = wallet.clone();
    let replacement = wallet_cards([(added.card_id(), added)]);
    let mut generation = u64::MAX - 1;

    assert!(replace_wallet_cards(&mut wallet, &mut generation, replacement).is_err());
    assert_eq!((wallet, generation), (original_wallet, u64::MAX - 1));
}

#[test]
fn replay_adopts_recorded_wallet_generation() {
    let mut generation = 10;

    adopt_recorded_wallet_generation(&mut generation, 10).unwrap();
    assert_eq!(generation, 10);

    adopt_recorded_wallet_generation(&mut generation, 12).unwrap();
    assert_eq!(generation, 12);
}

#[test]
fn replay_rejects_decreasing_recorded_wallet_generation() {
    let mut generation = 10;

    assert!(adopt_recorded_wallet_generation(&mut generation, 9).is_err());
    assert_eq!(generation, 10);
}

#[test]
fn invocation_wallet_pin_establishes_base_before_ordered_mutations() {
    let wallet_id_hash = [0x42; 32];
    let base_card = concrete_card(CardId::new());
    let stale_card = concrete_card(CardId::new());
    let derived_card = concrete_card(CardId::new());
    let mut wallet = wallet_cards([
        (base_card.card_id(), base_card.clone()),
        (stale_card.card_id(), stale_card),
    ]);
    let mut generation = 1;

    assert!(
        apply_invocation_wallet_pin(
            &mut wallet,
            wallet_id_hash,
            &mut generation,
            InvocationWalletPin {
                wallet_token: WalletVersionToken {
                    wallet_id_hash,
                    generation: 1,
                },
                pinned_card_ids: vec![base_card.card_id()],
                scope_card_id: Some(CardId::new()),
            },
        )
        .unwrap()
    );
    assert_eq!(wallet.len(), 1);
    assert_eq!(
        wallet.get(&base_card.card_id()).map(Arc::as_ref),
        Some(&base_card)
    );
    assert_eq!(generation, 1);
    assert!(!wallet.contains_key(&derived_card.card_id()));

    assert!(add_wallet_card(&mut wallet, &mut generation, derived_card.clone()).unwrap());
    adopt_recorded_wallet_generation(&mut generation, 2).unwrap();
    assert_eq!(wallet.len(), 2);
    assert_eq!(
        wallet.get(&base_card.card_id()).map(Arc::as_ref),
        Some(&base_card)
    );
    assert_eq!(
        wallet.get(&derived_card.card_id()).map(Arc::as_ref),
        Some(&derived_card)
    );
    assert_eq!(generation, 2);
}

#[test]
fn invocation_wallet_pins_share_retained_membership_and_reject_atomically() {
    let original = wallet_cards((0..1024).map(|_| {
        let card = concrete_card(CardId::new());
        (card.card_id(), card)
    }));
    let hash = [0x42; 32];
    let ids = original.keys().copied().collect::<Vec<_>>();
    let pin = |ids, hash, generation| InvocationWalletPin {
        wallet_token: WalletVersionToken {
            wallet_id_hash: hash,
            generation,
        },
        pinned_card_ids: ids,
        scope_card_id: None,
    };
    let mut missing = ids[1..].to_vec();
    missing.push(CardId::new());
    missing.sort_unstable();
    for invalid in [
        pin(ids.clone(), [0x43; 32], 8),
        pin(ids.iter().rev().copied().collect(), hash, 8),
        pin(vec![ids[0], ids[0]], hash, 8),
        pin(ids.clone(), hash, 7),
        pin(missing, hash, 9),
    ] {
        let mut wallet = original.clone();
        let mut generation = 8;
        assert!(apply_invocation_wallet_pin(&mut wallet, hash, &mut generation, invalid).is_err());
        assert!(wallet.ptr_eq(&original));
        assert_eq!(generation, 8);
    }

    let mut wallet = original.clone();
    let mut generation = 8;
    assert!(
        !apply_invocation_wallet_pin(
            &mut wallet,
            hash,
            &mut generation,
            pin(ids.clone(), hash, 9)
        )
        .unwrap()
    );
    assert!(wallet.ptr_eq(&original));
    assert_eq!(generation, 9);
    let mut prefixes = vec![wallet.clone()];
    for end in (0..ids.len()).rev() {
        assert!(
            apply_invocation_wallet_pin(
                &mut wallet,
                hash,
                &mut generation,
                pin(ids[..end].to_vec(), hash, 9)
            )
            .unwrap()
        );
        assert_eq!(wallet.len(), end);
        prefixes.push(wallet.clone());
    }
    let entries = prefixes
        .iter()
        .flat_map(|wallet| {
            wallet
                .values()
                .map(|card| std::ptr::from_ref(card) as usize)
        })
        .collect::<HashSet<_>>();
    let payloads = prefixes
        .iter()
        .flat_map(|wallet| wallet.values().map(|card| Arc::as_ptr(card) as usize))
        .collect::<HashSet<_>>();
    assert_eq!(payloads.len(), ids.len());
    assert!(
        entries.len() < ids.len() * 128,
        "pins rebuilt {} membership entries",
        entries.len()
    );
    eprintln!(
        "wallet pins: prefixes={}, payloads={}, membership entries={}",
        prefixes.len(),
        payloads.len(),
        entries.len()
    );
}

fn polymorphic_card(card_id: CardId) -> StoredCard {
    StoredCard::Polymorphic(PolymorphicCard {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
    })
}

#[test]
fn transfer_start_removes_only_concrete_card_from_matching_source_agent() {
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "source-agent".to_string(),
    };
    let source_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: agent_id.clone(),
    });
    let concrete = concrete_card(CardId::new());
    let polymorphic = polymorphic_card(CardId::new());

    assert!(transfer_started_removes_source_membership(
        Some(&concrete),
        &source_holder,
        &agent_id,
    ));
    assert!(!transfer_started_removes_source_membership(
        Some(&polymorphic),
        &source_holder,
        &agent_id,
    ));
    let different_source = CardHolder::Agent(AgentCardHolder {
        agent_id: AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: agent_id.agent_id.clone(),
        },
    });
    assert!(!transfer_started_removes_source_membership(
        Some(&concrete),
        &different_source,
        &agent_id,
    ));
}

#[test]
fn pending_transfer_does_not_block_later_local_card_event() {
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "target-agent".to_string(),
        },
    });
    let transfer_card = concrete_card(CardId::new());
    let revoked_card_id = CardId::new();
    let pending_transfer = PendingCardEventRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::from_u64(1),
        entity_parent_start_index: None,
        event: QueuedCardEvent::transfer_started(Uuid::new_v4(), transfer_card, target_holder),
    };
    assert!(next_drainable_card_events(vec![pending_transfer.clone()]).is_empty());

    let pending_events = vec![
        pending_transfer,
        PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(2),
            entity_parent_start_index: None,
            event: QueuedCardEvent::revoke(revoked_card_id),
        },
    ];

    let next = next_drainable_card_events(pending_events);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].oplog_index, OplogIndex::from_u64(2));
    assert!(matches!(
        &next[0].event,
        QueuedCardEvent::Revoke(event) if event.card_id == revoked_card_id
    ));
}

#[test]
fn received_transfer_is_drained_as_a_target_wallet_event() {
    let source_card_id = CardId::new();
    let card = concrete_card(CardId::new());
    let receipt = PendingCardEventRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::from_u64(1),
        entity_parent_start_index: None,
        event: QueuedCardEvent::transfer_received(Uuid::new_v4(), source_card_id, card.clone()),
    };

    let next = next_drainable_card_events(vec![receipt.clone()]);

    assert_eq!(next, vec![receipt]);
    assert!(matches!(
        &next[0].event,
        QueuedCardEvent::TransferReceived(event)
            if event.source_card_id == source_card_id
                && event.card_id == card.card_id()
                && event.card.as_ref() == Some(&card)
    ));
}

#[test]
fn pending_revocations_are_batched_without_crossing_an_install() {
    let first = CardId::new();
    let second = CardId::new();
    let after_install = CardId::new();
    let install = concrete_card(CardId::new());
    let pending_events = vec![
        PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(1),
            entity_parent_start_index: None,
            event: QueuedCardEvent::revoke(first),
        },
        PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(2),
            entity_parent_start_index: None,
            event: QueuedCardEvent::revoke(second),
        },
        PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(3),
            entity_parent_start_index: None,
            event: QueuedCardEvent::install(install),
        },
        PendingCardEventRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(4),
            entity_parent_start_index: None,
            event: QueuedCardEvent::revoke(after_install),
        },
    ];

    let next = next_drainable_card_events(pending_events);
    assert_eq!(next.len(), 2);
    assert!(matches!(
        &next[0].event,
        QueuedCardEvent::Revoke(event) if event.card_id == first
    ));
    assert!(matches!(
        &next[1].event,
        QueuedCardEvent::Revoke(event) if event.card_id == second
    ));
}

#[test]
fn shard_assignment_recovery_restarts_idle_workers_with_pending_invocations() {
    let mut status = AgentStatusRecord {
        status: AgentStatus::Idle,
        ..AgentStatusRecord::default()
    };
    status.pending_invocations.push(PendingInvocationRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL,
        idempotency_key: None,
        manual_update_target_revision: Some(ComponentRevision::INITIAL),
    });

    assert!(should_restart_after_shard_assignment_change(&status));
}

#[test]
fn shard_assignment_recovery_restarts_idle_workers_with_pending_updates() {
    let mut status = AgentStatusRecord {
        status: AgentStatus::Idle,
        ..AgentStatusRecord::default()
    };
    status.pending_updates.push_back(PendingUpdateRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL,
        admission_index: OplogIndex::INITIAL,
        target_revision: ComponentRevision::INITIAL,
        kind: PendingUpdateKind::Automatic,
    });

    assert!(should_restart_after_shard_assignment_change(&status));
}

#[test]
fn shard_assignment_recovery_skips_suspended_workers_without_pending_work() {
    let status = AgentStatusRecord {
        status: AgentStatus::Suspended,
        ..AgentStatusRecord::default()
    };

    assert!(!should_restart_after_shard_assignment_change(&status));
}

#[test]
fn shard_assignment_recovery_preserves_explicit_interrupt_with_pending_work() {
    let mut status = AgentStatusRecord {
        status: AgentStatus::Interrupted,
        current_idempotency_key: Some(IdempotencyKey::fresh()),
        ..AgentStatusRecord::default()
    };
    assert!(!should_restart_after_shard_assignment_change(&status));

    status.pending_invocations.push(PendingInvocationRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL,
        idempotency_key: Some(IdempotencyKey::fresh()),
        manual_update_target_revision: None,
    });
    status.pending_updates.push_back(PendingUpdateRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL.next(),
        admission_index: OplogIndex::INITIAL.next(),
        target_revision: ComponentRevision::INITIAL,
        kind: PendingUpdateKind::Automatic,
    });
    assert!(!should_restart_after_shard_assignment_change(&status));

    status.status = AgentStatus::Suspended;
    assert!(should_restart_after_shard_assignment_change(&status));
}

fn recovered_agent() -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "recovered".to_string(),
        },
    )
}

#[test]
fn shard_assignment_recovery_restarts_a_worker_whose_status_was_computed() {
    let status = AgentStatusRecord {
        status: AgentStatus::Running,
        ..AgentStatusRecord::default()
    };

    let recovered = recovered_status(&recovered_agent(), Ok(Some(status))).unwrap();

    assert_eq!(
        recovered.map(|status| status.status),
        Some(AgentStatus::Running)
    );
}

/// No oplog means the index entry outlived the worker: nothing to resume, so it is skipped.
#[test]
fn shard_assignment_recovery_skips_a_worker_whose_oplog_is_gone() {
    assert!(matches!(
        recovered_status(&recovered_agent(), Ok(None)),
        Ok(None)
    ));
}

/// A status that could not be *computed* is not a worker that is gone. Skipping it would
/// acknowledge the assignment with the worker stopped and nothing recording that it should
/// run; failing the assignment is what gets it retried.
#[test]
fn shard_assignment_recovery_fails_on_a_status_it_cannot_compute() {
    let error = recovered_status(
        &recovered_agent(),
        Err("failed to load durable stream session payload: redis failover".into()),
    )
    .expect_err("an uncomputable status was skipped rather than failing the assignment");

    assert!(
        error.to_string().contains("cannot be resumed")
            && error.to_string().contains("redis failover"),
        "{error}"
    );
}

/// An agent whose oplog does not read is skipped and does not fail the assignment: no retry of
/// the assignment would read it, and failing it would keep every other agent from resuming.
#[test]
fn shard_assignment_recovery_skips_an_agent_whose_oplog_does_not_read() {
    let unreadable = crate::services::oplog::UnreadableOplog {
        agent_id: recovered_agent().agent_id(),
        error: crate::services::oplog::OplogReadError::Gap {
            start: OplogIndex::INITIAL,
            end: OplogIndex::from_u64(2),
        },
    };
    assert!(matches!(
        recovered_status(&recovered_agent(), Err(StatusError::Unreadable(unreadable))),
        Ok(None)
    ));
}

/// An oplog whose storage did not answer can be read by a retry, so it fails the assignment,
/// which is retried, instead of leaving the agent out of the recovery for good.
#[test]
fn shard_assignment_recovery_fails_on_an_oplog_that_is_unavailable() {
    let unavailable = crate::services::oplog::UnreadableOplog {
        agent_id: recovered_agent().agent_id(),
        error: crate::services::oplog::OplogReadError::source_unavailable(
            crate::services::oplog::OplogReadSource::Archive(1),
            "injected archive outage",
        ),
    };
    let error = recovered_status(
        &recovered_agent(),
        Err(StatusError::Unreadable(unavailable)),
    )
    .expect_err("an unavailable oplog was skipped rather than failing the assignment");
    assert!(
        error.to_string().contains("injected archive outage"),
        "{error}"
    );
}

/// An agent whose restart finds its oplog unreadable, for example while rebuilding its stream
/// session index, is skipped: the other agents of the scan are still resumed.
#[test]
fn shard_assignment_recovery_skips_a_restart_that_finds_the_oplog_failed() {
    let unreadable = WorkerExecutorError::oplog_failed(
        recovered_agent().agent_id(),
        "corrupt oplog data from archive layer 1",
    );
    assert!(recovered_restart::<()>(&recovered_agent(), Err(unreadable.clone())).is_ok());
    assert!(
        recovered_restart::<()>(
            &recovered_agent(),
            Err(WorkerExecutorError::failed_to_resume_worker(
                recovered_agent().agent_id(),
                unreadable,
            )),
        )
        .is_ok()
    );
}

/// A revoke that races recovery refuses the worker an epoch. That agent is skipped, so the rest
/// of the scan is still resumed. Any other restart failure still fails the assignment.
#[test]
fn shard_assignment_recovery_skips_a_worker_whose_shard_left_the_assignment() {
    assert!(recovered_restart(&recovered_agent(), Ok(())).is_ok());
    assert!(
        recovered_restart::<()>(
            &recovered_agent(),
            Err(WorkerExecutorError::ShardingNotReady)
        )
        .is_ok()
    );
    let agent = recovered_agent();
    assert!(
        recovered_restart::<()>(
            &agent,
            Err(WorkerExecutorError::oplog_fenced(
                agent.agent_id.clone(),
                2,
                Some(3)
            ))
        )
        .is_ok()
    );

    let error = recovered_restart::<()>(
        &recovered_agent(),
        Err(WorkerExecutorError::runtime("instance failed to start")),
    )
    .expect_err("a restart failure other than a lost shard was skipped");
    assert!(
        error.to_string().contains("failed to restart")
            && error.to_string().contains("instance failed to start"),
        "{error}"
    );
}

struct PendingRequestBody;

impl http_body::Body for PendingRequestBody {
    type Data = Bytes;
    type Error = ErrorCode;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Pending
    }

    fn is_end_stream(&self) -> bool {
        false
    }
}

async fn poll_p3_send_request_io_with_open_body(
    connection_pool: Option<HttpConnectionPool>,
) -> Poll<Result<(), ErrorCode>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (server_done_tx, server_done_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        loop {
            let mut buf = [0; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert_ne!(n, 0, "client closed before sending request headers");
            received.extend_from_slice(&buf[..n]);
            if received.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        let _ = server_done_rx.await;
    });

    let mut hooks = DurableHttpHooks {
        connection_pool,
        is_replay: Arc::new(AtomicBool::new(false)),
    };
    let request = ::http::Request::post(format!("http://{addr}/upload"))
        .body(PendingRequestBody.boxed_unsync())
        .unwrap();

    let send = wasmtime_wasi_http::p3::WasiHttpHooks::send_request(
        &mut hooks,
        request,
        None,
        Box::new(async { Ok(()) }),
    );
    let send = Box::into_pin(send);
    let (_response, io) = tokio::time::timeout(Duration::from_secs(5), send)
        .await
        .expect("server responded before request body completed")
        .expect("send_request should return response headers successfully");
    let mut io = Box::into_pin(io);

    let poll = io.as_mut().poll(&mut Context::from_waker(Waker::noop()));

    let _ = server_done_tx.send(());
    server.await.unwrap();

    poll
}

#[test]
#[timeout(120000)]
async fn p3_pooled_send_request_io_future_waits_for_open_request_body_transmission() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    assert!(
        matches!(
            poll_p3_send_request_io_with_open_body(None).await,
            Poll::Pending
        ),
        "the default p3 request transmission future should remain pending while the request body is still open"
    );

    let pool = HttpConnectionPool::new(wasmtime_wasi_http::p2::HttpConnectionPoolConfig {
        max_idle_per_host: 1,
        idle_timeout: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(5),
        max_connections_per_host: 1,
        max_total_connections: 1,
        max_host_entries: 16,
    });

    assert!(
        matches!(
            poll_p3_send_request_io_with_open_body(Some(pool)).await,
            Poll::Pending
        ),
        "the p3 request transmission future must not resolve while the request body is still open"
    );
}
