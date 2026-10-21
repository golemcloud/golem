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

use super::*;

impl Drop for AgentFilesystemLimitRegistration {
    fn drop(&mut self) {
        self.resource_limits
            .agent_filesystem_limit_targets
            .remove_if_sync(&self.owned_agent_id, |registered| {
                registered
                    .upgrade()
                    .is_none_or(|registered| Arc::ptr_eq(&registered, &self.target))
            });
    }
}

impl AtomicResourceEntry {
    pub fn oplog_writes_per_second(&self) -> u64 {
        self.oplog_writes_per_second.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn set_oplog_writes_per_second(&self, value: u64) {
        self.oplog_writes_per_second.store(value, Ordering::Release);
    }

    pub fn max_memory_limit(&self) -> usize {
        self.max_memory.load(Ordering::Acquire)
    }

    pub(crate) fn register_agent_memory_limit_target(
        &self,
        target: Weak<dyn AgentMemoryLimitTarget>,
    ) {
        self.agent_memory_limit_targets.lock().unwrap().push(target);
    }

    pub(crate) fn update_memory_limit(&self, limit: u64) {
        let previous = self.max_memory.swap(limit as usize, Ordering::AcqRel) as u64;
        if limit >= previous {
            return;
        }

        let targets = {
            let mut registered = self.agent_memory_limit_targets.lock().unwrap();
            let mut targets = Vec::with_capacity(registered.len());
            registered.retain(|target| {
                target.upgrade().is_some_and(|target| {
                    targets.push(target);
                    true
                })
            });
            targets
        };
        for target in targets {
            target.enforce_limit(limit);
        }
    }

    pub fn max_table_elements_limit(&self) -> usize {
        self.max_table_elements.load(Ordering::Acquire)
    }

    pub fn max_disk_space_limit(&self) -> u64 {
        self.max_disk_space.load(Ordering::Acquire)
    }

    pub(crate) fn register_agent_filesystem_limit_target(
        self: &Arc<Self>,
        owned_agent_id: OwnedAgentId,
        update: impl Fn(u64) -> AgentFilesystemLimitUpdate + Send + Sync + 'static,
    ) -> AgentFilesystemLimitRegistration {
        let target = Arc::new(AgentFilesystemLimitTarget {
            update: Arc::new(update),
        });
        self.agent_filesystem_limit_targets
            .upsert_sync(owned_agent_id.clone(), Arc::downgrade(&target));
        AgentFilesystemLimitRegistration {
            resource_limits: Arc::clone(self),
            owned_agent_id,
            target,
        }
    }

    #[doc(hidden)]
    pub async fn apply_agent_filesystem_limit(
        &self,
        allocated_bytes: u64,
    ) -> Result<(), (OwnedAgentId, WorkerExecutorError)> {
        let _update = self.filesystem_limit_update.lock().await;
        self.max_disk_space
            .store(allocated_bytes, Ordering::Release);
        self.apply_agent_filesystem_limit_to_targets(allocated_bytes)
            .await
    }

    // Registry refresh publishes desired limits without waiting for an executing invocation to
    // acknowledge its filesystem command. One consumer retains only the latest pending value.
    pub(super) fn queue_agent_filesystem_limit(
        self: &Arc<Self>,
        account_id: AccountId,
        allocated_bytes: u64,
    ) {
        self.max_disk_space
            .store(allocated_bytes, Ordering::Release);
        self.filesystem_limit_updates
            .get_or_init(|| {
                let (sender, mut updates) = tokio::sync::watch::channel(allocated_bytes);
                let entry = Arc::downgrade(self);
                tokio::spawn(async move {
                    while updates.changed().await.is_ok() {
                        let Some(entry) = entry.upgrade() else {
                            break;
                        };
                        let _update = entry.filesystem_limit_update.lock().await;
                        let allocated_bytes = *updates.borrow_and_update();
                        if let Err((owned_agent_id, error)) = entry
                            .apply_agent_filesystem_limit_to_targets(allocated_bytes)
                            .await
                        {
                            error!(
                                account_id = %account_id,
                                agent_id = %owned_agent_id,
                                limit = allocated_bytes,
                                error = %error,
                                "Failed to apply managed agent filesystem limit"
                            );
                        }
                    }
                });
                sender
            })
            .send_replace(allocated_bytes);
    }

    pub(super) async fn apply_agent_filesystem_limit_to_targets(
        &self,
        allocated_bytes: u64,
    ) -> Result<(), (OwnedAgentId, WorkerExecutorError)> {
        let mut targets = Vec::new();
        self.agent_filesystem_limit_targets
            .iter_sync(|owned_agent_id, target| {
                if let Some(target) = target.upgrade() {
                    targets.push((owned_agent_id.clone(), target));
                }
                true
            });
        let mut first_error = None;
        for (owned_agent_id, target) in targets {
            if let Err(error) = (target.update)(allocated_bytes).await
                && first_error.is_none()
            {
                first_error = Some((owned_agent_id, error));
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub fn max_concurrent_agents_per_executor(&self) -> u64 {
        self.max_concurrent_agents_per_executor
            .load(Ordering::Acquire)
    }

    /// Overwrite the concurrent agent limit. Used in tests to simulate a plan
    /// upgrade without going through the full registry/batch sync path.
    #[cfg(test)]
    pub(crate) fn set_max_concurrent_agents_per_executor(&self, limit: u64) {
        self.max_concurrent_agents_per_executor
            .store(limit, Ordering::Release);
    }
}
