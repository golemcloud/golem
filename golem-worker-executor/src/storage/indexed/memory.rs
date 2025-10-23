// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::storage::indexed::{IndexedStorage, IndexedStorageNamespace, ScanCursor};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::ops::Bound::Included;
use std::time::Duration;

#[derive(Debug)]
pub struct InMemoryIndexedStorage {
    data: scc::HashMap<String, BTreeMap<u64, Vec<u8>>>,
}

impl Default for InMemoryIndexedStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryIndexedStorage {
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::new(),
        }
    }

    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        format!("{namespace:?}/{key}")
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
        Ok(self.data.contains_async(&composite_key).await)
    }

    async fn scan(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        let mut result = Vec::new();
        let composite_pattern = Self::composite_key(namespace.clone(), pattern);
        let composite_prefix = Self::composite_key(namespace, "");

        if composite_pattern.ends_with('*')
            && !composite_pattern[0..composite_pattern.len() - 1].contains('*')
        {
            let prefix = &composite_pattern[0..composite_pattern.len() - 1];
            let mut idx = 0;
            let mut has_more = false;

            self.data
                .iter_async(|key, _| {
                    idx += 1;
                    if idx > cursor && key.starts_with(prefix) {
                        result.push(key[composite_prefix.len()..].to_string());

                        if (result.len() as u64) == count {
                            has_more = true;
                            false
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                })
                .await;

            if has_more {
                Ok((idx, result))
            } else {
                Ok((0, result))
            }
        } else {
            Err("Pattern not supported by the in-memory indexed storage implementation".to_string())
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
        let mut entry = self
            .data
            .entry_async(composite_key.clone())
            .await
            .or_default();
        if let std::collections::btree_map::Entry::Vacant(e) = entry.entry(id) {
            e.insert(value.to_vec());
            Ok(())
        } else {
            Err("Key already exists".to_string())
        }
    }

    async fn length(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| entry.len() as u64)
            .await
            .unwrap_or_default())
    }

    async fn delete(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let composite_key = Self::composite_key(namespace, key);
        self.data.remove_async(&composite_key).await;
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
    ) -> Result<Vec<(u64, Bytes)>, String> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let mut result = Vec::new();
                for (id, value) in entry.range((Included(start_id), Included(end_id))) {
                    result.push((*id, Bytes::from(value.clone())));
                }
                result
            })
            .await
            .unwrap_or_default())
    }

    async fn first(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let first = entry.first_key_value();
                first.map(|(id, value)| (*id, Bytes::from(value.clone())))
            })
            .await
            .flatten())
    }

    async fn last(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let last = entry.last_key_value();
                last.map(|(id, value)| (*id, Bytes::from(value.clone())))
            })
            .await
            .flatten())
    }

    async fn closest(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Bytes)>, String> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                entry
                    .keys()
                    .find(|k| **k >= id)
                    .map(|key| (*key, Bytes::from(entry[key].clone())))
            })
            .await
            .flatten())
    }

    async fn drop_prefix(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), String> {
        let composite_key = Self::composite_key(namespace, key);
        self.data
            .update_async(&composite_key, |_, entry| {
                entry.retain(|k, _| *k > last_dropped_id);
            })
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::storage::indexed::{IndexedStorageLabelledApi, IndexedStorageNamespace};
    use assert2::check;

    #[test]
    async fn closest_exact_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 1, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 2, &200)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 3, &300)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 4, &400)
            .await
            .unwrap();

        let result = api
            .closest(IndexedStorageNamespace::OpLog, key, 3)
            .await
            .unwrap();

        check!(result == Some((3, 300)));
    }

    #[test]
    async fn closest_no_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 1, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 2, &200)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 3, &300)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 4, &400)
            .await
            .unwrap();

        let result: Option<(u64, i32)> = api
            .closest(IndexedStorageNamespace::OpLog, key, 5)
            .await
            .unwrap();

        check!(result == None);
    }

    #[test]
    async fn closest_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 10, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 20, &200)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 30, &300)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 40, &400)
            .await
            .unwrap();

        let result = api
            .closest(IndexedStorageNamespace::OpLog, key, 33) // 40 is the closest that is <= 33
            .await
            .unwrap();

        check!(result == Some((40, 400)));
    }

    #[test]
    async fn read() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 10, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 20, &200)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 30, &300)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 40, &400)
            .await
            .unwrap();

        let result = api
            .read(IndexedStorageNamespace::OpLog, key, 20, 40)
            .await
            .unwrap();

        check!(result == vec![(20, 200), (30, 300), (40, 400)]);
    }

    #[test]
    async fn read_wider() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 10, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 20, &200)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 30, &300)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 40, &400)
            .await
            .unwrap();

        let result = api
            .read(IndexedStorageNamespace::OpLog, key, 1, 100)
            .await
            .unwrap();

        check!(result == vec![(10, 100), (20, 200), (30, 300), (40, 400)]);
    }

    #[test]
    async fn first() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 10, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 20, &200)
            .await
            .unwrap();

        let result = api
            .first(IndexedStorageNamespace::OpLog, key)
            .await
            .unwrap();

        check!(result == Some((10, 100)));
    }

    #[test]
    async fn last() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 10, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 20, &200)
            .await
            .unwrap();

        let result = api.last(IndexedStorageNamespace::OpLog, key).await.unwrap();

        check!(result == Some((20, 200)));
    }

    #[test]
    async fn drop_prefix() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(IndexedStorageNamespace::OpLog, key, 1, &100)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 2, &200)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 3, &300)
            .await
            .unwrap();
        api.append(IndexedStorageNamespace::OpLog, key, 4, &400)
            .await
            .unwrap();

        storage
            .with("test", "test")
            .drop_prefix(IndexedStorageNamespace::OpLog, key, 2)
            .await
            .unwrap();

        let result = api
            .read(IndexedStorageNamespace::OpLog, key, 1, 4)
            .await
            .unwrap();

        check!(result == vec![(3, 300), (4, 400)]);
    }
}
