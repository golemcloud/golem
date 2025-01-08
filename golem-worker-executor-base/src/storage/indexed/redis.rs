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

use crate::storage::indexed::{IndexedStorage, IndexedStorageNamespace, ScanCursor};
use async_trait::async_trait;
use bytes::Bytes;
use fred::types::{RedisKey, RedisValue, XCapKind};
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

    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog => format!("worker:oplog:{key}"),
            IndexedStorageNamespace::CompressedOpLog { level } => {
                format!("worker:c{level}-oplog:{key}")
            }
        }
    }

    fn parse_composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        let prefix = Self::composite_key(namespace, "");
        if key.starts_with(&prefix) {
            key[prefix.len()..].to_string()
        } else {
            key.to_string()
        }
    }

    const KEY: &'static str = "key";

    fn parse_entry_id(id: &str) -> Result<u64, String> {
        if let Some((id, _)) = id.split_once('-') {
            id.parse::<u64>()
                .map_err(|e| format!("Failed to parse {id} as u64: {e}"))
        } else {
            id.parse::<u64>()
                .map_err(|e| format!("Failed to parse {id} as u64: {e}"))
        }
    }

    fn process_stream(
        &self,
        svc_name: &'static str,
        entity_name: &'static str,
        items: Vec<HashMap<String, HashMap<String, Bytes>>>,
    ) -> Result<Vec<(u64, Bytes)>, String> {
        let mut result = Vec::new();
        for item in items {
            for (id, value) in item {
                let id = Self::parse_entry_id(&id)?;
                for (key, value) in value {
                    if key == Self::KEY {
                        record_redis_deserialized_size(svc_name, entity_name, value.len());
                        result.push((id, value));
                    }
                }
            }
        }
        Ok(result)
    }
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
        namespace: IndexedStorageNamespace,
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
        namespace: IndexedStorageNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        let (cursor, keys) = self
            .redis
            .with(svc_name, api_name)
            .scan(
                Self::composite_key(namespace.clone(), pattern),
                cursor,
                count,
            )
            .await
            .map_err(|e| e.to_string())?;
        let keys = keys
            .into_iter()
            .map(|k| Self::parse_composite_key(namespace.clone(), &k))
            .collect();
        Ok((cursor, keys))
    }

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
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
        namespace: IndexedStorageNamespace,
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
        namespace: IndexedStorageNamespace,
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
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Bytes)>, String> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrange(Self::composite_key(namespace, key), start_id, end_id, None)
            .await
            .map_err(|e| e.to_string())?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result)
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrange(Self::composite_key(namespace, key), "-", "+", Some(1))
            .await
            .map_err(|e| e.to_string())?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result.into_iter().next())
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrevrange(Self::composite_key(namespace, key), "+", "-", Some(1))
            .await
            .map_err(|e| e.to_string())?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result.into_iter().next())
    }

    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Bytes)>, String> {
        let items: Vec<HashMap<String, HashMap<String, Bytes>>> = self
            .redis
            .with(svc_name, api_name)
            .xrange(Self::composite_key(namespace, key), id, "+", Some(1))
            .await
            .map_err(|e| e.to_string())?;

        let result = self.process_stream(svc_name, entity_name, items)?;
        Ok(result.into_iter().next())
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), String> {
        let _: u64 = self
            .redis
            .with(svc_name, api_name)
            .xtrim(
                Self::composite_key(namespace, key),
                (XCapKind::MinID, last_dropped_id + 1),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
