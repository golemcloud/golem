use super::*;
use crate::services::oplog::OplogFence;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::worker::invocation_loop::{MonthlyResourceAdmission, monthly_resource_admission};
use crate::worker::stop::tests::running_lifecycle;
use crate::worker::{
    FinalWorkerState, InvocationOutcomeVerdict, InvocationOutcomeWrite,
    PendingLiveInvocationDisposition, PendingWorkerInterrupt, StopPublication, StoppingWorker,
    UnloadReason, UnloadRequest, WaitingWorker, complete_stopping_worker,
};
use crate::workerctx::default::Context;
use futures::FutureExt;
use golem_common::model::oplog::AgentError;
use golem_common::model::{AgentId, ShardEpoch, Timestamp};
use golem_common::one_shot::OneShotEvent;
use std::sync::{Arc, OnceLock};
use test_r::test;
use tokio::sync::oneshot;

fn demand(kind: InterruptKind) -> PendingWorkerInterrupt {
    PendingWorkerInterrupt {
        kind,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
    }
}

fn context(mode: AgentMode, generation: u64, key: &IdempotencyKey) -> OutcomeContext {
    OutcomeContext {
        generation,
        key: Some(key.clone()),
        mode,
    }
}

fn guest_trap() -> TrapType {
    TrapType::Error {
        error: AgentError::DeterministicTrap("unreachable".into()),
        retry_from: OplogIndex::INITIAL,
        in_atomic_region: false,
        atomic_region_had_side_effects: false,
        semantic_trap_retry_override: None,
    }
}

async fn selected_suspend(
    generation: u64,
    kind: InterruptKind,
) -> (
    Option<SelectedInvocationOutcome>,
    InvocationOutcomeWrite,
    IdempotencyKey,
) {
    let interrupts = Interrupts::default();
    let mut guard = interrupts.lock().await;
    let key = IdempotencyKey::fresh();
    let mut selected = None;
    let (_, writer) = claim_failure::<Context>(
        &mut selected,
        &mut guard,
        context(AgentMode::Durable, generation, &key),
        &TrapType::Interrupt(kind),
        InvocationFailureOrigin::Lifecycle,
        None,
        |_| None,
    )
    .unwrap();
    (selected, writer, key)
}

fn stopping_lifecycle() -> WorkerInstance {
    WorkerInstance::Stopping(StoppingWorker {
        notify: OneShotEvent::new(),
        final_state: FinalWorkerState::Unloaded {
            startup_failure: None,
        },
        pending_live_invocations: PendingLiveInvocationDisposition::Preserve,
        concurrent_agent_permit_held: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    })
}

#[test]
fn marker_generation_capture_requires_started_matching_attempt() {
    let attempt = Uuid::new_v4();
    let other = Uuid::new_v4();
    assert_ne!(attempt, other);
    for (lifecycle, expected) in [
        (running_lifecycle(Arc::default(), attempt, true), Some(7)),
        (running_lifecycle(Arc::default(), attempt, false), None),
        (running_lifecycle(Arc::default(), other, true), None),
        (
            WorkerInstance::WaitingForPermit(WaitingWorker {
                handle: std::sync::Mutex::new(None),
                start_attempt: attempt,
            }),
            None,
        ),
        (
            WorkerInstance::Unloaded {
                startup_failure: None,
            },
            None,
        ),
        (stopping_lifecycle(), None),
    ] {
        assert_eq!(started_marker_generation(&lifecycle, attempt, 7), expected);
    }
}

#[test]
async fn pre_owner_attempt_cannot_reuse_old_committed_suspend_with_same_counter() {
    let old_attempt = Uuid::new_v4();
    let new_attempt = Uuid::new_v4();
    assert_ne!(old_attempt, new_attempt);
    let old_runtime = running_lifecycle(Arc::default(), old_attempt, true);
    assert_eq!(
        started_marker_generation(&old_runtime, old_attempt, 7),
        Some(7)
    );
    let old_kind = InterruptKind::Suspend(Timestamp::from(191_u64));
    let (selected, writer, key) = selected_suspend(7, old_kind).await;
    let receipt = selected
        .as_ref()
        .unwrap()
        .writer_completion
        .clone()
        .unwrap();
    writer.complete();
    assert_eq!(
        receipt.clone().await.unwrap(),
        InvocationOutcomeVerdict::Committed
    );
    drop(old_runtime);

    // The global counter still names the old owner until this new attempt begins.
    let pre_owner = running_lifecycle(Arc::default(), new_attempt, false);
    let established = started_marker_generation(&pre_owner, new_attempt, 7);
    assert_eq!(established, None);
    for requested in [old_kind, InterruptKind::Suspend(Timestamp::from(192_u64))] {
        assert!(
            matches!(
                unloaded_lifecycle_marker(
                    requested,
                    AgentStatus::Suspended,
                    7,
                    established,
                    selected.as_ref()
                )
                .unwrap(),
                Some(OplogEntry::Suspend { .. })
            ),
            "new pre-owner attempt owes a marker even for the exact old kind: {requested:?}"
        );
    }
    let selected = selected.as_ref().unwrap();
    assert_eq!(selected.key.as_ref(), Some(&key));
    assert!(matches!(&selected.failure, Some(TrapType::Interrupt(kind)) if *kind == old_kind));
    assert!(
        selected
            .writer_completion
            .as_ref()
            .unwrap()
            .ptr_eq(&receipt)
    );
    assert_eq!(receipt.await.unwrap(), InvocationOutcomeVerdict::Committed);
}

