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

#[derive(Clone)]
pub struct OplogShutdownHandle {
    fence: Arc<dyn Fn() + Send + Sync>,
    close_and_wait: Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>,
}

impl OplogShutdownHandle {
    pub fn new(
        fence: impl Fn() + Send + Sync + 'static,
        close_and_wait: impl Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            fence: Arc::new(fence),
            close_and_wait: Arc::new(close_and_wait),
        }
    }

    pub fn from_completion(completion: OplogCloseCompletion) -> Self {
        Self::new(|| {}, move || completion.clone().boxed())
    }

    pub fn fence(&self) {
        (self.fence)();
    }

    pub async fn close_and_wait(&self) -> Result<(), String> {
        (self.close_and_wait)().await
    }
}
pub type OplogCloseCompletion = Shared<BoxFuture<'static, Result<(), String>>>;

struct OpenOplogEntry {
    oplog: Weak<dyn Oplog>,
    closed: OplogCloseCompletion,
    /// The epoch the opener that constructed this handle asked it to assert.
    requested_epoch: Option<ShardEpoch>,
}

type OplogSlot = Arc<Mutex<Option<OpenOplogEntry>>>;

/// Exclusive ownership of a logical oplog's cold lifecycle. The primary service owns the slot;
/// wrapper construction uses the same guard, including when no primary handle exists yet.
pub struct OplogLifecycleGuard {
    agent_id: AgentId,
    slot: Option<OwnedMutexGuard<Option<OpenOplogEntry>>>,
    owner: OpenOplogs,
}

impl OplogLifecycleGuard {
    pub fn assert_agent(&self, agent_id: &AgentId) {
        assert_eq!(&self.agent_id, agent_id);
    }
}

impl Drop for OplogLifecycleGuard {
    fn drop(&mut self) {
        let guard = self.slot.take().unwrap();
        let slot = OwnedMutexGuard::mutex(&guard).clone();
        drop(guard);
        self.owner.release_if_unused(&self.agent_id, &slot);
    }
}

#[derive(Clone)]
pub struct OpenOplogs {
    oplogs: Cache<AgentId, (), OplogSlot, ()>,
}

impl OpenOplogs {
    pub fn new(name: &'static str) -> Self {
        Self {
            oplogs: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                name,
            ),
        }
    }

    async fn slot(&self, agent_id: &AgentId) -> OplogSlot {
        self.oplogs
            .get_or_insert_simple(agent_id, || async { Ok(Arc::new(Mutex::new(None))) })
            .await
            .unwrap()
    }

    pub async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        let slot = self.slot(agent_id).await.lock_owned().await;
        OplogLifecycleGuard {
            agent_id: agent_id.clone(),
            slot: Some(slot),
            owner: self.clone(),
        }
    }

    fn release_if_unused(&self, agent_id: &AgentId, slot: &OplogSlot) {
        // Every waiter and live handle owns a slot reference. Removing the last unused slot
        // cannot split waiters between two locks or let an old handle evict its replacement.
        self.oplogs.remove_if_cached_sync(agent_id, |current| {
            Arc::ptr_eq(current, slot)
                && Arc::strong_count(current) == 2
                && current.try_lock().is_ok_and(|entry| {
                    entry
                        .as_ref()
                        .is_none_or(|entry| entry.closed.peek().is_some())
                })
        });
    }

    pub async fn get_or_open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        agent_id: &AgentId,
        constructor: impl OplogConstructor,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        lifecycle.assert_agent(agent_id);
        let requested_epoch = constructor.shard_epoch();
        let slot = self.slot(agent_id).await;
        let is_primary = Arc::ptr_eq(
            &slot,
            OwnedMutexGuard::mutex(lifecycle.slot.as_ref().unwrap()),
        );
        // Wrapper slots are only reached under the primary lifecycle guard. Their nested
        // locks protect cached handles during construction, not independent lifecycles.
        let mut wrapper_slot = if is_primary {
            None
        } else {
            Some(slot.lock().await)
        };
        let cached = if let Some(wrapper) = &wrapper_slot {
            wrapper.as_ref()
        } else {
            lifecycle.slot.as_ref().unwrap().as_ref()
        };
        match cached.and_then(|entry| entry.oplog.upgrade()) {
            Some(oplog) if !oplog.is_retired() => {
                let opened_with = cached.and_then(|entry| entry.requested_epoch);
                if can_reuse(&*oplog, opened_with, requested_epoch) {
                    return Ok(oplog);
                }
                // Replaced without waiting for it to close: see `can_reuse`.
            }
            live => {
                if let Some(oplog) = live {
                    oplog.retire();
                }
                if let Some(cached) = cached {
                    // Completion, including an error, proves the old layer no longer owns running
                    // work. The new attempt reloads persisted state rather than inheriting the old
                    // error.
                    let _ = cached.closed.clone().await;
                }
            }
        }
        let owner = self.clone();
        let close_agent_id = agent_id.clone();
        let close_slot = slot.clone();
        let close = Box::new(move || owner.release_if_unused(&close_agent_id, &close_slot));
        // A failed open caches nothing: the slot keeps what it held, and the next open tries again.
        let oplog = constructor.create_oplog(lifecycle, close).await?;
        let closed = oplog.closed();
        let entry = Some(OpenOplogEntry {
            oplog: Arc::downgrade(&oplog),
            closed: closed.clone(),
            requested_epoch,
        });
        if let Some(wrapper) = &mut wrapper_slot {
            **wrapper = entry;
        } else {
            **lifecycle.slot.as_mut().unwrap() = entry;
        }
        // Retain the cache slot until asynchronous last-handle cleanup finishes, even when
        // nobody reopens it. A cold open that wins this race awaits the same completion above.
        let owner = self.clone();
        let agent_id = agent_id.clone();
        let cleanup_slot = slot.clone();
        tokio::spawn(async move {
            let _ = closed.await;
            owner.release_if_unused(&agent_id, &cleanup_slot);
        });
        Ok(oplog)
    }
}

/// Whether a live cached handle can be handed to an opener asking for `requested`. It cannot,
/// and is replaced, when:
/// - it is fenced: the storage refused one of its writes, so every later one is refused too;
/// - or it belongs to an older ownership generation: `requested` is newer than the epoch it was
///   opened with (`None` is older than any epoch). The replacement records the newer epoch, on
///   the primary oplog or, for an ephemeral oplog, on its archive layers.
///
/// A replaced handle may still be held by a worker that is stopping, so nobody waits for it to
/// close; it keeps any background work, such as an archive transfer, until its holder drops it.
/// An equal or older request gets the cached handle, so no two live handles assert one epoch.
fn can_reuse(
    oplog: &dyn Oplog,
    opened_with: Option<ShardEpoch>,
    requested: Option<ShardEpoch>,
) -> bool {
    let fenced = oplog.fence().is_some();
    let older_generation = requested > opened_with;
    !fenced && !older_generation
}

impl Debug for OpenOplogs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenOplogs").finish()
    }
}

#[async_trait]
pub trait OplogConstructor: Send {
    async fn create_oplog(
        self,
        lifecycle: &mut OplogLifecycleGuard,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError>;

    /// The epoch the oplog this constructor builds is asked to assert, or `None` when it asserts
    /// none. The open-oplog cache compares it with the epoch a cached
    /// handle was opened with, so it has no default: a layer that left it out would hand an
    /// older generation's handle to every newer opener.
    fn shard_epoch(&self) -> Option<ShardEpoch>;
}
