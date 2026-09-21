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

use std::fmt::{self, Debug, Display, Formatter};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use desert_rust::{BinaryDeserializer, BinarySerializer};
use golem_common::model::agent::AgentMode;
use golem_common::model::{AgentId, ShardEpoch};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::repo::RepoError;
use uuid::Uuid;

pub mod memory;
pub mod multi_sqlite;
pub mod postgres;
pub mod redis;
pub mod sqlite;

pub type ScanCursor = u64;

/// Typed error for [`IndexedStorage`] operations.
///
/// `Transient` errors are safe to retry. `Indeterminate` errors can only be retried by callers
/// that reconcile a possibly committed write. `Conflict` and `Other` errors are not retriable.
#[derive(Debug, Clone)]
pub enum IndexedStorageError {
    /// The operation did not take effect, or is idempotent, so the caller may retry it.
    Transient(String),
    /// A write may have taken effect, so a retry must reconcile the stored value on conflict.
    Indeterminate(String),
    /// The requested index already exists.
    Conflict(String),
    /// Permanent error — data issue or schema error. Caller should not retry.
    Other(String),
    /// The write was refused because the epoch it asserted is not the one recorded for the key,
    /// or the record is held by another writer at that epoch.
    ///
    /// Never retriable - retrying cannot make the record name this writer again. It is not a
    /// failure of the storage either: the write was rejected on purpose, by a newer claim.
    Fenced {
        key: String,
        expected: ShardEpoch,
        actual: Option<ShardEpoch>,
        /// The stored epoch equals the asserted one but another writer recorded it, so the epoch
        /// alone no longer says who may write - see [`WriterId`].
        writer_conflict: bool,
    },
}

/// The process behind a write, recorded alongside the epoch it asserts.
///
/// One value per process, kept for the life of the process, so that whatever issues epochs can
/// re-issue one without this process losing the keys it already holds at it.
///
/// What it buys is the one thing an epoch cannot say by itself: which of two writers presenting
/// the same number recorded it. Whoever did may go on writing at that epoch; anybody else is
/// refused, and the refusal says the epoch is shared rather than stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WriterId(pub Uuid);

impl WriterId {
    /// This process's writer identity, created once on first use.
    pub fn process() -> Self {
        static PROCESS: OnceLock<WriterId> = OnceLock::new();
        *PROCESS.get_or_init(|| WriterId(Uuid::new_v4()))
    }
}

impl Display for WriterId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl IndexedStorageError {
    pub fn is_retriable(&self) -> bool {
        matches!(self, IndexedStorageError::Transient(_))
    }
}

impl Display for IndexedStorageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            IndexedStorageError::Transient(msg) => write!(f, "Transient storage error: {msg}"),
            IndexedStorageError::Indeterminate(msg) => {
                write!(f, "Indeterminate storage error: {msg}")
            }
            IndexedStorageError::Conflict(msg) => write!(f, "Storage conflict: {msg}"),
            IndexedStorageError::Other(msg) => write!(f, "Storage error: {msg}"),
            IndexedStorageError::Fenced {
                key,
                expected,
                actual,
                writer_conflict,
            } => match actual {
                Some(actual) if *writer_conflict => write!(
                    f,
                    "Write fenced for key {key}: asserted epoch {expected}, \
                     which another writer holds - the stored epoch is {actual}"
                ),
                Some(actual) => write!(
                    f,
                    "Write fenced for key {key}: asserted epoch {expected}, \
                     the stored epoch is {actual}"
                ),
                None => write!(
                    f,
                    "Write fenced for key {key}: asserted epoch {expected}, \
                     but no epoch is stored for it"
                ),
            },
        }
    }
}

impl std::error::Error for IndexedStorageError {}

impl From<String> for IndexedStorageError {
    fn from(s: String) -> Self {
        IndexedStorageError::Other(s)
    }
}

/// Carries a fence rejection out of a transaction closure.
///
/// [`Pool::with_tx_err`] requires its error type to be `From<RepoError>`, and
/// [`IndexedStorageError`] deliberately is not: each backend converts a `RepoError` through its
/// own classifier, which decides whether the failure is retriable and annotates a unique
/// violation as a possible ownership mismatch. A blanket `From` would flatten all of that into
/// `Other`. So the closure fails with this instead, and the backend maps it back at the boundary
/// with the classifier it would have used anyway - which keeps `with_tx_err`'s labelled rollback
/// and its metrics rather than hand-rolling `begin`/`rollback` at every early return.
#[derive(Debug)]
pub(crate) enum FencedTxError {
    Repo(RepoError),
    Fenced {
        key: String,
        expected: ShardEpoch,
        actual: Option<ShardEpoch>,
        writer_conflict: bool,
    },
    /// A stored value the schema should have made impossible - a negative epoch, say. Not a fence:
    /// nobody took the key over, the row itself cannot be trusted.
    Corrupt(String),
}

