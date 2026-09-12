use super::*;
use futures::future::join_all;
use std::sync::atomic::AtomicU64;
use test_r::test;

fn test_cache(name: &'static str) -> Cache<u64, (), u64, String> {
    Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::None,
        name,
    )
}

fn bounded_cache(capacity: usize, name: &'static str) -> Cache<u64, (), u64, String> {
    Cache::new(
        Some(capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::None,
        name,
    )
}

// ---- Race condition proof tests ----

#[test]
async fn broadcast_late_subscribe_misses_message() {
    let (tx, _rx) = tokio::sync::broadcast::channel::<Result<u64, String>>(1);
    let tx_clone = tx.clone();

    let _ = tx.send(Ok(42));

    let mut rx = tx_clone.subscribe();
    let result = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(
        result.is_err(),
        "broadcast late subscriber misses the message (proving the original bug)"
    );
}

#[test]
async fn watch_late_subscribe_sees_message() {
    // Note: `_rx` keeps the initial receiver alive, so `send` succeeds.
    let (tx, _rx) = tokio::sync::watch::channel::<Option<Result<u64, String>>>(None);

    let _ = tx.send(Some(Ok(42)));

    let mut rx = tx.subscribe();
    let result =
        tokio::time::timeout(Duration::from_millis(200), rx.wait_for(|v| v.is_some())).await;
    assert!(result.is_ok(), "watch late subscriber sees the message");
    let val = result.unwrap().unwrap().clone().unwrap();
    assert_eq!(val, Ok(42));
}

#[test]
async fn watch_send_without_receivers_loses_value() {
    // The initial receiver is dropped immediately, matching how the cache
    // creates the channel in `get_or_add_as_pending`.
    let (tx, _) = tokio::sync::watch::channel::<Option<Result<u64, String>>>(None);

    assert!(tx.send(Some(Ok(42))).is_err());

    let mut rx = tx.subscribe();
    let result =
        tokio::time::timeout(Duration::from_millis(200), rx.wait_for(|v| v.is_some())).await;
    assert!(
        result.is_err(),
        "send on a receiver-less watch channel fails without storing the value \
         (proving the lost-wakeup bug)"
    );
}

#[test]
async fn watch_send_replace_without_receivers_stores_value() {
    let (tx, _) = tokio::sync::watch::channel::<Option<Result<u64, String>>>(None);

    tx.send_replace(Some(Ok(42)));

    let mut rx = tx.subscribe();
    let result =
        tokio::time::timeout(Duration::from_millis(200), rx.wait_for(|v| v.is_some())).await;
    assert!(
        result.is_ok(),
        "send_replace stores the value even with no receivers"
    );
    let val = result.unwrap().unwrap().clone().unwrap();
    assert_eq!(val, Ok(42));
}

// ---- Basic operations ----

#[test]
async fn get_or_insert_simple_inserts_and_returns_value() {
    let cache = test_cache("basic_insert");
    let result = cache.get_or_insert_simple(&1, || async { Ok(42u64) }).await;
    assert_eq!(result, Ok(42));
}

#[test]
async fn get_or_insert_simple_returns_cached_value_on_second_call() {
    let cache = test_cache("cached_second");
    let call_count = Arc::new(AtomicU64::new(0));

    let cc = call_count.clone();
    let r1 = cache
        .get_or_insert_simple(&1, || async move {
            cc.fetch_add(1, Ordering::SeqCst);
            Ok(42u64)
        })
        .await;

    let cc = call_count.clone();
    let r2 = cache
        .get_or_insert_simple(&1, || async move {
            cc.fetch_add(1, Ordering::SeqCst);
            Ok(99u64)
        })
        .await;

    assert_eq!(r1, Ok(42));
    assert_eq!(r2, Ok(42));
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
async fn try_get_returns_none_for_missing_key() {
    let cache = test_cache("try_get_miss");
    assert_eq!(cache.try_get(&1).await, None);
}

#[test]
async fn try_get_returns_value_for_cached_key() {
    let cache = test_cache("try_get_hit");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();
    assert_eq!(cache.try_get(&1).await, Some(42));
}

#[test]
async fn get_returns_none_for_missing_key() {
    let cache = test_cache("get_miss");
    assert_eq!(cache.get(&1).await, None);
}

#[test]
async fn get_returns_value_for_cached_key() {
    let cache = test_cache("get_hit");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();
    assert_eq!(cache.get(&1).await, Some(42));
}

#[test]
async fn remove_if_cached_removes_when_predicate_matches() {
    let cache = test_cache("remove_if_cached_match");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();

    let removed = cache.remove_if_cached(&1, |v| *v == 42).await;
    assert!(removed);
    assert!(!cache.contains_key(&1).await);
}

#[test]
async fn remove_if_cached_no_op_when_predicate_does_not_match() {
    let cache = test_cache("remove_if_cached_no_match");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();

    let removed = cache.remove_if_cached(&1, |v| *v == 100).await;
    assert!(!removed);
    assert!(cache.contains_key(&1).await);
    assert_eq!(cache.try_get(&1).await, Some(42));
}

#[test]
async fn remove_if_cached_predicate_is_evaluated_under_the_remove_lock() {
    // Regression: an earlier implementation evaluated the predicate during
    // a separate read pass and then later removed any `Cached` entry under
    // the same key. If the original value was replaced by a fresh one
    // between those two steps (for example because the caller dropped its
    // stale entry, another caller produced a fresh one, and then this
    // method ran with a stale predicate), the fresh value could be wrongly
    // removed. The current implementation runs the predicate inside the
    // atomic remove, so this can no longer happen.
    let cache: Cache<u64, (), Arc<u64>, String> = Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::None,
        "remove_if_cached_atomic",
    );
    let v1 = cache
        .get_or_insert_simple(&1, async || Ok(Arc::new(42u64)))
        .await
        .unwrap();
    cache.remove(&1).await;
    let v2 = cache
        .get_or_insert_simple(&1, async || Ok(Arc::new(43u64)))
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&v1, &v2));

    // Try to remove using a predicate that only matches v1's identity.
    let removed = cache
        .remove_if_cached(&1, |current| Arc::ptr_eq(current, &v1))
        .await;
    assert!(!removed, "v1-targeted removal must not delete v2");
    assert_eq!(cache.try_get(&1).await, Some(v2));
}