#[test]
async fn captured_started_marker_generation_survives_stopping_finalization() {
    let attempt = Uuid::new_v4();
    let running = running_lifecycle(Arc::default(), attempt, true);
    let established = started_marker_generation(&running, attempt, 7);
    assert_eq!(established, Some(7));
    let admission = InterruptKind::Suspend(Timestamp::from(195_u64));
    let monitor = InterruptKind::Suspend(Timestamp::from(196_u64));
    assert_ne!(admission, monitor);
    let (selected, writer, key) = selected_suspend(7, admission).await;
    let receipt = selected
        .as_ref()
        .unwrap()
        .writer_completion
        .clone()
        .unwrap();
    writer.complete();
    drop(running);
    let stopping = stopping_lifecycle();
    assert_eq!(started_marker_generation(&stopping, attempt, 7), None);
    assert!(
        unloaded_lifecycle_marker(
            monitor,
            AgentStatus::Suspended,
            7,
            established,
            selected.as_ref()
        )
        .unwrap()
        .is_none()
    );
    let WorkerInstance::Stopping(stopping) = stopping else {
        unreachable!()
    };
    let (unloaded, _) = complete_stopping_worker(
        stopping,
        FinalWorkerState::Unloaded {
            startup_failure: None,
        },
    );
    assert_eq!(started_marker_generation(&unloaded, attempt, 7), None);
    assert!(
        unloaded_lifecycle_marker(
            monitor,
            AgentStatus::Suspended,
            7,
            established,
            selected.as_ref()
        )
        .unwrap()
        .is_none()
    );
    let selected = selected.as_ref().unwrap();
    assert_eq!(selected.key.as_ref(), Some(&key));
    assert!(selected.matches_terminal_writer(7, &Some(key.clone()), admission, &receipt));
    assert!(!selected.matches_terminal_writer(7, &Some(key), monitor, &receipt));
    assert!(
        selected
            .writer_completion
            .as_ref()
            .unwrap()
            .ptr_eq(&receipt)
    );
    assert_eq!(receipt.await.unwrap(), InvocationOutcomeVerdict::Committed);
}

#[test]
async fn committed_admission_suspend_satisfies_later_marker_without_claiming_its_cause() {
    let admission = InterruptKind::Suspend(Timestamp::from(201_u64));
    let monitor = InterruptKind::Suspend(Timestamp::from(202_u64));
    assert_ne!(admission, monitor);
    let attempt = Uuid::new_v4();
    let lifecycle = running_lifecycle(Arc::default(), attempt, true);
    let established = started_marker_generation(&lifecycle, attempt, 7);
    assert_eq!(established, Some(7));
    drop(lifecycle);
    let (selected, writer, key) = selected_suspend(7, admission).await;
    let receipt = selected
        .as_ref()
        .unwrap()
        .writer_completion
        .clone()
        .unwrap();
    assert!(receipt.clone().now_or_never().is_none());
    writer.complete();
    assert!(
        unloaded_lifecycle_marker(
            monitor,
            AgentStatus::Suspended,
            7,
            established,
            selected.as_ref()
        )
        .unwrap()
        .is_none()
    );
    let selected = selected.as_ref().unwrap();
    assert_eq!(selected.key.as_ref(), Some(&key));
    assert!(matches!(&selected.failure, Some(TrapType::Interrupt(kind)) if *kind == admission));
    assert!(
        selected
            .writer_completion
            .as_ref()
            .unwrap()
            .ptr_eq(&receipt)
    );
    assert!(selected.matches_terminal_writer(7, &Some(key.clone()), admission, &receipt));
    assert!(!selected.matches_terminal_writer(7, &Some(key), monitor, &receipt));
    let interrupts = Interrupts::default();
    published(&interrupts, monitor).await;
    let mut guard = interrupts.lock().await;
    assert!(!guard.claim_written_terminal(admission));
    assert!(matches!(&*guard, WorkerInterruptState::Pending(pending) if pending.kind == monitor));
    assert_eq!(guard.take().unwrap().kind, monitor);
    assert!(
        unloaded_lifecycle_marker(
            monitor,
            AgentStatus::Suspended,
            7,
            established,
            Some(selected)
        )
        .unwrap()
        .is_none()
    );
    assert!(
        matches!(&*guard, WorkerInterruptState::TerminalClaimed(pending) if pending.kind == monitor)
    );
    assert!(guard.take().is_none());
}

