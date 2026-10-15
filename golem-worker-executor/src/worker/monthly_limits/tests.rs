use super::*;
use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
use crate::services::golem_config::ResourceUsageMeteringConfig;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::resource_usage_metering::ResourceUsageAccount;
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::{test, timeout};

struct MonthlyStopOwner {
    fingerprint: AgentFingerprint,
    start_attempt: Uuid,
    generation: u64,
    target: Arc<MonthlyWindowTarget>,
    current: Mutex<Option<Arc<MonthlyWindowTarget>>>,
    permit: Option<crate::services::active_agents::ConcurrentAgentPermit>,
    resources: Arc<super::super::instance::OwnerRuntimeResources>,
    interrupts: Arc<super::super::interrupt::Interrupts>,
    progress: super::super::StopProgress,
    monthly: Arc<
        Mutex<
            Option<(
                InterruptKind,
                crate::services::resource_limits::MonthlyResourceExhaustion,
            )>,
        >,
    >,
    selected: Arc<Mutex<Option<super::super::SelectedInvocationOutcome>>>,
}

#[test]
fn trace_context_requires_the_actual_active_monthly_target() {
    let target = Arc::new(MonthlyWindowTarget {
        diagnostic_window_id: 9,
        trace_origin: golem_common::tracing::TraceOrigin::none(),
        fingerprint: AgentFingerprint::new(),
        start_attempt: Uuid::new_v4(),
        resident_generation: 1,
        active: Mutex::new(true),
        #[cfg(feature = "test-utils")]
        panic: CancellationToken::new(),
        #[cfg(feature = "test-utils")]
        exited: CancellationToken::new(),
    });
    let current = Mutex::new(None);
    assert!(
        target.trace_if_current(&current).is_none(),
        "absent/all-off target cannot be fabricated"
    );
    *current.lock().unwrap() = Some(target.clone());
    let captured = target.trace_if_current(&current).unwrap();
    *target.active.lock().unwrap() = false;
    assert!(target.trace_if_current(&current).is_none());
    *target.active.lock().unwrap() = true;
    let replacement = Arc::new(MonthlyWindowTarget {
        // Diagnostic attributes never replace the real Arc identity check.
        diagnostic_window_id: 9,
        trace_origin: golem_common::tracing::TraceOrigin::none(),
        fingerprint: target.fingerprint,
        start_attempt: target.start_attempt,
        resident_generation: 2,
        active: Mutex::new(true),
        #[cfg(feature = "test-utils")]
        panic: CancellationToken::new(),
        #[cfg(feature = "test-utils")]
        exited: CancellationToken::new(),
    });
    *current.lock().unwrap() = Some(replacement.clone());
    assert!(target.trace_if_current(&current).is_none());
    assert_eq!(
        replacement
            .trace_if_current(&current)
            .unwrap()
            .resident_generation,
        2
    );
    *current.lock().unwrap() = None;
    assert!(target.trace_if_current(&current).is_none());
    assert_eq!(captured.id, 9);
    assert_eq!(captured.resident_generation, 1);
}

impl MonthlyStopOwner {
    fn new(
        entry: Arc<AtomicResourceEntry>,
        permit: crate::services::active_agents::ConcurrentAgentPermit,
        generation: u64,
    ) -> Self {
        let fingerprint = AgentFingerprint::new();
        let start_attempt = Uuid::new_v4();
        let target = Arc::new(MonthlyWindowTarget {
            diagnostic_window_id: 1,
            trace_origin: golem_common::tracing::TraceOrigin::none(),
            fingerprint,
            start_attempt,
            resident_generation: generation,
            active: Mutex::new(true),
            #[cfg(feature = "test-utils")]
            panic: CancellationToken::new(),
            #[cfg(feature = "test-utils")]
            exited: CancellationToken::new(),
        });
        Self {
            fingerprint,
            start_attempt,
            generation,
            current: Mutex::new(Some(target.clone())),
            target,
            permit: Some(permit),
            resources: Arc::new(super::super::instance::OwnerRuntimeResources::new(
                entry,
                Arc::new(std::sync::RwLock::new(
                    crate::model::ExecutionStatus::loading(AgentMode::Durable),
                )),
            )),
            interrupts: Arc::default(),
            progress: Arc::default(),
            monthly: Arc::default(),
            selected: Arc::default(),
        }
    }

    fn sample(&self) -> MonthlyCapacity {
        let mode = self
            .resources
            .execution_status()
            .read()
            .unwrap()
            .agent_mode();
        self.resources
            .with_monthly_capacity(mode, |capacity| capacity)
    }

    // Composes the production bodies, not Worker cache lookup or the lifecycle actor mailbox.
    fn register(
        &self,
        proposed: MonthlyCapacity,
        interrupts: &mut super::super::interrupt::InterruptGuard<'_>,
    ) -> Option<InterruptKind> {
        if !self.target.matches_current(
            self.fingerprint,
            self.start_attempt,
            self.generation,
            self.permit.is_some(),
            &self.current,
        ) {
            return None;
        }
        let active = self.target.active.lock().unwrap();
        if !*active {
            return None;
        }
        let mode = self
            .resources
            .execution_status()
            .read()
            .unwrap()
            .agent_mode();
        self.resources.with_monthly_capacity(mode, |current| {
            let (kind, admission) = queue_monthly_capacity(
                proposed,
                current,
                interrupts,
                &self.progress,
                &self.monthly,
            )?;
            self.retain(admission);
            Some(kind)
        })
    }

    fn retain(&self, mut admission: std::sync::MutexGuard<'_, super::super::StopProgressState>) {
        let (publication, leader) =
            super::super::stop::publication(&admission, &self.resources.execution_status());
        let is_leader = leader.is_none();
        let receipt = publication.release_scope.accepted_driver();
        let driver_publication = publication.clone();
        let driver_interrupts = self.interrupts.clone();
        let monthly = self.monthly.clone();
        let selected = self.selected.clone();
        let generation = self.generation;
        let attempt = self.start_attempt;
        let resources = self.resources.clone();
        let task = tokio::spawn(async move {
            if let Some(leader) = leader {
                super::super::stop::publish_follower(&driver_interrupts, leader).await?;
            } else {
                let (pending, writer) = super::super::stop::freeze_and_selected_writer(
                    &driver_interrupts,
                    &driver_publication,
                    &monthly,
                    &selected,
                    generation,
                )
                .await?;
                let committed = super::super::stop::join_selected_writer(writer).await?;
                let operations = crate::durable_host::tool::operation::OwnerToolOperations::new();
                crate::services::active_agents::EntityFence {
                    operations: &operations,
                    entities: &Mutex::new(Default::default()),
                    accepting: &AtomicBool::new(true),
                    generation: &std::sync::atomic::AtomicU64::new(0),
                    resources: &resources,
                }
                .begin(
                    crate::durable_host::tool::operation::OwnerFailureWinner::Lifecycle(
                        pending.kind,
                    ),
                )
                .await;
                super::super::stop::verify_owner_fence(&operations, pending.kind)?;
                let lifecycle = super::super::stop::tests::running_lifecycle(
                    driver_interrupts.clone(),
                    attempt,
                    true,
                );
                super::super::stop::publish(
                    super::super::stop::PublicationRuntime {
                        lifecycle: &lifecycle,
                        resident_generation: &std::sync::atomic::AtomicU64::new(generation),
                        accepted_generation: generation,
                        started_resident_attempt: Some(attempt),
                    },
                    &driver_interrupts,
                    &resources.execution_status(),
                    &selected,
                    &driver_publication,
                    pending,
                    committed.as_ref(),
                )
                .await?;
            }
            Ok(false)
        });
        drop(super::super::stop::retain_driver(
            &mut admission,
            publication,
            is_leader,
            true,
            receipt,
            task,
        ));
    }
}

const STOP_FUEL_BATCH: u64 = 1_000_000_000;