impl From<RepoError> for FencedTxError {
    fn from(err: RepoError) -> Self {
        FencedTxError::Repo(err)
    }
}

impl FencedTxError {
    /// `classify` is the backend's own `RepoError` classifier.
    pub(crate) fn into_indexed_storage_error(
        self,
        classify: fn(RepoError) -> IndexedStorageError,
    ) -> IndexedStorageError {
        match self {
            FencedTxError::Repo(err) => classify(err),
            FencedTxError::Fenced {
                key,
                expected,
                actual,
                writer_conflict,
            } => IndexedStorageError::Fenced {
                key,
                expected,
                actual,
                writer_conflict,
            },
            FencedTxError::Corrupt(msg) => IndexedStorageError::Other(msg),
        }
    }
}

/// Where a [`IndexedStorage::scan_stable`] walk left off. Only the backend that produced it can
/// read it; a caller passes it back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanResume {
    /// The last position reached in the backend's walk order: usually the last key handed back,
    /// but the multi-SQLite backend names the last file it finished.
    Marker(String),
    /// The iteration cursor of a backend with no key order to seek in.
    Cursor(ScanCursor),
}

impl ScanResume {
    /// The marker this token carries, or an error if `backend` was handed a token it did not
    /// produce.
    pub fn into_marker(self, backend: &str) -> Result<String, IndexedStorageError> {
        match self {
            ScanResume::Marker(marker) => Ok(marker),
            ScanResume::Cursor(_) => Err(Self::foreign(backend)),
        }
    }

    /// The cursor this token carries, or an error if `backend` was handed a token it did not
    /// produce.
    pub fn into_cursor(self, backend: &str) -> Result<ScanCursor, IndexedStorageError> {
        match self {
            ScanResume::Cursor(cursor) => Ok(cursor),
            ScanResume::Marker(_) => Err(Self::foreign(backend)),
        }
    }

    fn foreign(backend: &str) -> IndexedStorageError {
        IndexedStorageError::Other(format!(
            "{backend} indexed storage was handed a resume token it did not produce"
        ))
    }
}

/// Generic indexed storage interface
///
/// The storage holds indexes identified by keys. Each index is a sequence of entries,
/// where each entry has a numeric identifier and an arbitrary binary payload. The numeric
/// identifiers are unique and monotonically increasing within each index, but not necessarily
/// contiguous.
#[async_trait]
pub trait IndexedStorage: Debug + Sync {
    /// Gets the number of available replicas in the storage cluster
    async fn number_of_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> Result<u8, IndexedStorageError>;

    /// Wait until all write operations are propagated to at least the given number of replicas,
    /// or the maximum `number_of_replicas` if it is smaller.
    async fn wait_for_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, IndexedStorageError>;

    /// Checks if a key exists in the storage
    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError>;

    /// Returns keys in the given meta-namespace, optionally filtered by key prefix, in a
    /// paginated way. If there are no more pages to scan, the returned cursor will be 0.
    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), IndexedStorageError>;

    /// Pages the keys of a namespace so that the caller can delete the keys it was handed without
    /// the walk skipping any. [`Self::scan`] cannot: its cursor is a position, so a delete behind
    /// it makes the next page step over keys nothing has seen.
    ///
    /// `resume` is `None` for the first page, then whatever the previous call returned; the
    /// returned token is `None` once the walk is done. A backend that pages in key order only
    /// learns that from a short page, so it may take one extra, empty call. A key present for the
    /// whole walk is returned at least once; a key the caller deletes may or may not be.
    async fn scan_stable(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<ScanResume>,
        count: u64,
    ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError>;

    /// Appends an entry to the given key with the given id. `expected_epoch` is checked as in
    /// [`Self::append_many`].
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
    ) -> Result<(), IndexedStorageError>;

    /// Appends multiple entries to the given key with the given ids, all or nothing.
    ///
    /// `expected_epoch` is the writer generation the caller believes it holds for this key. It is
    /// checked against the record [`Self::set_key_epoch`] keeps, atomically with the insert, and
    /// the whole batch is refused with [`IndexedStorageError::Fenced`] unless the record holds
    /// exactly that epoch and names this writer. A key with no record refuses too. `None` asserts
    /// nothing and is for writers that hold no epoch. The check is once per call, never per entry.
    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError>;

    /// Gets the number of entries in the index of the given key
    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError>;

    /// Deletes the index of the given key
    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError>;

    /// Reads a closed range of entries from the index of the given key
    async fn read(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError>;

    /// Gets the first entry in the index of the given key
    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError>;

    /// Gets the last entry in the index of the given key
    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError>;

    /// Gets the id of the last entry in the index of the given key, without reading its payload,
    /// which can be arbitrarily large.
    async fn last_id(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError>;

    /// Gets the entry with the closest id to the given id in the index of the given key,
    /// in a way that `id` is less or equal to the id of the returned entry.
    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError>;

    /// Deletes the entry with the closest id to the given id in the index of the given key,
    /// in a way that `last_dropped_id` is greater to the id of the deleted entries.
    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), IndexedStorageError>;

    /// Records the writer generation for the given key: `epoch`, and this process as the writer
    /// holding it. A monotonic compare-and-set - accepted when `epoch` is above the stored one, or
    /// equal to it and recorded by this same writer, and refused with
    /// [`IndexedStorageError::Fenced`] otherwise. Inserts the record if the key has none.
    ///
    /// Monotonic rather than a plain overwrite so that a writer holding a stale epoch cannot walk
    /// the record backwards and let itself back in. Equality is what the writer ([`WriterId`])
    /// settles: the process that already holds the epoch may record it again, while another
    /// process presenting the same epoch is refused rather than sharing it.
    ///
    /// That holds only for a key that already has a record. A key with none accepts any epoch,
    /// whether it was never written or its record was removed by [`Self::delete_key_epoch`].
    async fn set_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError>;

    /// Forgets the writer generation recorded for the given key, so that an append still asserting
    /// the old epoch is refused by the absent record. Meant to run before the key's entries are
    /// deleted. A later [`Self::set_key_epoch`] at any epoch writes a new record. Idempotent.
    async fn delete_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError>;
}

