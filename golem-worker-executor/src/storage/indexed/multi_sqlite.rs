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
    FencedTxError, IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace,
    IndexedStorageNamespace, ScanResume,
};
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::config::DbSqliteConfig;
use golem_common::model::{AgentId, ShardEpoch};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, OnceCell, RwLock};

const LISTING_TTL: Duration = Duration::from_secs(10);
static ROOTS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<DatabaseRoot>>>> = OnceLock::new();
static FILES: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<FileLifetime>>>> = OnceLock::new();
static POISONED: OnceLock<std::sync::Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// Shared by every pool generation and admitted operation for one canonical filename.
struct FileLifetime {
    path: PathBuf,
    gate: Arc<RwLock<()>>,
    registrations: AtomicUsize,
    retired: Notify,
    #[cfg(test)]
    waiting_retired: Notify,
}

impl FileLifetime {
    fn get(path: PathBuf) -> Arc<Self> {
        let mut files = FILES.get_or_init(Default::default).lock().unwrap();
        if let Some(file) = files.get(&path).and_then(Weak::upgrade) {
            return file;
        }
        let file = Arc::new(Self {
            path: path.clone(),
            gate: Arc::new(RwLock::new(())),
            registrations: AtomicUsize::new(0),
            retired: Notify::new(),
            #[cfg(test)]
            waiting_retired: Notify::new(),
        });
        files.insert(path, Arc::downgrade(&file));
        file
    }

    fn poison(&self) {
        POISONED
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(self.path.clone());
    }

    fn is_poisoned(&self) -> bool {
        POISONED
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .contains(&self.path)
    }

    /// Exclude admissions and evict the current cached value before waiting here.
    /// Retirement itself never acquires the admission gate.
    async fn wait_retired(&self) {
        loop {
            let notified = self.retired.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.registrations.load(Ordering::Acquire) == 0 {
                return;
            }
            #[cfg(test)]
            self.waiting_retired.notify_one();
            notified.await;
        }
    }
}

struct Completion {
    file: Arc<FileLifetime>,
    complete: bool,
}

impl Drop for FileLifetime {
    fn drop(&mut self) {
        let mut files = FILES.get_or_init(Default::default).lock().unwrap();
        if files
            .get(&self.path)
            .is_some_and(|weak| std::ptr::eq(weak.as_ptr(), self))
        {
            files.remove(&self.path);
        }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        if !self.complete {
            self.file.poison();
        }
    }
}

/// Created before opening any initialization connection, retained through both pool closes.
struct Registration(Completion);

impl Registration {
    fn new(file: Arc<FileLifetime>) -> Self {
        file.registrations.fetch_add(1, Ordering::AcqRel);
        Self(Completion {
            file,
            complete: false,
        })
    }

    fn finish(mut self) {
        self.0.complete = true;
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // Publish poison before publishing completion, including on interrupted retirement.
        if !self.0.complete {
            self.0.file.poison();
        }
        self.0.file.registrations.fetch_sub(1, Ordering::AcqRel);
        self.0.file.retired.notify_waiters();
    }
}

struct ManagedStorage {
    storage: Option<SqliteIndexedStorage>,
    registration: Option<Registration>,
    runtime: tokio::runtime::Handle,
}

impl Deref for ManagedStorage {
    type Target = SqliteIndexedStorage;
    fn deref(&self) -> &Self::Target {
        self.storage.as_ref().unwrap()
    }
}

impl Drop for ManagedStorage {
    fn drop(&mut self) {
        let storage = self.storage.take().unwrap();
        let registration = self.registration.take().unwrap();
        // Transfer the owned pools and registration together, before returning from final Drop.
        // Shutdown cancellation drops an incomplete registration and permanently poisons the path.
        self.runtime.spawn(async move {
            storage.close().await;
            drop(storage);
            registration.finish();
        });
    }
}

/// IndexedStorage implementation using a separate SQLite database per agent and namespace.
#[derive(Clone)]
pub struct MultiSqliteIndexedStorage {
    root: Arc<DatabaseRoot>,
}

struct DatabaseRoot {
    cache: Cache<String, (), Arc<ManagedStorage>, IndexedStorageError>,
    epochs: OnceCell<SqliteIndexedStorage>,
    hash_cache: Mutex<HashCache>,
    listing_cache: Mutex<HashMap<String, CachedListing>>,
    root_dir: PathBuf,
    max_connections: u32,
    foreign_keys: bool,
    #[cfg(test)]
    snapshot_failure: AtomicUsize,
}

struct HashCache {
    hash_per_agent_id: HashMap<AgentId, String>,
    agent_id_per_hash: HashMap<String, AgentId>,
}

struct CachedListing {
    files: Arc<Vec<String>>,
    read_at: Instant,
    deletion_dirty: bool,
}

type Epochs = Vec<(String, String, i64)>;

enum Access {
    Create,
    Read,
    Move,
    PlainDelete,
    Delete {
        namespace: String,
        key: String,
        expected: Option<ShardEpoch>,
    },
    DropPrefix {
        namespace: String,
        key: String,
        expected: Option<ShardEpoch>,
    },
    DeleteEmpty {
        namespace: String,
        key: String,
        expected: Option<ShardEpoch>,
    },
    SetEpoch {
        namespace: String,
        key: String,
        epoch: i64,
    },
}

impl Access {
    fn exclusive(&self) -> bool {
        !matches!(self, Self::Create | Self::Read | Self::Move)
    }

    fn reclaims(&self) -> bool {
        matches!(
            self,
            Self::PlainDelete | Self::Delete { .. } | Self::DropPrefix { .. }
        )
    }

    fn absent(&self, epochs: &mut Epochs) -> Result<(), IndexedStorageError> {
        let (namespace, key, expected) = match self {
            Self::Delete {
                namespace,
                key,
                expected,
            }
            | Self::DropPrefix {
                namespace,
                key,
                expected,
            }
            | Self::DeleteEmpty {
                namespace,
                key,
                expected,
            } => (namespace, key, expected),
            Self::SetEpoch {
                namespace,
                key,
                epoch,
            } => {
                if let Some((_, _, stored)) = epochs
                    .iter_mut()
                    .find(|(ns, k, _)| ns == namespace && k == key)
                {
                    if *stored < 0 || *stored > *epoch {
                        FencedTxError::check_record(
                            key,
                            ShardEpoch(*epoch as u64),
                            Some(*stored),
                            SqliteIndexedStorage::negative_epoch_message,
                        )
                        .map_err(|e| {
                            e.into_indexed_storage_error(SqliteIndexedStorage::classify_repo_error)
                        })?;
                    }
                    *stored = *epoch;
                } else {
                    epochs.push((namespace.clone(), key.clone(), *epoch));
                }
                return Ok(());
            }
            Self::Move => {
                return Err(IndexedStorageError::Other(
                    "source index is missing, empty, gapped, or has an unexpected tip".into(),
                ));
            }
            Self::Read | Self::Create | Self::PlainDelete => return Ok(()),
        };
        let stored = epochs
            .iter()
            .find(|(ns, k, _)| ns == namespace && k == key)
            .map(|(_, _, epoch)| *epoch);
        if let Some(expected) = expected {
            // Delete alone permits an already-gone key without an epoch record.
            if stored.is_some() || !matches!(self, Self::Delete { .. }) {
                FencedTxError::check_record(
                    key,
                    *expected,
                    stored,
                    SqliteIndexedStorage::negative_epoch_message,
                )
                .map_err(|e| {
                    e.into_indexed_storage_error(SqliteIndexedStorage::classify_repo_error)
                })?;
            }
        }
        if matches!(self, Self::Delete { .. }) {
            epochs.retain(|(ns, k, _)| ns != namespace || k != key);
        }
        Ok(())
    }
}