#[test]
#[timeout("5s")]
async fn monthly_supersedes_restart_and_unpublished_stop_owns_deadline_classification() {
    use crate::model::TrapType;
    use crate::services::resource_limits::MonthlyResourceExhaustion;
    use crate::worker::outcome::{
        OutcomeContext, claim_failure, lock_for_failure, monthly_interrupt_error,
    };
    use crate::worker::{InvocationFailureOrigin, WorkerInterruptState};
    use crate::workerctx::default::Context;
    use golem_common::model::oplog::AgentError;
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let (entry, owner, _target) = prepaid_stop_owners().await;
        let mut policy = stop_fuel_policy();
        policy.available_memory_gb_seconds = 0;
        assert!(entry.apply_monthly_snapshot_for_test(1, policy, 0));
        let proposed = owner.sample();
        assert_eq!(proposed.exhaustion, Some(MonthlyResourceExhaustion::Memory));
        let status = owner.resources.execution_status();
        status.write().unwrap().set_agent_mode(mode);
        let mut raw = match &*status.read().unwrap() {
            crate::model::ExecutionStatus::Loading {
                interrupt_signal, ..
            } => interrupt_signal.subscribe(),
            _ => unreachable!(),
        };
        let mut guard = owner.interrupts.lock().await;
        assert!(guard.queue(PendingWorkerInterrupt {
            kind: InterruptKind::Restart,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Restart)
        }));
        owner.retain(owner.progress.lock().unwrap());
        let accepted = owner.register(proposed, &mut guard).unwrap();
        assert_eq!(owner.progress.lock().unwrap().tasks.len(), 2);
        assert!(
            matches!(*guard, WorkerInterruptState::Unpublished(pending) if pending.kind == accepted)
        );
        assert!(
            owner
                .progress
                .lock()
                .unwrap()
                .publication
                .as_ref()
                .unwrap()
                .0
                .cause
                .get()
                .is_none()
        );
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        // The real publication guard orders a deadline-origin candidate behind the accepted stop.
        // Holding that guard lets this test poll the actual waiter before the driver can publish.
        let mut wait = Box::pin(lock_for_failure(
            &owner.interrupts,
            &owner.progress,
            InvocationFailureOrigin::InvocationDeadline,
        ));
        assert!(futures::poll!(wait.as_mut()).is_pending());
        drop(guard);
        let (mut guard, publication_error) = wait.await;
        assert!(publication_error.is_none());
        let key = golem_common::model::IdempotencyKey::fresh();
        let trap = TrapType::Error {
            error: AgentError::InternalError("invocation timer elapsed".into()),
            retry_from: golem_common::model::OplogIndex::INITIAL,
            in_atomic_region: false,
            atomic_region_had_side_effects: false,
            semantic_trap_retry_override: None,
        };
        let config = crate::services::golem_config::GolemConfig::default();
        let (selected, writer) = claim_failure::<Context>(
            &mut owner.selected.lock().unwrap(),
            &mut guard,
            OutcomeContext {
                generation: owner.generation,
                key: Some(key),
                mode,
            },
            &trap,
            InvocationFailureOrigin::InvocationDeadline,
            None,
            |kind| monthly_interrupt_error(kind, mode, &owner.monthly.lock().unwrap(), &config),
        )
        .unwrap();
        if mode == AgentMode::Durable {
            assert!(matches!(selected, TrapType::Interrupt(kind) if kind == accepted));
        } else {
            assert!(
                matches!(selected, TrapType::Error { error: AgentError::EphemeralCannotSuspend(reason), .. }
                if reason.reason == "monthly memory exhausted")
            );
        }
        writer.complete();
        drop(guard);
        super::super::join_accepted_stops(&owner.progress)
            .await
            .unwrap();
        assert_eq!(raw.recv().await.unwrap(), accepted);
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
}

