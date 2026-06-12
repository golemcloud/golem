use super::concurrent_agents_scheduler::ConcurrentAgentsScheduler;
use super::concurrent_agents_semaphore::ConcurrentAgentsSemaphore;
use super::fs_semaphore::*;
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentId;
use std::sync::Arc;
use std::time::Duration;
use test_r::{non_flaky, test, timeout};
use tokio::sync::Barrier;
use uuid::Uuid;

test_r::enable!();

fn concurrent_agents_semaphore() -> ConcurrentAgentsSemaphore {
    ConcurrentAgentsSemaphore::new()
}

fn account() -> AccountId {
    AccountId(Uuid::new_v4())
}

fn resource_entry_with_agent_limit(limit: u64) -> Arc<AtomicResourceEntry> {
    Arc::new(AtomicResourceEntry::new(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        limit,
    ))
}

fn unlimited_resource_entry() -> Arc<AtomicResourceEntry> {
    Arc::new(AtomicResourceEntry::new(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
    ))
}

#[test]
fn bytes_to_permits_exact_kb_boundary() {
    assert_eq!(bytes_to_filesystem_storage_permits(1024), 1);
}

#[test]
fn bytes_to_permits_rounds_up_partial_kb() {
    assert_eq!(bytes_to_filesystem_storage_permits(1), 1);
    assert_eq!(bytes_to_filesystem_storage_permits(1025), 2);
}

#[test]
fn bytes_to_permits_zero_bytes() {
    assert_eq!(bytes_to_filesystem_storage_permits(0), 0);
}

#[test]
fn bytes_to_permits_1gb() {
    assert_eq!(
        bytes_to_filesystem_storage_permits(1024 * 1024 * 1024),
        1_048_576
    );
}

#[test]
fn bytes_to_permits_very_large_saturates_at_u32_max() {
    assert_eq!(bytes_to_filesystem_storage_permits(u64::MAX), u32::MAX);
}

#[test]
fn bytes_to_permits_just_under_4tb() {
    let just_under: u64 = (u32::MAX as u64) * 1024;
    assert_eq!(bytes_to_filesystem_storage_permits(just_under), u32::MAX);
}

#[test]
fn storage_pool_permits_10gb() {
    let ten_gb: usize = 10 * 1024 * 1024 * 1024;
    assert_eq!(
        filesystem_storage_pool_bytes_to_permits(ten_gb),
        10 * 1024 * 1024
    );
}

fn filesystem_storage_semaphore(pool_bytes: usize) -> FilesystemStorageSemaphore {
    FilesystemStorageSemaphore::new(pool_bytes, Duration::from_millis(1))
}

#[test]
async fn try_acquire_succeeds_when_space_available() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(4 * 1024); // 4 KB pool
    let permit = filesystem_storage_semaphore.try_acquire(2 * 1024).await; // ask for 2 KB
    assert!(permit.is_some());
    assert_eq!(filesystem_storage_semaphore.available_bytes(), 2 * 1024);
}

#[test]
async fn try_acquire_returns_none_when_pool_exhausted() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(2 * 1024); // 2 KB pool
    let _permit = filesystem_storage_semaphore
        .try_acquire(2 * 1024)
        .await
        .unwrap(); // exhaust it
    let second = filesystem_storage_semaphore.try_acquire(1024).await; // no space left
    assert!(second.is_none());
}

#[test]
async fn try_acquire_zero_bytes_always_succeeds() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(0); // empty pool — 0 bytes → 0 permits
    let permit = filesystem_storage_semaphore.try_acquire(0).await;
    assert!(permit.is_some());
}

#[test]
async fn dropping_permit_returns_space_to_pool() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(4 * 1024);
    {
        let _permit = filesystem_storage_semaphore
            .try_acquire(4 * 1024)
            .await
            .unwrap();
        assert_eq!(filesystem_storage_semaphore.available_bytes(), 0);
    } // permit dropped here
    assert_eq!(filesystem_storage_semaphore.available_bytes(), 4 * 1024);
}

#[test]
async fn multiple_permits_are_independent() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(6 * 1024); // 6 KB pool
    let p1 = filesystem_storage_semaphore
        .try_acquire(2 * 1024)
        .await
        .unwrap();
    let p2 = filesystem_storage_semaphore
        .try_acquire(2 * 1024)
        .await
        .unwrap();
    assert_eq!(filesystem_storage_semaphore.available_bytes(), 2 * 1024);
    drop(p1);
    assert_eq!(filesystem_storage_semaphore.available_bytes(), 4 * 1024);
    drop(p2);
    assert_eq!(filesystem_storage_semaphore.available_bytes(), 6 * 1024);
}

