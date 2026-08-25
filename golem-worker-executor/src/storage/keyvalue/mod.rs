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

pub mod memory;
pub mod multi_sqlite;
pub mod namespace_routed;
pub mod postgres;
pub mod redis;
pub mod retrying;
pub mod sqlite;

use async_trait::async_trait;
use bytes::Bytes;
use desert_rust::{BinaryDeserializer, BinarySerializer};
use golem_common::SafeDisplay;
use golem_common::model::AgentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::repo::RepoError;
use std::fmt::{Debug, Display, Formatter};

/// Error returned by every [`KeyValueStorage`] operation.
///
/// The variants classify how a failure may be *retried* rather than mirroring any one backend's
/// error taxonomy. Each backend maps its own errors onto them, which is what lets a single retry
/// policy - [`retrying::RetryingKeyValueStorage`] - behave identically no matter which backend is
/// configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyValueStorageError {
    /// A transient failure that happened before the operation reached the backend: a connection
    /// pool acquisition timeout, or a request the client refused to dispatch. The operation is
    /// guaranteed not to have been applied, so retrying it is safe even when it is not idempotent.
    NotAttempted(String),
    /// A transient failure where the operation may or may not have been applied: a mid-operation
    /// I/O error, a timeout waiting for the response, or a dropped connection. Retrying is only
    /// safe for idempotent operations.
    Transient(String),
    /// Any other failure, including every failure not known to be transient. Never retried.
    Other(String),
}

impl KeyValueStorageError {
    pub fn other(message: impl Display) -> Self {
        Self::Other(message.to_string())
    }

    /// Whether retrying the failed operation is safe. `idempotent` describes the operation, not the
    /// error: an operation is idempotent when running it twice has the same effect and yields the
    /// same result as running it once.
    pub fn is_retryable(&self, idempotent: bool) -> bool {
        match self {
            Self::NotAttempted(_) => true,
            Self::Transient(_) => idempotent,
            Self::Other(_) => false,
        }
    }
}

impl Display for KeyValueStorageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAttempted(message) => write!(
                f,
                "Transient key-value storage error (not attempted): {message}"
            ),
            Self::Transient(message) => write!(f, "Transient key-value storage error: {message}"),
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for KeyValueStorageError {}

impl From<KeyValueStorageError> for String {
    fn from(error: KeyValueStorageError) -> Self {
        error.to_string()
    }
}

impl From<String> for KeyValueStorageError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<RepoError> for KeyValueStorageError {
    fn from(error: RepoError) -> Self {
        // `to_safe_string` because repository errors can carry query fragments and row contents.
        let message = error.to_safe_string();
        if error.is_pool_timeout() {
            // Acquiring a connection timed out, so no statement reached the database.
            Self::NotAttempted(message)
        } else if error.is_transient() {
            Self::Transient(message)
        } else {
            Self::Other(message)
        }
    }
}

#[async_trait]
pub trait KeyValueStorage: Debug {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError>;

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), KeyValueStorageError>;

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, KeyValueStorageError>;

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, KeyValueStorageError>;

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, KeyValueStorageError>;

    /// Returns every `(field, value)` pair stored under `namespace` in a single atomic read.
    ///
    /// For the per-agent hash namespaces (split agent status / checkpoint) this is one round-trip
    /// that observes a consistent snapshot of all fields (Redis `HGETALL`, a single
    /// `SELECT ... WHERE namespace`, or one locked scan in memory) — unlike `keys` + `get_many`,
    /// which is two round-trips and can observe a torn write made between them.
    async fn get_all(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, KeyValueStorageError>;

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), KeyValueStorageError>;

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), KeyValueStorageError>;

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, KeyValueStorageError>;

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, KeyValueStorageError>;

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError>;

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError>;

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, KeyValueStorageError>;

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError>;

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError>;

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError>;

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError>;
}

pub trait KeyValueStorageLabelledApi<T: KeyValueStorage + ?Sized> {
    fn with(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> LabelledKeyValueStorage<'_, T>;

    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityKeyValueStorage<'_, T>;
}

impl<T: ?Sized + KeyValueStorage> KeyValueStorageLabelledApi<T> for T {
    fn with(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> LabelledKeyValueStorage<'_, T> {
        LabelledKeyValueStorage::new(svc_name, api_name, self)
    }
    fn with_entity(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
    ) -> LabelledEntityKeyValueStorage<'_, T> {
        LabelledEntityKeyValueStorage::new(svc_name, api_name, entity_name, self)
    }
}

pub struct LabelledKeyValueStorage<'a, S: KeyValueStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + KeyValueStorage> LabelledKeyValueStorage<'a, S> {
    pub fn new(svc_name: &'static str, api_name: &'static str, storage: &'a S) -> Self {
        Self {
            svc_name,
            api_name,
            storage,
        }
    }

    pub async fn del(&self, namespace: KeyValueStorageNamespace, key: &str) -> Result<(), String> {
        self.storage
            .del(self.svc_name, self.api_name, namespace, key)
            .await
            .map_err(Into::into)
    }

    pub async fn del_many(
        &self,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        self.storage
            .del_many(self.svc_name, self.api_name, namespace, keys)
            .await
            .map_err(Into::into)
    }

    pub async fn exists(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, key)
            .await
            .map_err(Into::into)
    }

    pub async fn keys(&self, namespace: KeyValueStorageNamespace) -> Result<Vec<String>, String> {
        self.storage
            .keys(self.svc_name, self.api_name, namespace)
            .await
            .map_err(Into::into)
    }
}

