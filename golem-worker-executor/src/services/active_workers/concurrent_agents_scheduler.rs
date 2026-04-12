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

use super::concurrent_agents_semaphore::ConcurrentAgentsSemaphore;
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::account::AccountId;
use golem_common::model::AgentId;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;
use tracing::debug;

/// Fair per-account FIFO scheduler built on top of [`ConcurrentAgentsSemaphore`].
///
/// Ensures that:
/// 1. Workers within an account are scheduled in FIFO order.
/// 2. A worker that finishes and re-requests a slot goes to the back of the
///    queue.
/// 3. De-duplication prevents the same agent from being queued twice.
/// 4. Dropping the [`ConcurrentAgentPermit`] notifies the scheduler to wake the
///    next queued agent — fully synchronously, no spawned tasks.
pub struct ConcurrentAgentsScheduler {
    permits: Arc<ConcurrentAgentsSemaphore>,
    accounts: scc::HashMap<AccountId, Arc<AccountScheduler>>,
}

struct AccountScheduler {
    resource_entry: Arc<AtomicResourceEntry>,
    /// Raw tokio semaphore for synchronous try-acquire in Drop paths.
    raw_semaphore: Arc<tokio::sync::Semaphore>,
    state: std::sync::Mutex<AccountSchedulerState>,
}

struct AccountSchedulerState {
    running_count: usize,
    ready_queue: VecDeque<QueuedAgent>,
    ready_set: HashSet<AgentId>,
}

struct QueuedAgent {
    agent_id: AgentId,
    waker: tokio::sync::oneshot::Sender<OwnedSemaphorePermit>,
}

/// RAII permit returned by [`ConcurrentAgentsScheduler::acquire`].
///
/// On drop, decrements the account's running count and wakes the next queued
/// agent (if any). The drop handler is fully synchronous.
pub struct ConcurrentAgentPermit {
    raw: Option<OwnedSemaphorePermit>,
    account: Option<Arc<AccountScheduler>>,
    account_id: AccountId,
}

impl Drop for ConcurrentAgentPermit {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // Return the raw permit to the semaphore first so it is available
            // for the next queued agent's synchronous try-acquire.
            drop(raw);

            if let Some(ref account) = self.account {
                try_grant_next_sync(account, &self.account_id);
            }
        }
    }
}

impl ConcurrentAgentPermit {
    /// Consumes the permit without triggering the drop notification.
    #[allow(dead_code)]
    pub fn into_inner(mut self) -> Option<OwnedSemaphorePermit> {
        self.account = None;
        self.raw.take()
    }
}

impl Default for ConcurrentAgentsScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl ConcurrentAgentsScheduler {
    pub fn new() -> Self {
        Self {
            permits: Arc::new(ConcurrentAgentsSemaphore::new()),
            accounts: scc::HashMap::new(),
        }
    }

    /// Register an account with its shared resource entry.
    ///
    /// Delegates to the underlying [`ConcurrentAgentsSemaphore`] and stores
    /// the resource entry and raw semaphore reference so the scheduler can
    /// read the current limit and try-acquire permits synchronously in the
    /// Drop path.
    pub async fn register_account(
        &self,
        account_id: AccountId,
        resource_entry: Arc<AtomicResourceEntry>,
    ) {
        self.permits
            .register_account(account_id, resource_entry.clone())
            .await;

        // Get the raw tokio semaphore that was just created/retrieved.
        let raw_semaphore = self
            .permits
            .raw_semaphore(&account_id)
            .await
            .expect("semaphore must exist after register_account");

        self.accounts
            .entry_async(account_id)
            .await
            .or_insert_with(|| {
                Arc::new(AccountScheduler {
                    resource_entry,
                    raw_semaphore,
                    state: std::sync::Mutex::new(AccountSchedulerState {
                        running_count: 0,
                        ready_queue: VecDeque::new(),
                        ready_set: HashSet::new(),
                    }),
                })
            });
    }

