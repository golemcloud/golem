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

use crate::WorkerExecutorTestDependencies;
use async_trait::async_trait;
use golem_common::config::DbPostgresConfig;
use golem_common::config::RedisConfig;
use golem_common::model::AgentFingerprint;
use golem_common::model::environment::EnvironmentId;
use golem_common::redis::RedisPool;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::components::redis::Redis;
use golem_worker_executor::services::golem_config::KeyValueStoragePostgresConfig;
use golem_worker_executor::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_worker_executor::storage::keyvalue::multi_sqlite::MultiSqliteKeyValueStorage;
use golem_worker_executor::storage::keyvalue::namespace_routed::NamespaceRoutedKeyValueStorage;
use golem_worker_executor::storage::keyvalue::postgres::PostgresKeyValueStorage;
use golem_worker_executor::storage::keyvalue::redis::RedisKeyValueStorage;
use golem_worker_executor::storage::keyvalue::sqlite::SqliteKeyValueStorage;
use golem_worker_executor::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use pretty_assertions::assert_eq;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use url::Url;
use uuid::{Uuid, uuid};

#[async_trait]
trait GetKeyValueStorage: Debug {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync>;

    /// A new storage, and the Redis pool of its Redis backend when it has one. The pool has the
    /// key prefix of the storage.
    async fn get_key_value_storage_and_redis(
        &self,
    ) -> (Arc<dyn KeyValueStorage + Send + Sync>, Option<RedisPool>) {
        (self.get_key_value_storage().await, None)
    }

    fn observes_expiry(&self) -> bool {
        false
    }

    /// Whether the storage keeps the status baseline in its Redis backend.
    fn stores_the_baseline_in_redis(&self) -> bool {
        false
    }
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

#[test_dep(scope = Shared, tagged_as = "in_memory")]
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
        self.get_key_value_storage_and_redis().await.0
    }

    async fn get_key_value_storage_and_redis(
        &self,
    ) -> (Arc<dyn KeyValueStorage + Send + Sync>, Option<RedisPool>) {
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
            tls: false,
        })
        .await
        .unwrap();
        let kvs = RedisKeyValueStorage::new(redis_pool.clone());
        (Arc::new(kvs), Some(redis_pool))
    }

    fn observes_expiry(&self) -> bool {
        true
    }

    fn stores_the_baseline_in_redis(&self) -> bool {
        true
    }
}

#[test_dep(scope = Shared, tagged_as = "redis")]
async fn redis_storage(
    deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    let redis = deps.redis.clone();
    let redis_monitor = deps.redis_monitor.clone();
    redis.assert_valid();
    redis_monitor.assert_valid();
    Arc::new(RedisKeyValueStorageWrapper { redis })
}

struct SqliteKeyValueStorageWrapper {
    tempdirs: Arc<Mutex<Vec<TempDir>>>,
}

impl SqliteKeyValueStorageWrapper {
    fn new() -> Self {
        Self {
            tempdirs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Debug for SqliteKeyValueStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("SqliteKeyValueStorageWrapper")
    }
}

#[async_trait]
impl GetKeyValueStorage for SqliteKeyValueStorageWrapper {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
        let tempdir = tempfile::tempdir().unwrap();
        let database = tempdir.path().join("kv.db").to_string_lossy().into_owned();
        self.tempdirs.lock().unwrap().push(tempdir);
        let config = golem_common::config::DbSqliteConfig {
            database,
            max_connections: 10,
            foreign_keys: false,
        };
        let kvs = SqliteKeyValueStorage::configured(&config).await.unwrap();
        Arc::new(kvs)
    }
}

struct MultiSqliteKeyValueStorageWrapper {
    tempdirs: Arc<Mutex<Vec<TempDir>>>,
}

impl MultiSqliteKeyValueStorageWrapper {
    fn new() -> Self {
        Self {
            tempdirs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Debug for MultiSqliteKeyValueStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("MultiSqliteKeyValueStorageWrapper")
    }
}

#[async_trait]
impl GetKeyValueStorage for MultiSqliteKeyValueStorageWrapper {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().to_path_buf();
        self.tempdirs.lock().unwrap().push(tempdir);

        let kvs = MultiSqliteKeyValueStorage::new(&path, 10, true);
        Arc::new(kvs)
    }
}

