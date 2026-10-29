use super::{
    CommandOutcome, ConcurrentAgentPermitState, InvocationLoop, MonthlyResourceAdmission,
    OwnerFailureWinner, PeriodicFailure, PeriodicSnapshotAction, PermitAcquisition,
    ResidentAgentOwnership, ResidentWakeup, TrapType, acquire_permit_or_interrupt,
    apply_delayed_retry_commands_after_unload, catch_invocation_loop_panic,
    coalesce_delayed_retry_prefix, coalesce_filesystem_limit_update,
    failed_agent_invocation_outcome, finish_filesystem_limit_unload,
    infrastructure_failure_outcome, measured_capture, monthly_resource_admission,
    monthly_resource_admission_for_capacity, periodic_failure_outcome,
    periodic_snapshot_failure_outcome, publish_unload_outcome, run_invocation_loop_task,
    should_terminally_interrupt_ephemeral, snapshot_action_at, snapshot_baseline_timestamp,
    spawn_module_owned_unload, successful_agent_invocation_outcome,
    unload_resident_agent_ownership, wait_for_resident_wakeup, with_end,
};
use crate::sandbox_filesystem::ScriptedSandboxFilesystem;
use crate::services::active_agents::stop_loaded_idle_if_eligible;
use crate::services::agent_filesystem::{
    AccessError, FilesystemStorageError, FlushLevel, Follow, OpenNode, PathTarget, Target,
    attributes, billing_metered_resident_with_open_node_for_unload_test, close, delete,
    filesystem_activity, flush, metered_resident_with_open_node_for_unload_test,
    resident_for_unload_test, seal,
};
use crate::services::resource_limits::{
    AtomicResourceEntry, MonthlyResourceCapacityError, MonthlyResourceExhaustion,
};

#[test]
fn unavailable_monthly_policy_is_not_quota_suspension_or_ephemeral_resource_exhaustion() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        assert_eq!(
            monthly_resource_admission_for_capacity(
                Err(MonthlyResourceCapacityError::Unavailable),
                mode,
            ),
            MonthlyResourceAdmission::Unavailable
        );
    }
    let error = MonthlyResourceCapacityError::unavailable_error();
    assert!(matches!(error, WorkerExecutorError::Runtime { .. }));
}
use crate::services::resource_usage_metering::close_window;
use crate::worker::invocation::InvokeResult;
use crate::worker::{
    CreateWorkerInstanceError, EvictionClass, FilesystemPressureEligibility, FinalWorkerState,
    PendingLiveInvocationDisposition, RetryDecision, RunningAgent, StoppingWorker, UnloadReason,
    WorkerCommand, WorkerInstance, complete_stopping_worker,
};
use crate::workerctx::default::Context;
use golem_common::model::AgentInvocationKind;
use golem_common::model::agent::{AgentMode, OwnerKind};
use golem_common::model::oplog::AgentError;
use golem_common::model::{OplogIndex, Timestamp};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use test_r::{test, timeout};

#[test]
fn ephemeral_outer_loop_restarts_become_terminal_interruptions() {
    for decision in [
        RetryDecision::Immediate,
        RetryDecision::Delayed(Duration::from_millis(37)),
        RetryDecision::ReacquirePermits,
    ] {
        assert!(should_terminally_interrupt_ephemeral(
            OwnerKind::ComponentAgent,
            AgentMode::Ephemeral,
            Some(&decision),
        ));
        assert!(!should_terminally_interrupt_ephemeral(
            OwnerKind::ComponentAgent,
            AgentMode::Durable,
            Some(&decision),
        ));
    }
    let stop = RetryDecision::TryStop(Timestamp::from(1234));
    assert!(!should_terminally_interrupt_ephemeral(
        OwnerKind::ComponentAgent,
        AgentMode::Ephemeral,
        Some(&stop),
    ));
    assert!(should_terminally_interrupt_ephemeral(
        OwnerKind::EphemeralExternalTool,
        AgentMode::Ephemeral,
        Some(&stop),
    ));
    for decision in [None, Some(&RetryDecision::None)] {
        assert!(!should_terminally_interrupt_ephemeral(
            OwnerKind::ComponentAgent,
            AgentMode::Ephemeral,
            decision,
        ));
    }
}