#[test]
async fn try_acquire_rounds_up_to_kb_boundary() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(2 * 1024); // 2 KB = 2 permits
    // 1 byte rounds up to 1 KB = 1 permit; should leave 1 KB
    let _p = filesystem_storage_semaphore.try_acquire(1).await.unwrap();
    assert_eq!(filesystem_storage_semaphore.available_bytes(), 1024);
}

#[test]
async fn acquire_succeeds_immediately_when_space_available() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(4 * 1024);
    // pool has space so it succeeds on the first try without invoking free_up
    let permit = filesystem_storage_semaphore
        .acquire(2 * 1024, || async { false })
        .await;
    assert_eq!(permit.num_permits(), 2); // 2 KB = 2 permits
    assert_eq!(filesystem_storage_semaphore.available_bytes(), 2 * 1024);
}

#[test]
async fn acquire_succeeds_after_free_up_releases_space() {
    let filesystem_storage_semaphore = filesystem_storage_semaphore(4 * 1024);
    let _held = filesystem_storage_semaphore
        .try_acquire(4 * 1024)
        .await
        .unwrap(); // exhaust pool

    // Share the inner semaphore Arc with the closure so it can add permits
    // back to simulate a worker releasing its storage on eviction.
    let sem_arc = filesystem_storage_semaphore.inner_semaphore().clone();
    let released = std::sync::atomic::AtomicBool::new(false);
    let permit = filesystem_storage_semaphore
        .acquire(2 * 1024, || {
            let sem = sem_arc.clone();
            let already = released.fetch_or(true, std::sync::atomic::Ordering::SeqCst);
            async move {
                if !already {
                    sem.add_permits(2); // 2 permits = 2 KB freed
                    true
                } else {
                    false
                }
            }
        })
        .await;
    assert_eq!(permit.num_permits(), 2);
}

// ---------------------------------------------------------------------------
// ConcurrentAgentsSemaphore
// ---------------------------------------------------------------------------

#[test]
async fn concurrent_agents_acquire_succeeds_when_capacity_available() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    sem.register_account(acc, resource_entry_with_agent_limit(3))
        .await;

    let permit = sem.acquire(acc, || async { false }).await;
    drop(permit);
    assert_eq!(sem.available_permits(&acc).await, Some(3));
}

#[test]
async fn concurrent_agents_acquire_panics_for_unregistered_account() {
    let sem = concurrent_agents_semaphore();
    let acc = account();

    let err = match tokio::spawn(async move { sem.acquire(acc, || async { false }).await }).await {
        Ok(_) => panic!("acquire should panic for unregistered account"),
        Err(err) => err,
    };

    assert!(err.is_panic());
}

#[test]
async fn concurrent_agents_try_acquire_now_returns_none_when_at_limit() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    sem.register_account(acc, resource_entry_with_agent_limit(1))
        .await;

    let _p = sem.try_acquire_now(acc).await.unwrap(); // exhaust
    let second = sem.try_acquire_now(acc).await;
    assert!(second.is_none());
}

#[test]
async fn concurrent_agents_dropping_permit_returns_slot_to_pool() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    sem.register_account(acc, resource_entry_with_agent_limit(1))
        .await;

    {
        let _p = sem.try_acquire_now(acc).await.unwrap();
        assert_eq!(sem.available_permits(&acc).await, Some(0));
    } // permit dropped here

    assert_eq!(sem.available_permits(&acc).await, Some(1));
}

#[test]
async fn concurrent_agents_multiple_permits_consumed_and_returned_independently() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    sem.register_account(acc, resource_entry_with_agent_limit(3))
        .await;

    let p1 = sem.try_acquire_now(acc).await.unwrap();
    let p2 = sem.try_acquire_now(acc).await.unwrap();
    assert_eq!(sem.available_permits(&acc).await, Some(1));

    drop(p1);
    assert_eq!(sem.available_permits(&acc).await, Some(2));
    drop(p2);
    assert_eq!(sem.available_permits(&acc).await, Some(3));
}

