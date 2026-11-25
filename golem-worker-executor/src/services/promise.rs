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

use crate::metrics::promises::record_promise_created;
use crate::services::active_agents;
use crate::services::component;
use crate::services::golem_config;
use crate::services::oplog;
use crate::services::worker;
use crate::services::worker_activator::{ExactExistingDelivery, WorkerActivator};
use crate::services::{
    HasComponentService, HasConfig, HasOplogService, HasWorkerService, UsesAllDeps,
};
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use futures::future::BoxFuture;

use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentFingerprint, AgentStatus, OwnedAgentId, PromiseId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct PromiseHandle {
    inner: Arc<PromiseHandleInner>,
}

#[cfg_attr(not(test), derive(Debug))]
pub struct PromiseHandleInner {
    notify: Notify,
    state: Mutex<Option<Vec<u8>>>,
    /// Whether this handle's state reflects storage: it was completed, or a storage read found
    /// no completion yet. A handle that is merely *registered* is not hydrated, and `poll` does
    /// not hand it out until it is - see [`DefaultPromiseService::poll`].
    hydrated: AtomicBool,
    /// Test-only seam: pauses after observing an incomplete promise but before
    /// awaiting notification, so a test can deterministically interleave completion.
    #[cfg(test)]
    await_ready_interleave: std::sync::Mutex<Option<AwaitReadyInterleaveHook>>,
}

#[cfg(test)]
impl std::fmt::Debug for PromiseHandleInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromiseHandleInner").finish_non_exhaustive()
    }
}

#[cfg(test)]
type AwaitReadyInterleaveHook =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

impl PromiseHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(PromiseHandleInner {
                notify: Notify::new(),
                state: Mutex::new(None),
                hydrated: AtomicBool::new(false),
                #[cfg(test)]
                await_ready_interleave: std::sync::Mutex::new(None),
            }),
        }
    }

    pub fn downgrade(&self) -> Weak<PromiseHandleInner> {
        Arc::downgrade(&self.inner)
    }

    fn is_hydrated(&self) -> bool {
        self.inner.hydrated.load(Ordering::Acquire)
    }

    fn mark_hydrated(&self) {
        self.inner.hydrated.store(true, Ordering::Release);
    }

    pub async fn is_ready(&self) -> bool {
        self.inner.state.lock().await.is_some()
    }

    pub fn is_ready_now(&self) -> bool {
        self.inner
            .state
            .try_lock()
            .map(|state| state.is_some())
            .unwrap_or(false)
    }

    pub async fn await_ready(&self) {
        loop {
            // Register before checking state so completion cannot happen between the check and wait.
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if self.is_ready().await {
                return;
            }

            #[cfg(test)]
            {
                let hook = self.inner.await_ready_interleave.lock().unwrap().clone();
                if let Some(hook) = hook {
                    hook().await;
                }
            }
            notified.await;
        }
    }

    #[cfg(test)]
    fn set_await_ready_interleave(&self, hook: AwaitReadyInterleaveHook) {
        *self.inner.await_ready_interleave.lock().unwrap() = Some(hook);
    }

    pub async fn get(&self) -> Option<Vec<u8>> {
        let state = self.inner.state.lock().await;
        state.clone()
    }

    pub async fn complete(&self, data: Vec<u8>) -> bool {
        let mut state = self.inner.state.lock().await;
        if state.is_some() {
            return false;
        }
        // State update and wakeup must remain atomic with respect to awaiting handles.
        *state = Some(data);
        // A completed handle needs no storage read to be trusted.
        self.mark_hydrated();
        self.inner.notify.notify_waiters();
        true
    }
}

/// Service implementing creation, completion and polling of promises
#[async_trait]
pub trait PromiseService: Send + Sync {
    /// poll and complete for a given promise must be called on the same
    async fn create(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        oplog_idx: OplogIndex,
    ) -> Result<PromiseId, WorkerExecutorError>;

    /// Promise IDs carry no environment, so every operation on an existing promise takes the
    /// environment of the agent that created it.
    async fn poll(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
    ) -> Result<PromiseHandle, WorkerExecutorError>;

