use super::*;
use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
use crate::services::golem_config::ResourceUsageMeteringConfig;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::{AtomicResourceEntry, MonthlyResourceExhaustion};
use crate::services::resource_usage_metering::ResourceUsageAccount;
use crate::worker::{InterruptEstablishment, WorkerInterruptState};
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::{test, timeout};

const STOP_FUEL_BATCH: u64 = 1_000_000_000;

// Only the queue/admission inputs live here. Establishment and physical cleanup are exercised
// through the real Worker in integration tests, not a second test-side stop implementation.
struct MonthlyStopOwner {
    fingerprint: AgentFingerprint,
    start_attempt: Uuid,
    generation: u64,
    target: Arc<MonthlyWindowTarget>,
    current: Mutex<Option<Arc<MonthlyWindowTarget>>>,
    permit: crate::services::active_agents::ConcurrentAgentPermit,
    resources: Arc<super::super::instance::OwnerRuntimeResources>,
    interrupts: Arc<super::super::interrupt::Interrupts>,
    tail: Mutex<Arc<InterruptEstablishment>>,
    monthly: Mutex<Option<(InterruptKind, MonthlyResourceExhaustion)>>,
}

fn target(
    fingerprint: AgentFingerprint,
    start_attempt: Uuid,
    generation: u64,
) -> Arc<MonthlyWindowTarget> {
    Arc::new(MonthlyWindowTarget {
        diagnostic_window_id: 1,
        trace_origin: golem_common::tracing::TraceOrigin::none(),
        fingerprint,
        start_attempt,
        resident_generation: generation,
        active: Mutex::new(true),
        failure: Mutex::new(None),
    })
}

impl MonthlyStopOwner {
    fn new(
        entry: Arc<AtomicResourceEntry>,
        permit: crate::services::active_agents::ConcurrentAgentPermit,
        generation: u64,
    ) -> Self {
        let fingerprint = AgentFingerprint::new();
        let start_attempt = Uuid::new_v4();
        let target = target(fingerprint, start_attempt, generation);
        Self {
            fingerprint,
            start_attempt,
            generation,
            current: Mutex::new(Some(target.clone())),
            target,
            permit,
            resources: Arc::new(super::super::instance::OwnerRuntimeResources::new(
                entry,
                Arc::new(std::sync::RwLock::new(
                    crate::model::ExecutionStatus::loading(AgentMode::Durable),
                )),
            )),
            interrupts: Arc::default(),
            tail: Mutex::new(InterruptEstablishment::ready()),
            monthly: Mutex::new(None),
        }
    }

    fn sample(&self) -> MonthlyCapacity {
        self.resources
            .with_monthly_capacity(AgentMode::Durable, |capacity| capacity)
    }

    fn register(
        &self,
        proposed: MonthlyCapacity,
        interrupts: &mut super::super::interrupt::InterruptGuard<'_>,
    ) -> Option<PendingWorkerInterrupt> {
        if !self
            .target
            .matches_current(self.start_attempt, true, &self.current)
            || !*self.target.active.lock().unwrap()
        {
            return None;
        }
        self.resources
            .with_monthly_capacity(AgentMode::Durable, |current| {
                queue_monthly_capacity(proposed, current, interrupts, &self.tail, &self.monthly)
            })
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
    let callee = scheduler
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
        MonthlyStopOwner::new(entry, callee, 8),
    )
}

#[test]
fn trace_context_requires_the_actual_active_monthly_target() {
    let fingerprint = AgentFingerprint::new();
    let attempt = Uuid::new_v4();
    let target = target(fingerprint, attempt, 1);
    let current = Mutex::new(Some(target.clone()));
    let trace = target.trace_if_current(&current).unwrap();
    assert_eq!(trace.id, target.diagnostic_window_id);
    assert_eq!(trace.fingerprint, fingerprint);
    assert_eq!(trace.start_attempt, attempt);
    assert_eq!(trace.resident_generation, 1);
    let replacement = self::target(fingerprint, attempt, 1);
    *current.lock().unwrap() = Some(replacement.clone());
    assert!(target.trace_if_current(&current).is_none());
    assert!(replacement.trace_if_current(&current).is_some());
    *replacement.active.lock().unwrap() = false;
    assert!(replacement.trace_if_current(&current).is_none());
    *current.lock().unwrap() = None;
    assert!(target.trace_if_current(&current).is_none());
}