#[test]
async fn concurrent_agents_different_accounts_are_independent() {
    let sem = concurrent_agents_semaphore();
    let acc1 = account();
    let acc2 = account();
    sem.register_account(acc1, resource_entry_with_agent_limit(1))
        .await;
    sem.register_account(acc2, resource_entry_with_agent_limit(2))
        .await;

    // Exhaust acc1
    let _p1 = sem.try_acquire_now(acc1).await.unwrap();
    assert!(
        sem.try_acquire_now(acc1).await.is_none(),
        "acc1 should be at limit"
    );

    // acc2 is unaffected
    let p2a = sem.try_acquire_now(acc2).await;
    let p2b = sem.try_acquire_now(acc2).await;
    assert!(p2a.is_some(), "acc2 first permit should succeed");
    assert!(p2b.is_some(), "acc2 second permit should succeed");
    assert!(
        sem.try_acquire_now(acc2).await.is_none(),
        "acc2 should now be at limit"
    );
}

#[test]
async fn concurrent_agents_unlimited_acquire_always_succeeds() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    sem.register_account(acc, unlimited_resource_entry()).await;

    // Many acquires in a row — none should block or fail.
    for _ in 0..10 {
        let permit = sem.acquire(acc, || async { false }).await;
        drop(permit);
    }
}

#[test]
async fn concurrent_agents_acquire_succeeds_immediately_when_capacity_available() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    sem.register_account(acc, resource_entry_with_agent_limit(2))
        .await;

    let permit = sem.acquire(acc, || async { false }).await;
    assert_eq!(sem.available_permits(&acc).await, Some(1));
    drop(permit);
    assert_eq!(sem.available_permits(&acc).await, Some(2));
}

#[test]
async fn concurrent_agents_acquire_calls_free_up_when_at_limit() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    let entry = resource_entry_with_agent_limit(1);
    sem.register_account(acc, entry).await;

    // Exhaust the single slot.
    let held = sem.try_acquire_now(acc).await.unwrap();

    // The free_up closure drops the held permit on the first call, simulating
    // an idle agent being evicted to make room.
    let held_cell = std::sync::Mutex::new(Some(held));
    let freed = sem
        .acquire(acc, || {
            let p = held_cell.lock().unwrap().take();
            async move { p.is_some() }
        })
        .await;

    // We should have the new permit and the pool should be exhausted.
    assert_eq!(sem.available_permits(&acc).await, Some(0));
    drop(freed);
    assert_eq!(sem.available_permits(&acc).await, Some(1));
}

#[test]
async fn concurrent_agents_register_account_is_idempotent() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    let entry = resource_entry_with_agent_limit(2);

    sem.register_account(acc, entry.clone()).await;
    sem.register_account(acc, entry.clone()).await; // second call must be a no-op

    // Pool should still be 2, not doubled to 4.
    assert_eq!(sem.available_permits(&acc).await, Some(2));
}

#[test]
async fn concurrent_agents_limit_increase_grows_pool() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    let entry = resource_entry_with_agent_limit(2);
    sem.register_account(acc, entry.clone()).await;

    // Exhaust the pool.
    let _p1 = sem.try_acquire_now(acc).await.unwrap();
    let _p2 = sem.try_acquire_now(acc).await.unwrap();
    assert!(
        sem.try_acquire_now(acc).await.is_none(),
        "pool should be exhausted at limit=2"
    );

    // Simulate a plan upgrade via the shared AtomicResourceEntry.
    entry.set_max_concurrent_agents_per_executor(4);

    // The next acquire detects the increase and grows the pool.
    let p3 = sem.try_acquire_now(acc).await;
    let p4 = sem.try_acquire_now(acc).await;
    assert!(
        p3.is_some(),
        "first new slot from plan upgrade should be available"
    );
    assert!(
        p4.is_some(),
        "second new slot from plan upgrade should be available"
    );
    assert!(
        sem.try_acquire_now(acc).await.is_none(),
        "pool should be exhausted at new limit=4"
    );
}

