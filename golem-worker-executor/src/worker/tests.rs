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
use golem_common::model::oplog::AgentError;
use golem_common::model::{AssistedSelection, UsableAutomaticSnapshot};
use golem_common::schema::SchemaValue;
use test_r::test;

/// A terminal entry of an interrupt that an archive layer did not store fails the waiters
/// with the cause. Only a fence is a lost shard, whose waiters are sent to the new owner.
#[test]
fn only_a_fenced_terminal_write_is_a_lost_shard() {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "terminal-write".to_string(),
    };
    assert!(matches!(
        terminal_write_failure(OplogError::ArchiveWrite(crate::services::oplog::ArchiveWriteFailure {
            agent_id: agent_id.clone(),
            level: 0,
            details: "injected archive failure".to_string(),
        })),
        Ok(WorkerExecutorError::OplogFailed { details, .. })
            if details.contains("injected archive failure")
    ));
    let fence = OplogFence {
        agent_id,
        expected_epoch: ShardEpoch(1),
        actual_epoch: Some(ShardEpoch(2)),
    };
    assert_eq!(
        terminal_write_failure(OplogError::Fenced(fence.clone())),
        Err(fence)
    );
}

#[test]
fn an_ephemeral_agent_takes_no_snapshot_and_a_durable_agent_keeps_its_policy() {
    let policies = || {
        [
            SnapshotPolicy::EveryNInvocation { count: 1 },
            SnapshotPolicy::Periodic {
                period: Duration::from_secs(1),
            },
            SnapshotPolicy::Disabled,
        ]
    };
    let resolved =
        |mode| policies().map(|policy| format!("{:?}", snapshot_policy_of_mode(mode, policy)));

    assert_eq!(
        resolved(AgentMode::Durable),
        policies().map(|policy| format!("{policy:?}"))
    );
    assert_eq!(
        resolved(AgentMode::Ephemeral),
        [(); 3].map(|()| format!("{:?}", SnapshotPolicy::Disabled))
    );
}

/// The first reason that is not `Requested` is the lost shard; a requested retirement and a
/// later reason change nothing. The lost shard has one store, so every reader sees it.
#[test]
fn only_the_first_retirement_for_a_moved_shard_records_the_lost_shard() {
    let recorded = |lost: bool, reason: RetirementReason| {
        newly_lost(lost, &reason).map(|reason| format!("{reason:?}"))
    };

    assert_eq!(
        [
            recorded(false, RetirementReason::Requested),
            recorded(false, RetirementReason::ShardRevoked),
            recorded(false, RetirementReason::ShardNotAssigned),
            recorded(false, RetirementReason::Fenced(None)),
            recorded(true, RetirementReason::ShardRevoked),
            recorded(true, RetirementReason::Requested),
        ],
        [
            None,
            Some("ShardRevoked".to_string()),
            Some("ShardNotAssigned".to_string()),
            Some("Fenced(None)".to_string()),
            None,
            None,
        ]
    );
}

fn assisted_pending(
    source_revision: ComponentRevision,
    source_revision_start_index: OplogIndex,
    target_revision: ComponentRevision,
) -> PendingUpdateRef {
    PendingUpdateRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::from_u64(10),
        admission_index: OplogIndex::from_u64(10),
        target_revision,
        kind: PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
            source_revision_start_index,
            snapshot: UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(7),
                component_revision: source_revision,
                filesystem_snapshot: None,
            },
        })),
    }
}

#[test]
fn startup_charge_skips_rejected_assisted_targets() {
    let source_revision = ComponentRevision::new(2).unwrap();
    let source_revision_start_index = OplogIndex::from_u64(4);
    let rejected_target = ComponentRevision::new(3).unwrap();
    let next_target = ComponentRevision::new(4).unwrap();
    let mut status = AgentStatusRecord {
        component_revision: source_revision,
        component_revision_start_index: source_revision_start_index,
        ..Default::default()
    };
    status.pending_updates.push_back(assisted_pending(
        source_revision,
        source_revision_start_index.next(),
        rejected_target,
    ));

    assert_eq!(startup_component_charge_revision(&status), source_revision);

    status.pending_updates.push_back(PendingUpdateRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::from_u64(11),
        admission_index: OplogIndex::from_u64(11),
        target_revision: next_target,
        kind: PendingUpdateKind::Automatic,
    });
    assert_eq!(startup_component_charge_revision(&status), next_target);
}

/// A revert checks the target and the record of the head that a start instantiates only when
/// the strategy of the head is chosen: an automatic update still at its admission entry is
/// left to the start, also behind a snapshot-assisted head whose source does not hold.
#[test]
fn a_revert_checks_only_a_head_whose_strategy_is_chosen() {
    let source_revision = ComponentRevision::new(2).unwrap();
    let source_revision_start_index = OplogIndex::from_u64(4);
    let target = ComponentRevision::new(4).unwrap();
    let automatic = |oplog_index: u64, admission_index: u64| PendingUpdateRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::from_u64(oplog_index),
        admission_index: OplogIndex::from_u64(admission_index),
        target_revision: target,
        kind: PendingUpdateKind::Automatic,
    };
    let status_of = |updates: Vec<PendingUpdateRef>| AgentStatusRecord {
        component_revision: source_revision,
        component_revision_start_index: source_revision_start_index,
        pending_updates: updates.into_iter().collect(),
        ..Default::default()
    };
    let stale = assisted_pending(
        source_revision,
        source_revision_start_index.next(),
        ComponentRevision::new(3).unwrap(),
    );
    let checked =
        |status: &AgentStatusRecord| revert_checked_head(status).map(|update| update.oplog_index);

    assert_eq!(checked(&status_of(vec![automatic(11, 11)])), None);
    assert_eq!(
        checked(&status_of(vec![automatic(12, 11)])),
        Some(OplogIndex::from_u64(12))
    );
    assert_eq!(
        checked(&status_of(vec![stale.clone(), automatic(11, 11)])),
        None
    );
    assert_eq!(
        checked(&status_of(vec![stale, automatic(12, 11)])),
        Some(OplogIndex::from_u64(12))
    );
}

/// Admission and the epoch read are not atomic. An agent whose shard left the assignment in
/// between must be refused, not handed an oplog that asserts nothing.
#[test]
fn an_agent_whose_shard_left_the_assignment_is_refused_an_epoch() {
    let agent = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "fenced".to_string(),
    };

    assert!(matches!(shard_epoch_to_assert(None, &agent), Ok(None)));
    assert!(matches!(
        shard_epoch_to_assert(
            Some(&ShardAssignment::unexpiring(1, [ShardId::new(0)])),
            &agent
        ),
        Ok(Some(ShardEpoch(0)))
    ));
    assert!(matches!(
        shard_epoch_to_assert(Some(&ShardAssignment::unexpiring(1, [])), &agent),
        Err(WorkerExecutorError::ShardingNotReady)
    ));
}

