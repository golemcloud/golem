use std::collections::HashMap;

use async_trait::async_trait;
use bytes::Bytes;
use fred::prelude::RedisValue;
use fred::types::RedisKey;
use golem_common::metrics::redis::record_redis_serialized_size;
use golem_common::model::OplogEntry;
use golem_common::model::WorkerId;
use golem_common::redis::RedisPool;

use crate::metrics::oplog::record_oplog_call;

#[async_trait]
pub trait OplogService {
    async fn append(&self, worker_id: &WorkerId, arrays: &[OplogEntry]);

    async fn get_size(&self, worker_id: &WorkerId) -> i32;

    async fn delete(&self, worker_id: &WorkerId);

    async fn read(&self, worker_id: &WorkerId, idx: i32, n: i32) -> Vec<OplogEntry>;
}

#[derive(Clone, Debug)]
pub struct OplogServiceDefault {
    redis: RedisPool,
}

impl OplogServiceDefault {
    pub fn new(redis: RedisPool) -> Self {
        Self { redis }
    }
}

#[async_trait]
impl OplogService for OplogServiceDefault {
    async fn append(&self, worker_id: &WorkerId, arrays: &[OplogEntry]) {
        record_oplog_call("append");

        let key = get_oplog_redis_key(worker_id);

        let last: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with("oplog", "append")
            .xrevrange(key, "+", "-", Some(1))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get last oplog entry for instance {worker_id} from Redis: {err}")
            });

        let mut idx = if last.is_empty() {
            1
        } else {
            last[0]
                .keys()
                .next()
                .unwrap_or_else(|| panic!("No keys in last oplog entry for {worker_id}"))
                .split('-')
                .collect::<Vec<&str>>()[0]
                .parse::<i64>()
                .unwrap_or_else(|err| {
                    panic!("Failed to parse the index in the key of oplog entry for {worker_id}: {err}")
                })
                + 1
        };

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
                    false,
                    None,
                    idx.to_string(),
                    (field, RedisValue::Bytes(value)),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to append oplog entry for worker {worker_id} in Redis: {err}")
                });
            idx += 1;
        }
    }

    async fn get_size(&self, worker_id: &WorkerId) -> i32 {
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

    async fn read(&self, worker_id: &WorkerId, idx: i32, n: i32) -> Vec<OplogEntry> {
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

#[cfg(test)]
pub struct OplogServiceMock {}

#[cfg(test)]
impl Default for OplogServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl OplogServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(test)]
#[async_trait]
impl OplogService for OplogServiceMock {
    async fn append(&self, _worker_id: &WorkerId, _arrays: &[OplogEntry]) {
        unimplemented!()
    }

    async fn get_size(&self, _worker_id: &WorkerId) -> i32 {
        unimplemented!()
    }

    async fn delete(&self, _worker_id: &WorkerId) {
        unimplemented!()
    }

    async fn read(&self, _worker_id: &WorkerId, _idx: i32, _n: i32) -> Vec<OplogEntry> {
        unimplemented!()
    }
}
