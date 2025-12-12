// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::All;
use crate::metrics::promises::record_promise_created;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use golem_common::model::account::AccountId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{OwnedWorkerId, PromiseId, WorkerId, WorkerStatus};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct PromiseHandle {
    inner: Arc<PromiseHandleInner>,
}

#[derive(Debug)]
pub struct PromiseHandleInner {
    notify: Notify,
    state: Mutex<Option<Vec<u8>>>,
}

impl PromiseHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(PromiseHandleInner {
                notify: Notify::new(),
                state: Mutex::new(None),
            }),
        }
    }

    pub fn downgrade(&self) -> Weak<PromiseHandleInner> {
        Arc::downgrade(&self.inner)
    }

    pub async fn is_ready(&self) -> bool {
        self.inner.state.lock().await.is_some()
    }

    pub async fn await_ready(&self) {
        if self.is_ready().await {
            return;
        }
        self.inner.notify.notified().await;
    }

    pub async fn get(&self) -> Option<Vec<u8>> {
        let state = self.inner.state.lock().await;
        state.clone()
    }

    pub async fn complete(&self, data: Vec<u8>) {
        let mut state = self.inner.state.lock().await;
        *state = Some(data);
        self.inner.notify.notify_waiters();
    }
}

/// Service implementing creation, completion and polling of promises
#[async_trait]
pub trait PromiseService: Send + Sync {
    /// poll and complete for a given promise must be called on the same
    async fn create(&self, worker_id: &WorkerId, oplog_idx: OplogIndex) -> PromiseId;

    async fn poll(&self, promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError>;

    /// Completes a promise with the given payload.
    /// If the promise was not completed before, it returns true. If the promise was completed before, it returns false.
    async fn complete(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
        completed_by: &AccountId,
    ) -> Result<bool, WorkerExecutorError>;

    // Hint the promise service that a promise might be dropped, making sure it collects any dangling references
    async fn cleanup(&self);
}

pub struct LazyPromiseService(RwLock<Option<Box<dyn PromiseService>>>);

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
        let _ = self.0.write().await.insert(Box::new(value));
    }
}

#[async_trait]
impl PromiseService for LazyPromiseService {
    async fn create(&self, worker_id: &WorkerId, oplog_idx: OplogIndex) -> PromiseId {
        let lock = self.0.read().await;
        lock.as_ref().unwrap().create(worker_id, oplog_idx).await
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError> {
        let lock = self.0.read().await;
        lock.as_ref().unwrap().poll(promise_id).await
    }

    async fn complete(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
        completed_by: &AccountId,
    ) -> Result<bool, WorkerExecutorError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .complete(promise_id, data, completed_by)
            .await
    }

    // Hint the promise service that a promise might be dropped, making sure it collects any dangling references
    async fn cleanup(&self) {
        let lock = self.0.read().await;
        lock.as_ref().unwrap().cleanup().await
    }
}

struct PromiseRegistry {
    handles: HashMap<PromiseId, Weak<PromiseHandleInner>>,
}

impl PromiseRegistry {
    fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    fn get(&mut self, id: &PromiseId) -> Option<PromiseHandle> {
        if let Some(weak) = self.handles.get(id) {
            if let Some(inner) = weak.upgrade() {
                return Some(PromiseHandle { inner });
            }
        };
        None
    }

    fn get_or_insert(&mut self, id: &PromiseId) -> PromiseHandle {
        self.get(id).unwrap_or_else(|| {
            let handle = PromiseHandle::new();
            self.handles.insert(id.clone(), handle.downgrade());
            handle
        })
    }

    async fn complete(&mut self, id: &PromiseId, data: Vec<u8>) {
        if let Some(weak) = self.handles.get(id) {
            if let Some(inner) = weak.upgrade() {
                tokio::spawn(async move {
                    let mut state = inner.state.lock().await;
                    *state = Some(data.clone());
                    inner.notify.notify_waiters();
                });
            }
        }
    }

    pub fn cleanup(&mut self) {
        self.handles.retain(|_, weak| weak.strong_count() > 0);
    }
}

pub struct DefaultPromiseService<Ctx: WorkerCtx> {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    services: All<Ctx>,
    registry: Mutex<PromiseRegistry>,
}