#[test]
fn a_kept_shard_supersedes_an_agent_only_when_its_epoch_rose() {
    assert!(epoch_superseded(Some(ShardEpoch(0)), Some(ShardEpoch(1))));
    assert!(!epoch_superseded(Some(ShardEpoch(1)), Some(ShardEpoch(1))));
    // An equal-revision redelivery carrying a lower epoch must not retire a newer handle.
    assert!(!epoch_superseded(Some(ShardEpoch(1)), Some(ShardEpoch(0))));
    assert!(!epoch_superseded(None, Some(ShardEpoch(1))));
    assert!(!epoch_superseded(Some(ShardEpoch(0)), None));
}

#[test]
fn a_delivery_gives_up_agents_off_its_shards_or_behind_their_shards_epoch() {
    let agent = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "swept".to_string(),
    };
    let at_epoch = |epoch: u64| ShardAssignment {
        shard_epochs: HashMap::from([(ShardId::new(0), ShardEpoch(epoch))]),
        ..ShardAssignment::unexpiring(1, [])
    };

    // Membership: no assignment, or one without the shard, gives the agent up whatever its
    // oplog asserts.
    for held in [None, Some(ShardEpoch(0))] {
        assert!(retired_by_assignment(None, &agent, held));
        assert!(retired_by_assignment(
            Some(&ShardAssignment::unexpiring(1, [])),
            &agent,
            held
        ));
    }

    // A kept shard gives the agent up only when its epoch rose past the one the oplog asserts.
    assert!(!retired_by_assignment(
        Some(&at_epoch(1)),
        &agent,
        Some(ShardEpoch(1))
    ));
    assert!(retired_by_assignment(
        Some(&at_epoch(1)),
        &agent,
        Some(ShardEpoch(0))
    ));
    assert!(!retired_by_assignment(
        Some(&at_epoch(0)),
        &agent,
        Some(ShardEpoch(1))
    ));
    // An agent asserting nothing, such as one still being created, is judged by membership.
    assert!(!retired_by_assignment(Some(&at_epoch(1)), &agent, None));
}

#[test]
fn scheduled_archive_remains_valid_after_the_oplog_advances() {
    let scheduled = OplogIndex::from_u64(10);
    assert!(scheduled_archive_is_current(scheduled, scheduled));
    assert!(scheduled_archive_is_current(
        OplogIndex::from_u64(11),
        scheduled
    ));
    assert!(!scheduled_archive_is_current(
        OplogIndex::from_u64(9),
        scheduled
    ));
}

#[test]
fn cancelled_resident_requests_are_pruned_without_dropping_snapshots() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let read = QueuedWorkerInvocation::ReadFile {
        path: CanonicalFilePath::from_abs_str("/file").unwrap(),
        selection: FileByteSelection::Full,
        sender,
    };
    assert!(!read.is_abandoned());
    drop(receiver);
    let mut queue = VecDeque::from([read, QueuedWorkerInvocation::SaveSnapshot]);
    queue.retain(|invocation| !invocation.is_abandoned());
    assert_eq!(queue.len(), 1);
    assert!(matches!(queue[0], QueuedWorkerInvocation::SaveSnapshot));
}

#[test]
async fn resident_filesystem_requests_receive_lifecycle_failures() {
    let error = WorkerExecutorError::runtime("initialization failed");
    let (sender, receiver) = tokio::sync::oneshot::channel();
    QueuedWorkerInvocation::ReadFile {
        path: CanonicalFilePath::from_abs_str("/file").unwrap(),
        selection: FileByteSelection::Full,
        sender,
    }
    .fail(&error);
    assert!(matches!(
        receiver.await.unwrap(),
        Err(FileReadError::Lifecycle)
    ));

    let (sender, receiver) = oneshot::channel();
    QueuedWorkerInvocation::GetFileSystemNode {
        path: CanonicalFilePath::from_abs_str("/file").unwrap(),
        sender,
    }
    .fail(&error);
    assert!(matches!(receiver.await.unwrap(), Err(actual) if actual == error));
}

#[test]
fn external_tool_deployment_revision_fence_is_optional_and_exact() {
    let listed = DeploymentRevision::try_from(7_u64).unwrap();
    let changed = DeploymentRevision::try_from(8_u64).unwrap();

    assert!(require_expected_tool_deployment_revision(None, changed).is_ok());
    assert!(require_expected_tool_deployment_revision(Some(listed), listed).is_ok());
    let error = require_expected_tool_deployment_revision(Some(listed), changed).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Invalid request: external tool deployment revision does not match"
    );
}

#[test]
fn recovery_acknowledgement_keeps_newer_cancellation_work_dirty() {
    let key = crate::durable_host::durable_stream::tests::identity().invocation;
    let mut cache = DurableTopologyRecoveryCache::default();
    let mut control = SessionControlMetadata::default();
    control.advance_coverage(OplogIndex::from_u64(12));
    cache.sessions.insert(key.clone(), control);
    cache.dirty.insert(key.clone());
    cache.acknowledge_recovery(&key, OplogIndex::from_u64(11));
    assert!(cache.dirty.contains(&key));
    cache.acknowledge_recovery(&key, OplogIndex::from_u64(12));
    assert!(!cache.dirty.contains(&key));
}

#[test]
async fn deletion_handles_retain_one_result_for_current_and_late_waiters() {
    let (handle, sender) = DeletionHandle::new();
    let first = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.wait().await })
    };
    tokio::task::yield_now().await;
    assert!(!first.is_finished());

    let error = WorkerExecutorError::runtime("injected deletion failure");
    sender.send(Err(error.clone())).unwrap();
    assert_eq!(first.await.unwrap(), Err(error.clone()));
    assert_eq!(handle.wait().await, Err(error));
}

#[test]
async fn deletion_sender_drop_completes_an_attempt_dropped_before_polling() {
    let (handle, sender) = DeletionHandle::new();
    drop(sender);

    let error = handle
        .wait()
        .await
        .expect_err("sender drop must publish failure");
    assert!(error.to_string().contains("terminal result"));
}

#[test]
fn recovery_error_classification_keeps_only_transient_failures_retryable() {
    assert!(matches!(
        recovery_agent_error(&WorkerExecutorError::runtime("temporary outage")),
        AgentError::Unknown(message) if message.contains("temporary outage")
    ));
    assert!(matches!(
        recovery_agent_error(&WorkerExecutorError::invalid_request("invalid export")),
        AgentError::InvalidRequest(message) if message == "invalid export"
    ));
    assert!(matches!(
        recovery_agent_error(&WorkerExecutorError::ComponentParseFailed {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            reason: "invalid wasm".to_string(),
        }),
        AgentError::InternalError(message) if message.contains("invalid wasm")
    ));
    let component_id = ComponentId::new();
    assert!(is_infrastructure_recovery_error(
        &WorkerExecutorError::ComponentNotFound { component_id }
    ));
    assert!(is_infrastructure_recovery_error(
        &WorkerExecutorError::recovery_required("payload backend unavailable")
    ));
    assert!(!is_infrastructure_recovery_error(
        &WorkerExecutorError::ComponentParseFailed {
            component_id,
            component_revision: ComponentRevision::INITIAL,
            reason: "invalid wasm".to_string(),
        }
    ));
}

