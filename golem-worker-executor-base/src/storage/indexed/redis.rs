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

use crate::storage::indexed::{IndexedStorage, ScanCursor};
use async_trait::async_trait;
use bytes::Bytes;
use fred::types::{RedisKey, RedisValue};
use golem_common::metrics::redis::{record_redis_deserialized_size, record_redis_serialized_size};
use golem_common::redis::RedisPool;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug)]
pub struct RedisIndexedStorage {
    redis: RedisPool,
}

impl RedisIndexedStorage {
    pub fn new(redis: RedisPool) -> Self {
        Self { redis }
    }

    fn composite_key(namespace: Option<&str>, key: &str) -> String {
        match namespace {
            Some(namespace) => format!("{}:{}", namespace, key),
            None => key.to_string(),
        }
    }

    const KEY: &'static str = "key";
}

#[async_trait]
impl IndexedStorage for RedisIndexedStorage {
    async fn number_of_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> Result<u8, String> {
        self.redis
            .with(svc_name, api_name)
            .info_connected_slaves()
            .await
            .map_err(|e| e.to_string())
    }

    async fn wait_for_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, String> {
        self.redis
            .with(svc_name, api_name)
            .wait(replicas as i64, timeout.as_millis() as i64)
            .await
            .map(|r| r as u8)
            .map_err(|e| e.to_string())
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        key: &str,
    ) -> Result<bool, String> {
        self.redis
            .with(svc_name, api_name)
            .exists(Self::composite_key(namespace, key))
            .await
            .map_err(|e| e.to_string())
    }

    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        self.redis
            .with(svc_name, api_name)
            .scan(Self::composite_key(namespace, pattern), cursor, count)
            .await
            .map_err(|e| e.to_string())
    }

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: Option<&str>,
        key: &str,
        id: u64,
        value: &[u8],
    ) -> Result<(), String> {
        record_redis_serialized_size(svc_name, entity_name, value.len());

        let _: String = self
            .redis
            .with(svc_name, api_name)
            .xadd(
                Self::composite_key(namespace, key),
                false,
                None,
                id.to_string(),
                (
                    RedisKey::from(Self::KEY),
                    RedisValue::Bytes(Bytes::copy_from_slice(value)),
                ),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        key: &str,
    ) -> Result<u64, String> {
        self.redis
            .with(svc_name, api_name)
            .xlen(Self::composite_key(namespace, key))
            .await
            .map_err(|e| e.to_string())
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: Option<&str>,
        key: &str,
    ) -> Result<(), String> {
        self.redis
            .with(svc_name, api_name)
            .del(Self::composite_key(namespace, key))
            .await
            .map_err(|e| e.to_string())
    }

    async fn read(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: Option<&str>,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<Bytes>, String> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrange(Self::composite_key(namespace, key), start_id, end_id, None)
            .await
            .map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        for item in items {
            for (_, value) in item {
                for (key, value) in value {
                    if key == Self::KEY {
                        record_redis_deserialized_size(svc_name, entity_name, value.len());
                        result.push(value);
                    }
                }
            }
        }
        Ok(result)
    }
}