#[test]
async fn unwritten_idle_status_wrong_generation_or_missing_receipt_requires_suspend() {
    let admission = InterruptKind::Suspend(Timestamp::from(211_u64));
    let later = InterruptKind::Suspend(Timestamp::from(212_u64));
    let (selected, writer, _) = selected_suspend(7, admission).await;
    writer.complete();
    for status in [
        AgentStatus::Idle,
        AgentStatus::Running,
        AgentStatus::Retrying,
    ] {
        assert!(matches!(
            unloaded_lifecycle_marker(later, status, 7, Some(7), selected.as_ref()).unwrap(),
            Some(OplogEntry::Suspend { .. })
        ));
    }
    assert!(matches!(
        unloaded_lifecycle_marker(later, AgentStatus::Suspended, 8, Some(8), selected.as_ref())
            .unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));
    assert!(matches!(
        unloaded_lifecycle_marker(later, AgentStatus::Suspended, 7, Some(7), None).unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));
    assert!(matches!(
        unloaded_lifecycle_marker(later, AgentStatus::Suspended, 7, Some(8), selected.as_ref())
            .unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));
    let unclaimed_receipt = SelectedInvocationOutcome {
        generation: 7,
        key: selected.as_ref().unwrap().key.clone(),
        failure: Some(TrapType::Interrupt(admission)),
        writer_claimed: false,
        writer_completion: selected.as_ref().unwrap().writer_completion.clone(),
    };
    assert!(matches!(
        unloaded_lifecycle_marker(
            later,
            AgentStatus::Suspended,
            7,
            Some(7),
            Some(&unclaimed_receipt)
        )
        .unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));
    let missing_receipt = SelectedInvocationOutcome {
        generation: 7,
        key: Some(IdempotencyKey::fresh()),
        failure: Some(TrapType::Interrupt(admission)),
        writer_claimed: true,
        writer_completion: None,
    };
    assert!(matches!(
        unloaded_lifecycle_marker(
            later,
            AgentStatus::Suspended,
            7,
            Some(7),
            Some(&missing_receipt)
        )
        .unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));

    let interrupts = Interrupts::default();
    let key = IdempotencyKey::fresh();
    let mut success = None;
    let mut guard = interrupts.lock().await;
    let writer = claim_success::<Context>(
        &mut success,
        &mut guard,
        context(AgentMode::Durable, 7, &key),
        |_| None,
    )
    .unwrap();
    writer.complete();
    assert!(matches!(
        unloaded_lifecycle_marker(later, AgentStatus::Idle, 7, Some(7), success.as_ref()).unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));
    assert!(matches!(
        unloaded_lifecycle_marker(later, AgentStatus::Suspended, 7, Some(7), success.as_ref())
            .unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));
    assert_eq!(success.as_ref().unwrap().key.as_ref(), Some(&key));
    assert!(success.as_ref().unwrap().failure.is_none());
    drop(guard);

    published(&interrupts, admission).await;
    let mut guard = interrupts.lock().await;
    let mut reserved = None;
    assert!(
        claim_success::<Context>(
            &mut reserved,
            &mut guard,
            context(AgentMode::Durable, 7, &key),
            |_| None
        )
        .is_err()
    );
    assert!(!reserved.as_ref().unwrap().writer_claimed);
    assert!(reserved.as_ref().unwrap().writer_completion.is_none());
    assert!(matches!(
        unloaded_lifecycle_marker(later, AgentStatus::Suspended, 7, Some(7), reserved.as_ref())
            .unwrap(),
        Some(OplogEntry::Suspend { .. })
    ));
}

#[test]
async fn distinct_interrupt_and_independent_outcome_do_not_reuse_suspend_ownership() {
    let admission = InterruptKind::Suspend(Timestamp::from(221_u64));
    let (selected, writer, _) = selected_suspend(7, admission).await;
    writer.complete();
    assert!(matches!(
        unloaded_lifecycle_marker(
            InterruptKind::Interrupt(Timestamp::from(222_u64)),
            AgentStatus::Suspended,
            7,
            Some(7),
            selected.as_ref()
        )
        .unwrap(),
        Some(OplogEntry::Interrupted { .. })
    ));
    for (origin, trap) in [
        (InvocationFailureOrigin::Independent, guest_trap()),
        (
            InvocationFailureOrigin::InvocationDeadline,
            TrapType::Error {
                error: AgentError::InternalError("invocation timer elapsed".into()),
                retry_from: OplogIndex::INITIAL,
                in_atomic_region: false,
                atomic_region_had_side_effects: false,
                semantic_trap_retry_override: None,
            },
        ),
    ] {
        let interrupts = Interrupts::default();
        let mut guard = interrupts.lock().await;
        let mut selected = None;
        let (_, writer) = claim_failure::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 7, &IdempotencyKey::fresh()),
            &trap,
            origin,
            None,
            |_| None,
        )
        .unwrap();
        writer.complete();
        assert!(matches!(
            unloaded_lifecycle_marker(
                admission,
                AgentStatus::Suspended,
                7,
                Some(7),
                selected.as_ref()
            )
            .unwrap(),
            Some(OplogEntry::Suspend { .. })
        ));
        match origin {
            InvocationFailureOrigin::Independent => {
                assert!(matches!(&selected.as_ref().unwrap().failure,
                Some(TrapType::Error { error: AgentError::DeterministicTrap(message), .. }) if message == "unreachable"))
            }
            InvocationFailureOrigin::InvocationDeadline => {
                assert!(matches!(&selected.as_ref().unwrap().failure,
                Some(TrapType::Error { error: AgentError::InternalError(message), .. }) if message == "invocation timer elapsed"))
            }
            InvocationFailureOrigin::Lifecycle => unreachable!(),
        }
    }
}

