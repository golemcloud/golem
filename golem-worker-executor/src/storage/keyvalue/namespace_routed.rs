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

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct NamespaceRoutedKeyValueStorage {
    cache: Arc<dyn KeyValueStorage + Send + Sync>,
    persistent: Arc<dyn KeyValueStorage + Send + Sync>,
}

impl NamespaceRoutedKeyValueStorage {
    pub fn new(
        cache: Arc<dyn KeyValueStorage + Send + Sync>,
        persistent: Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Self {
        Self { cache, persistent }
    }

    fn backend_for_namespace(
        &self,
        namespace: &KeyValueStorageNamespace,
    ) -> &Arc<dyn KeyValueStorage + Send + Sync> {
        match namespace.expiry() {
            Some(_) => &self.cache,
            None => &self.persistent,
        }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl KeyValueStorage for NamespaceRoutedKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        let backend = self.backend_for_namespace(&namespace);
        if Arc::ptr_eq(backend, &self.cache) {
            crate::metrics::workers::record_worker_kv_cache_value_size(value.len());
        }
        backend
            .set(svc_name, api_name, entity_name, namespace, key, value)
            .await
    }

    async fn set_with_expiry(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
        expiry: Duration,
    ) -> Result<(), KeyValueStorageError> {
        if namespace.expiry().is_none() {
            return Err(KeyValueStorageError::Other(
                "expiry is only supported for cache-routed namespaces".to_string(),
            ));
        }
        let backend = self.backend_for_namespace(&namespace);
        crate::metrics::workers::record_worker_kv_cache_value_size(value.len());
        backend
            .set_with_expiry(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                value,
                expiry,
            )
            .await
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), KeyValueStorageError> {
        let backend = self.backend_for_namespace(&namespace);
        if Arc::ptr_eq(backend, &self.cache) {
            for (_, value) in pairs {
                crate::metrics::workers::record_worker_kv_cache_value_size(value.len());
            }
        }
        backend
            .set_many(svc_name, api_name, entity_name, namespace, pairs)
            .await
    }

    async fn compare_and_set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        expected: Option<&[u8]>,
        deletes: &[&str],
        pairs: &[(&str, &[u8])],
    ) -> Result<bool, KeyValueStorageError> {
        let backend = self.backend_for_namespace(&namespace);
        if Arc::ptr_eq(backend, &self.cache) {
            for (_, value) in pairs {
                crate::metrics::workers::record_worker_kv_cache_value_size(value.len());
            }
        }
        backend
            .compare_and_set_many(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                expected,
                deletes,
                pairs,
            )
            .await
    }

