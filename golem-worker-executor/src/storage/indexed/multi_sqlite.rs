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
    ScanCursor, ScanResume, WriterId,
};
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::config::DbSqliteConfig;
use golem_common::model::AgentId;
use golem_common::model::ShardEpoch;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// How long a cached directory listing is served. A backstop only:
/// [`MultiSqliteIndexedStorage::storage_by_db_name`] clears the cache whenever it creates a file,
/// so this matters only for files created outside the process.
const LISTING_TTL: Duration = Duration::from_secs(10);

/// IndexedStorage implementation that uses multiple separate SQLite databases depending
/// on the namespace.
pub struct MultiSqliteIndexedStorage {
    cache: Cache<String, (), SqliteIndexedStorage, IndexedStorageError>,
    hash_cache: Arc<Mutex<HashCache>>,
    /// The `.db` files under each meta-namespace prefix, sorted, with the time they were read.
    /// See [`MultiSqliteIndexedStorage::namespace_db_files`].
    listing_cache: Arc<Mutex<HashMap<String, CachedListing>>>,
    root_dir: PathBuf,
    max_connections: u32,
    foreign_keys: bool,
    /// Handed to every per-namespace SQLite storage this opens, so the whole fan-out writes as one
    /// process. See [`WriterId`].
    writer_id: WriterId,
}

struct HashCache {
    hash_per_agent_id: HashMap<AgentId, String>,
    agent_id_per_hash: HashMap<String, AgentId>,
}