#[test]
async fn refused_lost_and_pending_lifecycle_receipts_remain_typed_failures() {
    let admission = InterruptKind::Suspend(Timestamp::from(231_u64));
    let later = InterruptKind::Suspend(Timestamp::from(232_u64));
    for state in ["refused", "lost", "pending"] {
        let (selected, writer, _) = selected_suspend(7, admission).await;
        let mut writer = Some(writer);
        let fence = OplogFence {
            agent_id: AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: "marker-writer".into(),
            },
            expected_epoch: ShardEpoch(3),
            actual_epoch: Some(ShardEpoch(4)),
        };
        match state {
            "refused" => writer.take().unwrap().refused(fence.clone()),
            "lost" => drop(writer.take()),
            _ => {}
        }
        let error =
            unloaded_lifecycle_marker(later, AgentStatus::Suspended, 7, Some(7), selected.as_ref())
                .unwrap_err();
        let retirement = crate::worker::RetirementReason::of_error(&error);
        if state == "refused" {
            assert!(
                matches!(retirement, Some(crate::worker::RetirementReason::Fenced(Some(actual))) if actual == fence)
            );
        } else {
            assert!(
                retirement.is_none(),
                "writer failure must not invent a lost shard"
            );
        }
        match state {
            "refused" => assert!(
                matches!(error, WorkerExecutorError::OplogFenced { agent_id, expected_epoch: 3, actual_epoch: Some(4) } if agent_id == fence.agent_id)
            ),
            "lost" => assert!(
                matches!(error, WorkerExecutorError::Runtime { details } if details == "Invocation outcome writer was lost")
            ),
            _ => assert!(
                matches!(error, WorkerExecutorError::Runtime { details } if details == "Lifecycle marker writer is still pending")
            ),
        }
        if let Some(writer) = writer {
            assert!(
                selected
                    .as_ref()
                    .unwrap()
                    .writer_completion
                    .clone()
                    .unwrap()
                    .now_or_never()
                    .is_none()
            );
            writer.complete();
            assert!(
                unloaded_lifecycle_marker(
                    later,
                    AgentStatus::Suspended,
                    7,
                    Some(7),
                    selected.as_ref()
                )
                .unwrap()
                .is_none()
            );
        }
    }
}

async fn published(interrupts: &Interrupts, kind: InterruptKind) {
    let mut guard = interrupts.lock().await;
    assert!(guard.queue(demand(kind)));
    guard.freeze();
    guard.publish();
}

async fn retained_publication(
    interrupts: &Interrupts,
    kind: InterruptKind,
    freezing: bool,
) -> (
    StopProgress,
    Arc<StopPublication>,
    oneshot::Sender<Result<(), WorkerExecutorError>>,
) {
    {
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(demand(kind)));
        if freezing {
            guard.freeze();
        }
    }
    let publication = Arc::new(StopPublication {
        release_scope: Default::default(),
        cause: OnceLock::new(),
        receipt: Arc::new(tokio::sync::broadcast::channel(1).0),
        signal: None,
        published: tokio::sync::Notify::new(),
    });
    let (send, receive) = oneshot::channel();
    let completion = async move { receive.await.unwrap() }.boxed().shared();
    let progress: StopProgress = Arc::default();
    progress.lock().unwrap().publication = Some((publication.clone(), completion));
    (progress, publication, send)
}

#[test]
async fn published_terminal_owns_success_cutoff_and_reserved_failure_writer() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for exhaustion in [
            MonthlyResourceExhaustion::Compute,
            MonthlyResourceExhaustion::Memory,
            MonthlyResourceExhaustion::DurableStorage,
            MonthlyResourceExhaustion::EphemeralStorage,
        ] {
            let interrupts = Interrupts::default();
            let kind = InterruptKind::Suspend(Timestamp::from(100_u64));
            published(&interrupts, kind).await;
            let key = IdempotencyKey::fresh();
            let mut selected = None;
            let config = GolemConfig::default();
            let map =
                |kind| monthly_interrupt_error(kind, mode, &Some((kind, exhaustion)), &config);
            let mut guard = interrupts.lock().await;
            let result =
                claim_success::<Context>(&mut selected, &mut guard, context(mode, 7, &key), map);
            assert!(result.is_err());
            let reserved = selected.as_ref().unwrap();
            assert_eq!(reserved.generation, 7);
            assert_eq!(reserved.key.as_ref(), Some(&key));
            assert!(!reserved.writer_claimed);
            assert!(reserved.writer_completion.is_none());
            if mode == AgentMode::Durable {
                assert!(
                    matches!(&reserved.failure, Some(TrapType::Interrupt(actual)) if *actual == kind)
                );
            } else {
                assert!(matches!(&reserved.failure, Some(TrapType::Error { .. })));
            }
            let (_, writer) = claim_failure::<Context>(
                &mut selected,
                &mut guard,
                context(mode, 7, &key),
                &guest_trap(),
                InvocationFailureOrigin::Independent,
                None,
                map,
            )
            .unwrap();
            assert!(selected.as_ref().unwrap().writer_claimed);
            let completion = selected
                .as_ref()
                .unwrap()
                .writer_completion
                .clone()
                .unwrap();
            assert!(completion.clone().now_or_never().is_none());
            assert!(
                claim_failure::<Context>(
                    &mut selected,
                    &mut guard,
                    context(mode, 7, &key),
                    &guest_trap(),
                    InvocationFailureOrigin::Independent,
                    None,
                    map
                )
                .is_none()
            );
            writer.complete();
            assert_eq!(
                completion.await.unwrap(),
                InvocationOutcomeVerdict::Committed
            );
        }
    }
}

