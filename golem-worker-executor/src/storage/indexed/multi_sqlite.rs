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

use super::{IndexedStorage, IndexedStorageMetaNamespace, IndexedStorageNamespace, ScanCursor};
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use async_trait::async_trait;
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

/// IndexedStorage implementation that uses multiple separate SQLite databases depending
/// on the namespace.
pub struct MultiSqliteIndexedStorage {
    cache: Cache<String, (), SqliteIndexedStorage, String>,
    hash_cache: Arc<Mutex<HashCache>>,
    root_dir: PathBuf,
    max_connections: u32,
    foreign_keys: bool,
}

struct HashCache {
    hash_per_worker_id: HashMap<WorkerId, String>,
    worker_id_per_hash: HashMap<String, WorkerId>,
}

impl MultiSqliteIndexedStorage {
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
                "multi-sqlite-indexed",
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
    ) -> Result<SqliteIndexedStorage, String> {
        let config = DbSqliteConfig {
            database,
            max_connections,
            foreign_keys,
        };
        let pool = SqlitePool::configured(&config)
            .await
            .map_err(|e| format!("Failed to initialize sqlite database: {:?}", e))?;
        SqliteIndexedStorage::new(pool).await
    }

    async fn storage_by_namespace(
        &self,
        namespace: &IndexedStorageNamespace,
    ) -> Result<SqliteIndexedStorage, String> {
        let db = self.namespace_to_db(namespace).await;
        self.storage_by_db_name(db).await
    }

    async fn storage_by_db_name(&self, db: String) -> Result<SqliteIndexedStorage, String> {
        let max_connections = self.max_connections;
        let foreign_keys = self.foreign_keys;
        let db_path = self.root_dir.join(db.clone()).to_string_lossy().to_string();
        self.cache
            .get_or_insert_simple(&db, async move || {
                Self::init_storage(max_connections, foreign_keys, db_path).await
            })
            .await
    }

    async fn namespace_to_db(&self, namespace: &IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog { worker_id } => {
                format!("oplog-{}.db", self.worker_id_hash(worker_id).await)
            }
            IndexedStorageNamespace::CompressedOpLog { worker_id, level } => {
                format!(
                    "compressed-oplog-l{}-{}.db",
                    level,
                    self.worker_id_hash(worker_id).await
                )
            }
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

impl Debug for MultiSqliteIndexedStorage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "MultiSqliteIndexedStorage")
    }
}

#[async_trait]
impl IndexedStorage for MultiSqliteIndexedStorage {
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
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.storage_by_namespace(&namespace)
            .await?
            .exists(svc_name, api_name, namespace, key)
            .await
    }

    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        use std::fs;

        let prefix = match namespace {
            IndexedStorageMetaNamespace::Oplog => "oplog-".to_string(),
            IndexedStorageMetaNamespace::CompressedOplog { level } => {
                format!("compressed-oplog-l{}-", level)
            }
        };

        // List all .db files matching the namespace prefix, sorted consistently
        let mut matching_files: Vec<_> = fs::read_dir(&self.root_dir)
            .map_err(|e| format!("Failed to read root directory: {:?}", e))?
            .filter_map(|entry| {
                entry.ok().and_then(|e| {
                    let path = e.path();
                    let file_name = path.file_name()?.to_string_lossy().to_string();
                    if file_name.starts_with(&prefix) && file_name.ends_with(".db") {
                        Some(file_name)
                    } else {
                        None
                    }
                })
            })
            .collect();
        matching_files.sort();

        // Decode cursor: upper 32 bits = file index, lower 32 bits = scan cursor within file
        let file_index = (cursor >> 32) as usize;
        let file_cursor = cursor & 0xFFFFFFFF;

        let mut results = Vec::new();
        let mut current_file_cursor = file_cursor;

        for (idx, file_name) in matching_files.iter().enumerate().skip(file_index) {
            let storage = self.storage_by_db_name(file_name.clone()).await?;

            let (next_cursor, mut file_results) = storage
                .scan(
                    svc_name,
                    api_name,
                    namespace.clone(),
                    pattern,
                    current_file_cursor,
                    count - results.len() as u64,
                )
                .await?;

            results.append(&mut file_results);

            if results.len() as u64 >= count {
                // Encode next cursor: file index in upper 32 bits, file cursor in lower 32 bits
                let next_combined_cursor = ((idx as u64) << 32) | (next_cursor & 0xFFFFFFFF);
                return Ok((
                    next_combined_cursor,
                    results.into_iter().take(count as usize).collect(),
                ));
            }

            current_file_cursor = 0;
        }

        Ok((0, results))
    }

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
    ) -> Result<(), String> {
        self.storage_by_namespace(&namespace)
            .await?
            .append(svc_name, api_name, entity_name, namespace, key, id, value)
            .await
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String> {
        self.storage_by_namespace(&namespace)
            .await?
            .length(svc_name, api_name, namespace, key)
            .await
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        self.storage_by_namespace(&namespace)
            .await?
            .delete(svc_name, api_name, namespace, key)
            .await
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
    ) -> Result<Vec<(u64, Vec<u8>)>, String> {
        self.storage_by_namespace(&namespace)
            .await?
            .read(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                start_id,
                end_id,
            )
            .await
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        self.storage_by_namespace(&namespace)
            .await?
            .first(svc_name, api_name, entity_name, namespace, key)
            .await
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        self.storage_by_namespace(&namespace)
            .await?
            .last(svc_name, api_name, entity_name, namespace, key)
            .await
    }

    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        self.storage_by_namespace(&namespace)
            .await?
            .closest(svc_name, api_name, entity_name, namespace, key, id)
            .await
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), String> {
        self.storage_by_namespace(&namespace)
            .await?
            .drop_prefix(svc_name, api_name, namespace, key, last_dropped_id)
            .await
    }
}
