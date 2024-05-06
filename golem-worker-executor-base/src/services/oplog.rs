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

use async_mutex::Mutex;
use std::collections::{HashMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use fred::prelude::RedisValue;
use fred::types::RedisKey;
use golem_common::metrics::redis::record_redis_serialized_size;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::WorkerId;
use golem_common::redis::RedisPool;
use tracing::error;

use crate::metrics::oplog::record_oplog_call;

#[async_trait]
pub trait OplogService {
    async fn create(
        &self,
        worker_id: &WorkerId,
        initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync>;
    async fn open(&self, worker_id: &WorkerId) -> Arc<dyn Oplog + Send + Sync>;

    async fn get_size(&self, worker_id: &WorkerId) -> u64;

    async fn delete(&self, worker_id: &WorkerId);

    async fn read(&self, worker_id: &WorkerId, idx: u64, n: u64) -> Vec<OplogEntry>;
}

/// An open oplog providing write access
#[async_trait]
pub trait Oplog: Debug {
    async fn add(&self, entry: OplogEntry);
    async fn commit(&self);

    async fn current_oplog_index(&self) -> u64;

    /// Waits until Redis writes all changes into at least `replicas` replicas (or the maximum
    /// available).
    /// Returns true if the maximum possible number of replicas is reached within the timeout,
    /// otherwise false.
    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool;

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry;

    async fn add_and_commit(&self, entry: OplogEntry) -> OplogIndex {
        let idx = self.current_oplog_index().await;
        self.add(entry).await;
        self.commit().await;
        idx
    }
}

#[derive(Clone, Debug)]
pub struct RedisOplogService {
    redis: RedisPool,
    replicas: u8,
    max_operations_before_commit: u64,
}

impl RedisOplogService {
    pub async fn new(redis: RedisPool, max_operations_before_commit: u64) -> Self {
        let replicas = redis
            .with("oplog", "new")
            .info_connected_slaves()
            .await
            .unwrap_or_else(|err| panic!("failed to get the number of replicas from Redis: {err}"));
        Self {
            redis,
            replicas,
            max_operations_before_commit,
        }
    }
}

#[async_trait]
impl OplogService for RedisOplogService {
    async fn create(
        &self,
        worker_id: &WorkerId,
        initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync> {
        record_oplog_call("create");

        let key = get_oplog_redis_key(worker_id);
        let already_exists: bool = self
            .redis
            .with("oplog", "create")
            .exists(&key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to check if oplog exists for worker {worker_id} in Redis: {err}")
            });

        if already_exists {
            panic!("oplog for worker {worker_id} already exists in Redis")
        }

        let value = self.redis.serialize(&initial_entry).unwrap_or_else(|err| {
            panic!(
                "failed to serialize initial oplog entry for worker {worker_id}: {:?}: {err}",
                initial_entry
            )
        });

        record_redis_serialized_size("oplog", "entry", value.len());

        let field: RedisKey = "key".into();
        let _: String = self
            .redis
            .with("oplog", "create")
            .xadd(key, false, None, "1", (field, RedisValue::Bytes(value)))
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to append initial oplog entry for worker {worker_id} in Redis: {err}"
                )
            });

        self.open(worker_id).await
    }

    async fn open(&self, worker_id: &WorkerId) -> Arc<dyn Oplog + Send + Sync> {
        let key = get_oplog_redis_key(worker_id);
        let oplog_size: u64 = self
            .redis
            .with("oplog", "open")
            .xlen(&key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get oplog size for worker {worker_id} from Redis: {err}")
            });
        Arc::new(RedisOplog::new(
            self.redis.clone(),
            self.replicas,
            self.max_operations_before_commit,
            key,
            oplog_size,
        ))
    }

    async fn get_size(&self, worker_id: &WorkerId) -> u64 {
        record_oplog_call("get_size");

        let key = get_oplog_redis_key(worker_id);

        self.redis
            .with("oplog", "get_size")
            .xlen(key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get oplog size for worker {worker_id} from Redis: {err}")
            })
    }

    async fn delete(&self, worker_id: &WorkerId) {
        record_oplog_call("drop");

        let key = get_oplog_redis_key(worker_id);
        let _: () = self
            .redis
            .with("oplog", "drop")
            .del(key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to drop oplog for worker {worker_id} in Redis: {err}")
            });
    }

    async fn read(&self, worker_id: &WorkerId, idx: u64, n: u64) -> Vec<OplogEntry> {
        record_oplog_call("read");

        let key = get_oplog_redis_key(worker_id);

        let results: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with("oplog", "read")
            .xrange(key, idx + 1, idx + n, None)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to read oplog for worker {worker_id} from Redis: {err}")
            });

        let mut entries: Vec<OplogEntry> = Vec::new();

        for result in results.iter() {
            for (_, value) in result.iter() {
                for (_, value) in value.iter() {
                    let deserialized =
                        self.redis
                            .deserialize::<OplogEntry>(value)
                            .unwrap_or_else(|err| {
                                panic!("failed to deserialize oplog entry {:?}: {err}", value)
                            });

                    entries.push(deserialized);
                }
            }
        }

        entries
    }
}