#[test]
async fn concurrent_agents_unlimited_to_limited_transition_allocates_permits() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    let entry = unlimited_resource_entry();
    sem.register_account(acc, entry.clone()).await;

    // Unlimited mode should always acquire immediately.
    assert!(sem.try_acquire_now(acc).await.is_some());

    // Downgrade from unlimited to a finite limit.
    entry.set_max_concurrent_agents_per_executor(2);

    // After transition, finite permits must be available.
    let p1 = sem.try_acquire_now(acc).await;
    let p2 = sem.try_acquire_now(acc).await;
    assert!(p1.is_some(), "first finite slot should be available");
    assert!(p2.is_some(), "second finite slot should be available");
    assert!(
        sem.try_acquire_now(acc).await.is_none(),
        "pool should be exhausted at new finite limit"
    );
}

#[test]
async fn concurrent_agents_limit_decrease_shrinks_available_pool() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    let entry = resource_entry_with_agent_limit(4);
    sem.register_account(acc, entry.clone()).await;

    // Consume 2 of 4 slots — 2 remain available.
    let _p1 = sem.try_acquire_now(acc).await.unwrap();
    let _p2 = sem.try_acquire_now(acc).await.unwrap();
    assert_eq!(sem.available_permits(&acc).await, Some(2));

    // Simulate a plan downgrade to limit=2 via the shared AtomicResourceEntry.
    entry.set_max_concurrent_agents_per_executor(2);

    // The next acquire detects the decrease and consumes the excess available
    // permits. The 2 running agents keep their held permits.
    let result = sem.try_acquire_now(acc).await;
    // After sync: 2 agents running, new cap=2, so 0 slots remain for new agents.
    assert!(
        result.is_none(),
        "no slots available after downgrade consumed excess permits"
    );
    assert_eq!(sem.available_permits(&acc).await, Some(0));

    // When one of the running agents stops, its permit is returned via Drop.
    drop(_p1);
    assert_eq!(sem.available_permits(&acc).await, Some(1));
}

#[test]
async fn concurrent_agents_limit_decrease_does_not_affect_running_agents() {
    let sem = concurrent_agents_semaphore();
    let acc = account();
    let entry = resource_entry_with_agent_limit(3);
    sem.register_account(acc, entry.clone()).await;

    // All 3 slots consumed by running agents.
    let p1 = sem.try_acquire_now(acc).await.unwrap();
    let p2 = sem.try_acquire_now(acc).await.unwrap();
    let p3 = sem.try_acquire_now(acc).await.unwrap();
    assert_eq!(sem.available_permits(&acc).await, Some(0));

    // Downgrade to limit=1.
    entry.set_max_concurrent_agents_per_executor(1);

    // Sync is triggered on the next try_acquire_now call. No available permits
    // to consume, so nothing changes for the currently running agents.
    let result = sem.try_acquire_now(acc).await;
    assert!(result.is_none(), "no new agents can start at new limit=1");

    // All 3 running agents are still alive — drop them one by one.
    drop(p3);
    drop(p2);
    drop(p1);
    // After all drops the pool reflects returned permits; new starts are gated
    // by new_limit=1 on the next acquire call.
    let new_agent = sem.try_acquire_now(acc).await;
    assert!(
        new_agent.is_some(),
        "one new agent can start within new limit=1"
    );
    assert!(
        sem.try_acquire_now(acc).await.is_none(),
        "second new agent must wait at limit=1"
    );
}

#[test]
async fn concurrent_agents_plan_upgrade_does_not_over_add_under_parallel_acquires() {
    let sem = Arc::new(concurrent_agents_semaphore());
    let acc = account();
    let entry = resource_entry_with_agent_limit(1);
    sem.register_account(acc, entry.clone()).await;

    // Keep one running agent so the upgraded capacity for new acquires is 999.
    let _held = sem.try_acquire_now(acc).await.unwrap();

    entry.set_max_concurrent_agents_per_executor(1000);

    let task_count = 4000;
    let barrier = Arc::new(Barrier::new(task_count + 1));

    let mut tasks = Vec::with_capacity(task_count);
    for _ in 0..task_count {
        let sem = sem.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            sem.try_acquire_now(acc).await
        }));
    }

    barrier.wait().await;

    let mut acquired = Vec::new();
    for task in tasks {
        let permit = task.await.unwrap();
        if let Some(permit) = permit {
            acquired.push(permit);
        }
    }

    assert!(
        acquired.len() <= 999,
        "upgraded capacity is 999 new slots (one already running), but got {}",
        acquired.len()
    );
}

// ---------------------------------------------------------------------------
// ConcurrentAgentsScheduler — fairness tests
// ---------------------------------------------------------------------------