#[test]
fn normal_tokio_ticks_are_not_early_and_rearm_after_a_rejected_proposal() {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            let (entry, owner, _other) = prepaid_stop_owners().await;
            let mut policy = stop_fuel_policy();
            policy.available_memory_gb_seconds = 0;
            assert!(entry.apply_monthly_snapshot_for_test(1, policy, 0));
            let proposed = owner.sample();
            let mut check = MonthlyMemoryCheck::new(entry, None);
            let started = tokio::time::Instant::now();

            let duration = check.duration();
            assert_eq!(duration, Duration::from_secs(30));
            {
                let wait = check.wait(tokio::time::sleep(duration));
                tokio::pin!(wait);
                assert!(futures::poll!(wait.as_mut()).is_pending());
                tokio::time::advance(Duration::from_secs(29)).await;
                assert!(futures::poll!(wait.as_mut()).is_pending());
                tokio::time::advance(Duration::from_secs(1)).await;
                assert!(wait.await.is_ok());
            }

            let original_tail = owner.tail.lock().unwrap().clone();
            let mut stale = proposed;
            stale.policy_revision += 1;
            {
                let mut guard = owner.interrupts.lock().await;
                assert!(owner.register(stale, &mut guard).is_none());
                assert!(!guard.has_interrupt());
            }
            assert!(owner.monthly.lock().unwrap().is_none());
            assert!(Arc::ptr_eq(&original_tail, &owner.tail.lock().unwrap()));

            let duration = check.duration();
            assert_eq!(duration, Duration::from_secs(30));
            {
                let wait = check.wait(tokio::time::sleep(duration));
                tokio::pin!(wait);
                assert!(futures::poll!(wait.as_mut()).is_pending());
                tokio::time::advance(Duration::from_secs(29)).await;
                assert!(futures::poll!(wait.as_mut()).is_pending());
                tokio::time::advance(Duration::from_secs(1)).await;
                assert!(wait.await.is_ok());
            }
            assert_eq!(started.elapsed(), Duration::from_secs(60));
            assert_eq!(owner.sample(), proposed);
            let mut guard = owner.interrupts.lock().await;
            let accepted = owner.register(proposed, &mut guard).unwrap();
            assert!(matches!(accepted.kind, InterruptKind::Suspend(_)));
            assert!(guard.has_interrupt());
            assert_eq!(
                owner.monthly.lock().unwrap().as_ref().unwrap().0,
                accepted.kind
            );
        });
}

#[test]
fn a_capacity_update_rearms_the_normal_tokio_timer_from_the_update() {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            let (entry, _owner, _other) = prepaid_stop_owners().await;
            let mut policy = stop_fuel_policy();
            policy.available_memory_gb_seconds = 0;
            assert!(entry.apply_monthly_snapshot_for_test(1, policy.clone(), 0));
            let mut check = MonthlyMemoryCheck::new(entry.clone(), None);
            let started = tokio::time::Instant::now();
            let duration = check.duration();
            assert_eq!(duration, Duration::from_secs(30));
            {
                let wait = check.wait(tokio::time::sleep(duration));
                tokio::pin!(wait);
                assert!(futures::poll!(wait.as_mut()).is_pending());
                tokio::time::advance(Duration::from_secs(11)).await;
                assert!(futures::poll!(wait.as_mut()).is_pending());
                assert!(entry.apply_monthly_snapshot_for_test(2, policy, 0));
                assert!(wait.await.is_ok());
            }
            let duration = check.duration();
            assert_eq!(duration, Duration::from_secs(30));
            let wait = check.wait(tokio::time::sleep(duration));
            tokio::pin!(wait);
            assert!(futures::poll!(wait.as_mut()).is_pending());
            // Neither the original deadline at 30 nor the instant before 41 may fire it.
            tokio::time::advance(Duration::from_secs(19)).await;
            assert!(futures::poll!(wait.as_mut()).is_pending());
            tokio::time::advance(Duration::from_secs(10)).await;
            assert!(futures::poll!(wait.as_mut()).is_pending());
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(wait.await.is_ok());
            assert_eq!(started.elapsed(), Duration::from_secs(41));
        });
}

