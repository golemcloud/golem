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

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::mapref::entry::Entry;
use dashmap::{DashMap, DashSet};

#[derive(Debug)]
pub struct InMemoryKeyValueStorage {
    kvs: DashMap<String, Vec<u8>>,
    sets: DashMap<String, DashSet<Vec<u8>>>,
    sorted_sets: DashMap<String, Vec<(f64, Vec<u8>)>>,
}

impl Default for InMemoryKeyValueStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryKeyValueStorage {
    pub fn new() -> Self {
        Self {
            kvs: DashMap::new(),
            sets: DashMap::new(),
            sorted_sets: DashMap::new(),
        }
    }

    pub fn kvs(&self) -> &DashMap<String, Vec<u8>> {
        &self.kvs
    }

    pub fn sets(&self) -> &DashMap<String, DashSet<Vec<u8>>> {
        &self.sets
    }

    pub fn sorted_sets(&self) -> &DashMap<String, Vec<(f64, Vec<u8>)>> {
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
        self.kvs
            .insert(Self::composite_key(&namespace, key), value.to_vec());
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
        for (key, value) in pairs {
            self.kvs
                .insert(Self::composite_key(&namespace, key), value.to_vec());
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
        match self.kvs.entry(Self::composite_key(&namespace, key)) {
            Entry::Occupied(_) => Ok(false),
            Entry::Vacant(entry) => {
                entry.insert(value.to_vec());
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
        match self.kvs.get(&Self::composite_key(&namespace, key)) {
            Some(value) => Ok(Some(Bytes::from(value.value().clone()))),
            None => Ok(None),
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
        let mut result = Vec::new();
        for key in keys {
            result.push(
                self.get(svc_name, api_name, entity_name, namespace.clone(), &key)
                    .await?,
            );
        }
        Ok(result)
    }

    async fn del(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        self.kvs.remove(&Self::composite_key(&namespace, key));
        Ok(())
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        for key in keys {
            self.del(svc_name, api_name, namespace.clone(), &key)
                .await?;
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
        Ok(self.kvs.contains_key(&Self::composite_key(&namespace, key)))
    }

    async fn keys(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        let prefix = Self::composite_key(&namespace, "");
        Ok(self
            .kvs
            .iter()
            .filter_map(|item| {
                if item.key().starts_with(&prefix) {
                    Some(item.key()[prefix.len()..].to_string())
                } else {
                    None
                }
            })
            .collect())
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
            .entry(Self::composite_key(&namespace, key))
            .or_default();
        set.insert(value.to_vec());
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
        match self.sets.get_mut(&Self::composite_key(&namespace, key)) {
            Some(mut entry) => {
                entry.value_mut().remove(value);
                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn members_of_set(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        match self.sets.get(&Self::composite_key(&namespace, key)) {
            Some(entry) => Ok(entry
                .value()
                .iter()
                .map(|v| Bytes::from(v.clone()))
                .collect()),
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
            .entry(Self::composite_key(&namespace, key))
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
            .entry(Self::composite_key(&namespace, key))
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
        match self.sorted_sets.get(&Self::composite_key(&namespace, key)) {
            Some(entry) => Ok(entry
                .iter()
                .map(|(score, value)| (*score, Bytes::from(value.clone())))
                .collect()),
            None => Ok(Vec::new()),
        }
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
        match self.sorted_sets.get(&Self::composite_key(&namespace, key)) {
            Some(entry) => Ok(entry
                .iter()
                .filter(|(score, _)| *score >= min && *score <= max)
                .map(|(score, value)| (*score, Bytes::from(value.clone())))
                .collect()),
            None => Ok(Vec::new()),
        }
    }
}