#[test]
async fn actual_claim_preserves_independent_guest_trap_but_elects_deadline_and_lifecycle_causes() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for origin in [
            InvocationFailureOrigin::Independent,
            InvocationFailureOrigin::InvocationDeadline,
            InvocationFailureOrigin::Lifecycle,
        ] {
            let interrupts = Interrupts::default();
            let kind = InterruptKind::Interrupt(Timestamp::from(10_u64));
            published(&interrupts, kind).await;
            let mut guard = interrupts.lock().await;
            let mut selected = None;
            let key = IdempotencyKey::fresh();
            let trap = guest_trap();
            let (actual, writer) = claim_failure::<Context>(
                &mut selected,
                &mut guard,
                context(mode, 3, &key),
                &trap,
                origin,
                None,
                |_| None,
            )
            .unwrap();
            if matches!(origin, InvocationFailureOrigin::Independent) {
                assert!(
                    matches!(actual, TrapType::Error { error: AgentError::DeterministicTrap(ref message), .. } if message == "unreachable")
                );
            } else {
                assert!(matches!(actual, TrapType::Interrupt(actual) if actual == kind));
            }
            let completion = selected
                .as_ref()
                .unwrap()
                .writer_completion
                .clone()
                .unwrap();
            assert!(guard.take().is_none());
            writer.complete();
            assert_eq!(
                completion.await.unwrap(),
                InvocationOutcomeVerdict::Committed
            );
        }
    }
}

#[test]
async fn unpublished_and_freezing_terminal_wait_for_actual_publication() {
    for freezing in [false, true] {
        let interrupts = Interrupts::default();
        let kind = InterruptKind::Suspend(Timestamp::from(20_u64));
        let (progress, publication, send) = retained_publication(&interrupts, kind, freezing).await;
        let mut wait = Box::pin(lock_for_outcome(&interrupts, &progress));
        assert!(futures::poll!(wait.as_mut()).is_pending());
        assert!(
            interrupts.try_lock().is_some(),
            "publication wait must release the real interrupt guard"
        );
        {
            let mut guard = interrupts.lock().await;
            guard.freeze();
            guard.publish();
        }
        publication.published.notify_waiters();
        let mut guard = wait.await.unwrap();
        assert_eq!(guard.claim_pending_terminal().unwrap().kind, kind);
        send.send(Ok(())).unwrap();
    }
}

#[test]
async fn exhausted_admission_cannot_replace_frozen_explicit_interrupt_or_its_single_writer() {
    let entry = AtomicResourceEntry::new_with_monthly_policy(
        golem_service_base::model::MonthlyResourcePolicy {
            period: golem_common::model::account_usage::AccountUsagePeriod::current(),
            mode: golem_common::model::account_usage::MonthlyUsageMode::HardLimit,
            available_fuel: 0,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        usize::MAX,
        usize::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
    );
    let interrupts = Interrupts::default();
    let accepted = InterruptKind::Interrupt(Timestamp::from(21_u64));
    let (progress, publication, send) = retained_publication(&interrupts, accepted, true).await;
    assert_eq!(
        monthly_resource_admission(&entry, AgentMode::Durable),
        MonthlyResourceAdmission::Suspend
    );
    let candidate = TrapType::Interrupt(InterruptKind::Suspend(Timestamp::from(22_u64)));
    let key = IdempotencyKey::fresh();
    let mut selected = None;
    let mut wait = Box::pin(lock_for_failure(
        &interrupts,
        &progress,
        InvocationFailureOrigin::Lifecycle,
    ));
    assert!(futures::poll!(wait.as_mut()).is_pending());
    {
        let mut guard = interrupts
            .try_lock()
            .expect("admission wait releases the real interrupt guard");
        assert!(
            matches!(&*guard, WorkerInterruptState::Freezing(pending) if pending.kind == accepted)
        );
        assert!(!guard.queue(demand(InterruptKind::Suspend(Timestamp::from(23_u64)))));
        assert!(
            guard.freeze().is_none(),
            "the accepted cause was already frozen"
        );
        assert!(
            matches!(&*guard, WorkerInterruptState::Freezing(pending) if pending.kind == accepted)
        );
        assert!(selected.is_none(), "no writer before actual publication");
        guard.publish();
    }
    publication.published.notify_waiters();
    let (mut guard, publication_error) = wait.await;
    assert!(publication_error.is_none());
    let (actual, writer) = claim_failure::<Context>(
        &mut selected,
        &mut guard,
        context(AgentMode::Durable, 8, &key),
        &candidate,
        InvocationFailureOrigin::Lifecycle,
        publication_error,
        |_| None,
    )
    .unwrap();
    assert!(matches!(actual, TrapType::Interrupt(kind) if kind == accepted));
    let outcome = selected.as_ref().unwrap();
    assert_eq!(outcome.generation, 8);
    assert_eq!(outcome.key.as_ref(), Some(&key));
    assert!(matches!(&outcome.failure, Some(TrapType::Interrupt(kind)) if *kind == accepted));
    assert!(outcome.writer_claimed);
    let completion = outcome.writer_completion.clone().unwrap();
    assert!(completion.clone().now_or_never().is_none());
    assert!(
        guard.take().is_none(),
        "the elected terminal is claimed exactly once"
    );
    assert!(
        claim_failure::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 8, &key),
            &candidate,
            InvocationFailureOrigin::Lifecycle,
            None,
            |_| None,
        )
        .is_none()
    );
    assert!(
        completion.ptr_eq(
            selected
                .as_ref()
                .unwrap()
                .writer_completion
                .as_ref()
                .unwrap()
        )
    );
    writer.complete();
    assert_eq!(
        completion.await.unwrap(),
        InvocationOutcomeVerdict::Committed
    );
    send.send(Ok(())).unwrap();
}