#[test_dep(scope = Shared, tagged_as = "sqlite")]
async fn sqlite_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    Arc::new(SqliteKeyValueStorageWrapper::new())
}

#[test_dep(scope = Shared, tagged_as = "multi_sqlite")]
async fn multi_sqlite_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    Arc::new(MultiSqliteKeyValueStorageWrapper::new())
}

struct PostgresKeyValueStorageWrapper {
    postgres: DockerPostgresRdb,
}

impl Debug for PostgresKeyValueStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("PostgresKeyValueStorageWrapper")
    }
}

#[async_trait]
impl GetKeyValueStorage for PostgresKeyValueStorageWrapper {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
        let db_name = format!("kv_{}", Uuid::new_v4().simple());

        let admin_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.postgres.public_connection_string())
            .await
            .expect("Cannot create postgres admin pool");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\";"))
            .execute(&admin_pool)
            .await
            .expect("Cannot create postgres test database");

        let postgres = DbPostgresConfig {
            host: "localhost".to_string(),
            database: db_name,
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            port: Url::parse(&self.postgres.public_connection_string())
                .expect("Invalid postgres connection string")
                .port()
                .expect("Postgres connection string missing port"),
            max_connections: 10,
            schema: None,
            acquire_timeout: None,
        };

        let config = KeyValueStoragePostgresConfig { postgres };

        let kvs = PostgresKeyValueStorage::configured(&config)
            .await
            .expect("Cannot create postgres key value storage");
        Arc::new(kvs)
    }
}

#[test_dep(scope = Shared, tagged_as = "postgres")]
async fn postgres_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    let unique_network_id = Uuid::new_v4().to_string();
    let postgres = DockerPostgresRdb::new(&unique_network_id, false).await;
    Arc::new(PostgresKeyValueStorageWrapper { postgres })
}

struct NamespaceRoutedKeyValueStorageWrapper {
    redis: Arc<dyn Redis + Send + Sync>,
    postgres: DockerPostgresRdb,
}

impl Debug for NamespaceRoutedKeyValueStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("NamespaceRoutedKeyValueStorageWrapper")
    }
}

#[async_trait]
impl GetKeyValueStorage for NamespaceRoutedKeyValueStorageWrapper {
    async fn get_key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
        self.get_key_value_storage_and_redis().await.0
    }

    async fn get_key_value_storage_and_redis(
        &self,
    ) -> (Arc<dyn KeyValueStorage + Send + Sync>, Option<RedisPool>) {
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
            tls: false,
        })
        .await
        .expect("Cannot create redis pool for routed kvs");
        let redis_storage: Arc<dyn KeyValueStorage + Send + Sync> =
            Arc::new(RedisKeyValueStorage::new(redis_pool.clone()));

        let db_name = format!("kv_routed_{}", Uuid::new_v4().simple());
        let admin_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.postgres.public_connection_string())
            .await
            .expect("Cannot create postgres admin pool");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\";"))
            .execute(&admin_pool)
            .await
            .expect("Cannot create postgres test database for routed kvs");

        let postgres = DbPostgresConfig {
            host: "localhost".to_string(),
            database: db_name,
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            port: Url::parse(&self.postgres.public_connection_string())
                .expect("Invalid postgres connection string")
                .port()
                .expect("Postgres connection string missing port"),
            max_connections: 10,
            schema: None,
            acquire_timeout: None,
        };
        let postgres_config = KeyValueStoragePostgresConfig { postgres };
        let postgres_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
            PostgresKeyValueStorage::configured(&postgres_config)
                .await
                .expect("Cannot create postgres key value storage for routed kvs"),
        );

        (
            Arc::new(NamespaceRoutedKeyValueStorage::new(
                redis_storage,
                postgres_storage,
            )),
            Some(redis_pool),
        )
    }

    fn observes_expiry(&self) -> bool {
        true
    }
}

