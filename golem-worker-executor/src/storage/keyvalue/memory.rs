// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use scc::hash_map::Entry;

#[derive(Debug)]
pub struct InMemoryKeyValueStorage {
    kvs: scc::HashMap<String, Vec<u8>>,
    sets: scc::HashMap<String, scc::HashSet<Vec<u8>>>,
    sorted_sets: scc::HashMap<String, Vec<(f64, Vec<u8>)>>,
    /// Guards the plain key-value (`kvs`) operations so that multi-key writes/reads
    /// (`set_many`/`get_many`/`del_many`) are atomic with respect to each other and to single-key
    /// writes. `scc::HashMap` only provides per-key atomicity; this lock provides the cross-key
    /// all-or-nothing semantics the split agent-status cache relies on. Writes take the write
    /// lock, reads take the read lock. (`sets`/`sorted_sets` are independent and not guarded.)
    kvs_lock: tokio::sync::RwLock<()>,
}

impl Default for InMemoryKeyValueStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryKeyValueStorage {
    pub fn new() -> Self {
        Self {
            kvs: scc::HashMap::new(),
            sets: scc::HashMap::new(),
            sorted_sets: scc::HashMap::new(),
            kvs_lock: tokio::sync::RwLock::new(()),
        }
    }

    pub fn kvs(&self) -> &scc::HashMap<String, Vec<u8>> {
        &self.kvs
    }

    pub fn sets(&self) -> &scc::HashMap<String, scc::HashSet<Vec<u8>>> {
        &self.sets
    }

    pub fn sorted_sets(&self) -> &scc::HashMap<String, Vec<(f64, Vec<u8>)>> {
        &self.sorted_sets
    }

    fn composite_key(namespace: &KeyValueStorageNamespace, key: &str) -> String {
        format!("{namespace:?}/{key}")
    }
}

#[async_trait]
impl KeyValueStorage for InMemoryKeyValueStorage {
    async fn set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let _guard = self.kvs_lock.write().await;
        self.kvs
            .upsert_async(Self::composite_key(&namespace, key), value.to_vec())
            .await;
        Ok(())
    }

    async fn set_many(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        let _guard = self.kvs_lock.write().await;
        for (key, value) in pairs {
            self.kvs
                .upsert_async(Self::composite_key(&namespace, key), value.to_vec())
                .await;
        }
        Ok(())
    }

    async fn set_if_not_exists(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String> {
        let _guard = self.kvs_lock.write().await;
        match self
            .kvs
            .entry_async(Self::composite_key(&namespace, key))
            .await
        {
            Entry::Occupied(_) => Ok(false),
            Entry::Vacant(entry) => {
                entry.insert_entry(value.to_vec());
                Ok(true)
            }
        }
    }

    async fn get(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        let _guard = self.kvs_lock.read().await;
        Ok(self
            .kvs
            .read_async(&Self::composite_key(&namespace, key), |_, value| {
                Bytes::from(value.clone())
            })
            .await)
    }

    async fn get_many(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        // Single read lock for the whole batch so the returned values are a consistent snapshot.
        let _guard = self.kvs_lock.read().await;
        let mut result = Vec::new();
        for key in keys {
            result.push(
                self.kvs
                    .read_async(&Self::composite_key(&namespace, &key), |_, value| {
                        Bytes::from(value.clone())
                    })
                    .await,
            );
        }
        Ok(result)
    }

    async fn get_all(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, String> {
        // Single read lock for the whole scan so the returned pairs are a consistent snapshot.
        let _guard = self.kvs_lock.read().await;
        let prefix = Self::composite_key(&namespace, "");
        let mut result = Vec::new();
        self.kvs
            .iter_async(|key, value| {
                if key.starts_with(&prefix) {
                    result.push((key[prefix.len()..].to_string(), Bytes::from(value.clone())));
                }
                true
            })
            .await;
        Ok(result)
    }

    async fn del(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let _guard = self.kvs_lock.write().await;
        self.kvs
            .remove_async(&Self::composite_key(&namespace, key))
            .await;
        Ok(())
    }

    async fn del_many(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        let _guard = self.kvs_lock.write().await;
        for key in keys {
            self.kvs
                .remove_async(&Self::composite_key(&namespace, &key))
                .await;
        }
        Ok(())
    }

    async fn exists(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        let _guard = self.kvs_lock.read().await;
        Ok(self
            .kvs
            .contains_async(&Self::composite_key(&namespace, key))
            .await)
    }

    async fn keys(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        let _guard = self.kvs_lock.read().await;
        let prefix = Self::composite_key(&namespace, "");
        let mut result = Vec::new();
        self.kvs
            .iter_async(|key, _| {
                if key.starts_with(&prefix) {
                    result.push(key[prefix.len()..].to_string());
                }
                true
            })
            .await;
        Ok(result)
    }

    async fn add_to_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let set = self
            .sets
            .entry_async(Self::composite_key(&namespace, key))
            .await
            .or_default();
        let _ = set.replace_async(value.to_vec()).await;
        Ok(())
    }

    async fn remove_from_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        match self
            .sets
            .entry_async(Self::composite_key(&namespace, key))
            .await
        {
            Entry::Occupied(mut entry) => {
                entry.get_mut().remove_async(value).await;
            }
            Entry::Vacant(_) => {}
        }
        Ok(())
    }

    async fn members_of_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        match self
            .sets
            .get_async(&Self::composite_key(&namespace, key))
            .await
        {
            Some(entry) => {
                let mut result = Vec::new();
                entry
                    .iter_async(|v| {
                        result.push(Bytes::from(v.clone()));
                        true
                    })
                    .await;
                Ok(result)
            }
            None => Ok(Vec::new()),
        }
    }

    async fn add_to_sorted_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), String> {
        let mut entry = self
            .sorted_sets
            .entry_async(Self::composite_key(&namespace, key))
            .await
            .or_default();
        entry.retain(|(_, v)| v != value);
        entry.push((score, value.to_vec()));
        entry.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        Ok(())
    }

    async fn remove_from_sorted_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let mut entry = self
            .sorted_sets
            .entry_async(Self::composite_key(&namespace, key))
            .await
            .or_default();
        entry.retain(|(_, v)| v != value);
        entry.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        Ok(())
    }

    async fn get_sorted_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        Ok(self
            .sorted_sets
            .read_async(&Self::composite_key(&namespace, key), |_, entry| {
                entry
                    .iter()
                    .map(|(score, value)| (*score, Bytes::from(value.clone())))
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default())
    }

    async fn query_sorted_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        Ok(self
            .sorted_sets
            .read_async(&Self::composite_key(&namespace, key), |_, entry| {
                entry
                    .iter()
                    .filter(|(score, _)| *score >= min && *score <= max)
                    .map(|(score, value)| (*score, Bytes::from(value.clone())))
                    .collect()
            })
            .await
            .unwrap_or_default())
    }
}