fn stop_fuel_policy() -> golem_service_base::model::MonthlyResourcePolicy {
    golem_service_base::model::MonthlyResourcePolicy {
        period: golem_common::model::account_usage::AccountUsagePeriod::current(),
        mode: golem_common::model::account_usage::MonthlyUsageMode::HardLimit,
        available_fuel: 2 * STOP_FUEL_BATCH,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

async fn prepaid_stop_owners() -> (Arc<AtomicResourceEntry>, MonthlyStopOwner, MonthlyStopOwner) {
    let entry = Arc::new(AtomicResourceEntry::new_with_monthly_policy(
        stop_fuel_policy(),
        usize::MAX,
        usize::MAX,
        u64::MAX,
        2,
    ));
    let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
    let account = AccountId::new();
    scheduler.register_account(account, entry.clone()).await;
    let caller = scheduler
        .acquire(
            account,
            AgentId {
                component_id: ComponentId::new(),
                agent_id: "stop-caller".into(),
            },
        )
        .await;
    let target = scheduler
        .acquire(
            account,
            AgentId {
                component_id: ComponentId::new(),
                agent_id: "stop-target".into(),
            },
        )
        .await;
    (
        entry.clone(),
        MonthlyStopOwner::new(entry.clone(), caller, 7),
        MonthlyStopOwner::new(entry, target, 8),
    )
}

#[test]
#[timeout("10s")]
async fn two_prepaid_stop_registrations_precede_either_real_refund() {
    use crate::services::resource_limits::MonthlyResourceExhaustion;
    use crate::workerctx::default::tests::PrepaidStore;
    let (entry, caller, target) = prepaid_stop_owners().await;
    let mut caller_store = PrepaidStore::new(&caller.resources, STOP_FUEL_BATCH);
    let mut target_store = PrepaidStore::new(&target.resources, STOP_FUEL_BATCH);
    caller_store.borrow();
    target_store.borrow();
    caller_store.execute().await;
    target_store.execute().await;
    assert_eq!(entry.fuel_delta(), 2 * STOP_FUEL_BATCH as i64);
    assert!(caller.sample().exhaustion.is_none());
    assert!(target.sample().exhaustion.is_none());
    assert!(entry.apply_monthly_snapshot_for_test(1, stop_fuel_policy(), 0));
    let caller_proposal = caller.sample();
    let target_proposal = target.sample();
    assert_eq!(caller_proposal.fuel_generation, Some(0));
    assert_eq!(target_proposal.fuel_generation, Some(0));
    assert_eq!(
        caller_proposal.exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    assert_eq!(
        target_proposal.exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    let mut caller_guard = caller.interrupts.lock().await;
    let mut target_guard = target.interrupts.lock().await;
    let caller_kind = caller.register(caller_proposal, &mut caller_guard).unwrap();
    let target_kind = target.register(target_proposal, &mut target_guard).unwrap();
    for (owner, kind) in [(&caller, caller_kind), (&target, target_kind)] {
        let progress = owner.progress.lock().unwrap();
        assert_eq!(progress.tasks.len(), 1);
        assert!(progress.ready().is_none());
        let publication = &progress.publication.as_ref().unwrap().0;
        assert!(publication.cause.get().is_none());
        assert_eq!(
            *owner.monthly.lock().unwrap(),
            Some((kind, MonthlyResourceExhaustion::Compute))
        );
        assert!(matches!(
            publication.receipt.subscribe().try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
    // Both actual registrations exist while the real guards exclude driver freezing.
    assert_eq!(entry.fuel_delta(), 2 * STOP_FUEL_BATCH as i64);
    let target_consumed = target_store.refund();
    assert!(target_consumed > 0 && target_consumed < STOP_FUEL_BATCH);
    assert_eq!(
        entry.fuel_delta(),
        STOP_FUEL_BATCH as i64 + target_consumed as i64
    );
    assert!(caller.sample().exhaustion.is_none());
    let caller_consumed = caller_store.refund();
    assert!(caller_consumed > 0 && caller_consumed < STOP_FUEL_BATCH);
    assert_eq!(
        entry.fuel_delta(),
        (caller_consumed + target_consumed) as i64
    );
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    drop(caller_guard);
    drop(target_guard);
    for (owner, kind) in [(&caller, caller_kind), (&target, target_kind)] {
        super::super::join_accepted_stops(&owner.progress)
            .await
            .unwrap();
        let progress = owner.progress.lock().unwrap();
        assert_eq!(progress.tasks.len(), 1);
        assert!(progress.ready().unwrap().is_ok());
        assert_eq!(
            progress
                .publication
                .as_ref()
                .unwrap()
                .0
                .cause
                .get()
                .unwrap()
                .kind,
            kind
        );
        assert_eq!(
            *owner.monthly.lock().unwrap(),
            Some((kind, MonthlyResourceExhaustion::Compute))
        );
    }
}

#[test]
#[timeout("10s")]
async fn captured_exhausted_proposal_is_rejected_after_other_store_refund_with_same_identity() {
    use crate::workerctx::default::tests::PrepaidStore;
    let (entry, caller, target) = prepaid_stop_owners().await;
    let mut caller_store = PrepaidStore::new(&caller.resources, STOP_FUEL_BATCH);
    let mut target_store = PrepaidStore::new(&target.resources, STOP_FUEL_BATCH);
    caller_store.borrow();
    target_store.borrow();
    caller_store.execute().await;
    target_store.execute().await;
    assert!(entry.apply_monthly_snapshot_for_test(1, stop_fuel_policy(), 0));
    let proposed = caller.sample();
    assert!(proposed.exhaustion.is_some());
    let target_proposal = target.sample();
    let mut target_guard = target.interrupts.lock().await;
    assert!(
        target
            .register(target_proposal, &mut target_guard)
            .is_some()
    );
    let fingerprint = caller.fingerprint;
    let attempt = caller.start_attempt;
    let generation = caller.generation;
    let window = caller.current.lock().unwrap().as_ref().unwrap().clone();
    let mut raw = match &*caller.resources.execution_status().read().unwrap() {
        crate::model::ExecutionStatus::Loading {
            interrupt_signal, ..
        } => interrupt_signal.subscribe(),
        _ => panic!("body fixture must retain its original execution status"),
    };
    let before = entry.fuel_delta();
    let consumed = target_store.refund();
    assert!(consumed > 0 && consumed < STOP_FUEL_BATCH);
    assert_eq!(
        entry.fuel_delta() - before,
        -(STOP_FUEL_BATCH as i64 - consumed as i64)
    );
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    let restored = caller.sample();
    assert_eq!(restored.policy_revision, proposed.policy_revision);
    assert_eq!(restored.period, proposed.period);
    assert_eq!(restored.fuel_generation, proposed.fuel_generation);
    assert_eq!(restored.exhaustion, None);
    assert_eq!(caller.fingerprint, fingerprint);
    assert_eq!(caller.start_attempt, attempt);
    assert_eq!(caller.generation, generation);
    assert!(Arc::ptr_eq(
        caller.current.lock().unwrap().as_ref().unwrap(),
        &window
    ));
    assert!(caller.permit.is_some() && *caller.target.active.lock().unwrap());
    assert!(
        caller
            .target
            .matches_current(fingerprint, attempt, generation, true, &caller.current)
    );
    let mut guard = caller.interrupts.lock().await;
    assert!(caller.register(proposed, &mut guard).is_none());
    assert!(!guard.has_interrupt());
    {
        let progress = caller.progress.lock().unwrap();
        assert!(progress.tasks.is_empty());
        assert!(progress.publication.is_none());
        assert!(caller.monthly.lock().unwrap().is_none());
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            *caller.resources.execution_status().read().unwrap(),
            crate::model::ExecutionStatus::Loading { .. }
        ));
    }
    drop(guard);
    drop(target_guard);
    super::super::join_accepted_stops(&target.progress)
        .await
        .unwrap();
    let caller_consumed = caller_store.refund();
    assert_eq!(entry.fuel_delta(), (caller_consumed + consumed) as i64);
}

#[test]
#[timeout("5s")]
async fn monthly_target_identity_mismatch_does_not_read_current_window() {
    let (_entry, owner, _target) = prepaid_stop_owners().await;
    let current = Mutex::new(None::<Arc<MonthlyWindowTarget>>);
    assert!(
        std::panic::catch_unwind(|| {
            let _held = current.lock().unwrap();
            panic!("unused current window is poisoned");
        })
        .is_err()
    );
    for (fingerprint, attempt, generation, permit) in [
        (
            AgentFingerprint::new(),
            owner.start_attempt,
            owner.generation,
            true,
        ),
        (owner.fingerprint, Uuid::new_v4(), owner.generation, true),
        (
            owner.fingerprint,
            owner.start_attempt,
            owner.generation + 1,
            true,
        ),
        (
            owner.fingerprint,
            owner.start_attempt,
            owner.generation,
            false,
        ),
    ] {
        assert!(
            !owner
                .target
                .matches_current(fingerprint, attempt, generation, permit, &current)
        );
    }
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
}

#[test]
#[timeout("5s")]
async fn monthly_body_rejects_changed_identity_capacity_retirement_and_terminal_claim() {
    let (entry, owner, _target) = prepaid_stop_owners().await;
    {
        let available = owner.sample();
        assert!(available.exhaustion.is_none());
        let mut guard = owner.interrupts.lock().await;
        assert!(owner.register(available, &mut guard).is_none());
        assert!(!guard.has_interrupt());
    }
    let target = &owner.target;
    assert!(target.matches_current(
        owner.fingerprint,
        owner.start_attempt,
        owner.generation,
        true,
        &owner.current
    ));
    assert!(!target.matches_current(
        AgentFingerprint::new(),
        owner.start_attempt,
        owner.generation,
        true,
        &owner.current
    ));
    assert!(!target.matches_current(
        owner.fingerprint,
        Uuid::new_v4(),
        owner.generation,
        true,
        &owner.current
    ));
    assert!(!target.matches_current(
        owner.fingerprint,
        owner.start_attempt,
        owner.generation + 1,
        true,
        &owner.current
    ));
    assert!(!target.matches_current(
        owner.fingerprint,
        owner.start_attempt,
        owner.generation,
        false,
        &owner.current
    ));
    assert!(!target.matches_current(
        owner.fingerprint,
        owner.start_attempt,
        owner.generation,
        true,
        &Mutex::new(None)
    ));
    let equal_fields = Arc::new(MonthlyWindowTarget {
        diagnostic_window_id: owner.target.diagnostic_window_id,
        trace_origin: golem_common::tracing::TraceOrigin::none(),
        fingerprint: owner.fingerprint,
        start_attempt: owner.start_attempt,
        resident_generation: owner.generation,
        active: Mutex::new(true),
        #[cfg(feature = "test-utils")]
        panic: CancellationToken::new(),
        #[cfg(feature = "test-utils")]
        exited: CancellationToken::new(),
    });
    assert!(!target.matches_current(
        owner.fingerprint,
        owner.start_attempt,
        owner.generation,
        true,
        &Mutex::new(Some(equal_fields))
    ));
    let mut policy = stop_fuel_policy();
    policy.available_fuel = 0;
    assert!(entry.apply_monthly_snapshot_for_test(1, policy, 0));
    let current = owner.sample();
    assert!(current.exhaustion.is_some());
    let status = owner.resources.execution_status();
    let (publication, _) =
        super::super::stop::publication(&owner.progress.lock().unwrap(), &status);
    let mut raw = publication.signal.as_ref().unwrap().subscribe();
    let mut receipt = publication.receipt.subscribe();
    let mut guard = owner.interrupts.lock().await;
    let mut changes = [current; 4];
    changes[0].policy_revision += 1;
    changes[1].period.year += 1;
    changes[2].fuel_generation = Some(99);
    changes[3].exhaustion = None;
    for proposed in changes {
        assert!(owner.register(proposed, &mut guard).is_none());
        assert!(!guard.has_interrupt());
    }
    *owner.target.active.lock().unwrap() = false;
    assert!(owner.register(current, &mut guard).is_none());
    *owner.target.active.lock().unwrap() = true;
    owner.progress.lock().unwrap().retired = true;
    assert!(matches!(*guard, super::super::WorkerInterruptState::Idle));
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
    assert!(owner.progress.lock().unwrap().publication.is_none());
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(
        queue_monthly_capacity(
            current,
            owner.sample(),
            &mut guard,
            &owner.progress,
            &owner.monthly
        )
        .is_none()
    );
    assert!(matches!(*guard, super::super::WorkerInterruptState::Idle));
    assert!(owner.progress.lock().unwrap().retired);
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
    assert!(owner.progress.lock().unwrap().publication.is_none());
    assert!(publication.cause.get().is_none());
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    owner.progress.lock().unwrap().retired = false;
    let user = InterruptKind::Interrupt(Timestamp::from(30_u64));
    assert!(guard.queue(PendingWorkerInterrupt {
        kind: user,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt)
    }));
    guard.freeze();
    guard.publish();
    assert_eq!(guard.claim_pending_terminal().unwrap().kind, user);
    let claimed = match *guard {
        super::super::WorkerInterruptState::TerminalClaimed(claimed) => claimed,
        _ => panic!("user terminal must remain claimed"),
    };
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
    assert!(owner.progress.lock().unwrap().publication.is_none());
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(
        queue_monthly_capacity(
            current,
            owner.sample(),
            &mut guard,
            &owner.progress,
            &owner.monthly
        )
        .is_none()
    );
    assert!(
        matches!(*guard, super::super::WorkerInterruptState::TerminalClaimed(actual)
        if actual.kind == claimed.kind
            && actual.reacquire_permits == claimed.reacquire_permits
            && actual.unload_request.reason == claimed.unload_request.reason
            && actual.unload_request.deadline == claimed.unload_request.deadline)
    );
    assert!(!owner.progress.lock().unwrap().retired);
    assert!(publication.cause.get().is_none());
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
    assert!(owner.progress.lock().unwrap().publication.is_none());
}

#[cfg(feature = "test-utils")]
#[test]
async fn native_idle_observation_requires_pending_not_ready() {
    let observed = std::sync::atomic::AtomicUsize::new(0);
    observe_native_idle(std::future::ready(()), || {
        observed.fetch_add(1, Ordering::Relaxed);
    })
    .await;
    assert_eq!(observed.load(Ordering::Relaxed), 0);
    let (send, receive) = tokio::sync::oneshot::channel::<()>();
    let future = observe_native_idle(receive, || {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    tokio::pin!(future);
    assert!(futures::poll!(&mut future).is_pending());
    assert_eq!(observed.load(Ordering::Relaxed), 1);
    send.send(()).unwrap();
    future.await.unwrap();
    assert_eq!(observed.load(Ordering::Relaxed), 1);
}

async fn unmetered_window() -> (
    ExecutionWindow,
    Arc<AtomicBool>,
    crate::metrics::resource_release::tests::TestMetrics,
) {
    let (window, held, metrics, _, _) = unmetered_window_with_account().await;
    (window, held, metrics)
}

async fn unmetered_window_with_account() -> (
    ExecutionWindow,
    Arc<AtomicBool>,
    crate::metrics::resource_release::tests::TestMetrics,
    Arc<ConcurrentAgentsScheduler>,
    AccountId,
) {
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let memory = LinearMemoryTracker::new_with_metering(
        0,
        0,
        AgentMode::Durable,
        false,
        entry.clone(),
        Arc::new(Mutex::new(MemoryGrant::inert(0))),
        false,
    );
    let account = ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone());
    let meter = resource_usage_metering::create_unbound_meter(
        ResourceUsageMeteringConfig {
            compute: false,
            memory: false,
            filesystem: false,
        },
        account,
    );
    let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
    let account_id = AccountId(Uuid::new_v4());
    scheduler.register_account(account_id, entry).await;
    let permit = scheduler
        .acquire(
            account_id,
            AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "unmetered-close".to_string(),
            },
        )
        .await;
    let mut window = resource_usage_metering::open_window(&meter, permit)
        .await
        .unwrap();
    let held = Arc::new(AtomicBool::new(false));
    window.track_permit_for_test(held.clone());
    assert!(held.load(Ordering::Acquire));
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let window = ExecutionWindow::unmonitored_with_scope(window, scope.clone());
    scope.start(
        crate::metrics::resource_release::Origin::AcceptedStop,
        crate::metrics::resource_release::Cause::Interrupt,
    );
    scope.seal();
    (window, held, metrics, scheduler, account_id)
}

#[test]
#[timeout("5s")]
async fn published_stop_retains_window_until_real_entity_drain_completes() {
    use crate::durable_host::tool::operation::{
        BodyAdmissionState, OwnerFailureWinner, OwnerToolOperationContext, OwnerToolOperations,
        ToolLiveAdmissionOutcome,
    };
    use crate::model::ExecutionStatus;
    use crate::services::active_agents::EntityFence;
    use crate::worker::entity_slot::EntitySlot;
    use crate::worker::instance::OwnerRuntimeResources;
    use crate::worker::owner_lane::{EntityCallMode, OwnerInvocationId, OwnerLane};
    use crate::worker::stop::{self, PublicationRuntime};
    use golem_common::model::agent::{AgentPrincipal, Principal};
    use golem_common::model::entity::{
        EntityActivation, EntityActivationPolicy, EntityInvocationDescriptor, EntityInvocationId,
        EntityInvocationScope, ExecutableTarget, FilesystemCapability, InvocationExecutionMode,
        OwnedAgentEntityId, ToolInvocationDescriptor, ToolOutputContract,
    };
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::tool::{SecretKeyScope, ToolFilesystemAccess};
    use golem_common::model::{IdempotencyKey, OwnedAgentId};
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
    use std::sync::RwLock;
    use std::sync::atomic::AtomicU64;

    struct StoreData(Arc<AtomicBool>);
    impl Drop for StoreData {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    let (window, held, metrics, scheduler, account_id) = unmetered_window_with_account().await;
    let progress = window.stop_progress.clone();
    progress.lock().unwrap().release_scope = window.release_scope.clone();
    let owner = OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "publication-drain".into(),
        },
    );
    let activation = Arc::new(
        EntityActivation::new(
            ExecutableTarget::new(ComponentId::new(), 1_u64.try_into().unwrap()),
            1_u64.try_into().unwrap(),
            EntityActivationPolicy::ToolMiddleware {
                middleware_name: "publication-drain".try_into().unwrap(),
                provision: Default::default(),
                config_keys_readable: Default::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Allowed,
            },
            FilesystemCapability::Capable,
        )
        .unwrap(),
    );
    let entity = OwnedAgentEntityId {
        owner: owner.clone(),
        entity: activation.entity(),
    };
    let invocation = EntityInvocationId::new(entity.clone(), OplogIndex::from_u64(2)).unwrap();
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner.agent_id.clone(),
    });
    let scope = EntityInvocationScope::new(
        invocation.clone(),
        OplogIndex::from_u64(1),
        activation.clone(),
        principal.clone(),
        InvocationExecutionMode::Live,
        IdempotencyKey::fresh(),
        false,
        false,
        IdempotencyKey::fresh(),
    )
    .unwrap();
    let slot = Arc::new(EntitySlot::new(entity.clone()));
    let registration = slot.register(&scope, Default::default()).unwrap();
    let status = Arc::new(RwLock::new(ExecutionStatus::Running {
        agent_mode: AgentMode::Durable,
        timestamp: Timestamp::now_utc(),
        interrupt_signal: Arc::new(tokio::sync::broadcast::channel(8).0),
    }));
    let resources = Arc::new(OwnerRuntimeResources::new(
        Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1)),
        status.clone(),
    ));
    registration
        .attach_linear_memory(LinearMemoryTracker::new_with_metering(
            0,
            0,
            AgentMode::Durable,
            false,
            resources.resource_limits(),
            Arc::new(Mutex::new(MemoryGrant::inert(0))),
            false,
        ))
        .unwrap();
    let store_dropped = Arc::new(AtomicBool::new(false));
    let engine = wasmtime::Engine::default();
    let store =
        super::super::ObservedPrimaryStore::new(&engine, StoreData(store_dropped.clone()), None);

    let lane = OwnerLane::new(owner.clone());
    let parent = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
    let primary = lane
        .enter_primary(OplogIndex::from_u64(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let operations = OwnerToolOperations::new();
    let operation = operations
        .create(OwnerToolOperationContext {
            parent: parent.clone(),
            call_mode: EntityCallMode::Asynchronous,
            activation,
            principal,
            descriptor: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
                attempt_ordinal: 0,
                command_path: Vec::new(),
                args: Vec::new(),
                has_stdin: false,
                has_stdout: false,
                declares_stdout: false,
                has_stderr: false,
                declares_stderr: false,
                output_contract: ToolOutputContract {
                    result: None,
                    errors: Vec::new(),
                },
            }),
            input: TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
                SchemaValue::Tuple {
                    elements: Vec::new(),
                },
            ),
        })
        .accept(invocation.clone())
        .unwrap();
    assert_eq!(
        operation.activate_live_attachment_memory_accounting().await,
        ToolLiveAdmissionOutcome::Admitted
    );
    assert!(operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
    assert!(operation.register_body(&lane).unwrap());
    let parent_wait = lane
        .await_invocations(&parent, [OwnerInvocationId::Entity(invocation.clone())])
        .unwrap();
    assert!(operation.acquire_registered_body().await.unwrap());
    assert_eq!(
        lane.holder(),
        Some(OwnerInvocationId::Entity(invocation.clone()))
    );
    let entities = Arc::new(Mutex::new(std::collections::HashMap::from([(
        entity.entity,
        slot.clone(),
    )])));
    let interrupts = Arc::new(super::super::interrupt::Interrupts::default());
    let kind = InterruptKind::Interrupt(Timestamp::from(85_u64));
    assert!(interrupts.lock().await.queue(PendingWorkerInterrupt {
        kind,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
    }));
    let (drain_pending, observed_drain) = tokio::sync::oneshot::channel();
    let (publication, driver, mut raw, mut receipt) = {
        let mut admission = progress.lock().unwrap();
        let (publication, leader) = stop::publication(&admission, &status);
        assert!(leader.is_none());
        let raw = publication.signal.as_ref().unwrap().subscribe();
        let receipt = publication.receipt.subscribe();
        let captured = publication.clone();
        let captured_interrupts = interrupts.clone();
        let captured_status = status.clone();
        let captured_operations = operations.clone();
        let captured_entities = entities.clone();
        let captured_owner = owner.clone();
        let task = tokio::spawn(async move {
            let (pending, writer) = stop::freeze_and_selected_writer(
                &captured_interrupts,
                &captured,
                &Mutex::new(None),
                &Mutex::new(None),
                7,
            )
            .await?;
            let committed = stop::join_selected_writer(writer).await?;
            let accepting = AtomicBool::new(true);
            let fence_generation = AtomicU64::new(0);
            let fence = EntityFence {
                operations: &captured_operations,
                entities: &captured_entities,
                accepting: &accepting,
                generation: &fence_generation,
                resources: &resources,
            };
            fence
                .begin(OwnerFailureWinner::Lifecycle(pending.kind))
                .await;
            stop::verify_owner_fence(&captured_operations, pending.kind)?;
            let attempt = Uuid::new_v4();
            let lifecycle = super::super::stop::tests::running_lifecycle(
                captured_interrupts.clone(),
                attempt,
                true,
            );
            let acknowledge = stop::publish(
                PublicationRuntime {
                    lifecycle: &lifecycle,
                    resident_generation: &AtomicU64::new(7),
                    accepted_generation: 7,
                    started_resident_attempt: Some(attempt),
                },
                &captured_interrupts,
                &captured_status,
                &Mutex::new(None),
                &captured,
                pending,
                committed.as_ref(),
            )
            .await?;
            assert!(
                !acknowledge,
                "a Running Store owns acknowledgement; entity drain is not its public receipt"
            );
            let mut drain = Box::pin(fence.drain(&captured_owner));
            assert!(futures::poll!(&mut drain).is_pending());
            drain_pending.send(()).unwrap();
            drain.await;
            Ok(acknowledge)
        });
        let accepted = publication.release_scope.accepted_driver();
        let driver = stop::retain_driver(
            &mut admission,
            publication.clone(),
            true,
            true,
            accepted,
            task,
        );
        (publication, driver, raw, receipt)
    };
    assert_eq!(raw.recv().await.unwrap(), kind);
    observed_drain.await.unwrap();
    assert_eq!(publication.cause.get().unwrap().kind, kind);
    assert_eq!(slot.active_invocation_count(), 1);
    assert!(slot.active_invocations()[0].store_attached);
    assert!(registration.cancellation().is_cancelled());
    assert!(!store_dropped.load(Ordering::Acquire));
    assert_eq!(
        lane.holder(),
        Some(OwnerInvocationId::Entity(invocation.clone()))
    );
    assert!(
        entities.try_lock().is_ok(),
        "the actual drain cannot hold the slot-map mutex across await"
    );
    assert!(driver.clone().now_or_never().is_none());
    let mut joining = Box::pin(super::super::join_accepted_stops(&progress));
    assert!(futures::poll!(&mut joining).is_pending());
    let mut closing = Box::pin(window.close(Instant::now() + Duration::from_secs(1)));
    assert!(futures::poll!(&mut closing).is_pending());
    let mut contender = Box::pin(scheduler.acquire(
        account_id,
        AgentId {
            component_id: ComponentId::new(),
            agent_id: "publication-contender".into(),
        },
    ));
    assert!(futures::poll!(&mut contender).is_pending());
    assert!(held.load(Ordering::Acquire));
    assert_eq!(metrics.pending("accepted_stop", "permit_released"), 1.0);
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    drop(store);
    assert!(store_dropped.load(Ordering::Acquire));
    drop(operation);
    assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(invocation)));
    drop(registration);
    assert!(!driver.await.unwrap());
    joining.await.unwrap();
    closing.await.unwrap();
    parent_wait.wait().await;
    assert_eq!(lane.holder(), Some(parent));
    primary.complete();
    assert_eq!(slot.active_invocation_count(), 0);
    assert!(operations.metadata().operations.is_empty());
    assert!(!held.load(Ordering::Acquire));
    let acquired = contender.await;
    drop(acquired);
    stop::acknowledge_without_store(&publication, false);
    assert!(
        matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "no fabricated Store acknowledgement: the separate no-Store matrix and real Waiting tests own those promises"
    );
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released"),
        1.0
    );
    metrics.assert_finished("accepted_stop", "released");
    assert_eq!(
        metrics.value(
            "golem_agent_resource_release_unfinished",
            &[("origin", "accepted_stop")]
        ),
        0.0
    );
}