fn agent(name: &str) -> AgentId {
    AgentId {
        component_id: ComponentId::new(),
        agent_id: name.to_string(),
    }
}

fn scheduler() -> Arc<ConcurrentAgentsScheduler> {
    Arc::new(ConcurrentAgentsScheduler::new())
}

async fn wait_for_queue_len(sched: &ConcurrentAgentsScheduler, acc: &AccountId, expected: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if sched.queue_len(acc).await == Some(expected) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timed out waiting for scheduler queue length");
}

#[test]
async fn scheduler_acquire_panics_for_unregistered_account() {
    let sched = scheduler();
    let acc = account();

    let err = match tokio::spawn(async move { sched.acquire(acc, agent("A")).await }).await {
        Ok(_) => panic!("scheduler acquire should panic for unregistered account"),
        Err(err) => err,
    };

    assert!(err.is_panic());
}

#[test]
#[timeout("5s")]
#[non_flaky(10)]
async fn scheduler_fifo_admission() {
    let sched = scheduler();
    let acc = account();
    sched
        .register_account(acc, resource_entry_with_agent_limit(2))
        .await;

    // A and B acquire first — should succeed immediately.
    let a = sched.acquire(acc, agent("A")).await;
    let b = sched.acquire(acc, agent("B")).await;
    assert_eq!(sched.running_count(&acc).await, Some(2));
    assert_eq!(sched.queue_len(&acc).await, Some(0));

    // C queues first, then D queues behind it.
    let sched2 = sched.clone();
    let acc2 = acc;
    let c_handle = tokio::spawn(async move { sched2.acquire(acc2, agent("C")).await });
    wait_for_queue_len(&sched, &acc, 1).await;

    let sched3 = sched.clone();
    let d_handle = tokio::spawn(async move { sched3.acquire(acc, agent("D")).await });
    wait_for_queue_len(&sched, &acc, 2).await;

    // Drop A => C should get slot.
    drop(a);
    let c = c_handle.await.unwrap();
    assert_eq!(sched.running_count(&acc).await, Some(2));

    // Drop B => D should get slot.
    drop(b);
    let d = d_handle.await.unwrap();
    assert_eq!(sched.running_count(&acc).await, Some(2));
    assert_eq!(sched.queue_len(&acc).await, Some(0));

    drop(c);
    drop(d);
}

#[test]
async fn scheduler_back_of_queue_reentry() {
    let sched = scheduler();
    let acc = account();
    sched
        .register_account(acc, resource_entry_with_agent_limit(1))
        .await;

    // A acquires the single slot.
    let a = sched.acquire(acc, agent("A")).await;
    assert_eq!(sched.running_count(&acc).await, Some(1));

    // B queues first, then C queues behind it.
    let sched2 = sched.clone();
    let b_handle = tokio::spawn(async move { sched2.acquire(acc, agent("B")).await });
    wait_for_queue_len(&sched, &acc, 1).await;

    let sched3 = sched.clone();
    let c_handle = tokio::spawn(async move { sched3.acquire(acc, agent("C")).await });
    wait_for_queue_len(&sched, &acc, 2).await;

    // A finishes and re-requests. It should go to the back of the queue.
    drop(a);

    // B should get the slot first (it was queued before A's re-request).
    let b = b_handle.await.unwrap();
    assert_eq!(sched.running_count(&acc).await, Some(1));

    // Now A re-requests — it should go to the back of the queue behind C.
    let sched4 = sched.clone();
    let a2_handle = tokio::spawn(async move { sched4.acquire(acc, agent("A")).await });
    wait_for_queue_len(&sched, &acc, 2).await;

    // Drop B => C should get next, not A.
    drop(b);
    let c = c_handle.await.unwrap();
    assert_eq!(sched.running_count(&acc).await, Some(1));

    // Drop C => finally A gets the slot.
    drop(c);
    let _a2 = a2_handle.await.unwrap();
    assert_eq!(sched.running_count(&acc).await, Some(1));
    assert_eq!(sched.queue_len(&acc).await, Some(0));
}