#[test]
async fn published_idle_stop_does_not_poll_a_ready_permit_acquisition() {
    use super::InnerInvocationLoopResult;
    use crate::worker::interrupt::Interrupts;
    use crate::worker::{InterruptEstablishment, PendingWorkerInterrupt};

    for kind in [
        InterruptKind::Interrupt(Timestamp::from(101_u64)),
        InterruptKind::Suspend(Timestamp::from(102_u64)),
        InterruptKind::Restart,
        InterruptKind::Jump,
        InterruptKind::ShardLost,
    ] {
        let interrupts = Interrupts::default();
        let request = PendingWorkerInterrupt {
            kind,
            reacquire_permits: false,
            unload_request: super::UnloadRequest::ordinary(UnloadReason::from_interrupt(kind)),
            establishment: InterruptEstablishment::ready(),
        };
        {
            let mut guard = interrupts.lock().await;
            assert!(guard.queue(request.clone()));
        }
        let (_sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let acquisitions = AtomicUsize::new(0);
        let result = acquire_permit_or_interrupt(
            &interrupts,
            &mut receiver,
            || async { acquisitions.fetch_add(1, Ordering::SeqCst) },
            |_| panic!("no command was queued"),
        )
        .await
        .unwrap();
        let PermitAcquisition::Interrupted(accepted) = result else {
            panic!("published stop must precede even an immediately ready permit");
        };
        assert_eq!(accepted.kind, kind);
        assert_eq!(
            accepted.unload_request.reason,
            request.unload_request.reason
        );
        let result = InnerInvocationLoopResult::default().with_unloaded_interrupt(Some(accepted));
        let carried = result
            .unloaded_interrupt
            .expect("unwritten lifecycle demand");
        assert_eq!(carried.kind, request.kind);
        assert_eq!(carried.reacquire_permits, request.reacquire_permits);
        assert_eq!(carried.unload_request.reason, request.unload_request.reason);
        assert_eq!(
            carried.unload_request.deadline,
            request.unload_request.deadline
        );
        assert_eq!(result.retry_decision, Some(request.retry_decision()));
        assert_eq!(
            result.final_interrupt,
            (!matches!(kind, InterruptKind::Restart)).then_some(kind)
        );
        assert!(!result.cleanup_ephemeral_worker);
        assert!(result.recovery_failure.is_none());
        assert!(result.cleanup_failure.is_none());
        assert_eq!(acquisitions.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn active_invocation_failure_is_not_an_unloaded_interrupt_obligation() {
    use super::InnerInvocationLoopResult;

    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let outcome = failed_agent_invocation_outcome(mode, RetryDecision::None, None);
        let (decision, cleanup_ephemeral_worker) = match outcome {
            CommandOutcome::BreakInnerLoop(decision) => (decision, false),
            CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(decision, None) => {
                (decision, true)
            }
            _ => panic!("terminal invocation failure must end the resident loop"),
        };
        let error = WorkerExecutorError::runtime("selected independent invocation failure");
        let result = InnerInvocationLoopResult {
            retry_decision: Some(decision),
            recovery_failure: Some(error.clone()),
            cleanup_ephemeral_worker,
            ..Default::default()
        }
        .with_unloaded_interrupt(None);
        assert!(result.unloaded_interrupt.is_none());
        assert!(result.final_interrupt.is_none());
        assert!(result.unload_request.is_none());
        assert_eq!(result.retry_decision, Some(RetryDecision::None));
        assert_eq!(
            result.recovery_failure.unwrap().to_string(),
            error.to_string()
        );
        assert_eq!(
            result.cleanup_ephemeral_worker,
            mode == AgentMode::Ephemeral
        );
    }
}

#[test]
async fn blocked_idle_permit_wait_joins_establishment_and_retains_commands() {
    use crate::worker::interrupt::Interrupts;
    use crate::worker::{InterruptEstablishment, PendingWorkerInterrupt};

    struct PendingAcquisition(Arc<AtomicUsize>);
    impl Drop for PendingAcquisition {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    for kind in [
        InterruptKind::Interrupt(Timestamp::from(101_u64)),
        InterruptKind::Suspend(Timestamp::from(102_u64)),
        InterruptKind::Restart,
        InterruptKind::Jump,
    ] {
        let interrupts = Arc::new(Interrupts::default());
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let held = semaphore.clone().acquire_owned().await.unwrap();
        let acquisitions = AtomicUsize::new(0);
        let cancelled = Arc::new(AtomicUsize::new(0));
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut deferred = VecDeque::new();
        let mut wait = Box::pin(acquire_permit_or_interrupt(
            &interrupts,
            &mut receiver,
            || {
                let semaphore = semaphore.clone();
                let cancelled = cancelled.clone();
                let acquisitions = &acquisitions;
                async move {
                    acquisitions.fetch_add(1, Ordering::SeqCst);
                    let _pending = PendingAcquisition(cancelled);
                    semaphore.acquire_owned().await.unwrap()
                }
            },
            |command| InvocationLoop::<Context>::defer_wakeup(&mut deferred, command),
        ));
        assert!(futures::poll!(wait.as_mut()).is_pending());
        assert_eq!(acquisitions.load(Ordering::SeqCst), 1);
        assert_eq!(cancelled.load(Ordering::SeqCst), 0);

        let (update, updated) = futures::channel::oneshot::channel();
        sender
            .send(WorkerCommand::UpdateFilesystemLimit {
                allocated_bytes: 4096,
                sender: update,
            })
            .unwrap();
        assert!(futures::poll!(wait.as_mut()).is_pending());
        assert_eq!(acquisitions.load(Ordering::SeqCst), 1);

        let establishment = Arc::new(InterruptEstablishment::new(InterruptEstablishment::ready()));
        {
            let mut guard = interrupts.lock().await;
            assert!(guard.queue(PendingWorkerInterrupt {
                kind,
                reacquire_permits: false,
                unload_request: super::UnloadRequest::ordinary(UnloadReason::from_interrupt(kind)),
                establishment: establishment.clone(),
            }));
        }
        if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
            assert!(futures::poll!(wait.as_mut()).is_pending());
            assert_eq!(cancelled.load(Ordering::SeqCst), 1);
            assert!(interrupts.try_lock().is_some());
        }
        sender.send(WorkerCommand::WorkAvailable).unwrap();
        assert!(futures::poll!(wait.as_mut()).is_pending());
        assert_eq!(cancelled.load(Ordering::SeqCst), 1);
        establishment.complete();
        let PermitAcquisition::Interrupted(accepted) = wait.await.unwrap() else {
            panic!("accepted stop must cancel the blocked permit wait");
        };
        assert_eq!(accepted.kind, kind);
        assert!(Arc::ptr_eq(&accepted.establishment, &establishment));
        assert_eq!(acquisitions.load(Ordering::SeqCst), 1);
        assert_eq!(cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(semaphore.available_permits(), 0);
        let Some(WorkerCommand::UpdateFilesystemLimit {
            allocated_bytes,
            sender,
        }) = deferred.pop_front()
        else {
            panic!("the unchanged filesystem-limit target must retain its reply");
        };
        assert_eq!(allocated_bytes, 4096);
        sender.send(Ok(())).unwrap();
        updated.await.unwrap().unwrap();
        drop(held);
        assert_eq!(semaphore.available_permits(), 1);
    }
}

#[test]
async fn a_measured_capture_gives_the_outcome_of_a_capture_and_the_error_of_a_failed_one() {
    let label = |_: &u8| "captured";
    let recorded = Mutex::new(Vec::new());
    let record = |outcome, _| recorded.lock().unwrap().push(outcome);
    let captured = measured_capture(|| async { Ok(7) }, label, record).await;
    let failed = measured_capture(
        || async { Err(crate::services::agent_filesystem::CaptureError::Busy) },
        label,
        record,
    )
    .await;

    assert_eq!(
        (captured.ok(), failed.map_err(|error| error.to_string())),
        (
            Some(7),
            Err("agent filesystem has an open call".to_string())
        )
    );
    assert_eq!(recorded.into_inner().unwrap(), ["captured", "busy"]);
}

#[test]
fn a_periodic_record_that_did_not_reach_the_oplog_skips_stops_or_retries() {
    let fence = crate::services::oplog::OplogFence {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId::new(),
            agent_id: "fenced".to_string(),
        },
        expected_epoch: golem_common::model::ShardEpoch(2),
        actual_epoch: Some(golem_common::model::ShardEpoch(3)),
    };
    let archive_write = crate::services::oplog::OplogError::ArchiveWrite(
        crate::services::oplog::ArchiveWriteFailure {
            agent_id: golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "archive-write".to_string(),
            },
            level: 0,
            details: "failed".to_string(),
        },
    );
    assert_eq!(
        [
            PeriodicFailure::Entry("no payload".to_string()),
            PeriodicFailure::Write(crate::services::oplog::OplogError::Fenced(fence)),
            PeriodicFailure::Write(crate::services::oplog::OplogError::Payload(
                "failed".to_string()
            )),
            PeriodicFailure::Write(archive_write.clone()),
        ]
        .each_ref()
        .map(periodic_failure_outcome),
        [
            CommandOutcome::Continue,
            CommandOutcome::BreakInnerLoop(RetryDecision::None),
            CommandOutcome::BreakInnerLoop(RetryDecision::Immediate),
            CommandOutcome::BreakOuterLoop(Some(WorkerExecutorError::from(archive_write))),
        ]
    );
}

#[test]
fn filesystem_turns_allow_pending_invocations_and_updates_to_progress() {
    use super::{
        AgentStatusRecord, CanonicalFilePath, FileByteSelection, IdempotencyKey,
        PendingInvocationRef, QueuedWorkerInvocation, SelectedWork,
    };
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::{PendingUpdateKind, PendingUpdateRef};

    for update in [false, true] {
        let pending = PendingInvocationRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(4),
            idempotency_key: Some(IdempotencyKey::new("method".into())),
            manual_update_target_revision: None,
        };
        let mut status = AgentStatusRecord {
            pending_invocations: vec![pending],
            ..Default::default()
        };
        if update {
            status.pending_updates.push_back(PendingUpdateRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(5),
                admission_index: OplogIndex::from_u64(5),
                target_revision: ComponentRevision::INITIAL,
                kind: PendingUpdateKind::Automatic,
            });
        }
        let status = Arc::new(status);
        let mut queue = VecDeque::new();
        let mut receivers = Vec::new();
        let mut turn_used = false;
        for turn in 0..20 {
            // Keep the resident queue nonempty, including while ordinary work is selected.
            let (sender, receiver) = tokio::sync::oneshot::channel();
            receivers.push(receiver);
            queue.push_back(QueuedWorkerInvocation::ReadFile {
                path: CanonicalFilePath::from_abs_str("/file").unwrap(),
                selection: FileByteSelection::Full,
                sender,
            });
            let selected = SelectedWork::next(&mut queue, status.clone(), None, &mut turn_used);
            if turn % 2 == 0 {
                assert!(matches!(
                    selected,
                    SelectedWork::Resident(QueuedWorkerInvocation::ReadFile { .. })
                ));
            } else if update {
                assert!(matches!(selected, SelectedWork::ApplyPendingUpdate));
            } else {
                assert!(
                    matches!(selected, SelectedWork::Durable(pending) if pending.oplog_index == OplogIndex::from_u64(4))
                );
            }
        }
    }
}

#[test]
fn filesystem_turns_preserve_initialization_and_control_priority() {
    use super::{
        AgentStatusRecord, CanonicalFilePath, IdempotencyKey, PendingInvocationRef,
        QueuedWorkerInvocation, SelectedWork,
    };
    let constructor = IdempotencyKey::new("init-agent".into());
    let status = Arc::new(AgentStatusRecord {
        pending_invocations: vec![PendingInvocationRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(2),
            idempotency_key: Some(constructor.clone()),
            manual_update_target_revision: None,
        }],
        ..Default::default()
    });
    let (sender, _receiver) = futures::channel::oneshot::channel();
    let mut queue = VecDeque::from([QueuedWorkerInvocation::GetFileSystemNode {
        path: CanonicalFilePath::from_abs_str("/").unwrap(),
        sender,
    }]);
    let mut turn_used = false;
    assert!(matches!(
        SelectedWork::next(
            &mut queue,
            status.clone(),
            Some(&constructor),
            &mut turn_used
        ),
        SelectedWork::Durable(_)
    ));
    assert_eq!(queue.len(), 1);
    // Once initialized, listings also consume the filesystem turn.
    assert!(matches!(
        SelectedWork::next(&mut queue, status.clone(), None, &mut turn_used),
        SelectedWork::Resident(_)
    ));
    assert!(turn_used);
    queue.push_front(QueuedWorkerInvocation::SaveSnapshot);
    assert!(matches!(
        SelectedWork::next(&mut queue, status, None, &mut turn_used),
        SelectedWork::Resident(QueuedWorkerInvocation::SaveSnapshot)
    ));
    assert!(turn_used);
    assert!(matches!(
        SelectedWork::next(
            &mut queue,
            Arc::new(AgentStatusRecord::default()),
            None,
            &mut turn_used
        ),
        SelectedWork::Idle(_)
    ));
    assert!(!turn_used);
}