    /// Completes a promise with the given payload.
    /// If the promise was not completed before, it returns true. If the promise was completed before, it returns false.
    async fn complete(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError>;

    /// Persists completion and notifies registered handles without activating an agent. The
    /// caller excludes a concurrent removal of the agent's promises: `complete` through
    /// `PromiseWorkerAccess::run_excluding_removal`, scheduled completion through the agent's
    /// oplog lifecycle guard, which deletion holds too.
    async fn complete_without_wakeup(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError>;

    // Hint the promise service that a promise might be dropped, making sure it collects any dangling references
    async fn cleanup(&self);
}

pub struct LazyPromiseService(RwLock<Option<Arc<dyn PromiseService>>>);

impl Default for LazyPromiseService {
    fn default() -> Self {
        Self::new()
    }
}

impl LazyPromiseService {
    pub fn new() -> LazyPromiseService {
        Self(RwLock::new(None))
    }

    pub async fn set_implementation(&self, value: impl PromiseService + 'static) {
        let _ = self.0.write().await.insert(Arc::new(value));
    }

    /// Clones the implementation out so that the lock is never held across the delegated await:
    /// promise host calls run in wasmtime store contexts where holding an async lock across an
    /// await can deadlock the store (wasmtime#11869/#11870).
    async fn implementation(&self) -> Arc<dyn PromiseService> {
        self.0.read().await.as_ref().unwrap().clone()
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl PromiseService for LazyPromiseService {
    async fn create(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        oplog_idx: OplogIndex,
    ) -> Result<PromiseId, WorkerExecutorError> {
        self.implementation()
            .await
            .create(owned_agent_id, fingerprint, oplog_idx)
            .await
    }

    async fn poll(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
    ) -> Result<PromiseHandle, WorkerExecutorError> {
        self.implementation()
            .await
            .poll(environment_id, promise_id)
            .await
    }

    async fn complete(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        crate::metrics::promises::record_promise_data_size(data.len());
        let start = std::time::Instant::now();

        let result = self
            .implementation()
            .await
            .complete(environment_id, promise_id, data)
            .await;

        let outcome = match &result {
            Ok(true) => "fulfilled",
            Ok(false) => "already_fulfilled",
            Err(_) => "failed",
        };
        crate::metrics::promises::record_promise_completion(start.elapsed(), outcome);

        result
    }

    async fn complete_without_wakeup(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        crate::metrics::promises::record_promise_data_size(data.len());
        let start = std::time::Instant::now();
        let result = self
            .implementation()
            .await
            .complete_without_wakeup(environment_id, promise_id, data)
            .await;
        let outcome = match &result {
            Ok(true) => "fulfilled",
            Ok(false) => "already_fulfilled",
            Err(_) => "failed",
        };
        crate::metrics::promises::record_promise_completion(start.elapsed(), outcome);
        result
    }

    async fn cleanup(&self) {
        self.implementation().await.cleanup().await
    }
}

struct PromiseRegistry {
    handles: HashMap<(EnvironmentId, PromiseId), Weak<PromiseHandleInner>>,
}

impl PromiseRegistry {
    fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    fn get(&mut self, id: &(EnvironmentId, PromiseId)) -> Option<PromiseHandle> {
        if let Some(weak) = self.handles.get(id)
            && let Some(inner) = weak.upgrade()
        {
            return Some(PromiseHandle { inner });
        };
        None
    }

    fn get_or_insert(&mut self, id: &(EnvironmentId, PromiseId)) -> PromiseHandle {
        self.get(id).unwrap_or_else(|| {
            let handle = PromiseHandle::new();
            self.handles.insert(id.clone(), handle.downgrade());
            handle
        })
    }

    pub fn cleanup(&mut self) {
        self.handles.retain(|_, weak| weak.strong_count() > 0);
    }
}

/// A lighter trait than `HasAll` that only provides the required functionality
/// for `DefaultPromiseService::complete`, breaking the circular Arc reference
/// `All -> LazyPromiseService -> DefaultPromiseService -> All`.
#[async_trait]
pub trait PromiseWorkerAccess: Send + Sync {
    /// Looks up an immutable completion for this exact promise incarnation after its KV base
    /// key has been removed. An orphan KV result is not authoritative without that base key.
    async fn completed_promise(
        &self,
        _environment_id: EnvironmentId,
        _promise_id: &PromiseId,
    ) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
        Ok(None)
    }

    /// Activates the incarnation that created the given promise if it is in a state that
    /// requires activation (Interrupted, Running, Suspended, Retrying). Never creates an agent:
    /// a missing agent or another incarnation is left alone.
    async fn activate_worker_if_needed(
        &self,
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
    ) -> Result<(), WorkerExecutorError>;

    /// Runs `operation`, which checks and writes one of the agent's promises, while no removal of
    /// the agent's state runs on this executor (`WorkerService::run_excluding_removal`).
    async fn run_excluding_removal<'a>(
        &self,
        owned_agent_id: &OwnedAgentId,
        operation: BoxFuture<'a, Result<bool, WorkerExecutorError>>,
    ) -> Result<bool, WorkerExecutorError>;
}

pub struct DefaultPromiseService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    worker_access: Arc<dyn PromiseWorkerAccess>,
    registry: Mutex<PromiseRegistry>,
}

impl DefaultPromiseService {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        worker_access: Arc<dyn PromiseWorkerAccess>,
    ) -> Self {
        Self {
            key_value_storage,
            worker_access,
            registry: Mutex::new(PromiseRegistry::new()),
        }
    }

