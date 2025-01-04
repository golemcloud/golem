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

use async_trait::async_trait;
use bytes::Bytes;
use fred::types::SetOptions;
use golem_common::metrics::redis::{record_redis_deserialized_size, record_redis_serialized_size};
use golem_common::redis::RedisPool;
use std::collections::HashMap;
use tracing::debug;

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};

#[derive(Debug)]
pub struct RedisKeyValueStorage {
    redis: RedisPool,
}

impl RedisKeyValueStorage {
    pub fn new(redis: RedisPool) -> Self {
        Self { redis }
    }

    fn use_hash(namespace: &KeyValueStorageNamespace) -> Option<String> {
        match namespace {
            KeyValueStorageNamespace::Worker => None,
            KeyValueStorageNamespace::Promise => Some("promises".to_string()),
            KeyValueStorageNamespace::Schedule => None,
            KeyValueStorageNamespace::UserDefined { account_id, bucket } => {
                Some(format!("user-defined:{account_id}:{bucket}"))
            }
        }
    }
}

#[async_trait]
impl KeyValueStorage for RedisKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hset(ns, (key, value))
                .await
                .map_err(|redis_err| redis_err.to_string()),
            None => self
                .redis
                .with(svc_name, api_name)
                .set(key, value, None, None, false)
                .await
                .map_err(|redis_err| redis_err.to_string()),
        }
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        let mut map: HashMap<&str, &[u8]> = HashMap::new();
        for (k, v) in pairs {
            map.insert(*k, *v);
            record_redis_serialized_size(svc_name, entity_name, v.len());
        }
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hmset(ns, map)
                .await
                .map_err(|redis_err| redis_err.to_string()),
            None => self
                .redis
                .with(svc_name, api_name)
                .mset(map)
                .await
                .map_err(|redis_err| redis_err.to_string()),
        }
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        match Self::use_hash(&namespace) {
            Some(ns) => {
                let result: bool = self
                    .redis
                    .with(svc_name, api_name)
                    .hsetnx(ns, key, value)
                    .await
                    .map_err(|redis_err| redis_err.to_string())?;

                debug!("set_if_not_exists hsetnx result: {:?}", result);
                Ok(result)
            }
            None => {
                let result: Option<String> = self
                    .redis
                    .with(svc_name, api_name)
                    .set(key, value, None, Some(SetOptions::NX), false)
                    .await
                    .map_err(|redis_err| redis_err.to_string())?;

                debug!("set_if_not_exists result: {:?}", result);
                Ok(result == Some("OK".to_string()))
            }
        }
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        let serialized: Option<Bytes> = match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hget(ns, key)
                .await
                .map_err(|redis_err| redis_err.to_string())?,
            None => self
                .redis
                .with(svc_name, api_name)
                .get(key)
                .await
                .map_err(|redis_err| redis_err.to_string())?,
        };

        if let Some(serialized) = serialized {
            record_redis_deserialized_size(svc_name, entity_name, serialized.len());
            Ok(Some(serialized))
        } else {
            Ok(None)
        }
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        let serialized: Vec<Option<Bytes>> = match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hmget(ns, keys)
                .await
                .map_err(|redis_err| redis_err.to_string())?,
            None => self
                .redis
                .with(svc_name, api_name)
                .mget(keys)
                .await
                .map_err(|redis_err| redis_err.to_string())?,
        };

        for s in serialized.iter().flatten() {
            record_redis_deserialized_size(svc_name, entity_name, s.len());
        }

        Ok(serialized)
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hdel(ns, key)
                .await
                .map_err(|redis_err| redis_err.to_string()),
            None => self
                .redis
                .with(svc_name, api_name)
                .del(key)
                .await
                .map_err(|redis_err| redis_err.to_string()),
        }
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hdel(ns, keys)
                .await
                .map_err(|redis_err| redis_err.to_string()),
            None => self
                .redis
                .with(svc_name, api_name)
                .del_many(keys)
                .await
                .map_err(|redis_err| redis_err.to_string()),
        }
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hexists(ns, key)
                .await
                .map_err(|redis_err| redis_err.to_string()),
            None => self
                .redis
                .with(svc_name, api_name)
                .exists(key)
                .await
                .map_err(|redis_err| redis_err.to_string()),
        }
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        match Self::use_hash(&namespace) {
            Some(ns) => self
                .redis
                .with(svc_name, api_name)
                .hkeys(ns)
                .await
                .map_err(|redis_err| redis_err.to_string()),
            None => self
                .redis
                .with(svc_name, api_name)
                .keys("*".to_string())
                .await
                .map_err(|redis_err| redis_err.to_string()),
        }
    }

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{}:{}", ns, key),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .sadd(&key, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{}:{}", ns, key),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .srem(&key, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{}:{}", ns, key),
            None => key.to_string(),
        };
        let members: Vec<Bytes> = self
            .redis
            .with(svc_name, api_name)
            .smembers(&key)
            .await
            .map_err(|e| e.to_string())?;

        for member in &members {
            record_redis_deserialized_size(svc_name, entity_name, member.len());
        }

        Ok(members)
    }

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), String> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{}:{}", ns, key),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .zadd(&key, None, None, false, false, (score, value))
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{}:{}", ns, key),
            None => key.to_string(),
        };
        self.redis
            .with(svc_name, api_name)
            .zrem(&key, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{}:{}", ns, key),
            None => key.to_string(),
        };
        let pairs: Vec<(Bytes, f64)> = self
            .redis
            .with(svc_name, api_name)
            .zrange(&key, 0, -1, None, false, None, true)
            .await
            .map_err(|e| e.to_string())?;

        for (data, _score) in &pairs {
            record_redis_deserialized_size(svc_name, entity_name, data.len());
        }

        Ok(pairs
            .into_iter()
            .map(|(data, score)| (score, data))
            .collect())
    }

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let key = match Self::use_hash(&namespace) {
            Some(ns) => format!("{}:{}", ns, key),
            None => key.to_string(),
        };
        let pairs: Vec<(Bytes, f64)> = self
            .redis
            .with(svc_name, api_name)
            .zrangebyscore(&key, min, max, true, None)
            .await
            .map_err(|e| e.to_string())?;

        for (data, _score) in &pairs {
            record_redis_deserialized_size(svc_name, entity_name, data.len());
        }

        Ok(pairs
            .into_iter()
            .map(|(data, score)| (score, data))
            .collect())
    }
}