#[test]
fn queued_inspections_survive_guest_suspend_but_not_quota_or_terminal_stops() {
    assert_eq!(
        super::resident_work_disposition(UnloadReason::Suspend),
        PendingLiveInvocationDisposition::Preserve
    );
    for reason in [
        UnloadReason::MemoryLimit,
        UnloadReason::FilesystemLimit,
        UnloadReason::Failure,
        UnloadReason::Interrupt,
        UnloadReason::Deleting,
    ] {
        assert_eq!(
            super::resident_work_disposition(reason),
            PendingLiveInvocationDisposition::Fail
        );
    }
}

struct TestPermit {
    held: Arc<AtomicBool>,
    drops: Arc<AtomicUsize>,
    held_while_dropping: Arc<AtomicBool>,
}

struct TestStoreOwner {
    node: Option<OpenNode>,
    dropped: Arc<AtomicBool>,
}

#[test]
fn monthly_resource_admission_blocks_guest_work_by_agent_mode() {
    assert_eq!(
        monthly_resource_admission_for_capacity(Ok(()), AgentMode::Durable),
        MonthlyResourceAdmission::Admit
    );
    assert_eq!(
        monthly_resource_admission_for_capacity(
            Err(MonthlyResourceExhaustion::Compute.into()),
            AgentMode::Durable,
        ),
        MonthlyResourceAdmission::Suspend
    );
    assert_eq!(
        monthly_resource_admission_for_capacity(
            Err(MonthlyResourceExhaustion::Compute.into()),
            AgentMode::Ephemeral
        ),
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute)
    );
    assert_eq!(
        monthly_resource_admission_for_capacity(
            Err(MonthlyResourceExhaustion::Memory.into()),
            AgentMode::Durable,
        ),
        MonthlyResourceAdmission::Suspend
    );
    assert_eq!(
        monthly_resource_admission_for_capacity(
            Err(MonthlyResourceExhaustion::Memory.into()),
            AgentMode::Ephemeral
        ),
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Memory)
    );
    assert_eq!(
        monthly_resource_admission_for_capacity(
            Err(MonthlyResourceExhaustion::DurableStorage.into()),
            AgentMode::Durable
        ),
        MonthlyResourceAdmission::Suspend
    );
    assert_eq!(
        monthly_resource_admission_for_capacity(
            Err(MonthlyResourceExhaustion::EphemeralStorage.into()),
            AgentMode::Ephemeral
        ),
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::EphemeralStorage)
    );
}