#[test]
fn nonmatching_storage_update_and_normal_tick_keep_the_actual_queue_empty() {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
                let mut policy = stop_fuel_policy();
                let entry = Arc::new(AtomicResourceEntry::new_with_monthly_policy(
                    policy.clone(),
                    usize::MAX,
                    usize::MAX,
                    u64::MAX,
                    1,
                ));
                let mut check = MonthlyMemoryCheck::new(entry.clone(), None);
                let interrupts = super::super::interrupt::Interrupts::default();
                let tail = Mutex::new(InterruptEstablishment::ready());
                let original_tail = tail.lock().unwrap().clone();
                let monthly = Mutex::new(None);
                let started = tokio::time::Instant::now();
                let duration = check.duration();
                assert_eq!(duration, Duration::from_secs(30));
                {
                    let wait = check.wait(tokio::time::sleep(duration));
                    tokio::pin!(wait);
                    assert!(futures::poll!(wait.as_mut()).is_pending());
                    tokio::time::advance(Duration::from_secs(11)).await;
                    assert!(futures::poll!(wait.as_mut()).is_pending());
                    if mode == AgentMode::Durable {
                        policy.available_ephemeral_storage_byte_seconds = 0;
                    } else {
                        policy.available_durable_storage_byte_seconds = 0;
                    }
                    assert!(entry.apply_monthly_snapshot_for_test(1, policy, 1));
                    assert!(wait.await.is_ok());
                }
                let current = entry.with_monthly_capacity(mode, None, |capacity| capacity);
                assert_eq!(current.policy_revision, 1);
                assert_eq!(current.exhaustion, None);
                let other = if mode == AgentMode::Durable {
                    AgentMode::Ephemeral
                } else {
                    AgentMode::Durable
                };
                assert_eq!(
                    entry.with_monthly_capacity(other, None, |capacity| capacity.exhaustion),
                    Some(if other == AgentMode::Durable {
                        MonthlyResourceExhaustion::DurableStorage
                    } else {
                        MonthlyResourceExhaustion::EphemeralStorage
                    }),
                );
                {
                    let mut guard = interrupts.lock().await;
                    assert!(
                        queue_monthly_capacity(current, current, &mut guard, &tail, &monthly)
                            .is_none()
                    );
                    assert!(!guard.has_interrupt());
                }
                assert!(monthly.lock().unwrap().is_none());
                assert!(Arc::ptr_eq(&original_tail, &tail.lock().unwrap()));
                let duration = check.duration();
                assert_eq!(duration, Duration::from_secs(30));
                {
                    let wait = check.wait(tokio::time::sleep(duration));
                    tokio::pin!(wait);
                    assert!(futures::poll!(wait.as_mut()).is_pending());
                    tokio::time::advance(Duration::from_secs(19)).await;
                    assert!(futures::poll!(wait.as_mut()).is_pending());
                    tokio::time::advance(Duration::from_secs(10)).await;
                    assert!(futures::poll!(wait.as_mut()).is_pending());
                    tokio::time::advance(Duration::from_secs(1)).await;
                    assert!(wait.await.is_ok());
                }
                assert_eq!(started.elapsed(), Duration::from_secs(41));
                let after_tick = entry.with_monthly_capacity(mode, None, |capacity| capacity);
                assert_eq!(after_tick, current);
                let mut guard = interrupts.lock().await;
                assert!(
                    queue_monthly_capacity(after_tick, after_tick, &mut guard, &tail, &monthly)
                        .is_none()
                );
                assert!(!guard.has_interrupt());
                assert!(monthly.lock().unwrap().is_none());
                assert!(Arc::ptr_eq(&original_tail, &tail.lock().unwrap()));
            }
        });
}