#[test_dep(scope = Shared, tagged_as = "namespace_routed")]
async fn namespace_routed_storage(
    deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetKeyValueStorage + Send + Sync> {
    let redis = deps.redis.clone();
    let redis_monitor = deps.redis_monitor.clone();
    redis.assert_valid();
    redis_monitor.assert_valid();

    let unique_network_id = Uuid::new_v4().to_string();
    let postgres = DockerPostgresRdb::new(&unique_network_id, false).await;

    Arc::new(NamespaceRoutedKeyValueStorageWrapper { redis, postgres })
}

#[derive(Debug)]
struct Namespaces {
    pub ns: KeyValueStorageNamespace,
    pub ns2: KeyValueStorageNamespace,
}

#[test_dep(scope = PerWorker, tagged_as = "ns1")]
fn ns() -> Namespaces {
    Namespaces {
        ns: KeyValueStorageNamespace::Worker {
            owned_agent_id: crate::fresh_owned_agent_id(),
        },
        ns2: KeyValueStorageNamespace::UserDefined {
            environment_id: EnvironmentId(uuid!("296aa41a-ff44-4882-8f34-08b7fe431aa4")),
            bucket: "test-bucket".into(),
        },
    }
}

#[test_dep(scope = PerWorker, tagged_as = "ns2")]
fn ns2() -> Namespaces {
    Namespaces {
        ns: KeyValueStorageNamespace::UserDefined {
            environment_id: EnvironmentId(uuid!("296aa41a-ff44-4882-8f34-08b7fe431aa4")),
            bucket: "test-bucket".into(),
        },
        ns2: KeyValueStorageNamespace::Worker {
            owned_agent_id: crate::fresh_owned_agent_id(),
        },
    }
}

// Exercises the per-agent `AgentStatus` namespace (Redis-hash routed) used by the split agent
// status cache, across every backend.
#[test_dep(scope = PerWorker, tagged_as = "ns3")]
fn ns3() -> Namespaces {
    Namespaces {
        ns: KeyValueStorageNamespace::AgentStatus {
            owned_agent_id: crate::fresh_owned_agent_id(),
            fingerprint: AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000001")),
        },
        ns2: KeyValueStorageNamespace::UserDefined {
            environment_id: EnvironmentId(uuid!("296aa41a-ff44-4882-8f34-08b7fe431aa4")),
            bucket: "test-bucket-2".into(),
        },
    }
}

#[test_dep(scope = PerWorker, tagged_as = "ns4")]
fn ns4() -> Namespaces {
    Namespaces {
        ns: KeyValueStorageNamespace::AgentInvocationResultIndex {
            owned_agent_id: crate::fresh_owned_agent_id(),
            fingerprint: AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000001")),
        },
        ns2: KeyValueStorageNamespace::UserDefined {
            environment_id: EnvironmentId(uuid!("296aa41a-ff44-4882-8f34-08b7fe431aa4")),
            bucket: "test-bucket-3".into(),
        },
    }
}

inherit_test_dep!(WorkerExecutorTestDependencies);

define_matrix_dimension!(kvs: Arc<dyn GetKeyValueStorage + Send + Sync> -> "in_memory", "redis", "sqlite", "multi_sqlite", "postgres", "namespace_routed");
define_matrix_dimension!(nss: Namespaces -> "ns1", "ns2", "ns3", "ns4");

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
            [key1.to_string(), key2.to_string(), key3.to_string()].into(),
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
            ns.clone(),
            [key1.to_string(), key2.to_string(), key3.to_string()].into(),
        )
        .await
        .unwrap();
    assert_eq!(result1, vec![None, None, None]);
    assert_eq!(
        result2,
        vec![Some(value1.into()), Some(value2.into()), None]
    );
    let repeated = kvs
        .get_many(
            "test",
            "api",
            "entity",
            ns,
            [key2, key3, key1, key2, key3, key1]
                .map(str::to_string)
                .into(),
        )
        .await
        .unwrap();
    assert_eq!(
        repeated,
        vec![
            Some(value2.into()),
            None,
            Some(value1.into()),
            Some(value2.into()),
            None,
            Some(value1.into())
        ]
    );
}

#[test]
#[tracing::instrument]
async fn get_all_returns_namespace_snapshot(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let kvs = kvs.get_key_value_storage().await;
    let agent_id = crate::fresh_owned_agent_id();
    let other_agent_id = agent_id.clone();
    let ns = KeyValueStorageNamespace::AgentStatus {
        owned_agent_id: agent_id,
        fingerprint: AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000001")),
    };
    let other_ns = KeyValueStorageNamespace::AgentStatus {
        owned_agent_id: other_agent_id,
        fingerprint: AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000002")),
    };

    kvs.set_many(
        "test",
        "api",
        "entity",
        ns.clone(),
        &[("field1", b"value1"), ("field2", b"value2")],
    )
    .await
    .unwrap();
    kvs.set("test", "api", "entity", other_ns, "field3", b"value3")
        .await
        .unwrap();

    let mut result = kvs.get_all("test", "api", "entity", ns).await.unwrap();
    result.sort_by(|(left, _), (right, _)| left.cmp(right));

    assert_eq!(
        result,
        vec![
            ("field1".to_string(), bytes::Bytes::from_static(b"value1")),
            ("field2".to_string(), bytes::Bytes::from_static(b"value2")),
        ]
    );
}