#[test]
#[timeout("5s")]
async fn closed_window_two_registered_drivers_keep_real_panic_as_failed_readiness() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    progress.lock().unwrap().release_scope = window.release_scope.clone();
    window
        .close(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!held.load(Ordering::Acquire));
    let interrupts = Arc::new(super::super::interrupt::Interrupts::default());
    let mut guard = interrupts.lock().await;
    let first = PendingWorkerInterrupt {
        kind: InterruptKind::Restart,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Restart),
    };
    assert!(guard.queue(first));
    let publication = {
        let mut admission = progress.lock().unwrap();
        let (publication, _) = super::super::stop::publication(
            &admission,
            &std::sync::RwLock::new(crate::model::ExecutionStatus::loading(AgentMode::Durable)),
        );
        let predecessor = admission.release.clone();
        let captured = interrupts.clone();
        let task = tokio::spawn(super::super::run_stop_after_release::<bool>(
            predecessor,
            async move {
                let _guard = captured.lock().await;
                panic!("actual closed-window owner task panic");
            },
        ));
        let receipt = publication.release_scope.accepted_driver();
        drop(super::super::stop::retain_driver(
            &mut admission,
            publication.clone(),
            true,
            true,
            receipt,
            task,
        ));
        publication
    };
    let terminal = InterruptKind::Interrupt(Timestamp::from(100_u64));
    assert!(guard.queue(PendingWorkerInterrupt {
        kind: terminal,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt)
    }));
    {
        let mut admission = progress.lock().unwrap();
        let (same, leader) = super::super::stop::publication(
            &admission,
            &std::sync::RwLock::new(crate::model::ExecutionStatus::loading(AgentMode::Durable)),
        );
        assert!(Arc::ptr_eq(&publication, &same));
        let captured = interrupts.clone();
        let leader = leader.unwrap();
        let task = tokio::spawn(async move {
            super::super::stop::publish_follower(&captured, leader).await?;
            Ok(false)
        });
        let receipt = same.release_scope.accepted_driver();
        drop(super::super::stop::retain_driver(
            &mut admission,
            same,
            false,
            true,
            receipt,
            task,
        ));
        assert_eq!(admission.tasks.len(), 2);
        assert!(admission.ready().is_none());
    }
    assert!(publication.cause.get().is_none());
    drop(guard);
    let error = super::super::join_accepted_stops(&progress)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("actual closed-window owner task panic")
    );
    assert_eq!(
        progress.lock().unwrap().ready().unwrap().unwrap_err(),
        error
    );
    assert_eq!(progress.lock().unwrap().completed_failure().unwrap(), error);
    let tasks = progress.lock().unwrap().tasks.clone();
    for task in tasks {
        assert_eq!(task.await.unwrap_err(), error);
    }
    assert!(publication.cause.get().is_none());
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(
        metrics.value(
            "golem_agent_resource_cleanup_failures_total",
            &[("stage", "end_to_end"), ("reason", "panic")]
        ),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn all_off_ready_close_spawns_no_task_and_completes_its_seal() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    assert_eq!(progress.lock().unwrap().window_close_tasks, 0);
    assert!(!held.load(Ordering::Acquire));
    assert!(closing.now_or_never().unwrap().is_ok());
    assert!(
        progress
            .lock()
            .unwrap()
            .release
            .clone()
            .unwrap()
            .now_or_never()
            .unwrap()
            .is_ok()
    );
    metrics.assert_finished("accepted_stop", "released");
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released"),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn all_off_pending_close_retains_task_and_permit_after_observer_drop() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let (release, wait) = tokio::sync::oneshot::channel();
    progress
        .lock()
        .unwrap()
        .tasks
        .push(async move { wait.await.unwrap() }.boxed().shared());
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    let successor = progress.lock().unwrap().release.clone().unwrap();
    assert_eq!(progress.lock().unwrap().window_close_tasks, 1);
    drop(closing);
    assert!(held.load(Ordering::Acquire));
    assert!(successor.clone().now_or_never().is_none());
    assert_eq!(metrics.pending("accepted_stop", "permit_released"), 1.0);
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released"),
        0.0
    );
    metrics.advance(2000);
    release.send(Ok(())).unwrap();
    successor.await.unwrap();
    assert!(!held.load(Ordering::Acquire));
    metrics.assert_finished("accepted_stop", "released");
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released"),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn stopping_instance_reports_real_permit_release_before_state_transition() {
    let (window, held, metrics) = unmetered_window().await;
    let instance = WorkerInstance::Stopping(super::super::StoppingWorker {
        notify: golem_common::one_shot::OneShotEvent::new(),
        final_state: super::super::FinalWorkerState::Unloaded {
            startup_failure: None,
        },
        pending_live_invocations: super::super::PendingLiveInvocationDisposition::Preserve,
        concurrent_agent_permit_held: held.clone(),
    });
    let progress = window.stop_progress.clone();
    let (release, wait) = tokio::sync::oneshot::channel();
    progress
        .lock()
        .unwrap()
        .tasks
        .push(async move { wait.await.unwrap() }.boxed().shared());
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    let successor = progress.lock().unwrap().release.clone().unwrap();
    assert!(held.load(Ordering::Acquire));
    assert!(instance.concurrent_agent_permit_is_held());
    assert!(successor.clone().now_or_never().is_none());
    assert_eq!(metrics.pending("accepted_stop", "permit_released"), 1.0);
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released"),
        0.0
    );
    metrics.advance(2000);
    release.send(Ok(())).unwrap();
    successor.await.unwrap();
    closing.await.unwrap();
    assert!(matches!(instance, WorkerInstance::Stopping(_)));
    assert!(!held.load(Ordering::Acquire));
    assert!(!instance.concurrent_agent_permit_is_held());
    metrics.assert_finished("accepted_stop", "released");
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released"),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn all_off_ready_close_converts_panic_and_keeps_the_failed_seal() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    progress.lock().unwrap().tasks.push(
        async { panic!("injected unmetered stop cleanup panic") }
            .boxed()
            .shared(),
    );
    let error = window
        .close(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected unmetered stop cleanup panic")
    );
    assert_eq!(progress.lock().unwrap().window_close_tasks, 0);
    assert!(!held.load(Ordering::Acquire));
    assert!(
        progress
            .lock()
            .unwrap()
            .release
            .clone()
            .unwrap()
            .now_or_never()
            .unwrap()
            .is_err()
    );
    metrics.assert_finished("accepted_stop", "released_with_failure");
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released_with_failure"),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn all_off_ready_close_retains_prior_cleanup_error() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let error = WorkerExecutorError::runtime("prior stop cleanup failed");
    progress
        .lock()
        .unwrap()
        .tasks
        .push(std::future::ready(Err(error.clone())).boxed().shared());
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    assert_eq!(progress.lock().unwrap().window_close_tasks, 0);
    assert_eq!(closing.await.unwrap_err(), error);
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(
        progress
            .lock()
            .unwrap()
            .release
            .clone()
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err(),
        error
    );
    metrics.assert_finished("accepted_stop", "released_with_failure");
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released_with_failure"),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn actual_seal_release_blocks_registered_late_work_until_physical_permit_drop() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let (release_first, first) = tokio::sync::oneshot::channel();
    progress
        .lock()
        .unwrap()
        .tasks
        .push(async move { first.await.unwrap() }.boxed().shared());
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    let predecessor = progress.lock().unwrap().release.clone().unwrap();
    let polled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let late_held = held.clone();
    let late_polled = polled.clone();
    let late = super::super::run_stop_after_release(Some(predecessor.clone()), async move {
        late_polled.fetch_add(1, Ordering::SeqCst);
        assert!(
            !late_held.load(Ordering::Acquire),
            "late work cannot touch a physically retained owner"
        );
        Ok(())
    })
    .boxed()
    .shared();
    progress.lock().unwrap().tasks.push(late.clone());
    assert!(late.clone().now_or_never().is_none());
    assert_eq!(polled.load(Ordering::SeqCst), 0);
    assert!(held.load(Ordering::Acquire));
    assert!(predecessor.clone().now_or_never().is_none());
    assert_eq!(metrics.pending("accepted_stop", "permit_released"), 1.0);
    drop(closing);
    release_first.send(Ok(())).unwrap();
    super::super::join_accepted_stops(&progress).await.unwrap();
    assert_eq!(polled.load(Ordering::SeqCst), 1);
    assert!(!held.load(Ordering::Acquire));
    assert!(progress.lock().unwrap().ready().unwrap().is_ok());
    metrics.assert_finished("accepted_stop", "released");
}

