// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::BASE_DEPS;
use golem_common::config::RedisConfig;
use golem_common::redis::RedisPool;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::config::TestDependencies;
use golem_worker_executor_base::storage::indexed::memory::InMemoryIndexedStorage;
use golem_worker_executor_base::storage::indexed::redis::RedisIndexedStorage;
use golem_worker_executor_base::storage::indexed::{IndexedStorage, IndexedStorageNamespace};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) trait GetIndexedStorage {
    fn get_indexed_storage(&self) -> &dyn IndexedStorage;
}

struct InMemoryIndexedStorageWrapper {
    kvs: InMemoryIndexedStorage,
}

impl GetIndexedStorage for InMemoryIndexedStorageWrapper {
    fn get_indexed_storage(&self) -> &dyn IndexedStorage {
        &self.kvs
    }
}

pub(crate) async fn in_memory_storage() -> impl GetIndexedStorage {
    let kvs = InMemoryIndexedStorage::new();
    InMemoryIndexedStorageWrapper { kvs }
}

struct RedisIndexedStorageWrapper {
    kvs: RedisIndexedStorage,
    _redis: Arc<dyn Redis + Send + Sync>,
    _monitor: Arc<dyn RedisMonitor + Send + Sync>,
}

impl GetIndexedStorage for RedisIndexedStorageWrapper {
    fn get_indexed_storage(&self) -> &dyn IndexedStorage {
        &self.kvs
    }
}

pub(crate) async fn redis_storage() -> impl GetIndexedStorage {
    let redis = BASE_DEPS.redis();
    let redis_monitor = BASE_DEPS.redis_monitor();
    redis.assert_valid();
    redis_monitor.assert_valid();
    let random_prefix = Uuid::new_v4();
    let redis_pool = RedisPool::configured(&RedisConfig {
        host: redis.public_host(),
        port: redis.public_port(),
        database: 0,
        tracing: false,
        pool_size: 1,
        retries: Default::default(),
        key_prefix: random_prefix.to_string(),
        username: None,
        password: None,
    })
    .await
    .unwrap();
    let kvs = RedisIndexedStorage::new(redis_pool);
    RedisIndexedStorageWrapper {
        kvs,
        _redis: redis,
        _monitor: redis_monitor,
    }
}

pub fn ns() -> IndexedStorageNamespace {
    IndexedStorageNamespace::OpLog
}

pub fn ns2() -> IndexedStorageNamespace {
    IndexedStorageNamespace::CompressedOpLog { level: 1 }
}

