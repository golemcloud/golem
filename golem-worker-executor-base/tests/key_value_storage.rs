// Copyright 2024-2025 Golem Cloud
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

use crate::WorkerExecutorTestDependencies;
use async_trait::async_trait;
use golem_common::config::RedisConfig;
use golem_common::model::AccountId;
use golem_common::redis::RedisPool;
use golem_service_base::storage::sqlite::SqlitePool;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::config::TestDependencies;
use golem_worker_executor_base::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_worker_executor_base::storage::keyvalue::redis::RedisKeyValueStorage;
use golem_worker_executor_base::storage::keyvalue::sqlite::SqliteKeyValueStorage;
use golem_worker_executor_base::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use sqlx::sqlite::SqlitePoolOptions;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use uuid::Uuid;

#[async_trait]
trait GetKeyValueStorage: Debug {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync>;
}

struct InMemoryKeyValueStorageWrapper;

impl Debug for InMemoryKeyValueStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("InMemoryKeyValueStorageWrapper")
    }
}

#[async_trait]
impl GetKeyValueStorage for InMemoryKeyValueStorageWrapper {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
        Arc::new(InMemoryKeyValueStorage::new())
    }
}

#[test_dep(tagged_as = "in_memory")]
async fn in_memory_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    Arc::new(InMemoryKeyValueStorageWrapper)
}

struct RedisKeyValueStorageWrapper {
    redis: Arc<dyn Redis + Send + Sync>,
}

impl Debug for RedisKeyValueStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedisKeyValueStorageWrapper")
    }
}

#[async_trait]
impl GetKeyValueStorage for RedisKeyValueStorageWrapper {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
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
        let kvs = RedisKeyValueStorage::new(redis_pool);
        Arc::new(kvs)
    }
}

#[test_dep(tagged_as = "redis")]
async fn redis_storage(
    deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    let redis = deps.redis();
    let redis_monitor = deps.redis_monitor();
    redis.assert_valid();
    redis_monitor.assert_valid();
    Arc::new(RedisKeyValueStorageWrapper { redis })
}

struct SqliteKeyValueStorageWrapper;

impl Debug for SqliteKeyValueStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("SqliteKeyValueStorageWrapper")
    }
}

#[async_trait]
impl GetKeyValueStorage for SqliteKeyValueStorageWrapper {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
        let sqlx_pool_sqlite = SqlitePoolOptions::new()
            .max_connections(10)
            .connect("sqlite::memory:")
            .await
            .expect("Cannot create db options");

        let pool = SqlitePool::new(sqlx_pool_sqlite)
            .await
            .expect("Cannot connect to sqlite db");

        let kvs = SqliteKeyValueStorage::new(pool).await.unwrap();
        Arc::new(kvs)
    }
}

#[test_dep(tagged_as = "sqlite")]
async fn sqlite_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    Arc::new(SqliteKeyValueStorageWrapper)
}

#[derive(Debug)]
struct Namespaces {
    pub ns: KeyValueStorageNamespace,
    pub ns2: KeyValueStorageNamespace,
}

#[test_dep(tagged_as = "ns1")]
fn ns() -> Namespaces {
    Namespaces {
        ns: KeyValueStorageNamespace::Worker,
        ns2: KeyValueStorageNamespace::UserDefined {
            account_id: AccountId::generate(),
            bucket: "test-bucket".to_string(),
        },
    }
}

#[test_dep(tagged_as = "ns2")]
fn ns2() -> Namespaces {
    Namespaces {
        ns: KeyValueStorageNamespace::UserDefined {
            account_id: AccountId::generate(),
            bucket: "test-bucket".to_string(),
        },
        ns2: KeyValueStorageNamespace::Worker,
    }
}

inherit_test_dep!(WorkerExecutorTestDependencies);

define_matrix_dimension!(kvs: Arc<dyn GetKeyValueStorage + Send + Sync> -> "in_memory", "redis", "sqlite");
define_matrix_dimension!(nss: Namespaces -> "ns1", "ns2");

#[test]
#[tracing::instrument]
async fn get_set_get(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let key = "key";
    let value = "value".as_bytes();

    let result1 = kvs
        .get("test", "api", "entity", ns.clone(), key)
        .await
        .unwrap();
    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();
    let result2 = kvs.get("test", "api", "entity", ns, key).await.unwrap();
    assert_eq!(result1, None);
    assert_eq!(result2, Some(value.into()));
}

#[test]
#[tracing::instrument]
async fn namespaces_are_separate(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns1 = nss.ns.clone();
    let ns2 = nss.ns2.clone();

    let key = "key";
    let value = "value".as_bytes();
    let value2 = "value2".as_bytes();

    let result11 = kvs
        .get("test", "api", "entity", ns1.clone(), key)
        .await
        .unwrap();
    kvs.set("test", "api", "entity", ns1.clone(), key, value)
        .await
        .unwrap();
    let result12 = kvs
        .get("test", "api", "entity", ns2.clone(), key)
        .await
        .unwrap();
    kvs.set("test", "api", "entity", ns2.clone(), key, value2)
        .await
        .unwrap();
    let result21 = kvs.get("test", "api", "entity", ns1, key).await.unwrap();
    let result22 = kvs.get("test", "api", "entity", ns2, key).await.unwrap();
    assert_eq!(result11, None);
    assert_eq!(result12, None);
    assert_eq!(result21, Some(value.into()));
    assert_eq!(result22, Some(value2.into()));
}

