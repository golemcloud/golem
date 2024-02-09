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

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::AccountId;
use golem_common::redis::RedisPool;

use crate::services::golem_config::KeyValueServiceConfig;

/// Service implementing a persistent key-value store
#[async_trait]
pub trait KeyValueService {
    async fn delete(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<()>;

    async fn delete_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<()>;

    async fn exists(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool>;

    async fn get(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>>;

    async fn get_keys(&self, account_id: AccountId, bucket: String) -> anyhow::Result<Vec<String>>;

    async fn get_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>>;

    async fn set(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()>;

    async fn set_many(
        &self,
        account_id: AccountId,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<()>;
}

pub fn configured(
    config: &KeyValueServiceConfig,
    redis_pool: RedisPool,
) -> Arc<dyn KeyValueService + Send + Sync> {
    match config {
        KeyValueServiceConfig::InMemory => Arc::new(KeyValueServiceInMemory::new()),
        KeyValueServiceConfig::Redis => Arc::new(KeyValueServiceRedis::new(redis_pool.clone())),
    }
}

#[derive(Clone, Debug)]
pub struct KeyValueServiceRedis {
    redis: RedisPool,
}

impl KeyValueServiceRedis {
    pub fn new(redis: RedisPool) -> Self {
        Self { redis }
    }
}

#[async_trait]
impl KeyValueService for KeyValueServiceRedis {
    async fn delete(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<()> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        self.redis
            .with("key_value", "delete")
            .hdel(bucket, key)
            .await?;
        Ok(())
    }

    async fn delete_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<()> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        self.redis
            .with("key_value", "delete_many")
            .hdel(bucket, keys)
            .await?;
        Ok(())
    }

    async fn exists(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        let exists: bool = self
            .redis
            .with("key_value", "exists")
            .hexists(bucket, key)
            .await?;
        Ok(exists)
    }

    async fn get(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        let incoming_value: Option<Vec<u8>> = self
            .redis
            .with("key_value", "get")
            .hget(bucket, key)
            .await?;
        Ok(incoming_value)
    }

    async fn get_keys(&self, account_id: AccountId, bucket: String) -> anyhow::Result<Vec<String>> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        let keys: Vec<String> = self
            .redis
            .with("key_value", "get_keys")
            .hkeys(bucket)
            .await?;
        Ok(keys)
    }

    async fn get_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        let incoming_values: Vec<Option<Vec<u8>>> = self
            .redis
            .with("key_value", "get_many")
            .hmget(bucket, keys)
            .await?;
        Ok(incoming_values)
    }

    async fn set(
        &self,
        account_id: AccountId,
        bucket: String,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        self.redis
            .with("key_value", "set")
            .hset(bucket, (key, Bytes::from(outgoing_value)))
            .await?;
        Ok(())
    }

    async fn set_many(
        &self,
        account_id: AccountId,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<()> {
        let bucket = format!("instance:keyvalue:{}:{}", account_id, bucket);
        let key_values: Vec<(String, Bytes)> = key_values
            .into_iter()
            .map(|(key, value)| (key, Bytes::from(value)))
            .collect();
        self.redis
            .with("key_value", "set_many")
            .hmset(bucket, key_values)
            .await?;
        Ok(())
    }
}

type Bucket = HashMap<String, Vec<u8>>;
type Buckets = HashMap<String, Bucket>;

pub struct KeyValueServiceInMemory {
    buckets: Arc<RwLock<Buckets>>,
}

impl Default for KeyValueServiceInMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyValueServiceInMemory {
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(RwLock::new(Buckets::new())),
        }
    }
}

#[async_trait]
impl KeyValueService for KeyValueServiceInMemory {
    async fn delete(
        &self,
        _account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<()> {
        let mut buckets = self.buckets.write().unwrap();
        if let Some(bucket) = buckets.get_mut(&bucket) {
            match bucket.entry(key) {
                Entry::Occupied(entry) => entry.remove(),
                Entry::Vacant(_) => {
                    anyhow::bail!("Key does not exist");
                }
            };
            Ok(())
        } else {
            anyhow::bail!("Container does not exist");
        }
    }

    async fn delete_many(
        &self,
        account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<()> {
        for key in keys {
            self.delete(account_id.clone(), bucket.clone(), key).await?;
        }
        Ok(())
    }

    async fn exists(
        &self,
        _account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool> {
        let buckets = self.buckets.read().unwrap();
        if let Some(bucket) = buckets.get(&bucket) {
            Ok(bucket.contains_key(&key))
        } else {
            anyhow::bail!("Container does not exist");
        }
    }

    async fn get(
        &self,
        _account_id: AccountId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let buckets = self.buckets.read().unwrap();
        if let Some(bucket) = buckets.get(&bucket) {
            Ok(bucket.get(&key).cloned())
        } else {
            anyhow::bail!("Container does not exist");
        }
    }

    async fn get_keys(
        &self,
        _account_id: AccountId,
        bucket: String,
    ) -> anyhow::Result<Vec<String>> {
        let buckets = self.buckets.read().unwrap();
        if let Some(bucket) = buckets.get(&bucket) {
            Ok(bucket.keys().cloned().collect())
        } else {
            anyhow::bail!("Container does not exist");
        }
    }

    async fn get_many(
        &self,
        _account_id: AccountId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        let mut result = Vec::new();
        for key in keys {
            result.push(self.get(_account_id.clone(), bucket.clone(), key).await?);
        }
        Ok(result)
    }

    async fn set(
        &self,
        _account_id: AccountId,
        bucket: String,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()> {
        let mut buckets = self.buckets.write().unwrap();
        if let Some(bucket) = buckets.get_mut(&bucket) {
            bucket.insert(key, outgoing_value);
            Ok(())
        } else {
            anyhow::bail!("Container does not exist");
        }
    }

    async fn set_many(
        &self,
        _account_id: AccountId,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<()> {
        for (key, value) in key_values {
            self.set(_account_id.clone(), bucket.clone(), key, value)
                .await?;
        }
        Ok(())
    }
}
