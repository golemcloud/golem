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
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use desert_rust::{BinaryDeserializer, BinarySerializer};
use golem_common::model::AgentId;
use golem_common::model::agent::AgentMode;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::repo::is_transient_sqlx_error;

pub mod memory;
pub mod multi_sqlite;
pub mod postgres;
pub mod redis;
pub mod sqlite;

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
    /// A scan resume token is not valid for this backend.
    InvalidResume(String),
    /// Permanent error — data issue or schema error. Caller should not retry.
    Other(String),
}

impl IndexedStorageError {
    /// Classifies failures that happen while a lazily-created backend is opened or migrated.
    /// The indexed operation has not started yet, so a transient cause is safe to retry.
    pub fn initialization_failed(context: &str, error: anyhow::Error) -> Self {
        let transient = error
            .chain()
            .filter_map(|cause| cause.downcast_ref::<sqlx::Error>())
            .any(is_transient_sqlx_error);
        let message = format!("{context}: {error:#}");
        if transient {
            Self::Transient(message)
        } else {
            Self::Other(message)
        }
    }

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
            IndexedStorageError::InvalidResume(msg) => write!(f, "Invalid scan resume: {msg}"),
            IndexedStorageError::Other(msg) => write!(f, "Storage error: {msg}"),
        }
    }
}

impl std::error::Error for IndexedStorageError {}

impl From<String> for IndexedStorageError {
    fn from(s: String) -> Self {
        IndexedStorageError::Other(s)
    }
}

/// Where a [`IndexedStorage::scan_stable`] walk left off. Only the backend that produced it can
/// read it; a caller passes it back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum ScanResume {
    /// The last position reached in the backend's walk order: usually the last key handed back,
    /// but the multi-SQLite backend names the last file it finished.
    Marker(String),
    /// The iteration cursor of a backend with no key order to seek in.
    Cursor(u64),
}

impl ScanResume {
    /// The marker this token carries, or an error if `backend` was handed a token it did not
    /// produce.
    pub fn into_marker(self, backend: &str) -> Result<String, IndexedStorageError> {
        match self {
            ScanResume::Marker(marker) if marker.contains('\0') => Err(
                IndexedStorageError::InvalidResume(format!("{backend} marker contains NUL")),
            ),
            ScanResume::Marker(marker) => Ok(marker),
            ScanResume::Cursor(_) => Err(Self::foreign(backend)),
        }
    }

    /// The cursor this token carries, or an error if `backend` was handed a token it did not
    /// produce.
    pub fn into_cursor(self, backend: &str) -> Result<u64, IndexedStorageError> {
        match self {
            ScanResume::Cursor(cursor) => Ok(cursor),
            ScanResume::Marker(_) => Err(Self::foreign(backend)),
        }
    }