#[test]
#[tracing::instrument]
async fn get_set_get_many(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let key1 = "key1";
    let key2 = "key2";
    let key3 = "key3";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = kvs
        .get_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            vec![key1.to_string(), key2.to_string(), key3.to_string()],
        )
        .await
        .unwrap();
    kvs.set_many(
        "test",
        "api",
        "entity",
        ns.clone(),
        &[(key1, value1), (key2, value2)],
    )
    .await
    .unwrap();
    let result2 = kvs
        .get_many(
            "test",
            "api",
            "entity",
            ns,
            vec![key1.to_string(), key2.to_string(), key3.to_string()],
        )
        .await
        .unwrap();
    assert_eq!(result1, vec![None, None, None]);
    assert_eq!(
        result2,
        vec![Some(value1.into()), Some(value2.into()), None]
    );
}

#[test]
#[tracing::instrument]
async fn set_if_not_exists(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let key = "key";
    let value1 = "value".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = kvs
        .set_if_not_exists("test", "api", "entity", ns.clone(), key, value1)
        .await
        .unwrap();
    let result2 = kvs
        .set_if_not_exists("test", "api", "entity", ns.clone(), key, value2)
        .await
        .unwrap();
    let result3 = kvs.get("test", "api", "entity", ns, key).await.unwrap();
    assert!(result1);
    assert!(!result2);
    assert_eq!(result3, Some(value1.into()));
}

#[test]
#[tracing::instrument]
async fn del(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let key = "key";
    let value = "value".as_bytes();

    kvs.del("test", "api", ns.clone(), key).await.unwrap(); // deleting non-existing key must succeed
    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();
    let result1 = kvs
        .get("test", "api", "entity", ns.clone(), key)
        .await
        .unwrap();
    kvs.del("test", "api", ns.clone(), key).await.unwrap();
    let result2 = kvs.get("test", "api", "entity", ns, key).await.unwrap();

    assert_eq!(result1, Some(value.into()));
    assert_eq!(result2, None);
}

