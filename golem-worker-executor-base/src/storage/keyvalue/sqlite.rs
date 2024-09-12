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

use crate::storage::{
    keyvalue::{KeyValueStorage, KeyValueStorageNamespace},
    sqlite_types::SqlitePool,
};
use async_trait::async_trait;
use bytes::Bytes;
use std::fmt;

#[derive(Debug)]
pub struct SqliteKeyValueStorage {
    pool: SqlitePool,
}

impl SqliteKeyValueStorage {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    fn to_string<T: fmt::Debug>(t: &T) -> String {
        format!("{t:?}")
    }
}

#[async_trait]
impl KeyValueStorage for SqliteKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .set(key, value, &Self::to_string(&namespace))
            .await
            .map_err(|e| e.to_string())
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .set_many(&Self::to_string(&namespace), pairs)
            .await
            .map_err(|e| e.to_string())
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String> {
        self.pool
            .with(svc_name, api_name)
            .set_if_not_exists(&Self::to_string(&namespace), key, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        self.pool
            .with(svc_name, api_name)
            .get(&Self::to_string(&namespace), key)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        self.pool
            .with(svc_name, api_name)
            .get_many(&Self::to_string(&namespace), keys)
            .await
            .map_err(|e| e.to_string())
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .del(&Self::to_string(&namespace), key)
            .await
            .map_err(|e| e.to_string())
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .del_many(&Self::to_string(&namespace), keys)
            .await
            .map_err(|e| e.to_string())
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.pool
            .with(svc_name, api_name)
            .exists(&Self::to_string(&namespace), key)
            .await
            .map_err(|e| e.to_string())
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        self.pool
            .with(svc_name, api_name)
            .keys(&Self::to_string(&namespace))
            .await
            .map_err(|e| e.to_string())
    }

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .add_to_set(&Self::to_string(&namespace), key, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .remove_from_set(&Self::to_string(&namespace), key, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        self.pool
            .with(svc_name, api_name)
            .members_of_set(&Self::to_string(&namespace), key)
            .await
            .map_err(|e| e.to_string())
    }

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .add_to_sorted_set(&Self::to_string(&namespace), key, score, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.pool
            .with(svc_name, api_name)
            .remove_from_sorted_set(&Self::to_string(&namespace), key, value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        self.pool
            .with(svc_name, api_name)
            .get_sorted_set(&Self::to_string(&namespace), key)
            .await
            .map_err(|e| e.to_string())
    }

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        self.pool
            .with(svc_name, api_name)
            .query_sorted_set(&Self::to_string(&namespace), key, min, max)
            .await
            .map_err(|e| e.to_string())
    }
}
