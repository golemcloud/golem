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

//! One retry policy for the whole [`KeyValueStorage`] interface.

use crate::storage::keyvalue::retrying::Idempotence::{Idempotent, NonIdempotent};
use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::RetryConfig;
use golem_common::retries::get_delay;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::sync::Arc;
use tracing::warn;

/// Whether an operation may be retried after it may already have been applied.
///
/// This is a property of the operation, not of the failure: see the per-method justifications on
/// the [`KeyValueStorage`] implementation below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Idempotence {
    /// Running the operation twice has the same effect, and yields the same result, as running it
    /// once - so any transient failure may be retried.
    Idempotent,
    /// The result depends on whether *this* call performed the write, so the operation may only be
    /// retried when the backend is known not to have attempted it.
    NonIdempotent,
}

/// Wraps any [`KeyValueStorage`] and retries transient backend failures with backoff, so that every
/// method of the interface behaves the same way whichever backend is configured.
///
/// Backends classify their own failures into [`KeyValueStorageError`]; this decorator owns the
/// policy. It is applied to the outermost storage instance - above
/// [`crate::storage::keyvalue::namespace_routed::NamespaceRoutedKeyValueStorage`] - so a namespace
/// routed to Redis and a namespace routed to PostgreSQL get the identical policy, and no backend
/// (present or future) can opt out of it.
///
/// Exhausted retries return the error to the caller rather than aborting the process.
pub struct RetryingKeyValueStorage {
    inner: Arc<dyn KeyValueStorage + Send + Sync>,
    retry_config: RetryConfig,
}

impl RetryingKeyValueStorage {
    pub fn new(inner: Arc<dyn KeyValueStorage + Send + Sync>, retry_config: RetryConfig) -> Self {
        Self {
            inner,
            retry_config,
        }
    }

    async fn retry<T, F, Fut>(
        &self,
        op_name: &'static str,
        idempotence: Idempotence,
        mut op: F,
    ) -> Result<T, KeyValueStorageError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, KeyValueStorageError>>,
    {
        let idempotent = idempotence == Idempotent;
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            match op().await {
                Ok(value) => return Ok(value),
                Err(err) if err.is_retryable(idempotent) => {
                    match get_delay(&self.retry_config, attempts) {
                        Some(delay) => {
                            warn!(
                                op = op_name,
                                attempt = attempts,
                                delay_ms = delay.as_millis() as u64,
                                "Transient key-value storage error, retrying: {err}"
                            );
                            tokio::time::sleep(delay).await;
                        }
                        None => {
                            warn!(
                                op = op_name,
                                attempts = attempts,
                                "Transient key-value storage error, giving up: {err}"
                            );
                            return Err(err);
                        }
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }
}

impl Debug for RetryingKeyValueStorage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "RetryingKeyValueStorage({:?})", self.inner)
    }
}

