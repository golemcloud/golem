// Copyright 2024 Golem Cloud
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

#[cfg(any(feature = "mocks", test))]
use std::collections::HashSet;
use std::ops::DerefMut;
use std::sync::Arc;

use async_mutex::Mutex;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use fred::prelude::*;
use golem_common::metrics::redis::record_redis_serialized_size;
use golem_common::model::{PromiseId, WorkerId};
use golem_common::redis::RedisPool;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::GolemError;
use crate::metrics::promises::record_promise_created;
use crate::services::golem_config::PromisesConfig;

/// Service implementing creation, completion and polling of promises
#[async_trait]
pub trait PromiseService {
    async fn create(&self, worker_id: &WorkerId, oplog_idx: u64) -> PromiseId;

    async fn wait_for(&self, promise_id: PromiseId) -> Result<Vec<u8>, GolemError>;

    async fn poll(&self, promise_id: PromiseId) -> Result<Option<Vec<u8>>, GolemError>;

    async fn complete(&self, promise_id: PromiseId, data: Vec<u8>) -> Result<bool, GolemError>;

    async fn delete(&self, promise_id: PromiseId);
}

pub fn configured(
    config: &PromisesConfig,
    redis_pool: RedisPool,
) -> Arc<dyn PromiseService + Send + Sync> {
    match config {
        PromisesConfig::InMemory => Arc::new(PromiseServiceInMemory::new()),
        PromisesConfig::Redis => Arc::new(PromiseServiceRedis::new(redis_pool.clone())),
    }
}

#[derive(Clone, Debug)]
pub struct PromiseServiceRedis {
    redis: RedisPool,
    promises: Arc<DashMap<PromiseId, PromiseState>>,
}