struct CachedListing {
    files: Arc<Vec<String>>,
    read_at: Instant,
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
            listing_cache: Arc::new(Mutex::new(HashMap::new())),
            root_dir: root_dir.to_path_buf(),
            max_connections,
            foreign_keys,
            writer_id: WriterId::process(),
        }
    }

    /// Writes as `writer_id` rather than as this process's own. The fan-out backend uses it to
    /// give every storage it opens one identity, and a test uses it to play two processes racing
    /// over one key inside a single process.
    pub fn for_writer(mut self, writer_id: WriterId) -> Self {
        self.writer_id = writer_id;
        self
    }

    async fn init_storage(
        max_connections: u32,
        foreign_keys: bool,
        database: String,
        writer_id: WriterId,
    ) -> Result<SqliteIndexedStorage, IndexedStorageError> {
        let config = DbSqliteConfig {
            database,
            max_connections,
            foreign_keys,
        };
        let storage = SqliteIndexedStorage::configured(&config)
            .await
            .map_err(IndexedStorageError::Other)?;
        Ok(storage.for_writer(writer_id))
    }

    async fn storage_by_namespace(
        &self,
        namespace: &IndexedStorageNamespace,
    ) -> Result<SqliteIndexedStorage, IndexedStorageError> {
        let db = self.namespace_to_db(namespace).await;
        self.storage_by_db_name(db).await
    }

    /// The filename prefix every `.db` file under a meta-namespace shares.
    fn db_prefix(namespace: &IndexedStorageMetaNamespace) -> String {
        match namespace {
            IndexedStorageMetaNamespace::Oplog { agent_mode } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!("{mode}-oplog-")
            }
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!("{mode}-compressed-oplog-l{}-", level)
            }
        }
    }

    /// The `.db` files a namespace is spread over, sorted.
    ///
    /// A walk asks for this once per page, and the directory holds a file per agent that ever had
    /// entries, so the listing is cached for [`LISTING_TTL`] and read on a blocking thread.
    async fn namespace_db_files(
        &self,
        namespace: &IndexedStorageMetaNamespace,
    ) -> Result<Arc<Vec<String>>, IndexedStorageError> {
        let db_prefix = Self::db_prefix(namespace);
        if let Some(cached) = self.listing_cache.lock().await.get(&db_prefix)
            && cached.read_at.elapsed() < LISTING_TTL
        {
            return Ok(cached.files.clone());
        }

        let root_dir = self.root_dir.clone();
        let prefix = db_prefix.clone();
        let files = tokio::task::spawn_blocking(move || Self::read_db_files(&root_dir, &prefix))
            .await
            .map_err(|e| {
                IndexedStorageError::Other(format!("Failed to list the root directory: {:?}", e))
            })??;

        let files = Arc::new(files);
        self.listing_cache.lock().await.insert(
            db_prefix,
            CachedListing {
                files: files.clone(),
                read_at: Instant::now(),
            },
        );
        Ok(files)
    }

    /// Blocking half of [`Self::namespace_db_files`].
    fn read_db_files(root_dir: &Path, db_prefix: &str) -> Result<Vec<String>, IndexedStorageError> {
        use std::fs;

        let mut matching_files: Vec<_> = fs::read_dir(root_dir)
            .map_err(|e| {
                IndexedStorageError::Other(format!("Failed to read root directory: {:?}", e))
            })?
            .filter_map(|entry| {
                entry.ok().and_then(|e| {
                    let path = e.path();
                    let file_name = path.file_name()?.to_string_lossy().to_string();
                    if file_name.starts_with(db_prefix) && file_name.ends_with(".db") {
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
        let writer_id = self.writer_id;
        let db_path = self.root_dir.join(db.clone()).to_string_lossy().to_string();
        // Set when this call creates the file, which makes cached listings stale. Checked only on a
        // cache miss, since a hit means the file is already open.
        let created = Arc::new(AtomicBool::new(false));
        let flag = created.clone();
        let existing = db_path.clone();
        let storage = self
            .cache
            .get_or_insert_simple(&db, async move || {
                flag.store(!Path::new(&existing).exists(), Ordering::SeqCst);
                Self::init_storage(max_connections, foreign_keys, db_path, writer_id).await
            })
            .await?;
        if created.load(Ordering::SeqCst) {
            self.listing_cache.lock().await.clear();
        }
        Ok(storage)
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
            IndexedStorageNamespace::StagedOpLog {
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
    async fn set_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        new_epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError> {
        self.storage_by_namespace(&namespace)
            .await?
            .set_key_epoch(svc_name, api_name, namespace, key, new_epoch)
            .await
    }

    async fn delete_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.storage_by_namespace(&namespace)
            .await?
            .delete_with_epoch(svc_name, api_name, namespace, key, expected_epoch)
            .await
    }

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
        let matching_files = self.namespace_db_files(&namespace).await?;

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
        // Walks files rather than keys: the token names the last file finished, and each file is
        // read whole. Files are emptied, never deleted, so the token stays a valid seek position.
        // Drained files stay listed, so a call stops after opening `count` files even if they were
        // all empty.
        let after = resume
            .map(|resume| resume.into_marker("Multi-SQLite"))
            .transpose()?;

        let files = self.namespace_db_files(&namespace).await?;
        // The listing is sorted, so the marker is found by bisection.
        let start = match after.as_deref() {
            Some(after) => files.partition_point(|file| file.as_str() <= after),
            None => 0,
        };

        // A zero count still opens one file, so every call makes progress.
        let page = count.max(1);
        let mut keys = Vec::new();
        let mut last_file = None;
        let mut opened = 0;
        for file_name in files[start..].iter().cloned() {
            if opened >= page {
                break;
            }
            opened += 1;
            let storage = self.storage_by_db_name(file_name.clone()).await?;

            // The whole file, which holds a single namespace.
            let mut within = None;
            loop {
                let (next, page) = storage
                    .scan_stable(svc_name, api_name, namespace.clone(), prefix, within, page)
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

        // The walk ends at the end of the file list, not on a short page, since the files a page
        // opened may simply have been empty.
        let exhausted = opened < page && (keys.len() as u64) < count;
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
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.storage_by_namespace(&namespace)
            .await?
            .append(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                id,
                value,
                expected_epoch,
            )
            .await
    }

    /// Overridden rather than inherited. The trait default loops [`Self::append`], which would
    /// resolve the per-agent database and re-check the fence once per entry, in a separate
    /// transaction each time - so a batch could land half-written, and the contract that the
    /// fence is checked once per call would not hold.
    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.storage_by_namespace(namespace)
            .await?
            .append_many(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                pairs,
                expected_epoch,
            )
            .await
    }

    async fn move_if_absent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        source_namespace: IndexedStorageNamespace,
        source_key: &str,
        target_namespace: IndexedStorageNamespace,
        target_key: &str,
        expected_last_id: u64,
    ) -> Result<bool, IndexedStorageError> {
        let source_db = self.namespace_to_db(&source_namespace).await;
        let target_db = self.namespace_to_db(&target_namespace).await;
        if source_db != target_db {
            return Err(IndexedStorageError::Other(
                "multi-SQLite cannot atomically move indexes across databases".to_string(),
            ));
        }
        self.storage_by_db_name(target_db)
            .await?
            .move_if_absent(
                svc_name,
                api_name,
                source_namespace,
                source_key,
                target_namespace,
                target_key,
                expected_last_id,
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use test_r::test;

    fn oplog_namespace(agent_id: &str) -> IndexedStorageNamespace {
        IndexedStorageNamespace::OpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: agent_id.to_string(),
            },
            agent_mode: AgentMode::Durable,
        }
    }

    #[test]
    async fn append_many_preserves_per_agent_database_routing() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(tempdir.path(), 1, false);
        let first_namespace = oplog_namespace("first-agent");
        let second_namespace = oplog_namespace("second-agent");

        storage
            .append_many(
                "test",
                "append_many",
                "entry",
                &first_namespace,
                "shared-key",
                vec![(1, Bytes::from_static(b"first-agent-value"))].into(),
                None,
            )
            .await
            .unwrap();
        storage
            .append_many(
                "test",
                "append_many",
                "entry",
                &second_namespace,
                "shared-key",
                vec![(1, Bytes::from_static(b"second-agent-value"))].into(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            storage
                .read("test", "read", "entry", first_namespace, "shared-key", 1, 1,)
                .await
                .unwrap(),
            vec![(1, b"first-agent-value".to_vec())]
        );
        assert_eq!(
            storage
                .read(
                    "test",
                    "read",
                    "entry",
                    second_namespace,
                    "shared-key",
                    1,
                    1,
                )
                .await
                .unwrap(),
            vec![(1, b"second-agent-value".to_vec())]
        );
    }
}