pub struct LabelledEntityKeyValueStorage<'a, S: KeyValueStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    entity_name: &'static str,
    storage: &'a S,
}

impl<'a, S: ?Sized + KeyValueStorage> LabelledEntityKeyValueStorage<'a, S> {
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

    pub async fn set<V: BinarySerializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;

        self.storage
            .set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn set_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        self.storage
            .set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                value,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn set_if_not_exists<V: BinarySerializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<bool, String> {
        let serialized = serialize(value)?;
        self.storage
            .set_if_not_exists(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn set_many<V: BinarySerializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &V)],
    ) -> Result<(), String> {
        let pairs = pairs
            .iter()
            .map(|(k, v)| serialize(v).map(|v| (k.to_string(), v.to_vec())))
            .collect::<Result<Vec<_>, String>>()?;
        let pairs_refs: Vec<(&str, &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        self.storage
            .set_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                &pairs_refs,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn set_many_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        self.storage
            .set_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                pairs,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn get<V: BinaryDeserializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<V>, String> {
        match self.get_attempt_deserialize(namespace, key).await? {
            Some(inner) => Ok(Some(inner?)),
            None => Ok(None),
        }
    }

    pub async fn get_attempt_deserialize<V: BinaryDeserializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Result<V, String>>, String> {
        let maybe_bytes = self
            .storage
            .get(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?;
        if let Some(bytes) = maybe_bytes {
            let value: Result<V, String> = deserialize(&bytes);
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    pub async fn get_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        self.storage
            .get(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn get_many<V: BinaryDeserializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<V>>, String> {
        let maybe_bytes = self
            .storage
            .get_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                keys,
            )
            .await?;
        let mut values = Vec::new();
        for maybe_bytes in maybe_bytes {
            if let Some(bytes) = maybe_bytes {
                let value: V = deserialize(&bytes)?;
                values.push(Some(value));
            } else {
                values.push(None);
            }
        }
        Ok(values)
    }

    pub async fn get_many_raw(
        &self,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        self.storage
            .get_many(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                keys,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn get_all_raw(
        &self,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, String> {
        self.storage
            .get_all(self.svc_name, self.api_name, self.entity_name, namespace)
            .await
            .map_err(Into::into)
    }

    pub async fn add_to_set<V: BinarySerializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .add_to_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn remove_from_set<V: BinarySerializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .remove_from_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn members_of_set<V: BinaryDeserializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<V>, String> {
        let maybe_bytes = self
            .storage
            .members_of_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?;
        let mut values = Vec::new();
        for bytes in maybe_bytes {
            let value: V = deserialize(&bytes)?;
            values.push(value);
        }
        Ok(values)
    }

    pub async fn add_to_sorted_set<V: BinarySerializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .add_to_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                score,
                &serialized,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn remove_from_sorted_set<V: BinarySerializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &V,
    ) -> Result<(), String> {
        let serialized = serialize(value)?;
        self.storage
            .remove_from_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                &serialized,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn get_sorted_set<V: BinaryDeserializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, V)>, String> {
        let maybe_bytes = self
            .storage
            .get_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
            )
            .await?;
        let mut values = Vec::new();
        for (score, bytes) in maybe_bytes {
            let value: V = deserialize(&bytes)?;
            values.push((score, value));
        }
        Ok(values)
    }

    pub async fn query_sorted_set<V: BinaryDeserializer>(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, V)>, String> {
        let maybe_bytes = self
            .storage
            .query_sorted_set(
                self.svc_name,
                self.api_name,
                self.entity_name,
                namespace,
                key,
                min,
                max,
            )
            .await?;
        let mut values = Vec::new();
        for (score, bytes) in maybe_bytes {
            let value: V = deserialize(&bytes)?;
            values.push((score, value));
        }
        Ok(values)
    }
}

/// Various namespaces for key-value storage
#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum KeyValueStorageNamespace {
    RunningWorkers,
    Worker {
        agent_id: AgentId,
    },
    /// Per-agent cached status. Unlike `Worker` (a flat key space), this namespace is stored as
    /// one structure-per-agent (a Redis hash) so the cached `AgentStatusRecord` can be split into
    /// independently written fields: a small fixed-size `core`, the `regions`, the `updates`, and
    /// one field per idempotency key (`ir:{key}`). This keeps the per-commit write small and
    /// decoupled from the unbounded parts of the status. The `agent_id` is part of the namespace so
    /// each agent gets its own isolated key space (enabling per-agent `keys`/`del_many`).
    AgentStatus {
        agent_id: AgentId,
    },
    /// Per-agent *clean* cached status checkpoint. Same physical layout as [`Self::AgentStatus`]
    /// (one structure-per-agent split into `core` / `regions` / `updates` / `ir:{key}`), but
    /// written only at structurally clean boundaries (snapshot save, throttled idle) where no
    /// jumpable oplog region is open. It is never advanced by the background status flusher, so it
    /// always holds a baseline before any later jump region and lets the status recompute fold
    /// forward from it instead of re-reading the whole oplog from index 1.
    AgentStatusCheckpoint {
        agent_id: AgentId,
    },
    Promise {
        agent_id: AgentId,
    },
    Schedule,
    UserDefined {
        environment_id: EnvironmentId,
        bucket: String,
    },
}