    async fn exists(
        &self,
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
    ) -> Result<bool, WorkerExecutorError> {
        self.key_value_storage
            .with("promise", "complete")
            .exists(
                promise_namespace(environment_id, promise_id),
                &promise_key(environment_id, promise_id),
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to check if promise {promise_id} exists in storage: {err}"
                ))
            })
    }

    async fn completed_data(
        &self,
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
    ) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
        let state = self
            .key_value_storage
            .with_entity("promise", "get-completed", "promise")
            .get(
                promise_namespace(environment_id, promise_id),
                &promise_result_key(environment_id, promise_id),
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get promise {promise_id} from storage: {err}"
                ))
            })?;

        Ok(state.and_then(|state| match state {
            RedisPromiseState::Complete(data) => Some(data),
            RedisPromiseState::Pending => None,
        }))
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl PromiseService for DefaultPromiseService {
    async fn create(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        oplog_idx: OplogIndex,
    ) -> Result<PromiseId, WorkerExecutorError> {
        let environment_id = owned_agent_id.environment_id;
        let promise_id = PromiseId {
            agent_id: owned_agent_id.agent_id,
            fingerprint,
            oplog_idx,
        };
        debug!("Created promise {promise_id}");

        let key = promise_key(environment_id, &promise_id);
        self.key_value_storage
            .with_entity("promise", "create", "promise")
            .set_if_not_exists(
                promise_namespace(environment_id, &promise_id),
                &key,
                &RedisPromiseState::Pending,
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to set promise {promise_id} in storage: {err}"
                ))
            })?;

        record_promise_created();
        crate::metrics::promises::inc_promise_pending_count();

        Ok(promise_id)
    }

    async fn poll(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
    ) -> Result<PromiseHandle, WorkerExecutorError> {
        let registry_key = (environment_id, promise_id.clone());
        // Completed handles retain their immutable payload. Pending handles must check storage
        // again: sealing can replace their KV base key with a recorded completion.
        if let Some(handle) = self.registry.lock().await.get(&registry_key)
            && handle.is_hydrated()
            && handle.is_ready_now()
        {
            return Ok(handle);
        }

        let exists = self.exists(environment_id, &promise_id).await?;
        let recorded_completion = if exists {
            None
        } else {
            Some(
                self.worker_access
                    .completed_promise(environment_id, &promise_id)
                    .await?
                    .ok_or_else(|| WorkerExecutorError::PromiseNotFound {
                        promise_id: promise_id.clone(),
                    })?,
            )
        };

        // The handle is registered before the completion is read, so that concurrent polls share
        // one handle and one local completion wakes them all. Until that read succeeds the handle
        // is not hydrated, and the fast path above does not return it: a poll that raced this one
        // and found the registered handle falls through to a read of its own. Whichever read
        // succeeds first hydrates the shared handle. A read that fails leaves it unhydrated, so
        // the failure is reported to that caller alone and the next poll reads storage again.
        //
        // Handing out the bare registered handle instead would let a caller retain one that
        // nothing will ever hydrate: if this read then failed, every later poll would take the
        // fast path and return that handle without a storage read, and a completion that was
        // already durable when the read failed would never reach its waiter.
        let handle = {
            let mut reg = self.registry.lock().await;
            reg.get_or_insert(&registry_key)
        };

        let completed_data = if exists {
            let data = self.completed_data(environment_id, &promise_id).await?;
            if self.exists(environment_id, &promise_id).await? {
                data
            } else {
                // Sealing can remove both KV keys after the existence read, and a stale
                // writer can replace the result with an orphan before this read finishes.
                self.worker_access
                    .completed_promise(environment_id, &promise_id)
                    .await?
            }
        } else {
            recorded_completion
        };
        if let Some(data) = completed_data {
            let _ = handle.complete(data).await;
        }
        handle.mark_hydrated();

        Ok(handle)
    }

    async fn complete(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        // Agent deletion removes the deleted incarnation's promises while it excludes this
        // operation. The existence check and the write therefore both precede that removal, which
        // then removes the row too, or both follow it and find no promise. Activation runs after
        // the exclusion ends.
        let written = self
            .worker_access
            .run_excluding_removal(
                &OwnedAgentId::new(environment_id, &promise_id.agent_id),
                self.complete_without_wakeup(environment_id, promise_id.clone(), data),
            )
            .await?;
        // Retry activation even for duplicate completions: persistence may have succeeded before
        // an earlier activation failed.
        self.worker_access
            .activate_worker_if_needed(environment_id, &promise_id)
            .await?;
        Ok(written)
    }

    async fn complete_without_wakeup(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        let key = promise_result_key(environment_id, &promise_id);

        if !self.exists(environment_id, &promise_id).await? {
            let original = self
                .worker_access
                .completed_promise(environment_id, &promise_id)
                .await?
                .ok_or_else(|| WorkerExecutorError::PromiseNotFound {
                    promise_id: promise_id.clone(),
                })?;
            let handle = self
                .registry
                .lock()
                .await
                .get(&(environment_id, promise_id));
            if let Some(handle) = handle {
                let _ = handle.complete(original).await;
            }
            return Ok(false);
        };

        let mut written: bool = self
            .key_value_storage
            .with_entity("promise", "complete", "promise")
            .set_if_not_exists(
                promise_namespace(environment_id, &promise_id),
                &key,
                &RedisPromiseState::Complete(data.clone()),
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to set promise {promise_id} in storage: {err}"
                ))
            })?;

        let completed_data = if written {
            Some(data)
        } else {
            self.completed_data(environment_id, &promise_id).await?
        };
        // Recheck after both the write and any duplicate-result read: a stale writer can
        // recreate an orphan result after sealing removed both keys.
        let completed_data = if !self.exists(environment_id, &promise_id).await? {
            written = false;
            Some(
                self.worker_access
                    .completed_promise(environment_id, &promise_id)
                    .await?
                    .ok_or_else(|| WorkerExecutorError::PromiseNotFound {
                        promise_id: promise_id.clone(),
                    })?,
            )
        } else {
            match completed_data {
                Some(data) => Some(data),
                None => {
                    self.worker_access
                        .completed_promise(environment_id, &promise_id)
                        .await?
                }
            }
        };
        let handle = {
            let mut reg = self.registry.lock().await;
            reg.get(&(environment_id, promise_id.clone()))
        };
        if let (Some(handle), Some(data)) = (handle, completed_data) {
            let _ = handle.complete(data).await;
        }

        // Decrement the pending count on the first successful completion.
        if written {
            crate::metrics::promises::dec_promise_pending_count();
        }

        Ok(written)
    }

    async fn cleanup(&self) {
        self.registry.lock().await.cleanup();
    }
}

/// Default implementation of `PromiseWorkerAccess` that holds only the individual
/// services it needs, avoiding the circular `All -> PromiseService -> All` reference.
pub struct DefaultPromiseWorkerAccess<Ctx: WorkerCtx> {
    component_service: Arc<dyn component::ComponentService>,
    worker_service: Arc<dyn worker::WorkerService>,
    active_agents: Arc<active_agents::ActiveAgents<Ctx>>,
    oplog_service: Arc<dyn oplog::OplogService>,
    config: Arc<golem_config::GolemConfig>,
    worker_activator: Arc<dyn WorkerActivator<Ctx>>,
}

impl<Ctx: WorkerCtx> DefaultPromiseWorkerAccess<Ctx> {
    pub fn new(
        component_service: Arc<dyn component::ComponentService>,
        worker_service: Arc<dyn worker::WorkerService>,
        active_agents: Arc<active_agents::ActiveAgents<Ctx>>,
        oplog_service: Arc<dyn oplog::OplogService>,
        config: Arc<golem_config::GolemConfig>,
        worker_activator: Arc<dyn WorkerActivator<Ctx>>,
    ) -> Self {
        Self {
            component_service,
            worker_service,
            active_agents,
            oplog_service,
            config,
            worker_activator,
        }
    }
}

/// Adapter to satisfy the `HasOplogService + HasConfig + HasComponentService + HasWorkerService`
/// bounds required by `calculate_last_known_status_with_checkpoint` without pulling in the
/// full `All<Ctx>`.
struct StatusDeps {
    oplog_service: Arc<dyn oplog::OplogService>,
    config: Arc<golem_config::GolemConfig>,
    component_service: Arc<dyn component::ComponentService>,
    worker_service: Arc<dyn worker::WorkerService>,
}

impl HasOplogService for StatusDeps {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService> {
        self.oplog_service.clone()
    }
}

impl HasConfig for StatusDeps {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.config.clone()
    }
}

impl HasComponentService for StatusDeps {
    fn component_service(&self) -> Arc<dyn component::ComponentService> {
        self.component_service.clone()
    }
}

impl HasWorkerService for StatusDeps {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService> {
        self.worker_service.clone()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> PromiseWorkerAccess for DefaultPromiseWorkerAccess<Ctx> {
    async fn completed_promise(
        &self,
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
    ) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
        match self.oplog_service.seal_service() {
            Some(seal_service) => {
                seal_service
                    .completed_promise(environment_id, promise_id)
                    .await
            }
            None => Ok(None),
        }
    }