#[test]
#[tracing::instrument]
async fn del_many(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let key1 = "key";
    let key2 = "key2";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    kvs.del_many(
        "test",
        "api",
        ns.clone(),
        vec![key1.to_string(), key2.to_string()],
    )
    .await
    .unwrap(); // deleting non-existing key must succeed
    kvs.set("test", "api", "entity", ns.clone(), key1, value1)
        .await
        .unwrap();
    kvs.set("test", "api", "entity", ns.clone(), key2, value2)
        .await
        .unwrap();
    let result1 = kvs
        .get("test", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();
    let result2 = kvs
        .get("test", "api", "entity", ns.clone(), key2)
        .await
        .unwrap();
    kvs.del_many(
        "test",
        "api",
        ns.clone(),
        vec![key1.to_string(), key2.to_string()],
    )
    .await
    .unwrap();
    let result3 = kvs
        .get("test", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();
    let result4 = kvs
        .get("test", "api", "entity", ns.clone(), key2)
        .await
        .unwrap();

    assert_eq!(result1, Some(value1.into()));
    assert_eq!(result2, Some(value2.into()));
    assert_eq!(result3, None);
    assert_eq!(result4, None);
}

#[test]
#[tracing::instrument]
async fn exists_set_exists(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let key = "key";
    let value = "value".as_bytes();

    let result1 = kvs.exists("test", "api", ns.clone(), key).await.unwrap();
    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();
    let result2 = kvs.exists("test", "api", ns, key).await.unwrap();
    assert!(!result1);
    assert!(result2);
}

#[test]
#[tracing::instrument]
async fn exists_is_per_namespace(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();
    let ns2 = nss.ns2.clone();

    let key = "key";
    let value = "value".as_bytes();

    let result1 = kvs.exists("test", "api", ns.clone(), key).await.unwrap();
    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();
    let result2 = kvs.exists("test", "api", ns, key).await.unwrap();
    let result3 = kvs.exists("test", "api", ns2, key).await.unwrap();
    assert!(!result1);
    assert!(result2);
    assert!(!result3);
}

#[test]
#[tracing::instrument]
async fn keys(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns2.clone();

    let key1 = "key1";
    let key2 = "key2";

    let keys1 = kvs.keys("test", "api", ns.clone()).await.unwrap();
    kvs.set(
        "test",
        "api",
        "entity",
        ns.clone(),
        key1,
        "value1".as_bytes(),
    )
    .await
    .unwrap();
    kvs.set(
        "test",
        "api",
        "entity",
        ns.clone(),
        key2,
        "value2".as_bytes(),
    )
    .await
    .unwrap();
    let keys2 = kvs.keys("test", "api", ns.clone()).await.unwrap();
    kvs.del("test", "api", ns.clone(), key1).await.unwrap();
    let keys3 = kvs.keys("test", "api", ns).await.unwrap();

    tracing::debug!("keys2: {keys2:?}");

    assert_eq!(keys1, Vec::<String>::new());
    assert_eq!(keys2.len(), 2);
    assert!(keys2.contains(&key1.to_string()));
    assert!(keys2.contains(&key2.to_string()));
    assert_eq!(keys3, vec![key2.to_string()]);
}

#[test]
#[tracing::instrument]
async fn sets(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let set1 = "set1";
    let set2 = "set2";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    let s11 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s21 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.add_to_set("test", "api", "entity", ns.clone(), set1, value1)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();

    kvs.add_to_set("test", "api", "entity", ns.clone(), set2, value3)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set2, value3)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap();

    let s12 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s22 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();
    kvs.remove_from_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap();

    let s13 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s23 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap(); // can remove non-existing value
    kvs.remove_from_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap(); // can remove non-existing value

    let s14 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s24 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    assert2::check!(s11 == Vec::<Vec<u8>>::new());
    assert2::check!(s21 == Vec::<Vec<u8>>::new());
    assert2::check!(s12.len() == 2);
    assert2::check!(s12.contains(&value1.to_vec().into()));
    assert2::check!(s12.contains(&value2.to_vec().into()));
    assert2::check!(s22.len() == 2);
    assert2::check!(s22.contains(&value2.to_vec().into()));
    assert2::check!(s22.contains(&value3.to_vec().into()));
    assert2::check!(s13.len() == 1);
    assert2::check!(s13.contains(&value1.to_vec().into()));
    assert2::check!(s23.len() == 1);
    assert2::check!(s23.contains(&value3.to_vec().into()));
    assert2::check!(s14.len() == 1);
    assert2::check!(s14.contains(&value1.to_vec().into()));
    assert2::check!(s24.len() == 1);
    assert2::check!(s24.contains(&value3.to_vec().into()));
}

#[test]
#[tracing::instrument]
async fn sorted_sets(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let set1 = "set1";
    let set2 = "set2";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();
    let value4 = "value4".as_bytes();

    let s11 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s21 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 4.0, value4)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, value1)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 4.0, value4)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 3.0, value3)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 3.0, value3)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 2.0, value2)
        .await
        .unwrap();

    let s12 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s22 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();
    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap();

    let s13 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s23 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap(); // can remove non-existing value
    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap(); // can remove non-existing value

    let s14 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s24 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    assert_eq!(s11, Vec::<(f64, bytes::Bytes)>::new());
    assert_eq!(s21, Vec::<(f64, bytes::Bytes)>::new());

    assert_eq!(
        s12,
        vec![
            (1.0, value1.into()),
            (2.0, value2.into()),
            (4.0, value4.into())
        ]
    );
    assert_eq!(
        s22,
        vec![
            (2.0, value2.into()),
            (3.0, value3.into()),
            (4.0, value4.into())
        ]
    );

    assert_eq!(s13, vec![(1.0, value1.into()), (4.0, value4.into())]);
    assert_eq!(s23, vec![(3.0, value3.into()), (4.0, value4.into())]);

    assert_eq!(s14, vec![(1.0, value1.into()), (4.0, value4.into())]);
    assert_eq!(s24, vec![(3.0, value3.into()), (4.0, value4.into())]);
}

#[test]
#[tracing::instrument]
async fn add_to_sorted_set_updates_score(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let set1 = "set1";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, value1)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 3.0, value2)
        .await
        .unwrap();

    let result = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();

    assert_eq!(result, vec![(1.0, value1.into()), (3.0, value2.into())]);
}

#[test]
#[tracing::instrument]
async fn query_sorted_set(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
    #[dimension(nss)] nss: &Namespaces,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = nss.ns.clone();

    let set1 = "set1";
    let set2 = "set2";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();
    let value4 = "value4".as_bytes();

    let result1 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 0.0, 4.0)
        .await
        .unwrap();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, value1)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 3.0, value3)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 4.0, value4)
        .await
        .unwrap();

    let result2 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 0.0, 4.0)
        .await
        .unwrap();
    let result3 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, 3.0)
        .await
        .unwrap();
    let result4 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 1.5, 3.2)
        .await
        .unwrap();
    let result5 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set2, 4.0, 4.0)
        .await
        .unwrap();

    assert_eq!(result1, Vec::<(f64, bytes::Bytes)>::new());
    assert_eq!(
        result2,
        vec![
            (1.0, value1.into()),
            (2.0, value2.into()),
            (3.0, value3.into())
        ]
    );
    assert_eq!(
        result3,
        vec![
            (1.0, value1.into()),
            (2.0, value2.into()),
            (3.0, value3.into())
        ]
    );
    assert_eq!(result4, vec![(2.0, value2.into()), (3.0, value3.into())]);
    assert_eq!(result5, vec![(4.0, value4.into())]);
}
