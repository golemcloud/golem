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

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tracing::debug;

/// Executor-wide storage semaphore. One permit = `FILESYSTEM_STORAGE_PERMIT_SIZE_KB` KB.
///
/// Extracted as a standalone struct so it can be unit-tested independently of
/// the `WorkerCtx`-generic `ActiveWorkers`.
pub struct FilesystemStorageSemaphore {
    semaphore: Arc<Semaphore>,
    /// Held during non-blocking priority acquires to interrupt any in-progress
    /// blocking `acquire` loops, preventing starvation of high-priority callers.
    priority_lock: Arc<Mutex<()>>,
    acquire_retry_delay: Duration,
}

impl FilesystemStorageSemaphore {
    pub(crate) fn new(pool_bytes: usize, acquire_retry_delay: Duration) -> Self {
        let permits = filesystem_storage_pool_bytes_to_permits(pool_bytes);
        Self {
            semaphore: Arc::new(Semaphore::new(permits)),
            priority_lock: Arc::new(Mutex::new(())),
            acquire_retry_delay,
        }
    }

    /// Available bytes remaining in the pool (rounded down to KB boundary).
    #[cfg(test)]
    pub(crate) fn available_bytes(&self) -> u64 {
        self.semaphore.available_permits() as u64 * FILESYSTEM_STORAGE_PERMIT_SIZE_KB * 1024
    }

    /// Expose the inner semaphore for tests that need to simulate external
    /// permit changes (e.g. worker eviction releasing storage).
    #[cfg(test)]
    pub(crate) fn inner_semaphore(&self) -> &Arc<Semaphore> {
        &self.semaphore
    }

    /// Blocking acquire. Loops until `storage_bytes` are available, calling
    /// `try_free_up` each time permits are exhausted. If `try_free_up` returns
    /// `false` (nothing to evict), sleeps `acquire_retry_delay` before retrying.
    pub(crate) async fn acquire<F, Fut>(
        &self,
        storage_bytes: u64,
        try_free_up: F,
    ) -> OwnedSemaphorePermit
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let permits = bytes_to_filesystem_storage_permits(storage_bytes);
        loop {
            let available = self.semaphore.available_permits();
            let lock = self.priority_lock.lock().await;
            let result = self.semaphore.clone().try_acquire_many_owned(permits);
            drop(lock);
            match result {
                Ok(permit) => {
                    debug!(
                        "Acquired {} storage permits ({} bytes) of {}, new available: {}, permit size: {}",
                        permits,
                        storage_bytes,
                        available,
                        self.semaphore.available_permits(),
                        permit.num_permits()
                    );
                    break permit;
                }
                Err(TryAcquireError::Closed) => panic!("worker storage semaphore has been closed"),
                Err(TryAcquireError::NoPermits) => {
                    debug!(
                        "Not enough storage to allocate {} permits (available: {}), trying to free some up",
                        permits,
                        self.semaphore.available_permits()
                    );
                    if try_free_up().await {
                        debug!("Freed up some storage, retrying");
                        continue;
                    } else {
                        debug!("Could not free up storage, retrying after some time");
                        tokio::time::sleep(self.acquire_retry_delay).await;
                    }
                }
            }
        }
    }

    /// Non-blocking priority acquire. Grabs the priority lock to interrupt any
    /// in-progress blocking `acquire` loops, then attempts once.
    ///
    /// Returns `None` if `storage_bytes` are not available even after
    /// interrupting waiting acquires.
    pub(crate) async fn try_acquire(&self, storage_bytes: u64) -> Option<OwnedSemaphorePermit> {
        let permits = bytes_to_filesystem_storage_permits(storage_bytes);
        let mut lock = None;
        loop {
            match self.semaphore.clone().try_acquire_many_owned(permits) {
                Ok(permit) => {
                    debug!(
                        "Acquired {} storage permits ({} bytes), available now: {}",
                        permits,
                        storage_bytes,
                        self.semaphore.available_permits()
                    );
                    break Some(permit);
                }
                Err(TryAcquireError::Closed) => panic!("worker storage semaphore has been closed"),
                Err(TryAcquireError::NoPermits) => {
                    if lock.is_none() {
                        debug!(
                            "Not enough storage to acquire {} permits (available: {}), cancelling waiting acquires and retry",
                            permits,
                            self.semaphore.available_permits()
                        );
                        lock = Some(self.priority_lock.lock().await);
                        continue;
                    } else {
                        debug!(
                            "Not enough storage to acquire {} permits (available: {})",
                            permits,
                            self.semaphore.available_permits()
                        );
                        break None;
                    }
                }
            }
        }
    }
}

/// One storage semaphore permit represents this many kilobytes. Using KB units
/// keeps the permit count within `u32` range while supporting up to ~4 TB of
/// addressable storage space (4_294_967_295 KB ≈ 4 TB).
pub const FILESYSTEM_STORAGE_PERMIT_SIZE_KB: u64 = 1;

/// Convert a byte count to the number of storage semaphore permits needed,
/// rounding up so that partial kilobytes always consume a full permit.
pub fn bytes_to_filesystem_storage_permits(bytes: u64) -> u32 {
    let kb = bytes.div_ceil(FILESYSTEM_STORAGE_PERMIT_SIZE_KB * 1024);
    kb.min(u32::MAX as u64) as u32
}

/// Convert a storage semaphore pool size in bytes to the number of permits to
/// initialise the semaphore with.
pub fn filesystem_storage_pool_bytes_to_permits(bytes: usize) -> usize {
    bytes.div_ceil(FILESYSTEM_STORAGE_PERMIT_SIZE_KB as usize * 1024)
}