#[test]
async fn failure_provenance_uses_actual_publication_wait_not_a_private_selector() {
    for origin in [
        InvocationFailureOrigin::Independent,
        InvocationFailureOrigin::InvocationDeadline,
        InvocationFailureOrigin::Lifecycle,
    ] {
        let interrupts = Interrupts::default();
        let kind = InterruptKind::Suspend(Timestamp::from(30_u64));
        let (progress, publication, send) = retained_publication(&interrupts, kind, true).await;
        let mut wait = Box::pin(lock_for_failure(&interrupts, &progress, origin));
        if matches!(origin, InvocationFailureOrigin::Independent) {
            let (guard, error) = wait.await;
            assert!(error.is_none());
            assert!(
                matches!(&*guard, WorkerInterruptState::Freezing(pending) if pending.kind == kind)
            );
            drop(guard);
        } else {
            assert!(futures::poll!(wait.as_mut()).is_pending());
            interrupts.lock().await.publish();
            publication.published.notify_waiters();
            let (guard, error) = wait.await;
            assert!(error.is_none());
            assert!(
                matches!(&*guard, WorkerInterruptState::Pending(pending) if pending.kind == kind)
            );
        }
        send.send(Ok(())).unwrap();
    }
}

#[test]
async fn publication_failure_remains_infrastructure_not_a_quota_cause() {
    let interrupts = Interrupts::default();
    let kind = InterruptKind::Suspend(Timestamp::from(40_u64));
    let (progress, _publication, send) = retained_publication(&interrupts, kind, true).await;
    let mut wait = Box::pin(lock_for_failure(
        &interrupts,
        &progress,
        InvocationFailureOrigin::Lifecycle,
    ));
    assert!(futures::poll!(wait.as_mut()).is_pending());
    let error = WorkerExecutorError::runtime("publisher failed");
    send.send(Err(error.clone())).unwrap();
    let (mut guard, failure) = wait.await;
    assert_eq!(failure, Some(error));
    let key = IdempotencyKey::fresh();
    let mut selected = None;
    let (actual, writer) = claim_failure::<Context>(
        &mut selected,
        &mut guard,
        context(AgentMode::Durable, 1, &key),
        &TrapType::Interrupt(kind),
        InvocationFailureOrigin::Lifecycle,
        failure,
        |_| None,
    )
    .unwrap();
    assert!(matches!(actual, TrapType::Error { .. }));
    writer.complete();
}

#[test]
async fn matching_selected_writer_stays_authoritative_while_generation_and_key_are_exact() {
    let interrupts = Interrupts::default();
    let mut guard = interrupts.lock().await;
    let key = IdempotencyKey::fresh();
    let mut selected = None;
    let writer = claim_success::<Context>(
        &mut selected,
        &mut guard,
        context(AgentMode::Durable, 1, &key),
        |_| None,
    )
    .unwrap();
    let completion = selected
        .as_ref()
        .unwrap()
        .writer_completion
        .clone()
        .unwrap();
    assert!(
        claim_success::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 1, &key),
            |_| None
        )
        .is_err()
    );
    assert!(
        claim_failure::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 1, &key),
            &guest_trap(),
            InvocationFailureOrigin::Independent,
            None,
            |_| None
        )
        .is_none()
    );
    assert!(
        completion.ptr_eq(
            selected
                .as_ref()
                .unwrap()
                .writer_completion
                .as_ref()
                .unwrap()
        )
    );
    writer.complete();
    assert_eq!(
        completion.await.unwrap(),
        InvocationOutcomeVerdict::Committed
    );
    let next = claim_success::<Context>(
        &mut selected,
        &mut guard,
        context(AgentMode::Durable, 2, &key),
        |_| None,
    )
    .unwrap();
    assert_eq!(selected.as_ref().unwrap().generation, 2);
    next.complete();
    let other = IdempotencyKey::fresh();
    let next = claim_success::<Context>(
        &mut selected,
        &mut guard,
        context(AgentMode::Durable, 2, &other),
        |_| None,
    )
    .unwrap();
    assert_eq!(selected.as_ref().unwrap().key.as_ref(), Some(&other));
    next.complete();
}