#[test]
fn storage_gate_composes_with_invocation_admission_and_isolates_agent_modes() {
    let period = golem_common::model::account_usage::AccountUsagePeriod::current();
    let durable_exhausted = AtomicResourceEntry::new_with_monthly_policy(
        golem_service_base::model::MonthlyResourcePolicy {
            period,
            mode: golem_common::model::account_usage::MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 0,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 1,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        usize::MAX,
        usize::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
    );
    assert_eq!(
        monthly_resource_admission(&durable_exhausted, AgentMode::Durable),
        MonthlyResourceAdmission::Suspend
    );
    assert_eq!(
        monthly_resource_admission(&durable_exhausted, AgentMode::Ephemeral),
        MonthlyResourceAdmission::Admit
    );

    let ephemeral_exhausted = AtomicResourceEntry::new_with_monthly_policy(
        golem_service_base::model::MonthlyResourcePolicy {
            period,
            mode: golem_common::model::account_usage::MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 1,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 0,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        usize::MAX,
        usize::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
    );
    assert_eq!(
        monthly_resource_admission(&ephemeral_exhausted, AgentMode::Ephemeral),
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::EphemeralStorage)
    );
    assert_eq!(
        monthly_resource_admission(&ephemeral_exhausted, AgentMode::Durable),
        MonthlyResourceAdmission::Admit
    );
}

#[test]
#[timeout("5s")]
async fn loaded_idle_wait_wakes_for_a_terminal_filesystem_failure() {
    let (filesystem, control, window, generation_handle, node) =
        metered_resident_with_open_node_for_unload_test().await;
    let activity = filesystem_activity(&filesystem);
    let (_commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let waiting =
        tokio::spawn(async move { wait_for_resident_wakeup(&mut receiver, &activity).await });
    control.push_flush(Err(FilesystemStorageError::io(
        "detached terminal flush",
        None,
        std::io::ErrorKind::Other.into(),
    )));

    assert!(matches!(
        flush(&generation_handle, &node, FlushLevel::Data)
            .unwrap()
            .await,
        Err(crate::services::agent_filesystem::Error::RuntimeInvalidated)
    ));
    assert!(matches!(
        waiting.await.unwrap(),
        ResidentWakeup::FilesystemTerminalFailure
    ));

    control.push_observe_allocation(Err(FilesystemStorageError::verification(
        "observe unsupported allocation during idle wakeup cleanup",
        None,
    )));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_close(Ok(()));
    close(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

impl Drop for TestStoreOwner {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
        drop(self.node.take());
    }
}

impl Drop for TestPermit {
    fn drop(&mut self) {
        self.held_while_dropping
            .store(self.held.load(Ordering::Acquire), Ordering::Release);
        self.drops.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
async fn idle_filesystem_limit_downgrade_upgrade_commands_coalesce_without_lost_work() {
    let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let (downgrade_sender, downgrade_result) = futures::channel::oneshot::channel();
    let (upgrade_sender, upgrade_result) = futures::channel::oneshot::channel();
    commands.send(WorkerCommand::WorkAvailable).unwrap();
    commands
        .send(WorkerCommand::UpdateFilesystemLimit {
            allocated_bytes: 1024 * 1024,
            sender: upgrade_sender,
        })
        .unwrap();
    let mut deferred_wakeups = VecDeque::new();

    let update = coalesce_filesystem_limit_update(
        4096,
        downgrade_sender,
        &mut receiver,
        &mut deferred_wakeups,
    );

    assert_eq!(update.allocated_bytes, 1024 * 1024);
    assert_eq!(update.senders.len(), 2);
    for sender in update.senders {
        let _ = sender.send(Ok(()));
    }
    assert!(downgrade_result.await.unwrap().is_ok());
    assert!(upgrade_result.await.unwrap().is_ok());
    assert!(receiver.try_recv().is_err());
    assert_eq!(deferred_wakeups.len(), 1);
    assert!(matches!(
        deferred_wakeups.pop_front(),
        Some(WorkerCommand::WorkAvailable)
    ));
}

#[test]
async fn delayed_retry_coalesces_only_the_captured_work_prefix() {
    let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let (limit_sender, limit_result) = futures::channel::oneshot::channel();
    commands.send(WorkerCommand::WorkAvailable).unwrap();
    commands.send(WorkerCommand::WorkAvailable).unwrap();
    commands.send(WorkerCommand::InternalStatusChanged).unwrap();
    commands
        .send(WorkerCommand::UpdateFilesystemLimit {
            allocated_bytes: 4096,
            sender: limit_sender,
        })
        .unwrap();
    let prefix_len = receiver.len();

    commands.send(WorkerCommand::WorkAvailable).unwrap();
    let mut deferred_wakeups = VecDeque::new();
    let captured =
        coalesce_delayed_retry_prefix::<Context>(&mut receiver, &mut deferred_wakeups, prefix_len);

    assert_eq!(captured.filesystem_limit_updates.len(), 1);
    assert_eq!(deferred_wakeups.len(), 1);
    assert!(matches!(
        deferred_wakeups.pop_front(),
        Some(WorkerCommand::WorkAvailable)
    ));
    assert!(matches!(
        receiver.try_recv(),
        Ok(WorkerCommand::WorkAvailable)
    ));

    apply_delayed_retry_commands_after_unload(captured);
    assert!(deferred_wakeups.is_empty());
    assert!(limit_result.await.unwrap().is_ok());
}

#[test]
async fn idle_over_limit_update_publishes_suspend_only_after_cleanup() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let cleanup_events = Arc::clone(&events);
    let suspend_events = Arc::clone(&events);
    let (release_cleanup, cleanup_released) = tokio::sync::oneshot::channel();

    let completion = tokio::spawn(finish_filesystem_limit_unload(
        true,
        async move {
            let _ = cleanup_released.await;
            cleanup_events.lock().unwrap().push("cleanup");
            None
        },
        move || async move {
            suspend_events.lock().unwrap().push("suspend");
        },
    ));
    tokio::task::yield_now().await;
    assert!(events.lock().unwrap().is_empty());

    release_cleanup.send(()).unwrap();
    assert!(completion.await.unwrap().is_none());
    assert_eq!(*events.lock().unwrap(), ["cleanup", "suspend"]);
}

fn complete_stopping(final_state: FinalWorkerState) -> WorkerInstance {
    let (instance, notify) = complete_stopping_worker(
        StoppingWorker {
            notify: golem_common::one_shot::OneShotEvent::new(),
            final_state: FinalWorkerState::Unloaded {
                startup_failure: None,
            },
            pending_live_invocations: PendingLiveInvocationDisposition::Preserve,
        },
        final_state,
    );
    notify.set();
    instance
}

#[test]
async fn dropped_unload_observer_does_not_cancel_module_owned_cleanup() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let completed = Arc::new(AtomicBool::new(false));
    let completed_in_task = Arc::clone(&completed);

    let observer = spawn_module_owned_unload(async move {
        let _ = started_tx.send(());
        let _ = release_rx.await;
        completed_in_task.store(true, Ordering::Release);
        None
    });
    started_rx.await.unwrap();
    drop(observer);
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        while !completed.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
async fn unload_task_panic_is_reported_by_the_observer() {
    let observer = spawn_module_owned_unload(async move {
        panic!("injected unload panic");
    });
    let cleanup = observer.cleanup.clone();
    let error = observer.await.expect("panic must become a cleanup error");

    assert!(error.to_string().contains("unload task panicked"));
    assert_eq!(cleanup.wait().await.unwrap_err(), error);
    assert_eq!(cleanup.retry().wait().await.unwrap_err(), error);
}

#[test]
async fn unload_observer_completes_only_after_module_owned_cleanup() {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let observer = spawn_module_owned_unload(async move {
        let _ = release_rx.await;
        None
    });
    tokio::pin!(observer);

    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut observer)
            .await
            .is_err()
    );
    release_tx.send(()).unwrap();
    assert!(observer.await.is_none());
}

#[test]
async fn invocation_loop_panic_is_returned_to_owned_cleanup_boundary() {
    let error = catch_invocation_loop_panic(async {
        panic!("injected invocation-loop panic");
    })
    .await
    .expect_err("panic must become a lifecycle error");

    assert!(
        error
            .to_string()
            .contains("invocation loop panicked: injected invocation-loop panic")
    );
}

#[test]
async fn invocation_loop_task_handles_panic_before_resident_creation() {
    let handled = Arc::new(Mutex::new(None));
    let handled_by_task = Arc::clone(&handled);

    run_invocation_loop_task(
        async { panic!("injected startup panic") },
        move |error| async move {
            *handled_by_task.lock().unwrap() = Some(error);
        },
    )
    .await;

    assert!(
        handled
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|error| error.to_string().contains("injected startup panic"))
    );
}

#[test]
#[timeout("5s")]
async fn invocation_loop_panic_with_resident_owner_uses_filesystem_drop_cleanup() {
    let (filesystem, control) = resident_for_unload_test().await;
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let handled = Arc::new(AtomicBool::new(false));
    let handled_by_task = Arc::clone(&handled);

    run_invocation_loop_task(
        async move {
            let _filesystem = filesystem;
            panic!("injected resident panic");
        },
        move |_error| async move {
            handled_by_task.store(true, Ordering::Release);
        },
    )
    .await;

    deletion.wait_started().await;
    assert!(handled.load(Ordering::Acquire));
    deletion.release();
}

#[test]
#[timeout("5s")]
async fn unload_deadline_during_drain_keeps_final_sample_and_deletion_owned() {
    let (filesystem, control, window, _generation_handle, node) =
        billing_metered_resident_with_open_node_for_unload_test().await;
    control.push_close(Ok(()));
    let close = control.block("close");
    control.push_observe_allocation(Ok(crate::storage_volume::FilesystemAllocation {
        allocated_bytes: 100,
        filesystem_objects: 1,
    }));
    let sample = control.block("observe_allocation");
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let settled = window.settlement_observer();
    let held = Arc::new(AtomicBool::new(false));
    let mut permits = ConcurrentAgentPermitState::new(None, held.clone());
    permits.install_window(super::ExecutionWindow::unmonitored(window));
    let before_unload = control.calls().len();
    let observer = unload_resident_agent_ownership(
        ResidentAgentOwnership {
            runtime: (),
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now(),
        &mut permits,
        &Arc::new(Mutex::new(None)),
        None,
    );
    let cleanup = observer.cleanup.clone();
    assert!(
        observer
            .await
            .unwrap()
            .to_string()
            .contains("unload deadline")
    );
    assert!(held.load(Ordering::Acquire));
    assert!(!settled());
    assert!(
        !control.calls()[before_unload..]
            .iter()
            .any(|call| { call.starts_with("close(") || call.starts_with("delete_and_verify(") })
    );
    drop(node);
    close.wait_started().await;
    close.release();
    sample.wait_started().await;
    assert!(held.load(Ordering::Acquire));
    assert!(!settled());
    sample.release();
    deletion.wait_started().await;
    assert!(settled());
    assert!(!held.load(Ordering::Acquire));
    let mut completion = Box::pin(cleanup.wait());
    assert!(futures::poll!(completion.as_mut()).is_pending());
    deletion.release();
    completion.await.unwrap();
}

#[test]
#[timeout("5s")]
async fn pending_filesystem_deletion_does_not_hold_scheduler_capacity() {
    use crate::services::active_agents::ConcurrentAgentsScheduler;
    use golem_common::model::AgentId;
    use golem_common::model::account::AccountId;
    use uuid::Uuid;

    let (filesystem, control) = resident_for_unload_test().await;
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
    let account = AccountId(Uuid::new_v4());
    scheduler
        .register_account(account, Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1)))
        .await;
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId::new(),
        agent_id: "unload-capacity".to_string(),
    };
    let permit = scheduler.acquire(account, agent_id.clone()).await;
    let mut waiting = Box::pin(scheduler.acquire(account, agent_id));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    let window = crate::services::agent_filesystem::open_resource_usage_window(&filesystem, permit)
        .await
        .unwrap();
    let mut permits = ConcurrentAgentPermitState::new(None, Arc::new(AtomicBool::new(false)));
    permits.install_window(super::ExecutionWindow::unmonitored(window));
    let observer = unload_resident_agent_ownership(
        ResidentAgentOwnership {
            runtime: (),
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now() + Duration::from_secs(1),
        &mut permits,
        &Arc::new(Mutex::new(None)),
        None,
    );
    deletion.wait_started().await;
    let replacement = waiting.await;
    let mut observer = Box::pin(observer);
    assert!(futures::poll!(observer.as_mut()).is_pending());
    deletion.release();
    assert!(observer.await.is_none());
    drop(replacement);
}

