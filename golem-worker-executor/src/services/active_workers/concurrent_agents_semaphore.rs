// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::account::AccountId;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tracing::debug;

pub struct ConcurrentAgentsSemaphore {
    accounts: scc::HashMap<AccountId, AccountConcurrencyEntry>,
}

struct AccountConcurrencyEntry {
    semaphore: Arc<Semaphore>,
    /// Shared reference to the account's resource limits. The semaphore reads
    /// `max_concurrent_agents_per_executor` from this on every acquire to detect
    /// plan changes and resize the pool.
    resource_entry: Arc<AtomicResourceEntry>,
    /// The limit the semaphore was last sized to. Tracks the last-known limit so
    /// that both increases and decreases can be applied as a delta.
    ///
    /// On an increase: `add_permits(delta)` grows the available pool.
    /// On a decrease: `current_limit` is lowered and the cap enforcement step
    /// trims excess available permits using `forget()`.
    current_limit: u64,
    /// Total permits ever issued to this semaphore (initial + added via
    /// `add_permits`). Used together with `semaphore.available_permits()` to
    /// derive how many permits are currently held by running agents:
    ///   in_use = total_issued - available
    /// This lets cap enforcement correctly trim excess available permits after a
    /// downgrade even when some permits are still held by running agents.
    total_issued: usize,
}

impl Default for ConcurrentAgentsSemaphore {
    fn default() -> Self {
        Self::new()
    }
}

impl ConcurrentAgentsSemaphore {
    pub fn new() -> Self {
        Self {
            accounts: scc::HashMap::new(),
        }
    }

    /// Register an account with its shared resource entry.
    ///
    /// Creates a per-account semaphore sized to the current
    /// `max_concurrent_agents_per_executor` limit. If the account is already
    /// registered this is a no-op.
    pub async fn register_account(
        &self,
        account_id: AccountId,
        resource_entry: Arc<AtomicResourceEntry>,
    ) {
        let limit = resource_entry.max_concurrent_agents_per_executor();
        let permits = if limit == u64::MAX {
            // Unlimited — create a semaphore with a large but finite number of
            // permits so we never block. We will bypass the semaphore entirely
            // in acquire/try_acquire for the unlimited case.
            0
        } else {
            limit as usize
        };

        // entry_async returns an OccupiedEntry or inserts a new one.
        self.accounts
            .entry_async(account_id)
            .await
            .or_insert_with(|| AccountConcurrencyEntry {
                semaphore: Arc::new(Semaphore::new(permits)),
                resource_entry,
                current_limit: limit,
                total_issued: permits,
            });
    }

    /// Blocking acquire of one concurrent-agent permit for `account_id`.
    ///
    /// First calls `try_free_up` to attempt eviction of an idle agent from the
    /// same account. If that succeeds a permit becomes available immediately.
    /// If nothing can be evicted, waits efficiently via `semaphore.acquire_owned()`
    /// until a running agent stops and returns its permit.
    ///
    /// If the account's plan limit changed since last time, the semaphore pool
    /// is resized before attempting the acquire (grown on upgrade, shrunk on
    /// downgrade — see `sync_semaphore_limit`).
    ///
    /// Returns immediately without touching the semaphore when the limit is
    /// `u64::MAX` (unlimited), returning a zero-permit sentinel.
    pub async fn acquire<F, Fut>(
        &self,
        account_id: AccountId,
        try_free_up: F,
    ) -> OwnedSemaphorePermit
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let semaphore = match self
            .accounts
            .read_async(&account_id, |_, e| e.semaphore.clone())
            .await
        {
            Some(s) => s,
            None => {
                // Account not registered — should not happen in production but
                // defend gracefully: waiting on a 0-permit semaphore will
                // deadlock, surfacing the bug.
                debug!("ConcurrentAgentsSemaphore: acquire called for unregistered account {account_id}");
                Arc::new(Semaphore::new(0))
            }
        };

        // Sync the semaphore pool size with the current plan limit (up or down).
        self.sync_semaphore_limit(&account_id, &semaphore).await;

        // Unlimited accounts bypass the semaphore entirely.
        if self.is_unlimited(&account_id).await {
            return semaphore
                .clone()
                .try_acquire_many_owned(0)
                .expect("acquiring 0 permits must always succeed");
        }

