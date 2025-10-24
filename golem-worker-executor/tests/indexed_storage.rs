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

use crate::WorkerExecutorTestDependencies;
use assert2::check;
use async_trait::async_trait;
use golem_common::config::RedisConfig;
use golem_common::redis::RedisPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_test_framework::components::redis::Redis;
use golem_worker_executor::storage::indexed::memory::InMemoryIndexedStorage;
use golem_worker_executor::storage::indexed::redis::RedisIndexedStorage;
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor::storage::indexed::{
    IndexedStorage, IndexedStorageNamespace, ScanCursor,
};
use sqlx::sqlite::SqlitePoolOptions;
use std::fmt::Debug;
use std::sync::Arc;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use uuid::Uuid;

#[async_trait]
trait GetIndexedStorage: Debug {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync>;
}

struct InMemoryIndexedStorageWrapper;

impl Debug for InMemoryIndexedStorageWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InMemoryIndexedStorageWrapper")
    }
}

#[async_trait]
impl GetIndexedStorage for InMemoryIndexedStorageWrapper {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
        let kvs = InMemoryIndexedStorage::new();
        Arc::new(kvs)
    }
}

#[test_dep(tagged_as = "in_memory")]
async fn in_memory_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    Arc::new(InMemoryIndexedStorageWrapper)
}

struct RedisIndexedStorageWrapper {
    redis: Arc<dyn Redis + Send + Sync>,
}

impl Debug for RedisIndexedStorageWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RedisIndexedStorageWrapper")
    }
}

#[async_trait]
impl GetIndexedStorage for RedisIndexedStorageWrapper {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
        let random_prefix = Uuid::new_v4();
        let redis_pool = RedisPool::configured(&RedisConfig {
            host: self.redis.public_host(),
            port: self.redis.public_port(),
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
        Arc::new(kvs)
    }
}

#[test_dep(tagged_as = "redis")]
async fn redis_storage(
    deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    let redis = deps.redis.clone();
    let redis_monitor = deps.redis_monitor.clone();
    redis.assert_valid();
    redis_monitor.assert_valid();
    Arc::new(RedisIndexedStorageWrapper { redis })
}

struct SqliteIndexedStorageWrapper;

impl Debug for SqliteIndexedStorageWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqliteIndexedStorageWrapper")
    }
}

#[async_trait]
impl GetIndexedStorage for SqliteIndexedStorageWrapper {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
        let sqlx_pool_sqlite = SqlitePoolOptions::new()
            .max_connections(10)
            .connect("sqlite::memory:")
            .await
            .expect("Cannot create db options");

        let pool = SqlitePool::new(sqlx_pool_sqlite.clone(), sqlx_pool_sqlite);
        let sis = SqliteIndexedStorage::new(pool).await.unwrap();
        Arc::new(sis)
    }
}

#[test_dep(tagged_as = "sqlite")]
async fn sqlite_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    Arc::new(SqliteIndexedStorageWrapper)
}

#[test_dep(tagged_as = "ns1")]
fn ns() -> IndexedStorageNamespace {
    IndexedStorageNamespace::OpLog
}

#[test_dep(tagged_as = "ns2")]
fn ns2() -> IndexedStorageNamespace {
    IndexedStorageNamespace::CompressedOpLog { level: 1 }
}

inherit_test_dep!(WorkerExecutorTestDependencies);

define_matrix_dimension!(is: Arc<dyn GetIndexedStorage + Send + Sync> -> "in_memory", "redis", "sqlite");

#[test]
#[tracing::instrument]
async fn exists_append(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();

    let result1 = is.exists("svc", "api", ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    let result2 = is.exists("svc", "api", ns.clone(), key1).await.unwrap();

    check!(result1 == false);
    check!(result2 == true);
}

#[test]
#[tracing::instrument]
async fn namespaces_are_separate(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns1: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();

    is.append("svc", "api", "entity", ns1.clone(), key1, 1, value1)
        .await
        .unwrap();
    let result = is.exists("svc", "api", ns2.clone(), key1).await.unwrap();

    check!(result == false);
}

#[test]
#[tracing::instrument]
async fn can_append_and_get(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 2, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 3, value3)
        .await
        .unwrap();

    let result = is
        .read("svc", "api", "entity", ns.clone(), key1, 1, 3)
        .await
        .unwrap();

    check!(result == vec![(1, value1.into()), (2, value2.into()), (3, value3.into())]);
}

#[test]
#[tracing::instrument]
async fn append_cannot_overwrite(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    let result1 = is
        .append("svc", "api", "entity", ns.clone(), key1, 1, value2)
        .await;

    check!(result1.is_err());
}

#[test]
#[tracing::instrument]
async fn append_can_skip(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 4, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 8, value2)
        .await
        .unwrap();

    let result = is
        .read("svc", "api", "entity", ns.clone(), key1, 1, 10)
        .await
        .unwrap();

    check!(result == vec![(4, value1.into()), (8, value2.into())]);
}

#[test]
#[tracing::instrument]
async fn length(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = is.length("svc", "api", ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 4, value1)
        .await
        .unwrap();
    let result2 = is.length("svc", "api", ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 8, value2)
        .await
        .unwrap();
    let result3 = is.length("svc", "api", ns.clone(), key1).await.unwrap();

    check!(result1 == 0);
    check!(result2 == 1);
    check!(result3 == 2);
}