#[test]
#[timeout("5s")]
async fn concrete_unload_running_agent_publishes_only_after_verified_deletion() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control, window, generation_handle, node) =
        metered_resident_with_open_node_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_close(Ok(()));
    let close = control.block("close");
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let held = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored_with_scope(
        window,
        scope.clone(),
    ));
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));
    let store_dropped = Arc::new(AtomicBool::new(false));

    let observer = InvocationLoop::<Context>::unload_running_agent(
        RunningAgent {
            runtime: TestStoreOwner {
                node: Some(node),
                dropped: Arc::clone(&store_dropped),
            },
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now() + Duration::from_secs(1),
        &mut permit_state,
        &activity,
        None,
    );
    let publication = tokio::spawn(async move {
        let cleanup_error = observer.await;
        publish_unload_outcome(
            cleanup_error,
            None,
            |error| async move { complete_stopping(FinalWorkerState::CleanupFailed(error)) },
            |startup_failure| async move {
                complete_stopping(FinalWorkerState::Unloaded { startup_failure })
            },
        )
        .await
    });

    let late_target = PathTarget::at_root(&generation_handle, "late-dispatch").unwrap();
    assert!(matches!(
        attributes(&generation_handle, Target::Path(&late_target, Follow::Yes)),
        Err(AccessError::Revoked)
    ));
    assert!(activity.lock().unwrap().is_none());
    close.wait_started().await;
    assert!(store_dropped.load(Ordering::Acquire));
    assert!(held.load(Ordering::Acquire));
    metrics.advance(1000);
    close.release();

    deletion.wait_started().await;
    assert!(!held.load(Ordering::Acquire));
    tokio::task::yield_now().await;
    assert!(!publication.is_finished());
    assert_eq!(metrics.pending("unload", "filesystem_deleted"), 1.0);
    metrics.advance(2000);
    let driver = scope.accepted_driver();
    let follower = scope.accepted_driver();
    scope.freeze_cause(crate::metrics::resource_release::Cause::Interrupt);
    follower.complete();
    assert_eq!(metrics.pending("accepted_stop", "end_to_end"), 1.0);
    assert_eq!(
        metrics.count("accepted_stop", "filesystem_drained", "released"),
        0.0
    );
    metrics.advance(4000);
    deletion.release();
    let instance = publication.await.unwrap();
    assert!(matches!(instance, WorkerInstance::Unloaded { .. }));
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(metrics.pending("accepted_stop", "end_to_end"), 1.0);
    assert_eq!(
        metrics.count("accepted_stop", "end_to_end", "released"),
        0.0
    );
    metrics.advance(1000);
    driver.complete();
    metrics.assert_finished("accepted_stop", "released");
    assert!(metrics.export().contains(
        "cause=\"explicit_stop\",origin=\"unload\",outcome=\"released\",stage=\"end_to_end\"} 7"
    ));
    assert_eq!(
        metrics.count("accepted_stop", "window_prepared", "released"),
        0.0
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("delete_and_verify("))
            .count(),
        1
    );
    let export = metrics.export();
    assert!(export.contains("cause=\"interrupt\",origin=\"accepted_stop\",outcome=\"released\",stage=\"end_to_end\"} 5"), "{export}");
    metrics.assert_finished("unload", "released");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn concrete_unload_deletion_failure_publishes_cleanup_failed() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control, window, _generation_handle, node) =
        metered_resident_with_open_node_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_close(Ok(()));
    control.push_delete_and_verify(Err(FilesystemStorageError::verification(
        "injected verified deletion failure",
        None,
    )));
    let held = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored_with_scope(
        window,
        scope.clone(),
    ));
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

    let observer = InvocationLoop::<Context>::unload_running_agent(
        RunningAgent {
            runtime: TestStoreOwner {
                node: Some(node),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now() + Duration::from_secs(1),
        &mut permit_state,
        &activity,
        None,
    );
    let cleanup = observer.cleanup.clone();
    let cleanup_error = observer
        .await
        .expect("verified deletion failure must fail unload");
    assert!(
        !held.load(Ordering::Acquire),
        "failed filesystem deletion must not retain execution capacity"
    );
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let repair = cleanup.retry();
    deletion.wait_started().await;
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    deletion.release();
    repair.wait().await.unwrap();
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(cleanup.wait().await.unwrap_err(), cleanup_error);

    let instance = publish_unload_outcome(
        Some(cleanup_error),
        None,
        |error| async move { complete_stopping(FinalWorkerState::CleanupFailed(error)) },
        |startup_failure| async move {
            complete_stopping(FinalWorkerState::Unloaded { startup_failure })
        },
    )
    .await;
    assert!(matches!(instance, WorkerInstance::CleanupFailed(_)));
    metrics.assert_finished("unload", "released_with_failure");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn dropped_production_unload_observer_does_not_cancel_owned_cleanup() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control, window, _generation_handle, node) =
        metered_resident_with_open_node_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_close(Ok(()));
    let close = control.block("close");
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let held = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored_with_scope(
        window,
        scope.clone(),
    ));
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

    let observer = unload_resident_agent_ownership::<_, ScriptedSandboxFilesystem>(
        ResidentAgentOwnership {
            runtime: TestStoreOwner {
                node: Some(node),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now() + Duration::from_secs(1),
        &mut permit_state,
        &activity,
        None,
    );
    let cleanup = observer.cleanup.clone();
    drop(observer);

    close.wait_started().await;
    close.release();
    deletion.wait_started().await;
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    deletion.release();
    cleanup.wait().await.unwrap();
    metrics.assert_finished("unload", "released");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn production_unload_starts_final_observation_after_native_close_drains() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control, window, _generation_handle, node) =
        billing_metered_resident_with_open_node_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_close(Ok(()));
    let close = control.block("close");
    control.push_observe_allocation(Ok(crate::storage_volume::FilesystemAllocation {
        allocated_bytes: 100,
        filesystem_objects: 1,
    }));
    let final_observation = control.block("observe_allocation");
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let settled = window.settlement_observer();
    let held = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored_with_scope(
        window,
        scope.clone(),
    ));
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

    let observer = unload_resident_agent_ownership::<_, ScriptedSandboxFilesystem>(
        ResidentAgentOwnership {
            runtime: TestStoreOwner {
                node: Some(node),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now() + Duration::from_secs(1),
        &mut permit_state,
        &activity,
        None,
    );

    close.wait_started().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), final_observation.wait_started())
            .await
            .is_err()
    );
    close.release();
    final_observation.wait_started().await;
    final_observation.release();
    deletion.wait_started().await;
    assert!(!held.load(Ordering::Acquire));
    assert!(
        settled(),
        "execution accounting must settle before filesystem deletion"
    );
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    deletion.release();

    assert!(observer.await.is_none());
    assert!(settled());
    assert!(!held.load(Ordering::Acquire));
    metrics.assert_finished("unload", "released");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn startup_disposal_releases_execution_before_failed_deletion_and_repair() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control, window, _generation_handle, node) =
        billing_metered_resident_with_open_node_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_close(Ok(()));
    control.push_observe_allocation(Ok(crate::storage_volume::FilesystemAllocation {
        allocated_bytes: 100,
        filesystem_objects: 1,
    }));
    control.push_delete_and_verify(Err(FilesystemStorageError::verification(
        "startup deletion failure",
        None,
    )));
    let deletion = control.block("delete_and_verify");
    let settled = window.settlement_observer();
    let window = super::ExecutionWindow::unmonitored_with_scope(window, scope.clone());
    drop(node);
    let task = tokio::spawn(super::cleanup_startup_filesystem(
        seal(filesystem),
        window,
        WorkerExecutorError::runtime("startup failed"),
    ));
    deletion.wait_started().await;
    assert!(settled());
    assert_eq!(metrics.count("unload", "permit_released", "released"), 1.0);
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    assert!(!task.is_finished());
    deletion.release();
    let CreateWorkerInstanceError::Failed {
        error,
        filesystem_cleanup_failure,
    } = task.await.unwrap()
    else {
        panic!("cleanup must return a startup failure");
    };
    assert!(error.to_string().contains("startup failed"));
    assert!(error.to_string().contains("startup deletion failure"));
    assert!(settled());
    assert_eq!(metrics.count("unload", "permit_released", "released"), 1.0);
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    let (cleanup, sender) = super::UnloadCleanup::new(None);
    sender
        .send(Err(filesystem_cleanup_failure.unwrap()))
        .unwrap();
    let original = cleanup.wait().await.unwrap_err();
    control.push_delete_and_verify(Ok(()));
    let repair_gate = control.block("delete_and_verify");
    let repair = cleanup.retry();
    repair_gate.wait_started().await;
    assert!(settled());
    assert_eq!(metrics.count("unload", "permit_released", "released"), 1.0);
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    repair_gate.release();
    repair.wait().await.unwrap();
    assert!(settled());
    assert_eq!(metrics.count("unload", "permit_released", "released"), 1.0);
    assert_eq!(cleanup.wait().await.unwrap_err(), original);
    metrics.assert_finished("unload", "released_with_failure");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn unload_preparation_defers_allocation_to_window_close() {
    use futures::FutureExt;

    let (filesystem, control, window, _generation_handle, node) =
        billing_metered_resident_with_open_node_for_unload_test().await;
    control.push_close(Ok(()));
    control.push_observe_allocation(Ok(crate::storage_volume::FilesystemAllocation {
        allocated_bytes: 100,
        filesystem_objects: 1,
    }));
    control.push_delete_and_verify(Ok(()));
    let final_sample = control.block("observe_allocation");
    let observations = || {
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count()
    };
    let before = observations();
    let mut preparation = Box::pin(super::ExecutionWindow::unmonitored(window).prepare_disposal());
    let prepared = preparation.as_mut().now_or_never();
    let prepared_without_sampling = prepared.is_some();
    let after_preparation = observations();
    final_sample.release();
    let (window, error) = match prepared {
        Some(result) => result,
        None => preparation.await,
    };
    assert!(error.is_none());
    let result = crate::services::resource_usage_metering::close_window(
        window,
        super::resource_usage_close_deadline(),
    )
    .await;
    result.unwrap();
    drop(node);
    drop(filesystem);
    assert!(
        prepared_without_sampling,
        "unload preparation must leave final allocation sampling to ordinary window close"
    );
    assert_eq!(after_preparation, before);
    assert_eq!(observations(), before + 1);
}