#[test]
#[timeout("5s")]
async fn actual_failed_and_lost_progress_keep_successor_health_after_physical_release() {
    for lost in [false, true] {
        let (window, held, metrics) = unmetered_window().await;
        let progress = window.stop_progress.clone();
        let (release, receive) = tokio::sync::oneshot::channel();
        let error = WorkerExecutorError::runtime(if lost {
            "registered completion was lost"
        } else {
            "registered stop work failed"
        });
        let missing = error.clone();
        progress.lock().unwrap().tasks.push(
            async move { receive.await.unwrap_or(Err(missing)) }
                .boxed()
                .shared(),
        );
        let closing = window.close(Instant::now() + Duration::from_secs(1));
        let predecessor = progress.lock().unwrap().release.clone().unwrap();
        let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let entered_in_work = entered.clone();
        let late = super::super::run_stop_after_release(Some(predecessor), async move {
            entered_in_work.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .boxed()
        .shared();
        progress.lock().unwrap().tasks.push(late.clone());
        assert!(late.clone().now_or_never().is_none());
        assert!(held.load(Ordering::Acquire));
        if lost {
            drop(release);
        } else {
            release.send(Err(error.clone())).unwrap();
        }
        assert_eq!(closing.await.unwrap_err(), error);
        assert_eq!(
            super::super::join_accepted_stops(&progress)
                .await
                .unwrap_err(),
            error
        );
        assert_eq!(
            progress.lock().unwrap().ready().unwrap().unwrap_err(),
            error
        );
        assert_eq!(
            entered.load(Ordering::SeqCst),
            0,
            "failed physical predecessor must not enter replacement work"
        );
        assert!(!held.load(Ordering::Acquire));
        metrics.assert_finished("accepted_stop", "released_with_failure");
    }
}

#[test]
#[timeout("5s")]
async fn real_driver_task_panic_keeps_failed_seal_and_readiness_not_success() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    progress.lock().unwrap().release_scope = window.release_scope.clone();
    let task = tokio::spawn(super::super::run_stop_after_release::<bool>(None, async {
        panic!("registered physical stop work panicked");
    }));
    {
        let mut admission = progress.lock().unwrap();
        let (publication, _) = super::super::stop::publication(
            &admission,
            &std::sync::RwLock::new(crate::model::ExecutionStatus::loading(AgentMode::Durable)),
        );
        let receipt = publication.release_scope.accepted_driver();
        drop(super::super::stop::retain_driver(
            &mut admission,
            publication,
            true,
            true,
            receipt,
            task,
        ));
    }
    let error = window
        .close(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("registered physical stop work panicked")
    );
    assert!(super::super::join_accepted_stops(&progress).await.is_err());
    assert!(progress.lock().unwrap().ready().unwrap().is_err());
    assert!(!held.load(Ordering::Acquire));
    metrics.assert_finished("accepted_stop", "released_with_failure");
}

#[test]
#[timeout("5s")]
async fn orderly_seal_close_preserves_operational_enforcement_failure_separately() {
    let (window, held, metrics) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let operational = WorkerExecutorError::runtime("policy unavailable during execution");
    progress.lock().unwrap().enforcement_failure = Some(operational.clone());
    window
        .close(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!held.load(Ordering::Acquire));
    assert!(progress.lock().unwrap().ready().unwrap().is_ok());
    assert_eq!(
        progress.lock().unwrap().enforcement_failure,
        Some(operational)
    );
    metrics.assert_finished("accepted_stop", "released");
}

#[test]
#[timeout("5s")]
async fn retiring_actual_seal_retains_registered_work_and_missing_release_is_error() {
    let progress: super::super::StopProgress = Arc::default();
    let (release, receive) = tokio::sync::oneshot::channel();
    progress
        .lock()
        .unwrap()
        .tasks
        .push(async move { receive.await.unwrap() }.boxed().shared());
    let mut seal = super::super::StopAdmissionSeal::seal(&progress, true);
    let successor = progress.lock().unwrap().release.clone().unwrap();
    assert!(progress.lock().unwrap().retired);
    let mut joined = Box::pin(seal.join());
    assert!(futures::poll!(joined.as_mut()).is_pending());
    assert!(successor.clone().now_or_never().is_none());
    release.send(Ok(())).unwrap();
    joined.await.unwrap();
    drop(seal);
    assert!(
        successor.await.is_err(),
        "dropping an uncompleted physical seal must not manufacture release"
    );
}

#[test]
#[timeout("5s")]
async fn same_revision_memory_credit_rejects_captured_exhaustion_without_stop() {
    use crate::services::resource_limits::MonthlyResourceExhaustion;
    let (entry, owner, _other) = prepaid_stop_owners().await;
    let fingerprint = owner.fingerprint;
    let attempt = owner.start_attempt;
    let generation = owner.generation;
    let target = owner.current.lock().unwrap().as_ref().unwrap().clone();
    let mut exhausted = stop_fuel_policy();
    exhausted.available_memory_gb_seconds = 0;
    assert!(entry.apply_monthly_snapshot_for_test(1, exhausted, 0));
    let proposed = owner.sample();
    assert_eq!(proposed.exhaustion, Some(MonthlyResourceExhaustion::Memory));
    let status = owner.resources.execution_status();
    let (publication, _) =
        super::super::stop::publication(&owner.progress.lock().unwrap(), &status);
    let mut raw = publication.signal.as_ref().unwrap().subscribe();
    let mut receipt = publication.receipt.subscribe();
    assert_eq!(entry.fuel_delta(), 0);
    assert!(entry.apply_monthly_snapshot_for_test(2, stop_fuel_policy(), 0));
    let credited = owner.sample();
    assert_eq!(credited.exhaustion, None);
    assert_eq!(credited.policy_revision, proposed.policy_revision);
    assert_eq!(credited.period, proposed.period);
    assert_eq!(credited.fuel_generation, proposed.fuel_generation);
    assert_eq!(owner.fingerprint, fingerprint);
    assert_eq!(owner.start_attempt, attempt);
    assert_eq!(owner.generation, generation);
    assert!(Arc::ptr_eq(
        owner.current.lock().unwrap().as_ref().unwrap(),
        &target
    ));
    assert!(Arc::ptr_eq(&owner.target, &target));
    assert!(owner.permit.is_some() && *target.active.lock().unwrap());
    assert!(target.matches_current(fingerprint, attempt, generation, true, &owner.current));
    let mut guard = owner.interrupts.lock().await;
    assert!(matches!(*guard, super::super::WorkerInterruptState::Idle));
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
    assert!(owner.progress.lock().unwrap().publication.is_none());
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(
        queue_monthly_capacity(
            proposed,
            credited,
            &mut guard,
            &owner.progress,
            &owner.monthly
        )
        .is_none()
    );
    assert!(matches!(*guard, super::super::WorkerInterruptState::Idle));
    assert!(!owner.progress.lock().unwrap().retired);
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
    assert!(owner.progress.lock().unwrap().publication.is_none());
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(publication.cause.get().is_none());
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(entry.fuel_delta(), 0);
}

#[test]
#[timeout("5s")]
async fn replaced_monthly_target_cannot_match_otherwise_equal_capacity() {
    let (entry, owner, _other) = prepaid_stop_owners().await;
    let mut policy = stop_fuel_policy();
    policy.available_memory_gb_seconds = 0;
    assert!(entry.apply_monthly_snapshot_for_test(1, policy, 0));
    let proposed = owner.sample();
    let generation = std::sync::atomic::AtomicU64::new(owner.generation);
    let old_running = super::super::stop::tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    assert!(owner.target.matches_running_owner(
        &old_running,
        owner.fingerprint,
        &generation,
        &owner.current
    ));
    let replacement = Arc::new(MonthlyWindowTarget {
        diagnostic_window_id: 2,
        trace_origin: golem_common::tracing::TraceOrigin::none(),
        fingerprint: owner.fingerprint,
        start_attempt: Uuid::new_v4(),
        resident_generation: owner.generation + 1,
        active: Mutex::new(true),
        #[cfg(feature = "test-utils")]
        panic: CancellationToken::new(),
        #[cfg(feature = "test-utils")]
        exited: CancellationToken::new(),
    });
    *owner.current.lock().unwrap() = Some(replacement.clone());
    *owner.target.active.lock().unwrap() = false;
    let generation = std::sync::atomic::AtomicU64::new(replacement.resident_generation);
    let running = super::super::stop::tests::running_lifecycle(
        owner.interrupts.clone(),
        replacement.start_attempt,
        true,
    );
    assert_eq!(owner.sample(), proposed);
    assert!(proposed.exhaustion.is_some());
    assert_ne!(replacement.start_attempt, owner.start_attempt);
    assert!(replacement.resident_generation > owner.generation);
    assert!(!Arc::ptr_eq(&owner.target, &replacement));
    assert!(!owner.target.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &owner.current
    ));
    assert!(replacement.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &owner.current
    ));
    let equal_fields = Arc::new(MonthlyWindowTarget {
        diagnostic_window_id: replacement.diagnostic_window_id,
        trace_origin: golem_common::tracing::TraceOrigin::none(),
        fingerprint: replacement.fingerprint,
        start_attempt: replacement.start_attempt,
        resident_generation: replacement.resident_generation,
        active: Mutex::new(true),
        #[cfg(feature = "test-utils")]
        panic: CancellationToken::new(),
        #[cfg(feature = "test-utils")]
        exited: CancellationToken::new(),
    });
    *owner.current.lock().unwrap() = Some(equal_fields.clone());
    assert_eq!(owner.sample(), proposed);
    assert!(!replacement.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &owner.current
    ));
    assert!(equal_fields.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &owner.current
    ));
    assert!(!owner.interrupts.lock().await.has_interrupt());
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(owner.progress.lock().unwrap().tasks.is_empty());
    assert!(owner.progress.lock().unwrap().publication.is_none());
}

