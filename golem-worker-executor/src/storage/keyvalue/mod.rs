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
pub mod sqlite;

use async_trait::async_trait;
use bytes::Bytes;
use desert_rust::{BinaryDeserializer, BinarySerializer};
use golem_common::SafeDisplay;
use golem_common::model::AgentId;
use golem_common::model::RetryConfig;
use golem_common::model::environment::EnvironmentId;
use golem_common::retries::get_delay;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::future::Future;
use tracing::warn;

/// Runs a key-value storage database operation, retrying connection pool acquisition timeouts
/// according to `retry_config`. A pool timeout happens before any statement runs, so retrying is
/// safe even for non-idempotent operations. Any other error, or a pool timeout after the configured
/// retries are exhausted, is converted to a safe error string and returned to the caller unchanged.
pub(crate) async fn retry_on_pool_timeout<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &str,
    mut op: F,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RepoError>>,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) if err.is_pool_timeout() => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    warn!(
                        op = op_name,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient key-value storage error (connection pool timeout), retrying"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    return Err(err.to_safe_string());
                }
            }
            Err(err) => return Err(err.to_safe_string()),
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
    ) -> Result<(), String>;

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String>;

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String>;

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String>;

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String>;

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
    ) -> Result<Vec<(String, Bytes)>, String>;

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String>;

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String>;

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String>;

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String>;

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String>;

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String>;

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String>;

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), String>;

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String>;

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String>;

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, String>;
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
    }

    pub async fn del_many(
        &self,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        self.storage
            .del_many(self.svc_name, self.api_name, namespace, keys)
            .await
    }

    pub async fn exists(
        &self,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, key)
            .await
    }

    pub async fn keys(&self, namespace: KeyValueStorageNamespace) -> Result<Vec<String>, String> {
        self.storage
            .keys(self.svc_name, self.api_name, namespace)
            .await
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
    }

    pub async fn get_all_raw(
        &self,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, String> {
        self.storage
            .get_all(self.svc_name, self.api_name, self.entity_name, namespace)
            .await
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

#[cfg(test)]
mod tests {
    use super::retry_on_pool_timeout;
    use golem_common::model::RetryConfig;
    use golem_service_base::repo::RepoError;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use test_r::test;

    fn fast_retry(max_attempts: u32) -> RetryConfig {
        RetryConfig {
            max_attempts,
            min_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            max_jitter_factor: None,
        }
    }

    fn pool_timeout() -> RepoError {
        RepoError::from(sqlx::Error::PoolTimedOut)
    }

    #[test]
    async fn retries_pool_timeout_then_succeeds() {
        let attempts = AtomicU32::new(0);
        let result: Result<u32, String> = retry_on_pool_timeout(&fast_retry(5), "test", || {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move { if n < 2 { Err(pool_timeout()) } else { Ok(42) } }
        })
        .await;

        assert_eq!(result, Ok(42));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    async fn does_not_retry_non_pool_timeout_error() {
        let attempts = AtomicU32::new(0);
        let result: Result<u32, String> = retry_on_pool_timeout(&fast_retry(5), "test", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async move { Err(RepoError::UniqueViolation("duplicate".to_string())) }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn gives_up_after_exhausting_attempts() {
        let attempts = AtomicU32::new(0);
        let result: Result<u32, String> = retry_on_pool_timeout(&fast_retry(3), "test", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async move { Err(pool_timeout()) }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