impl<Ctx: WorkerCtx> DefaultPromiseService<Ctx> {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        services: All<Ctx>,
    ) -> Self {
        Self {
            key_value_storage,
            services,
            registry: Mutex::new(PromiseRegistry::new()),
        }
    }

    async fn exists(&self, promise_id: &PromiseId) -> bool {
        self.key_value_storage
            .with("promise", "complete")
            .exists(
                KeyValueStorageNamespace::Promise,
                &get_promise_redis_key(promise_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to check if promise {promise_id} exists in Redis: {err}")
            })
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> PromiseService for DefaultPromiseService<Ctx> {
    async fn create(&self, worker_id: &WorkerId, oplog_idx: OplogIndex) -> PromiseId {
        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx,
        };
        debug!("Created promise {promise_id}");

        let key = get_promise_redis_key(&promise_id);
        self.key_value_storage
            .with_entity("promise", "create", "promise")
            .set_if_not_exists(
                KeyValueStorageNamespace::Promise,
                &key,
                &RedisPromiseState::Pending,
            )
            .await
            .unwrap_or_else(|err| panic!("failed to set promise {promise_id} in Redis: {err}"));

        record_promise_created();

        // start tracking the promise locally so poll does not need to go to redis
        {
            let mut reg = self.registry.lock().await;
            reg.get_or_insert(&promise_id);
        };

        promise_id
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError> {
        // Fast path: check local registry first
        if let Some(handle) = self.registry.lock().await.get(&promise_id) {
            return Ok(handle.clone());
        }

        if !self.exists(&promise_id).await {
            return Err(WorkerExecutorError::PromiseNotFound { promise_id });
        }

        let handle = {
            let mut reg = self.registry.lock().await;
            reg.get_or_insert(&promise_id)
        };

        // Check if already completed in Redis
        if let Some(RedisPromiseState::Complete(data)) = self
            .key_value_storage
            .with_entity("promise", "poll", "promise")
            .get(
                KeyValueStorageNamespace::Promise,
                &get_promise_result_redis_key(&promise_id),
            )
            .await
            .unwrap_or_else(|err| panic!("failed to get promise {promise_id} from Redis: {err}"))
        {
            handle.complete(data).await;
        }

        Ok(handle)
    }

    async fn complete(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
        completed_by: &AccountId,
    ) -> Result<bool, WorkerExecutorError> {
        let key = get_promise_result_redis_key(&promise_id);

        if !self.exists(&promise_id).await {
            return Err(WorkerExecutorError::PromiseNotFound { promise_id });
        };

        let written: bool = self
            .key_value_storage
            .with_entity("promise", "complete", "promise")
            .set_if_not_exists(
                KeyValueStorageNamespace::Promise,
                &key,
                &RedisPromiseState::Complete(data.clone()),
            )
            .await
            .unwrap_or_else(|err| panic!("failed to set promise {promise_id} in Redis: {err}"));

        // Also wake any in-memory handle, ensuring that still running workers that wait on the pollable can continue
        {
            let mut reg = self.registry.lock().await;
            reg.complete(&promise_id, data.clone()).await;
        }

        // Wake up the worker that owns the promise, ensuring that it resumes its work.
        // We do this unconditionally here as the only reason complete will be called again during replay is if we managed to write
        // the result to redis, but failed before the worker could persist the result.
        {
            let worker_id = promise_id.worker_id.clone();

            let component_metdata = self
                .services
                .component_service
                .get_metadata(&worker_id.component_id, None)
                .await?;

            let owned_worker_id = OwnedWorkerId {
                environment_id: component_metdata.environment_id,
                worker_id,
            };

            let metadata = Worker::<Ctx>::get_latest_metadata(&self.services, &owned_worker_id)
                .await
                .ok_or(WorkerExecutorError::worker_not_found(
                    owned_worker_id.worker_id(),
                ))?;

            let should_activate = match &metadata.last_known_status.status {
                WorkerStatus::Interrupted
                | WorkerStatus::Running
                | WorkerStatus::Suspended
                | WorkerStatus::Retrying => true,
                WorkerStatus::Exited | WorkerStatus::Failed | WorkerStatus::Idle => false,
            };

            tracing::warn!(
                "complete_promise, worker-status of {}: {}, should_activate={}",
                owned_worker_id.worker_id,
                metadata.last_known_status.status,
                should_activate
            );

            if should_activate {
                Worker::get_or_create_running(
                    &self.services,
                    completed_by,
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                )
                .await?;
            }
        }

        Ok(written)
    }

    async fn cleanup(&self) {
        self.registry.lock().await.cleanup();
    }
}

fn get_promise_redis_key(promise_id: &PromiseId) -> String {
    promise_id.to_redis_key()
}

fn get_promise_result_redis_key(promise_id: &PromiseId) -> String {
    format!("{}:completed", promise_id.to_redis_key())
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
impl PromiseService for PromiseServiceMock {
    async fn create(&self, _worker_id: &WorkerId, _oplog_idx: OplogIndex) -> PromiseId {
        unimplemented!()
    }

    async fn poll(&self, _promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError> {
        unimplemented!()
    }

    async fn complete(
        &self,
        promise_id: PromiseId,
        _data: Vec<u8>,
        _completed_by: &AccountId,
    ) -> Result<bool, WorkerExecutorError> {
        self.completed.lock().await.insert(promise_id);
        Ok(true)
    }

    async fn cleanup(&self) {}
}