#[test]
fn recovery_error_classification_unwraps_failed_resume() {
    let error = WorkerExecutorError::failed_to_resume_worker(
        AgentId {
            component_id: ComponentId::new(),
            agent_id: "agent".to_string(),
        },
        WorkerExecutorError::InvocationFailed {
            error: AgentError::TransientError("retry me".to_string()),
            stderr: String::new(),
        },
    );

    assert_eq!(
        recovery_agent_error(&error),
        AgentError::TransientError("retry me".to_string())
    );
}

#[test]
fn recovery_error_classification_preserves_read_only_violation() {
    assert!(matches!(
        recovery_agent_error(&WorkerExecutorError::ReadOnlyViolation {
            method: "get-state".to_string(),
            host_function: "filesystem.write".to_string(),
        }),
        AgentError::ReadOnlyViolation(_)
    ));
}

/// A lost shard is neither recorded nor retried in place, whichever path it failed: a recovery
/// `Error` entry, like an in-place retry of a streaming-session completion, would write to or
/// reopen an oplog whose owner has changed. Every path classifies with the same predicate, so
/// the shapes one of them recognises are recognised by all.
#[test]
fn a_failure_that_is_a_lost_shard_is_given_up_on_every_path() {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "fenced".to_string(),
    };
    let fence = OplogFence {
        agent_id: agent_id.clone(),
        expected_epoch: ShardEpoch(2),
        actual_epoch: Some(ShardEpoch(3)),
    };

    // A text failure is never a fence: only the typed refusal is.
    assert!(
        RetirementReason::of_error(&WorkerExecutorError::runtime(
            OplogError::Fenced(fence.clone()).to_string()
        ))
        .is_none()
    );
    // The typed refusal keeps its epochs.
    assert!(matches!(
        RetirementReason::of_error(&WorkerExecutorError::oplog_fenced(agent_id, 2, Some(3))),
        Some(RetirementReason::Fenced(Some(carried))) if carried == fence
    ));
    // Without a latched fence: a shard revoked or reassigned interrupts the agent with
    // `ShardLost` and writes nothing, so no fence ever latches.
    assert!(matches!(
        RetirementReason::of_error(&WorkerExecutorError::Interrupted {
            kind: InterruptKind::ShardLost
        }),
        Some(RetirementReason::Fenced(None))
    ));

    // Every other failure is still the agent's own, to be recorded or retried.
    for kind in [
        InterruptKind::Interrupt(Timestamp::now_utc()),
        InterruptKind::Suspend(Timestamp::now_utc()),
        InterruptKind::Restart,
        InterruptKind::Jump,
    ] {
        assert!(
            RetirementReason::of_error(&WorkerExecutorError::Interrupted { kind }).is_none(),
            "{kind:?} is not a lost shard"
        );
    }
    assert!(RetirementReason::of_error(&WorkerExecutorError::runtime("boom")).is_none());
}

#[test]
fn pending_manual_update_keeps_storage_key_but_has_no_semantic_key() {
    let target_revision = ComponentRevision::new(2).unwrap();
    let (semantic_key, storage_key, payload, _) =
        into_pending_invocation_parts(AgentInvocation::ManualUpdate { target_revision });

    assert!(semantic_key.is_none());
    assert!(!storage_key.value.is_empty());
    assert!(matches!(
        payload,
        AgentInvocationPayload::ManualUpdate {
            target_revision: actual
        } if actual == target_revision
    ));
}

#[test]
fn rejected_automatic_snapshot_uses_manual_replay_revision() {
    let active_revision = ComponentRevision::new(3).unwrap();
    let replay_revision = ComponentRevision::new(2).unwrap();
    let snapshot_index = OplogIndex::from_u64(10);
    let status = AgentStatusRecord {
        component_revision: active_revision,
        component_revision_for_replay: replay_revision,
        last_automatic_snapshot: Some(golem_common::model::AutomaticSnapshot {
            index: snapshot_index,
            timestamp: Timestamp::from(1_000),
            component_revision: active_revision,
            files: golem_common::model::SnapshotFiles::Unnamed,
        }),
        ..Default::default()
    };

    let rejecting = |index| SnapshotExclusions::default().rejecting(index, &status);
    let rejected = rejecting(snapshot_index);
    let other = rejecting(OplogIndex::from_u64(9));
    assert_eq!(
        StartSelection::of(&status, &rejected, false).replay_revision,
        replay_revision
    );
    assert_eq!(
        StartSelection::of(&status, &other, false).replay_revision,
        active_revision
    );
}

#[test]
fn active_filesystem_effects_exclude_an_otherwise_idle_worker() {
    assert!(running_worker_can_be_evicted(
        true, false, false, false, false, false
    ));
    assert!(!running_worker_can_be_evicted(
        true, false, false, false, true, false
    ));
    assert!(!running_worker_can_be_evicted(
        true, false, false, false, false, true
    ));
}

#[test]
async fn cache_retirement_guard_rolls_back_and_wakes_parked_recovery() {
    let in_progress = AtomicBool::new(true);
    let changed = tokio::sync::Notify::new();
    let mut notification = Box::pin(changed.notified());
    notification.as_mut().enable();
    {
        let _retirement = WorkerCacheRetirement {
            in_progress: &in_progress,
            changed: &changed,
            committed: false,
        };
    }
    assert!(!in_progress.load(Ordering::Acquire));
    assert!(futures::poll!(notification.as_mut()).is_ready());

    in_progress.store(true, Ordering::Release);
    let mut notification = Box::pin(changed.notified());
    notification.as_mut().enable();
    WorkerCacheRetirement {
        in_progress: &in_progress,
        changed: &changed,
        committed: false,
    }
    .commit();
    assert!(in_progress.load(Ordering::Acquire));
    assert!(futures::poll!(notification.as_mut()).is_ready());
}

#[test]
fn merging_unloaded_states_preserves_startup_failure() {
    let state = merge_final_worker_state(
        FinalWorkerState::Unloaded {
            startup_failure: None,
        },
        FinalWorkerState::Unloaded {
            startup_failure: Some(WorkerExecutorError::runtime("startup failed")),
        },
    );

    assert!(matches!(
        state,
        FinalWorkerState::Unloaded {
            startup_failure: Some(_)
        }
    ));
}