fn get_oplog_redis_key(worker_id: &WorkerId) -> String {
    format!("instance:oplog:{}", worker_id.to_redis_key())
}

struct RedisOplog {
    state: Arc<Mutex<RedisOplogState>>,
    key: String,
}

impl RedisOplog {
    fn new(
        redis: RedisPool,
        replicas: u8,
        max_operations_before_commit: u64,
        key: String,
        oplog_size: u64,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(RedisOplogState {
                redis,
                replicas,
                max_operations_before_commit,
                key: key.clone(),
                buffer: VecDeque::new(),
                last_committed_idx: oplog_size,
                last_oplog_idx: oplog_size,
            })),
            key,
        }
    }
}

struct RedisOplogState {
    redis: RedisPool,
    replicas: u8,
    max_operations_before_commit: u64,
    key: String,
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: u64,
    last_committed_idx: u64,
}

impl RedisOplogState {
    async fn append(&mut self, arrays: &[OplogEntry]) {
        record_oplog_call("append");

        for entry in arrays {
            let value = self.redis.serialize(entry).unwrap_or_else(|err| {
                panic!(
                    "failed to serialize oplog entry for {}: {:?}: {err}",
                    self.key, entry
                )
            });

            record_redis_serialized_size("oplog", "entry", value.len());

            let field: RedisKey = "key".into();
            let id = self.last_committed_idx + 1;

            let _: String = self
                .redis
                .with("oplog", "append")
                .xadd(
                    &self.key,
                    true,
                    None,
                    id.to_string(),
                    (field, RedisValue::Bytes(value)),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to append oplog entry for {} in Redis: {err}",
                        self.key
                    )
                });
            self.last_committed_idx += 1;
        }
    }

    async fn add(&mut self, entry: OplogEntry) {
        self.buffer.push_back(entry);
        if self.buffer.len() > self.max_operations_before_commit as usize {
            self.commit().await;
        }
        self.last_oplog_idx += 1;
    }

    async fn commit(&mut self) {
        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();
        self.append(&entries).await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        let replicas = replicas.min(self.replicas);
        match self
            .redis
            .with("oplog", "wait_for_replicas")
            .wait(replicas as i64, timeout.as_millis() as i64)
            .await
        {
            Ok(n) => n as u8 == replicas,
            Err(err) => {
                error!("Failed to execute WAIT command: {:?}", err);
                false
            }
        }
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let results: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with("oplog", "read")
            .xrange(&self.key, oplog_index + 1, oplog_index + 1, None)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to read oplog entry {oplog_index} from {} from Redis: {err}",
                    self.key
                )
            });

        let mut entries: Vec<OplogEntry> = Vec::new();

        for result in results.iter() {
            for (_, value) in result.iter() {
                for (_, value) in value.iter() {
                    let deserialized =
                        self.redis
                            .deserialize::<OplogEntry>(value)
                            .unwrap_or_else(|err| {
                                panic!("failed to deserialize oplog entry {:?}: {err}", value)
                            });

                    entries.push(deserialized);
                }
            }
        }

        entries.into_iter().next().unwrap_or_else(|| {
            panic!(
                "Missing oplog entry {oplog_index} for {} in Redis",
                self.key
            )
        })
    }
}

impl Debug for RedisOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

#[async_trait]
impl Oplog for RedisOplog {
    async fn add(&self, entry: OplogEntry) {
        let mut state = self.state.lock().await;
        state.add(entry).await
    }

    async fn commit(&self) {
        let mut state = self.state.lock().await;
        state.commit().await
    }

    async fn current_oplog_index(&self) -> u64 {
        let state = self.state.lock().await;
        state.last_oplog_idx
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        let mut state = self.state.lock().await;
        state.commit().await;
        state.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let state = self.state.lock().await;
        state.read(oplog_index).await
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct OplogServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for OplogServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl OplogServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl OplogService for OplogServiceMock {
    async fn create(
        &self,
        _worker_id: &WorkerId,
        _initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn open(&self, _worker_id: &WorkerId) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn get_size(&self, _worker_id: &WorkerId) -> u64 {
        unimplemented!()
    }

    async fn delete(&self, _worker_id: &WorkerId) {
        unimplemented!()
    }

    async fn read(&self, _worker_id: &WorkerId, _idx: u64, _n: u64) -> Vec<OplogEntry> {
        unimplemented!()
    }
}