#[test]
#[tracing::instrument]
async fn agent_invocation_result_index_is_separate_from_agent_status(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let kvs = kvs.get_key_value_storage().await;
    let agent_id = crate::fresh_owned_agent_id();
    let status_ns = KeyValueStorageNamespace::AgentStatus {
        owned_agent_id: agent_id.clone(),
        fingerprint: AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000001")),
    };
    let result_index_ns = KeyValueStorageNamespace::AgentInvocationResultIndex {
        owned_agent_id: agent_id.clone(),
        fingerprint: AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000001")),
    };
    let next_result_index_ns = KeyValueStorageNamespace::AgentInvocationResultIndex {
        owned_agent_id: agent_id,
        fingerprint: AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000002")),
    };

    kvs.set(
        "test",
        "api",
        "entity",
        status_ns.clone(),
        "same-key",
        b"status-value",
    )
    .await
    .unwrap();
    kvs.set(
        "test",
        "api",
        "entity",
        result_index_ns.clone(),
        "same-key",
        b"result-index-value",
    )
    .await
    .unwrap();

    assert_eq!(
        kvs.get("test", "api", "entity", status_ns, "same-key")
            .await
            .unwrap(),
        Some(bytes::Bytes::from_static(b"status-value"))
    );
    assert_eq!(
        kvs.get("test", "api", "entity", result_index_ns, "same-key",)
            .await
            .unwrap(),
        Some(bytes::Bytes::from_static(b"result-index-value"))
    );
    assert_eq!(
        kvs.get("test", "api", "entity", next_result_index_ns, "same-key",)
            .await
            .unwrap(),
        None,
        "an invocation mapping from fingerprint F1 must be invisible to F2"
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

fn cas_namespace() -> KeyValueStorageNamespace {
    KeyValueStorageNamespace::AgentDurableStreamSessionIndex {
        owned_agent_id: crate::fresh_owned_agent_id(),
        fingerprint: AgentFingerprint::new(),
    }
}

#[test]
async fn compare_and_set_many_contract(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = cas_namespace();

    assert!(
        kvs.compare_and_set_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "coverage",
            None,
            &[],
            &[("coverage", b"one"), ("row", b"first")]
        )
        .await
        .unwrap()
    );
    assert!(
        !kvs.compare_and_set_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "coverage",
            Some(b"wrong"),
            &["row"],
            &[
                ("coverage", b"bad"),
                ("row", b"bad"),
                ("loser-only", b"bad")
            ]
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "row")
            .await
            .unwrap()
            .as_deref(),
        Some(b"first".as_slice())
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "loser-only")
            .await
            .unwrap(),
        None
    );
    assert!(
        kvs.compare_and_set_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "coverage",
            Some(b"one"),
            &[],
            &[("coverage", b"two"), ("row", b"second")]
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "row")
            .await
            .unwrap()
            .as_deref(),
        Some(b"second".as_slice())
    );
    assert!(
        kvs.compare_and_set_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "absent-guard",
            None,
            &["missing-row"],
            &[("other-row", b"value")],
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "absent-guard")
            .await
            .unwrap(),
        None
    );
    assert!(
        kvs.compare_and_set_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "empty-guard",
            None,
            &[],
            &[("empty-guard", b"")],
        )
        .await
        .unwrap()
    );
    assert!(
        !kvs.compare_and_set_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "empty-guard",
            None,
            &["other-row"],
            &[("other-row", b"bad")],
        )
        .await
        .unwrap()
    );
    assert!(
        kvs.compare_and_set_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "empty-guard",
            Some(b""),
            &["empty-guard", "row", "overlap"],
            &[("other-row", b"updated"), ("overlap", b"upserted")],
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "empty-guard")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "row")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns, "overlap")
            .await
            .unwrap()
            .as_deref(),
        Some(b"upserted".as_slice())
    );
}