#[test]
async fn scheduler_cancelled_waiter_is_skipped() {
    let sched = scheduler();
    let acc = account();
    sched
        .register_account(acc, resource_entry_with_agent_limit(1))
        .await;

    let a = sched.acquire(acc, agent("A")).await;

    // B queues then is cancelled.
    let sched2 = sched.clone();
    let b_handle = tokio::spawn(async move { sched2.acquire(acc, agent("B")).await });
    wait_for_queue_len(&sched, &acc, 1).await;

    // C also queues.
    let sched3 = sched.clone();
    let c_handle = tokio::spawn(async move { sched3.acquire(acc, agent("C")).await });
    wait_for_queue_len(&sched, &acc, 2).await;

    // Cancel B.
    b_handle.abort();
    let err = match b_handle.await {
        Ok(_) => panic!("cancelled waiter should not complete successfully"),
        Err(err) => err,
    };
    assert!(err.is_cancelled());

    // Drop A => scheduler should skip cancelled B and grant C.
    drop(a);
    let _c = c_handle.await.unwrap();
    assert_eq!(sched.running_count(&acc).await, Some(1));
}

#[test]
async fn scheduler_unlimited_bypasses_queue() {
    let sched = scheduler();
    let acc = account();
    sched
        .register_account(acc, unlimited_resource_entry())
        .await;

    // Many concurrent acquires — all should succeed immediately.
    let mut permits = Vec::new();
    for i in 0..20 {
        let p = sched.acquire(acc, agent(&format!("W{i}"))).await;
        permits.push(p);
    }
    // Unlimited accounts bypass the queue — no queueing should happen.
    assert_eq!(sched.queue_len(&acc).await, Some(0));
    drop(permits);
}

#[test]
async fn scheduler_accounts_are_independent() {
    let sched = scheduler();
    let acc1 = account();
    let acc2 = account();
    sched
        .register_account(acc1, resource_entry_with_agent_limit(1))
        .await;
    sched
        .register_account(acc2, resource_entry_with_agent_limit(1))
        .await;

    // Both accounts can have one running agent simultaneously.
    let a1 = sched.acquire(acc1, agent("A1")).await;
    let a2 = sched.acquire(acc2, agent("A2")).await;
    assert_eq!(sched.running_count(&acc1).await, Some(1));
    assert_eq!(sched.running_count(&acc2).await, Some(1));
    drop(a1);
    drop(a2);
}

// ── Component module charge against the admission gate ───────────────────────

mod component_module_charge {
    use super::super::admission::{AdmissionController, AdmissionPolicy};
    use super::super::component_charge::ComponentChargeRegistry;
    use super::super::memory_probe::{MemoryProbe, MemorySnapshot};
    use super::super::{ComponentChargeKey, GateChargeSource, HeldComponentCharge};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use std::sync::Arc;
    use test_r::test;
    use uuid::Uuid;

    /// Probe reporting a fixed limit and zero resident memory, so the gate's
    /// reservation is driven entirely by what is charged through it.
    #[derive(Debug)]
    struct FixedProbe {
        limit: u64,
    }

    impl MemoryProbe for FixedProbe {
        fn snapshot(&self) -> MemorySnapshot {
            MemorySnapshot {
                limit_bytes: self.limit,
                current_bytes: 0,
            }
        }
    }

    fn key() -> ComponentChargeKey {
        (ComponentId(Uuid::new_v4()), ComponentRevision::INITIAL)
    }

    /// The first worker of a component reserves the module's bytes with the gate,
    /// so admissible headroom drops by the module size before it faults into
    /// memory. A second worker of the same component reserves nothing more, and
    /// the reservation is released only when the last worker unloads.
    #[test]
    async fn module_charge_reserves_with_gate_until_last_worker_unloads() {
        let limit = 1000u64;
        let module_bytes = 200u64;
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe { limit }),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let registry = ComponentChargeRegistry::new(GateChargeSource {
            admission: Some(controller.clone()),
        });
        let component = key();

        assert_eq!(controller.headroom_bytes(), limit);

        let first = registry.acquire(component, module_bytes).await;
        assert_eq!(
            controller.headroom_bytes(),
            limit - module_bytes,
            "first worker of a component must reserve the module size with the gate"
        );

        let second = registry.acquire(component, module_bytes).await;
        assert_eq!(
            controller.headroom_bytes(),
            limit - module_bytes,
            "a second worker of the same component must not reserve the module again"
        );

        drop(first);
        assert_eq!(
            controller.headroom_bytes(),
            limit - module_bytes,
            "the module stays reserved while any worker of the component is resident"
        );