#[test]
async fn actual_writer_receipt_keeps_commit_refusal_and_loss_distinct() {
    for verdict in 0..3 {
        let interrupts = Interrupts::default();
        let mut guard = interrupts.lock().await;
        let key = IdempotencyKey::fresh();
        let mut selected = None;
        let writer = claim_success::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 1, &key),
            |_| None,
        )
        .unwrap();
        let completion = selected
            .as_ref()
            .unwrap()
            .writer_completion
            .clone()
            .unwrap();
        assert!(completion.clone().now_or_never().is_none());
        let fence = OplogFence {
            agent_id: AgentId {
                component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
                agent_id: "writer".into(),
            },
            expected_epoch: ShardEpoch(1),
            actual_epoch: Some(ShardEpoch(2)),
        };
        match verdict {
            0 => {
                writer.complete();
                assert_eq!(
                    completion.await.unwrap(),
                    InvocationOutcomeVerdict::Committed
                );
            }
            1 => {
                writer.refused(fence.clone());
                assert_eq!(
                    completion.await.unwrap(),
                    InvocationOutcomeVerdict::Refused(fence)
                );
            }
            _ => {
                drop(writer);
                assert!(completion.await.is_err());
            }
        }
        assert!(selected.as_ref().unwrap().writer_claimed);
    }
}

#[test]
async fn nonterminal_restart_does_not_acquire_a_terminal_publication_fence() {
    let interrupts = Interrupts::default();
    assert!(
        interrupts
            .lock()
            .await
            .queue(demand(InterruptKind::Restart))
    );
    let progress: StopProgress = Arc::default();
    let mut wait = Box::pin(lock_for_outcome(&interrupts, &progress));
    assert!(futures::poll!(wait.as_mut()).is_ready());
}

#[test]
async fn freezing_duplicate_lifecycle_claim_joins_publication_without_replacing_selected_writer() {
    let interrupts = Interrupts::default();
    let key = IdempotencyKey::fresh();
    let kind = InterruptKind::Suspend(Timestamp::from(60_u64));
    let mut selected = None;
    let (_, writer) = claim_failure::<Context>(
        &mut selected,
        &mut interrupts.lock().await,
        context(AgentMode::Durable, 9, &key),
        &TrapType::Interrupt(kind),
        InvocationFailureOrigin::Lifecycle,
        None,
        |_| None,
    )
    .unwrap();
    let completion = selected
        .as_ref()
        .unwrap()
        .writer_completion
        .clone()
        .unwrap();
    let (progress, publication, send) = retained_publication(&interrupts, kind, true).await;
    let mut wait = Box::pin(lock_for_failure(
        &interrupts,
        &progress,
        InvocationFailureOrigin::Lifecycle,
    ));
    assert!(futures::poll!(wait.as_mut()).is_pending());
    assert!(completion.clone().now_or_never().is_none());
    assert!(
        completion.ptr_eq(
            selected
                .as_ref()
                .unwrap()
                .writer_completion
                .as_ref()
                .unwrap()
        )
    );
    interrupts.lock().await.publish();
    publication.published.notify_waiters();
    let (mut guard, error) = wait.await;
    assert!(error.is_none());
    assert!(
        claim_failure::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 9, &key),
            &TrapType::Interrupt(kind),
            InvocationFailureOrigin::Lifecycle,
            None,
            |_| None
        )
        .is_none()
    );
    assert!(matches!(&*guard, WorkerInterruptState::Pending(pending) if pending.kind == kind));
    assert!(
        completion.ptr_eq(
            selected
                .as_ref()
                .unwrap()
                .writer_completion
                .as_ref()
                .unwrap()
        )
    );
    writer.complete();
    assert_eq!(
        completion.await.unwrap(),
        InvocationOutcomeVerdict::Committed
    );
    assert!(
        claim_failure::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 9, &key),
            &TrapType::Interrupt(kind),
            InvocationFailureOrigin::Lifecycle,
            None,
            |_| None
        )
        .is_none()
    );
    send.send(Ok(())).unwrap();
}

#[test]
async fn absent_invocation_key_still_has_one_exact_generation_writer() {
    let interrupts = Interrupts::default();
    let mut guard = interrupts.lock().await;
    let mut selected = None;
    let writer = claim_success::<Context>(
        &mut selected,
        &mut guard,
        OutcomeContext {
            generation: 4,
            key: None,
            mode: AgentMode::Durable,
        },
        |_| None,
    )
    .unwrap();
    assert!(
        claim_success::<Context>(
            &mut selected,
            &mut guard,
            OutcomeContext {
                generation: 4,
                key: None,
                mode: AgentMode::Durable
            },
            |_| None
        )
        .is_err()
    );
    assert!(
        claim_failure::<Context>(
            &mut selected,
            &mut guard,
            OutcomeContext {
                generation: 4,
                key: None,
                mode: AgentMode::Durable
            },
            &guest_trap(),
            InvocationFailureOrigin::Independent,
            None,
            |_| None
        )
        .is_none()
    );
    writer.complete();
}