#[test]
async fn remove_if_cached_does_not_remove_pending() {
    let cache = test_cache("remove_if_cached_pending");
    let entered = Arc::new(tokio::sync::Notify::new());
    let proceed = Arc::new(tokio::sync::Notify::new());

    let cache_clone = cache.clone();
    let e = entered.clone();
    let p = proceed.clone();
    let producer = tokio::spawn(async move {
        cache_clone
            .get_or_insert_simple(&1, || async move {
                e.notify_one();
                p.notified().await;
                Ok(42u64)
            })
            .await
    });

    entered.notified().await;
    // Pending entries are never removed by `remove_if_cached`.
    let removed = cache.remove_if_cached(&1, |_| true).await;
    assert!(!removed);
    assert!(cache.contains_key(&1).await);

    proceed.notify_one();
    let _ = producer.await.unwrap();
    assert_eq!(cache.try_get(&1).await, Some(42));
}

#[test]
async fn entries_older_than_only_returns_expired_cached_entries_without_refreshing_them() {
    let cache = test_cache("entries_older_than");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        cache.entries_older_than(Duration::from_millis(20)).await,
        vec![(1, 42)]
    );

    assert_eq!(
        cache.entries_older_than(Duration::from_millis(20)).await,
        vec![(1, 42)],
        "inspecting an expired entry must not make it fresh"
    );
    assert_eq!(cache.get(&1).await, Some(42));
    assert!(
        cache
            .entries_older_than(Duration::from_millis(20))
            .await
            .is_empty(),
        "a normal cache lookup must refresh the entry"
    );
}

#[test]
async fn remove_if_cached_older_than_requires_both_expiry_and_predicate() {
    let cache = test_cache("remove_if_cached_older_than");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();

    assert!(
        !cache
            .remove_if_cached_older_than(&1, Duration::from_millis(20), |_| true)
            .await,
        "a fresh entry must not be removed"
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !cache
            .remove_if_cached_older_than(&1, Duration::from_millis(20), |value| *value == 100)
            .await,
        "an expired entry whose value does not match must not be removed"
    );
    assert!(
        cache
            .remove_if_cached_older_than(&1, Duration::from_millis(20), |value| *value == 42)
            .await
    );
    assert!(!cache.contains_key(&1).await);
}

#[test]
async fn remove_deletes_cached_value() {
    let cache = test_cache("remove_test");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();
    assert!(cache.contains_key(&1).await);

    cache.remove(&1).await;

    assert!(!cache.contains_key(&1).await);
    assert_eq!(cache.get(&1).await, None);
}

#[test]
async fn remove_nonexistent_key_is_noop() {
    let cache = test_cache("remove_noop");
    cache.remove(&999).await;
    assert!(!cache.contains_key(&999).await);
}

#[test]
async fn contains_key_reflects_state() {
    let cache = test_cache("contains_key");
    assert!(!cache.contains_key(&1).await);

    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();
    assert!(cache.contains_key(&1).await);

    cache.remove(&1).await;
    assert!(!cache.contains_key(&1).await);
}

#[test]
async fn iter_returns_all_cached_pairs() {
    let cache = test_cache("iter_test");
    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();
    cache
        .get_or_insert_simple(&2, || async { Ok(20u64) })
        .await
        .unwrap();
    cache
        .get_or_insert_simple(&3, || async { Ok(30u64) })
        .await
        .unwrap();

    let mut pairs = cache.iter().await;
    pairs.sort_by_key(|(k, _)| *k);
    assert_eq!(pairs, vec![(1, 10), (2, 20), (3, 30)]);
}

#[test]
async fn keys_returns_all_keys() {
    let cache = test_cache("keys_test");
    cache
        .get_or_insert_simple(&5, || async { Ok(50u64) })
        .await
        .unwrap();
    cache
        .get_or_insert_simple(&10, || async { Ok(100u64) })
        .await
        .unwrap();

    let mut keys = cache.keys().await;
    keys.sort();
    assert_eq!(keys, vec![5, 10]);
}