        drop(second);
        assert_eq!(
            controller.headroom_bytes(),
            limit,
            "the module reservation is released when the last worker unloads"
        );
    }

    /// A `RunningWorker` stores its component charge as
    /// `Box<dyn HeldComponentCharge>` and releases it by dropping that box when
    /// the worker unloads. Dropping the box must still release the module
    /// reservation with the gate, i.e. the concrete charge's release runs through
    /// the trait object exactly as it would for a live worker.
    #[test]
    async fn dropping_boxed_charge_releases_the_reservation() {
        let limit = 1000u64;
        let module_bytes = 200u64;
        let controller = Arc::new(AdmissionController::new(
            Box::new(FixedProbe { limit }),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));
        let registry = ComponentChargeRegistry::new(GateChargeSource {
            admission: Some(controller.clone()),
        });

        let charge = registry.acquire(key(), module_bytes).await;
        // Store it exactly as RunningWorker does.
        let boxed: Box<dyn HeldComponentCharge> = Box::new(charge);
        assert_eq!(controller.headroom_bytes(), limit - module_bytes);

        drop(boxed);
        assert_eq!(
            controller.headroom_bytes(),
            limit,
            "dropping the boxed charge (as on worker unload) must release the reservation"
        );
    }
}

// ── ConcurrentAgentsScheduler — model-based liveness property ────────────────
//
// The scheduler keeps its own `running_count` integer alongside the real tokio
// semaphore permits. The two must stay in lockstep: every increment of
// `running_count` must be matched by exactly one decrement, regardless of how a
// granted slot is disposed of (released by a live worker, or dropped inside a
// cancelled waiter's oneshot channel). If they drift, the scheduler wedges —
// `running_count` sticks at the limit while permits are actually free, and
// every future acquire queues forever. This is the production deadlock the
// property is designed to catch.
//
// The model drives random interleavings of acquire / release / cancel against
// the real scheduler and, after every step, asserts the *liveness* invariant:
// whenever fewer permits are genuinely held than the limit allows, a fresh
// acquire must succeed promptly. A leaked `running_count` violates this.
mod scheduler_liveness {
    use super::super::concurrent_agents_scheduler::{
        ConcurrentAgentPermit, ConcurrentAgentsScheduler,
    };
    use super::{account, agent, resource_entry_with_agent_limit};
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::test;
    use tokio::task::JoinHandle;

    /// One step in a randomized scheduler workload.
    #[derive(Debug, Clone)]
    enum Op {
        /// Acquire a permit and hold it (resolves immediately if capacity is
        /// free, otherwise the in-flight acquire is parked in `pending`).
        Acquire,
        /// Release a currently-held permit, if any.
        Release(prop::sample::Index),
        /// Cancel an in-flight (likely queued) acquire, if any. Exercises both
        /// "cancelled while queued" and "cancelled just after being granted".
        CancelPending(prop::sample::Index),
        /// Release a held permit and, in the same step, cancel an in-flight
        /// acquire. This is the deadly race: the released slot may be granted
        /// to the in-flight acquire's oneshot and then the acquire is cancelled
        /// before it can receive it. The slot must still be released.
        ReleaseThenCancel(prop::sample::Index, prop::sample::Index),
    }

    fn arb_ops() -> impl Strategy<Value = Vec<Op>> {
        prop::collection::vec(
            prop_oneof![
                3 => Just(Op::Acquire),
                2 => any::<prop::sample::Index>().prop_map(Op::Release),
                2 => any::<prop::sample::Index>().prop_map(Op::CancelPending),
                3 => (any::<prop::sample::Index>(), any::<prop::sample::Index>())
                    .prop_map(|(a, b)| Op::ReleaseThenCancel(a, b)),
            ],
            1..60,
        )
    }