#[test]
#[timeout("5s")]
async fn execution_window_closes_before_filesystem_deletion() {
    let (filesystem, control, window, _generation_handle, node) =
        billing_metered_resident_with_open_node_for_unload_test().await;
    control.push_close(Ok(()));
    control.push_observe_allocation(Ok(crate::storage_volume::FilesystemAllocation {
        allocated_bytes: 100,
        filesystem_objects: 1,
    }));
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let settled = window.settlement_observer();
    let held = Arc::new(AtomicBool::new(false));
    let store_dropped = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored(window));
    let observer = unload_resident_agent_ownership(
        ResidentAgentOwnership {
            runtime: TestStoreOwner {
                node: Some(node),
                dropped: store_dropped.clone(),
            },
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now() + Duration::from_secs(1),
        &mut permit_state,
        &Arc::new(Mutex::new(None)),
        None,
    );

    deletion.wait_started().await;
    let settled_before_deletion = settled();
    let held_during_deletion = held.load(Ordering::Acquire);
    let store_dropped_before_deletion = store_dropped.load(Ordering::Acquire);
    deletion.release();
    assert!(observer.await.is_none());
    assert!(store_dropped_before_deletion);
    assert!(
        settled_before_deletion,
        "execution accounting must close before filesystem deletion"
    );
    assert!(
        !held_during_deletion,
        "filesystem deletion must not retain the execution permit"
    );
}