// ---- Error handling ----

#[test]
async fn get_or_insert_propagates_error() {
    let cache = test_cache("error_propagate");
    let result: Result<u64, String> = cache
        .get_or_insert_simple(&1, || async { Err("boom".to_string()) })
        .await;
    assert_eq!(result, Err("boom".to_string()));
}

#[test]
async fn failed_insert_does_not_cache_allows_retry() {
    let cache = test_cache("error_retry");
    let call_count = Arc::new(AtomicU64::new(0));

    let cc = call_count.clone();
    let r1 = cache
        .get_or_insert_simple(&1, || async move {
            cc.fetch_add(1, Ordering::SeqCst);
            Err::<u64, _>("fail".to_string())
        })
        .await;
    assert!(r1.is_err());

    let cc = call_count.clone();
    let r2 = cache
        .get_or_insert_simple(&1, || async move {
            cc.fetch_add(1, Ordering::SeqCst);
            Ok(42u64)
        })
        .await;
    assert_eq!(r2, Ok(42));
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
async fn concurrent_waiters_receive_error_from_owner() {
    let cache = test_cache("error_concurrent");

    let futs: Vec<_> = (0..5)
        .map(|_| {
            let cache = cache.clone();
            async move {
                cache
                    .get_or_insert_simple(&1, || async { Err::<u64, _>("fail".to_string()) })
                    .await
            }
        })
        .collect();

    let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
        .await
        .expect("concurrent error test timed out");

    for r in results {
        assert!(r.is_err());
    }
}

// ---- Concurrency / deduplication ----

#[test]
async fn f2_called_only_once_for_concurrent_requests() {
    let cache = test_cache("dedup");
    let call_count = Arc::new(AtomicU64::new(0));

    let futs: Vec<_> = (0..5)
        .map(|_| {
            let cache = cache.clone();
            let call_count = call_count.clone();
            async move {
                cache
                    .get_or_insert_simple(&1, || async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Ok(42u64)
                    })
                    .await
            }
        })
        .collect();

    let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
        .await
        .expect("dedup test timed out");

    for r in &results {
        assert_eq!(r, &Ok(42));
    }
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "f2 should be called exactly once"
    );
}

#[test]
async fn concurrent_requests_for_different_keys_execute_independently() {
    let cache = test_cache("different_keys");

    let futs: Vec<_> = (0u64..10)
        .map(|i| {
            let cache = cache.clone();
            async move {
                cache
                    .get_or_insert_simple(&i, || async move { Ok(i * 10) })
                    .await
            }
        })
        .collect();

    let results = join_all(futs).await;
    for (i, r) in results.into_iter().enumerate() {
        assert_eq!(r.unwrap(), (i as u64) * 10);
    }
}

#[test]
async fn get_waits_for_pending_item_to_complete() {
    let cache = test_cache("get_pending");
    let f2_entered = Arc::new(tokio::sync::Notify::new());
    let f2_proceed = Arc::new(tokio::sync::Notify::new());

    let cache_clone = cache.clone();
    let entered = f2_entered.clone();
    let proceed = f2_proceed.clone();
    let producer = tokio::spawn(async move {
        cache_clone
            .get_or_insert_simple(&1, || async move {
                entered.notify_one();
                proceed.notified().await;
                Ok(42u64)
            })
            .await
    });

    // Wait until f2 is actually running (item is pending)
    f2_entered.notified().await;

    // try_get should return None while pending
    assert_eq!(cache.try_get(&1).await, None);
    // contains_key should still be true (pending entry exists)
    assert!(cache.contains_key(&1).await);

    // Start a get that should wait for the pending item
    let cache_clone = cache.clone();
    let getter = tokio::spawn(async move { cache_clone.get(&1).await });

    // Let the getter task start and subscribe
    tokio::task::yield_now().await;

    // Unblock the producer
    f2_proceed.notify_one();

    let producer_result = tokio::time::timeout(Duration::from_secs(5), producer)
        .await
        .expect("producer timed out")
        .unwrap();
    assert_eq!(producer_result, Ok(42));

    let getter_result = tokio::time::timeout(Duration::from_secs(5), getter)
        .await
        .expect("getter timed out")
        .unwrap();
    assert_eq!(getter_result, Some(42));
}

#[test]
async fn get_returns_none_when_pending_item_fails() {
    let cache = test_cache("get_pending_fail");
    let f2_entered = Arc::new(tokio::sync::Notify::new());
    let f2_proceed = Arc::new(tokio::sync::Notify::new());

    let cache_clone = cache.clone();
    let entered = f2_entered.clone();
    let proceed = f2_proceed.clone();
    let producer = tokio::spawn(async move {
        cache_clone
            .get_or_insert_simple(&1, || async move {
                entered.notify_one();
                proceed.notified().await;
                Err::<u64, _>("fail".to_string())
            })
            .await
    });

    f2_entered.notified().await;

    let cache_clone = cache.clone();
    let getter = tokio::spawn(async move { cache_clone.get(&1).await });

    tokio::task::yield_now().await;
    f2_proceed.notify_one();

    let producer_result = producer.await.unwrap();
    assert!(producer_result.is_err());

    let getter_result = tokio::time::timeout(Duration::from_secs(5), getter)
        .await
        .expect("getter timed out")
        .unwrap();
    assert_eq!(getter_result, None);
}

