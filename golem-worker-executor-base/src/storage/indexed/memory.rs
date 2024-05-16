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

use crate::storage::indexed::{IndexedStorage, IndexedStorageNamespace, ScanCursor};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use std::collections::BTreeMap;
use std::ops::Bound::Included;
use std::time::Duration;

#[derive(Debug)]
pub struct InMemoryIndexedStorage {
    data: DashMap<String, BTreeMap<u64, Vec<u8>>>,
}

impl Default for InMemoryIndexedStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryIndexedStorage {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
        }
    }

    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        format!("{:?}/{}", namespace, key)
    }
}

#[async_trait]
impl IndexedStorage for InMemoryIndexedStorage {
    async fn number_of_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
    ) -> Result<u8, String> {
        Ok(1)
    }

    async fn wait_for_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<u8, String> {
        Ok(1)
    }

    async fn exists(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self.data.contains_key(&composite_key))
    }

    async fn scan(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        pattern: &str,
        cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        // NOTE: not supporting cursor/count now
        if cursor == 0 {
            let mut result = Vec::new();
            let composite_pattern = Self::composite_key(namespace, pattern);

            if composite_pattern.ends_with('*')
                && !composite_pattern[0..composite_pattern.len() - 1].contains('*')
            {
                let prefix = &composite_pattern[0..composite_pattern.len() - 1];
                for entry in &self.data {
                    if entry.key().starts_with(prefix) {
                        result.push(entry.key().clone());
                    }
                }

                Ok((ScanCursor::MAX, result))
            } else {
                Err(
                    "Pattern not supported by the in-memory indexed storage implementation"
                        .to_string(),
                )
            }
        } else {
            Ok((ScanCursor::MAX, Vec::new()))
        }
    }

    async fn append(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: &[u8],
    ) -> Result<(), String> {
        let composite_key = Self::composite_key(namespace, key);
        let mut entry = self.data.entry(composite_key.clone()).or_default();
        entry.insert(id, value.to_vec());
        Ok(())
    }

    async fn length(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String> {
        let composite_key = Self::composite_key(namespace, key);
        match self.data.get(&composite_key) {
            Some(entry) => Ok(entry.len() as u64),
            None => Ok(0),
        }
    }

    async fn delete(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let composite_key = Self::composite_key(namespace, key);
        self.data.remove(&composite_key);
        Ok(())
    }

    async fn read(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<Bytes>, String> {
        let composite_key = Self::composite_key(namespace, key);
        let entry = self
            .data
            .get(&composite_key)
            .ok_or_else(|| "Key not found".to_string())?;

        let mut result = Vec::new();
        for (_id, value) in entry.range((Included(start_id), Included(end_id))) {
            result.push(Bytes::from(value.clone()));
        }

        Ok(result)
    }
}
