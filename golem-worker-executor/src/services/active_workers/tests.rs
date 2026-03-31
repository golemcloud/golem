use super::concurrent_agents_semaphore::ConcurrentAgentsSemaphore;
use super::fs_semaphore::*;
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::account::AccountId;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
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
