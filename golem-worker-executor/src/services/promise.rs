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

use super::worker_proxy::WorkerProxy;
use crate::metrics::promises::record_promise_created;
use crate::services::worker_proxy::WorkerProxyError;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use async_trait::async_trait;
use bincode::{Decode, Encode};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{PromiseId, WorkerId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
#[cfg(test)]
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::Mutex;
use tracing::debug;

/// Service implementing creation, completion and polling of promises
#[async_trait]
pub trait PromiseService: Send + Sync {
    async fn create(&self, worker_id: &WorkerId, oplog_idx: OplogIndex) -> PromiseId;

    async fn poll(&self, promise_id: PromiseId) -> Result<Option<Vec<u8>>, WorkerExecutorError>;

    async fn complete(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError>;

    async fn delete(&self, promise_id: PromiseId);
}

#[derive(Clone)]
pub struct DefaultPromiseService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    worker_proxy: Arc<dyn WorkerProxy>,
}

impl DefaultPromiseService {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy>,
    ) -> Self {
        Self {
            key_value_storage,
            worker_proxy,
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
impl PromiseService for DefaultPromiseService {
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
        promise_id
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
        if !self.exists(&promise_id).await {
            Err(WorkerExecutorError::PromiseNotFound { promise_id })
        } else {
            let response: Option<RedisPromiseState> = self
                .key_value_storage
                .with_entity("promise", "poll", "promise")
                .get(
                    KeyValueStorageNamespace::Promise,
                    &get_promise_result_redis_key(&promise_id),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to get promise {promise_id} from Redis: {err}")
                });

            match response {
                Some(RedisPromiseState::Complete(data)) => Ok(Some(data)),
                _ => Ok(None),
            }
        }
    }

    async fn complete(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
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

        // Wake up the worker that owns the promise, ensuring that it resumes its work.
        // We do this unconditionally here as the only reason complete will be called again during replay is if we managed to write
        // the result to redis, but failed before the worker could persist the result.
        {
            let resume_result = self.worker_proxy.resume(&promise_id.worker_id, false).await;
            match resume_result {
                // InvalidRequest will be returned if the worker is already running or failed, we are fine with those
                Ok(_)
                | Err(WorkerProxyError::InternalError(WorkerExecutorError::InvalidRequest {
                    ..
                })) => {}
                Err(other) => Err(other)?,
            }
        }

        Ok(written)
    }

    async fn delete(&self, promise_id: PromiseId) {
        let key1 = get_promise_redis_key(&promise_id);
        let key2 = get_promise_result_redis_key(&promise_id);
        self.key_value_storage
            .with("promise", "delete")
            .del_many(KeyValueStorageNamespace::Promise, vec![key1, key2])
            .await
            .unwrap_or_else(|err| {
                panic!("failed to delete promise {promise_id} from Redis: {err}")
            });
    }
}

fn get_promise_redis_key(promise_id: &PromiseId) -> String {
    promise_id.to_redis_key()
}

fn get_promise_result_redis_key(promise_id: &PromiseId) -> String {
    format!("{}:completed", promise_id.to_redis_key())
}

#[derive(Debug, Eq, PartialEq, Encode, Decode)]
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

    async fn poll(&self, _promise_id: PromiseId) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn complete(
        &self,
        promise_id: PromiseId,
        _data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        self.completed.lock().await.insert(promise_id);
        Ok(true)
    }

    async fn delete(&self, _promise_id: PromiseId) {
        unimplemented!()
    }
}
