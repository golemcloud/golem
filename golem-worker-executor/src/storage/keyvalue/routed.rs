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
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RoutedKeyValueStorage {
    redis: Arc<dyn KeyValueStorage + Send + Sync>,
    postgres: Arc<dyn KeyValueStorage + Send + Sync>,
}

impl RoutedKeyValueStorage {
    pub fn new(
        redis: Arc<dyn KeyValueStorage + Send + Sync>,
        postgres: Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Self {
        Self { redis, postgres }
    }

    fn backend_for_namespace(
        &self,
        namespace: &KeyValueStorageNamespace,
    ) -> &Arc<dyn KeyValueStorage + Send + Sync> {
        match namespace {
            KeyValueStorageNamespace::Worker { .. } => &self.redis,
            _ => &self.postgres,
        }
    }
}

#[async_trait]
impl KeyValueStorage for RoutedKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.backend_for_namespace(&namespace)
            .set(svc_name, api_name, entity_name, namespace, key, value)
            .await
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        self.backend_for_namespace(&namespace)
            .set_many(svc_name, api_name, entity_name, namespace, pairs)
            .await
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
        self.backend_for_namespace(&namespace)
            .set_if_not_exists(svc_name, api_name, entity_name, namespace, key, value)
            .await
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        self.backend_for_namespace(&namespace)
            .get(svc_name, api_name, entity_name, namespace, key)
            .await
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        self.backend_for_namespace(&namespace)
            .get_many(svc_name, api_name, entity_name, namespace, keys)
            .await
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        self.backend_for_namespace(&namespace)
            .del(svc_name, api_name, namespace, key)
            .await
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        self.backend_for_namespace(&namespace)
            .del_many(svc_name, api_name, namespace, keys)
            .await
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.backend_for_namespace(&namespace)
            .exists(svc_name, api_name, namespace, key)
            .await
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        self.backend_for_namespace(&namespace)
            .keys(svc_name, api_name, namespace)
            .await
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
        self.backend_for_namespace(&namespace)
            .add_to_set(svc_name, api_name, entity_name, namespace, key, value)
            .await
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
        self.backend_for_namespace(&namespace)
            .remove_from_set(svc_name, api_name, entity_name, namespace, key, value)
            .await
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        self.backend_for_namespace(&namespace)
            .members_of_set(svc_name, api_name, entity_name, namespace, key)
            .await
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
        self.backend_for_namespace(&namespace)
            .add_to_sorted_set(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                score,
                value,
            )
            .await
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
        self.backend_for_namespace(&namespace)
            .remove_from_sorted_set(svc_name, api_name, entity_name, namespace, key, value)
            .await
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        self.backend_for_namespace(&namespace)
            .get_sorted_set(svc_name, api_name, entity_name, namespace, key)
            .await
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
        self.backend_for_namespace(&namespace)
            .query_sorted_set(svc_name, api_name, entity_name, namespace, key, min, max)
            .await
    }
}