#[test]
fn concurrent_startup_callers_share_one_attempt() {
    let tracker = Arc::new(StartupAttemptTracker::default());
    let barrier = Arc::new(std::sync::Barrier::new(16));
    let handles = (0..16)
        .map(|_| {
            let tracker = tracker.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                tracker.begin(None)
            })
        })
        .collect::<Vec<_>>();

    let attempts = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert!(attempts.iter().all(|attempt| *attempt == attempts[0]));
    assert_eq!(tracker.pending(), Some(attempts[0]));
}

#[test]
fn oom_retry_preserves_startup_attempt() {
    let tracker = StartupAttemptTracker::default();
    let initial = tracker.begin(None);

    let retry = tracker.begin(Some(initial));

    assert_eq!(retry, initial);
    assert_eq!(tracker.current().unwrap(), Some(initial));
    assert!(!tracker.complete(Uuid::new_v4(), &Ok(())));
    assert_eq!(tracker.pending(), Some(initial));
    assert!(tracker.complete(initial, &Ok(())));
    assert_eq!(tracker.current().unwrap(), None);
}

#[test]
fn startup_success_requires_matching_active_attempt() {
    let tracker = StartupAttemptTracker::default();
    let attempt = tracker.begin(None);

    assert!(!tracker.complete_success_if_active(attempt, None));
    assert_eq!(tracker.pending(), Some(attempt));
    assert!(!tracker.complete_success_if_active(attempt, Some(Uuid::new_v4())));
    assert_eq!(tracker.pending(), Some(attempt));
    assert!(tracker.complete_success_if_active(attempt, Some(attempt)));
    assert_eq!(tracker.current().unwrap(), None);
}

#[test]
fn stop_wins_atomic_startup_completion() {
    let tracker = Arc::new(StartupAttemptTracker::default());
    let attempt = tracker.begin(None);
    let active_attempt = Arc::new(StdMutex::new(Some(attempt)));
    let (stop_holds_instance, wait_for_stop) = std::sync::mpsc::channel();
    let (allow_stop_to_finish, finish_stop) = std::sync::mpsc::channel();

    let stop = {
        let active_attempt = active_attempt.clone();
        std::thread::spawn(move || {
            let mut active_attempt = active_attempt.lock().unwrap();
            *active_attempt = None;
            stop_holds_instance.send(()).unwrap();
            finish_stop.recv().unwrap();
        })
    };

    wait_for_stop.recv().unwrap();
    let (completion_started, wait_for_completion) = std::sync::mpsc::channel();
    let completion = {
        let tracker = tracker.clone();
        let active_attempt = active_attempt.clone();
        std::thread::spawn(move || {
            completion_started.send(()).unwrap();
            let active_attempt = *active_attempt.lock().unwrap();
            assert!(!tracker.complete_success_if_active(attempt, active_attempt));
            let error = WorkerExecutorError::unknown("Worker stopped before startup completed");
            assert!(tracker.complete(attempt, &Err(error.clone())));
            tracker.current()
        })
    };

    wait_for_completion.recv().unwrap();
    allow_stop_to_finish.send(()).unwrap();
    stop.join().unwrap();
    let error = WorkerExecutorError::unknown("Worker stopped before startup completed");
    assert!(
        matches!(completion.join().unwrap(), Err(actual) if actual.to_string() == error.to_string())
    );
}

#[test]
async fn stopping_durable_stream_reconciler_waits_for_in_flight_pass() {
    let reconciler = Arc::new(DurableStreamAttachmentReconciler::new(
        CancellationToken::new(),
    ));
    let shutdown = reconciler.shutdown.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        started_tx.send(()).unwrap();
        finish_rx.await.unwrap();
        completed_tx.send(()).unwrap();
        shutdown.cancelled().await;
    });
    reconciler.start(handle);
    started_rx.await.unwrap();

    let mut stopping = tokio::spawn({
        let reconciler = reconciler.clone();
        async move { reconciler.stop().await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut stopping)
            .await
            .is_err(),
        "stop returned before the in-flight pass completed"
    );
    finish_tx.send(()).unwrap();

    completed_rx
        .await
        .expect("in-flight reconciliation pass did not complete");
    tokio::time::timeout(Duration::from_secs(1), stopping)
        .await
        .expect("stop did not return after the in-flight pass completed")
        .expect("stop task failed");
    assert!(reconciler.handle.lock().await.is_none());
}

#[test]
async fn cancelled_reconciler_stop_retains_the_in_flight_pass_barrier() {
    let reconciler = Arc::new(DurableStreamAttachmentReconciler::new(
        CancellationToken::new(),
    ));
    let shutdown = reconciler.shutdown.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        started_tx.send(()).unwrap();
        finish_rx.await.unwrap();
        shutdown.cancelled().await;
    });
    reconciler.start(handle);
    started_rx.await.unwrap();

    let first_stop = tokio::spawn({
        let reconciler = reconciler.clone();
        async move { reconciler.stop().await }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if reconciler.handle.try_lock().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first stop did not acquire the handle barrier");
    first_stop.abort();
    first_stop.await.expect_err("first stop was not cancelled");

    let mut second_stop = tokio::spawn({
        let reconciler = reconciler.clone();
        async move { reconciler.stop().await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut second_stop)
            .await
            .is_err(),
        "retrying stop lost the in-flight pass barrier"
    );
    finish_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(1), second_stop)
        .await
        .expect("retrying stop did not return after the in-flight pass completed")
        .expect("retrying stop task failed");
    assert!(reconciler.handle.lock().await.is_none());
}

#[test]
async fn quiescent_durable_stream_maintenance_waits_for_committed_state_change() {
    let shutdown = CancellationToken::new();
    let changed = tokio::sync::Notify::new();
    let notification = changed.notified();
    tokio::pin!(notification);
    notification.as_mut().enable();
    let mut waiting = Box::pin(wait_for_durable_stream_maintenance(
        &shutdown,
        notification,
        None,
    ));

    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut waiting)
            .await
            .is_err(),
        "quiescent maintenance woke without a committed stream-state change"
    );
    changed.notify_waiters();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("maintenance did not observe committed stream-state change")
    );
}

#[test]
async fn failed_durable_stream_maintenance_keeps_its_retry_deadline() {
    let shutdown = CancellationToken::new();

    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_durable_stream_maintenance(
                &shutdown,
                std::future::pending(),
                Some(Duration::from_millis(10)),
            ),
        )
        .await
        .expect("failed maintenance did not wake at its retry deadline")
    );
}