    /// Let any synchronous grant/drain bookkeeping triggered by a release or
    /// cancellation settle before the next observation.
    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    proptest! {
        // Cap shrink iterations so a failing (buggy) run cannot spend minutes
        // re-running wedging inputs against the overall timeout while shrinking.
        #![proptest_config(ProptestConfig { cases: 128, max_shrink_iters: 64, ..ProptestConfig::default() })]

        /// Liveness: under any interleaving of acquire / release / cancel, the
        /// scheduler never wedges. After each step, if fewer permits are held
        /// than the limit, a fresh acquire must succeed within a short timeout.
        /// At the end, draining all held permits must let the account return to
        /// full capacity.
        #[test]
        fn scheduler_never_wedges_under_churn(
            limit in 1usize..6,
            ops in arb_ops(),
        ) {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
                .build()
                .unwrap();

            rt.block_on(async move {
                // Bound the whole case so a wedge fails fast and deterministically
                // rather than hanging the test suite. A correct scheduler completes
                // a 60-op workload in well under a second; the bug deadlocks here,
                // so a tight bound makes the failure (and any shrinking) quick.
                let outcome = tokio::time::timeout(Duration::from_secs(3), async move {
                    run_workload(limit, ops).await
                })
                .await;

                match outcome {
                    Ok(result) => result,
                    Err(_elapsed) => Err(TestCaseError::fail(
                        "scheduler workload did not complete within the overall timeout — \
                         deadlock (running_count leaked above true occupancy)",
                    )),
                }
            })?;
        }
    }

    /// Drives one randomized workload against a freshly-registered account and
    /// returns `Err` if the liveness invariant is ever violated. Factored out of
    /// the proptest body so the whole run can be wrapped in an overall timeout.
    async fn run_workload(limit: usize, ops: Vec<Op>) -> Result<(), TestCaseError> {
        // Short per-acquire timeout: a wedge must surface quickly, but allow
        // enough slack for genuine multi-thread scheduling jitter.
        const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

        let sched = Arc::new(ConcurrentAgentsScheduler::new());
        let acc = account();
        sched
            .register_account(acc, resource_entry_with_agent_limit(limit as u64))
            .await;

        // Permits we are deliberately holding (count against the limit).
        let mut held: Vec<ConcurrentAgentPermit> = Vec::new();
        // In-flight acquires not yet resolved (queued or just granted).
        let mut pending: Vec<JoinHandle<ConcurrentAgentPermit>> = Vec::new();
        let mut counter = 0usize;

        for op in ops {
            match op {
                Op::Acquire => {
                    counter += 1;
                    let sched = sched.clone();
                    let name = format!("W{counter}");
                    let handle =
                        tokio::spawn(async move { sched.acquire(acc, agent(&name)).await });
                    pending.push(handle);
                }
                Op::Release(idx) => {
                    if !held.is_empty() {
                        let i = idx.index(held.len());
                        drop(held.remove(i));
                    }
                }
                Op::CancelPending(idx) => {
                    if !pending.is_empty() {
                        let i = idx.index(pending.len());
                        pending.remove(i).abort();
                    }
                }
                Op::ReleaseThenCancel(ri, ci) => {
                    if !held.is_empty() {
                        let i = ri.index(held.len());
                        drop(held.remove(i));
                    }
                    if !pending.is_empty() {
                        let i = ci.index(pending.len());
                        pending.remove(i).abort();
                    }
                }
            }

            settle().await;

            // Collect any in-flight acquires that have now resolved into
            // held permits, so `held.len()` reflects true occupancy.
            let mut still_pending = Vec::new();
            for h in pending.drain(..) {
                if h.is_finished() {
                    if let Ok(permit) = h.await {
                        held.push(permit);
                    }
                    // Cancelled/aborted handles are simply dropped.
                } else {
                    still_pending.push(h);
                }
            }
            pending = still_pending;

            // Liveness invariant: if we are below the limit, a fresh
            // acquire must succeed promptly. A leaked running_count
            // would make this hang and trip the timeout.
            if held.len() < limit {
                let probe =
                    tokio::time::timeout(PROBE_TIMEOUT, sched.acquire(acc, agent("probe"))).await;
                prop_assert!(
                    probe.is_ok(),
                    "scheduler wedged: held {} < limit {} but acquire timed out",
                    held.len(),
                    limit,
                );
                // Release the probe immediately.
                drop(probe.ok());
                settle().await;
            }
        }

        // Abort everything still queued, drop all held permits, and
        // confirm the account drains back to full capacity: `limit`
        // fresh acquires must all succeed.
        for h in pending.drain(..) {
            h.abort();
            let _ = h.await;
        }
        held.clear();
        settle().await;

        let mut drained = Vec::new();
        for _ in 0..limit {
            let p = tokio::time::timeout(PROBE_TIMEOUT, sched.acquire(acc, agent("drain"))).await;
            prop_assert!(
                p.is_ok(),
                "scheduler did not return to full capacity after churn",
            );
            drained.push(p.unwrap());
        }
        Ok(())
    }
}