impl MultiSqliteIndexedStorage {
    pub fn new(root_dir: &Path, max_connections: u32, foreign_keys: bool) -> Self {
        std::fs::create_dir_all(root_dir)
            .expect("Failed to create root directory for sqlite storage");
        let root_dir = std::fs::canonicalize(root_dir)
            .expect("Failed to resolve root directory for sqlite storage");
        let mut roots = ROOTS.get_or_init(Default::default).lock().unwrap();
        let root = roots
            .get(&root_dir)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let root = Arc::new(DatabaseRoot {
                    epochs: OnceCell::new(),
                    cache: Cache::new(
                        Some(1024),
                        FullCacheEvictionMode::LeastRecentlyUsed(1),
                        BackgroundEvictionMode::OlderThan {
                            ttl: Duration::from_secs(21600),
                            period: Duration::from_secs(60),
                        },
                        "multi-sqlite-indexed",
                    ),
                    hash_cache: Mutex::new(HashCache {
                        hash_per_agent_id: HashMap::new(),
                        agent_id_per_hash: HashMap::new(),
                    }),
                    listing_cache: Mutex::new(HashMap::new()),
                    root_dir: root_dir.clone(),
                    max_connections,
                    foreign_keys,
                    #[cfg(test)]
                    snapshot_failure: AtomicUsize::new(0),
                });
                roots.insert(root_dir, Arc::downgrade(&root));
                root
            });
        Self { root }
    }

    async fn agent_id_hash(&self, agent_id: &AgentId) -> String {
        let mut cache = self.root.hash_cache.lock().await;
        if let Some(hash) = cache.hash_per_agent_id.get(agent_id) {
            return hash.clone();
        }
        let hash = blake3::hash(agent_id.to_string().as_bytes()).to_string();
        cache
            .hash_per_agent_id
            .insert(agent_id.clone(), hash.clone());
        cache
            .agent_id_per_hash
            .insert(hash.clone(), agent_id.clone());
        hash
    }

    async fn namespace_to_db(&self, namespace: &IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog {
                agent_id,
                agent_mode,
            }
            | IndexedStorageNamespace::StagedOpLog {
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
            IndexedStorageNamespace::BlobOplogManifest {
                agent_id,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!(
                    "{mode}-blob-oplog-l{}-{}.db",
                    level,
                    self.agent_id_hash(agent_id).await
                )
            }
        }
    }

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
            IndexedStorageMetaNamespace::BlobOplogManifest { agent_mode, level } => {
                let mode = super::agent_mode_prefix(*agent_mode);
                format!("{mode}-blob-oplog-l{}-", level)
            }
        }
    }

    async fn namespace_db_files(
        &self,
        namespace: &IndexedStorageMetaNamespace,
        fresh_walk: bool,
    ) -> Result<Arc<Vec<String>>, IndexedStorageError> {
        let db_prefix = Self::db_prefix(namespace);
        // Serialize listing publication with file-creation invalidation.
        let mut cache = self.root.listing_cache.lock().await;
        if let Some(cached) = cache.get(&db_prefix)
            && cached.read_at.elapsed() < LISTING_TTL
            && !(fresh_walk && cached.deletion_dirty)
        {
            return Ok(cached.files.clone());
        }
        let root_dir = self.root.root_dir.clone();
        let prefix = db_prefix.clone();
        let files = tokio::task::spawn_blocking(move || Self::read_db_files(&root_dir, &prefix))
            .await
            .map_err(|e| {
                IndexedStorageError::Other(format!("Failed to list the root directory: {e:?}"))
            })??;
        let files = Arc::new(files);
        cache.insert(
            db_prefix,
            CachedListing {
                files: files.clone(),
                read_at: Instant::now(),
                deletion_dirty: false,
            },
        );
        Ok(files)
    }

    fn read_db_files(root_dir: &Path, db_prefix: &str) -> Result<Vec<String>, IndexedStorageError> {
        let mut files: Vec<_> = std::fs::read_dir(root_dir)
            .map_err(|e| {
                IndexedStorageError::Other(format!("Failed to read root directory: {e:?}"))
            })?
            .filter_map(|entry| {
                entry.ok().and_then(|e| {
                    let path = e.path();
                    let name = path.file_name()?.to_string_lossy().to_string();
                    (name.starts_with(db_prefix) && name.ends_with(".db")).then_some(name)
                })
            })
            .collect();
        files.sort();
        Ok(files)
    }

    /// The task owns admission, cache loading, and SQL settlement independently of its waiter.
    async fn with_database<T, F, Fut>(
        &self,
        db: String,
        operation: F,
    ) -> Result<T, IndexedStorageError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<ManagedStorage>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, IndexedStorageError>> + Send + 'static,
    {
        self.with_access(db, Access::Create, operation)
            .await
            .map(Option::unwrap)
    }

    async fn with_existing<T, F, Fut>(
        &self,
        db: String,
        operation: F,
    ) -> Result<Option<T>, IndexedStorageError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<ManagedStorage>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, IndexedStorageError>> + Send + 'static,
    {
        let file = FileLifetime::get(self.root.root_dir.join(&db));
        let lease = file.gate.clone().read_owned().await;
        // Cancellation after admission can abandon pooled I/O. Publish uncertainty before
        // releasing the lease, so a waiting delete cannot physically reclaim this file.
        let mut completion = Completion {
            file: file.clone(),
            complete: false,
        };
        if let Some(storage) = self.root.cache.get(&db).await {
            golem_common::metrics::caching::record_cache_hit("multi-sqlite-indexed");
            let result = operation(storage).await;
            if result.is_err() {
                // A replacement waits for this generation to retire; do not wait under a shared lease.
                self.root.cache.remove(&db).await;
            }
            completion.complete = true;
            return result.map(Some);
        }
        completion.complete = true;
        drop(completion);
        drop(lease);
        self.with_access(db, Access::Read, operation).await
    }

    async fn with_access<T, F, Fut>(
        &self,
        db: String,
        access: Access,
        operation: F,
    ) -> Result<Option<T>, IndexedStorageError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<ManagedStorage>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, IndexedStorageError>> + Send + 'static,
    {
        let root = self.root.clone();
        let file = FileLifetime::get(root.root_dir.join(&db));
        let task = tokio::spawn(async move {
            let mut exclusive = if access.exclusive() {
                Some(file.gate.clone().write_owned().await)
            } else {
                None
            };
            let mut reader = if exclusive.is_none() {
                Some(file.gate.clone().read_owned().await)
            } else {
                None
            };
            let mut completion = Completion {
                file: file.clone(),
                complete: false,
            };
            let mut authority_transfer = false;
            let work = async {
                let mut cached = root.cache.get(&db).await;
                if cached.is_none() && exclusive.is_none() {
                    drop(reader.take());
                    exclusive = Some(file.gate.clone().write_owned().await);
                    cached = root.cache.get(&db).await;
                }
                let storage = match cached {
                    Some(storage) => {
                        golem_common::metrics::caching::record_cache_hit("multi-sqlite-indexed");
                        storage
                    }
                    None => {
                        // Wait before registering the new generation, including generations from old roots.
                        file.wait_retired().await;
                        let metadata = root.epoch_storage().await?;
                        let snapshot = metadata.database_epoch_snapshot(&db).await?;
                        let has_snapshot = snapshot.is_some();
                        let existed = tokio::fs::try_exists(&file.path).await.map_err(|e| {
                            IndexedStorageError::Other(format!(
                                "Failed to inspect database file: {e}"
                            ))
                        })?;
                        if snapshot.is_some() || !existed {
                            if !matches!(access, Access::Create)
                                && !(matches!(access, Access::SetEpoch { .. })
                                    && snapshot.is_none())
                            {
                                let mut epochs = snapshot.unwrap_or_default();
                                access.absent(&mut epochs)?;
                                let empty_snapshot = epochs.is_empty();
                                if has_snapshot
                                    && matches!(
                                        access,
                                        Access::Delete { .. } | Access::SetEpoch { .. }
                                    )
                                {
                                    authority_transfer = true;
                                    metadata.save_database_epoch_snapshot(&db, epochs).await?;
                                }
                                if has_snapshot && empty_snapshot && !file.is_poisoned() {
                                    // A snapshot can still cover an old physical file after a crash.
                                    authority_transfer = true;
                                    root.unlink(&file, &db).await?;
                                    metadata.delete_database_epoch_snapshot(&db).await?;
                                }
                                authority_transfer = false;
                                return Ok(None);
                            }
                            if snapshot.is_some() {
                                if file.is_poisoned() {
                                    return Err(IndexedStorageError::Other("Database recovery requires a process restart after uncertain retirement".into()));
                                }
                                authority_transfer = true;
                                root.unlink(&file, &db).await?;
                            }
                        }
                        let registration = Registration::new(file.clone());
                        let config = DbSqliteConfig {
                            database: file.path.to_string_lossy().into_owned(),
                            max_connections: root.max_connections,
                            foreign_keys: root.foreign_keys,
                        };
                        let storage = Arc::new(ManagedStorage {
                            storage: Some(SqliteIndexedStorage::configured_managed(&config).await?),
                            registration: Some(registration),
                            runtime: tokio::runtime::Handle::current(),
                        });
                        let created = !existed || snapshot.is_some();
                        if let Some(epochs) = snapshot {
                            storage.restore_database_epochs(epochs).await?;
                            #[cfg(test)]
                            root.fail_snapshot_boundary(2)?;
                            metadata.delete_database_epoch_snapshot(&db).await?;
                            #[cfg(test)]
                            root.fail_snapshot_boundary(3)?;
                        }
                        if created {
                            root.invalidate_creation(&db).await;
                        }
                        let storage = root
                            .cache
                            .get_or_insert_simple(&db, async || Ok(storage))
                            .await?;
                        authority_transfer = false;
                        storage
                    }
                };
                if !access.exclusive()
                    && let Some(writer) = exclusive.take()
                {
                    reader = Some(writer.downgrade());
                }
                let value = operation(storage.clone()).await?;
                if access.reclaims()
                    && !file.is_poisoned()
                    && let Some(epochs) = storage.empty_database_epochs().await?
                {
                    // Even an ambiguous metadata write must not leave a warm authoritative bypass.
                    root.cache.remove(&db).await;
                    let empty_snapshot = epochs.is_empty();
                    authority_transfer = true;
                    root.epoch_storage()
                        .await?
                        .save_database_epoch_snapshot(&db, epochs)
                        .await?;
                    #[cfg(test)]
                    root.fail_snapshot_boundary(1)?;
                    drop(storage);
                    file.wait_retired().await;
                    if !file.is_poisoned() {
                        root.unlink(&file, &db).await?;
                        if empty_snapshot {
                            root.epoch_storage()
                                .await?
                                .delete_database_epoch_snapshot(&db)
                                .await?;
                        }
                    }
                    authority_transfer = false;
                }
                Ok(Some(value))
            };
            let result = work.await;
            if result.is_err() {
                root.cache.remove(&db).await;
                if authority_transfer {
                    file.poison();
                }
            }
            completion.complete = true;
            result
        });
        task.await
            .map_err(|e| IndexedStorageError::Other(format!("Database operation failed: {e}")))?
    }
}