#[test]
async fn compare_and_mutate_many_contract(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = cas_namespace();
    let expiry = Duration::from_secs(60);

    kvs.set_with_expiry(
        "test",
        "api",
        "entity",
        ns.clone(),
        "delete-me",
        b"old",
        expiry,
    )
    .await
    .unwrap();
    assert!(
        kvs.compare_and_mutate_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "guard",
            None,
            &[("guard", b"one"), ("written", b"value")],
            &["delete-me"],
            expiry,
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "delete-me")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "written")
            .await
            .unwrap()
            .as_deref(),
        Some(b"value".as_slice())
    );

    assert!(
        !kvs.compare_and_mutate_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            "guard",
            Some(b"wrong"),
            &[("written", b"changed"), ("loser", b"value")],
            &["guard"],
            expiry,
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "guard")
            .await
            .unwrap()
            .as_deref(),
        Some(b"one".as_slice())
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "written")
            .await
            .unwrap()
            .as_deref(),
        Some(b"value".as_slice())
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns, "loser").await.unwrap(),
        None
    );
}

#[test]
async fn compare_and_delete_flat_key_contract(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let kvs = kvs.get_key_value_storage().await;
    let agent_id = crate::fresh_owned_agent_id();
    let namespace = KeyValueStorageNamespace::Worker {
        owned_agent_id: agent_id,
    };
    let key = format!("flat-cas-delete:{}", Uuid::new_v4());
    let expiry = Duration::from_secs(60);

    kvs.set("test", "api", "entity", namespace.clone(), &key, b"first")
        .await
        .unwrap();
    kvs.set(
        "test",
        "api",
        "entity",
        namespace.clone(),
        &key,
        b"replacement",
    )
    .await
    .unwrap();
    assert!(
        !kvs.compare_and_mutate_many(
            "test",
            "api",
            "entity",
            namespace.clone(),
            &key,
            Some(b"first"),
            &[],
            &[&key],
            expiry,
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", namespace.clone(), &key)
            .await
            .unwrap()
            .as_deref(),
        Some(b"replacement".as_slice())
    );
    assert!(
        kvs.compare_and_mutate_many(
            "test",
            "api",
            "entity",
            namespace.clone(),
            &key,
            Some(b"replacement"),
            &[],
            &[&key],
            expiry,
        )
        .await
        .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", namespace, &key)
            .await
            .unwrap(),
        None
    );
}

#[test]
async fn set_with_expiry_contract(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let storage = kvs.get_key_value_storage().await;
    let ns = cas_namespace();
    storage
        .set_with_expiry(
            "test",
            "api",
            "entity",
            ns.clone(),
            "expiring",
            b"value",
            Duration::from_millis(50),
        )
        .await
        .unwrap();
    assert_eq!(
        storage
            .get("test", "api", "entity", ns.clone(), "expiring")
            .await
            .unwrap()
            .as_deref(),
        Some(b"value".as_slice())
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    let after_expiry = storage
        .get("test", "api", "entity", ns, "expiring")
        .await
        .unwrap();
    if kvs.observes_expiry() {
        assert_eq!(after_expiry, None);

        let agent_id = crate::fresh_owned_agent_id();
        let fingerprint = AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000001"));
        let physical_hash_key = format!("agent-status:{}:{fingerprint}", agent_id.storage_key());
        storage
            .set(
                "test",
                "api",
                "entity",
                KeyValueStorageNamespace::Worker {
                    owned_agent_id: agent_id.clone(),
                },
                &physical_hash_key,
                b"wrong-type",
            )
            .await
            .unwrap();
        assert!(
            storage
                .set_with_expiry(
                    "test",
                    "api",
                    "entity",
                    KeyValueStorageNamespace::AgentStatus {
                        owned_agent_id: agent_id,
                        fingerprint,
                    },
                    "field",
                    b"value",
                    Duration::from_secs(60),
                )
                .await
                .is_err()
        );
    } else {
        assert_eq!(after_expiry.as_deref(), Some(b"value".as_slice()));
    }
}

#[test]
async fn namespace_routing_rejects_expiry_for_persistent_namespaces(
    _deps: &WorkerExecutorTestDependencies,
) {
    let shared: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(InMemoryKeyValueStorage::new());
    let kvs = NamespaceRoutedKeyValueStorage::new(shared.clone(), shared.clone());

    let result = kvs
        .set_with_expiry(
            "test",
            "api",
            "entity",
            KeyValueStorageNamespace::RunningWorkers,
            "worker",
            b"value",
            Duration::from_secs(60),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        shared
            .get(
                "test",
                "api",
                "entity",
                KeyValueStorageNamespace::RunningWorkers,
                "worker"
            )
            .await
            .unwrap(),
        None
    );
    let result = kvs
        .compare_and_mutate_many(
            "test",
            "api",
            "entity",
            KeyValueStorageNamespace::RunningWorkers,
            "guard",
            None,
            &[("worker", b"value")],
            &[],
            Duration::from_secs(60),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        shared
            .get(
                "test",
                "api",
                "entity",
                KeyValueStorageNamespace::RunningWorkers,
                "worker"
            )
            .await
            .unwrap(),
        None
    );
}

#[test]
async fn concurrent_compare_and_set_many_has_one_winner(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let kvs = kvs.get_key_value_storage().await;
    let ns = cas_namespace();
    kvs.set_many(
        "test",
        "api",
        "entity",
        ns.clone(),
        &[("stale", b"old"), ("winner", b"old")],
    )
    .await
    .unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    let tasks = (0..16).map(|writer| {
        let kvs = kvs.clone();
        let ns = ns.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            let value = [writer];
            barrier.wait().await;
            kvs.compare_and_set_many(
                "test",
                "api",
                "entity",
                ns,
                "coverage",
                None,
                &["stale", "winner"],
                &[("coverage", &value), ("winner", &value)],
            )
            .await
            .unwrap()
        })
    });
    let results = futures::future::join_all(tasks).await;
    assert_eq!(
        results
            .into_iter()
            .map(Result::unwrap)
            .filter(|won| *won)
            .count(),
        1
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns.clone(), "coverage")
            .await
            .unwrap(),
        kvs.get("test", "api", "entity", ns.clone(), "winner")
            .await
            .unwrap()
    );
    assert_eq!(
        kvs.get("test", "api", "entity", ns, "stale").await.unwrap(),
        None
    );
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
        [key1.to_string(), key2.to_string()].into(),
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
        [key1.to_string(), key2.to_string()].into(),
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

    assert_eq!(s11, Vec::<Vec<u8>>::new());
    assert_eq!(s21, Vec::<Vec<u8>>::new());
    assert_eq!(s12.len(), 2);
    assert!(s12.contains(&value1.to_vec().into()));
    assert!(s12.contains(&value2.to_vec().into()));
    assert_eq!(s22.len(), 2);
    assert!(s22.contains(&value2.to_vec().into()));
    assert!(s22.contains(&value3.to_vec().into()));
    assert_eq!(s13.len(), 1);
    assert!(s13.contains(&value1.to_vec().into()));
    assert_eq!(s23.len(), 1);
    assert!(s23.contains(&value3.to_vec().into()));
    assert_eq!(s14.len(), 1);
    assert!(s14.contains(&value1.to_vec().into()));
    assert_eq!(s24.len(), 1);
    assert!(s24.contains(&value3.to_vec().into()));
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