pub trait IndexedStorageLabelledApi<T: IndexedStorage + ?Sized> {
    fn with(&self, svc_name: &'static str, api_name: &'static str)
    -> LabelledIndexedStorage<'_, T>;

    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityIndexedStorage<'_, T>;
}

impl<T: ?Sized + IndexedStorage> IndexedStorageLabelledApi<T> for T {
    fn with(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> LabelledIndexedStorage<'_, T> {
        LabelledIndexedStorage::new(svc_name, api_name, self)
    }
    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityIndexedStorage<'_, T> {
        LabelledEntityIndexedStorage::new(svc_name, api_name, entity_name, self)
    }
}

pub struct LabelledIndexedStorage<'a, S: IndexedStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + IndexedStorage> LabelledIndexedStorage<'a, S> {
    pub fn new(svc_name: &'static str, api_name: &'static str, storage: &'a S) -> Self {
        Self {
            svc_name,
            api_name,
            storage,
        }
    }

    pub async fn number_of_replicas(&self) -> Result<u8, IndexedStorageError> {
        self.storage
            .number_of_replicas(self.svc_name, self.api_name)
            .await
    }

    pub async fn wait_for_replicas(
        &self,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        self.storage
            .wait_for_replicas(self.svc_name, self.api_name, replicas, timeout)
            .await
    }