#[test]
async fn durable_stream_maintenance_retains_changes_during_a_pass_and_stops_when_parked() {
    let shutdown = CancellationToken::new();
    let changed = tokio::sync::Notify::new();
    let notification = changed.notified();
    tokio::pin!(notification);
    notification.as_mut().enable();
    changed.notify_waiters();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_durable_stream_maintenance(&shutdown, notification, None),
        )
        .await
        .expect("a change during maintenance was lost before parking")
    );

    let mut parked = Box::pin(wait_for_durable_stream_maintenance(
        &shutdown,
        changed.notified(),
        None,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut parked)
            .await
            .is_err(),
        "a consumed notification must not keep maintenance running"
    );
    shutdown.cancel();
    assert!(
        !tokio::time::timeout(Duration::from_secs(1), parked)
            .await
            .expect("shutdown did not wake parked maintenance")
    );
}

#[test]
fn allocated_memory_sums_unique_untouched_backings() -> anyhow::Result<()> {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(
        &engine,
        r#"(module
                (memory $aliased 2 3)
                (export "a" (memory $aliased))
                (export "b" (memory $aliased))
                (memory 4 5)
            )"#,
    )?;
    let mut store = Store::new(&engine, ());
    wasmtime::Instance::new(&mut store, &module, &[])?;

    assert_eq!(
        instance::allocated_linear_memory_bytes(&store),
        Ok(6 * 65_536)
    );
    Ok(())
}

#[test]
fn reconstructed_ephemeral_agent_is_terminal() {
    let instance = WorkerInstance::Unloaded {
        startup_failure: Some(inactive_ephemeral_agent_error()),
    };

    assert!(matches!(
        instance.startup_failure(),
        Some(WorkerExecutorError::InvalidRequest { details })
            if details == "An ephemeral agent cannot accept another invocation or be resumed"
    ));
}

#[test]
fn filesystem_cleanup_failure_overrides_pending_unload() {
    let cleanup_error = WorkerExecutorError::runtime("cleanup failed");
    let final_state = merge_final_worker_state(
        FinalWorkerState::Unloaded {
            startup_failure: None,
        },
        FinalWorkerState::CleanupFailed(cleanup_error),
    );

    assert!(matches!(final_state, FinalWorkerState::CleanupFailed(_)));
}

#[test]
fn filesystem_cleanup_failure_prevents_successful_deletion() {
    let final_state = merge_final_worker_state(
        FinalWorkerState::Deleting,
        FinalWorkerState::CleanupFailed(WorkerExecutorError::runtime("cleanup failed")),
    );

    assert!(matches!(final_state, FinalWorkerState::CleanupFailed(_)));
}

#[test]
async fn panicked_run_loop_join_becomes_cleanup_failure_and_notifies_stopper() {
    let run_loop = tokio::spawn(async { panic!("injected run-loop panic") });
    let error = run_loop.await.expect_err("run loop must panic");
    let error = WorkerExecutorError::runtime(format!(
        "invocation loop task stopped unexpectedly: {error}"
    ));
    let notify = OneShotEvent::new();
    let waiter = notify.clone();
    let mut instance = WorkerInstance::Stopping(StoppingWorker {
        notify: notify.clone(),
        final_state: FinalWorkerState::Unloaded {
            startup_failure: None,
        },
        pending_live_invocations: PendingLiveInvocationDisposition::Preserve,
    });

    merge_run_loop_failure(&mut instance, error);
    notify.set();

    tokio::time::timeout(Duration::from_secs(1), waiter.wait())
        .await
        .expect("stop notification must not be stranded");
    assert!(matches!(
        instance,
        WorkerInstance::Stopping(StoppingWorker {
            final_state: FinalWorkerState::CleanupFailed(_),
            ..
        })
    ));
}

#[test]
fn no_store_restart_acknowledges_receipt_and_clears_latched_interrupt() {
    let mut status = ExecutionStatus::loading(AgentMode::Durable);
    let receipt = Arc::new(tokio::sync::broadcast::channel(1).0);
    let mut observer = receipt.subscribe();
    assert!(status.publish_interrupt(InterruptKind::Restart, receipt, None));

    acknowledge_no_store_interruption(&mut status, Some(InterruptKind::Restart));

    assert!(matches!(status, ExecutionStatus::Suspended { .. }));
    assert_eq!(observer.try_recv(), Ok(()));
}

#[test]
fn no_store_acknowledgement_preserves_a_different_published_kind() {
    let mut status = ExecutionStatus::loading(AgentMode::Durable);
    let receipt = Arc::new(tokio::sync::broadcast::channel(1).0);
    let mut observer = receipt.subscribe();
    let kind = InterruptKind::Interrupt(Timestamp::now_utc());
    assert!(status.publish_interrupt(kind, receipt, None));

    acknowledge_no_store_interruption(&mut status, Some(InterruptKind::Restart));

    assert!(
        matches!(status, ExecutionStatus::Interrupting { interrupt_kind, .. } if interrupt_kind == kind)
    );
    assert_eq!(
        observer.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    );
}

#[test]
fn post_join_runtime_retains_cleanup_health_before_stopping_completion() {
    let error = WorkerExecutorError::runtime("actor loss cleanup");
    let mut runtime = WorkerInstance::Stopping(StoppingWorker {
        notify: OneShotEvent::new(),
        final_state: FinalWorkerState::Deleting,
        pending_live_invocations: PendingLiveInvocationDisposition::Fail,
    });
    assert!(runtime.retained_cleanup_failure().is_none());

    merge_run_loop_failure(&mut runtime, error.clone());
    assert_eq!(runtime.retained_cleanup_failure(), Some(&error));
    runtime = WorkerInstance::CleanupFailed(error.clone());
    assert_eq!(runtime.retained_cleanup_failure(), Some(&error));
}

#[test]
fn ephemeral_agent_accepts_only_one_invocation_identity() {
    let first = IdempotencyKey::fresh();
    let second = IdempotencyKey::fresh();
    let mut state = EphemeralInvocationState::Available;

    assert!(state.accept(&first).is_ok());
    assert!(state.accept(&first).is_ok());
    assert!(state.accept(&second).is_err());

    let mut reconstructed = EphemeralInvocationState::Accepted(None);
    assert!(reconstructed.accept(&first).is_err());
}

#[test]
fn hydrated_invocation_result_cache_is_bounded() {
    let first = IdempotencyKey::new("first".to_string());
    let second = IdempotencyKey::new("second".to_string());
    let mut cache = HydratedInvocationResultCache::new(1);
    cache.insert(
        first.clone(),
        InvocationResult::Lazy {
            oplog_idx: OplogIndex::from_u64(2),
        },
        0,
        OplogIndex::from_u64(2),
    );

    cache.insert(
        second.clone(),
        InvocationResult::Lazy {
            oplog_idx: OplogIndex::from_u64(4),
        },
        0,
        OplogIndex::from_u64(4),
    );

    assert!(!cache.contains_key(&first));
    assert!(cache.contains_key(&second));
}