fn non_running_monthly_lifecycles(
    running: WorkerInstance,
    attempt: Uuid,
) -> (
    [WorkerInstance; 4],
    futures::channel::oneshot::Sender<Result<(), WorkerExecutorError>>,
) {
    let WorkerInstance::Running(inner) = &running else {
        panic!("fixture requires Running");
    };
    let held = inner.concurrent_agent_permit_held.clone();
    let (handle, pending) = super::super::DeletionHandle::new();
    (
        [
            WorkerInstance::Deleting(super::super::DeletingWorker {
                runtime: Box::new(running),
                handle,
                completed_stage: super::super::WorkerDeletionStage::Claimed,
            }),
            WorkerInstance::WaitingForPermit(super::super::WaitingWorker {
                handle: Mutex::new(None),
                start_attempt: attempt,
            }),
            WorkerInstance::Stopping(super::super::StoppingWorker {
                notify: golem_common::one_shot::OneShotEvent::new(),
                final_state: super::super::FinalWorkerState::Unloaded {
                    startup_failure: None,
                },
                pending_live_invocations: super::super::PendingLiveInvocationDisposition::Preserve,
                concurrent_agent_permit_held: held,
            }),
            WorkerInstance::Unloaded {
                startup_failure: None,
            },
        ],
        pending,
    )
}