    async fn activate_worker_if_needed(
        &self,
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id = OwnedAgentId::new(environment_id, &promise_id.agent_id);

        // Check if worker is active first, otherwise fall back to stored metadata. Only the
        // incarnation that created the promise is activated; a deleted or re-created agent is not.
        let metadata = if let Some(worker) = self.active_agents.try_get(&owned_agent_id).await {
            if worker.get_initial_worker_metadata().fingerprint != promise_id.fingerprint {
                return Ok(());
            }
            match crate::worker::Worker::get_latest_metadata(worker.all(), &owned_agent_id).await? {
                Some(metadata) => metadata,
                None => return Ok(()),
            }
        } else if let Some(worker::GetWorkerMetadataResult {
            mut initial_worker_metadata,
            last_known_status,
        }) = self.worker_service.get(&owned_agent_id).await?
        {
            if initial_worker_metadata.fingerprint != promise_id.fingerprint {
                return Ok(());
            }
            let status_deps = StatusDeps {
                oplog_service: self.oplog_service.clone(),
                config: self.config.clone(),
                component_service: self.component_service.clone(),
                worker_service: self.worker_service.clone(),
            };
            let agent_mode = initial_worker_metadata.agent_mode;
            let Some(last_known_status) = calculate_last_known_status_with_checkpoint(
                &status_deps,
                &owned_agent_id,
                initial_worker_metadata.fingerprint,
                agent_mode,
                last_known_status,
            )
            .await
            .map_err(WorkerExecutorError::from)?
            else {
                return Ok(());
            };
            initial_worker_metadata.last_known_status = last_known_status;
            initial_worker_metadata
        } else {
            return Ok(());
        };

        if metadata.fingerprint != promise_id.fingerprint {
            return Ok(());
        }

        let should_activate = match &metadata.last_known_status.status {
            AgentStatus::Interrupted
            | AgentStatus::Running
            | AgentStatus::Suspended
            | AgentStatus::Retrying => true,
            AgentStatus::Exited
            | AgentStatus::Failed
            | AgentStatus::Idle
            | AgentStatus::Sandboxed => false,
        };

        if should_activate {
            // Never creates: an agent deleted or re-created since the check above is left alone.
            self.worker_activator
                .enqueue_exact_existing(
                    &owned_agent_id,
                    promise_id.fingerprint,
                    ExactExistingDelivery::Resume,
                )
                .await?;
        }

        Ok(())
    }

    async fn run_excluding_removal<'a>(
        &self,
        owned_agent_id: &OwnedAgentId,
        operation: BoxFuture<'a, Result<bool, WorkerExecutorError>>,
    ) -> Result<bool, WorkerExecutorError> {
        self.worker_service
            .run_excluding_removal(&owned_agent_id.agent_id, operation)
            .await
    }
}

/// The storage namespace of the promises of one agent incarnation.
pub fn promise_namespace_of(
    owned_agent_id: &OwnedAgentId,
    fingerprint: AgentFingerprint,
) -> KeyValueStorageNamespace {
    KeyValueStorageNamespace::Promise {
        owned_agent_id: owned_agent_id.clone(),
        fingerprint,
    }
}

fn promise_namespace(
    environment_id: EnvironmentId,
    promise_id: &PromiseId,
) -> KeyValueStorageNamespace {
    promise_namespace_of(
        &OwnedAgentId::new(environment_id, &promise_id.agent_id),
        promise_id.fingerprint,
    )
}

fn promise_key(environment_id: EnvironmentId, promise_id: &PromiseId) -> String {
    promise_id.storage_key(environment_id)
}

fn promise_result_key(environment_id: EnvironmentId, promise_id: &PromiseId) -> String {
    format!("{}:completed", promise_id.storage_key(environment_id))
}

#[derive(Debug, Eq, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum RedisPromiseState {
    Pending,
    Complete(Vec<u8>),
}

#[cfg(test)]
pub struct PromiseServiceMock {
    completed: Arc<Mutex<HashSet<PromiseId>>>,
}

#[cfg(test)]
impl Default for PromiseServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl PromiseServiceMock {
    pub fn new() -> Self {
        Self {
            completed: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn all_completed(&self) -> HashSet<PromiseId> {
        self.completed.lock().await.clone()
    }
}

#[cfg(test)]
#[async_trait]
#[golem_async_trait::region_simple]
impl PromiseService for PromiseServiceMock {
    async fn create(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _oplog_idx: OplogIndex,
    ) -> Result<PromiseId, WorkerExecutorError> {
        unimplemented!()
    }

    async fn poll(
        &self,
        _environment_id: EnvironmentId,
        _promise_id: PromiseId,
    ) -> Result<PromiseHandle, WorkerExecutorError> {
        unimplemented!()
    }

    async fn complete(
        &self,
        _environment_id: EnvironmentId,
        promise_id: PromiseId,
        _data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        self.completed.lock().await.insert(promise_id);
        Ok(true)
    }

    async fn complete_without_wakeup(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        self.complete(environment_id, promise_id, data).await
    }

    async fn cleanup(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::keyvalue::KeyValueStorageError;
    use crate::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::AgentId;
    use proptest::prelude::*;
    use std::sync::Arc;
    use test_r::test;
    use tokio::sync::Barrier;
    use uuid::Uuid;

    test_r::enable!();

    struct NoopPromiseWorkerAccess;

    #[async_trait::async_trait]
    impl PromiseWorkerAccess for NoopPromiseWorkerAccess {
        async fn activate_worker_if_needed(
            &self,
            _environment_id: EnvironmentId,
            _promise_id: &PromiseId,
        ) -> Result<(), WorkerExecutorError> {
            Ok(())
        }

        async fn run_excluding_removal<'a>(
            &self,
            _owned_agent_id: &OwnedAgentId,
            operation: futures::future::BoxFuture<'a, Result<bool, WorkerExecutorError>>,
        ) -> Result<bool, WorkerExecutorError> {
            operation.await
        }
    }

    fn environment_id() -> EnvironmentId {
        EnvironmentId(Uuid::from_u128(1))
    }

    fn promise_id() -> PromiseId {
        PromiseId {
            agent_id: AgentId::from_parts(7, Uuid::new_v4().as_u128() >> 38),
            fingerprint: AgentFingerprint::new(),
            oplog_idx: OplogIndex::from_u64(1),
        }
    }

    #[derive(Default)]
    struct RecordedPromiseWorkerAccess {
        completions: Mutex<HashMap<(EnvironmentId, PromiseId), Vec<u8>>>,
        lookups: Mutex<Vec<(EnvironmentId, PromiseId)>>,
        activations: Mutex<Vec<(EnvironmentId, PromiseId)>>,
    }

    #[async_trait]
    impl PromiseWorkerAccess for RecordedPromiseWorkerAccess {
        async fn completed_promise(
            &self,
            environment_id: EnvironmentId,
            promise_id: &PromiseId,
        ) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
            let key = (environment_id, promise_id.clone());
            self.lookups.lock().await.push(key.clone());
            Ok(self.completions.lock().await.get(&key).cloned())
        }

        async fn activate_worker_if_needed(
            &self,
            environment_id: EnvironmentId,
            promise_id: &PromiseId,
        ) -> Result<(), WorkerExecutorError> {
            self.activations
                .lock()
                .await
                .push((environment_id, promise_id.clone()));
            Ok(())
        }

        async fn run_excluding_removal<'a>(
            &self,
            _owned_agent_id: &OwnedAgentId,
            operation: BoxFuture<'a, Result<bool, WorkerExecutorError>>,
        ) -> Result<bool, WorkerExecutorError> {
            operation.await
        }
    }