#[test]
async fn concurrent_get_or_insert_with_instant_completion_does_not_hang() {
    for iteration in 0..100 {
        let cache = test_cache("instant_race");

        let futs: Vec<_> = (0..10)
            .map(|_| {
                let cache = cache.clone();
                async move {
                    cache
                        .get_or_insert_simple(&1u64, || async { Ok(42u64) })
                        .await
                }
            })
            .collect();

        let result = tokio::time::timeout(Duration::from_secs(5), join_all(futs)).await;

        match result {
            Ok(results) => {
                for r in results {
                    assert_eq!(r.unwrap(), 42);
                }
            }
            Err(_) => {
                panic!("Timed out on iteration {iteration} - cache race condition caused a hang");
            }
        }

        cache.remove(&1u64).await;
    }
}

#[test]
async fn concurrent_insert_then_error_then_retry_succeeds() {
    let cache = test_cache("insert_error_retry");

    // First round: all concurrent requests get an error
    let futs: Vec<_> = (0..5)
        .map(|_| {
            let cache = cache.clone();
            async move {
                cache
                    .get_or_insert_simple(&1, || async move { Err::<u64, _>("fail".to_string()) })
                    .await
            }
        })
        .collect();

    let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
        .await
        .expect("timed out");
    for r in &results {
        assert!(r.is_err());
    }

    // Second round: retry should succeed, key should not be stuck
    let r = cache.get_or_insert_simple(&1, || async { Ok(99u64) }).await;
    assert_eq!(r, Ok(99));
}

// ---- Eviction ----