#[test]
#[timeout("5s")]
async fn monthly_supersedes_restart_without_reclassifying_an_already_observed_deadline() {
    use crate::model::TrapType;
    use crate::worker::outcome::monthly_failure;
    use crate::workerctx::default::Context;
    use golem_common::model::oplog::AgentError;
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let (entry, owner, _other) = prepaid_stop_owners().await;
        let mut policy = stop_fuel_policy();
        policy.available_memory_gb_seconds = 0;
        assert!(entry.apply_monthly_snapshot_for_test(1, policy, 0));
        let proposed = owner.sample();
        assert_eq!(proposed.exhaustion, Some(MonthlyResourceExhaustion::Memory));
        let mut guard = owner.interrupts.lock().await;
        let restart = super::super::register_interrupt_request(
            &mut guard,
            &owner.tail,
            InterruptKind::Restart,
            false,
            UnloadRequest::ordinary(UnloadReason::Restart),
        )
        .unwrap();
        let accepted = owner.register(proposed, &mut guard).unwrap();
        assert!(
            matches!(&*guard, WorkerInterruptState::Pending(pending) if pending.kind == accepted.kind)
        );
        assert!(!Arc::ptr_eq(
            &restart.establishment,
            &accepted.establishment
        ));
        assert!(Arc::ptr_eq(
            accepted
                .establishment
                .predecessor
                .lock()
                .unwrap()
                .as_ref()
                .unwrap(),
            &restart.establishment
        ));
        let trap = TrapType::Error {
            error: AgentError::InternalError("invocation timer elapsed".into()),
            retry_from: golem_common::model::OplogIndex::INITIAL,
            semantic_trap_retry_override: None,
        };
        assert!(
            monthly_failure::<Context>(
                &trap,
                mode,
                &owner.monthly.lock().unwrap(),
                &crate::services::golem_config::GolemConfig::default()
            )
            .is_none()
        );
        assert!(
            matches!(trap, TrapType::Error { error: AgentError::InternalError(message), retry_from, semantic_trap_retry_override: None }
            if message == "invocation timer elapsed" && retry_from == golem_common::model::OplogIndex::INITIAL)
        );
        assert!(
            matches!(&*guard, WorkerInterruptState::Pending(pending) if pending.kind == accepted.kind)
        );
    }
}

#[test]
#[timeout("10s")]
async fn two_prepaid_stop_registrations_precede_either_real_refund() {
    use crate::workerctx::default::tests::PrepaidStore;
    let (entry, caller, callee) = prepaid_stop_owners().await;
    let mut caller_store = PrepaidStore::new(&caller.resources, STOP_FUEL_BATCH);
    let mut callee_store = PrepaidStore::new(&callee.resources, STOP_FUEL_BATCH);
    caller_store.borrow();
    callee_store.borrow();
    caller_store.execute().await;
    callee_store.execute().await;
    assert_eq!(entry.fuel_delta(), 2 * STOP_FUEL_BATCH as i64);
    assert!(caller.sample().exhaustion.is_none());
    assert!(callee.sample().exhaustion.is_none());
    assert!(entry.apply_monthly_snapshot_for_test(1, stop_fuel_policy(), 0));
    let mut caller_guard = caller.interrupts.lock().await;
    let mut callee_guard = callee.interrupts.lock().await;
    for (owner, guard) in [(&caller, &mut caller_guard), (&callee, &mut callee_guard)] {
        let proposed = owner.sample();
        assert_eq!(proposed.fuel_generation, Some(0));
        assert_eq!(
            proposed.exhaustion,
            Some(MonthlyResourceExhaustion::Compute)
        );
        let pending = owner.register(proposed, guard).unwrap();
        assert!(
            matches!(&**guard, WorkerInterruptState::Pending(actual) if actual.kind == pending.kind)
        );
        assert_eq!(
            *owner.monthly.lock().unwrap(),
            Some((pending.kind, MonthlyResourceExhaustion::Compute))
        );
        assert!(Arc::ptr_eq(
            &owner.tail.lock().unwrap(),
            &pending.establishment
        ));
    }
    assert_eq!(entry.fuel_delta(), 2 * STOP_FUEL_BATCH as i64);
    let callee_consumed = callee_store.refund();
    assert!(callee_consumed > 0 && callee_consumed < STOP_FUEL_BATCH);
    assert_eq!(
        entry.fuel_delta(),
        STOP_FUEL_BATCH as i64 + callee_consumed as i64
    );
    assert!(caller.sample().exhaustion.is_none());
    let caller_consumed = caller_store.refund();
    assert!(caller_consumed > 0 && caller_consumed < STOP_FUEL_BATCH);
    assert_eq!(
        entry.fuel_delta(),
        (caller_consumed + callee_consumed) as i64
    );
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    for (owner, guard) in [(&caller, &mut caller_guard), (&callee, &mut callee_guard)] {
        let pending = guard.take().unwrap();
        assert_eq!(
            *owner.monthly.lock().unwrap(),
            Some((pending.kind, MonthlyResourceExhaustion::Compute))
        );
    }
}