#[test]
async fn deadline_and_lifecycle_candidates_keep_the_typed_published_monthly_cause() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for origin in [
            InvocationFailureOrigin::InvocationDeadline,
            InvocationFailureOrigin::Lifecycle,
        ] {
            for exhaustion in [
                MonthlyResourceExhaustion::Compute,
                MonthlyResourceExhaustion::Memory,
                MonthlyResourceExhaustion::EphemeralStorage,
            ] {
                let interrupts = Interrupts::default();
                let kind = InterruptKind::Suspend(Timestamp::from(70_u64));
                published(&interrupts, kind).await;
                let mut guard = interrupts.lock().await;
                let key = IdempotencyKey::fresh();
                let mut selected = None;
                let candidate = if matches!(origin, InvocationFailureOrigin::InvocationDeadline) {
                    TrapType::Error {
                        error: AgentError::InternalError("invocation timer elapsed".into()),
                        retry_from: OplogIndex::INITIAL,
                        in_atomic_region: false,
                        atomic_region_had_side_effects: false,
                        semantic_trap_retry_override: None,
                    }
                } else {
                    TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::from(71_u64)))
                };
                let config = GolemConfig::default();
                let map = |actual| {
                    monthly_interrupt_error(actual, mode, &Some((kind, exhaustion)), &config)
                };
                let (actual, writer) = claim_failure::<Context>(
                    &mut selected,
                    &mut guard,
                    context(mode, 2, &key),
                    &candidate,
                    origin,
                    None,
                    map,
                )
                .unwrap();
                if mode == AgentMode::Durable {
                    assert!(matches!(actual, TrapType::Interrupt(selected) if selected == kind));
                } else {
                    match (exhaustion, actual) {
                        (
                            MonthlyResourceExhaustion::Compute,
                            TrapType::Error {
                                error: AgentError::EphemeralFuelExhausted(_),
                                ..
                            },
                        ) => {}
                        (
                            MonthlyResourceExhaustion::Memory
                            | MonthlyResourceExhaustion::EphemeralStorage,
                            TrapType::Error {
                                error: AgentError::EphemeralCannotSuspend(reason),
                                ..
                            },
                        ) => {
                            assert_eq!(reason.reason, exhaustion.reason());
                        }
                        _ => panic!("published monthly cause lost its typed error"),
                    }
                }
                writer.complete();
            }
        }
    }
}

#[test]
async fn deadline_selected_first_retains_its_writer_through_a_later_published_stop() {
    let interrupts = Interrupts::default();
    let key = IdempotencyKey::fresh();
    let mut selected = None;
    let deadline = TrapType::Error {
        error: AgentError::InternalError("invocation timer elapsed".into()),
        retry_from: OplogIndex::INITIAL,
        in_atomic_region: false,
        atomic_region_had_side_effects: false,
        semantic_trap_retry_override: None,
    };
    let (_, writer) = claim_failure::<Context>(
        &mut selected,
        &mut interrupts.lock().await,
        context(AgentMode::Durable, 5, &key),
        &deadline,
        InvocationFailureOrigin::InvocationDeadline,
        None,
        |_| None,
    )
    .unwrap();
    let completion = selected
        .as_ref()
        .unwrap()
        .writer_completion
        .clone()
        .unwrap();
    let kind = InterruptKind::Suspend(Timestamp::from(80_u64));
    published(&interrupts, kind).await;
    let mut guard = interrupts.lock().await;
    assert!(
        claim_failure::<Context>(
            &mut selected,
            &mut guard,
            context(AgentMode::Durable, 5, &key),
            &TrapType::Interrupt(kind),
            InvocationFailureOrigin::Lifecycle,
            None,
            |_| None
        )
        .is_none()
    );
    assert!(matches!(&selected.as_ref().unwrap().failure,
        Some(TrapType::Error { error: AgentError::InternalError(message), .. }) if message == "invocation timer elapsed"));
    assert!(
        completion.ptr_eq(
            selected
                .as_ref()
                .unwrap()
                .writer_completion
                .as_ref()
                .unwrap()
        )
    );
    assert!(completion.clone().now_or_never().is_none());
    writer.complete();
    assert_eq!(
        completion.await.unwrap(),
        InvocationOutcomeVerdict::Committed
    );
}

#[test]
fn monthly_mapping_requires_exact_accepted_kind_and_ephemeral_mode() {
    let config = GolemConfig::default();
    let kind = InterruptKind::Suspend(Timestamp::from(50_u64));
    let other = InterruptKind::Suspend(Timestamp::from(51_u64));
    for exhaustion in [
        MonthlyResourceExhaustion::Compute,
        MonthlyResourceExhaustion::Memory,
        MonthlyResourceExhaustion::DurableStorage,
        MonthlyResourceExhaustion::EphemeralStorage,
    ] {
        let accepted = Some((kind, exhaustion));
        assert!(monthly_interrupt_error(kind, AgentMode::Durable, &accepted, &config).is_none());
        assert!(monthly_interrupt_error(other, AgentMode::Ephemeral, &accepted, &config).is_none());
        let actual =
            monthly_interrupt_error(kind, AgentMode::Ephemeral, &accepted, &config).unwrap();
        let expected = crate::worker::invocation_loop::monthly_resource_exhausted_invocation_error(
            &config, exhaustion,
        );
        assert_eq!(actual, expected);
    }
}
