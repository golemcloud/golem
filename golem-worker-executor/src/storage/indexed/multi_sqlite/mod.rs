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
mod tests;