#[test]
fn hydrated_invocation_results_share_payloads_but_reject_deleted_branches() {
    let key = IdempotencyKey::new("shared-result".to_string());
    let index = OplogIndex::from_u64(4);
    let status = AgentStatusRecord::default();
    let generation = status.invocation_results.revert_generation();
    let result = Arc::new(InvocationResult::Cached {
        result: Ok(AgentInvocationOutput {
            result: AgentInvocationResult::AgentMethod {
                output: SchemaValue::List {
                    elements: (0..10_000)
                        .map(|i| SchemaValue::U8((i % 251) as u8))
                        .collect(),
                },
            },
            consumed_fuel: Some(123),
            invocation_status: None,
            component_revision: Some(ComponentRevision::INITIAL),
            agent_id: None,
            idempotency_key: Some(key.clone()),
            oplog_index: Some(index),
            agent_fingerprint: None,
        }),
    });
    let mut cache = HydratedInvocationResultCache::new(1);
    cache.insert(key.clone(), result.clone(), generation, index);
    let (first, found_index) = cache.get_valid(&key, &status).unwrap();
    assert_eq!(found_index, index);
    assert!(Arc::ptr_eq(first, &result));
    let follower = first.clone();
    cache.insert(key.clone(), follower.clone(), generation, index);
    assert!(Arc::ptr_eq(
        cache.get_valid(&key, &status).unwrap().0,
        &follower
    ));

    cache.insert(key.clone(), result.clone(), generation + 1, index);
    assert!(cache.get_valid(&key, &status).is_none());
    cache.remove(&key);
    assert!(cache.get_valid(&key, &status).is_none());
    match lookup_result_from_cached_result(&status, &key, Arc::unwrap_or_clone(follower)) {
        LookupResult::Complete(Ok(output)) => {
            assert_eq!(output.consumed_fuel, Some(123));
            assert_eq!(output.oplog_index, Some(index));
            let AgentInvocationResult::AgentMethod { output } = output.result else {
                panic!("expected method result");
            };
            let SchemaValue::List { elements } = output else {
                panic!("expected list output");
            };
            assert_eq!(elements.len(), 10_000);
            assert_eq!(elements[9_999], SchemaValue::U8(210));
        }
        other => panic!("expected completed result, got {other:?}"),
    }
}

#[test]
fn ephemeral_invocation_requires_a_final_phantom_id_even_when_it_may_exist() {
    let result = validate_resolved_invocation_identity(
        AgentMode::Ephemeral,
        None,
        &IdempotencyKey::fresh(),
        InvocationFreshnessDisposition::MayExist,
    );

    assert!(matches!(
        result,
        Err(WorkerExecutorError::InvalidRequest { details })
            if details == "An ephemeral invocation requires a final phantom agent ID"
    ));
}

#[test]
fn known_fresh_ephemeral_invocation_requires_key_derived_phantom_id() {
    let idempotency_key = IdempotencyKey::fresh();
    let expected_phantom_id = ephemeral_invocation_phantom_id(&idempotency_key);

    assert!(
        validate_resolved_invocation_identity(
            AgentMode::Ephemeral,
            Some(expected_phantom_id),
            &idempotency_key,
            InvocationFreshnessDisposition::KnownFresh,
        )
        .is_ok()
    );
    assert!(
        validate_resolved_invocation_identity(
            AgentMode::Ephemeral,
            Some(Uuid::new_v4()),
            &idempotency_key,
            InvocationFreshnessDisposition::KnownFresh,
        )
        .is_err()
    );
}

#[test]
fn durable_may_exist_invocation_identity_is_unchanged() {
    assert!(
        validate_resolved_invocation_identity(
            AgentMode::Durable,
            None,
            &IdempotencyKey::fresh(),
            InvocationFreshnessDisposition::MayExist,
        )
        .is_ok()
    );
}

#[test]
fn start_attempt_exact_duplicate_matches_and_mismatch_does_not() {
    use golem_common::model::durable_stream::{DurableStreamHandle, StreamId};

    let environment_id = golem_common::base_model::environment::EnvironmentId(Uuid::from_u128(1));
    let agent_id = AgentId {
        component_id: golem_common::base_model::component::ComponentId(Uuid::from_u128(2)),
        agent_id: "callee".to_string(),
    };
    let fingerprint = AgentFingerprint(Uuid::from_u128(3));
    let session_key = golem_common::base_model::durable_stream::StreamInvocationId {
        callee_environment_id: environment_id,
        callee: agent_id.clone(),
        callee_fingerprint: fingerprint,
        idempotency_key: IdempotencyKey::new("invocation".to_string()),
    };
    let attempt = StartAttemptDescriptor {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: session_key.clone(),
        attachment_id: golem_common::base_model::durable_stream::AttachmentId::primary(
            environment_id,
            &agent_id,
            &session_key.idempotency_key,
        )
        .unwrap(),
        expected_callee_fingerprint: fingerprint,
        attempt_id: golem_common::base_model::durable_stream::AttemptId::fresh(),
        invocation: PersistedStreamInvocationDescriptor {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key,
            target_component_revision: ComponentRevision::INITIAL,
            target:
                golem_common::base_model::durable_stream::PersistedInvocationTarget::AgentMethod {
                    method_name: "consume".to_string(),
                },
            invocation_value: vec![1],
            stream_handles: Vec::new(),
            execution_config: vec![2],
            effective_identity: vec![3],
        },
        effective_identity: vec![3],
        live_join_buffer_events: 8,
    };

    assert!(stream_attempt_matches(&attempt, &attempt.clone()));

    let mut mismatched = attempt.clone();
    mismatched.live_join_buffer_events += 1;
    assert!(!stream_attempt_matches(&attempt, &mismatched));

    let mut mismatched = attempt.clone();
    mismatched.invocation.target =
        golem_common::base_model::durable_stream::PersistedInvocationTarget::AgentMethod {
            method_name: "other".to_string(),
        };
    assert!(!stream_attempt_matches(&attempt, &mismatched));

    let mut mismatched = attempt.clone();
    mismatched.attempt_id = golem_common::base_model::durable_stream::AttemptId::fresh();
    assert!(!stream_attempt_matches(&attempt, &mismatched));

    let mut retried = attempt.clone();
    retried.attempt_id = golem_common::base_model::durable_stream::AttemptId::fresh();
    retried.attachment_id = golem_common::base_model::durable_stream::AttachmentId(Uuid::new_v4());
    retried.live_join_buffer_events += 1;
    retried.invocation.execution_config = vec![9];
    assert!(stream_slot_session_matches(&attempt, &retried));

    retried.invocation.invocation_value = vec![4];
    assert!(!stream_slot_session_matches(&attempt, &retried));

    let mut retried = attempt.clone();
    retried.effective_identity = vec![8];
    assert!(!stream_slot_session_matches(&attempt, &retried));

    let mut persisted = attempt.clone();
    persisted
        .invocation
        .stream_handles
        .push(DurableStreamHandle {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id: StreamId::derive(
                environment_id,
                &agent_id,
                fingerprint,
                OplogIndex::INITIAL,
            )
            .unwrap(),
            producer_environment_id: environment_id,
            producer: agent_id,
            expected_producer_fingerprint: fingerprint,
            producer_generation: OplogIndex::NONE,
            source_invocation: attempt.session_key.clone(),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([17; 32]),
        });
    let mut reissued = persisted.clone();
    reissued.invocation.stream_handles[0].producer_generation = OplogIndex::from_u64(19);
    assert!(stream_attempt_matches(&persisted, &reissued));
    assert!(stream_slot_session_matches(&persisted, &reissued));
}

