use super::fs_semaphore::*;
use std::time::Duration;
use test_r::test;

test_r::enable!();

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