#[test]
#[timeout("5s")]
async fn monthly_running_owner_guard_rejects_non_running_lifecycle() {
    let (entry, owner, _other) = prepaid_stop_owners().await;
    let mut policy = stop_fuel_policy();
    policy.available_memory_gb_seconds = 0;
    assert!(entry.apply_monthly_snapshot_for_test(1, policy, 0));
    assert!(owner.sample().exhaustion.is_some());
    assert!(!owner.progress.lock().unwrap().retired);
    let generation = std::sync::atomic::AtomicU64::new(owner.generation);
    let running = super::super::stop::tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    assert!(owner.target.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &owner.current
    ));
    let (lifecycles, _pending_deletion) =
        non_running_monthly_lifecycles(running, owner.start_attempt);
    for lifecycle in lifecycles {
        assert!(!owner.target.matches_running_owner(
            &lifecycle,
            owner.fingerprint,
            &generation,
            &owner.current
        ));
        assert!(!owner.progress.lock().unwrap().retired);
        assert!(!owner.interrupts.lock().await.has_interrupt());
        assert!(owner.monthly.lock().unwrap().is_none());
        assert!(owner.progress.lock().unwrap().tasks.is_empty());
        assert!(owner.progress.lock().unwrap().publication.is_none());
    }
}

#[test]
#[timeout("5s")]
async fn monthly_running_owner_guard_skips_current_window_for_non_running_lifecycle() {
    let (_entry, owner, _other) = prepaid_stop_owners().await;
    let current = Mutex::new(Some(owner.target.clone()));
    assert!(
        std::panic::catch_unwind(|| {
            let _held = current.lock().unwrap();
            panic!("non-Running current window must remain unused");
        })
        .is_err()
    );
    let generation = std::sync::atomic::AtomicU64::new(owner.generation);
    let running = super::super::stop::tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    let (lifecycles, _pending_deletion) =
        non_running_monthly_lifecycles(running, owner.start_attempt);
    for lifecycle in lifecycles {
        assert!(!owner.target.matches_running_owner(
            &lifecycle,
            owner.fingerprint,
            &generation,
            &current
        ));
    }
}

