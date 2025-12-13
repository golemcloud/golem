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

use crate::storage::keyvalue::sqlite::SqliteKeyValueStorage;
use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::config::DbSqliteConfig;
use golem_common::model::WorkerId;
use golem_service_base::db::sqlite::SqlitePool;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// KeyValueStorage implementation that uses multiple separate SQLite databases depending
/// on the namespace.
pub struct MultiSqliteKeyValueStorage {
    cache: Cache<String, (), SqliteKeyValueStorage, String>,
    hash_cache: Arc<Mutex<HashCache>>,
    root_dir: PathBuf,
    max_connections: u32,
    foreign_keys: bool,
}

struct HashCache {
    hash_per_worker_id: HashMap<WorkerId, String>,
    worker_id_per_hash: HashMap<String, WorkerId>,
}

impl MultiSqliteKeyValueStorage {
    pub fn new(root_dir: &Path, max_connections: u32, foreign_keys: bool) -> Self {
        if !root_dir.exists() {
            std::fs::create_dir_all(root_dir)
                .expect("Failed to create root directory for sqlite storage");
        }
        Self {
            cache: Cache::new(
                Some(1024),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: Duration::from_secs(21600),
                    period: Duration::from_secs(60),
                },
                "multi-sqlite-kvs",
            ),
            hash_cache: Arc::new(Mutex::new(HashCache {
                hash_per_worker_id: HashMap::new(),
                worker_id_per_hash: HashMap::new(),
            })),
            root_dir: root_dir.to_path_buf(),
            max_connections,
            foreign_keys,
        }
    }

    async fn init_storage(
        max_connections: u32,
        foreign_keys: bool,
        database: String,
    ) -> Result<SqliteKeyValueStorage, String> {
        let config = DbSqliteConfig {
            database,
            max_connections,
            foreign_keys,
        };
        let pool = SqlitePool::configured(&config)
            .await
            .map_err(|e| format!("Failed to initialize sqlite database: {:?}", e))?;
        SqliteKeyValueStorage::new(pool).await
    }

    async fn storage_by_namespace(
        &self,
        namespace: &KeyValueStorageNamespace,
    ) -> Result<SqliteKeyValueStorage, String> {
        let db = self.namespace_to_db(namespace).await;
        let max_connections = self.max_connections;
        let foreign_keys = self.foreign_keys;
        let db_path = self.root_dir.join(db.clone()).to_string_lossy().to_string();
        self.cache
            .get_or_insert_simple(&db, async move || {
                Self::init_storage(max_connections, foreign_keys, db_path).await
            })
            .await
    }

    async fn namespace_to_db(&self, namespace: &KeyValueStorageNamespace) -> String {
        match namespace {
            KeyValueStorageNamespace::RunningWorkers => "kv-running_workers".to_string(),
            KeyValueStorageNamespace::Worker { worker_id } => {
                format!("kv-worker-{}.db", self.worker_id_hash(worker_id).await)
            }
            KeyValueStorageNamespace::Promise { worker_id } => {
                format!("kv-worker-{}.db", self.worker_id_hash(worker_id).await)
            }
            KeyValueStorageNamespace::Schedule => "kv-schedule.db".to_string(),
            KeyValueStorageNamespace::UserDefined { .. } => "kv-user-defined.db".to_string(),
        }
    }

    async fn worker_id_hash(&self, worker_id: &WorkerId) -> String {
        let mut hash_cache = self.hash_cache.lock().await;
        match hash_cache.hash_per_worker_id.get(worker_id) {
            Some(hash) => hash.clone(),
            None => {
                let hash = format!("{}", blake3::hash(worker_id.to_string().as_bytes()));
                hash_cache
                    .hash_per_worker_id
                    .insert(worker_id.clone(), hash.clone());
                hash_cache
                    .worker_id_per_hash
                    .insert(hash.clone(), worker_id.clone());
                hash
            }
        }
    }
}

impl Debug for MultiSqliteKeyValueStorage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "MultiSqliteKeyValueStorage")
    }
}

#[async_trait]
impl KeyValueStorage for MultiSqliteKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
            .exists(svc_name, api_name, namespace, key)
            .await
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
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
        self.storage_by_namespace(&namespace)
            .await?
            .query_sorted_set(svc_name, api_name, entity_name, namespace, key, min, max)
            .await
    }
}