/// The live status cache is written with an expiry and the status baseline without one, as the
/// worker service writes them: a first write with no expected core, then a write that expects
/// the core of the first one. In every backend the baseline outlives the expiry of the cache.
/// After both writes, the Redis backend stores the baseline hash with no TTL (`PTTL` gives -1). Namespace routing
/// stores the baseline in its persistent backend, so its Redis backend has no key for it (`PTTL`
/// gives -2). The SQL and in-memory backends store no expiry, so for them the test shows only
/// that the hash stays.
#[test]
async fn the_status_baseline_outlives_the_expiry_of_the_status_cache(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let (storage, redis) = kvs.get_key_value_storage_and_redis().await;
    let owned_agent_id = crate::fresh_owned_agent_id();
    let fingerprint = AgentFingerprint::new();
    let baseline_key = format!(
        "agent-status-checkpoint:{}:{fingerprint}",
        owned_agent_id.storage_key()
    );
    let cache = KeyValueStorageNamespace::AgentStatus {
        owned_agent_id: owned_agent_id.clone(),
        fingerprint,
    };
    let baseline = KeyValueStorageNamespace::AgentStatusCheckpoint {
        owned_agent_id,
        fingerprint,
    };

    assert!(
        storage
            .compare_and_mutate_many(
                "test",
                "api",
                "entity",
                cache.clone(),
                "core",
                None,
                &[("core", b"cache")],
                &[],
                Duration::from_millis(50),
            )
            .await
            .unwrap()
    );
    assert!(
        storage
            .compare_and_set_many(
                "test",
                "api",
                "entity",
                baseline.clone(),
                "core",
                None,
                &[],
                &[("core", b"baseline")],
            )
            .await
            .unwrap()
    );
    assert!(
        storage
            .compare_and_set_many(
                "test",
                "api",
                "entity",
                baseline.clone(),
                "core",
                Some(b"baseline"),
                &[],
                &[("core", b"baseline-2")],
            )
            .await
            .unwrap()
    );
    tokio::time::sleep(Duration::from_millis(250)).await;

    let cached = storage
        .get_all("test", "api", "entity", cache)
        .await
        .unwrap();
    assert_eq!(cached.is_empty(), kvs.observes_expiry());
    assert_eq!(
        storage
            .get_all("test", "api", "entity", baseline)
            .await
            .unwrap()
            .into_iter()
            .map(|(field, value)| (field, value.to_vec()))
            .collect::<Vec<_>>(),
        vec![("core".to_string(), b"baseline-2".to_vec())]
    );
    if let Some(redis) = redis {
        let pttl = redis
            .with("test", "pttl")
            .eval(
                "return redis.call('PTTL', KEYS[1])",
                &[&baseline_key],
                vec![],
                None,
            )
            .await
            .unwrap()
            .convert::<i64>()
            .unwrap();
        assert_eq!(
            pttl,
            if kvs.stores_the_baseline_in_redis() {
                -1
            } else {
                -2
            }
        );
    }
}