impl DatabaseRoot {
    #[cfg(test)]
    fn fail_snapshot_boundary(&self, boundary: usize) -> Result<(), IndexedStorageError> {
        if self
            .snapshot_failure
            .compare_exchange(boundary, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Err(IndexedStorageError::Other(
                "Reported snapshot infrastructure failure".into(),
            ));
        }
        Ok(())
    }

    async fn epoch_storage(&self) -> Result<&SqliteIndexedStorage, IndexedStorageError> {
        self.epochs
            .get_or_try_init(|| async {
                SqliteIndexedStorage::configured(&DbSqliteConfig {
                    database: self
                        .root_dir
                        .join("epochs.db")
                        .to_string_lossy()
                        .into_owned(),
                    max_connections: 1,
                    foreign_keys: self.foreign_keys,
                })
                .await
            })
            .await
    }

    fn listing_prefix(db: &str) -> Option<&str> {
        db.rsplit_once('-')
            .map(|(prefix, _)| &db[..prefix.len() + 1])
    }

    async fn invalidate_creation(&self, db: &str) {
        if let Some(prefix) = Self::listing_prefix(db) {
            self.listing_cache.lock().await.remove(prefix);
        }
    }

    async fn unlink(&self, file: &FileLifetime, db: &str) -> Result<(), IndexedStorageError> {
        for suffix in ["-wal", "-shm", "-journal", ""] {
            let path = self.root_dir.join(format!("{db}{suffix}"));
            match tokio::fs::remove_file(path).await {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => {
                    file.poison();
                    return Err(IndexedStorageError::Other(format!(
                        "Failed to remove database file: {e}"
                    )));
                }
            }
        }
        if let Some(prefix) = Self::listing_prefix(db)
            && let Some(listing) = self.listing_cache.lock().await.get_mut(prefix)
        {
            listing.deletion_dirty = true;
        }
        Ok(())
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
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        let access = Access::SetEpoch {
            namespace: SqliteIndexedStorage::namespace(namespace.clone()),
            key: key.clone(),
            epoch: SqliteIndexedStorage::to_i64(new_epoch.0, "epoch")?,
        };
        self.with_access(db, access, move |s| async move {
            s.set_key_epoch(svc_name, api_name, namespace, &key, new_epoch)
                .await
        })
        .await
        .map(|_| ())
    }