#[test]
#[timeout("5s")]
async fn active_allocation_failure_remains_failed_after_successful_physical_deletion() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control, window, _generation_handle, node) =
        billing_metered_resident_with_open_node_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_observe_allocation(Err(FilesystemStorageError::verification(
        "active allocation observation failed",
        None,
    )));
    let observation = control.block("observe_allocation");
    let mut health = window.subscribe_enforcement_failure();
    observation.wait_started().await;
    observation.release();
    let failure =
        crate::services::resource_usage_metering::wait_for_enforcement_failure(&mut health).await;
    assert!(
        failure
            .to_string()
            .contains("active allocation observation failed")
    );
    control.push_close(Ok(()));
    control.push_observe_allocation(Ok(crate::storage_volume::FilesystemAllocation {
        allocated_bytes: 100,
        filesystem_objects: 1,
    }));
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let held = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored_with_scope(
        window,
        scope.clone(),
    ));
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));
    let observer = unload_resident_agent_ownership(
        ResidentAgentOwnership {
            runtime: TestStoreOwner {
                node: Some(node),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            filesystem,
        },
        UnloadReason::Failure,
        Instant::now() + Duration::from_secs(1),
        &mut permit_state,
        &activity,
        None,
    );
    let cleanup = observer.cleanup.clone();
    deletion.wait_started().await;
    assert!(!held.load(Ordering::Acquire));
    let error = observer.await.unwrap();
    assert!(
        error
            .to_string()
            .contains("active allocation observation failed")
    );
    assert_eq!(metrics.pending("unload", "end_to_end"), 1.0);
    deletion.release();
    assert_eq!(cleanup.wait().await.unwrap_err(), error);
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(cleanup.retry().wait().await.unwrap_err(), error);
    metrics.assert_finished("unload", "released_with_failure");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn concrete_unload_deadline_publishes_cleanup_failed_and_cleanup_continues() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control, window, _generation_handle, node) =
        metered_resident_with_open_node_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_close(Ok(()));
    let close = control.block("close");
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let held = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored_with_scope(
        window,
        scope.clone(),
    ));
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

    let observer = InvocationLoop::<Context>::unload_running_agent(
        RunningAgent {
            runtime: TestStoreOwner {
                node: Some(node),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            filesystem,
        },
        UnloadReason::ExplicitStop,
        Instant::now() + Duration::from_millis(20),
        &mut permit_state,
        &activity,
        None,
    );
    let cleanup = observer.cleanup.clone();

    close.wait_started().await;
    let cleanup_error = tokio::time::timeout(Duration::from_millis(200), observer)
        .await
        .expect("deadline must resolve the lifecycle observer")
        .expect("deadline must report cleanup failure");
    assert!(cleanup_error.to_string().contains("unload deadline"));
    assert!(held.load(Ordering::Acquire));
    let failure_count = metrics.value("golem_agent_resource_cleanup_failures_total", &[]);
    let retained = cleanup.with_failure(cleanup_error.clone());
    let retained_again = cleanup.with_failure(cleanup_error.clone());
    assert_eq!(
        metrics.value("golem_agent_resource_cleanup_failures_total", &[]),
        failure_count
    );
    assert_eq!(
        metrics.value(
            "golem_agent_resource_cleanup_failures_total",
            &[("stage", "end_to_end"), ("reason", "deadline")]
        ),
        1.0
    );

    let instance = publish_unload_outcome(
        Some(cleanup_error.clone()),
        None,
        |error| async move { complete_stopping(FinalWorkerState::CleanupFailed(error)) },
        |startup_failure| async move {
            complete_stopping(FinalWorkerState::Unloaded { startup_failure })
        },
    )
    .await;
    assert!(matches!(instance, WorkerInstance::CleanupFailed(_)));
    assert!(!matches!(instance, WorkerInstance::Stopping(_)));
    assert!(
        !control
            .calls()
            .iter()
            .any(|call| call.starts_with("delete_and_verify("))
    );

    let retry = cleanup.retry();
    let mut retry_wait = Box::pin(retry.wait());
    assert!(futures::poll!(retry_wait.as_mut()).is_pending());
    close.release();
    deletion.wait_started().await;
    assert!(futures::poll!(retry_wait.as_mut()).is_pending());
    deletion.release();
    deletion.wait_completed().await;
    retry_wait.await.unwrap();
    cleanup.wait().await.unwrap();
    assert_eq!(retained.wait().await.unwrap_err(), cleanup_error);
    assert_eq!(retained_again.wait().await.unwrap_err(), cleanup_error);
    assert_eq!(
        metrics.value(
            "golem_agent_resource_cleanup_failures_total",
            &[("reason", "other")]
        ),
        0.0
    );
    assert_eq!(
        metrics.value(
            "golem_agent_resource_cleanup_failures_total",
            &[("stage", "end_to_end"), ("reason", "deadline")]
        ),
        1.0
    );
    assert!(matches!(instance, WorkerInstance::CleanupFailed(error) if error == cleanup_error));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("delete_and_verify("))
            .count(),
        1
    );
    metrics.assert_finished("unload", "released_with_failure");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn concrete_failed_unload_retains_retry_owner_and_immutable_results() {
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let (filesystem, control) = resident_for_unload_test().await;
    filesystem.set_release_metrics_for_test(metrics.scope());
    control.push_delete_and_verify(Err(FilesystemStorageError::verification(
        "first cleanup failure",
        None,
    )));
    let mut permits = ConcurrentAgentPermitState::new(None, Arc::new(AtomicBool::new(false)));
    permits.release_scope = scope.clone();
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));
    let observer = unload_resident_agent_ownership(
        ResidentAgentOwnership {
            runtime: (),
            filesystem,
        },
        UnloadReason::Deleting,
        Instant::now() + Duration::from_secs(1),
        &mut permits,
        &activity,
        None,
    );
    let cleanup = observer.cleanup.clone();
    let original_error = observer.await.unwrap();
    assert!(original_error.to_string().contains("first cleanup failure"));
    assert_eq!(cleanup.wait().await.unwrap_err(), original_error);

    control.push_delete_and_verify(Err(FilesystemStorageError::verification(
        "persistent cleanup failure",
        None,
    )));
    let failed_retry = cleanup.retry();
    let retry_error = failed_retry.wait().await.unwrap_err();
    assert!(
        retry_error
            .to_string()
            .contains("persistent cleanup failure")
    );

    control.push_delete_and_verify(Ok(()));
    let gate = control.block("delete_and_verify");
    let successful_retry = failed_retry.retry();
    let waiting = {
        let cleanup = successful_retry.clone();
        tokio::spawn(async move { cleanup.wait().await })
    };
    gate.wait_started().await;
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    let mut late = Box::pin(successful_retry.wait());
    assert!(futures::poll!(late.as_mut()).is_pending());
    gate.release();
    late.await.unwrap();
    assert_eq!(cleanup.wait().await.unwrap_err(), original_error);
    assert_eq!(failed_retry.wait().await.unwrap_err(), retry_error);
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("delete_and_verify("))
            .count(),
        3
    );
    metrics.assert_finished("unload", "released_with_failure");
    assert_eq!(
        metrics.value(
            "golem_agent_filesystem_lifecycle_seconds",
            &[("operation", "delete")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn startup_rollback_retains_cleanup_retry_without_clearing_close_failure() {
    for close_error in [
        None,
        Some(WorkerExecutorError::runtime("unverified metering close")),
    ] {
        let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
        let (filesystem, control) = resident_for_unload_test().await;
        filesystem.set_release_metrics_for_test(metrics.scope());
        control.push_delete_and_verify(Err(FilesystemStorageError::verification(
            "startup cleanup failure",
            None,
        )));
        let failure = crate::worker::cleanup_typed_agent_filesystem(
            seal(filesystem),
            WorkerExecutorError::runtime("instantiation failure"),
            close_error.clone(),
        )
        .await;
        let CreateWorkerInstanceError::Failed {
            error,
            filesystem_cleanup_failure,
        } = failure
        else {
            panic!("cleanup must return a startup failure");
        };
        assert!(error.to_string().contains("instantiation failure"));
        assert!(error.to_string().contains("startup cleanup failure"));
        let (cleanup, sender) = super::UnloadCleanup::new(None);
        sender
            .send(Err(filesystem_cleanup_failure.unwrap()))
            .unwrap();
        assert_eq!(
            metrics.pending("filesystem_delete", "filesystem_deleted"),
            1.0
        );
        control.push_delete_and_verify(Ok(()));
        let retry = cleanup.retry();
        assert_eq!(retry.wait().await, close_error.map_or(Ok(()), Err));
        assert_eq!(
            metrics.value(
                "golem_agent_filesystem_lifecycle_seconds",
                &[("outcome", "success_after_failure")]
            ),
            1.0
        );
        assert_eq!(
            metrics.pending("filesystem_delete", "filesystem_deleted"),
            0.0
        );
        assert!(
            cleanup
                .wait()
                .await
                .unwrap_err()
                .to_string()
                .contains("startup cleanup failure")
        );
        assert_eq!(
            control
                .calls()
                .iter()
                .filter(|call| call.starts_with("delete_and_verify("))
                .count(),
            2
        );
    }
}

#[test]
#[timeout("5s")]
async fn filesystem_pressure_stop_seam_reaches_concrete_unload_with_exact_deadline() {
    let (filesystem, control, window, _generation_handle, node) =
        metered_resident_with_open_node_for_unload_test().await;
    control.push_close(Ok(()));
    let close = control.block("close");
    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let held = Arc::new(AtomicBool::new(false));
    let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
    permit_state.install_window(super::ExecutionWindow::unmonitored(window));
    let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));
    let deadline = Instant::now() + Duration::from_secs(1);
    let eligibility = FilesystemPressureEligibility {
        idle_since: 11,
        last_effect_completion: 7,
    };

    let pressure_stop = tokio::spawn(stop_loaded_idle_if_eligible(
        eligibility,
        crate::worker::UnloadRequest::new(UnloadReason::FilesystemPressure, deadline),
        move |target_class, expected_eligibility, unload_request: crate::worker::UnloadRequest| async move {
            assert_eq!(target_class, EvictionClass::LoadedIdle);
            assert_eq!(expected_eligibility, Some(eligibility));
            assert_eq!(unload_request.reason, UnloadReason::FilesystemPressure);
            assert_eq!(unload_request.deadline, deadline);
            InvocationLoop::<Context>::unload_running_agent(
                RunningAgent {
                    runtime: TestStoreOwner {
                        node: Some(node),
                        dropped: Arc::new(AtomicBool::new(false)),
                    },
                    filesystem,
                },
                unload_request.reason,
                unload_request.deadline,
                &mut permit_state,
                &activity,
                None,
            )
            .await
        },
    ));

    close.wait_started().await;
    assert!(held.load(Ordering::Acquire));
    close.release();
    deletion.wait_started().await;
    assert!(!held.load(Ordering::Acquire));
    deletion.release();
    assert!(pressure_stop.await.unwrap().is_none());
    assert!(!held.load(Ordering::Acquire));
}

#[test]
fn permit_state_stays_conservative_through_release_and_reacquisition() {
    let held = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let held_while_dropping = Arc::new(AtomicBool::new(false));
    let mut state = ConcurrentAgentPermitState::new(None, held.clone());

    state.install(TestPermit {
        held: held.clone(),
        drops: drops.clone(),
        held_while_dropping: held_while_dropping.clone(),
    });
    assert!(held.load(Ordering::Acquire));
    assert!(state.is_some());

    state.release();
    assert!(state.is_none());
    assert!(!held.load(Ordering::Acquire));
    assert!(held_while_dropping.load(Ordering::Acquire));
    assert_eq!(drops.load(Ordering::Acquire), 1);

    state.install(TestPermit {
        held: held.clone(),
        drops: drops.clone(),
        held_while_dropping,
    });
    assert!(held.load(Ordering::Acquire));
    state.release();
    assert_eq!(drops.load(Ordering::Acquire), 2);
}

/// A periodic snapshot record that did not reach the oplog because its storage did not answer
/// retries; an oplog that does not read for good stops the loop with its error.
#[test]
fn a_periodic_record_over_an_unavailable_oplog_retries() {
    use crate::services::oplog::{OplogError, OplogReadError, OplogReadSource, UnreadableOplog};
    let agent_id = golem_common::model::AgentId {
        component_id: golem_common::model::component::ComponentId::new(),
        agent_id: "periodic".to_string(),
    };
    let unreadable = |error| {
        PeriodicFailure::Write(OplogError::Unreadable(UnreadableOplog::new(
            &agent_id, error,
        )))
    };
    assert_eq!(
        periodic_failure_outcome(&unreadable(OplogReadError::source_unavailable(
            OplogReadSource::Archive(1),
            "outage",
        ))),
        CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
    );
    let corruption = UnreadableOplog::new(
        &agent_id,
        OplogReadError::corruption(OplogReadSource::Archive(1), "bad chunk"),
    );
    assert_eq!(
        periodic_failure_outcome(&PeriodicFailure::Write(OplogError::Unreadable(
            corruption.clone()
        ))),
        CommandOutcome::BreakOuterLoop(Some(WorkerExecutorError::from(corruption)))
    );
}

/// An oplog that failed for good stops the agent and carries the error to the stop, which
/// gives it to the waiting callers. A lost shard stops without it, and a failure that a later
/// attempt can pass reconstructs the agent after the delay.
#[test]
fn an_unreadable_oplog_stops_the_agent_on_a_shard_it_still_owns() {
    let unreadable = WorkerExecutorError::oplog_failed(
        golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId::new(),
            agent_id: "unreadable".to_string(),
        },
        "corrupt oplog data from archive layer 1",
    );
    let recovery = WorkerExecutorError::recovery_required("payload backend unavailable");
    let delay = || Duration::from_secs(3);
    assert_eq!(
        infrastructure_failure_outcome(false, &unreadable, delay),
        CommandOutcome::BreakOuterLoop(Some(unreadable.clone()))
    );
    assert_eq!(
        infrastructure_failure_outcome(true, &recovery, delay),
        CommandOutcome::BreakInnerLoop(RetryDecision::None)
    );
    assert_eq!(
        infrastructure_failure_outcome(false, &recovery, delay),
        CommandOutcome::BreakInnerLoopForRecovery {
            decision: RetryDecision::Delayed(Duration::from_secs(3)),
            error: recovery.clone(),
        }
    );
}