#[test]
#[timeout("5s")]
async fn monthly_running_owner_guard_checks_identity_generation_permit_and_target() {
    let (_entry, owner, _other) = prepaid_stop_owners().await;
    let generation = std::sync::atomic::AtomicU64::new(owner.generation);
    let running = super::super::stop::tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    assert!(owner.target.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &owner.current
    ));
    let poisoned = Mutex::new(Some(owner.target.clone()));
    assert!(
        std::panic::catch_unwind(|| {
            let _held = poisoned.lock().unwrap();
            panic!("identity mismatch must not read the current window");
        })
        .is_err()
    );
    for (fingerprint, attempt, generation, held) in [
        (
            AgentFingerprint::new(),
            owner.start_attempt,
            owner.generation,
            true,
        ),
        (owner.fingerprint, Uuid::new_v4(), owner.generation, true),
        (
            owner.fingerprint,
            owner.start_attempt,
            owner.generation + 1,
            true,
        ),
        (
            owner.fingerprint,
            owner.start_attempt,
            owner.generation,
            false,
        ),
    ] {
        let lifecycle =
            super::super::stop::tests::running_lifecycle(owner.interrupts.clone(), attempt, true);
        let WorkerInstance::Running(running) = &lifecycle else {
            panic!("fixture requires Running");
        };
        running
            .concurrent_agent_permit_held
            .store(held, Ordering::Release);
        let generation = std::sync::atomic::AtomicU64::new(generation);
        assert!(!owner.target.matches_running_owner(
            &lifecycle,
            fingerprint,
            &generation,
            &poisoned
        ));
    }
    assert!(!owner.target.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &Mutex::new(None)
    ));
    let equal_fields = Arc::new(MonthlyWindowTarget {
        diagnostic_window_id: owner.target.diagnostic_window_id,
        trace_origin: golem_common::tracing::TraceOrigin::none(),
        fingerprint: owner.fingerprint,
        start_attempt: owner.start_attempt,
        resident_generation: owner.generation,
        active: Mutex::new(true),
        #[cfg(feature = "test-utils")]
        panic: CancellationToken::new(),
        #[cfg(feature = "test-utils")]
        exited: CancellationToken::new(),
    });
    assert!(!owner.target.matches_running_owner(
        &running,
        owner.fingerprint,
        &generation,
        &Mutex::new(Some(equal_fields))
    ));
}