    pub async fn exists(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn scan(
        &self,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), IndexedStorageError> {
        self.storage
            .scan(
                self.svc_name,
                self.api_name,
                namespace,
                prefix,
                cursor,
                count,
            )
            .await
    }

    pub async fn scan_stable(
        &self,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<ScanResume>,
        count: u64,
    ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError> {
        self.storage
            .scan_stable(
                self.svc_name,
                self.api_name,
                namespace,
                prefix,
                resume,
                count,
            )
            .await
    }

    pub async fn length(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        self.storage
            .length(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn delete(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        self.storage
            .delete(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn drop_prefix(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), IndexedStorageError> {
        self.storage
            .drop_prefix(
                self.svc_name,
                self.api_name,
                namespace,
                key,
                last_dropped_id,
            )
            .await
    }
}

pub struct LabelledEntityIndexedStorage<'a, S: IndexedStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    entity_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + IndexedStorage> LabelledEntityIndexedStorage<'a, S> {
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

    /// Appends an entry to the given key with the given id, serializing the value first
    pub async fn append<V: BinarySerializer>(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: &V,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.storage
            .append(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
                serialize(value).map_err(IndexedStorageError::Other)?,
                expected_epoch,
            )
            .await
    }

    /// Appends an entry to the given key with the given id
    pub async fn append_raw(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.storage
            .append(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
                value,
                expected_epoch,
            )
            .await
    }

    /// Appends multiple entries to the given key with the given id.
    /// Returns the total number of bytes written to storage.
    pub async fn append_many<V: BinarySerializer>(
        &self,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: &[(u64, &V)],
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<u64, IndexedStorageError> {
        let mut serialized_pairs = Vec::with_capacity(pairs.len());
        let mut total_bytes = 0u64;
        for (id, value) in pairs {
            let bytes = serialize(value).map_err(IndexedStorageError::Other)?;
            total_bytes += bytes.len() as u64;
            serialized_pairs.push((*id, Bytes::from(bytes)));
        }
        self.append_many_raw(namespace, key, serialized_pairs.into(), expected_epoch)
            .await?;
        Ok(total_bytes)
    }

    /// Appends an already serialized batch without repeating serialization in a retry loop.
    pub async fn append_many_raw(
        &self,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.storage
            .append_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                pairs,
                expected_epoch,
            )
            .await
    }

    /// Reads a closed range of entries from the index of the given key, deserializing each entry
    pub async fn read<V: BinaryDeserializer>(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, V)>, IndexedStorageError> {
        let values = self
            .storage
            .read(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                start_id,
                end_id,
            )
            .await?;
        values
            .into_iter()
            .map(|(idx, bytes)| deserialize::<V>(&bytes).map(|v| (idx, v)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(IndexedStorageError::Other)
    }

    /// Reads a closed range of entries from the index of the given key, returning the raw bytes
    pub async fn read_raw(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        from: u64,
        count: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
        self.storage
            .read(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                from,
                count,
            )
            .await
    }

    /// Gets the first entry in the index of the given key, returning as raw bytes
    pub async fn first_raw(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        self.storage
            .first(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await
    }

    /// Gets the first entry in the index of the given key, deserializing the value
    pub async fn first<V: BinaryDeserializer>(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, V)>, IndexedStorageError> {
        if let Some((id, bytes)) = self
            .storage
            .first(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?
        {
            Ok(Some((
                id,
                deserialize::<V>(&bytes).map_err(IndexedStorageError::Other)?,
            )))
        } else {
            Ok(None)
        }
    }

    /// Gets the first entry in the index of the given key, returning only its id
    pub async fn first_id(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        self.first_raw(namespace, key).await.map(|r| r.map(|p| p.0))
    }

    /// Gets the last entry in the index of the given key, deserializing the value
    pub async fn last<V: BinaryDeserializer>(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, V)>, IndexedStorageError> {
        if let Some((id, bytes)) = self
            .storage
            .last(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?
        {
            Ok(Some((
                id,
                deserialize::<V>(&bytes).map_err(IndexedStorageError::Other)?,
            )))
        } else {
            Ok(None)
        }
    }

    /// Gets the last entry in the index of the given key, returning only its id
    pub async fn last_id(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        self.storage
            .last_id(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await
    }

    /// Gets the entry with the closest id to the given id in the index of the given key,
    /// in a way that `id` is less or equal to the id of the returned entry, returning as raw bytes
    pub async fn closest_raw(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        self.storage
            .closest(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
            )
            .await
    }

    /// Gets the entry with the closest id to the given id in the index of the given key,
    /// in a way that `id` is less or equal to the id of the returned entry, deserializing the value
    pub async fn closest<V: BinaryDeserializer>(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, V)>, IndexedStorageError> {
        if let Some((id, bytes)) = self
            .storage
            .closest(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
            )
            .await?
        {
            Ok(Some((
                id,
                deserialize::<V>(&bytes).map_err(IndexedStorageError::Other)?,
            )))
        } else {
            Ok(None)
        }
    }

    /// Gets the entry with the closest id to the given id in the index of the given key,
    /// in a way that `id` is less or equal to the id of the returned entry, returning only its id
    pub async fn closest_id(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<u64>, IndexedStorageError> {
        self.closest_raw(namespace, key, id)
            .await
            .map(|r| r.map(|p| p.0))
    }
}

/// Various namespaces for indexed storage
#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum IndexedStorageNamespace {
    OpLog {
        agent_id: AgentId,
        agent_mode: AgentMode,
    },
    CompressedOpLog {
        agent_id: AgentId,
        agent_mode: AgentMode,
        level: usize,
    },
}

/// Various namespaces for operations working on multiple indexed storage namespaces such as scan
#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum IndexedStorageMetaNamespace {
    Oplog { agent_mode: AgentMode },
    CompressedOplog { agent_mode: AgentMode, level: usize },
}

/// The resume token for a page of an ordered walk: the last key handed back, or `None` once a
/// short page shows the namespace is exhausted. Shared by every backend that pages in key order.
pub fn last_key_resume(keys: &[String], count: u64) -> Option<ScanResume> {
    if (keys.len() as u64) < count {
        None
    } else {
        keys.last().map(|key| ScanResume::Marker(key.clone()))
    }
}

/// Returns the symmetric per-mode prefix used by all indexed-storage backends.
pub fn agent_mode_prefix(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Durable => "durable",
        AgentMode::Ephemeral => "ephemeral",
    }
}
