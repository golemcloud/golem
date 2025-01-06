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

pub mod memory;
pub mod redis;
pub mod sqlite;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use golem_common::model::AccountId;
use golem_common::serialization::{deserialize, serialize};
use std::fmt::Debug;

#[async_trait]
pub trait KeyValueStorage: Debug {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String>;

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String>;

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String>;

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String>;

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String>;

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String>;

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String>;

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String>;

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String>;

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String>;

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String>;

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String>;

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), String>;

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String>;

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String>;

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, String>;
}

pub trait KeyValueStorageLabelledApi<T: KeyValueStorage + ?Sized> {
    fn with(&self, svc_name: &'static str, api_name: &'static str) -> LabelledKeyValueStorage<T>;

    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityKeyValueStorage<T>;
}

impl<T: ?Sized + KeyValueStorage> KeyValueStorageLabelledApi<T> for T {
    fn with(&self, svc_name: &'static str, api_name: &'static str) -> LabelledKeyValueStorage<T> {
        LabelledKeyValueStorage::new(svc_name, api_name, self)
    }
    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityKeyValueStorage<T> {
        LabelledEntityKeyValueStorage::new(svc_name, api_name, entity_name, self)
    }
}

pub struct LabelledKeyValueStorage<'a, S: KeyValueStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + KeyValueStorage> LabelledKeyValueStorage<'a, S> {
    pub fn new(svc_name: &'static str, api_name: &'static str, storage: &'a S) -> Self {
        Self {
            svc_name,
            api_name,
            storage,
        }
    }

    pub async fn del(&self, namespace: KeyValueStorageNamespace, key: &str) -> Result<(), String> {
        self.storage
            .del(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn del_many(
        &self,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        self.storage
            .del_many(self.svc_name, self.api_name, namespace, keys)
            .await
    }

    pub async fn exists(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn keys(&self, namespace: KeyValueStorageNamespace) -> Result<Vec<String>, String> {
        self.storage
            .keys(self.svc_name, self.api_name, namespace)
            .await
    }
}

pub struct LabelledEntityKeyValueStorage<'a, S: KeyValueStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    entity_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + KeyValueStorage> LabelledEntityKeyValueStorage<'a, S> {
    pub fn new(
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        storage: &'a S,
    ) -> Self {
        Self {
            svc_name,
            api_name,
            entity_name,
            storage,
        }
    }

    pub async fn set<V: Encode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;

        self.storage
            .set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
    }

    pub async fn set_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.storage
            .set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                value,
            )
            .await
    }

    pub async fn set_if_not_exists<V: Encode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<bool, String> {
        let serialized = serialize(value)?;
        self.storage
            .set_if_not_exists(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
    }

    pub async fn set_many<V: Encode>(
        &self,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &V)],
    ) -> Result<(), String> {
        let pairs = pairs
            .iter()
            .map(|(k, v)| serialize(v).map(|v| (k.to_string(), v.to_vec())))
            .collect::<Result<Vec<_>, String>>()?;
        let pairs_refs: Vec<(&str, &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        self.storage
            .set_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                &pairs_refs,
            )
            .await
    }

    pub async fn set_many_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        self.storage
            .set_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                pairs,
            )
            .await
    }

    pub async fn get<V: Decode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<V>, String> {
        let maybe_bytes = self
            .storage
            .get(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?;
        if let Some(bytes) = maybe_bytes {
            let value: V = deserialize(&bytes)?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    pub async fn get_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        self.storage
            .get(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await
    }

    pub async fn get_many<V: Decode>(
        &self,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<V>>, String> {
        let maybe_bytes = self
            .storage
            .get_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                keys,
            )
            .await?;
        let mut values = Vec::new();
        for maybe_bytes in maybe_bytes {
            if let Some(bytes) = maybe_bytes {
                let value: V = deserialize(&bytes)?;
                values.push(Some(value));
            } else {
                values.push(None);
            }
        }
        Ok(values)
    }

    pub async fn get_many_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        self.storage
            .get_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                keys,
            )
            .await
    }

    pub async fn add_to_set<V: Encode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .add_to_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
    }

    pub async fn remove_from_set<V: Encode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .remove_from_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
    }

    pub async fn members_of_set<V: Decode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<V>, String> {
        let maybe_bytes = self
            .storage
            .members_of_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?;
        let mut values = Vec::new();
        for bytes in maybe_bytes {
            let value: V = deserialize(&bytes)?;
            values.push(value);
        }
        Ok(values)
    }

    pub async fn add_to_sorted_set<V: Encode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .add_to_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                score,
                &serialized,
            )
            .await
    }

    pub async fn remove_from_sorted_set<V: Encode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .remove_from_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
    }

    pub async fn get_sorted_set<V: Decode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, V)>, String> {
        let maybe_bytes = self
            .storage
            .get_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?;
        let mut values = Vec::new();
        for (score, bytes) in maybe_bytes {
            let value: V = deserialize(&bytes)?;
            values.push((score, value));
        }
        Ok(values)
    }

    pub async fn query_sorted_set<V: Decode>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, V)>, String> {
        let maybe_bytes = self
            .storage
            .query_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                min,
                max,
            )
            .await?;
        let mut values = Vec::new();
        for (score, bytes) in maybe_bytes {
            let value: V = deserialize(&bytes)?;
            values.push((score, value));
        }
        Ok(values)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum KeyValueStorageNamespace {
    Worker,
    Promise,
    Schedule,
    UserDefined {
        account_id: AccountId,
        bucket: String,
    },
}