fn status_with_current_key(status: AgentStatus, key: &IdempotencyKey) -> AgentStatusRecord {
    AgentStatusRecord {
        status,
        current_idempotency_key: Some(key.clone()),
        ..AgentStatusRecord::default()
    }
}

#[test]
fn lookup_keeps_retrying_error_pending() {
    let key = IdempotencyKey::fresh();
    let lookup = lookup_result_from_cached_result(
        &status_with_current_key(AgentStatus::Retrying, &key),
        &key,
        InvocationResult::Cached {
            result: Err(FailedInvocationResult {
                trap_type: TrapType::Error {
                    error: AgentError::TransientError("in-function retry".to_string()),
                    retry_from: OplogIndex::from_u64(17),
                    semantic_trap_retry_override: None,
                },
                stderr: String::new(),
            }),
        },
    );

    assert!(matches!(lookup, LookupResult::Pending));
}

#[test]
fn lookup_reports_terminal_error_as_failure() {
    let key = IdempotencyKey::fresh();
    let lookup = lookup_result_from_cached_result(
        &status_with_current_key(AgentStatus::Failed, &key),
        &key,
        InvocationResult::Cached {
            result: Err(FailedInvocationResult {
                trap_type: TrapType::Error {
                    error: AgentError::TransientError("in-function retry".to_string()),
                    retry_from: OplogIndex::from_u64(17),
                    semantic_trap_retry_override: None,
                },
                stderr: String::new(),
            }),
        },
    );

    match lookup {
        LookupResult::Complete(Err(WorkerExecutorError::InvocationFailed {
            error: AgentError::TransientError(details),
            stderr,
        })) => {
            assert_eq!(details, "in-function retry");
            assert!(stderr.is_empty());
        }
        other => panic!("expected terminal lookup failure, got {other:?}"),
    }
}

#[test]
fn lookup_keeps_prior_retry_error_pending_during_recovery_failure() {
    let key = IdempotencyKey::fresh();
    let mut status = status_with_current_key(AgentStatus::Failed, &key);
    status.last_error_kind = Some(OplogErrorKind::Recovery);
    let lookup = lookup_result_from_cached_result(
        &status,
        &key,
        InvocationResult::Cached {
            result: Err(FailedInvocationResult {
                trap_type: TrapType::Error {
                    error: AgentError::TransientError("prior retry".to_string()),
                    retry_from: OplogIndex::from_u64(17),
                    semantic_trap_retry_override: None,
                },
                stderr: String::new(),
            }),
        },
    );

    assert!(matches!(lookup, LookupResult::Pending));
}

#[test]
fn reconstructed_permission_denial_keeps_its_executor_error_type() {
    let key = IdempotencyKey::fresh();
    let lookup = lookup_result_from_cached_result(
        &AgentStatusRecord::default(),
        &key,
        InvocationResult::Cached {
            result: Err(FailedInvocationResult {
                trap_type: TrapType::Error {
                    error: AgentError::PermissionDenied("permission denied".to_string()),
                    retry_from: OplogIndex::from_u64(17),
                    semantic_trap_retry_override: None,
                },
                stderr: String::new(),
            }),
        },
    );

    assert!(matches!(
        lookup,
        LookupResult::Complete(Err(WorkerExecutorError::PermissionDenied { details }))
            if details == "permission denied"
    ));
}

#[test]
fn invocation_rejection_fails_only_the_rejected_pending_key() {
    let rejected = IdempotencyKey::fresh();
    let still_pending = IdempotencyKey::fresh();
    let status = AgentStatusRecord {
        pending_invocations: vec![PendingInvocationRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(2),
            idempotency_key: Some(still_pending.clone()),
            manual_update_target_revision: None,
        }],
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        invocation_keys_to_fail(&status, Some(&rejected), false),
        vec![rejected]
    );
    assert_eq!(
        invocation_keys_to_fail(&status, None, true),
        vec![still_pending]
    );
}

#[test]
fn startup_charge_revision_uses_last_known_without_pending_update() {
    let last_known = ComponentRevision::INITIAL.next().unwrap();
    assert_eq!(
        component_charge_revision(None, last_known),
        last_known,
        "with no pending update the worker instantiates the last known revision"
    );
}

#[test]
fn startup_charge_revision_uses_pending_update_target() {
    let last_known = ComponentRevision::INITIAL;
    let target = ComponentRevision::INITIAL.next().unwrap();
    assert_eq!(
        component_charge_revision(Some(target), last_known),
        target,
        "at startup a queued pending update is applied by loading its target revision, so the charge must key to the target, not the last known revision"
    );
}

#[test]
fn classify_target_charge_charges_resolved_target() {
    assert_eq!(
        classify_target_charge(&Ok(ResolvedComponentCharge {
            module_bytes: 4096,
            initial_linear_memory_bytes: 8192,
            reserved_linear_memory_bytes: 16384,
            supported: true,
        })),
        TargetChargeAction::ChargeTarget(ResolvedComponentCharge {
            module_bytes: 4096,
            initial_linear_memory_bytes: 8192,
            reserved_linear_memory_bytes: 16384,
            supported: true,
        }),
        "a resolved target is charged with its own module size"
    );
}

#[test]
fn classify_target_charge_falls_back_for_unsupported_target_even_when_oversized() {
    assert_eq!(
        classify_target_charge(&Ok(ResolvedComponentCharge {
            module_bytes: 4096,
            initial_linear_memory_bytes: 4 * 1024 * 1024 * 1024,
            reserved_linear_memory_bytes: 4 * 1024 * 1024 * 1024,
            supported: false,
        })),
        TargetChargeAction::FallBackToCurrent,
    );
}

#[test]
fn classify_target_charge_retries_when_the_component_service_is_unavailable() {
    let unavailable = Err(WorkerExecutorError::ComponentServiceUnavailable {
        component_id: ComponentId(uuid::Uuid::new_v4()),
        component_revision: Some(ComponentRevision::new(2).unwrap()),
        reason: "transport".to_string(),
    });
    assert_eq!(
        classify_target_charge(&unavailable),
        TargetChargeAction::Retry,
        "the start passes an unavailable component service on without failing the update, so create_instance may still load the target"
    );
}