    /// Acquire a concurrent-agent permit for the given agent, respecting FIFO
    /// ordering within the account.
    ///
    /// If the account is unlimited (limit >= sentinel), bypasses the queue
    /// entirely and acquires directly from the underlying semaphore.
    ///
    /// Otherwise:
    /// - If `running_count < limit` and no older waiters exist, acquires
    ///   directly (fast path).
    /// - Otherwise, enqueues the agent and awaits a permit from the scheduler.
    pub async fn acquire(
        self: &Arc<Self>,
        account_id: AccountId,
        agent_id: AgentId,
    ) -> ConcurrentAgentPermit {
        let account = self.get_or_create_account(&account_id).await;
        let limit = account.resource_entry.max_concurrent_agents_per_executor();

        // Unlimited accounts bypass the queue entirely.
        if is_unlimited(limit) {
            let raw = self
                .permits
                .acquire(account_id, || async { false })
                .await;
            return ConcurrentAgentPermit {
                raw: Some(raw),
                account: None,
                account_id,
            };
        }

        enum AcquireDecision {
            FastPath,
            Queued(tokio::sync::oneshot::Receiver<OwnedSemaphorePermit>),
        }

        let decision = {
            let mut state = account.state.lock().unwrap();

            // Fast path: capacity available and no older waiters.
            if state.running_count < limit as usize && state.ready_queue.is_empty() {
                state.running_count += 1;
                AcquireDecision::FastPath
            } else {
                // Slow path: enqueue and wait.
                if !state.ready_set.contains(&agent_id) {
                    state.ready_set.insert(agent_id.clone());
                }

                let (tx, rx) = tokio::sync::oneshot::channel();
                state.ready_queue.push_back(QueuedAgent {
                    agent_id: agent_id.clone(),
                    waker: tx,
                });

                AcquireDecision::Queued(rx)
            }
            // MutexGuard dropped here before any .await
        };

        match decision {
            AcquireDecision::FastPath => {
                let raw = self
                    .permits
                    .acquire(account_id, || async { false })
                    .await;

                debug!(
                    "ConcurrentAgentsScheduler: fast-path permit for {agent_id} in account {account_id}"
                );

                return ConcurrentAgentPermit {
                    raw: Some(raw),
                    account: Some(account),
                    account_id,
                };
            }
            AcquireDecision::Queued(rx) => {
                debug!(
                    "ConcurrentAgentsScheduler: {agent_id} queued in account {account_id}, waiting for permit"
                );

                let raw = rx.await.expect(
                    "ConcurrentAgentsScheduler: oneshot sender dropped without sending — scheduler bug",
                );

                ConcurrentAgentPermit {
                    raw: Some(raw),
                    account: Some(account),
                    account_id,
                }
            }
        }
    }

    async fn get_or_create_account(&self, account_id: &AccountId) -> Arc<AccountScheduler> {
        // Fast path: account already registered.
        if let Some(account) = self
            .accounts
            .read_async(account_id, |_, v| v.clone())
            .await
        {
            return account;
        }

        // Slow path: create with unlimited defaults for unregistered accounts.
        // This should not happen in production (register_account is called
        // from Worker::new before any acquire), but handle gracefully.
        let raw_semaphore = Arc::new(tokio::sync::Semaphore::new(0));
        let resource_entry = Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        ));
        self.accounts
            .entry_async(*account_id)
            .await
            .or_insert_with(|| {
                Arc::new(AccountScheduler {
                    resource_entry,
                    raw_semaphore,
                    state: std::sync::Mutex::new(AccountSchedulerState {
                        running_count: 0,
                        ready_queue: VecDeque::new(),
                        ready_set: HashSet::new(),
                    }),
                })
            })
            .get()
            .clone()
    }

    #[cfg(test)]
    pub(crate) async fn running_count(&self, account_id: &AccountId) -> Option<usize> {
        let account = self
            .accounts
            .read_async(account_id, |_, v| v.clone())
            .await?;
        Some(account.state.lock().unwrap().running_count)
    }

    #[cfg(test)]
    pub(crate) async fn queue_len(&self, account_id: &AccountId) -> Option<usize> {
        let account = self
            .accounts
            .read_async(account_id, |_, v| v.clone())
            .await?;
        Some(account.state.lock().unwrap().ready_queue.len())
    }
}

/// Synchronously grant permits to queued agents after a permit is dropped.
///
/// Runs in the `Drop` implementation of [`ConcurrentAgentPermit`] so it must
/// be fully synchronous. Uses `tokio::sync::Semaphore::try_acquire_owned`
/// (which is synchronous despite being on a tokio type) to acquire permits
/// for queued agents.
fn try_grant_next_sync(account: &AccountScheduler, account_id: &AccountId) {
    let limit = account.resource_entry.max_concurrent_agents_per_executor();
    if is_unlimited(limit) {
        return;
    }

    let mut state = account.state.lock().unwrap();
    state.running_count = state.running_count.saturating_sub(1);

    while !state.ready_queue.is_empty() && state.running_count < limit as usize {
        let queued = state.ready_queue.pop_front().unwrap();
        state.ready_set.remove(&queued.agent_id);

        // tokio::sync::Semaphore::try_acquire_owned is synchronous.
        match account.raw_semaphore.clone().try_acquire_owned() {
            Ok(raw) => {
                state.running_count += 1;
                if queued.waker.send(raw).is_err() {
                    // Waiter was cancelled; the permit inside the oneshot
                    // is dropped, returning it to the semaphore. Decrement
                    // and try next.
                    state.running_count -= 1;
                    debug!(
                        "ConcurrentAgentsScheduler: waiter {} cancelled in account {account_id}, trying next",
                        queued.agent_id
                    );
                } else {
                    debug!(
                        "ConcurrentAgentsScheduler: granted permit to {} in account {account_id}",
                        queued.agent_id
                    );
                }
            }
            Err(_) => {
                // Semaphore exhausted — re-enqueue at front and stop.
                state.ready_set.insert(queued.agent_id.clone());
                state.ready_queue.push_front(queued);
                break;
            }
        }
    }
}

/// Returns `true` if the given limit value is at or above the unlimited sentinel.
fn is_unlimited(limit: u64) -> bool {
    limit >= AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS
}