#[test]
#[tracing::instrument]
async fn scan_empty(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

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

#[test]
#[tracing::instrument]
async fn scan_with_no_pattern_single_paged(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "key2";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key2, 1, value2)
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
    check!(result.contains(&key1.to_string()));
    check!(result.contains(&key2.to_string()));
}

#[test]
#[tracing::instrument]
async fn scan_with_no_pattern_paginated(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "key2";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 2, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key2, 1, value2)
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

        if !r1.is_empty() || cursor == 0 {
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

    // Note: Redis does not guarantee to return the asked number of items, it is just a hint.
    // check!(r1.len() == 1);
    // check!(r2.len() == 1);
    check!(all.contains(&key1.to_string()));
    check!(all.contains(&key2.to_string()));
}

#[test]
#[tracing::instrument]
async fn scan_with_prefix_pattern_single_paged(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "other2";
    let key3 = "key3";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key2, 1, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key3, 1, value3)
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
    check!(result.contains(&key1.to_string()));
    check!(result.contains(&key3.to_string()));
}

#[test]
#[tracing::instrument]
async fn scan_with_prefix_pattern_paginated(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "other2";
    let key3 = "key3";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key2, 1, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key3, 1, value3)
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

    // Note: Redis does not guarantee to return the asked number of items, it is just a hint.
    // check!(r1.len() == 1);
    // check!(r2.len() == 1);
    check!(all.contains(&key1.to_string()));
    check!(all.contains(&key3.to_string()));
}

#[test]
#[tracing::instrument]
async fn exists_append_delete(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();

    let result1 = is.exists("svc", "api", ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.delete("svc", "api", ns.clone(), key1).await.unwrap();
    let result2 = is.exists("svc", "api", ns.clone(), key1).await.unwrap();

    check!(result1 == false);
    check!(result2 == false);
}

#[test]
#[tracing::instrument]
async fn delete_is_per_namespace(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns1: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();

    is.append("svc", "api", "entity", ns1.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.delete("svc", "api", ns2.clone(), key1).await.unwrap();
    let result = is.exists("svc", "api", ns1.clone(), key1).await.unwrap();

    check!(result == true);
}

#[test]
#[tracing::instrument]
async fn delete_non_existing(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";

    let result = is.delete("svc", "api", ns.clone(), key1).await;

    check!(result.is_ok());
}

#[test]
#[tracing::instrument]
async fn first(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = is
        .first("svc", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 5, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 7, value2)
        .await
        .unwrap();
    let result2 = is
        .first("svc", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();

    check!(result1 == None);
    check!(result2 == Some((5, value1.into())));
}

#[test]
#[tracing::instrument]
async fn last(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = is
        .last("svc", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 5, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 7, value2)
        .await
        .unwrap();
    let result2 = is
        .last("svc", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();

    check!(result1 == None);
    check!(result2 == Some((7, value2.into())));
}

#[test]
#[tracing::instrument]
async fn closest_low(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 3)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 5, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 7, value2)
        .await
        .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 3)
        .await
        .unwrap();

    check!(result1 == None);
    check!(result2 == Some((5, value1.into())));
}

#[test]
#[tracing::instrument]
async fn closest_match(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 5)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 5, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 7, value2)
        .await
        .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 5)
        .await
        .unwrap();

    check!(result1 == None);
    check!(result2 == Some((5, value1.into())));
}

#[test]
#[tracing::instrument]
async fn closest_mid(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 6)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 5, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 7, value2)
        .await
        .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 6)
        .await
        .unwrap();

    check!(result1 == None);
    check!(result2 == Some((7, value2.into())));
}

#[test]
#[tracing::instrument]
async fn closest_high(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 10)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 5, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 7, value2)
        .await
        .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.clone(), key1, 10)
        .await
        .unwrap();

    check!(result1 == None);
    check!(result2 == None);
}

#[test]
#[tracing::instrument]
async fn drop_prefix_no_match(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 10, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 11, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 12, value3)
        .await
        .unwrap();

    is.drop_prefix("svc", "api", ns.clone(), key1, 5)
        .await
        .unwrap();
    let result = is
        .read("svc", "api", "entity", ns.clone(), key1, 1, 100)
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

#[test]
#[tracing::instrument]
async fn drop_prefix_partial(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 10, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 11, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 12, value3)
        .await
        .unwrap();

    is.drop_prefix("svc", "api", ns.clone(), key1, 10)
        .await
        .unwrap();
    let result = is
        .read("svc", "api", "entity", ns.clone(), key1, 1, 100)
        .await
        .unwrap();

    check!(result == vec![(11, value2.into()), (12, value3.into())]);
}

#[test]
#[tracing::instrument]
async fn drop_prefix_full(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespace,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespace,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    is.append("svc", "api", "entity", ns.clone(), key1, 10, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 11, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.clone(), key1, 12, value3)
        .await
        .unwrap();

    is.drop_prefix("svc", "api", ns.clone(), key1, 20)
        .await
        .unwrap();
    let result = is
        .read("svc", "api", "entity", ns.clone(), key1, 1, 100)
        .await
        .unwrap();

    check!(result == vec![]);
}