    #[test]
    async fn recorded_completion_poll_returns_hydrated_handle_without_recreating_cache() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let access = Arc::new(RecordedPromiseWorkerAccess::default());
        let id = promise_id();
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), id.clone()), vec![1]);
        let service = DefaultPromiseService::new(storage.clone(), access.clone());

        let handle = service.poll(environment_id(), id.clone()).await.unwrap();
        assert!(handle.is_hydrated());
        assert!(handle.is_ready_now());
        assert_eq!(handle.get().await, Some(vec![1]));
        let again = service.poll(environment_id(), id.clone()).await.unwrap();
        assert!(Arc::ptr_eq(&handle.inner, &again.inner));
        assert_eq!(access.lookups.lock().await.len(), 1);
        assert!(
            storage
                .keys("test", "test", promise_namespace(environment_id(), &id))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    async fn pending_handle_is_hydrated_from_recorded_completion_on_repoll() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let access = Arc::new(RecordedPromiseWorkerAccess::default());
        let service = DefaultPromiseService::new(storage.clone(), access.clone());
        let id = promise_id();
        service
            .create(
                &OwnedAgentId::new(environment_id(), &id.agent_id),
                id.fingerprint,
                id.oplog_idx,
            )
            .await
            .unwrap();
        let pending = service.poll(environment_id(), id.clone()).await.unwrap();
        assert!(!pending.is_ready().await);
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), id.clone()), vec![1]);
        storage
            .with("test", "test")
            .del(
                promise_namespace(environment_id(), &id),
                &promise_key(environment_id(), &id),
            )
            .await
            .unwrap();
        let completed = service.poll(environment_id(), id).await.unwrap();
        assert!(Arc::ptr_eq(&pending.inner, &completed.inner));
        assert!(completed.is_hydrated());
        assert_eq!(completed.get().await, Some(vec![1]));
    }

    #[test]
    async fn recorded_duplicate_notifies_original_payload_without_recreating_result_cache() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let faults = KeyValueStorageFaults::default();
        let access = Arc::new(RecordedPromiseWorkerAccess::default());
        let service = DefaultPromiseService::new(
            Arc::new(FaultInjectingKeyValueStorage::new(
                storage.clone(),
                faults.clone(),
            )),
            access.clone(),
        );
        let id = promise_id();
        service
            .create(
                &OwnedAgentId::new(environment_id(), &id.agent_id),
                id.fingerprint,
                id.oplog_idx,
            )
            .await
            .unwrap();
        let handle = service.poll(environment_id(), id.clone()).await.unwrap();
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), id.clone()), vec![1]);
        storage
            .with("test", "test")
            .del(
                promise_namespace(environment_id(), &id),
                &promise_key(environment_id(), &id),
            )
            .await
            .unwrap();

        let waiting = handle.await_ready();
        tokio::pin!(waiting);
        assert!(futures::FutureExt::now_or_never(waiting.as_mut()).is_none());
        let calls_before = faults.calls("complete");
        assert!(
            !service
                .complete_without_wakeup(environment_id(), id.clone(), vec![2])
                .await
                .unwrap()
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap();
        assert_eq!(handle.get().await, Some(vec![1]));
        assert!(access.activations.lock().await.is_empty());
        assert!(
            !service
                .complete(environment_id(), id.clone(), vec![3])
                .await
                .unwrap()
        );
        assert_eq!(
            *access.activations.lock().await,
            vec![(environment_id(), id.clone())]
        );
        // The label covers the base-key existence read as well as any result write.
        assert_eq!(faults.calls("complete") - calls_before, 2);
        assert_eq!(handle.get().await, Some(vec![1]));
        assert!(
            storage
                .keys("test", "test", promise_namespace(environment_id(), &id))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    async fn orphan_result_is_ignored_in_favor_of_recorded_completion() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let access = Arc::new(RecordedPromiseWorkerAccess::default());
        let id = promise_id();
        storage
            .with_entity("test", "test", "promise")
            .set_if_not_exists(
                promise_namespace(environment_id(), &id),
                &promise_result_key(environment_id(), &id),
                &RedisPromiseState::Complete(vec![99]),
            )
            .await
            .unwrap();
        let service = DefaultPromiseService::new(storage.clone(), access.clone());
        assert!(matches!(
            service.poll(environment_id(), id.clone()).await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        assert!(matches!(
            service
                .complete(environment_id(), id.clone(), vec![2])
                .await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), id.clone()), vec![1]);
        let handle = service.poll(environment_id(), id.clone()).await.unwrap();
        assert_eq!(handle.get().await, Some(vec![1]));
        assert!(
            !service
                .complete(environment_id(), id.clone(), vec![2])
                .await
                .unwrap()
        );
        assert_eq!(
            service.completed_data(environment_id(), &id).await.unwrap(),
            Some(vec![99])
        );
        assert!(!service.exists(environment_id(), &id).await.unwrap());
    }

    #[test]
    async fn recorded_completion_lookup_is_isolated_by_fingerprint_and_environment() {
        let access = Arc::new(RecordedPromiseWorkerAccess::default());
        let id = promise_id();
        let other = PromiseId {
            fingerprint: AgentFingerprint::new(),
            ..id.clone()
        };
        let other_environment = EnvironmentId(Uuid::from_u128(2));
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), id.clone()), vec![1]);
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), other.clone()), vec![2]);
        let service =
            DefaultPromiseService::new(Arc::new(InMemoryKeyValueStorage::new()), access.clone());
        for (promise, payload) in [(id.clone(), vec![1]), (other.clone(), vec![2])] {
            assert_eq!(
                service
                    .poll(environment_id(), promise.clone())
                    .await
                    .unwrap()
                    .get()
                    .await,
                Some(payload)
            );
            assert!(
                !service
                    .complete_without_wakeup(environment_id(), promise, vec![3])
                    .await
                    .unwrap()
            );
        }
        assert!(matches!(
            service.poll(other_environment, id.clone()).await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        assert!(matches!(
            service
                .complete_without_wakeup(other_environment, other.clone(), vec![3])
                .await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        assert!(
            access
                .lookups
                .lock()
                .await
                .contains(&(other_environment, id))
        );
        assert!(
            access
                .lookups
                .lock()
                .await
                .contains(&(other_environment, other))
        );
    }

    #[test]
    async fn seal_between_poll_base_and_result_reads_returns_original_completion() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let faults = KeyValueStorageFaults::default();
        let access = Arc::new(RecordedPromiseWorkerAccess::default());
        let service = DefaultPromiseService::new(
            Arc::new(FaultInjectingKeyValueStorage::new(
                storage.clone(),
                faults.clone(),
            )),
            access.clone(),
        );
        let id = promise_id();
        service
            .create(
                &OwnedAgentId::new(environment_id(), &id.agent_id),
                id.fingerprint,
                id.oplog_idx,
            )
            .await
            .unwrap();
        storage
            .with_entity("test", "test", "promise")
            .set_if_not_exists(
                promise_namespace(environment_id(), &id),
                &promise_result_key(environment_id(), &id),
                &RedisPromiseState::Complete(vec![1]),
            )
            .await
            .unwrap();

        let gate = faults.gate_next_pass("get-completed");
        let poll = service.poll(environment_id(), id.clone());
        tokio::pin!(poll);
        assert!(futures::FutureExt::now_or_never(poll.as_mut()).is_none());
        gate.entered().await;
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), id.clone()), vec![1]);
        storage
            .with("test", "test")
            .del_many(
                promise_namespace(environment_id(), &id),
                Arc::from([
                    promise_key(environment_id(), &id),
                    promise_result_key(environment_id(), &id),
                ]),
            )
            .await
            .unwrap();
        gate.release();

        let handle = poll.await.unwrap();
        assert!(handle.is_hydrated());
        assert!(handle.is_ready_now());
        assert_eq!(handle.get().await, Some(vec![1]));
        assert_eq!(access.lookups.lock().await.len(), 1);
    }

    #[test]
    async fn seal_racing_completion_preserves_original_payload_and_duplicate_result() {
        // Exercise both a stale successful insert and a duplicate whose result disappears
        // between its insert attempt and result read.
        for seal_before_write in [true, false] {
            let storage = Arc::new(InMemoryKeyValueStorage::new());
            let faults = KeyValueStorageFaults::default();
            let access = Arc::new(RecordedPromiseWorkerAccess::default());
            let service = DefaultPromiseService::new(
                Arc::new(FaultInjectingKeyValueStorage::new(
                    storage.clone(),
                    faults.clone(),
                )),
                access.clone(),
            );
            let id = promise_id();
            service
                .create(
                    &OwnedAgentId::new(environment_id(), &id.agent_id),
                    id.fingerprint,
                    id.oplog_idx,
                )
                .await
                .unwrap();
            let handle = service.poll(environment_id(), id.clone()).await.unwrap();
            storage
                .with_entity("test", "test", "promise")
                .set_if_not_exists(
                    promise_namespace(environment_id(), &id),
                    &promise_result_key(environment_id(), &id),
                    &RedisPromiseState::Complete(vec![1]),
                )
                .await
                .unwrap();

            let first_gate = if seal_before_write {
                faults.gate_next_pass("complete")
            } else {
                faults.gate_next_pass("get-completed")
            };
            let completion = service.complete(environment_id(), id.clone(), vec![99]);
            tokio::pin!(completion);
            assert!(futures::FutureExt::now_or_never(completion.as_mut()).is_none());
            first_gate.entered().await;
            let write_gate = if seal_before_write {
                // The first gate stops the existence read. The next stops the insert, after
                // that existence read has observed the base key.
                let gate = faults.gate_next_pass("complete");
                first_gate.release();
                assert!(futures::FutureExt::now_or_never(completion.as_mut()).is_none());
                gate.entered().await;
                Some(gate)
            } else {
                None
            };
            access
                .completions
                .lock()
                .await
                .insert((environment_id(), id.clone()), vec![1]);
            storage
                .with("test", "test")
                .del_many(
                    promise_namespace(environment_id(), &id),
                    Arc::from([
                        promise_key(environment_id(), &id),
                        promise_result_key(environment_id(), &id),
                    ]),
                )
                .await
                .unwrap();
            if let Some(gate) = write_gate {
                gate.release();
            } else {
                first_gate.release();
            }

            assert!(!completion.await.unwrap());
            assert!(handle.is_hydrated());
            assert_eq!(handle.get().await, Some(vec![1]));
            assert_eq!(access.lookups.lock().await.len(), 1);
            assert_eq!(
                *access.activations.lock().await,
                vec![(environment_id(), id.clone())]
            );
            assert!(!service.exists(environment_id(), &id).await.unwrap());
            assert_eq!(
                service.completed_data(environment_id(), &id).await.unwrap(),
                if seal_before_write {
                    Some(vec![99])
                } else {
                    None
                }
            );
        }
    }

    #[test]
    async fn base_key_keeps_pending_and_completed_promises_kv_first() {
        let access = Arc::new(RecordedPromiseWorkerAccess::default());
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let service = DefaultPromiseService::new(storage.clone(), access.clone());
        let id = promise_id();
        access
            .completions
            .lock()
            .await
            .insert((environment_id(), id.clone()), vec![99]);
        service
            .create(
                &OwnedAgentId::new(environment_id(), &id.agent_id),
                id.fingerprint,
                id.oplog_idx,
            )
            .await
            .unwrap();
        let handle = service.poll(environment_id(), id.clone()).await.unwrap();
        assert!(!handle.is_ready().await);
        assert!(
            service
                .complete(environment_id(), id.clone(), vec![1])
                .await
                .unwrap()
        );
        assert!(
            !service
                .complete(environment_id(), id.clone(), vec![2])
                .await
                .unwrap()
        );
        assert_eq!(handle.get().await, Some(vec![1]));
        let cold = DefaultPromiseService::new(storage, access.clone());
        assert_eq!(
            cold.poll(environment_id(), id).await.unwrap().get().await,
            Some(vec![1])
        );
        assert!(access.lookups.lock().await.is_empty());
        assert_eq!(access.activations.lock().await.len(), 2);
    }

    #[test]
    async fn create_leaves_registration_to_poll() {
        let service = DefaultPromiseService::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            Arc::new(NoopPromiseWorkerAccess),
        );
        let expected_id = promise_id();
        let id = service
            .create(
                &OwnedAgentId::new(environment_id(), &expected_id.agent_id),
                expected_id.fingerprint,
                expected_id.oplog_idx,
            )
            .await
            .unwrap();

        assert_eq!(id, expected_id);
        assert!(service.registry.lock().await.handles.is_empty());

        let payload = vec![3, 17, 42];
        assert!(
            service
                .complete(environment_id(), id.clone(), payload.clone())
                .await
                .unwrap()
        );
        assert!(service.registry.lock().await.handles.is_empty());

        let handle = service.poll(environment_id(), id.clone()).await.unwrap();
        assert_eq!(handle.get().await, Some(payload));
        let second_handle = service.poll(environment_id(), id.clone()).await.unwrap();
        assert!(Arc::ptr_eq(&handle.inner, &second_handle.inner));

        let weak = handle.downgrade();
        drop(handle);
        service.cleanup().await;
        assert!(
            service
                .registry
                .lock()
                .await
                .get(&(environment_id(), id.clone()))
                .is_some()
        );

        drop(second_handle);
        assert!(weak.upgrade().is_none());
        service.cleanup().await;
        assert!(service.registry.lock().await.handles.is_empty());
    }

    /// A re-created agent keeps its agent ID and starts its oplog again, so its promises reuse
    /// the oplog indexes of the deleted incarnation. Only the fingerprint tells them apart.
    #[test]
    async fn promises_of_two_incarnations_or_environments_are_separate() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let service =
            DefaultPromiseService::new(storage.clone(), Arc::new(NoopPromiseWorkerAccess));
        let current = promise_id();
        let deleted = PromiseId {
            fingerprint: AgentFingerprint::new(),
            ..current.clone()
        };
        assert_ne!(
            promise_namespace(environment_id(), &current),
            promise_namespace(environment_id(), &deleted)
        );

        let current = service
            .create(
                &OwnedAgentId::new(environment_id(), &current.agent_id),
                current.fingerprint,
                current.oplog_idx,
            )
            .await
            .unwrap();

        assert!(matches!(
            service.poll(environment_id(), deleted.clone()).await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        let other_environment = EnvironmentId(Uuid::from_u128(2));
        assert!(matches!(
            service.poll(other_environment, current.clone()).await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        assert!(matches!(
            service
                .complete(other_environment, current.clone(), vec![1])
                .await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        assert!(matches!(
            service
                .complete(environment_id(), deleted.clone(), vec![1])
                .await,
            Err(WorkerExecutorError::PromiseNotFound { .. })
        ));
        assert!(
            storage
                .keys(
                    "test",
                    "test",
                    promise_namespace(environment_id(), &deleted)
                )
                .await
                .unwrap()
                .is_empty()
        );

        let handle = service
            .poll(environment_id(), current.clone())
            .await
            .unwrap();
        assert!(!handle.is_ready().await);
        assert!(
            service
                .complete(environment_id(), current, vec![2])
                .await
                .unwrap()
        );
        assert_eq!(handle.get().await, Some(vec![2]));
    }

    #[test]
    async fn local_completion_updates_handle_before_returning() {
        let promise_id = promise_id();
        let mut registry = PromiseRegistry::new();
        let handle = registry.get_or_insert(&(environment_id(), promise_id.clone()));

        let completion_handle = registry
            .get(&(environment_id(), promise_id.clone()))
            .unwrap();
        let _ = completion_handle.complete(vec![1]).await;

        assert_eq!(handle.get().await, Some(vec![1]));
    }

    #[test]
    async fn promise_handle_keeps_first_completion_payload() {
        let handle = PromiseHandle::new();

        handle.complete(vec![1]).await;
        handle.complete(vec![2]).await;

        assert_eq!(handle.get().await, Some(vec![1]));
    }

    #[test]
    async fn duplicate_completion_wakes_local_handle_with_original_payload() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let service_a =
            DefaultPromiseService::new(storage.clone(), Arc::new(NoopPromiseWorkerAccess));
        let service_b = DefaultPromiseService::new(storage, Arc::new(NoopPromiseWorkerAccess));
        let id = promise_id();
        let id = service_a
            .create(
                &OwnedAgentId::new(environment_id(), &id.agent_id),
                id.fingerprint,
                id.oplog_idx,
            )
            .await
            .unwrap();
        let handle = service_b.poll(environment_id(), id.clone()).await.unwrap();

        assert!(
            service_a
                .complete(environment_id(), id.clone(), vec![1])
                .await
                .unwrap()
        );
        assert!(
            !service_b
                .complete(environment_id(), id, vec![2])
                .await
                .unwrap()
        );
        assert_eq!(handle.get().await, Some(vec![1]));
    }

    struct RefusePromiseActivation;

    #[async_trait]
    impl PromiseWorkerAccess for RefusePromiseActivation {
        async fn activate_worker_if_needed(
            &self,
            _environment_id: EnvironmentId,
            _promise_id: &PromiseId,
        ) -> Result<(), WorkerExecutorError> {
            Err(WorkerExecutorError::runtime("activation refused"))
        }

        async fn run_excluding_removal<'a>(
            &self,
            _owned_agent_id: &OwnedAgentId,
            operation: futures::future::BoxFuture<'a, Result<bool, WorkerExecutorError>>,
        ) -> Result<bool, WorkerExecutorError> {
            operation.await
        }
    }

    #[test]
    async fn no_wakeup_completion_notifies_duplicate_with_original_payload_without_activation() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let service_a =
            DefaultPromiseService::new(storage.clone(), Arc::new(RefusePromiseActivation));
        let service_b = DefaultPromiseService::new(storage, Arc::new(RefusePromiseActivation));
        let id = promise_id();
        let id = service_a
            .create(
                &OwnedAgentId::new(environment_id(), &id.agent_id),
                id.fingerprint,
                id.oplog_idx,
            )
            .await
            .unwrap();
        let handle_a = service_a.poll(environment_id(), id.clone()).await.unwrap();
        let handle_b = service_b.poll(environment_id(), id.clone()).await.unwrap();
        assert!(
            service_a
                .complete_without_wakeup(environment_id(), id.clone(), vec![1])
                .await
                .unwrap()
        );
        assert_eq!(handle_a.get().await, Some(vec![1]));
        assert!(!handle_b.is_ready().await);
        let waiting = handle_b.await_ready();
        tokio::pin!(waiting);
        assert!(futures::FutureExt::now_or_never(waiting.as_mut()).is_none());
        assert!(
            !service_b
                .complete_without_wakeup(environment_id(), id.clone(), vec![2])
                .await
                .unwrap()
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap();
        assert_eq!(handle_b.get().await, Some(vec![1]));
        // Ordinary completion still propagates activation failures, including on duplicates.
        assert!(
            service_b
                .complete(environment_id(), id, vec![3])
                .await
                .is_err()
        );
        assert_eq!(handle_b.get().await, Some(vec![1]));
    }

    /// A poll that finds a handle another poll has registered but not yet hydrated must read
    /// storage itself. Otherwise, if that other poll's read then fails, the handle it retained
    /// is one nothing will ever hydrate: every later poll takes the fast path, and a completion
    /// that was already durable never reaches the waiter.
    #[test]
    async fn concurrent_poll_survives_a_failed_initial_result_read() {
        let storage = Arc::new(InMemoryKeyValueStorage::new());
        let faults = KeyValueStorageFaults::default();
        let service = Arc::new(DefaultPromiseService::new(
            Arc::new(FaultInjectingKeyValueStorage::new(
                storage.clone(),
                faults.clone(),
            )),
            Arc::new(NoopPromiseWorkerAccess),
        ));
        // Completes through the same storage but its own registry, so the completion is durable
        // and nothing local has been woken - the shape of a completion from another executor.
        let completer = DefaultPromiseService::new(storage, Arc::new(NoopPromiseWorkerAccess));

        let id = promise_id();
        let id = service
            .create(
                &OwnedAgentId::new(environment_id(), &id.agent_id),
                id.fingerprint,
                id.oplog_idx,
            )
            .await
            .unwrap();
        assert!(
            completer
                .complete(environment_id(), id.clone(), vec![1])
                .await
                .unwrap()
        );

        // The first poll registers the handle, then pauses in its result read, which will fail.
        let gate = faults.gate_next(
            "get-completed",
            KeyValueStorageError::Transient("redis failover".to_string()),
        );
        let first = tokio::spawn({
            let service = service.clone();
            let id = id.clone();
            async move { service.poll(environment_id(), id).await }
        });
        gate.entered().await;

        // The second poll finds the registered handle. It must not stop there.
        let second = service.poll(environment_id(), id.clone()).await.unwrap();
        assert!(
            second.is_ready().await,
            "a poll that found an unhydrated handle returned it without reading storage"
        );

        gate.release();
        assert!(first.await.unwrap().is_err());

        // And the failure left nothing behind that a later poll would trust.
        let third = service.poll(environment_id(), id).await.unwrap();
        assert_eq!(third.get().await, Some(vec![1]));
    }

    #[test]
    async fn completion_does_not_miss_waiter_between_state_check_and_wait() {
        let handle = PromiseHandle::new();
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let hook: AwaitReadyInterleaveHook = {
            let entered = entered.clone();
            let resume = resume.clone();
            Arc::new(move || {
                let entered = entered.clone();
                let resume = resume.clone();
                Box::pin(async move {
                    entered.notify_one();
                    resume.notified().await;
                })
            })
        };
        handle.set_await_ready_interleave(hook);

        let waiter = tokio::spawn({
            let handle = handle.clone();
            async move {
                handle.await_ready().await;
                handle.get().await
            }
        });
        entered.notified().await;
        assert!(handle.complete(vec![1]).await);
        resume.notify_one();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter missed completion between state check and wait")
            .unwrap();
        assert_eq!(received, Some(vec![1]));
    }

    proptest! {
        #[test]
        fn concurrent_completions_keep_exactly_one_payload(
            payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..64), 1..32),
        ) {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let handle = PromiseHandle::new();
                let barrier = Arc::new(Barrier::new(payloads.len()));
                let completions = payloads.iter().cloned().map(|payload| {
                    let handle = handle.clone();
                    let barrier = barrier.clone();
                    tokio::spawn(async move {
                        barrier.wait().await;
                        handle.complete(payload).await
                    })
                });
                let results = futures::future::join_all(completions)
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                let stored = handle.get().await.unwrap();

                prop_assert_eq!(results.into_iter().filter(|completed| *completed).count(), 1);
                prop_assert!(payloads.contains(&stored));
                Ok(())
            }).unwrap();
        }

    }
}
