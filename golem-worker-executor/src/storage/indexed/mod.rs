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

use std::fmt::Debug;
use std::time::Duration;

use async_trait::async_trait;
use desert_rust::{BinaryDeserializer, BinarySerializer};
use golem_common::model::WorkerId;
use golem_common::serialization::{deserialize, serialize};

pub mod memory;
pub mod multi_sqlite;
pub mod redis;
pub mod sqlite;

pub type ScanCursor = u64;

/// Generic indexed storage interface
///
/// The storage holds indexes identified by keys. Each index is a sequence of entries,
/// where each entry has a numeric identifier and an arbitrary binary payload. The numeric
/// identifiers are unique and monotonically increasing within each index, but not necessarily
/// contiguous.
///
#[async_trait]
pub trait IndexedStorage: Debug + Sync {
    /// Gets the number of available replicas in the storage cluster
    async fn number_of_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> Result<u8, String>;

    /// Wait until all write operations are propagated to at least the given number of replicas,
    /// or the maximum `number_of_replicas` if it is smaller.
    async fn wait_for_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, String>;

    /// Checks if a key exists in the storage
    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, String>;

    /// Returns all the keys matching the given pattern, in a paginated way. If there are
    /// no more pages to scan, the returned cursor will be 0.
    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String>;

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
    ) -> Result<(), String>;

    /// Appends multiple entries to the given key with the given id
    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        pairs: Vec<(u64, Vec<u8>)>,
    ) -> Result<(), String> {
        for (id, value) in pairs {
            self.append(
                svc_name,
                api_name,
                entity_name,
                namespace.clone(),
                key,
                id,
                value,
            )
            .await?;
        }
        Ok(())
    }

    /// Gets the number of entries in the index of the given key
    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String>;

    /// Deletes the index of the given key
    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), String>;

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
    ) -> Result<Vec<(u64, Vec<u8>)>, String>;

    /// Gets the first entry in the index of the given key
    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, String>;

    /// Gets the last entry in the index of the given key
    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, String>;

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
    ) -> Result<Option<(u64, Vec<u8>)>, String>;

    /// Deletes the entry with the closest id to the given id in the index of the given key,
    /// in a way that `last_dropped_id` is greater to the id of the deleted entries.
    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), String>;
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

    pub async fn number_of_replicas(&self) -> Result<u8, String> {
        self.storage
            .number_of_replicas(self.svc_name, self.api_name)
            .await
    }

    pub async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<u8, String> {
        self.storage
            .wait_for_replicas(self.svc_name, self.api_name, replicas, timeout)
            .await
    }

    pub async fn exists(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn scan(
        &self,
        namespace: IndexedStorageMetaNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        self.storage
            .scan(
                self.svc_name,
                self.api_name,
                namespace,
                pattern,
                cursor,
                count,
            )
            .await
    }

    pub async fn length(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String> {
        self.storage
            .length(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn delete(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        self.storage
            .delete(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn drop_prefix(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), String> {
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
    ) -> Result<(), String> {
        self.storage
            .append(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                id,
                serialize(value)?,
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
    ) -> Result<(), String> {
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

    /// Appends multiple entries to the given key with the given id, serializing the value first
    pub async fn append_many<V: BinarySerializer>(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        pairs: &[(u64, &V)],
    ) -> Result<(), String> {
        let mut serialized_pairs = Vec::with_capacity(pairs.len());
        for (id, value) in pairs {
            serialized_pairs.push((*id, serialize(value)?));
        }
        self.storage
            .append_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                serialized_pairs,
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
    ) -> Result<Vec<(u64, V)>, String> {
        self.storage
            .read(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                start_id,
                end_id,
            )
            .await
            .and_then(|values| {
                values
                    .into_iter()
                    .map(|(idx, bytes)| deserialize::<V>(&bytes).map(|v| (idx, v)))
                    .collect::<Result<Vec<_>, _>>()
            })
    }

    /// Reads a closed range of entries from the index of the given key, returning the raw bytes
    pub async fn read_raw(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        from: u64,
        count: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, String> {
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
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
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
    ) -> Result<Option<(u64, V)>, String> {
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
            Ok(Some((id, deserialize::<V>(&bytes)?)))
        } else {
            Ok(None)
        }
    }

    /// Gets the first entry in the index of the given key, returning only its id
    pub async fn first_id(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, String> {
        self.first_raw(namespace, key).await.map(|r| r.map(|p| p.0))
    }

    /// Gets the last entry in the index of the given key, returning as raw bytes
    pub async fn last_raw(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        self.storage
            .last(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await
    }

    /// Gets the last entry in the index of the given key, deserializing the value
    pub async fn last<V: BinaryDeserializer>(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, V)>, String> {
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
            Ok(Some((id, deserialize::<V>(&bytes)?)))
        } else {
            Ok(None)
        }
    }

    /// Gets the last entry in the index of the given key, returning only its id
    pub async fn last_id(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, String> {
        self.last_raw(namespace, key).await.map(|r| r.map(|p| p.0))
    }

    /// Gets the entry with the closest id to the given id in the index of the given key,
    /// in a way that `id` is less or equal to the id of the returned entry, returning as raw bytes
    pub async fn closest_raw(
        &self,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
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
    ) -> Result<Option<(u64, V)>, String> {
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
            Ok(Some((id, deserialize::<V>(&bytes)?)))
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
    ) -> Result<Option<u64>, String> {
        self.closest_raw(namespace, key, id)
            .await
            .map(|r| r.map(|p| p.0))
    }
}

/// Various namespaces for indexed storage
#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum IndexedStorageNamespace {
    OpLog { worker_id: WorkerId },
    CompressedOpLog { worker_id: WorkerId, level: usize },
}

/// Various namespaces for operations working on multiple indexed storage namespaces such as scan
#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum IndexedStorageMetaNamespace {
    Oplog,
    CompressedOplog { level: usize },
}