        // Attempt eviction of an idle agent first. If one is evicted its permit
        // is returned to the pool via Drop, so we can grab it immediately.
        // If nothing is evicted, wait efficiently on the semaphore until a
        // running agent stops.
        loop {
            match semaphore.clone().try_acquire_owned() {
                Ok(permit) => {
                    debug!(
                        "ConcurrentAgentsSemaphore: acquired permit for {account_id}, available: {}",
                        semaphore.available_permits()
                    );
                    break permit;
                }
                Err(TryAcquireError::Closed) => {
                    panic!("concurrent agents semaphore for {account_id} has been closed")
                }
                Err(TryAcquireError::NoPermits) => {
                    debug!(
                        "ConcurrentAgentsSemaphore: no permits for {account_id}, trying to free one up"
                    );
                    if try_free_up().await {
                        // An idle agent was evicted; its Drop returns the permit
                        // to the pool. Retry the try_acquire immediately.
                        debug!(
                            "ConcurrentAgentsSemaphore: freed a slot for {account_id}, retrying"
                        );
                        continue;
                    }
                    // Nothing to evict — wait until a running agent stops.
                    debug!("ConcurrentAgentsSemaphore: nothing to free for {account_id}, waiting");
                    let permit =
                        semaphore.clone().acquire_owned().await.expect(
                            "concurrent agents semaphore for {account_id} must not be closed",
                        );
                    // Re-sync after waking in case the plan changed while waiting.
                    self.sync_semaphore_limit(&account_id, &semaphore).await;
                    break permit;
                }
            }
        }
    }

    /// Returns `true` if the account's limit is `u64::MAX` (unlimited).
    async fn is_unlimited(&self, account_id: &AccountId) -> bool {
        self.accounts
            .read_async(account_id, |_, e| {
                e.resource_entry.max_concurrent_agents_per_executor() == u64::MAX
            })
            .await
            .unwrap_or(false)
    }

    /// Synchronises the semaphore pool size with the current plan limit.
    ///
    /// Called before every acquire attempt so that plan changes take effect on
    /// the next agent startup without requiring an executor restart.
    ///
    /// Two things happen on each call:
    ///
    /// 1. **Plan change** (limit differs from `current_limit`):
    ///    - Increase: `add_permits(delta)` grows the pool capacity.
    ///    - Decrease: `current_limit` is updated; excess permits are trimmed
    ///      in step 2 below.
    ///
    /// 2. **Cap enforcement**: regardless of whether the plan changed, any
    ///    available permits that exceed the current limit are consumed via
    ///    `try_acquire_many_owned` + `forget()`. This handles the case where
    ///    running agents returned their permits via `Drop` and pushed the pool
    ///    above the cap while the limit was lower.
    ///
    /// Held permits (running agents) are never touched; the lower limit is
    /// enforced only for new agent starts.
    ///
    /// If the limit is `u64::MAX` (unlimited) the semaphore is not touched.
    async fn sync_semaphore_limit(&self, account_id: &AccountId, semaphore: &Arc<Semaphore>) {
        // Step 1: apply plan increase and update current_limit + total_issued.
        let maybe_increase: Option<u64> = self
            .accounts
            .read_async(account_id, |_, e| {
                let new_limit = e.resource_entry.max_concurrent_agents_per_executor();
                if new_limit == u64::MAX {
                    return None;
                }
                if new_limit > e.current_limit {
                    Some(new_limit - e.current_limit)
                } else {
                    None
                }
            })
            .await
            .flatten();

        if let Some(delta) = maybe_increase {
            semaphore.add_permits(delta as usize);
            debug!(
                "ConcurrentAgentsSemaphore: plan upgraded for {account_id}, added {delta} permits"
            );
        }

        // Update current_limit and total_issued atomically.
        self.accounts
            .update_async(account_id, |_, e| {
                let new_limit = e.resource_entry.max_concurrent_agents_per_executor();
                if new_limit != u64::MAX {
                    if new_limit > e.current_limit {
                        e.total_issued += (new_limit - e.current_limit) as usize;
                    }
                    e.current_limit = new_limit;
                }
            })
            .await;

        // Step 2: cap enforcement — trim available permits that exceed the cap.
        //
        // After a downgrade, running agents still hold their permits (we never
        // forcibly revoke them). But available permits in the pool may now exceed
        // the headroom the new cap allows. We compute:
        //
        //   in_use            = total_issued - available_permits
        //   target_available  = max(0, cap - in_use)
        //   excess            = available - target_available
        //
        // Permits in excess are consumed via try_acquire_many_owned + forget(),
        // which is the "acquire and forget" pattern: permanently removes them
        // from the pool without returning them on drop. This enforces the new cap
        // for newly starting agents while leaving running agents unaffected.
        //
        // This also fires every call (not just on plan changes) to handle the
        // case where running agents returned permits via Drop and temporarily
        // pushed available above the headroom.
        let excess: Option<usize> = self
            .accounts
            .read_async(account_id, |_, e| {
                let cap = e.current_limit as usize;
                let available = semaphore.available_permits();
                let in_use = e.total_issued.saturating_sub(available);
                let target_available = cap.saturating_sub(in_use);
                if available > target_available {
                    Some(available - target_available)
                } else {
                    None
                }
            })
            .await
            .flatten();

        if let Some(to_remove) = excess {
            if let Ok(permits) = semaphore.clone().try_acquire_many_owned(to_remove as u32) {
                permits.forget();
                // total_issued decreases to reflect the permanently consumed permits.
                self.accounts
                    .update_async(account_id, |_, e| {
                        e.total_issued = e.total_issued.saturating_sub(to_remove);
                    })
                    .await;
                debug!(
                    "ConcurrentAgentsSemaphore: trimmed {to_remove} excess permits for {account_id}"
                );
            }
        }
    }

    /// Available permit count for an account (for tests / observability).
    #[cfg(test)]
    pub(crate) async fn available_permits(&self, account_id: &AccountId) -> Option<usize> {
        self.accounts
            .read_async(account_id, |_, e| e.semaphore.available_permits())
            .await
    }

    /// Non-blocking single attempt: returns `Some(permit)` if one is available
    /// right now, `None` otherwise. Does not call `try_free_up` or wait.
    /// Intended for tests that need to assert exhaustion without blocking.
    #[cfg(test)]
    pub(crate) async fn try_acquire_now(
        &self,
        account_id: AccountId,
    ) -> Option<OwnedSemaphorePermit> {
        let semaphore = self
            .accounts
            .read_async(&account_id, |_, e| e.semaphore.clone())
            .await?;

        self.sync_semaphore_limit(&account_id, &semaphore).await;

        if self.is_unlimited(&account_id).await {
            return Some(
                semaphore
                    .clone()
                    .try_acquire_many_owned(0)
                    .expect("acquiring 0 permits must always succeed"),
            );
        }

        semaphore.clone().try_acquire_owned().ok()
    }
}
