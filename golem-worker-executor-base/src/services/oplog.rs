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

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use fred::prelude::RedisValue;
use fred::types::RedisKey;
use golem_common::metrics::redis::record_redis_serialized_size;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::WorkerId;
use golem_common::redis::RedisPool;
use tracing::error;

use crate::metrics::oplog::record_oplog_call;

#[async_trait]
pub trait OplogService {
    async fn create(&self, worker_id: &WorkerId, initial_entry: OplogEntry);

    async fn append(&self, worker_id: &WorkerId, arrays: &[OplogEntry]);

    async fn get_size(&self, worker_id: &WorkerId) -> u64;

    async fn delete(&self, worker_id: &WorkerId);

    async fn read(&self, worker_id: &WorkerId, idx: u64, n: u64) -> Vec<OplogEntry>;

    /// Waits until Redis writes all changes into at least `replicas` replicas (or the maximum
    /// available).
    /// Returns true if the maximum possible number of replicas is reached within the timeout,
    /// otherwise false.
    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool;
}

#[derive(Clone, Debug)]
pub struct OplogServiceDefault {
    redis: RedisPool,
    replicas: u8,
}

impl OplogServiceDefault {
    pub async fn new(redis: RedisPool) -> Self {
        let replicas = redis
            .with("oplog", "new")
            .info_connected_slaves()
            .await
            .unwrap_or_else(|err| panic!("failed to get the number of replicas from Redis: {err}"));
        Self { redis, replicas }
    }
}

#[async_trait]
impl OplogService for OplogServiceDefault {
    async fn create(&self, worker_id: &WorkerId, initial_entry: OplogEntry) {
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
    }

    async fn append(&self, worker_id: &WorkerId, arrays: &[OplogEntry]) {
        record_oplog_call("append");

        let key = get_oplog_redis_key(worker_id);

        let len: usize = self
            .redis
            .with("oplog", "append")
            .xlen(key)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get oplog size for worker {worker_id} from Redis: {err}")
            });
        let mut id = len + 1;

        for entry in arrays {
            let key = get_oplog_redis_key(worker_id);
            let value = self.redis.serialize(entry).unwrap_or_else(|err| {
                panic!(
                    "failed to serialize oplog entry for worker {worker_id}: {:?}: {err}",
                    entry
                )
            });

            record_redis_serialized_size("oplog", "entry", value.len());

            let field: RedisKey = "key".into();
            let _: String = self
                .redis
                .with("oplog", "append")
                .xadd(
                    key,
                    true,
                    None,
                    id.to_string(),
                    (field, RedisValue::Bytes(value)),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to append oplog entry for worker {worker_id} in Redis: {err}")
                });
            id += 1;
        }
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
}

fn get_oplog_redis_key(worker_id: &WorkerId) -> String {
    format!("instance:oplog:{}", worker_id.to_redis_key())
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
    async fn create(&self, _worker_id: &WorkerId, _initial_entry: OplogEntry) {
        unimplemented!()
    }

    async fn append(&self, _worker_id: &WorkerId, _arrays: &[OplogEntry]) {
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

    async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
        unimplemented!()
    }
}