#[test]
fn infrastructure_recovery_branch_preserves_owner_and_ephemeral_priority() {
    let result = Err(WorkerExecutorError::recovery_required(
        "payload backend unavailable",
    ));
    assert!(
        crate::durable_host::selected_recovery_error(None, AgentMode::Durable, &result).is_some()
    );
    assert!(
        crate::durable_host::selected_recovery_error(None, AgentMode::Ephemeral, &result).is_none()
    );

    let shard_loss = OwnerFailureWinner::Lifecycle(InterruptKind::ShardLost);
    assert!(
        crate::durable_host::selected_recovery_error(
            Some(&shard_loss),
            AgentMode::Durable,
            &result
        )
        .is_none()
    );
    let quota_throttle =
        OwnerFailureWinner::Lifecycle(InterruptKind::Suspend(Timestamp::now_utc()));
    assert!(
        crate::durable_host::selected_recovery_error(
            Some(&quota_throttle),
            AgentMode::Durable,
            &result,
        )
        .is_none()
    );
    for winner in [
        OwnerFailureWinner::Trap(TrapType::Exit),
        OwnerFailureWinner::Infrastructure(WorkerExecutorError::runtime("earlier failure")),
    ] {
        assert!(
            crate::durable_host::selected_recovery_error(
                Some(&winner),
                AgentMode::Durable,
                &result
            )
            .is_none()
        );
    }
    let recovery = WorkerExecutorError::recovery_required("elected recovery failure");
    let winner = OwnerFailureWinner::Infrastructure(recovery.clone());
    let interrupted = Err(WorkerExecutorError::Interrupted {
        kind: InterruptKind::Jump,
    });
    assert_eq!(
        crate::durable_host::selected_recovery_error(
            Some(&winner),
            AgentMode::Durable,
            &interrupted
        ),
        Some(&recovery)
    );
    assert!(
        crate::durable_host::selected_recovery_error(
            Some(&winner),
            AgentMode::Ephemeral,
            &interrupted
        )
        .is_none()
    );
}

#[test]
fn infrastructure_recovery_branch_rejects_ordinary_infrastructure_error() {
    let result = Err(WorkerExecutorError::runtime("host task failed"));
    assert!(
        crate::durable_host::selected_recovery_error(None, AgentMode::Durable, &result).is_none()
    );
}

#[test]
fn periodic_snapshot_uses_creation_time_until_the_first_snapshot() {
    let created_at = Timestamp::from(1_000);

    let baseline = snapshot_baseline_timestamp(None, None, created_at);

    assert_eq!(baseline, created_at);
}

#[test]
fn periodic_snapshot_waits_a_period_from_the_later_of_the_last_record_and_the_last_attempt() {
    let created_at = Timestamp::from(1_000);
    let record = Timestamp::from(2_000);
    let attempt = Timestamp::from(3_000);

    assert_eq!(
        [
            snapshot_baseline_timestamp(Some(record), Some(attempt), created_at),
            snapshot_baseline_timestamp(Some(attempt), Some(record), created_at),
            snapshot_baseline_timestamp(None, Some(attempt), created_at),
            snapshot_baseline_timestamp(Some(record), None, created_at),
        ],
        [attempt, attempt, attempt, record]
    );
    assert_eq!(
        snapshot_action_at(
            snapshot_baseline_timestamp(Some(record), Some(attempt), created_at),
            Duration::from_secs(5),
            Timestamp::from(4_000),
        ),
        PeriodicSnapshotAction::Wait(Duration::from_secs(4))
    );
}

#[test]
async fn a_periodic_attempt_that_takes_longer_than_its_period_waits_a_full_period_after_its_end() {
    let clock = std::sync::atomic::AtomicU64::new(1_000);
    let attempt = async {
        // The attempt waits for a busy check, for longer than the period of 1 s.
        clock.store(6_000, Ordering::SeqCst);
        "no record"
    };
    let (outcome, ended) =
        with_end(attempt, || Timestamp::from(clock.load(Ordering::SeqCst))).await;
    let baseline = snapshot_baseline_timestamp(None, Some(ended), Timestamp::from(500));

    assert_eq!((outcome, ended), ("no record", Timestamp::from(6_000)));
    assert_eq!(
        snapshot_action_at(baseline, Duration::from_secs(1), Timestamp::from(6_200)),
        PeriodicSnapshotAction::Wait(Duration::from_millis(800))
    );
}

#[test]
fn periodic_snapshot_is_due_once_the_period_elapsed() {
    let last_snapshot = Timestamp::from(1_000);
    let now = Timestamp::from(6_000);

    let action = snapshot_action_at(last_snapshot, Duration::from_secs(5), now);

    assert_eq!(action, PeriodicSnapshotAction::DueNow);
}

#[test]
fn periodic_snapshot_waits_for_the_remaining_idle_time() {
    let last_snapshot = Timestamp::from(1_000);
    let now = Timestamp::from(4_250);

    let action = snapshot_action_at(last_snapshot, Duration::from_secs(5), now);

    assert_eq!(
        action,
        PeriodicSnapshotAction::Wait(Duration::from_millis(1_750))
    );
}

#[test]
fn periodic_snapshot_failed_invocation_triggers_immediate_recovery() {
    let result = Ok(InvokeResult::Failed {
        consumed_fuel: 0,
        timed_out: false,
        error: AgentError::InternalError("boom".to_string()),
        retry_from: OplogIndex::INITIAL,
        semantic_trap_retry_override: None,
    });

    assert_eq!(
        periodic_snapshot_failure_outcome(&result),
        Some(CommandOutcome::BreakInnerLoop(RetryDecision::Immediate))
    );
}

#[test]
fn periodic_snapshot_invocation_error_triggers_immediate_recovery() {
    let result = Err(WorkerExecutorError::runtime("boom"));

    assert_eq!(
        periodic_snapshot_failure_outcome(&result),
        Some(CommandOutcome::BreakInnerLoop(RetryDecision::Immediate))
    );
}

#[test]
fn ephemeral_non_initialization_invocation_requests_archive_drain() {
    assert_eq!(
        successful_agent_invocation_outcome(
            AgentMode::Ephemeral,
            true,
            AgentInvocationKind::AgentMethod
        ),
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None, None)
    );
}

#[test]
fn ephemeral_agent_initialization_does_not_request_active_worker_cleanup() {
    assert_eq!(
        successful_agent_invocation_outcome(
            AgentMode::Ephemeral,
            true,
            AgentInvocationKind::AgentInitialization
        ),
        CommandOutcome::Continue
    );
}

#[test]
fn durable_invocation_does_not_request_active_worker_cleanup() {
    assert_eq!(
        successful_agent_invocation_outcome(
            AgentMode::Durable,
            true,
            AgentInvocationKind::AgentMethod
        ),
        CommandOutcome::Continue
    );
}

#[test]
fn terminal_ephemeral_failure_requests_archive_drain() {
    assert_eq!(
        failed_agent_invocation_outcome(AgentMode::Ephemeral, RetryDecision::None, None),
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None, None)
    );
}

/// A terminal ephemeral failure caused by the oplog of the agent gives its waiters that
/// error, with the archive layer and the storage cause, instead of the ordinary end of an
/// ephemeral agent. Any other failure keeps the ordinary end.
#[test]
fn a_terminal_ephemeral_oplog_failure_reaches_the_waiters() {
    let oplog_failure = WorkerExecutorError::oplog_failed(
        golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId::new(),
            agent_id: "ephemeral".to_string(),
        },
        "failed to write oplog data to archive layer 0: injected",
    );
    assert_eq!(
        failed_agent_invocation_outcome(
            AgentMode::Ephemeral,
            RetryDecision::None,
            Some(&oplog_failure)
        ),
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(
            RetryDecision::None,
            Some(oplog_failure.clone())
        )
    );
    assert_eq!(
        failed_agent_invocation_outcome(
            AgentMode::Ephemeral,
            RetryDecision::None,
            Some(&WorkerExecutorError::runtime("guest failure"))
        ),
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None, None)
    );
    assert_eq!(
        failed_agent_invocation_outcome(
            AgentMode::Durable,
            RetryDecision::None,
            Some(&oplog_failure)
        ),
        CommandOutcome::BreakInnerLoop(RetryDecision::None)
    );
}

#[test]
fn retryable_ephemeral_failure_does_not_request_archive_drain() {
    assert_eq!(
        failed_agent_invocation_outcome(AgentMode::Ephemeral, RetryDecision::Immediate, None),
        CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
    );
}

#[test]
fn terminal_durable_failure_does_not_request_archive_drain() {
    assert_eq!(
        failed_agent_invocation_outcome(AgentMode::Durable, RetryDecision::None, None),
        CommandOutcome::BreakInnerLoop(RetryDecision::None)
    );
}

#[test]
fn durable_live_streaming_invocation_always_reconstructs_the_store() {
    assert_eq!(
        successful_agent_invocation_outcome(
            AgentMode::Durable,
            true,
            AgentInvocationKind::AgentMethod
        ),
        CommandOutcome::Continue
    );
}

#[test]
fn ephemeral_live_streaming_invocation_archives_its_ephemeral_oplog() {
    assert_eq!(
        successful_agent_invocation_outcome(
            AgentMode::Ephemeral,
            true,
            AgentInvocationKind::AgentMethod
        ),
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None, None)
    );
}