#[test]
#[timeout("10s")]
async fn captured_exhausted_proposal_is_rejected_after_other_store_refund_with_same_identity() {
    use crate::workerctx::default::tests::PrepaidStore;
    let (entry, caller, callee) = prepaid_stop_owners().await;
    let mut caller_store = PrepaidStore::new(&caller.resources, STOP_FUEL_BATCH);
    let mut callee_store = PrepaidStore::new(&callee.resources, STOP_FUEL_BATCH);
    caller_store.borrow();
    callee_store.borrow();
    caller_store.execute().await;
    callee_store.execute().await;
    assert!(entry.apply_monthly_snapshot_for_test(1, stop_fuel_policy(), 0));
    let proposed = caller.sample();
    assert!(proposed.exhaustion.is_some());
    let mut callee_guard = callee.interrupts.lock().await;
    assert!(
        callee
            .register(callee.sample(), &mut callee_guard)
            .is_some()
    );
    let window = caller.current.lock().unwrap().as_ref().unwrap().clone();
    let original_tail = caller.tail.lock().unwrap().clone();
    let before = entry.fuel_delta();
    let consumed = callee_store.refund();
    assert!(consumed > 0 && consumed < STOP_FUEL_BATCH);
    assert_eq!(
        entry.fuel_delta() - before,
        -(STOP_FUEL_BATCH as i64 - consumed as i64)
    );
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    let restored = caller.sample();
    assert_eq!(
        (
            restored.policy_revision,
            restored.period,
            restored.fuel_generation
        ),
        (
            proposed.policy_revision,
            proposed.period,
            proposed.fuel_generation
        )
    );
    assert!(restored.exhaustion.is_none());
    assert!(Arc::ptr_eq(
        caller.current.lock().unwrap().as_ref().unwrap(),
        &window
    ));
    assert!(
        caller
            .target
            .matches_current(caller.start_attempt, true, &caller.current)
    );
    let mut guard = caller.interrupts.lock().await;
    assert!(caller.register(proposed, &mut guard).is_none());
    assert!(!guard.has_interrupt());
    assert!(caller.monthly.lock().unwrap().is_none());
    assert!(Arc::ptr_eq(&original_tail, &caller.tail.lock().unwrap()));
    assert!(matches!(
        *caller.resources.execution_status().read().unwrap(),
        crate::model::ExecutionStatus::Loading { .. }
    ));
    let caller_consumed = caller_store.refund();
    assert_eq!(entry.fuel_delta(), (caller_consumed + consumed) as i64);
}

#[test]
#[timeout("5s")]
async fn monthly_target_identity_mismatch_does_not_read_current_window() {
    let (_entry, owner, _other) = prepaid_stop_owners().await;
    let poisoned = Mutex::new(None::<Arc<MonthlyWindowTarget>>);
    assert!(
        std::panic::catch_unwind(|| {
            let _held = poisoned.lock().unwrap();
            panic!("unused current window");
        })
        .is_err()
    );
    for (attempt, held) in [(Uuid::new_v4(), true), (owner.start_attempt, false)] {
        assert!(!owner.target.matches_current(attempt, held, &poisoned));
    }
}