#[test]
fn classify_target_charge_falls_back_for_a_target_that_does_not_exist() {
    let not_found = Err(WorkerExecutorError::ComponentNotFound {
        component_id: ComponentId(uuid::Uuid::new_v4()),
    });
    assert_eq!(
        classify_target_charge(&not_found),
        TargetChargeAction::FallBackToCurrent,
        "the start fails the update of a target that does not exist and loads the current revision"
    );
}

#[test]
fn classify_target_charge_falls_back_for_a_refused_target() {
    let refused = Err(WorkerExecutorError::ComponentServiceRefused {
        component_id: ComponentId(uuid::Uuid::new_v4()),
        component_revision: Some(ComponentRevision::new(2).unwrap()),
        kind: golem_service_base::error::worker_executor::ComponentServiceRefusal::Unauthorized,
        reason: "unauthorized".to_string(),
    });
    assert_eq!(
        classify_target_charge(&refused),
        TargetChargeAction::FallBackToCurrent,
        "the start fails the update of a refused target and loads the current revision"
    );
}

#[test]
fn classify_target_charge_falls_back_for_any_other_error() {
    let other = Err(WorkerExecutorError::runtime(
        "component metadata cannot be parsed",
    ));
    assert_eq!(
        classify_target_charge(&other),
        TargetChargeAction::FallBackToCurrent,
        "the start fails the update for any other fetch error and loads the current revision, so a retry would never end"
    );
}

#[test]
fn interrupt_retry_decision_matrix() {
    fn decision(kind: InterruptKind, reacquire_permits: bool) -> RetryDecision {
        PendingWorkerInterrupt {
            kind,
            reacquire_permits,
            unload_request: UnloadRequest::ordinary(UnloadReason::from_interrupt(kind)),
            establishment: InterruptEstablishment::ready(),
        }
        .retry_decision()
    }

    // Restart-like interrupts retry immediately.
    assert_eq!(
        decision(InterruptKind::Restart, false),
        RetryDecision::Immediate
    );
    assert_eq!(
        decision(InterruptKind::Jump, false),
        RetryDecision::Immediate
    );

    // Explicit interrupts remain terminal.
    assert_eq!(
        decision(InterruptKind::Interrupt(Timestamp::now_utc()), false),
        RetryDecision::None
    );
    // Suspensions stop unless a newer wakeup supersedes them.
    let suspend_timestamp = Timestamp::now_utc();
    assert_eq!(
        decision(InterruptKind::Suspend(suspend_timestamp), false),
        RetryDecision::TryStop(suspend_timestamp)
    );
    // A lost shard is terminal here: a retry in place would reopen the oplog with the same
    // stale epoch, and the agent belongs to the shard's new owner now.
    assert_eq!(
        decision(InterruptKind::ShardLost, false),
        RetryDecision::None
    );
    assert_eq!(
        UnloadReason::from_interrupt(InterruptKind::ShardLost),
        UnloadReason::ShardLost
    );

    // Permit reacquisition overrides the kind-based decision for every kind.
    for kind in [
        InterruptKind::Restart,
        InterruptKind::Jump,
        InterruptKind::Interrupt(Timestamp::now_utc()),
        InterruptKind::Suspend(Timestamp::now_utc()),
        InterruptKind::ShardLost,
    ] {
        assert_eq!(
            decision(kind, true),
            RetryDecision::ReacquirePermits,
            "reacquire_permits must override the kind-based decision"
        );
    }
}

#[test]
fn interrupt_terminality_matrix() {
    fn terminal(kind: InterruptKind) -> bool {
        PendingWorkerInterrupt {
            kind,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::from_interrupt(kind)),
            establishment: InterruptEstablishment::ready(),
        }
        .is_terminal()
    }

    assert!(!terminal(InterruptKind::Restart));
    assert!(!terminal(InterruptKind::Jump));
    assert!(terminal(InterruptKind::Interrupt(Timestamp::now_utc())));
    assert!(terminal(InterruptKind::Suspend(Timestamp::now_utc())));
    assert!(terminal(InterruptKind::ShardLost));
}

#[test]
fn terminal_interrupt_claim_is_released_after_a_worker_generation() {
    let mut state = WorkerInterruptState::Idle;
    assert!(state.queue(PendingWorkerInterrupt {
        kind: InterruptKind::Suspend(Timestamp::now_utc()),
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Suspend),
        establishment: InterruptEstablishment::ready(),
    }));
    assert!(state.take().is_some());
    assert!(!state.queue(PendingWorkerInterrupt {
        kind: InterruptKind::Restart,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Restart),
        establishment: InterruptEstablishment::ready(),
    }));

    state.reset_terminal_for_new_generation();

    assert!(state.queue(PendingWorkerInterrupt {
        kind: InterruptKind::Restart,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Restart),
        establishment: InterruptEstablishment::ready(),
    }));
}

#[test]
fn a_later_terminal_request_has_its_own_establishment_after_claim() {
    let claimed = PendingWorkerInterrupt {
        kind: InterruptKind::Suspend(Timestamp::now_utc()),
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Suspend),
        establishment: InterruptEstablishment::ready(),
    };
    let mut state = WorkerInterruptState::Pending(claimed.clone());
    assert!(state.claim_pending_terminal().is_some());
    assert!(matches!(state, WorkerInterruptState::TerminalClaimed));
    assert!(state.claim_pending_terminal().is_none());

    let later = PendingWorkerInterrupt {
        kind: InterruptKind::Interrupt(Timestamp::now_utc()),
        unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
        establishment: InterruptEstablishment::ready(),
        ..claimed
    };
    assert!(state.queue(later.clone()));
    state.reset_terminal_for_new_generation();
    let actual = state.claim_pending_terminal().unwrap();
    assert_eq!(actual.kind, later.kind);
    assert!(Arc::ptr_eq(&actual.establishment, &later.establishment));
}

#[test]
fn terminal_interrupt_can_be_queued_for_a_resuming_claimed_generation() {
    let mut state = WorkerInterruptState::Pending(PendingWorkerInterrupt {
        kind: InterruptKind::Interrupt(Timestamp::now_utc()),
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
        establishment: InterruptEstablishment::ready(),
    });
    assert!(state.take().is_some());
    assert!(matches!(state, WorkerInterruptState::TerminalClaimed));

    let delete_interrupt = PendingWorkerInterrupt {
        kind: InterruptKind::Interrupt(Timestamp::now_utc()),
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Deleting),
        establishment: InterruptEstablishment::ready(),
    };
    assert!(state.queue(delete_interrupt));
    assert!(matches!(state, WorkerInterruptState::Pending(_)));
    state.reset_terminal_for_new_generation();
    assert!(matches!(state, WorkerInterruptState::Pending(_)));
    let claimed = state.take().unwrap();
    assert_eq!(claimed.unload_request.reason, UnloadReason::Deleting);
    assert!(matches!(state, WorkerInterruptState::TerminalClaimed));
}