macro_rules! test_indexed_storage {
    ( $name:ident, $init:expr ) => {
        mod $name {
            use crate::indexed_storage::GetIndexedStorage;
            use assert2::check;
            use golem_worker_executor_base::storage::indexed::ScanCursor;

            #[tokio::test]
            #[tracing::instrument]
            async fn exists_append() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();

                let result1 = is.exists("svc", "api", ns.clone(), &key1).await.unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let result2 = is.exists("svc", "api", ns.clone(), &key1).await.unwrap();

                check!(result1 == false);
                check!(result2 == true);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn namespaces_are_separate() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns1 = crate::indexed_storage::ns();
                let ns2 = crate::indexed_storage::ns2();

                let key1 = "key1";
                let value1 = "value1".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns1.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let result = is.exists("svc", "api", ns2.clone(), &key1).await.unwrap();

                check!(result == false);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn can_append_and_get() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();
                let value3 = "value3".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 2, value2)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 3, value3)
                    .await
                    .unwrap();

                let result = is
                    .read("svc", "api", "entity", ns.clone(), &key1, 1, 3)
                    .await
                    .unwrap();

                check!(result == vec![(1, value1.into()), (2, value2.into()), (3, value3.into())]);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn append_cannot_overwrite() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let result1 = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value2)
                    .await;

                check!(result1.is_err());
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn append_can_skip() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 4, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 8, value2)
                    .await
                    .unwrap();

                let result = is
                    .read("svc", "api", "entity", ns.clone(), &key1, 1, 10)
                    .await
                    .unwrap();

                check!(result == vec![(4, value1.into()), (8, value2.into())]);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn length() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let result1 = is.length("svc", "api", ns.clone(), &key1).await.unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 4, value1)
                    .await
                    .unwrap();
                let result2 = is.length("svc", "api", ns.clone(), &key1).await.unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 8, value2)
                    .await
                    .unwrap();
                let result3 = is.length("svc", "api", ns.clone(), &key1).await.unwrap();

                check!(result1 == 0);
                check!(result2 == 1);
                check!(result3 == 2);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn scan_empty() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let mut result: Vec<String> = Vec::new();
                let mut cursor = ScanCursor::default();
                loop {
                    let (next, chunk) = is
                        .scan("svc", "api", ns.clone(), "*", cursor, 10)
                        .await
                        .unwrap();
                    result.extend(chunk);
                    cursor = next;
                    if next == 0 {
                        break;
                    }
                }

                check!(result == Vec::<String>::new());
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn scan_with_no_pattern_single_paged() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let key2 = "key2";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key2, 1, value2)
                    .await
                    .unwrap();

                let mut result: Vec<String> = Vec::new();
                let mut cursor = ScanCursor::default();
                loop {
                    let (next, chunk) = is
                        .scan("svc", "api", ns.clone(), "*", cursor, 10)
                        .await
                        .unwrap();
                    result.extend(chunk);
                    cursor = next;
                    if next == 0 {
                        break;
                    }
                }

                result.sort();
                check!(result == vec![key1.to_string(), key2.to_string()]);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn scan_with_no_pattern_paginated() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let key2 = "key2";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key2, 1, value2)
                    .await
                    .unwrap();

                let mut r1: Vec<String> = Vec::new();
                let mut cursor = ScanCursor::default();
                loop {
                    let (next, chunk) = is
                        .scan("svc", "api", ns.clone(), "*", cursor, 1)
                        .await
                        .unwrap();
                    r1.extend(chunk);
                    cursor = next;

                    if r1.len() == 1 || cursor == 0 {
                        break;
                    }
                }

                let mut r2: Vec<String> = Vec::new();
                loop {
                    let (next, chunk) = is
                        .scan("svc", "api", ns.clone(), "*", cursor, 1)
                        .await
                        .unwrap();
                    r2.extend(chunk);
                    cursor = next;

                    if cursor == 0 {
                        break;
                    }
                }

                let mut all = Vec::new();
                all.extend(r1.clone());
                all.extend(r2.clone());
                all.sort();

                check!(r1.len() == 1);
                check!(r2.len() == 1);
                check!(all == vec![key1.to_string(), key2.to_string()]);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn scan_with_prefix_pattern_single_paged() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let key2 = "other2";
                let key3 = "key3";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();
                let value3 = "value3".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key2, 1, value2)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key3, 1, value3)
                    .await
                    .unwrap();

                let mut result: Vec<String> = Vec::new();
                let mut cursor = ScanCursor::default();
                loop {
                    let (next, chunk) = is
                        .scan("svc", "api", ns.clone(), "key*", cursor, 10)
                        .await
                        .unwrap();
                    result.extend(chunk);
                    cursor = next;
                    if next == 0 {
                        break;
                    }
                }

                result.sort();
                check!(result == vec![key1.to_string(), key3.to_string()]);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn scan_with_prefix_pattern_paginated() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let key2 = "other2";
                let key3 = "key3";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();
                let value3 = "value3".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key2, 1, value2)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key3, 1, value3)
                    .await
                    .unwrap();

                let mut r1: Vec<String> = Vec::new();
                let mut cursor = ScanCursor::default();
                loop {
                    let (next, chunk) = is
                        .scan("svc", "api", ns.clone(), "key*", cursor, 1)
                        .await
                        .unwrap();
                    r1.extend(chunk);
                    cursor = next;

                    if r1.len() == 1 || cursor == 0 {
                        break;
                    }
                }

                let mut r2: Vec<String> = Vec::new();
                loop {
                    let (next, chunk) = is
                        .scan("svc", "api", ns.clone(), "key*", cursor, 1)
                        .await
                        .unwrap();
                    r2.extend(chunk);
                    cursor = next;

                    if cursor == 0 {
                        break;
                    }
                }

                let mut all = Vec::new();
                all.extend(r1.clone());
                all.extend(r2.clone());
                all.sort();

                check!(r1.len() == 1);
                check!(r2.len() == 1);
                check!(all == vec![key1.to_string(), key3.to_string()]);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn exists_append_delete() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();

                let result1 = is.exists("svc", "api", ns.clone(), &key1).await.unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let _ = is.delete("svc", "api", ns.clone(), &key1).await.unwrap();
                let result2 = is.exists("svc", "api", ns.clone(), &key1).await.unwrap();

                check!(result1 == false);
                check!(result2 == false);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn delete_is_per_namespace() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns1 = crate::indexed_storage::ns();
                let ns2 = crate::indexed_storage::ns2();

                let key1 = "key1";
                let value1 = "value1".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns1.clone(), &key1, 1, value1)
                    .await
                    .unwrap();
                let _ = is.delete("svc", "api", ns2.clone(), &key1).await.unwrap();
                let result = is.exists("svc", "api", ns1.clone(), &key1).await.unwrap();

                check!(result == true);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn delete_non_existing() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";

                let result = is.delete("svc", "api", ns.clone(), &key1).await;

                check!(result.is_ok());
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn first() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let result1 = is
                    .first("svc", "api", "entity", ns.clone(), &key1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 5, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 7, value2)
                    .await
                    .unwrap();
                let result2 = is
                    .first("svc", "api", "entity", ns.clone(), &key1)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == Some((5, value1.into())));
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn last() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let result1 = is
                    .last("svc", "api", "entity", ns.clone(), &key1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 5, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 7, value2)
                    .await
                    .unwrap();
                let result2 = is
                    .last("svc", "api", "entity", ns.clone(), &key1)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == Some((7, value2.into())));
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn closest_low() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let result1 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 3)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 5, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 7, value2)
                    .await
                    .unwrap();
                let result2 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 3)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == Some((5, value1.into())));
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn closest_match() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let result1 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 5)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 5, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 7, value2)
                    .await
                    .unwrap();
                let result2 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 5)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == Some((5, value1.into())));
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn closest_mid() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let result1 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 6)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 5, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 7, value2)
                    .await
                    .unwrap();
                let result2 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 6)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == Some((7, value2.into())));
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn closest_high() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();

                let result1 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 10)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 5, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 7, value2)
                    .await
                    .unwrap();
                let result2 = is
                    .closest("svc", "api", "entity", ns.clone(), &key1, 10)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == None);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn drop_prefix_no_match() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();
                let value3 = "value3".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 10, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 11, value2)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 12, value3)
                    .await
                    .unwrap();

                let _ = is
                    .drop_prefix("svc", "api", ns.clone(), &key1, 5)
                    .await
                    .unwrap();
                let result = is
                    .read("svc", "api", "entity", ns.clone(), &key1, 1, 100)
                    .await
                    .unwrap();

                check!(
                    result
                        == vec![
                            (10, value1.into()),
                            (11, value2.into()),
                            (12, value3.into())
                        ]
                );
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn drop_prefix_partial() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();
                let value3 = "value3".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 10, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 11, value2)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 12, value3)
                    .await
                    .unwrap();

                let _ = is
                    .drop_prefix("svc", "api", ns.clone(), &key1, 10)
                    .await
                    .unwrap();
                let result = is
                    .read("svc", "api", "entity", ns.clone(), &key1, 1, 100)
                    .await
                    .unwrap();

                check!(result == vec![(11, value2.into()), (12, value3.into())]);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn drop_prefix_full() {
                let test = $init.await;
                let is = test.get_indexed_storage();
                let ns = crate::indexed_storage::ns();

                let key1 = "key1";
                let value1 = "value1".as_bytes();
                let value2 = "value2".as_bytes();
                let value3 = "value3".as_bytes();

                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 10, value1)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 11, value2)
                    .await
                    .unwrap();
                let _ = is
                    .append("svc", "api", "entity", ns.clone(), &key1, 12, value3)
                    .await
                    .unwrap();

                let _ = is
                    .drop_prefix("svc", "api", ns.clone(), &key1, 20)
                    .await
                    .unwrap();
                let result = is
                    .read("svc", "api", "entity", ns.clone(), &key1, 1, 100)
                    .await
                    .unwrap();

                check!(result == vec![]);
            }
        }
    };
}

test_indexed_storage!(in_memory, crate::indexed_storage::in_memory_storage());
test_indexed_storage!(redis, crate::indexed_storage::redis_storage());