#[test]
async fn lru_eviction_when_capacity_exceeded() {
    let cache = bounded_cache(3, "lru_eviction");

    // Insert keys with small gaps so last_access times differ
    cache
        .get_or_insert_simple(&0, || async { Ok(0u64) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    cache
        .get_or_insert_simple(&1, || async { Ok(1u64) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    cache
        .get_or_insert_simple(&2, || async { Ok(2u64) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;

    // Touch key 0 to make it the most recently used
    cache.get(&0).await;
    tokio::time::sleep(Duration::from_millis(5)).await;

    // Insert key 3: capacity exceeded, LRU eviction removes the oldest (key 1)
    cache
        .get_or_insert_simple(&3, || async { Ok(3u64) })
        .await
        .unwrap();

    // Key 0 was touched most recently, should still be present
    assert!(
        cache.contains_key(&0).await,
        "recently touched key 0 should survive eviction"
    );
    // Key 3 was just inserted
    assert!(
        cache.contains_key(&3).await,
        "newly inserted key 3 should be present"
    );
    // One of keys 1 or 2 should be evicted (the least recently used)
    let items = cache.iter().await;
    assert!(items.len() <= 3, "cache should not exceed capacity");
}

#[test]
async fn lru_eviction_evicts_oldest_key() {
    // More targeted: with capacity 2, insert A then B, then C should evict A
    let cache = bounded_cache(2, "lru_eviction_oldest");

    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    cache
        .get_or_insert_simple(&2, || async { Ok(20u64) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Insert key 3 → evicts key 1 (oldest)
    cache
        .get_or_insert_simple(&3, || async { Ok(30u64) })
        .await
        .unwrap();

    assert!(
        !cache.contains_key(&1).await,
        "oldest key 1 should have been evicted"
    );
    assert!(cache.contains_key(&2).await);
    assert!(cache.contains_key(&3).await);
}

#[test]
async fn concurrent_lru_evictions_subtract_only_actual_removals() {
    let capacity = 4usize;
    let cache = bounded_cache(capacity, "concurrent_lru_evictions");

    for i in 0..6u64 {
        cache
            .state
            .items
            .upsert_async(
                i,
                Item::Cached {
                    value: i,
                    last_access: Instant::now(),
                },
            )
            .await;
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    cache.state.count.store(6, Ordering::Relaxed);

    let arrived = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let arrived_for_hook = arrived.clone();
    cache.set_evict_before_retain_interleave(Arc::new(move || {
        let arrived = arrived_for_hook.clone();
        Box::pin(async move {
            arrived.fetch_add(1, Ordering::SeqCst);
            for _ in 0..1000 {
                if arrived.load(Ordering::SeqCst) >= 2 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            panic!("concurrent evictions did not both reach the pre-retain seam");
        })
    }));

    let evictions = vec![
        {
            let cache = cache.clone();
            tokio::spawn(async move { cache.evict().await })
        },
        {
            let cache = cache.clone();
            tokio::spawn(async move { cache.evict().await })
        },
    ];

    tokio::time::timeout(Duration::from_secs(5), join_all(evictions))
        .await
        .expect("concurrent evictions timed out");
    cache.clear_evict_before_retain_interleave();

    assert_eq!(cache.iter().await.len(), capacity);

    cache
        .get_or_insert_simple(&100u64, || async move { Ok(100) })
        .await
        .unwrap();

    let size = cache.iter().await.len();
    assert!(
        size <= capacity,
        "cache with capacity {capacity} grew to {size} cached entries after concurrent evictions; \
         count drifted below the real cached population"
    );
}

#[test]
async fn capacity_holds_when_insert_races_eviction() {
    // Deterministically reproduces the production count race. Capacity
    // eviction snapshots the surviving entry set and then *blindly stores*
    // that as the new size. If an insert lands between the snapshot and the
    // store, its increment is clobbered, so the cache's notion of its own
    // size drifts below the real number of cached entries. Because eviction
    // is only triggered when an insert observes the size exactly equal to
    // capacity, once the size has drifted below capacity the trigger is
    // never hit again and the cache grows without bound.
    //
    // A test seam pauses eviction at exactly that window so the race is
    // forced every run rather than relying on timing.
    let capacity = 4usize;
    let cache = bounded_cache(capacity, "capacity_race");

    // Fill exactly to capacity.
    for i in 0..capacity as u64 {
        cache
            .get_or_insert_simple(&i, || async move { Ok(i) })
            .await
            .unwrap();
    }
    assert_eq!(cache.iter().await.len(), capacity);

    // Arrange for a concurrent insert to fire while eviction is paused at the
    // seam (after computing survivors, before committing the size). The hook
    // runs once.
    let cache_for_hook = cache.clone();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_clone = fired.clone();
    cache.set_evict_interleave(Arc::new(move || {
        let cache = cache_for_hook.clone();
        let fired = fired_clone.clone();
        Box::pin(async move {
            if !fired.swap(true, Ordering::SeqCst) {
                // A unique insert completing inside the eviction window: its
                // count increment will be clobbered by the eviction's store.
                cache
                    .get_or_insert_simple(&1000u64, || async move { Ok(1000) })
                    .await
                    .unwrap();
            }
        })
    }));

    // This insert crosses capacity and triggers the (now racing) eviction.
    cache
        .get_or_insert_simple(&100u64, || async move { Ok(100) })
        .await
        .unwrap();
    cache.clear_evict_interleave();

    // After the clobber, insert more unique keys. With the bug, the size has
    // drifted below capacity so the eviction trigger is never hit again and
    // the real cached population grows unbounded past capacity.
    for k in 0..50u64 {
        cache
            .get_or_insert_simple(&(2000 + k), || async move { Ok(2000 + k) })
            .await
            .unwrap();
    }

    let size = cache.iter().await.len();
    assert!(
        size <= capacity,
        "cache with capacity {capacity} grew to {size} cached entries after an insert raced \
         eviction; the capacity bound is not being enforced"
    );
}

#[test]
async fn spawned_capacity_holds_when_insert_races_eviction() {
    let capacity = 4usize;
    let cache = bounded_cache(capacity, "spawned_capacity_race");

    for i in 0..capacity as u64 {
        cache
            .get_or_insert_simple_spawned(&i, move || async move { Ok(i) })
            .await
            .unwrap();
    }
    assert_eq!(cache.iter().await.len(), capacity);

    let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let completed_for_hook = completed.clone();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_for_hook = fired.clone();
    cache.set_evict_interleave(Arc::new(move || {
        let completed = completed_for_hook.clone();
        let fired = fired_for_hook.clone();
        Box::pin(async move {
            if !fired.swap(true, Ordering::SeqCst) {
                for _ in 0..1000 {
                    if completed.load(Ordering::SeqCst) >= 2 {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                panic!("spawned inserts did not complete while eviction was interleaved");
            }
        })
    }));

    // A 3-party barrier (two producers + the test) guarantees both spawned
    // producer futures are running before any of them proceeds, without
    // sleeps or `Notify` (whose `notify_waiters` is lost if a producer has
    // not started waiting yet).
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let tasks: Vec<_> = [100u64, 101u64]
        .into_iter()
        .map(|key| {
            let cache = cache.clone();
            let barrier = barrier.clone();
            let completed = completed.clone();
            tokio::spawn(async move {
                let result = cache
                    .get_or_insert_simple_spawned(&key, move || async move {
                        barrier.wait().await;
                        Ok(key)
                    })
                    .await;
                completed.fetch_add(1, Ordering::SeqCst);
                result
            })
        })
        .collect();

    barrier.wait().await;

    let results = tokio::time::timeout(Duration::from_secs(5), join_all(tasks))
        .await
        .expect("spawned inserts timed out");
    for result in results {
        result.unwrap().unwrap();
    }

    cache.clear_evict_interleave();

    let size = cache.iter().await.len();
    assert!(
        size <= capacity,
        "spawned cache with capacity {capacity} grew to {size} cached entries after inserts raced \
         eviction; the capacity bound is not being enforced"
    );
}

#[test]
async fn background_eviction_older_than_ttl() {
    let cache: Cache<u64, (), u64, String> = Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::OlderThan {
            ttl: Duration::from_millis(100),
            period: Duration::from_millis(50),
        },
        "bg_eviction_ttl",
    );

    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();
    assert!(cache.contains_key(&1).await);

    // Wait for TTL + a couple eviction periods to ensure background task runs
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        !cache.contains_key(&1).await,
        "entry should have been evicted by background task"
    );

    // Explicitly drop to abort background task before test ends
    drop(cache);
    tokio::task::yield_now().await;
}

#[test]
async fn background_eviction_lru_mode() {
    let cache: Cache<u64, (), u64, String> = Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::LeastRecentlyUsed {
            count: 1,
            period: Duration::from_millis(50),
        },
        "bg_eviction_lru",
    );

    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    cache
        .get_or_insert_simple(&2, || async { Ok(20u64) })
        .await
        .unwrap();

    assert!(cache.contains_key(&1).await);
    assert!(cache.contains_key(&2).await);

    // Wait for background eviction to run — it evicts 1 LRU entry per period
    tokio::time::sleep(Duration::from_millis(150)).await;

    // At least one entry should have been evicted
    let items = cache.iter().await;
    assert!(
        items.len() < 2,
        "background LRU eviction should have removed at least one entry"
    );

    drop(cache);
    tokio::task::yield_now().await;
}

#[test]
async fn background_eviction_preserves_recently_accessed() {
    let cache: Cache<u64, (), u64, String> = Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::OlderThan {
            ttl: Duration::from_millis(100),
            period: Duration::from_millis(50),
        },
        "bg_eviction_preserve",
    );

    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();

    // Keep accessing key 1 to prevent it from being evicted
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(cache.get(&1).await, Some(10));
    }

    // After 200ms of periodic access, key should still be present
    assert!(
        cache.contains_key(&1).await,
        "recently accessed entry should not be evicted"
    );

    drop(cache);
    tokio::task::yield_now().await;
}

// ---- get_or_insert_pending ----

#[test]
async fn get_or_insert_pending_returns_pending_for_new_key() {
    let cache: Cache<u64, String, u64, String> = Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::None,
        "pending_new",
    );

    let result = cache
        .get_or_insert_pending(
            &1,
            || "loading".to_string(),
            |_pv| Box::pin(async { Ok(42u64) }),
        )
        .await
        .unwrap();

    match result {
        PendingOrFinal::Pending(pv) => assert_eq!(pv, "loading"),
        PendingOrFinal::Final(_) => panic!("expected Pending"),
    }

    // Wait for background task to complete and cache the value
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(cache.get(&1).await, Some(42));
}

#[test]
async fn get_or_insert_pending_returns_final_for_cached_key() {
    let cache: Cache<u64, String, u64, String> = Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::None,
        "pending_cached",
    );

    cache
        .get_or_insert(&1, || "loading".to_string(), async |_| Ok(42u64))
        .await
        .unwrap();

    let result = cache
        .get_or_insert_pending(
            &1,
            || "loading".to_string(),
            |_pv| Box::pin(async { Ok(99u64) }),
        )
        .await
        .unwrap();

    match result {
        PendingOrFinal::Final(v) => assert_eq!(v, 42),
        PendingOrFinal::Pending(_) => panic!("expected Final"),
    }
}

#[test]
async fn get_or_insert_pending_error_does_not_leave_stale_pending() {
    let cache: Cache<u64, String, u64, String> = Cache::new(
        None,
        FullCacheEvictionMode::None,
        BackgroundEvictionMode::None,
        "pending_error",
    );

    let result = cache
        .get_or_insert_pending(
            &1,
            || "loading".to_string(),
            |_pv| Box::pin(async { Err::<u64, _>("fail".to_string()) }),
        )
        .await;
    assert!(result.is_ok()); // get_or_insert_pending itself succeeds

    // Wait for background task to run and fail
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Key should NOT be cached (error removes it), retry should work
    let result = cache
        .get_or_insert(&1, || "loading2".to_string(), async |_| Ok(42u64))
        .await;
    assert_eq!(result, Ok(42));
}

// ---- create_weak_remover ----

#[test]
async fn weak_conditional_remover_preserves_replacement() {
    let cache = test_cache("weak_conditional_remover");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();
    let old_remover = cache.create_weak_conditional_remover(1, |value| *value == 42);
    cache.remove(&1).await;
    cache
        .get_or_insert_simple(&1, || async { Ok(73u64) })
        .await
        .unwrap();
    old_remover();
    assert_eq!(cache.get(&1).await, Some(73));
    cache.create_weak_conditional_remover(1, |value| *value == 73)();
    assert!(!cache.contains_key(&1).await);
}

#[test]
async fn weak_remover_removes_entry() {
    let cache = test_cache("weak_remover");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();

    let remover = cache.create_weak_remover(1);
    remover();

    assert!(!cache.contains_key(&1).await);
}

#[test]
async fn weak_remover_after_cache_drop_is_noop() {
    let cache = test_cache("weak_remover_drop");
    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();

    let remover = cache.create_weak_remover(1);
    drop(cache);

    // Should not panic
    remover();
}

// ---- Multiple keys / isolation ----

#[test]
async fn multiple_keys_are_independent() {
    let cache = test_cache("multi_key");

    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();
    cache
        .get_or_insert_simple(&2, || async { Ok(20u64) })
        .await
        .unwrap();

    assert_eq!(cache.get(&1).await, Some(10));
    assert_eq!(cache.get(&2).await, Some(20));

    cache.remove(&1).await;
    assert_eq!(cache.get(&1).await, None);
    assert_eq!(cache.get(&2).await, Some(20));
}

// ---- Stress tests ----

#[test]
async fn stress_many_concurrent_keys() {
    let cache = test_cache("stress_keys");

    let futs: Vec<_> = (0u64..100)
        .map(|i| {
            let cache = cache.clone();
            async move {
                cache
                    .get_or_insert_simple(&i, || async move {
                        tokio::task::yield_now().await;
                        Ok(i * 100)
                    })
                    .await
            }
        })
        .collect();

    let results = tokio::time::timeout(Duration::from_secs(10), join_all(futs))
        .await
        .expect("stress test timed out");

    for (i, r) in results.into_iter().enumerate() {
        assert_eq!(r.unwrap(), (i as u64) * 100);
    }
}

#[test]
async fn stress_concurrent_mixed_operations() {
    let cache = test_cache("stress_mixed");

    // Pre-populate some keys
    for i in 0u64..10 {
        cache
            .get_or_insert_simple(&i, || async move { Ok(i) })
            .await
            .unwrap();
    }

    // Run readers, inserters, and removers concurrently using join_all
    let mut futs: Vec<Pin<Box<dyn Future<Output = ()>>>> = Vec::new();

    // Concurrent readers
    for i in 0u64..10 {
        let cache = cache.clone();
        futs.push(Box::pin(async move {
            for _ in 0..50 {
                let _ = cache.get(&i).await;
                let _ = cache.try_get(&i).await;
                tokio::task::yield_now().await;
            }
        }));
    }

    // Concurrent inserters for new keys
    for i in 10u64..20 {
        let cache = cache.clone();
        futs.push(Box::pin(async move {
            cache
                .get_or_insert_simple(&i, || async move { Ok(i * 10) })
                .await
                .unwrap();
        }));
    }

    // Concurrent removers
    for i in 0u64..5 {
        let cache = cache.clone();
        futs.push(Box::pin(async move {
            tokio::task::yield_now().await;
            cache.remove(&i).await;
        }));
    }

    tokio::time::timeout(Duration::from_secs(10), join_all(futs))
        .await
        .expect("stress mixed ops timed out");

    // Newly inserted keys (10..20) should still be present (not removed)
    for i in 10u64..20 {
        assert_eq!(
            cache.get(&i).await,
            Some(i * 10),
            "key {i} should still be cached"
        );
    }

    // Removed keys (0..5) should be gone
    for i in 0u64..5 {
        assert_eq!(
            cache.get(&i).await,
            None,
            "key {i} should have been removed"
        );
    }
}

// ---- Pending entry semantics: keys() vs iter() ----

#[test]
async fn keys_includes_pending_but_iter_excludes_pending() {
    let cache = test_cache("pending_visibility");
    let f2_entered = Arc::new(tokio::sync::Notify::new());
    let f2_proceed = Arc::new(tokio::sync::Notify::new());

    // Insert a cached value for key 1
    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();

    // Start a pending insert for key 2
    let cache_clone = cache.clone();
    let entered = f2_entered.clone();
    let proceed = f2_proceed.clone();
    let _producer = tokio::spawn(async move {
        cache_clone
            .get_or_insert_simple(&2, || async move {
                entered.notify_one();
                proceed.notified().await;
                Ok(20u64)
            })
            .await
    });

    f2_entered.notified().await;

    // keys() should include both cached and pending keys
    let mut keys = cache.keys().await;
    keys.sort();
    assert_eq!(keys, vec![1, 2], "keys() should include pending key 2");

    // iter() should only include cached values, not pending ones
    let pairs = cache.iter().await;
    assert_eq!(pairs, vec![(1, 10)], "iter() should exclude pending key 2");

    f2_proceed.notify_one();
}

// ---- Remove while pending ----

#[test]
async fn remove_cached_entry_while_other_key_is_pending() {
    let cache = test_cache("remove_cached_while_pending");
    let f2_entered = Arc::new(tokio::sync::Notify::new());
    let f2_proceed = Arc::new(tokio::sync::Notify::new());

    // Cache key 1
    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();

    // Start pending insert for key 2
    let cache_clone = cache.clone();
    let entered = f2_entered.clone();
    let proceed = f2_proceed.clone();
    let producer = tokio::spawn(async move {
        cache_clone
            .get_or_insert_simple(&2, || async move {
                entered.notify_one();
                proceed.notified().await;
                Ok(20u64)
            })
            .await
    });

    f2_entered.notified().await;

    // Remove cached key 1 while key 2 is pending — should not affect key 2
    cache.remove(&1).await;
    assert!(!cache.contains_key(&1).await);

    // Unblock key 2
    f2_proceed.notify_one();

    let result = tokio::time::timeout(Duration::from_secs(5), producer)
        .await
        .expect("producer timed out")
        .unwrap();
    assert_eq!(result, Ok(20));
    assert_eq!(cache.get(&2).await, Some(20));
}

// ---- Overwrite after remove ----

#[test]
async fn insert_after_remove_gets_new_value() {
    let cache = test_cache("insert_after_remove");

    cache
        .get_or_insert_simple(&1, || async { Ok(10u64) })
        .await
        .unwrap();
    assert_eq!(cache.get(&1).await, Some(10));

    cache.remove(&1).await;
    assert_eq!(cache.get(&1).await, None);

    cache
        .get_or_insert_simple(&1, || async { Ok(99u64) })
        .await
        .unwrap();
    assert_eq!(cache.get(&1).await, Some(99));
}

// ---- Clone semantics ----

#[test]
async fn cloned_cache_shares_state() {
    let cache = test_cache("clone_shares");
    let clone = cache.clone();

    cache
        .get_or_insert_simple(&1, || async { Ok(42u64) })
        .await
        .unwrap();

    assert_eq!(clone.get(&1).await, Some(42));

    clone.remove(&1).await;
    assert_eq!(cache.get(&1).await, None);
}

// ---- Concurrent get_or_insert for the same key with slow f2 ----

#[test]
async fn waiters_all_get_result_from_slow_producer() {
    let cache = test_cache("slow_producer");
    let call_count = Arc::new(AtomicU64::new(0));

    let futs: Vec<_> = (0..5)
        .map(|_| {
            let cache = cache.clone();
            let call_count = call_count.clone();
            async move {
                cache
                    .get_or_insert_simple(&1, || async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Ok(42u64)
                    })
                    .await
            }
        })
        .collect();

    let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
        .await
        .expect("slow producer test timed out");

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    for r in results {
        assert_eq!(r, Ok(42));
    }
}

// ---- get_or_insert_simple_spawned cancellation safety ----

#[test]
async fn spawned_owner_completing_before_subscribe_does_not_hang() {
    let cache = test_cache("spawned_pre_subscribe");

    // Delay the caller's subscription to the result watch until the
    // spawned owner has already cached the value and sent the result,
    // deterministically forcing the interleaving where the result is
    // sent on a channel that has no receivers yet.
    let cache_for_hook = cache.clone();
    cache.set_spawned_pre_subscribe_interleave(Arc::new(move || {
        let cache = cache_for_hook.clone();
        Box::pin(async move {
            for _ in 0..5000 {
                if cache.try_get(&42).await.is_some() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            panic!("spawned owner did not complete");
        })
    }));

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        cache.get_or_insert_simple_spawned(&42, || async { Ok(42u64) }),
    )
    .await
    .expect("caller hung because the owner's result was sent before the caller subscribed");
    assert_eq!(result, Ok(42));

    cache.clear_spawned_pre_subscribe_interleave();
}

#[test]
async fn spawned_owner_survives_caller_cancellation() {
    let cache = test_cache("spawned_cancellation");
    let call_count = Arc::new(AtomicU64::new(0));

    // Owner: spawn a get_or_insert_simple_spawned that sleeps for 200ms
    // before resolving, then immediately abort it so the original caller
    // never sees the result.
    let owner = {
        let cache = cache.clone();
        let call_count = call_count.clone();
        tokio::spawn(async move {
            cache
                .get_or_insert_simple_spawned(&7, move || async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Ok(99u64)
                })
                .await
        })
    };

    // Give the owner task a moment to register the pending entry.
    tokio::time::sleep(Duration::from_millis(20)).await;
    owner.abort();
    let _ = owner.await;

    // Even after the owner is aborted, the spawned producer must have
    // resolved the pending entry. A second caller must observe the
    // cached value within a sane timeout.
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        cache.get_or_insert_simple_spawned(&7, || async {
            // Not expected to be invoked because either:
            //  - the pending entry from the first call resolved, in
            //    which case this caller hits the Cached path; or
            //  - the pending entry is still in flight, in which case
            //    this caller is a waiter on the existing watch.
            Ok(0u64)
        }),
    )
    .await
    .expect("second call must not hang on an abandoned pending entry");

    assert_eq!(result, Ok(99u64));
    // The producer ran exactly once (the spawned owner's call).
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
async fn spawned_failure_removes_pending_and_allows_retry() {
    let cache: Cache<u64, (), u64, String> = test_cache("spawned_failure");

    let first = cache
        .get_or_insert_simple_spawned(&3, || async { Err::<u64, _>("transient".to_string()) })
        .await;
    assert_eq!(first, Err("transient".to_string()));

    // After failure, a follow-up call must run its own producer
    // (failures must not poison the cache).
    let second = cache
        .get_or_insert_simple_spawned(&3, || async { Ok::<u64, String>(123) })
        .await;
    assert_eq!(second, Ok(123));
}
