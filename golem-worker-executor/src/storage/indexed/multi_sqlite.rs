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

use super::{
    IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace, IndexedStorageNamespace,
    ScanCursor, ScanResume,
};
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::config::DbSqliteConfig;
use golem_common::model::AgentId;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// IndexedStorage implementation that uses multiple separate SQLite databases depending
/// on the namespace.
pub struct MultiSqliteIndexedStorage {
    cache: Cache<String, (), SqliteIndexedStorage, IndexedStorageError>,
    hash_cache: Arc<Mutex<HashCache>>,
    root_dir: PathBuf,
    max_connections: u32,
    foreign_keys: bool,
}

struct HashCache {
    hash_per_agent_id: HashMap<AgentId, String>,
    agent_id_per_hash: HashMap<String, AgentId>,
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
                hash_per_agent_id: HashMap::new(),
                agent_id_per_hash: HashMap::new(),
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
    ) -> Result<SqliteIndexedStorage, IndexedStorageError> {
        let config = DbSqliteConfig {
            database,
            max_connections,
            foreign_keys,
        };
        SqliteIndexedStorage::configured(&config)
            .await
            .map_err(IndexedStorageError::Other)
    }

    async fn storage_by_namespace(
        &self,
        namespace: &IndexedStorageNamespace,
    ) -> Result<SqliteIndexedStorage, IndexedStorageError> {
        let db = self.namespace_to_db(namespace).await;
        self.storage_by_db_name(db).await
    }

    /// The `.db` files a namespace is spread over, in a stable order.
    fn namespace_db_files(
        &self,
        namespace: &IndexedStorageMetaNamespace,
    ) -> Result<Vec<String>, IndexedStorageError> {
        use std::fs;

        let db_prefix = match namespace {
            IndexedStorageMetaNamespace::Oplog { agent_mode } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!("{mode}-oplog-")
            }
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!("{mode}-compressed-oplog-l{}-", level)
            }
        };

        let mut matching_files: Vec<_> = fs::read_dir(&self.root_dir)
            .map_err(|e| {
                IndexedStorageError::Other(format!("Failed to read root directory: {:?}", e))
            })?
            .filter_map(|entry| {
                entry.ok().and_then(|e| {
                    let path = e.path();
                    let file_name = path.file_name()?.to_string_lossy().to_string();
                    if file_name.starts_with(&db_prefix) && file_name.ends_with(".db") {
                        Some(file_name)
                    } else {
                        None
                    }
                })
            })
            .collect();
        matching_files.sort();
        Ok(matching_files)
    }

    async fn storage_by_db_name(
        &self,
        db: String,
    ) -> Result<SqliteIndexedStorage, IndexedStorageError> {
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
            IndexedStorageNamespace::OpLog {
                agent_id,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!("{mode}-oplog-{}.db", self.agent_id_hash(agent_id).await)
            }
            IndexedStorageNamespace::CompressedOpLog {
                agent_id,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!(
                    "{mode}-compressed-oplog-l{}-{}.db",
                    level,
                    self.agent_id_hash(agent_id).await
                )
            }
        }
    }

    async fn agent_id_hash(&self, agent_id: &AgentId) -> String {
        let mut hash_cache = self.hash_cache.lock().await;
        match hash_cache.hash_per_agent_id.get(agent_id) {
            Some(hash) => hash.clone(),
            None => {
                let hash = format!("{}", blake3::hash(agent_id.to_string().as_bytes()));
                hash_cache
                    .hash_per_agent_id
                    .insert(agent_id.clone(), hash.clone());
                hash_cache
                    .agent_id_per_hash
                    .insert(hash.clone(), agent_id.clone());
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
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn wait_for_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
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
        prefix: Option<&str>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), IndexedStorageError> {
        let matching_files = self.namespace_db_files(&namespace)?;

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
                    prefix,
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

    async fn scan_stable(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<ScanResume>,
        count: u64,
    ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError> {
        // A meta-namespace spans one file per namespace under it, and key order does not follow
        // file order, so this walks the files rather than the keys: the token names the last file
        // finished, and a file is always taken whole. Merging a page from every file instead would
        // read everything the meta-namespace holds to answer one page, which is both quadratic over
        // a pass and unbounded in memory.
        //
        // A file is never deleted, only emptied, which is what makes the token a seek: the name a
        // caller comes back with still sits where it did. It also means a drained meta-namespace is
        // a long row of empty files, so a call stops once it has opened `count` of them whether or
        // not it found anything. Otherwise one call would open every file the backend has ever
        // made, and the caller's page budget would bound round trips while bounding nothing here.
        let after = match resume {
            Some(ScanResume::Marker(file)) => Some(file),
            Some(ScanResume::Cursor(_)) => {
                return Err(IndexedStorageError::Other(
                    "Multi-SQLite indexed storage was handed a resume token it did not produce"
                        .to_string(),
                ));
            }
            None => None,
        };

        let mut keys = Vec::new();
        let mut last_file = None;
        let mut opened = 0;
        for file_name in self.namespace_db_files(&namespace)? {
            if after
                .as_deref()
                .is_some_and(|after| file_name.as_str() <= after)
            {
                continue;
            }
            if opened >= count.max(1) {
                break;
            }
            opened += 1;
            let storage = self.storage_by_db_name(file_name.clone()).await?;

            // Whole file, however many pages that takes. A file holds one namespace, so this is
            // bounded by what that namespace holds rather than by the meta-namespace.
            let mut within = None;
            loop {
                let (next, page) = storage
                    .scan_stable(
                        svc_name,
                        api_name,
                        namespace.clone(),
                        prefix,
                        within,
                        count.max(1),
                    )
                    .await?;
                keys.extend(page);
                match next {
                    Some(next) => within = Some(next),
                    None => break,
                }
            }

            last_file = Some(file_name);
            if keys.len() as u64 >= count {
                break;
            }
        }

        // The token is the last file finished, not the last key, so it cannot go through
        // `last_key_resume`. Exhaustion is reaching the end of the file list, which is why a short
        // page does not end the walk here: an empty page only means the files it opened were empty.
        let exhausted = opened < count.max(1) && (keys.len() as u64) < count;
        let next = match last_file {
            Some(file) if !exhausted => Some(ScanResume::Marker(file)),
            _ => None,
        };
        Ok((next, keys))
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
    ) -> Result<(), IndexedStorageError> {
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
    ) -> Result<u64, IndexedStorageError> {
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
    ) -> Result<(), IndexedStorageError> {
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
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
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
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
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
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        self.storage_by_namespace(&namespace)
            .await?
            .last(svc_name, api_name, entity_name, namespace, key)
            .await
    }

    async fn last_id(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        self.storage_by_namespace(&namespace)
            .await?
            .last_id(svc_name, api_name, entity_name, namespace, key)
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
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
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
    ) -> Result<(), IndexedStorageError> {
        self.storage_by_namespace(&namespace)
            .await?
            .drop_prefix(svc_name, api_name, namespace, key, last_dropped_id)
            .await
    }
}