    async fn compare_and_mutate_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        expected: Option<&[u8]>,
        sets: &[(&str, &[u8])],
        deletions: &[&str],
        expiry: Duration,
    ) -> Result<bool, KeyValueStorageError> {
        if namespace.expiry().is_none() {
            return Err(KeyValueStorageError::Other(
                "expiry is only supported for cache-routed namespaces".to_string(),
            ));
        }
        let backend = self.backend_for_namespace(&namespace);
        for (_, value) in sets {
            crate::metrics::workers::record_worker_kv_cache_value_size(value.len());
        }
        backend
            .compare_and_mutate_many(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                expected,
                sets,
                deletions,
                expiry,
            )
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
    ) -> Result<bool, KeyValueStorageError> {
        let backend = self.backend_for_namespace(&namespace);
        if Arc::ptr_eq(backend, &self.cache) {
            crate::metrics::workers::record_worker_kv_cache_value_size(value.len());
        }
        backend
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
    ) -> Result<Option<Bytes>, KeyValueStorageError> {
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
        keys: Arc<[String]>,
    ) -> Result<Vec<Option<Bytes>>, KeyValueStorageError> {
        self.backend_for_namespace(&namespace)
            .get_many(svc_name, api_name, entity_name, namespace, keys)
            .await
    }

    async fn get_all(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, KeyValueStorageError> {
        self.backend_for_namespace(&namespace)
            .get_all(svc_name, api_name, entity_name, namespace)
            .await
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), KeyValueStorageError> {
        self.backend_for_namespace(&namespace)
            .del(svc_name, api_name, namespace, key)
            .await
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Arc<[String]>,
    ) -> Result<(), KeyValueStorageError> {
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
    ) -> Result<bool, KeyValueStorageError> {
        self.backend_for_namespace(&namespace)
            .exists(svc_name, api_name, namespace, key)
            .await
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, KeyValueStorageError> {
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
    ) -> Result<(), KeyValueStorageError> {
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
    ) -> Result<(), KeyValueStorageError> {
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
    ) -> Result<Vec<Bytes>, KeyValueStorageError> {
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
    ) -> Result<(), KeyValueStorageError> {
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
    ) -> Result<(), KeyValueStorageError> {
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
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
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
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
        self.backend_for_namespace(&namespace)
            .query_sorted_set(svc_name, api_name, entity_name, namespace, key, min, max)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::AgentId;
    use test_r::test;

    #[test]
    fn snapshot_rejections_use_persistent_storage_not_status_cache() {
        let storage = NamespaceRoutedKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            Arc::new(InMemoryKeyValueStorage::new()),
        );
        let agent_id = golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId(uuid::Uuid::from_u128(1)),
            &AgentId::from_parts(7, 1),
        );
        assert!(Arc::ptr_eq(
            storage.backend_for_namespace(
                &KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
                    owned_agent_id: agent_id.clone(),
                }
            ),
            &storage.persistent,
        ));
        assert!(Arc::ptr_eq(
            storage.backend_for_namespace(&KeyValueStorageNamespace::AgentStatus {
                owned_agent_id: agent_id,
                fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::nil()),
            }),
            &storage.cache,
        ));
    }

    #[test]
    fn the_status_baseline_uses_persistent_storage() {
        let storage = NamespaceRoutedKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            Arc::new(InMemoryKeyValueStorage::new()),
        );
        let agent_id = golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId(uuid::Uuid::from_u128(1)),
            &AgentId::from_parts(7, 1),
        );
        assert!(Arc::ptr_eq(
            storage.backend_for_namespace(&KeyValueStorageNamespace::AgentStatusCheckpoint {
                owned_agent_id: agent_id,
                fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::nil()),
            }),
            &storage.persistent,
        ));
    }

    /// A write with an expiry to a namespace that has no expiry fails, also when the namespace
    /// is the status baseline. A write with an expiry to a derived cache succeeds.
    #[test]
    async fn an_expiry_is_refused_for_a_namespace_with_no_expiry() {
        let storage = NamespaceRoutedKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            Arc::new(InMemoryKeyValueStorage::new()),
        );
        let owned_agent_id = golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId(uuid::Uuid::from_u128(1)),
            &AgentId::from_parts(7, 2),
        );
        let fingerprint = golem_common::model::AgentFingerprint(uuid::Uuid::nil());
        let baseline = KeyValueStorageNamespace::AgentStatusCheckpoint {
            owned_agent_id: owned_agent_id.clone(),
            fingerprint,
        };
        let cache = KeyValueStorageNamespace::AgentStatus {
            owned_agent_id,
            fingerprint,
        };
        let expiry = Duration::from_secs(60);
        let sets: [(&str, &[u8]); 1] = [("core", b"v")];
        let set = |namespace| {
            storage.set_with_expiry("test", "set", "entity", namespace, "core", b"v", expiry)
        };
        let mutate = |namespace| {
            storage.compare_and_mutate_many(
                "test",
                "mutate",
                "entity",
                namespace,
                "core",
                None,
                &sets,
                &[],
                expiry,
            )
        };

        assert!(set(baseline.clone()).await.is_err());
        assert!(mutate(baseline).await.is_err());
        assert!(set(cache.clone()).await.is_ok());
        assert!(mutate(cache).await.is_ok());
    }
}
