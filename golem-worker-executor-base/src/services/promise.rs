// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[cfg(test)]
use std::collections::HashSet;
use std::ops::DerefMut;
use std::sync::Arc;

use async_mutex::Mutex;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use dashmap::DashMap;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{PromiseId, WorkerId};
use tokio::sync::oneshot;
use tracing::debug;

use crate::error::GolemError;
use crate::metrics::promises::record_promise_created;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};

/// Service implementing creation, completion and polling of promises
#[async_trait]
pub trait PromiseService {
    async fn create(&self, worker_id: &WorkerId, oplog_idx: OplogIndex) -> PromiseId;

    async fn wait_for(&self, promise_id: PromiseId) -> Result<Vec<u8>, GolemError>;

    async fn poll(&self, promise_id: PromiseId) -> Result<Option<Vec<u8>>, GolemError>;

    async fn complete(&self, promise_id: PromiseId, data: Vec<u8>) -> Result<bool, GolemError>;

    async fn delete(&self, promise_id: PromiseId);
}

#[derive(Clone, Debug)]
pub struct DefaultPromiseService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    promises: Arc<DashMap<PromiseId, PromiseState>>,
}

impl DefaultPromiseService {
    pub fn new(key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>) -> Self {
        Self {
            key_value_storage,
            promises: Arc::new(DashMap::new()),
        }
    }

    fn insert_if_empty(&self, key: PromiseId, value: PromiseState) {
        loop {
            match self.promises.try_entry(key.clone()) {
                Some(entry) => {
                    entry.or_insert(value);
                    break;
                }
                None => match self.promises.get(&key) {
                    Some(_) => break,
                    None => continue,
                },
            }
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

    async fn wait_for(&self, promise_id: PromiseId) -> Result<Vec<u8>, GolemError> {
        if !self.exists(&promise_id).await {
            Err(GolemError::PromiseNotFound { promise_id })
        } else {
            let response: Option<RedisPromiseState> = self
                .key_value_storage
                .with_entity("promise", "await", "promise")
                .get(
                    KeyValueStorageNamespace::Promise,
                    &get_promise_result_redis_key(&promise_id),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to get promise {promise_id} from Redis: {err}")
                });

            match response {
                Some(RedisPromiseState::Complete(data)) => Ok(data),
                _ => {
                    let (sender, receiver) = oneshot::channel::<Vec<u8>>();

                    let pending = PromiseState::Pending(
                        Arc::new(Mutex::new(Some(sender))),
                        Mutex::new(receiver),
                    );

                    self.insert_if_empty(promise_id.clone(), pending);

                    let entry = self.promises.get(&promise_id).unwrap_or_else(|| {
                        panic!(
                            "Promise {:?} not found after inserting it into the map!",
                            promise_id
                        )
                    });

                    let promise_state = entry.value();

                    match promise_state {
                        PromiseState::Pending(_, receiver) => {
                            let mut mutex_guard = receiver.lock().await;
                            let receiver = mutex_guard.deref_mut();
                            let data = receiver
                                .await
                                .map_err(|_| GolemError::PromiseDropped { promise_id })?;
                            Ok(data)
                        }
                        PromiseState::Complete(data) => Ok(data.clone()),
                    }
                }
            }
        }
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<Option<Vec<u8>>, GolemError> {
        if !self.exists(&promise_id).await {
            Err(GolemError::PromiseNotFound { promise_id })
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

    async fn complete(&self, promise_id: PromiseId, data: Vec<u8>) -> Result<bool, GolemError> {
        let key = get_promise_result_redis_key(&promise_id);

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

        if !self.exists(&promise_id).await {
            Err(GolemError::PromiseNotFound { promise_id })
        } else if written {
            let complete = PromiseState::Complete(data.clone());
            self.insert_if_empty(promise_id.clone(), complete);
            let entry = self.promises.get(&promise_id).unwrap_or_else(|| {
                panic!(
                    "Promise {:?} not found after inserting it into the map!",
                    promise_id.clone()
                )
            });
            let promise_state = entry.value();
            match promise_state {
                PromiseState::Pending(sender, _) => {
                    let mut mutex_guard = sender.lock().await;
                    let owned_sender =
                        mutex_guard
                            .take()
                            .ok_or(GolemError::PromiseAlreadyCompleted {
                                promise_id: promise_id.clone(),
                            })?;
                    owned_sender
                        .send(data)
                        .map_err(|_| GolemError::PromiseDropped { promise_id })?;
                    Ok(true)
                }
                _ => Ok(true),
            }
        } else {
            Ok(false)
        }
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

#[derive(Debug)]
enum PromiseState {
    Pending(
        Arc<Mutex<Option<oneshot::Sender<Vec<u8>>>>>,
        Mutex<oneshot::Receiver<Vec<u8>>>,
    ),
    Complete(Vec<u8>),
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

    async fn wait_for(&self, _promise_id: PromiseId) -> Result<Vec<u8>, GolemError> {
        unimplemented!()
    }

    async fn poll(&self, _promise_id: PromiseId) -> Result<Option<Vec<u8>>, GolemError> {
        unimplemented!()
    }

    async fn complete(&self, promise_id: PromiseId, _data: Vec<u8>) -> Result<bool, GolemError> {
        self.completed.lock().await.insert(promise_id);
        Ok(true)
    }

    async fn delete(&self, _promise_id: PromiseId) {
        unimplemented!()
    }
}