/// The status baseline of one agent is a separate hash for each fingerprint, in every backend: a
/// re-created agent never reads the baseline of the deleted incarnation.
#[test]
#[tracing::instrument]
async fn the_status_baseline_of_an_agent_is_separate_for_each_fingerprint(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(kvs)] kvs: &Arc<dyn GetKeyValueStorage + Send + Sync>,
) {
    let storage = kvs.get_key_value_storage().await;
    let owned_agent_id = crate::fresh_owned_agent_id();
    let baseline = |fingerprint| KeyValueStorageNamespace::AgentStatusCheckpoint {
        owned_agent_id: owned_agent_id.clone(),
        fingerprint: AgentFingerprint(fingerprint),
    };
    let deleted = baseline(uuid!("00000000-0000-0000-0000-000000000001"));
    let recreated = baseline(uuid!("00000000-0000-0000-0000-000000000002"));

    assert!(
        storage
            .compare_and_set_many(
                "test",
                "api",
                "entity",
                deleted.clone(),
                "core",
                None,
                &[],
                &[("core", b"deleted"), ("membership", b"deleted-membership")],
            )
            .await
            .unwrap()
    );
    assert_eq!(
        storage
            .get_all("test", "api", "entity", recreated.clone())
            .await
            .unwrap(),
        Vec::new(),
        "the baseline of fingerprint F1 must be invisible to F2"
    );
    assert!(
        storage
            .compare_and_set_many(
                "test",
                "api",
                "entity",
                recreated.clone(),
                "core",
                None,
                &[],
                &[("core", b"recreated")],
            )
            .await
            .unwrap(),
        "the compare of F2 must not see the core of F1"
    );

    let fields = |namespace| {
        let storage = storage.clone();
        async move {
            let mut fields: Vec<(String, Vec<u8>)> = storage
                .get_all("test", "api", "entity", namespace)
                .await
                .unwrap()
                .into_iter()
                .map(|(field, value)| (field, value.to_vec()))
                .collect();
            fields.sort();
            fields
        }
    };
    assert_eq!(
        fields(deleted).await,
        vec![
            ("core".to_string(), b"deleted".to_vec()),
            ("membership".to_string(), b"deleted-membership".to_vec()),
        ]
    );
    assert_eq!(
        fields(recreated).await,
        vec![("core".to_string(), b"recreated".to_vec())]
    );
}