#[test]
#[timeout("5s")]
async fn monthly_body_rejects_changed_capacity_inactive_target_and_terminal_claim() {
    let (entry, owner, _other) = prepaid_stop_owners().await;
    let mut guard = owner.interrupts.lock().await;
    assert!(owner.register(owner.sample(), &mut guard).is_none());
    let mut policy = stop_fuel_policy();
    policy.available_fuel = 0;
    assert!(entry.apply_monthly_snapshot_for_test(1, policy, 0));
    let current = owner.sample();
    assert!(current.exhaustion.is_some());
    let original_tail = owner.tail.lock().unwrap().clone();
    let mut changes = [current; 5];
    changes[0].policy_revision += 1;
    changes[1].period.year += 1;
    changes[2].fuel_generation = Some(99);
    changes[3].exhaustion = None;
    changes[4].policy_status = golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable;
    for proposed in changes {
        assert!(owner.register(proposed, &mut guard).is_none());
        assert!(!guard.has_interrupt());
        assert!(owner.monthly.lock().unwrap().is_none());
        assert!(Arc::ptr_eq(&original_tail, &owner.tail.lock().unwrap()));
    }
    *owner.target.active.lock().unwrap() = false;
    assert!(owner.register(current, &mut guard).is_none());
    assert!(!guard.has_interrupt());
    *owner.target.active.lock().unwrap() = true;
    let user = InterruptKind::Interrupt(Timestamp::from(30_u64));
    let queued = super::super::register_interrupt_request(
        &mut guard,
        &owner.tail,
        user,
        false,
        UnloadRequest::ordinary(UnloadReason::Interrupt),
    )
    .unwrap();
    let claimed = guard.claim_pending_terminal().unwrap();
    assert_eq!(claimed.kind, user);
    assert!(Arc::ptr_eq(&queued.establishment, &claimed.establishment));
    assert!(
        queue_monthly_capacity(
            current,
            owner.sample(),
            &mut guard,
            &owner.tail,
            &owner.monthly
        )
        .is_none()
    );
    assert!(matches!(*guard, WorkerInterruptState::TerminalClaimed));
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(Arc::ptr_eq(
        &queued.establishment,
        &owner.tail.lock().unwrap()
    ));
}