#[async_trait]
impl KeyValueStorage for RetryingKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        // Overwrites the key with the same bytes on every attempt.
        self.retry("set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .set(svc_name, api_name, entity_name, namespace, key, value)
                    .await
            }
        })
        .await
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), KeyValueStorageError> {
        // Same as `set`, for each pair.
        self.retry("set_many", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .set_many(svc_name, api_name, entity_name, namespace, pairs)
                    .await
            }
        })
        .await
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, KeyValueStorageError> {
        // The returned flag means "this call performed the write". If a first attempt did write but
        // its response was lost, a second attempt would report `false` and the caller would draw the
        // opposite conclusion - so this is the one operation retried only when the backend is known
        // not to have attempted it.
        self.retry("set_if_not_exists", NonIdempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .set_if_not_exists(svc_name, api_name, entity_name, namespace, key, value)
                    .await
            }
        })
        .await
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, KeyValueStorageError> {
        // Read-only.
        self.retry("get", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .get(svc_name, api_name, entity_name, namespace, key)
                    .await
            }
        })
        .await
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, KeyValueStorageError> {
        // Read-only.
        self.retry("get_many", Idempotent, || {
            let namespace = namespace.clone();
            let keys = keys.clone();
            async move {
                self.inner
                    .get_many(svc_name, api_name, entity_name, namespace, keys)
                    .await
            }
        })
        .await
    }

    async fn get_all(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, KeyValueStorageError> {
        // Read-only.
        self.retry("get_all", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .get_all(svc_name, api_name, entity_name, namespace)
                    .await
            }
        })
        .await
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), KeyValueStorageError> {
        // Deleting an already-deleted key is a no-op, and the result carries no "did it exist" bit.
        self.retry("del", Idempotent, || {
            let namespace = namespace.clone();
            async move { self.inner.del(svc_name, api_name, namespace, key).await }
        })
        .await
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), KeyValueStorageError> {
        // Same as `del`, for each key.
        self.retry("del_many", Idempotent, || {
            let namespace = namespace.clone();
            let keys = keys.clone();
            async move {
                self.inner
                    .del_many(svc_name, api_name, namespace, keys)
                    .await
            }
        })
        .await
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, KeyValueStorageError> {
        // Read-only.
        self.retry("exists", Idempotent, || {
            let namespace = namespace.clone();
            async move { self.inner.exists(svc_name, api_name, namespace, key).await }
        })
        .await
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, KeyValueStorageError> {
        // Read-only.
        self.retry("keys", Idempotent, || {
            let namespace = namespace.clone();
            async move { self.inner.keys(svc_name, api_name, namespace).await }
        })
        .await
    }

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        // Set semantics: adding a member already in the set is a no-op, and the result carries no
        // "was it already there" bit.
        self.retry("add_to_set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .add_to_set(svc_name, api_name, entity_name, namespace, key, value)
                    .await
            }
        })
        .await
    }

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        // Set semantics, as `add_to_set`.
        self.retry("remove_from_set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .remove_from_set(svc_name, api_name, entity_name, namespace, key, value)
                    .await
            }
        })
        .await
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, KeyValueStorageError> {
        // Read-only.
        self.retry("members_of_set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .members_of_set(svc_name, api_name, entity_name, namespace, key)
                    .await
            }
        })
        .await
    }

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        // Re-adding the same member with the same score overwrites it with itself.
        self.retry("add_to_sorted_set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .add_to_sorted_set(
                        svc_name,
                        api_name,
                        entity_name,
                        namespace,
                        key,
                        score,
                        value,
                    )
                    .await
            }
        })
        .await
    }

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        // Removing an absent member is a no-op, and the result carries no "was it there" bit.
        self.retry("remove_from_sorted_set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .remove_from_sorted_set(svc_name, api_name, entity_name, namespace, key, value)
                    .await
            }
        })
        .await
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
        // Read-only.
        self.retry("get_sorted_set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .get_sorted_set(svc_name, api_name, entity_name, namespace, key)
                    .await
            }
        })
        .await
    }

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
        // Read-only.
        self.retry("query_sorted_set", Idempotent, || {
            let namespace = namespace.clone();
            async move {
                self.inner
                    .query_sorted_set(svc_name, api_name, entity_name, namespace, key, min, max)
                    .await
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::RetryingKeyValueStorage;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use crate::storage::keyvalue::{
        KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace,
    };
    use async_trait::async_trait;
    use bytes::Bytes;
    use golem_common::model::RetryConfig;
    use std::sync::Arc;
    use std::sync::Mutex;
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

    /// Fails the first `failures` attempts of every operation with `error`, then delegates to an
    /// in-memory storage.
    #[derive(Debug)]
    struct FlakyKeyValueStorage {
        inner: InMemoryKeyValueStorage,
        error: KeyValueStorageError,
        remaining_failures: AtomicU32,
        attempts: AtomicU32,
        /// Every value handed to `set_if_not_exists`, in order, so a test can tell how many times
        /// the write actually reached the backend.
        writes: Mutex<Vec<Vec<u8>>>,
    }

    impl FlakyKeyValueStorage {
        fn new(error: KeyValueStorageError, failures: u32) -> Self {
            Self {
                inner: InMemoryKeyValueStorage::new(),
                error,
                remaining_failures: AtomicU32::new(failures),
                attempts: AtomicU32::new(0),
                writes: Mutex::new(Vec::new()),
            }
        }

        fn attempts(&self) -> u32 {
            self.attempts.load(Ordering::SeqCst)
        }

        fn fail_now(&self) -> Option<KeyValueStorageError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let remaining = self.remaining_failures.load(Ordering::SeqCst);
            if remaining > 0 {
                self.remaining_failures
                    .store(remaining - 1, Ordering::SeqCst);
                Some(self.error.clone())
            } else {
                None
            }
        }
    }

    #[async_trait]
    impl KeyValueStorage for FlakyKeyValueStorage {
        async fn set(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: KeyValueStorageNamespace,
            key: &str,
            value: &[u8],
        ) -> Result<(), KeyValueStorageError> {
            match self.fail_now() {
                Some(err) => Err(err),
                None => {
                    self.inner
                        .set(svc_name, api_name, entity_name, namespace, key, value)
                        .await
                }
            }
        }

        async fn set_many(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _pairs: &[(&str, &[u8])],
        ) -> Result<(), KeyValueStorageError> {
            unimplemented!()
        }

        async fn set_if_not_exists(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: KeyValueStorageNamespace,
            key: &str,
            value: &[u8],
        ) -> Result<bool, KeyValueStorageError> {
            self.writes.lock().unwrap().push(value.to_vec());
            match self.fail_now() {
                Some(err) => Err(err),
                None => {
                    self.inner
                        .set_if_not_exists(svc_name, api_name, entity_name, namespace, key, value)
                        .await
                }
            }
        }

        async fn get(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: KeyValueStorageNamespace,
            key: &str,
        ) -> Result<Option<Bytes>, KeyValueStorageError> {
            match self.fail_now() {
                Some(err) => Err(err),
                None => {
                    self.inner
                        .get(svc_name, api_name, entity_name, namespace, key)
                        .await
                }
            }
        }

        async fn get_many(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _keys: Vec<String>,
        ) -> Result<Vec<Option<Bytes>>, KeyValueStorageError> {
            unimplemented!()
        }

        async fn get_all(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
        ) -> Result<Vec<(String, Bytes)>, KeyValueStorageError> {
            unimplemented!()
        }

        async fn del(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _key: &str,
        ) -> Result<(), KeyValueStorageError> {
            unimplemented!()
        }

        async fn del_many(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _keys: Vec<String>,
        ) -> Result<(), KeyValueStorageError> {
            unimplemented!()
        }

        async fn exists(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _key: &str,
        ) -> Result<bool, KeyValueStorageError> {
            unimplemented!()
        }

        async fn keys(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _namespace: KeyValueStorageNamespace,
        ) -> Result<Vec<String>, KeyValueStorageError> {
            unimplemented!()
        }

        async fn add_to_set(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: KeyValueStorageNamespace,
            key: &str,
            value: &[u8],
        ) -> Result<(), KeyValueStorageError> {
            match self.fail_now() {
                Some(err) => Err(err),
                None => {
                    self.inner
                        .add_to_set(svc_name, api_name, entity_name, namespace, key, value)
                        .await
                }
            }
        }

        async fn remove_from_set(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _key: &str,
            _value: &[u8],
        ) -> Result<(), KeyValueStorageError> {
            unimplemented!()
        }

        async fn members_of_set(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: KeyValueStorageNamespace,
            key: &str,
        ) -> Result<Vec<Bytes>, KeyValueStorageError> {
            match self.fail_now() {
                Some(err) => Err(err),
                None => {
                    self.inner
                        .members_of_set(svc_name, api_name, entity_name, namespace, key)
                        .await
                }
            }
        }

        async fn add_to_sorted_set(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _key: &str,
            _score: f64,
            _value: &[u8],
        ) -> Result<(), KeyValueStorageError> {
            unimplemented!()
        }

        async fn remove_from_sorted_set(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _key: &str,
            _value: &[u8],
        ) -> Result<(), KeyValueStorageError> {
            unimplemented!()
        }

        async fn get_sorted_set(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _key: &str,
        ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
            unimplemented!()
        }

        async fn query_sorted_set(
            &self,
            _svc_name: &'static str,
            _api_name: &'static str,
            _entity_name: &'static str,
            _namespace: KeyValueStorageNamespace,
            _key: &str,
            _min: f64,
            _max: f64,
        ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
            unimplemented!()
        }
    }

    fn namespace() -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::RunningWorkers
    }

    #[test]
    async fn transient_failure_succeeds_on_retry() {
        let flaky = Arc::new(FlakyKeyValueStorage::new(
            KeyValueStorageError::Transient("connection reset".to_string()),
            2,
        ));
        let storage = RetryingKeyValueStorage::new(flaky.clone(), fast_retry(5));

        let result = storage
            .set("test", "api", "entity", namespace(), "key", b"value")
            .await;

        assert_eq!(result, Ok(()));
        assert_eq!(flaky.attempts(), 3);
        assert_eq!(
            flaky
                .inner
                .get("test", "api", "entity", namespace(), "key")
                .await,
            Ok(Some(Bytes::from_static(b"value")))
        );
    }

    #[test]
    async fn transient_read_failure_succeeds_on_retry() {
        let flaky = Arc::new(FlakyKeyValueStorage::new(
            KeyValueStorageError::Transient("connection reset".to_string()),
            1,
        ));
        let storage = RetryingKeyValueStorage::new(flaky.clone(), fast_retry(5));

        let result = storage
            .get("test", "api", "entity", namespace(), "missing")
            .await;

        assert_eq!(result, Ok(None));
        assert_eq!(flaky.attempts(), 2);
    }

    #[test]
    async fn exhausted_retries_return_the_error_instead_of_panicking() {
        let flaky = Arc::new(FlakyKeyValueStorage::new(
            KeyValueStorageError::Transient("connection reset".to_string()),
            u32::MAX,
        ));
        let storage = RetryingKeyValueStorage::new(flaky.clone(), fast_retry(3));

        let result = storage
            .add_to_set("test", "api", "entity", namespace(), "key", b"value")
            .await;

        assert_eq!(
            result,
            Err(KeyValueStorageError::Transient(
                "connection reset".to_string()
            ))
        );
        assert_eq!(flaky.attempts(), 3);
    }

    #[test]
    async fn non_transient_errors_are_not_retried() {
        let flaky = Arc::new(FlakyKeyValueStorage::new(
            KeyValueStorageError::Other("bad request".to_string()),
            u32::MAX,
        ));
        let storage = RetryingKeyValueStorage::new(flaky.clone(), fast_retry(5));

        let result = storage
            .members_of_set("test", "api", "entity", namespace(), "key")
            .await;

        assert_eq!(
            result,
            Err(KeyValueStorageError::Other("bad request".to_string()))
        );
        assert_eq!(flaky.attempts(), 1);
    }

    /// A transient failure may already have applied the write, so retrying would risk reporting
    /// `false` for a write this call actually performed.
    #[test]
    async fn set_if_not_exists_is_not_retried_after_a_possibly_applied_write() {
        let flaky = Arc::new(FlakyKeyValueStorage::new(
            KeyValueStorageError::Transient("connection reset".to_string()),
            1,
        ));
        let storage = RetryingKeyValueStorage::new(flaky.clone(), fast_retry(5));

        let result = storage
            .set_if_not_exists("test", "api", "entity", namespace(), "key", b"value")
            .await;

        assert_eq!(
            result,
            Err(KeyValueStorageError::Transient(
                "connection reset".to_string()
            ))
        );
        assert_eq!(flaky.attempts(), 1);
        assert_eq!(flaky.writes.lock().unwrap().len(), 1);
    }

    /// A failure to acquire a connection happens before the write is attempted, so even
    /// `set_if_not_exists` can be retried without changing what its result means.
    #[test]
    async fn set_if_not_exists_is_retried_when_the_write_was_not_attempted() {
        let flaky = Arc::new(FlakyKeyValueStorage::new(
            KeyValueStorageError::NotAttempted("pool timed out".to_string()),
            2,
        ));
        let storage = RetryingKeyValueStorage::new(flaky.clone(), fast_retry(5));

        let result = storage
            .set_if_not_exists("test", "api", "entity", namespace(), "key", b"value")
            .await;

        assert_eq!(result, Ok(true));
        assert_eq!(flaky.attempts(), 3);
    }
}