    fn foreign(backend: &str) -> IndexedStorageError {
        IndexedStorageError::InvalidResume(format!(
            "{backend} indexed storage was handed a resume token it did not produce"
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StableScanKeyBounds {
    lower: String,
    inclusive: bool,
    upper: Option<String>,
}

fn stable_scan_key_bounds(
    prefix: Option<&str>,
    resume: Option<ScanResume>,
    backend: &str,
) -> Result<StableScanKeyBounds, IndexedStorageError> {
    let prefix = prefix.unwrap_or_default();
    let marker = resume
        .map(|resume| resume.into_marker(backend))
        .transpose()?
        .unwrap_or_default();
    let inclusive = prefix > marker.as_str();

    Ok(StableScanKeyBounds {
        lower: if inclusive {
            prefix.to_string()
        } else {
            marker
        },
        inclusive,
        upper: scan_prefix_upper_bound(prefix),
    })
}

fn scan_prefix_upper_bound(prefix: &str) -> Option<String> {
    for (index, ch) in prefix.char_indices().rev() {
        if ch == char::MAX {
            continue;
        }

        let mut next = ch as u32 + 1;
        if next == 0xD800 {
            next = 0xE000;
        }

        let mut upper = prefix[..index].to_string();
        upper.push(char::from_u32(next).expect("successor must be a valid Unicode scalar"));
        return Some(upper);
    }

    None
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

    /// Checks if a key exists, including an empty key retained by `drop_prefix`.
    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError>;

    /// Pages the keys of a namespace so that the caller can delete the keys it was handed without
    /// the walk skipping any.
    ///
    /// `prefix` is a literal, case-sensitive UTF-8 prefix. `resume` is `None` for the first page,
    /// then whatever the previous call returned; the returned token is `None` once the walk is
    /// done. A backend that pages in key order only learns that from a short page, so it may take
    /// one extra, empty call. A key present for the whole walk is returned at least once; a key the
    /// caller deletes may or may not be.
    async fn scan_stable(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<ScanResume>,
        count: u64,
    ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError>;

    /// Appends an entry to the given key with the given id
    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
    ) -> Result<(), IndexedStorageError>;

    /// Appends multiple entries to the given key with the given id
    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
    ) -> Result<(), IndexedStorageError> {
        for (id, value) in pairs.iter() {
            self.append(
                svc_name,
                api_name,
                entity_name,
                (*namespace).clone(),
                key,
                *id,
                value.to_vec(),
            )
            .await?;
        }
        Ok(())
    }

    /// Atomically moves a stopped source index to a previously absent target index. The source
    /// must contain exactly ids 1..=expected_last_id. Returns false without mutation if the target
    /// exists. An invalid source remains unchanged.
    /// An indeterminate result must be reconciled against the target, not blindly retried.
    async fn move_if_absent(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _source_namespace: IndexedStorageNamespace,
        _source_key: &str,
        _target_namespace: IndexedStorageNamespace,
        _target_key: &str,
        _expected_last_id: u64,
    ) -> Result<bool, IndexedStorageError> {
        Err(IndexedStorageError::Other(
            "atomic index moves are unsupported".to_string(),
        ))
    }

    /// Gets the number of entries in the index of the given key
    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError>;

    /// Deletes the index and its key existence, allowing the name to be reused.
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
    /// The key remains present even when every entry is removed. Missing keys stay missing.
    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
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
    fn record(&self, operation: &'static str) {
        golem_service_base::metrics::storage::record_logical_operation(
            "indexed",
            operation,
            self.svc_name,
            self.api_name,
            "",
        );
    }

    pub fn new(svc_name: &'static str, api_name: &'static str, storage: &'a S) -> Self {
        Self {
            svc_name,
            api_name,
            storage,
        }
    }

    pub async fn number_of_replicas(&self) -> Result<u8, IndexedStorageError> {
        self.record("number_of_replicas");
        self.storage
            .number_of_replicas(self.svc_name, self.api_name)
            .await
    }

    pub async fn wait_for_replicas(
        &self,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        self.record("wait_for_replicas");
        self.storage
            .wait_for_replicas(self.svc_name, self.api_name, replicas, timeout)
            .await
    }

    pub async fn exists(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
        self.record("exists");
        self.storage
            .exists(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn move_if_absent(
        &self,
        source_namespace: IndexedStorageNamespace,
        source_key: &str,
        target_namespace: IndexedStorageNamespace,
        target_key: &str,
        expected_last_id: u64,
    ) -> Result<bool, IndexedStorageError> {
        self.record("move_if_absent");
        self.storage
            .move_if_absent(
                self.svc_name,
                self.api_name,
                source_namespace,
                source_key,
                target_namespace,
                target_key,
                expected_last_id,
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
        self.record("scan");
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
        self.record("length");
        self.storage
            .length(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn delete(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        self.record("delete");
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
        self.record("drop_prefix");
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
    fn record(&self, operation: &'static str) {
        golem_service_base::metrics::storage::record_logical_operation(
            "indexed",
            operation,
            self.svc_name,
            self.api_name,
            self.entity_name,
        );
    }

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
    ) -> Result<(), IndexedStorageError> {
        self.record("append");
        self.storage
            .append(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
                serialize(value).map_err(IndexedStorageError::Other)?,
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
    ) -> Result<(), IndexedStorageError> {
        self.record("append");
        self.storage
            .append(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
                value,
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
    ) -> Result<u64, IndexedStorageError> {
        let mut serialized_pairs = Vec::with_capacity(pairs.len());
        let mut total_bytes = 0u64;
        for (id, value) in pairs {
            let bytes = serialize(value).map_err(IndexedStorageError::Other)?;
            total_bytes += bytes.len() as u64;
            serialized_pairs.push((*id, Bytes::from(bytes)));
        }
        self.append_many_raw(namespace, key, serialized_pairs.into())
            .await?;
        Ok(total_bytes)
    }

    /// Appends an already serialized batch without repeating serialization in a retry loop.
    pub async fn append_many_raw(
        &self,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
    ) -> Result<(), IndexedStorageError> {
        self.record("append_many");
        self.storage
            .append_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                pairs,
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
        self.record("read");
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
        self.record("read");
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
        self.record("first");
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
        self.record("first");
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
        self.record("last");
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
        self.record("last");
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
        self.record("closest");
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
        self.record("closest");
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
    StagedOpLog {
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

#[cfg(test)]
mod tests {
    use super::{IndexedStorageError, ScanResume, scan_prefix_upper_bound, stable_scan_key_bounds};
    use proptest::prelude::*;
    use test_r::test;

    test_r::enable!();

    #[test]
    fn transient_indexed_storage_initialization_failure_is_retryable() {
        let error = IndexedStorageError::initialization_failed(
            "pool initialization failed",
            anyhow::Error::from(sqlx::Error::Io(std::io::Error::from(
                std::io::ErrorKind::WouldBlock,
            ))),
        );

        assert!(matches!(error, IndexedStorageError::Transient(_)));
        assert!(error.is_retriable());
    }

    #[test]
    fn permanent_indexed_storage_initialization_failure_is_not_retried() {
        let error = IndexedStorageError::initialization_failed(
            "migration failed",
            anyhow::Error::from(sqlx::Error::RowNotFound),
        );

        assert!(matches!(error, IndexedStorageError::Other(_)));
        assert!(!error.is_retriable());
    }

    #[test]
    fn scan_prefix_upper_bound_handles_unicode_boundaries() {
        let cases = [
            ("", None),
            ("abc", Some("abd")),
            ("\u{7f}", Some("\u{80}")),
            ("\u{ff}", Some("\u{100}")),
            ("\u{7ff}", Some("\u{800}")),
            ("\u{d7ff}", Some("\u{e000}")),
            ("\u{ffff}", Some("\u{10000}")),
            ("a\u{10ffff}", Some("b")),
            ("\u{10ffff}a", Some("\u{10ffff}b")),
            ("\u{10ffff}\u{10ffff}", None),
        ];

        for (prefix, expected) in cases {
            assert_eq!(scan_prefix_upper_bound(prefix).as_deref(), expected);
        }
    }

    #[test]
    fn stable_scan_uses_prefix_as_inclusive_first_lower_bound() {
        assert_eq!(
            stable_scan_key_bounds(Some("component:"), None, "test").unwrap(),
            super::StableScanKeyBounds {
                lower: "component:".to_string(),
                inclusive: true,
                upper: Some("component;".to_string()),
            }
        );
        assert_eq!(
            stable_scan_key_bounds(
                Some("component:"),
                Some(ScanResume::Marker("component:agent".to_string())),
                "test",
            )
            .unwrap(),
            super::StableScanKeyBounds {
                lower: "component:agent".to_string(),
                inclusive: false,
                upper: Some("component;".to_string()),
            }
        );
    }

    proptest! {
        #[test]
        fn prefix_interval_matches_starts_with(prefix in any::<String>(), keys in prop::collection::vec(any::<String>(), 0..64)) {
            let upper = scan_prefix_upper_bound(&prefix);
            for key in keys {
                let in_interval = key.as_str() >= prefix.as_str()
                    && upper.as_ref().is_none_or(|upper| key.as_str() < upper.as_str());
                prop_assert_eq!(in_interval, key.starts_with(&prefix));
            }
        }

        #[test]
        fn effective_bounds_match_prefix_and_resume(
            prefix in any::<String>(),
            marker in any::<String>().prop_filter("markers cannot contain NUL", |value| !value.contains('\0')),
            keys in prop::collection::vec(any::<String>(), 0..64),
        ) {
            let bounds = stable_scan_key_bounds(
                Some(&prefix),
                Some(ScanResume::Marker(marker.clone())),
                "test",
            ).unwrap();

            for key in keys {
                let above_lower = if bounds.inclusive {
                    key.as_str() >= bounds.lower.as_str()
                } else {
                    key.as_str() > bounds.lower.as_str()
                };
                let in_bounds = above_lower
                    && bounds.upper.as_ref().is_none_or(|upper| key.as_str() < upper.as_str());
                prop_assert_eq!(in_bounds, key.starts_with(&prefix) && key > marker);
            }
        }
    }
}