async fn unmetered_window() -> (
    ExecutionWindow,
    Arc<AtomicBool>,
    crate::metrics::resource_release::tests::TestMetrics,
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
    let account_id = AccountId::new();
    scheduler.register_account(account_id, entry).await;
    let permit = scheduler
        .acquire(
            account_id,
            AgentId {
                component_id: ComponentId::new(),
                agent_id: "unmetered-close".into(),
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
    (window, held, metrics)
}

#[test]
#[timeout("5s")]
async fn all_off_ready_close_spawns_no_task_and_releases_its_permit() {
    let (window, held, metrics) = unmetered_window().await;
    let close_tasks = window.close_tasks.clone();
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    assert_eq!(close_tasks.load(Ordering::Relaxed), 0);
    assert!(!held.load(Ordering::Acquire));
    assert!(closing.now_or_never().unwrap().is_ok());
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
    assert!(held.load(Ordering::Acquire));
    assert!(instance.concurrent_agent_permit_is_held());
    assert_eq!(metrics.pending("accepted_stop", "permit_released"), 1.0);
    assert_eq!(
        metrics.count("accepted_stop", "permit_released", "released"),
        0.0
    );
    window
        .close(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
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
async fn window_close_preserves_policy_unavailability_without_cleanup_failure() {
    let (mut window, held, metrics) = unmetered_window().await;
    let (_entry, owner, _other) = prepaid_stop_owners().await;
    let target = owner.target.clone();
    target.record_failure(MonthlyMonitorFailure::PolicyUnavailable);
    window.monitor = Some(MonthlyMonitor {
        target: target.clone(),
        cancelled: CancellationToken::new(),
        task: tokio::spawn(async { Ok(()) }),
    });
    window
        .close(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!held.load(Ordering::Acquire));
    assert!(target.cleanup_failure().is_none());
    assert!(!*target.active.lock().unwrap());
    assert!(target.policy_is_unavailable());
    assert_eq!(
        target.take_policy_failure(),
        Some(crate::services::resource_limits::MonthlyResourceCapacityError::unavailable_error())
    );
    assert!(!target.policy_is_unavailable());
    assert!(target.cleanup_failure().is_none());
    metrics.assert_finished("accepted_stop", "released");
}

#[test]
async fn policy_outage_does_not_erase_or_create_window_cleanup_failure() {
    for policy_first in [false, true] {
        let target = target(AgentFingerprint::new(), Uuid::new_v4(), 1);
        if policy_first {
            target.record_failure(MonthlyMonitorFailure::PolicyUnavailable);
            assert!(target.cleanup_failure().is_none());
        }
        let first = WorkerExecutorError::runtime("first monitor failure");
        target.record_failure(MonthlyMonitorFailure::Cleanup(first.clone()));
        target.record_failure(MonthlyMonitorFailure::PolicyUnavailable);
        target.record_failure(MonthlyMonitorFailure::Cleanup(
            WorkerExecutorError::runtime("later monitor failure"),
        ));
        assert_eq!(target.cleanup_failure(), Some(first));
        assert!(!target.policy_is_unavailable());
        assert!(target.take_policy_failure().is_none());
    }
}

#[test]
#[timeout("5s")]
async fn failed_monitor_join_retains_cleanup_failure_after_target_closure() {
    let target = target(AgentFingerprint::new(), Uuid::new_v4(), 1);
    target.record_failure(MonthlyMonitorFailure::PolicyUnavailable);
    *target.active.lock().unwrap() = false;
    let monitor = MonthlyMonitor {
        target: target.clone(),
        cancelled: CancellationToken::new(),
        task: tokio::spawn(async { panic!("monitor owner lost") }),
    };
    let error = join_monitor(Some(monitor)).await.unwrap_err();
    assert!(error.to_string().contains("Monthly monitor failed:"));
    assert!(error.to_string().contains("monitor owner lost"));
    assert_eq!(target.cleanup_failure(), Some(error.clone()));
    target.record_failure(MonthlyMonitorFailure::PolicyUnavailable);
    assert_eq!(target.cleanup_failure(), Some(error));
    assert!(target.take_policy_failure().is_none());
    assert!(!*target.active.lock().unwrap());
}

#[test]
#[timeout("5s")]
async fn same_revision_memory_credit_rejects_captured_exhaustion_without_stop() {
    let (entry, owner, _other) = prepaid_stop_owners().await;
    let window = owner.current.lock().unwrap().as_ref().unwrap().clone();
    let mut exhausted = stop_fuel_policy();
    exhausted.available_memory_gb_seconds = 0;
    assert!(entry.apply_monthly_snapshot_for_test(1, exhausted, 0));
    let proposed = owner.sample();
    assert_eq!(proposed.exhaustion, Some(MonthlyResourceExhaustion::Memory));
    assert_eq!(entry.fuel_delta(), 0);
    assert!(entry.apply_monthly_snapshot_for_test(2, stop_fuel_policy(), 0));
    let credited = owner.sample();
    assert_eq!(credited.exhaustion, None);
    assert_eq!(
        (
            credited.policy_revision,
            credited.period,
            credited.fuel_generation
        ),
        (
            proposed.policy_revision,
            proposed.period,
            proposed.fuel_generation
        )
    );
    assert!(Arc::ptr_eq(
        owner.current.lock().unwrap().as_ref().unwrap(),
        &window
    ));
    assert!(Arc::ptr_eq(&owner.target, &window));
    assert!(*window.active.lock().unwrap());
    assert!(window.matches_current(owner.start_attempt, true, &owner.current));
    let original_tail = owner.tail.lock().unwrap().clone();
    let mut guard = owner.interrupts.lock().await;
    assert!(
        queue_monthly_capacity(proposed, credited, &mut guard, &owner.tail, &owner.monthly)
            .is_none()
    );
    assert!(matches!(*guard, WorkerInterruptState::Idle));
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(Arc::ptr_eq(&original_tail, &owner.tail.lock().unwrap()));
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
    let running = super::super::lifecycle_tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    assert!(owner.target.matches_running_owner(&running, &owner.current));
    // A retry can keep its start attempt. Window identity must reject the queued sample
    // even before the old monitor is deactivated and with unchanged exhausted capacity.
    let retry = target(owner.fingerprint, owner.start_attempt, owner.generation + 1);
    *owner.current.lock().unwrap() = Some(retry.clone());
    assert!(*owner.target.active.lock().unwrap());
    assert_eq!(owner.sample(), proposed);
    assert!(!owner.target.matches_running_owner(&running, &owner.current));
    assert!(retry.matches_running_owner(&running, &owner.current));
    let original_tail = owner.tail.lock().unwrap().clone();
    {
        let mut guard = owner.interrupts.lock().await;
        assert!(owner.register(proposed, &mut guard).is_none());
        assert!(!guard.has_interrupt());
    }
    assert!(owner.monthly.lock().unwrap().is_none());
    assert!(Arc::ptr_eq(&original_tail, &owner.tail.lock().unwrap()));
    let replacement = target(owner.fingerprint, Uuid::new_v4(), owner.generation + 1);
    *owner.current.lock().unwrap() = Some(replacement.clone());
    *owner.target.active.lock().unwrap() = false;
    let running = super::super::lifecycle_tests::running_lifecycle(
        owner.interrupts.clone(),
        replacement.start_attempt,
        true,
    );
    assert_eq!(owner.sample(), proposed);
    assert!(proposed.exhaustion.is_some());
    assert_ne!(replacement.start_attempt, owner.start_attempt);
    assert!(replacement.resident_generation > owner.generation);
    assert!(!Arc::ptr_eq(&owner.target, &replacement));
    assert!(!owner.target.matches_running_owner(&running, &owner.current));
    assert!(replacement.matches_running_owner(&running, &owner.current));
    let equal_fields = target(
        replacement.fingerprint,
        replacement.start_attempt,
        replacement.resident_generation,
    );
    *owner.current.lock().unwrap() = Some(equal_fields.clone());
    assert_eq!(owner.sample(), proposed);
    assert!(!replacement.matches_running_owner(&running, &owner.current));
    assert!(equal_fields.matches_running_owner(&running, &owner.current));
    assert!(!owner.interrupts.has_interrupt().await);
    assert!(owner.monthly.lock().unwrap().is_none());
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
                handle: None,
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
    let running = super::super::lifecycle_tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    assert!(owner.target.matches_running_owner(&running, &owner.current));
    let (lifecycles, _pending) = non_running_monthly_lifecycles(running, owner.start_attempt);
    for lifecycle in lifecycles {
        assert!(
            !owner
                .target
                .matches_running_owner(&lifecycle, &owner.current)
        );
        assert!(!owner.interrupts.has_interrupt().await);
        assert!(owner.monthly.lock().unwrap().is_none());
    }
}

#[test]
#[timeout("5s")]
async fn monthly_running_owner_guard_skips_current_window_for_non_running_lifecycle() {
    let (_entry, owner, _other) = prepaid_stop_owners().await;
    let poisoned = Mutex::new(Some(owner.target.clone()));
    assert!(
        std::panic::catch_unwind(|| {
            let _held = poisoned.lock().unwrap();
            panic!("non-Running current window must remain unused");
        })
        .is_err()
    );
    let running = super::super::lifecycle_tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    let (lifecycles, _pending) = non_running_monthly_lifecycles(running, owner.start_attempt);
    for lifecycle in lifecycles {
        assert!(!owner.target.matches_running_owner(&lifecycle, &poisoned));
    }
}

#[test]
#[timeout("5s")]
async fn monthly_running_owner_guard_checks_start_permit_and_target() {
    let (_entry, owner, _other) = prepaid_stop_owners().await;
    let running = super::super::lifecycle_tests::running_lifecycle(
        owner.interrupts.clone(),
        owner.start_attempt,
        true,
    );
    assert!(owner.target.matches_running_owner(&running, &owner.current));
    let poisoned = Mutex::new(Some(owner.target.clone()));
    assert!(
        std::panic::catch_unwind(|| {
            let _held = poisoned.lock().unwrap();
            panic!("identity mismatch must not read the current window");
        })
        .is_err()
    );
    for (attempt, held) in [(Uuid::new_v4(), true), (owner.start_attempt, false)] {
        let lifecycle = super::super::lifecycle_tests::running_lifecycle(
            owner.interrupts.clone(),
            attempt,
            true,
        );
        let WorkerInstance::Running(inner) = &lifecycle else {
            unreachable!()
        };
        inner
            .concurrent_agent_permit_held
            .store(held, Ordering::Release);
        assert!(!owner.target.matches_running_owner(&lifecycle, &poisoned));
    }
    assert!(
        !owner
            .target
            .matches_running_owner(&running, &Mutex::new(None))
    );
    let equal_fields = target(owner.fingerprint, owner.start_attempt, owner.generation);
    assert!(
        !owner
            .target
            .matches_running_owner(&running, &Mutex::new(Some(equal_fields)))
    );
    // Keep the real permit alive for the entire identity test.
    drop(owner.permit);
}