    async fn delete_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        let access = Access::Delete {
            namespace: SqliteIndexedStorage::namespace(namespace.clone()),
            key: key.clone(),
            expected: expected_epoch,
        };
        self.with_access(db, access, move |s| async move {
            s.delete_with_epoch(svc_name, api_name, namespace, &key, expected_epoch)
                .await
        })
        .await
        .map(|_| ())
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
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_existing(db, move |s| async move {
            s.exists(svc_name, api_name, namespace, &key).await
        })
        .await
        .map(|value| value.unwrap_or_default())
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
        // Read whole files and stop after count files, including empty files.
        let after = resume
            .map(|resume| resume.into_marker("Multi-SQLite"))
            .transpose()?;
        let files = self.namespace_db_files(&namespace, after.is_none()).await?;
        let start = match after.as_deref() {
            Some(after) => files.partition_point(|file| file.as_str() <= after),
            None => 0,
        };
        let page = count.max(1);
        let mut keys = Vec::new();
        let mut last_file = None;
        let mut opened = 0;
        for file in files[start..].iter().cloned() {
            if opened >= page {
                break;
            }
            opened += 1;
            let ns = namespace.clone();
            let prefix = prefix.map(str::to_string);
            let chunk = self
                .with_existing(file.clone(), move |s| async move {
                    let mut keys = Vec::new();
                    let mut within = None;
                    loop {
                        let (next, chunk) = s
                            .scan_stable(
                                svc_name,
                                api_name,
                                ns.clone(),
                                prefix.as_deref(),
                                within,
                                page,
                            )
                            .await?;
                        keys.extend(chunk);
                        match next {
                            Some(next) => within = Some(next),
                            None => return Ok(keys),
                        }
                    }
                })
                .await?
                .unwrap_or_default();
            keys.extend(chunk);
            last_file = Some(file);
            if keys.len() as u64 >= count {
                break;
            }
        }
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
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_database(db, move |s| async move {
            s.append(
                svc_name,
                api_name,
                entity_name,
                namespace,
                &key,
                id,
                value,
                expected_epoch,
            )
            .await
        })
        .await
    }

    /// Keep the batch atomic and check its fence once, rather than calling append per entry.
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
        if pairs.is_empty() {
            return Ok(());
        }
        let db = self.namespace_to_db(namespace).await;
        let namespace = namespace.clone();
        let key = key.to_string();
        self.with_database(db, move |s| async move {
            s.append_many(
                svc_name,
                api_name,
                entity_name,
                &namespace,
                &key,
                pairs,
                expected_epoch,
            )
            .await
        })
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
        let source_key = source_key.to_string();
        let target_key = target_key.to_string();
        self.with_access(target_db, Access::Move, move |s| async move {
            s.move_if_absent(
                svc_name,
                api_name,
                source_namespace,
                &source_key,
                target_namespace,
                &target_key,
                expected_last_id,
            )
            .await
        })
        .await
        .map(Option::unwrap)
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_existing(db, move |s| async move {
            s.length(svc_name, api_name, namespace, &key).await
        })
        .await
        .map(|value| value.unwrap_or_default())
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_access(db, Access::PlainDelete, move |s| async move {
            s.delete(svc_name, api_name, namespace, &key).await
        })
        .await
        .map(|_| ())
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
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_existing(db, move |s| async move {
            s.read(
                svc_name,
                api_name,
                entity_name,
                namespace,
                &key,
                start_id,
                end_id,
            )
            .await
        })
        .await
        .map(|value| value.unwrap_or_default())
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_existing(db, move |s| async move {
            s.first(svc_name, api_name, entity_name, namespace, &key)
                .await
        })
        .await
        .map(Option::flatten)
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_existing(db, move |s| async move {
            s.last(svc_name, api_name, entity_name, namespace, &key)
                .await
        })
        .await
        .map(Option::flatten)
    }

    async fn last_id(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_existing(db, move |s| async move {
            s.last_id(svc_name, api_name, entity_name, namespace, &key)
                .await
        })
        .await
        .map(Option::flatten)
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
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        self.with_existing(db, move |s| async move {
            s.closest(svc_name, api_name, entity_name, namespace, &key, id)
                .await
        })
        .await
        .map(Option::flatten)
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        let access = Access::DropPrefix {
            namespace: SqliteIndexedStorage::namespace(namespace.clone()),
            key: key.clone(),
            expected: expected_epoch,
        };
        self.with_access(db, access, move |s| async move {
            s.drop_prefix(
                svc_name,
                api_name,
                namespace,
                &key,
                last_dropped_id,
                expected_epoch,
            )
            .await
        })
        .await
        .map(|_| ())
    }

    async fn delete_empty_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<bool, IndexedStorageError> {
        let db = self.namespace_to_db(&namespace).await;
        let key = key.to_string();
        let access = Access::DeleteEmpty {
            namespace: SqliteIndexedStorage::namespace(namespace.clone()),
            key: key.clone(),
            expected: expected_epoch,
        };
        self.with_access(db, access, move |s| async move {
            s.delete_empty_with_epoch(svc_name, api_name, namespace, &key, expected_epoch)
                .await
        })
        .await
        .map(|value| value.unwrap_or(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use test_r::test;
    use tokio::sync::oneshot;

    fn oplog_namespace(name: &str) -> IndexedStorageNamespace {
        IndexedStorageNamespace::OpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: name.to_string(),
            },
            agent_mode: AgentMode::Durable,
        }
    }

    async fn warm(storage: &MultiSqliteIndexedStorage, db: &str) {
        storage
            .with_database(db.to_string(), |_| async { Ok(()) })
            .await
            .unwrap();
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn cancelled_warm_read_poisons_before_exclusive_admission() {
        use golem_service_base::db::LabelledPoolTransaction;
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("cancelled-read");
        storage
            .append(
                "test",
                "seed",
                "entry",
                ns.clone(),
                "key",
                3,
                vec![37],
                None,
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        let cached = storage.root.cache.get(&db).await.unwrap();
        let connection = cached.hold_connection_for_test().await;
        drop(cached);
        let worker = storage.clone();
        let reading_ns = ns.clone();
        let (entered_tx, entered_rx) = oneshot::channel();
        let read = tokio::spawn(async move {
            worker
                .with_existing(db, move |pool| async move {
                    entered_tx.send(()).unwrap();
                    pool.length("test", "read", reading_ns, "key").await
                })
                .await
        });
        entered_rx.await.unwrap();
        let exclusive = file.gate.clone().write_owned();
        tokio::pin!(exclusive);
        assert!(futures::poll!(&mut exclusive).is_pending());
        read.abort();
        assert!(read.await.unwrap_err().is_cancelled());
        let lease = exclusive.await;
        assert!(file.is_poisoned());
        drop(lease);
        // The held reader also proves poisoned deletion skips the emptiness query.
        storage
            .delete("test", "delete", ns.clone(), "key")
            .await
            .unwrap();
        assert!(file.path.exists());
        assert!(
            storage
                .root
                .epoch_storage()
                .await
                .unwrap()
                .database_epoch_snapshot(file.path.file_name().unwrap().to_str().unwrap())
                .await
                .unwrap()
                .is_none()
        );
        connection.rollback().await.unwrap();
        assert_eq!(storage.length("test", "read", ns, "key").await.unwrap(), 0);

        let other = oplog_namespace("successful-read");
        storage
            .append(
                "test",
                "seed",
                "entry",
                other.clone(),
                "key",
                5,
                vec![53],
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .length("test", "read", other.clone(), "key")
                .await
                .unwrap(),
            1
        );
        let other_file = FileLifetime::get(
            storage
                .root
                .root_dir
                .join(storage.namespace_to_db(&other).await),
        );
        assert!(!other_file.is_poisoned());
        storage
            .delete("test", "delete", other, "key")
            .await
            .unwrap();
        assert!(!other_file.path.exists());
    }

    #[test]
    async fn cancelling_read_before_admission_does_not_poison() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("not-admitted");
        storage
            .append(
                "test",
                "seed",
                "entry",
                ns.clone(),
                "key",
                7,
                vec![71],
                None,
            )
            .await
            .unwrap();
        let file = FileLifetime::get(
            storage
                .root
                .root_dir
                .join(storage.namespace_to_db(&ns).await),
        );
        let lease = file.gate.clone().write_owned().await;
        let mut read = Box::pin(storage.length("test", "read", ns.clone(), "key"));
        assert!(futures::poll!(&mut read).is_pending());
        drop(read);
        assert!(!file.is_poisoned());
        drop(lease);
        assert_eq!(
            storage
                .length("test", "read", ns.clone(), "key")
                .await
                .unwrap(),
            1
        );
        storage.delete("test", "delete", ns, "key").await.unwrap();
        assert!(!file.path.exists());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn shared_readers_overlap_exclusive_waits_and_other_file_progresses() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 2, false);
        let second = MultiSqliteIndexedStorage::new(&dir.path().join("."), 2, false);
        assert!(Arc::ptr_eq(&storage.root, &second.root));
        warm(&storage, "first.db").await;
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let first = tokio::spawn(async move {
            second
                .with_database("first.db".into(), move |_| async move {
                    entered_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    Ok(())
                })
                .await
        });
        entered_rx.await.unwrap();
        // A second same-file reader completes while the first is held.
        warm(&storage, "first.db").await;
        let file = FileLifetime::get(storage.root.root_dir.join("first.db"));
        let exclusive = file.gate.clone().write_owned();
        tokio::pin!(exclusive);
        assert!(futures::poll!(&mut exclusive).is_pending());
        warm(&storage, "other.db").await;
        release_tx.send(()).unwrap();
        first.await.unwrap().unwrap();
        drop(exclusive.await);
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn eviction_during_operation_keeps_registration_until_completion() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        warm(&storage, "active.db").await;
        let file = FileLifetime::get(storage.root.root_dir.join("active.db"));
        let worker = storage.clone();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let work = tokio::spawn(async move {
            worker
                .with_database("active.db".into(), move |s| async move {
                    entered_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    // Retain the actual managed value through operation completion.
                    assert!(s.storage.is_some());
                    Ok(())
                })
                .await
        });
        entered_rx.await.unwrap();
        storage.root.cache.remove(&"active.db".to_string()).await;
        assert_eq!(file.registrations.load(Ordering::Acquire), 1);
        let retired = file.wait_retired();
        tokio::pin!(retired);
        assert!(futures::poll!(&mut retired).is_pending());
        release_tx.send(()).unwrap();
        work.await.unwrap().unwrap();
        retired.await;
        assert!(!file.is_poisoned());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn retirement_wait_includes_earlier_evicted_generation() {
        use golem_service_base::db::{LabelledPoolTransaction, PoolApi};
        for entrypoint in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
            let ns = oplog_namespace("retiring");
            storage
                .set_key_epoch("test", "epoch", ns.clone(), "key", ShardEpoch(11))
                .await
                .unwrap();
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    ns.clone(),
                    "key",
                    3,
                    vec![37],
                    Some(ShardEpoch(11)),
                )
                .await
                .unwrap();
            let db = storage.namespace_to_db(&ns).await;
            let file = FileLifetime::get(storage.root.root_dir.join(&db));
            let cached = storage.root.cache.get(&db).await.unwrap();
            // A real acquired connection holds SQLx graceful close, with no cleanup gate involved.
            let mut connection = cached.hold_connection_for_test().await;
            storage.root.cache.remove(&db).await;
            drop(cached);
            let worker = storage.clone();
            let requested_ns = ns.clone();
            let reopened = tokio::spawn(async move {
                match entrypoint {
                    0 => {
                        worker
                            .read("test", "read", "entry", requested_ns, "key", 1, 9)
                            .await
                    }
                    1 => worker
                        .set_key_epoch("test", "epoch", requested_ns, "key", ShardEpoch(19))
                        .await
                        .map(|_| Vec::new()),
                    2 => worker
                        .delete("test", "delete", requested_ns, "key")
                        .await
                        .map(|_| Vec::new()),
                    _ => worker
                        .drop_prefix(
                            "test",
                            "prefix",
                            requested_ns,
                            "key",
                            3,
                            Some(ShardEpoch(11)),
                        )
                        .await
                        .map(|_| Vec::new()),
                }
            });
            file.waiting_retired.notified().await;
            assert!(!reopened.is_finished());
            assert!(storage.root.cache.get(&db).await.is_none());
            assert_eq!(file.registrations.load(Ordering::Acquire), 1);
            warm(&storage, "unrelated.db").await;
            let unchanged: (i64, i64) = connection.fetch_one_as(sqlx::query_as(
                "SELECT (SELECT epoch FROM indexed_key_epoch WHERE key = 'key'), (SELECT COUNT(*) FROM index_storage WHERE key = 'key');",
            )).await.unwrap();
            assert_eq!(unchanged, (11, 1));
            connection.rollback().await.unwrap();
            let result = reopened.await.unwrap().unwrap();
            if entrypoint == 0 {
                assert_eq!(result, vec![(3, vec![37])]);
            }
            assert_eq!(
                storage
                    .read("test", "read", "entry", ns.clone(), "key", 1, 9)
                    .await
                    .unwrap(),
                if entrypoint < 2 {
                    vec![(3, vec![37])]
                } else {
                    Vec::new()
                }
            );
            if entrypoint == 1 {
                assert!(matches!(
                    storage
                        .append(
                            "test",
                            "stale",
                            "entry",
                            ns,
                            "key",
                            4,
                            vec![8],
                            Some(ShardEpoch(11))
                        )
                        .await,
                    Err(IndexedStorageError::Fenced {
                        actual: Some(ShardEpoch(19)),
                        ..
                    })
                ));
            }
            storage.root.cache.remove(&db).await;
            file.wait_retired().await;
            assert!(!file.is_poisoned());
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn cancellation_retains_admission_until_owned_operation_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let worker = storage.clone();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (finished_tx, finished_rx) = oneshot::channel();
        let request = tokio::spawn(async move {
            worker
                .with_database("cancel.db".into(), move |_| async move {
                    entered_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    finished_tx.send(()).unwrap();
                    Ok(())
                })
                .await
        });
        entered_rx.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let file = FileLifetime::get(storage.root.root_dir.join("cancel.db"));
        let exclusive = file.gate.clone().write_owned();
        tokio::pin!(exclusive);
        assert!(futures::poll!(&mut exclusive).is_pending());
        release_tx.send(()).unwrap();
        finished_rx.await.unwrap();
        drop(exclusive.await);
        assert!(!file.is_poisoned());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn completed_errors_retire_without_poison_and_reopening_waits() {
        use golem_service_base::db::LabelledPoolTransaction;
        for warm_read in [false, true] {
            for kind in 0..3 {
                let dir = tempfile::tempdir().unwrap();
                let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
                let db = "failed.db".to_string();
                warm(&storage, &db).await;
                let file = FileLifetime::get(storage.root.root_dir.join(&db));
                let cached = storage.root.cache.get(&db).await.unwrap();
                let connection = cached.hold_connection_for_test().await;
                drop(cached);
                let failure = move |_| async move {
                    Err::<(), _>(match kind {
                        0 => IndexedStorageError::Transient("completed failure".into()),
                        1 => IndexedStorageError::Indeterminate("completed failure".into()),
                        _ => IndexedStorageError::Other("completed failure".into()),
                    })
                };
                let result = if warm_read {
                    storage.with_existing(db.clone(), failure).await
                } else {
                    storage
                        .with_access(db.clone(), Access::Create, failure)
                        .await
                };
                assert!(result.is_err());
                assert!(!file.is_poisoned());
                assert!(storage.root.cache.get(&db).await.is_none());
                assert!(file.path.exists());
                let worker = storage.clone();
                let reopened = tokio::spawn(async move { warm(&worker, &db).await });
                file.waiting_retired.notified().await;
                assert!(!reopened.is_finished());
                assert_eq!(file.registrations.load(Ordering::Acquire), 1);
                connection.rollback().await.unwrap();
                reopened.await.unwrap();
                assert!(!file.is_poisoned());
            }
        }
    }

    #[test]
    async fn initialization_failure_poison_survives_reconstruction() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let path = storage.root.root_dir.join("bad.db");
        std::fs::create_dir(&path).unwrap();
        assert!(
            storage
                .with_database("bad.db".into(), |_| async { Ok(()) })
                .await
                .is_err()
        );
        drop(storage);
        assert!(FileLifetime::get(path).is_poisoned());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn cancelled_cold_request_finishes_loading_without_pending_cache_entry() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let file = FileLifetime::get(storage.root.root_dir.join("cold.db"));
        let exclusive = file.gate.clone().write_owned().await;
        let (finished_tx, finished_rx) = oneshot::channel();
        let mut request = Box::pin(
            storage.with_database("cold.db".into(), move |_| async move {
                finished_tx.send(()).unwrap();
                Ok(())
            }),
        );
        // Poll far enough to launch the owned task, then cancel only the originating wait.
        assert!(futures::poll!(&mut request).is_pending());
        drop(request);
        drop(exclusive);
        finished_rx.await.unwrap();
        warm(&storage, "cold.db").await;
        assert!(
            storage
                .root
                .cache
                .get(&"cold.db".to_string())
                .await
                .is_some()
        );
        assert!(!file.is_poisoned());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn root_reconstruction_keeps_previous_retiring_lifetime() {
        use golem_service_base::db::LabelledPoolTransaction;
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        warm(&storage, "old.db").await;
        let path = storage.root.root_dir.join("old.db");
        let file = FileLifetime::get(path.clone());
        let old_file = Arc::downgrade(&file);
        let old_root = Arc::downgrade(&storage.root);
        let cached = storage.root.cache.get(&"old.db".to_string()).await.unwrap();
        let connection = cached.hold_connection_for_test().await;
        drop(file);
        drop(cached);
        drop(storage);
        assert!(old_root.upgrade().is_none());
        let replacement = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let file = FileLifetime::get(path);
        assert!(Arc::ptr_eq(&old_file.upgrade().unwrap(), &file));
        let reopened = warm(&replacement, "old.db");
        tokio::pin!(reopened);
        assert!(futures::poll!(&mut reopened).is_pending());
        assert_eq!(file.registrations.load(Ordering::Acquire), 1);
        connection.rollback().await.unwrap();
        reopened.await;
        replacement.root.cache.remove(&"old.db".to_string()).await;
        file.wait_retired().await;
        assert!(!file.is_poisoned());
    }

    #[test]
    fn interrupted_retirement_permanently_poisons_reconstructed_path() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let storage = {
            let _entered = runtime.enter();
            MultiSqliteIndexedStorage::new(dir.path(), 1, false)
        };
        let path = storage.root.root_dir.join("interrupted.db");
        runtime.block_on(warm(&storage, "interrupted.db"));
        let cached = runtime
            .block_on(storage.root.cache.get(&"interrupted.db".to_string()))
            .unwrap();
        let connection = runtime.block_on(cached.hold_connection_for_test());
        drop(cached);
        // Final cache drop publishes retirement to the captured runtime even outside its context.
        drop(storage);
        runtime.block_on(async {
            tokio::task::yield_now().await;
        });
        drop(runtime);
        drop(connection);
        let file = FileLifetime::get(path.clone());
        assert!(file.is_poisoned());
        drop(file);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let replacement = {
            let _entered = runtime.enter();
            MultiSqliteIndexedStorage::new(dir.path(), 1, false)
        };
        runtime.block_on(warm(&replacement, "interrupted.db"));
        assert!(FileLifetime::get(path).is_poisoned());
        drop(replacement);
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

    #[test]
    async fn staging_publication_and_fencing_survive_reclamation() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let visible = oplog_namespace("staged");
        let IndexedStorageNamespace::OpLog {
            agent_id,
            agent_mode,
        } = visible.clone()
        else {
            unreachable!()
        };
        let stage = IndexedStorageNamespace::StagedOpLog {
            agent_id,
            agent_mode,
        };
        storage
            .append(
                "test",
                "append",
                "entry",
                stage.clone(),
                "stage",
                1,
                vec![1],
                None,
            )
            .await
            .unwrap();
        assert!(
            storage
                .move_if_absent("test", "publish", stage, "stage", visible.clone(), "key", 1)
                .await
                .unwrap()
        );
        storage
            .set_key_epoch("test", "epoch", visible.clone(), "key", ShardEpoch(7))
            .await
            .unwrap();
        assert!(matches!(
            storage
                .append(
                    "test",
                    "stale",
                    "entry",
                    visible.clone(),
                    "key",
                    2,
                    vec![2],
                    Some(ShardEpoch(6))
                )
                .await,
            Err(IndexedStorageError::Fenced { .. })
        ));
        let db = storage.namespace_to_db(&visible).await;
        assert!(!FileLifetime::get(storage.root.root_dir.join(&db)).is_poisoned());
        storage
            .delete_with_epoch("test", "delete", visible, "key", Some(ShardEpoch(7)))
            .await
            .unwrap();
        assert!(!storage.root.root_dir.join(db).exists());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn failed_staging_move_rolls_back_and_later_drain_reclaims() {
        for cold in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
            let visible = oplog_namespace("invalid-move");
            let IndexedStorageNamespace::OpLog {
                agent_id,
                agent_mode,
            } = visible.clone()
            else {
                unreachable!()
            };
            let stage = IndexedStorageNamespace::StagedOpLog {
                agent_id,
                agent_mode,
            };
            for id in [1, 3] {
                storage
                    .append(
                        "test",
                        "seed",
                        "entry",
                        stage.clone(),
                        "source",
                        id,
                        vec![id as u8],
                        None,
                    )
                    .await
                    .unwrap();
            }
            let db = storage.namespace_to_db(&stage).await;
            let file = FileLifetime::get(storage.root.root_dir.join(&db));
            if cold {
                storage.root.cache.remove(&db).await;
                file.wait_retired().await;
            }
            assert!(matches!(
                storage
                    .move_if_absent(
                        "test",
                        "move",
                        stage.clone(),
                        "source",
                        visible.clone(),
                        "target",
                        3
                    )
                    .await,
                Err(IndexedStorageError::Other(_))
            ));
            assert!(!file.is_poisoned());
            assert!(storage.root.cache.get(&db).await.is_none());
            assert!(
                !storage
                    .exists("test", "target", visible, "target")
                    .await
                    .unwrap()
            );
            assert_eq!(
                storage
                    .read("test", "source", "entry", stage.clone(), "source", 1, 3)
                    .await
                    .unwrap(),
                vec![(1, vec![1]), (3, vec![3])]
            );
            storage
                .delete("test", "drain", stage, "source")
                .await
                .unwrap();
            for suffix in ["", "-wal", "-shm", "-journal"] {
                assert!(!storage.root.root_dir.join(format!("{db}{suffix}")).exists());
            }
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn reclamation_churn_does_not_retain_empty_epoch_snapshots() {
        use golem_service_base::db::{LabelledPoolTransaction, PoolApi};
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        for agent in 0..9 {
            let mut ns = oplog_namespace(&format!("churn-{agent}"));
            if agent % 3 == 2 {
                let IndexedStorageNamespace::OpLog {
                    agent_id,
                    agent_mode,
                } = ns
                else {
                    unreachable!()
                };
                ns = IndexedStorageNamespace::CompressedOpLog {
                    agent_id,
                    agent_mode,
                    level: 1,
                };
            }
            let epoch = (agent % 3 == 1).then_some(ShardEpoch(17));
            if let Some(epoch) = epoch {
                storage
                    .set_key_epoch("test", "epoch", ns.clone(), "key", epoch)
                    .await
                    .unwrap();
            }
            storage
                .append(
                    "test",
                    "seed",
                    "entry",
                    ns.clone(),
                    "key",
                    1,
                    vec![37],
                    epoch,
                )
                .await
                .unwrap();
            let db = storage.namespace_to_db(&ns).await;
            match agent % 3 {
                0 => storage.delete("test", "drain", ns, "key").await.unwrap(),
                1 => storage
                    .delete_with_epoch("test", "drain", ns, "key", epoch)
                    .await
                    .unwrap(),
                _ => storage
                    .drop_prefix("test", "drain", ns, "key", 1, None)
                    .await
                    .unwrap(),
            }
            assert!(!storage.root.root_dir.join(&db).exists());
            assert!(
                storage
                    .root
                    .epoch_storage()
                    .await
                    .unwrap()
                    .database_epoch_snapshot(&db)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let mut connection = storage
            .root
            .epoch_storage()
            .await
            .unwrap()
            .hold_connection_for_test()
            .await;
        let (rows,): (i64,) = connection
            .fetch_one_as(sqlx::query_as(
                "SELECT COUNT(*) FROM index_storage WHERE namespace = 'database-epochs';",
            ))
            .await
            .unwrap();
        assert_eq!(rows, 0);
        connection.rollback().await.unwrap();
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn deleting_last_snapshot_epoch_cleans_residual_files_before_metadata() {
        for fail_unlink in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
            let ns = oplog_namespace("last-epoch");
            storage
                .append(
                    "test",
                    "seed",
                    "entry",
                    ns.clone(),
                    "key",
                    1,
                    vec![53],
                    None,
                )
                .await
                .unwrap();
            let db = storage.namespace_to_db(&ns).await;
            let file = FileLifetime::get(storage.root.root_dir.join(&db));
            storage.root.cache.remove(&db).await;
            file.wait_retired().await;
            let metadata = storage.root.epoch_storage().await.unwrap();
            metadata
                .save_database_epoch_snapshot(
                    &db,
                    vec![(
                        SqliteIndexedStorage::namespace(ns.clone()),
                        "key".into(),
                        17,
                    )],
                )
                .await
                .unwrap();
            if fail_unlink {
                std::fs::create_dir(storage.root.root_dir.join(format!("{db}-shm"))).unwrap();
            }
            let result = storage
                .delete_with_epoch("test", "delete", ns, "key", Some(ShardEpoch(17)))
                .await;
            if fail_unlink {
                assert!(result.is_err());
                assert!(file.path.exists());
                assert!(file.is_poisoned());
                assert_eq!(
                    metadata.database_epoch_snapshot(&db).await.unwrap(),
                    Some(Vec::new())
                );
            } else {
                result.unwrap();
                assert!(!file.path.exists());
                assert!(!file.is_poisoned());
                assert!(
                    metadata
                        .database_epoch_snapshot(&db)
                        .await
                        .unwrap()
                        .is_none()
                );
            }
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn trailing_read_cleans_empty_snapshot_after_interrupted_unlink_handoff() {
        for residual_file in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
            let ns = oplog_namespace("empty-handoff");
            let db = storage.namespace_to_db(&ns).await;
            if residual_file {
                storage
                    .append(
                        "test",
                        "seed",
                        "entry",
                        ns.clone(),
                        "key",
                        1,
                        vec![97],
                        None,
                    )
                    .await
                    .unwrap();
                storage.root.cache.remove(&db).await;
                FileLifetime::get(storage.root.root_dir.join(&db))
                    .wait_retired()
                    .await;
            }
            let metadata = storage.root.epoch_storage().await.unwrap();
            metadata
                .save_database_epoch_snapshot(&db, Vec::new())
                .await
                .unwrap();
            assert_eq!(storage.length("test", "read", ns, "key").await.unwrap(), 0);
            assert!(!storage.root.root_dir.join(&db).exists());
            assert!(
                metadata
                    .database_epoch_snapshot(&db)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    async fn scan_counts_empty_files_and_reads_each_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        warm(&storage, "durable-oplog-000.db").await;
        let ns = oplog_namespace("scan");
        for key in ["a", "b", "c"] {
            storage
                .append("test", "append", "entry", ns.clone(), key, 1, vec![1], None)
                .await
                .unwrap();
        }
        let meta = IndexedStorageMetaNamespace::Oplog {
            agent_mode: AgentMode::Durable,
        };
        let listing = storage.namespace_db_files(&meta, true).await.unwrap();
        assert!(Arc::ptr_eq(
            &listing,
            &storage.namespace_db_files(&meta, false).await.unwrap()
        ));
        let (resume, keys) = storage
            .scan_stable("test", "scan", meta.clone(), None, None, 1)
            .await
            .unwrap();
        assert!(keys.is_empty());
        assert!(resume.is_some());
        let (_, keys) = storage
            .scan_stable("test", "scan", meta, None, resume, 1)
            .await
            .unwrap();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn reclamation_drains_pools_and_preserves_all_epochs_for_recreation() {
        use golem_service_base::db::LabelledPoolTransaction;
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 2, false);
        let ns = oplog_namespace("drain");
        let IndexedStorageNamespace::OpLog {
            agent_id,
            agent_mode,
        } = ns.clone()
        else {
            unreachable!()
        };
        let stage = IndexedStorageNamespace::StagedOpLog {
            agent_id,
            agent_mode,
        };
        storage
            .set_key_epoch("test", "epoch", ns.clone(), "visible", ShardEpoch(31))
            .await
            .unwrap();
        storage
            .set_key_epoch("test", "epoch", stage.clone(), "unrelated", ShardEpoch(73))
            .await
            .unwrap();
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "visible",
                1,
                vec![9],
                Some(ShardEpoch(31)),
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        let path = storage.root.root_dir.join(&db);
        let file = FileLifetime::get(path.clone());
        let cached = storage.root.cache.get(&db).await.unwrap();
        let connection = cached.hold_connection_for_test().await;
        drop(cached);
        let worker = storage.clone();
        let deleting_ns = ns.clone();
        let deletion = tokio::spawn(async move {
            worker
                .delete("test", "delete", deleting_ns, "visible")
                .await
        });
        let metadata = storage.root.epoch_storage().await.unwrap();
        let snapshot = loop {
            if let Some(epochs) = metadata.database_epoch_snapshot(&db).await.unwrap() {
                break epochs;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(snapshot.len(), 2);
        assert!(storage.root.cache.get(&db).await.is_none());
        assert!(path.exists());
        assert_eq!(file.registrations.load(Ordering::Acquire), 1);
        assert!(!deletion.is_finished());
        connection.rollback().await.unwrap();
        deletion.await.unwrap().unwrap();
        assert_eq!(file.registrations.load(Ordering::Acquire), 0);
        assert!(!path.exists());
        assert!(
            !storage
                .exists("test", "exists", ns.clone(), "visible")
                .await
                .unwrap()
        );
        assert!(!path.exists());
        assert!(
            storage
                .delete_empty_with_epoch(
                    "test",
                    "empty",
                    stage.clone(),
                    "unrelated",
                    Some(ShardEpoch(73))
                )
                .await
                .unwrap()
        );
        storage
            .drop_prefix(
                "test",
                "prefix",
                ns.clone(),
                "visible",
                999,
                Some(ShardEpoch(31)),
            )
            .await
            .unwrap();
        assert!(matches!(
            storage
                .drop_prefix(
                    "test",
                    "prefix",
                    ns.clone(),
                    "missing",
                    1,
                    Some(ShardEpoch(31))
                )
                .await,
            Err(IndexedStorageError::Fenced { actual: None, .. })
        ));
        assert!(matches!(
            storage
                .set_key_epoch("test", "epoch", ns.clone(), "visible", ShardEpoch(30))
                .await,
            Err(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(31)),
                ..
            })
        ));
        storage
            .set_key_epoch("test", "epoch", ns.clone(), "visible", ShardEpoch(41))
            .await
            .unwrap();
        assert!(!path.exists());
        assert!(matches!(
            storage
                .append(
                    "test",
                    "stale",
                    "entry",
                    ns.clone(),
                    "visible",
                    1,
                    vec![3],
                    Some(ShardEpoch(31))
                )
                .await,
            Err(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(41)),
                ..
            })
        ));
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "visible",
                1,
                vec![4],
                Some(ShardEpoch(41)),
            )
            .await
            .unwrap();
        assert!(matches!(
            storage
                .append(
                    "test",
                    "stale",
                    "entry",
                    stage.clone(),
                    "unrelated",
                    1,
                    vec![8],
                    Some(ShardEpoch(41))
                )
                .await,
            Err(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(73)),
                ..
            })
        ));
        assert!(
            metadata
                .database_epoch_snapshot(&db)
                .await
                .unwrap()
                .is_none()
        );
        storage
            .delete_with_epoch("test", "delete", ns, "visible", Some(ShardEpoch(41)))
            .await
            .unwrap();
        assert!(!path.exists());
        let epochs = metadata
            .database_epoch_snapshot(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            epochs,
            vec![(
                SqliteIndexedStorage::namespace(stage),
                "unrelated".into(),
                73
            )]
        );
        assert!(!file.is_poisoned());
    }

    #[test]
    async fn staged_rows_other_keys_and_presence_markers_prevent_reclamation() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("presence");
        let IndexedStorageNamespace::OpLog {
            agent_id,
            agent_mode,
        } = ns.clone()
        else {
            unreachable!()
        };
        let stage = IndexedStorageNamespace::StagedOpLog {
            agent_id,
            agent_mode,
        };
        for (namespace, key, bytes) in [
            (ns.clone(), "visible", vec![1]),
            (stage.clone(), "stage", vec![7]),
            (ns.clone(), "other", vec![11]),
        ] {
            storage
                .append("test", "append", "entry", namespace, key, 1, bytes, None)
                .await
                .unwrap();
        }
        let db = storage.namespace_to_db(&ns).await;
        let path = storage.root.root_dir.join(&db);
        storage
            .delete("test", "delete", ns.clone(), "visible")
            .await
            .unwrap();
        assert!(path.exists());
        storage
            .delete("test", "delete", stage, "stage")
            .await
            .unwrap();
        assert!(path.exists());
        storage
            .drop_prefix("test", "prefix", ns.clone(), "other", 1, None)
            .await
            .unwrap();
        assert!(path.exists());
        assert!(
            storage
                .delete_empty_with_epoch("test", "empty", ns.clone(), "other", None)
                .await
                .unwrap()
        );
        assert!(path.exists());
        // Removing a marker is not a reclamation trigger; a successful destructive operation is.
        storage
            .delete("test", "delete", ns, "nonexistent")
            .await
            .unwrap();
        assert!(!path.exists());
    }

    #[test]
    async fn compressed_prefix_reclaims_and_retains_writer_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let IndexedStorageNamespace::OpLog {
            agent_id,
            agent_mode,
        } = oplog_namespace("compressed")
        else {
            unreachable!()
        };
        let ns = IndexedStorageNamespace::CompressedOpLog {
            agent_id,
            agent_mode,
            level: 3,
        };
        storage
            .set_key_epoch("test", "epoch", ns.clone(), "key", ShardEpoch(17))
            .await
            .unwrap();
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "key",
                5,
                vec![99],
                Some(ShardEpoch(17)),
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        storage
            .drop_prefix("test", "prefix", ns.clone(), "key", 5, Some(ShardEpoch(17)))
            .await
            .unwrap();
        assert!(!storage.root.root_dir.join(&db).exists());
        assert_eq!(
            storage
                .root
                .epoch_storage()
                .await
                .unwrap()
                .database_epoch_snapshot(&db)
                .await
                .unwrap(),
            Some(vec![(
                SqliteIndexedStorage::namespace(ns.clone()),
                "key".into(),
                17
            )])
        );
        assert_eq!(
            storage
                .length("test", "length", ns.clone(), "key")
                .await
                .unwrap(),
            0
        );
        assert!(
            storage
                .delete_empty_with_epoch("test", "empty", ns.clone(), "key", Some(ShardEpoch(17)))
                .await
                .unwrap()
        );
        assert!(!storage.root.root_dir.join(&db).exists());
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "key",
                6,
                vec![101],
                Some(ShardEpoch(17)),
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .read("test", "read", "entry", ns, "key", 1, 9)
                .await
                .unwrap(),
            vec![(6, vec![101])]
        );
    }

    #[test]
    async fn snapshots_are_authoritative_over_leftovers_including_empty_and_partial_restore() {
        for authoritative in [
            Vec::new(),
            vec![
                ("durable-worker-oplog".into(), "writer".into(), 19),
                ("durable-worker-staged-oplog".into(), "other".into(), 83),
            ],
        ] {
            let dir = tempfile::tempdir().unwrap();
            let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
            let ns = oplog_namespace("interrupted");
            let db = storage.namespace_to_db(&ns).await;
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    ns.clone(),
                    "leftover",
                    1,
                    vec![6],
                    None,
                )
                .await
                .unwrap();
            let cached = storage.root.cache.get(&db).await.unwrap();
            // Simulate a prior process stopping with only part of the authoritative epochs restored.
            cached
                .restore_database_epochs(authoritative.iter().take(1).cloned().collect())
                .await
                .unwrap();
            drop(cached);
            storage.root.cache.remove(&db).await;
            let file = FileLifetime::get(storage.root.root_dir.join(&db));
            file.wait_retired().await;
            let metadata = storage.root.epoch_storage().await.unwrap();
            metadata
                .save_database_epoch_snapshot(&db, authoritative.clone())
                .await
                .unwrap();
            assert!(
                !storage
                    .exists("test", "exists", ns.clone(), "leftover")
                    .await
                    .unwrap()
            );
            assert_eq!(file.path.exists(), !authoritative.is_empty());
            assert!(storage.root.cache.get(&db).await.is_none());
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    ns.clone(),
                    "new",
                    1,
                    vec![12],
                    None,
                )
                .await
                .unwrap();
            let cached = storage.root.cache.get(&db).await.unwrap();
            assert!(
                !cached
                    .exists("test", "exists", ns.clone(), "leftover")
                    .await
                    .unwrap()
            );
            cached.delete("test", "delete", ns, "new").await.unwrap();
            let mut restored = cached.empty_database_epochs().await.unwrap().unwrap();
            let mut expected = authoritative;
            restored.sort();
            expected.sort();
            assert_eq!(restored, expected);
            assert!(
                metadata
                    .database_epoch_snapshot(&db)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    async fn poisoned_successful_delete_leaves_empty_file_and_failed_mutation_never_reclaims() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("poisoned-delete");
        storage
            .set_key_epoch("test", "epoch", ns.clone(), "key", ShardEpoch(29))
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        assert!(matches!(
            storage
                .delete_with_epoch("test", "stale", ns.clone(), "key", Some(ShardEpoch(28)))
                .await,
            Err(IndexedStorageError::Fenced { .. })
        ));
        assert!(file.path.exists());
        assert!(!file.is_poisoned());
        assert!(
            storage
                .root
                .epoch_storage()
                .await
                .unwrap()
                .database_epoch_snapshot(&db)
                .await
                .unwrap()
                .is_none()
        );
        file.poison();
        storage
            .delete_with_epoch("test", "delete", ns, "key", Some(ShardEpoch(29)))
            .await
            .unwrap();
        assert!(file.path.exists());
        assert!(
            storage
                .root
                .cache
                .get(&db)
                .await
                .unwrap()
                .empty_database_epochs()
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    async fn absent_destructive_access_never_materializes_and_validates_snapshot_epochs() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("absent");
        let db = storage.namespace_to_db(&ns).await;
        let path = storage.root.root_dir.join(&db);
        storage
            .delete("test", "delete", ns.clone(), "key")
            .await
            .unwrap();
        storage
            .delete_with_epoch("test", "delete", ns.clone(), "key", Some(ShardEpoch(53)))
            .await
            .unwrap();
        assert!(
            storage
                .delete_empty_with_epoch("test", "empty", ns.clone(), "key", None)
                .await
                .unwrap()
        );
        assert!(matches!(
            storage
                .delete_empty_with_epoch("test", "empty", ns.clone(), "key", Some(ShardEpoch(53)))
                .await,
            Err(IndexedStorageError::Fenced { actual: None, .. })
        ));
        assert!(
            storage
                .move_if_absent(
                    "test",
                    "move",
                    ns.clone(),
                    "source",
                    ns.clone(),
                    "target",
                    1
                )
                .await
                .is_err()
        );
        assert!(!path.exists());
        let metadata = storage.root.epoch_storage().await.unwrap();
        metadata
            .save_database_epoch_snapshot(
                &db,
                vec![
                    (
                        SqliteIndexedStorage::namespace(ns.clone()),
                        "key".into(),
                        53,
                    ),
                    (
                        SqliteIndexedStorage::namespace(ns.clone()),
                        "other".into(),
                        97,
                    ),
                ],
            )
            .await
            .unwrap();
        assert!(matches!(
            storage
                .delete_with_epoch("test", "delete", ns.clone(), "key", Some(ShardEpoch(52)))
                .await,
            Err(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(53)),
                ..
            })
        ));
        assert!(
            storage
                .delete_empty_with_epoch("test", "empty", ns.clone(), "key", Some(ShardEpoch(53)))
                .await
                .unwrap()
        );
        storage
            .delete("test", "delete", ns.clone(), "key")
            .await
            .unwrap();
        assert_eq!(
            metadata
                .database_epoch_snapshot(&db)
                .await
                .unwrap()
                .unwrap()
                .len(),
            2
        );
        storage
            .delete_with_epoch("test", "delete", ns.clone(), "key", Some(ShardEpoch(53)))
            .await
            .unwrap();
        assert_eq!(
            metadata.database_epoch_snapshot(&db).await.unwrap(),
            Some(vec![(
                SqliteIndexedStorage::namespace(ns.clone()),
                "other".into(),
                97
            )])
        );
        assert!(
            storage
                .set_key_epoch("test", "epoch", ns, "overflow", ShardEpoch(u64::MAX))
                .await
                .is_err()
        );
        assert!(!path.exists());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn committed_snapshot_save_error_evicts_cache_and_keeps_snapshot_authority() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("committed-save");
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "key",
                1,
                vec![15],
                None,
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        let cached = storage.root.cache.get(&db).await.unwrap();
        storage.root.snapshot_failure.store(1, Ordering::Release);
        assert!(
            storage
                .delete("test", "delete", ns.clone(), "key")
                .await
                .is_err()
        );
        assert!(storage.root.cache.get(&db).await.is_none());
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        assert!(file.path.exists());
        assert!(file.is_poisoned());
        assert_eq!(
            storage
                .root
                .epoch_storage()
                .await
                .unwrap()
                .database_epoch_snapshot(&db)
                .await
                .unwrap(),
            Some(Vec::new())
        );
        // Make the retained physical generation observably disagree with snapshot authority.
        cached
            .append(
                "test",
                "leftover",
                "entry",
                ns.clone(),
                "key",
                2,
                vec![91],
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            cached
                .read("test", "read", "entry", ns.clone(), "key", 1, 9)
                .await
                .unwrap(),
            vec![(2, vec![91])]
        );
        drop(cached);
        file.wait_retired().await;
        assert!(
            storage
                .read("test", "read", "entry", ns, "key", 1, 9)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(file.path.exists());
        assert!(storage.root.cache.get(&db).await.is_none());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn restored_snapshot_delete_failures_do_not_publish_or_append() {
        for boundary in [2, 3] {
            let dir = tempfile::tempdir().unwrap();
            let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
            let ns = oplog_namespace("snapshot-delete");
            let db = storage.namespace_to_db(&ns).await;
            let file = FileLifetime::get(storage.root.root_dir.join(&db));
            let records = vec![(
                SqliteIndexedStorage::namespace(ns.clone()),
                "key".into(),
                67,
            )];
            let metadata = storage.root.epoch_storage().await.unwrap();
            metadata
                .save_database_epoch_snapshot(&db, records.clone())
                .await
                .unwrap();
            storage
                .root
                .snapshot_failure
                .store(boundary, Ordering::Release);
            assert!(
                storage
                    .append(
                        "test",
                        "append",
                        "entry",
                        ns.clone(),
                        "key",
                        1,
                        vec![3],
                        Some(ShardEpoch(67))
                    )
                    .await
                    .is_err()
            );
            assert!(storage.root.cache.get(&db).await.is_none());
            file.wait_retired().await;
            assert!(file.path.exists());
            assert!(file.is_poisoned());
            assert_eq!(
                metadata.database_epoch_snapshot(&db).await.unwrap(),
                if boundary == 2 {
                    Some(records.clone())
                } else {
                    None
                }
            );
            assert!(
                storage
                    .read("test", "read", "entry", ns.clone(), "key", 1, 9)
                    .await
                    .unwrap()
                    .is_empty()
            );
            if boundary == 2 {
                assert!(storage.root.cache.get(&db).await.is_none());
                assert!(
                    storage
                        .append(
                            "test",
                            "retry",
                            "entry",
                            ns,
                            "key",
                            2,
                            vec![4],
                            Some(ShardEpoch(67))
                        )
                        .await
                        .is_err()
                );
            } else {
                let cached = storage.root.cache.get(&db).await.unwrap();
                assert_eq!(cached.empty_database_epochs().await.unwrap(), Some(records));
                drop(cached);
                assert!(matches!(
                    storage
                        .append(
                            "test",
                            "stale",
                            "entry",
                            ns.clone(),
                            "key",
                            2,
                            vec![4],
                            Some(ShardEpoch(66))
                        )
                        .await,
                    Err(IndexedStorageError::Fenced {
                        actual: Some(ShardEpoch(67)),
                        ..
                    })
                ));
                storage
                    .append(
                        "test",
                        "retry",
                        "entry",
                        ns.clone(),
                        "key",
                        2,
                        vec![5],
                        Some(ShardEpoch(67)),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    storage
                        .read("test", "read", "entry", ns, "key", 1, 9)
                        .await
                        .unwrap(),
                    vec![(2, vec![5])]
                );
            }
        }
    }

    #[test]
    async fn metadata_save_failure_evicts_before_poison_and_never_unlinks() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("metadata-failure");
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "key",
                1,
                vec![15],
                None,
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        storage.root.epoch_storage().await.unwrap().close().await;
        assert!(storage.delete("test", "delete", ns, "key").await.is_err());
        assert!(storage.root.cache.get(&db).await.is_none());
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        file.wait_retired().await;
        assert!(file.path.exists());
        assert!(file.is_poisoned());
    }

    #[test]
    async fn restoration_failure_keeps_snapshot_and_registered_unpublished_pools() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("restore-failure");
        let db = storage.namespace_to_db(&ns).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        let record = (
            SqliteIndexedStorage::namespace(ns.clone()),
            "key".into(),
            67,
        );
        let metadata = storage.root.epoch_storage().await.unwrap();
        metadata
            .save_database_epoch_snapshot(&db, vec![record.clone(), record.clone()])
            .await
            .unwrap();
        assert!(
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    ns.clone(),
                    "key",
                    1,
                    vec![3],
                    None
                )
                .await
                .is_err()
        );
        assert!(storage.root.cache.get(&db).await.is_none());
        file.wait_retired().await;
        assert!(file.is_poisoned());
        assert_eq!(
            metadata.database_epoch_snapshot(&db).await.unwrap(),
            Some(vec![record.clone(), record])
        );
        assert!(
            !storage
                .exists("test", "exists", ns.clone(), "key")
                .await
                .unwrap()
        );
        assert!(
            storage
                .append("test", "append", "entry", ns, "key", 1, vec![4], None)
                .await
                .is_err()
        );
        assert!(file.path.exists());
    }

    #[test]
    async fn deletion_dirty_listing_is_stable_for_resume_and_refreshes_next_walk() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let first = oplog_namespace("listing-first");
        let second = oplog_namespace("listing-second");
        for (ns, key) in [(first.clone(), "alpha"), (second.clone(), "zeta")] {
            storage
                .append("test", "append", "entry", ns, key, 1, vec![1], None)
                .await
                .unwrap();
        }
        let meta = IndexedStorageMetaNamespace::Oplog {
            agent_mode: AgentMode::Durable,
        };
        let listing = storage.namespace_db_files(&meta, true).await.unwrap();
        let (resume, keys) = storage
            .scan_stable("test", "scan", meta.clone(), None, None, 1)
            .await
            .unwrap();
        assert_eq!(keys.len(), 1);
        assert!(resume.is_some());
        storage
            .delete("test", "delete", first, "alpha")
            .await
            .unwrap();
        storage
            .delete("test", "delete", second, "zeta")
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &listing,
            &storage.namespace_db_files(&meta, false).await.unwrap()
        ));
        let (last, keys) = storage
            .scan_stable("test", "scan", meta.clone(), None, resume, 1)
            .await
            .unwrap();
        assert!(keys.is_empty());
        assert!(last.is_some()); // A stale filename still consumes the single-file page.
        for db in listing.iter() {
            assert!(!storage.root.root_dir.join(db).exists());
        }
        let fresh = storage.namespace_db_files(&meta, true).await.unwrap();
        assert!(fresh.is_empty());
        assert!(!Arc::ptr_eq(&listing, &fresh));
        let (resume, keys) = storage
            .scan_stable("test", "scan", meta, None, None, 1)
            .await
            .unwrap();
        assert!(resume.is_none());
        assert!(keys.is_empty());
    }
}