impl PromiseServiceRedis {
    pub fn new(redis: RedisPool) -> Self {
        Self {
            redis,
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
}

#[async_trait]
impl PromiseService for PromiseServiceRedis {
    async fn create(&self, worker_id: &WorkerId, oplog_idx: u64) -> PromiseId {
        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx,
        };

        let key = get_promise_redis_key(&promise_id);
        let value = self
            .redis
            .serialize(&RedisPromiseState::Pending)
            .expect("failed to serialize RedisPromiseState::Pending");

        record_redis_serialized_size("promise", "promise", value.len());

        let _: () = self
            .redis
            .with("promise", "create")
            .set(key.clone(), value, None, Some(SetOptions::NX), false)
            .await
            .unwrap_or_else(|err| panic!("failed to set promise {promise_id} in Redis: {err}"));

        record_promise_created();
        promise_id
    }

    async fn wait_for(&self, promise_id: PromiseId) -> Result<Vec<u8>, GolemError> {
        let key = get_promise_redis_key(&promise_id);

        let response: Option<Bytes> = self
            .redis
            .with("promise", "await")
            .get(key)
            .await
            .unwrap_or_else(|err| panic!("failed to get promise {promise_id} from Redis: {err}"));

        let response: Option<RedisPromiseState> = response.map(|bs| {
            self.redis
                .deserialize(&bs)
                .expect("failed to deserialize RedisPromiseState")
        });

        match response {
            Some(RedisPromiseState::Complete(data)) => Ok(data),
            Some(RedisPromiseState::Pending) => {
                let (sender, receiver) = oneshot::channel::<Vec<u8>>();

                let pending =
                    PromiseState::Pending(Arc::new(Mutex::new(Some(sender))), Mutex::new(receiver));

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
            None => Err(GolemError::PromiseNotFound { promise_id }),
        }
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<Option<Vec<u8>>, GolemError> {
        let key = get_promise_redis_key(&promise_id);

        let response: Option<Bytes> = self
            .redis
            .with("promise", "poll")
            .get(key)
            .await
            .unwrap_or_else(|err| panic!("failed to get promise {promise_id} from Redis: {err}"));

        let response: Option<RedisPromiseState> = response.map(|bs| {
            self.redis
                .deserialize(&bs)
                .expect("failed to deserialize RedisPromiseState")
        });

        match response {
            Some(RedisPromiseState::Complete(data)) => Ok(Some(data)),
            Some(RedisPromiseState::Pending) => Ok(None),
            None => Err(GolemError::PromiseNotFound { promise_id }),
        }
    }

    async fn complete(&self, promise_id: PromiseId, data: Vec<u8>) -> Result<bool, GolemError> {
        let key = get_promise_redis_key(&promise_id);
        let value = self
            .redis
            .serialize(&RedisPromiseState::Complete(data.clone()))
            .expect("failed to serialize RedisPromiseState");

        let response: Option<Bytes> = self
            .redis
            .with("promise", "complete")
            .set(key, value, None, Some(SetOptions::XX), true)
            .await
            .unwrap_or_else(|err| panic!("failed to set promise {promise_id} in Redis: {err}"));

        let response: Option<RedisPromiseState> = response.map(|bs| {
            self.redis
                .deserialize(&bs)
                .expect("failed to deserialize RedisPromiseState")
        });

        match response {
            Some(RedisPromiseState::Complete(_)) => Ok(false),
            Some(RedisPromiseState::Pending) => {
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
            }
            None => Err(GolemError::PromiseNotFound { promise_id }),
        }
    }

    async fn delete(&self, promise_id: PromiseId) {
        let key = get_promise_redis_key(&promise_id);
        let _: u32 = self
            .redis
            .with("promise", "delete")
            .del(key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to delete promise {promise_id} from Redis: {err}")
            });
    }
}

fn get_promise_redis_key(promise_id: &PromiseId) -> String {
    format!("instance:promise:{}", promise_id.to_redis_key())
}

#[derive(Debug)]
enum PromiseState {
    Pending(
        Arc<Mutex<Option<oneshot::Sender<Vec<u8>>>>>,
        Mutex<oneshot::Receiver<Vec<u8>>>,
    ),
    Complete(Vec<u8>),
}

#[derive(Debug, Serialize, Deserialize, Encode, Decode)]
enum RedisPromiseState {
    Pending,
    Complete(Vec<u8>),
}

pub struct PromiseServiceInMemory {
    promises: Arc<DashMap<PromiseId, PromiseState>>,
}

impl Default for PromiseServiceInMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl PromiseServiceInMemory {
    pub fn new() -> Self {
        Self {
            promises: Arc::new(DashMap::new()),
        }
    }
}

#[async_trait]
impl PromiseService for PromiseServiceInMemory {
    async fn create(&self, worker_id: &WorkerId, oplog_idx: u64) -> PromiseId {
        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx,
        };

        let (sender, receiver) = oneshot::channel::<Vec<u8>>();
        let pending =
            PromiseState::Pending(Arc::new(Mutex::new(Some(sender))), Mutex::new(receiver));
        self.promises.insert(promise_id.clone(), pending);

        promise_id
    }

    async fn wait_for(&self, promise_id: PromiseId) -> Result<Vec<u8>, GolemError> {
        match self.promises.get(&promise_id) {
            Some(item) => match item.value() {
                PromiseState::Complete(data) => Ok(data.clone()),
                PromiseState::Pending(_, receiver) => {
                    let mut mutex_guard = receiver.lock().await;
                    let receiver = mutex_guard.deref_mut();
                    let data = receiver
                        .await
                        .map_err(|_| GolemError::PromiseDropped { promise_id })?;
                    Ok(data)
                }
            },
            None => Err(GolemError::PromiseNotFound { promise_id }),
        }
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<Option<Vec<u8>>, GolemError> {
        match self.promises.get(&promise_id) {
            Some(item) => match item.value() {
                PromiseState::Complete(data) => Ok(Some(data.clone())),
                PromiseState::Pending(_, _) => Ok(None),
            },
            None => Err(GolemError::PromiseNotFound { promise_id }),
        }
    }

    async fn complete(&self, promise_id: PromiseId, data: Vec<u8>) -> Result<bool, GolemError> {
        match self.promises.entry(promise_id.clone()) {
            Entry::Occupied(mut entry) => match entry.get() {
                PromiseState::Complete(_) => Ok(false),
                PromiseState::Pending(_, _) => {
                    let complete = PromiseState::Complete(data.clone());
                    *(entry.get_mut()) = complete;
                    Ok(true)
                }
            },
            Entry::Vacant(_) => Err(GolemError::PromiseNotFound { promise_id }),
        }
    }

    async fn delete(&self, promise_id: PromiseId) {
        self.promises.remove(&promise_id);
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct PromiseServiceMock {
    completed: Arc<Mutex<HashSet<PromiseId>>>,
}

#[cfg(any(feature = "mocks", test))]
impl Default for PromiseServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
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

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl PromiseService for PromiseServiceMock {
    async fn create(&self, _worker_id: &WorkerId, _oplog_idx: u64) -> PromiseId {
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
