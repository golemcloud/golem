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

use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::{DbPostgresConfig, RedisConfig};
use golem_common::model::AgentId;
use golem_common::model::ShardEpoch;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::redis::RedisPool;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::components::redis::Redis;
use golem_worker_executor::services::golem_config::IndexedStoragePostgresConfig;
use golem_worker_executor::storage::indexed::memory::InMemoryIndexedStorage;
use golem_worker_executor::storage::indexed::multi_sqlite::MultiSqliteIndexedStorage;
use golem_worker_executor::storage::indexed::postgres::PostgresIndexedStorage;
use golem_worker_executor::storage::indexed::redis::RedisIndexedStorage;
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace, ScanCursor, WriterId,
};
use golem_worker_executor_test_utils::WorkerExecutorTestDependencies;
use pretty_assertions::assert_eq;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use url::Url;
use uuid::Uuid;

#[async_trait]
trait GetIndexedStorage: Debug {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync>;

    /// Two handles onto the SAME store, writing as two different processes - what a shard manager
    /// that lost its state can produce by minting one epoch twice.
    async fn get_two_writers(
        &self,
    ) -> (
        Arc<dyn IndexedStorage + Send + Sync>,
        Arc<dyn IndexedStorage + Send + Sync>,
    );
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

    async fn get_two_writers(
        &self,
    ) -> (
        Arc<dyn IndexedStorage + Send + Sync>,
        Arc<dyn IndexedStorage + Send + Sync>,
    ) {
        let store = InMemoryIndexedStorage::new();
        let first = store.for_writer(WriterId(Uuid::new_v4()));
        let second = store.for_writer(WriterId(Uuid::new_v4()));
        (Arc::new(first), Arc::new(second))
    }
}

#[test_dep(scope = Shared, tagged_as = "in_memory")]
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
        Arc::new(RedisIndexedStorage::new(
            self.pool(&Uuid::new_v4().to_string()).await,
        ))
    }

    async fn get_two_writers(
        &self,
    ) -> (
        Arc<dyn IndexedStorage + Send + Sync>,
        Arc<dyn IndexedStorage + Send + Sync>,
    ) {
        let prefix = Uuid::new_v4().to_string();
        let first =
            RedisIndexedStorage::new(self.pool(&prefix).await).for_writer(WriterId(Uuid::new_v4()));
        let second =
            RedisIndexedStorage::new(self.pool(&prefix).await).for_writer(WriterId(Uuid::new_v4()));
        (Arc::new(first), Arc::new(second))
    }
}

impl RedisIndexedStorageWrapper {
    async fn pool(&self, key_prefix: &str) -> RedisPool {
        RedisPool::configured(&RedisConfig {
            host: self.redis.public_host(),
            port: self.redis.public_port(),
            database: 0,
            tracing: false,
            pool_size: 1,
            retries: Default::default(),
            key_prefix: key_prefix.to_string(),
            username: None,
            password: None,
            tls: false,
        })
        .await
        .unwrap()
    }
}

#[test_dep(scope = Shared, tagged_as = "redis")]
async fn redis_storage(
    deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    let redis = deps.redis.clone();
    let redis_monitor = deps.redis_monitor.clone();
    redis.assert_valid();
    redis_monitor.assert_valid();
    Arc::new(RedisIndexedStorageWrapper { redis })
}

struct SqliteIndexedStorageWrapper {
    tempdirs: Arc<Mutex<Vec<TempDir>>>,
}

impl SqliteIndexedStorageWrapper {
    fn new() -> Self {
        Self {
            tempdirs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Debug for SqliteIndexedStorageWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqliteIndexedStorageWrapper")
    }
}

#[async_trait]
impl GetIndexedStorage for SqliteIndexedStorageWrapper {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
        let tempdir = tempfile::tempdir().unwrap();
        let database = tempdir
            .path()
            .join("indexed.db")
            .to_string_lossy()
            .into_owned();
        self.tempdirs.lock().unwrap().push(tempdir);
        let config = golem_common::config::DbSqliteConfig {
            database,
            max_connections: 10,
            foreign_keys: false,
        };
        let sis = SqliteIndexedStorage::configured(&config).await.unwrap();
        Arc::new(sis)
    }

    async fn get_two_writers(
        &self,
    ) -> (
        Arc<dyn IndexedStorage + Send + Sync>,
        Arc<dyn IndexedStorage + Send + Sync>,
    ) {
        let tempdir = tempfile::tempdir().unwrap();
        let database = tempdir
            .path()
            .join("indexed.db")
            .to_string_lossy()
            .into_owned();
        self.tempdirs.lock().unwrap().push(tempdir);
        let config = golem_common::config::DbSqliteConfig {
            database,
            max_connections: 10,
            foreign_keys: false,
        };
        let first = SqliteIndexedStorage::configured(&config)
            .await
            .unwrap()
            .for_writer(WriterId(Uuid::new_v4()));
        let second = SqliteIndexedStorage::configured(&config)
            .await
            .unwrap()
            .for_writer(WriterId(Uuid::new_v4()));
        (Arc::new(first), Arc::new(second))
    }
}

#[test_dep(scope = Shared, tagged_as = "sqlite")]
async fn sqlite_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    Arc::new(SqliteIndexedStorageWrapper::new())
}

struct MultiSqliteIndexedStorageWrapper {
    tempdirs: Arc<Mutex<Vec<TempDir>>>,
}

impl MultiSqliteIndexedStorageWrapper {
    fn new() -> Self {
        Self {
            tempdirs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Debug for MultiSqliteIndexedStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("MultiSqliteIndexedStorageWrapper")
    }
}

#[async_trait]
impl GetIndexedStorage for MultiSqliteIndexedStorageWrapper {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().to_path_buf();
        self.tempdirs.lock().unwrap().push(tempdir);

        let storage = MultiSqliteIndexedStorage::new(&path, 10, true);
        Arc::new(storage)
    }

    async fn get_two_writers(
        &self,
    ) -> (
        Arc<dyn IndexedStorage + Send + Sync>,
        Arc<dyn IndexedStorage + Send + Sync>,
    ) {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().to_path_buf();
        self.tempdirs.lock().unwrap().push(tempdir);
        let first =
            MultiSqliteIndexedStorage::new(&path, 10, true).for_writer(WriterId(Uuid::new_v4()));
        let second =
            MultiSqliteIndexedStorage::new(&path, 10, true).for_writer(WriterId(Uuid::new_v4()));
        (Arc::new(first), Arc::new(second))
    }
}

#[test_dep(scope = Shared, tagged_as = "multi_sqlite")]
async fn multi_sqlite_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    Arc::new(MultiSqliteIndexedStorageWrapper::new())
}

struct PostgresIndexedStorageWrapper {
    postgres: DockerPostgresRdb,
}

impl Debug for PostgresIndexedStorageWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("PostgresIndexedStorageWrapper")
    }
}

impl PostgresIndexedStorageWrapper {
    /// A fresh database, and the config that reaches it. Separated from `get_indexed_storage` so a
    /// second storage can be opened onto the same database as a different writer.
    async fn fresh_database(&self) -> IndexedStoragePostgresConfig {
        let db_name = format!("idx_{}", Uuid::new_v4().simple());

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

        IndexedStoragePostgresConfig {
            postgres,
            drop_prefix_delete_batch_size: 1024,
            max_concurrent_ops: None,
        }
    }
}

#[async_trait]
impl GetIndexedStorage for PostgresIndexedStorageWrapper {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
        let config = self.fresh_database().await;
        let storage = PostgresIndexedStorage::configured(&config)
            .await
            .expect("Cannot create postgres indexed storage");

        Arc::new(storage)
    }

    async fn get_two_writers(
        &self,
    ) -> (
        Arc<dyn IndexedStorage + Send + Sync>,
        Arc<dyn IndexedStorage + Send + Sync>,
    ) {
        let config = self.fresh_database().await;
        let first = PostgresIndexedStorage::configured(&config)
            .await
            .expect("Cannot create postgres indexed storage")
            .for_writer(WriterId(Uuid::new_v4()));
        let second = PostgresIndexedStorage::configured(&config)
            .await
            .expect("Cannot create postgres indexed storage")
            .for_writer(WriterId(Uuid::new_v4()));
        (Arc::new(first), Arc::new(second))
    }
}

#[test_dep(scope = Shared, tagged_as = "postgres")]
async fn postgres_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    let unique_network_id = Uuid::new_v4().to_string();
    let postgres = DockerPostgresRdb::new(&unique_network_id, false).await;
    Arc::new(PostgresIndexedStorageWrapper { postgres })
}

/// A compressed level no other test writes to, so a walk over it sees a fixed set of keys.
const SCAN_STABLE_LEVEL: usize = 97;

#[derive(Debug, Clone)]
struct IndexedStorageNamespaces {
    ns: IndexedStorageNamespace,
    ns_other: IndexedStorageNamespace,
    meta: IndexedStorageMetaNamespace,
}

#[test_dep(scope = PerWorker, tagged_as = "ns1")]
fn ns() -> IndexedStorageNamespaces {
    IndexedStorageNamespaces {
        ns: IndexedStorageNamespace::OpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "test".to_string(),
            },
            agent_mode: AgentMode::Durable,
        },
        ns_other: IndexedStorageNamespace::OpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "test2".to_string(),
            },
            agent_mode: AgentMode::Durable,
        },
        meta: IndexedStorageMetaNamespace::Oplog {
            agent_mode: AgentMode::Durable,
        },
    }
}

#[test_dep(scope = PerWorker, tagged_as = "ns2")]
fn ns2() -> IndexedStorageNamespaces {
    IndexedStorageNamespaces {
        ns: IndexedStorageNamespace::CompressedOpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "test".to_string(),
            },
            agent_mode: AgentMode::Durable,
            level: 1,
        },
        ns_other: IndexedStorageNamespace::CompressedOpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "test2".to_string(),
            },
            agent_mode: AgentMode::Durable,
            level: 1,
        },
        meta: IndexedStorageMetaNamespace::CompressedOplog {
            agent_mode: AgentMode::Durable,
            level: 1,
        },
    }
}

inherit_test_dep!(WorkerExecutorTestDependencies);

define_matrix_dimension!(is: Arc<dyn GetIndexedStorage + Send + Sync> -> "in_memory", "redis", "sqlite", "multi_sqlite", "postgres");

#[test]
async fn staged_publication_preserves_atomic_visibility(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    let agent = AgentId {
        component_id: ComponentId::new(),
        agent_id: "fork-publication".into(),
    };
    let mode = AgentMode::Durable;
    let staged = IndexedStorageNamespace::StagedOpLog {
        agent_id: agent.clone(),
        agent_mode: mode,
    };
    let visible = IndexedStorageNamespace::OpLog {
        agent_id: agent.clone(),
        agent_mode: mode,
    };
    for (key, ids, expected) in [
        ("missing", vec![], 1),
        ("gap", vec![1, 3], 3),
        ("shifted", vec![2, 3], 2),
        ("wrong-tip", vec![1, 2, 3], 2),
        ("zero-tip", vec![1], 0),
    ] {
        for id in ids {
            storage
                .append(
                    "test",
                    "stage",
                    "entry",
                    staged.clone(),
                    key,
                    id,
                    vec![id as u8],
                    None,
                )
                .await
                .unwrap();
        }
        assert!(
            storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged.clone(),
                    key,
                    visible.clone(),
                    key,
                    expected,
                )
                .await
                .is_err()
        );
        assert!(
            !storage
                .exists("test", "visible", visible.clone(), key)
                .await
                .unwrap()
        );
    }
    for (key, value) in [("first", 17u8), ("second", 39)] {
        storage
            .append_many(
                "test",
                "stage",
                "entry",
                &staged,
                key,
                (1..=3)
                    .map(|id| (id, Bytes::from(vec![value, id as u8])))
                    .collect::<Vec<_>>()
                    .into(),
                None,
            )
            .await
            .unwrap();
    }
    assert!(
        storage
            .scan(
                "test",
                "scan",
                IndexedStorageMetaNamespace::Oplog { agent_mode: mode },
                None,
                0,
                100
            )
            .await
            .unwrap()
            .1
            .is_empty()
    );
    let (first, second) = tokio::join!(
        storage.move_if_absent(
            "test",
            "publish",
            staged.clone(),
            "first",
            visible.clone(),
            "target",
            3,
        ),
        storage.move_if_absent(
            "test",
            "publish",
            staged.clone(),
            "second",
            visible.clone(),
            "target",
            3,
        ),
    );
    let first = first.unwrap();
    assert_ne!(first, second.unwrap());
    let (winner, loser, value) = if first {
        ("first", "second", 17)
    } else {
        ("second", "first", 39)
    };
    let expected: Vec<_> = (1..=3).map(|id| (id, vec![value, id as u8])).collect();
    assert_eq!(
        storage
            .read("test", "read", "entry", visible.clone(), "target", 1, 3)
            .await
            .unwrap(),
        expected
    );
    assert!(
        !storage
            .exists("test", "stage", staged.clone(), winner)
            .await
            .unwrap()
    );
    assert!(
        storage
            .exists("test", "stage", staged.clone(), loser)
            .await
            .unwrap()
    );
    assert!(
        !storage
            .move_if_absent(
                "test",
                "publish",
                staged.clone(),
                loser,
                visible.clone(),
                "target",
                3,
            )
            .await
            .unwrap()
    );
    storage
        .delete("test", "discard", staged.clone(), loser)
        .await
        .unwrap();
    assert_eq!(
        storage
            .read("test", "read", "entry", visible.clone(), "target", 1, 3)
            .await
            .unwrap(),
        expected
    );

    for (key, first_id) in [("archived", 1), ("archived-arbitrary", 17)] {
        storage
            .append(
                "test",
                "create",
                "entry",
                visible.clone(),
                key,
                first_id,
                vec![61],
                None,
            )
            .await
            .unwrap();
        storage
            .drop_prefix("test", "archive", visible.clone(), key, first_id)
            .await
            .unwrap();
        assert!(
            storage
                .exists("test", "exists", visible.clone(), key)
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .length("test", "length", visible.clone(), key)
                .await
                .unwrap(),
            0
        );
        assert!(
            storage
                .first("test", "first", "entry", visible.clone(), key)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            storage
                .last("test", "last", "entry", visible.clone(), key)
                .await
                .unwrap()
                .is_none()
        );
        storage
            .append(
                "test",
                "stage",
                "entry",
                staged.clone(),
                key,
                1,
                vec![23],
                None,
            )
            .await
            .unwrap();
        assert!(
            !storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged.clone(),
                    key,
                    visible.clone(),
                    key,
                    1,
                )
                .await
                .unwrap()
        );
        storage
            .delete("test", "delete", visible.clone(), key)
            .await
            .unwrap();
        assert!(
            !storage
                .exists("test", "exists", visible.clone(), key)
                .await
                .unwrap()
        );
        assert!(
            storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged.clone(),
                    key,
                    visible.clone(),
                    key,
                    1,
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .read("test", "read", "entry", visible.clone(), key, 1, first_id)
                .await
                .unwrap(),
            vec![(1, vec![23])]
        );
    }
    storage
        .drop_prefix("test", "archive", visible.clone(), "never-existed", 50)
        .await
        .unwrap();
    assert!(
        !storage
            .exists("test", "exists", visible.clone(), "never-existed")
            .await
            .unwrap()
    );

    storage
        .append(
            "test",
            "stage",
            "entry",
            staged.clone(),
            "race",
            1,
            vec![51],
            None,
        )
        .await
        .unwrap();
    let (published, ordinary) = tokio::join!(
        storage.move_if_absent(
            "test",
            "publish",
            staged,
            "race",
            visible.clone(),
            "ordinary",
            1,
        ),
        storage.append(
            "test",
            "create",
            "entry",
            visible.clone(),
            "ordinary",
            1,
            vec![77],
            None,
        ),
    );
    let published = published.unwrap();
    assert_ne!(published, ordinary.is_ok());
    assert_eq!(
        storage
            .read("test", "read", "entry", visible, "ordinary", 1, 2)
            .await
            .unwrap(),
        vec![(1, vec![if published { 51 } else { 77 }])]
    );
}

#[test]
async fn postgres_singleton_append_many_preserves_storage_contract(
    #[tagged_as("postgres")] storage: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] primary: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] compressed: &IndexedStorageNamespaces,
) {
    let storage = storage.get_indexed_storage().await;
    let value = Bytes::from_static(&[0, 255, 17, 3]);
    // Once asserting no epoch, which is a lone autocommit INSERT, and once asserting the recorded
    // one, which goes through the fenced transaction. A caller must not be able to tell the two
    // paths apart.
    for (key, shard_epoch) in [
        ("singleton", None),
        ("fenced-singleton", Some(ShardEpoch(1))),
    ] {
        for ns in [primary, compressed] {
            if let Some(epoch) = shard_epoch {
                storage
                    .set_key_epoch("svc", "api", ns.ns.clone(), key, epoch)
                    .await
                    .unwrap();
            }
            storage
                .append_many(
                    "svc",
                    "api",
                    "entity",
                    &ns.ns,
                    key,
                    Arc::from([]),
                    shard_epoch,
                )
                .await
                .unwrap();
            assert!(
                !storage
                    .exists("svc", "api", ns.ns.clone(), key)
                    .await
                    .unwrap()
            );
            storage
                .append_many(
                    "svc",
                    "api",
                    "entity",
                    &ns.ns,
                    key,
                    Arc::from([(17, value.clone())]),
                    shard_epoch,
                )
                .await
                .unwrap();
            assert_eq!(
                storage
                    .read("svc", "api", "entity", ns.ns.clone(), key, 0, 100)
                    .await
                    .unwrap(),
                vec![(17, value.to_vec())]
            );
            assert_eq!(
                storage
                    .length("svc", "api", ns.ns.clone(), key)
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(
                storage
                    .last("svc", "api", "entity", ns.ns.clone(), key)
                    .await
                    .unwrap(),
                Some((17, value.to_vec()))
            );
            let (_, keys) = storage
                .scan(
                    "svc",
                    "api",
                    ns.meta.clone(),
                    Some(key),
                    ScanCursor::default(),
                    10,
                )
                .await
                .unwrap();
            assert_eq!(keys, vec![key.to_string()]);
            assert!(matches!(
                storage
                    .append_many(
                        "svc",
                        "api",
                        "entity",
                        &ns.ns,
                        key,
                        Arc::from([(u64::MAX, value.clone())]),
                        shard_epoch,
                    )
                    .await,
                Err(IndexedStorageError::Other(_))
            ));
            assert_eq!(
                storage
                    .length("svc", "api", ns.ns.clone(), key)
                    .await
                    .unwrap(),
                1
            );
        }
        assert!(matches!(
            storage
                .append_many(
                    "svc",
                    "api",
                    "entity",
                    &primary.ns,
                    key,
                    Arc::from([(17, Bytes::from_static(b"replacement"))]),
                    shard_epoch,
                )
                .await,
            Err(IndexedStorageError::Conflict(_))
        ));
        assert_eq!(
            storage
                .read("svc", "api", "entity", primary.ns.clone(), key, 0, 100)
                .await
                .unwrap(),
            vec![(17, value.to_vec())]
        );
    }
}

#[test]
async fn postgres_append_many_rolls_back_across_statement_chunks(
    #[tagged_as("postgres")] storage: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let storage = storage.get_indexed_storage().await;
    storage
        .append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            "atomic",
            1025,
            b"original".to_vec(),
            None,
        )
        .await
        .unwrap();
    let pairs: Arc<[(u64, Bytes)]> = (1..=1025)
        .map(|id| (id, Bytes::from_static(b"new")))
        .collect::<Vec<_>>()
        .into();
    assert!(matches!(
        storage
            .append_many("svc", "api", "entity", &ns.ns, "atomic", pairs, None)
            .await,
        Err(IndexedStorageError::Conflict(_))
    ));
    assert_eq!(
        storage
            .read("svc", "api", "entity", ns.ns.clone(), "atomic", 0, 2000)
            .await
            .unwrap(),
        vec![(1025, b"original".to_vec())]
    );
}

#[test]
#[tracing::instrument]
async fn exists_append(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();

    let result1 = is.exists("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1, None)
        .await
        .unwrap();
    let result2 = is.exists("svc", "api", ns.ns.clone(), key1).await.unwrap();

    assert_eq!(result1, false);
    assert_eq!(result2, true);
}

#[test]
#[tracing::instrument]
async fn namespaces_are_separate(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns1: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns1.ns.clone(),
        key1,
        1,
        value1,
        None,
    )
    .await
    .unwrap();
    let result = is.exists("svc", "api", ns2.ns.clone(), key1).await.unwrap();

    assert_eq!(result, false);
}

#[test]
#[tracing::instrument]
async fn can_append_and_get(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();
    let value3 = "value3".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        1,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        2,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        3,
        value3.clone(),
        None,
    )
    .await
    .unwrap();

    let result = is
        .read("svc", "api", "entity", ns.ns.clone(), key1, 1, 3)
        .await
        .unwrap();

    assert_eq!(result, vec![(1, value1), (2, value2), (3, value3)]);
}

#[test]
#[tracing::instrument]
async fn append_cannot_overwrite(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1, None)
        .await
        .unwrap();
    let result1 = is
        .append("svc", "api", "entity", ns.ns.clone(), key1, 1, value2, None)
        .await;

    assert!(result1.is_err());
}

#[test]
#[tracing::instrument]
async fn append_can_skip(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        4,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        8,
        value2.clone(),
        None,
    )
    .await
    .unwrap();

    let result = is
        .read("svc", "api", "entity", ns.ns.clone(), key1, 1, 10)
        .await
        .unwrap();

    assert_eq!(result, vec![(4, value1), (8, value2)]);
}

#[test]
#[tracing::instrument]
async fn length(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is.length("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 4, value1, None)
        .await
        .unwrap();
    let result2 = is.length("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 8, value2, None)
        .await
        .unwrap();
    let result3 = is.length("svc", "api", ns.ns.clone(), key1).await.unwrap();

    assert_eq!(result1, 0);
    assert_eq!(result2, 1);
    assert_eq!(result3, 2);
}

#[test]
#[tracing::instrument]
async fn scan_empty(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let mut result: Vec<String> = Vec::new();
    let mut cursor = ScanCursor::default();
    loop {
        let (next, chunk) = is
            .scan("svc", "api", ns.meta.clone(), None, cursor, 10)
            .await
            .unwrap();
        result.extend(chunk);
        cursor = next;
        if next == 0 {
            break;
        }
    }

    assert_eq!(result, Vec::<String>::new());
}

#[test]
#[tracing::instrument]
async fn scan_with_no_pattern_single_paged(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "key2";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1, None)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key2, 1, value2, None)
        .await
        .unwrap();

    let mut result: Vec<String> = Vec::new();
    let mut cursor = ScanCursor::default();
    loop {
        let (next, chunk) = is
            .scan("svc", "api", ns.meta.clone(), None, cursor, 10)
            .await
            .unwrap();
        result.extend(chunk);
        cursor = next;
        if next == 0 {
            break;
        }
    }

    result.sort();
    assert!(result.contains(&key1.to_string()));
    assert!(result.contains(&key2.to_string()));
}

#[test]
#[tracing::instrument]
async fn scan_with_no_pattern_paginated(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "key2";
    let key3 = "key2";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();
    let value3 = "value3".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        1,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        2,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns2.ns.clone(),
        key2,
        1,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns_other.clone(),
        key3,
        3,
        value3.clone(),
        None,
    )
    .await
    .unwrap();

    let mut r1: Vec<String> = Vec::new();
    let mut cursor = ScanCursor::default();
    loop {
        let (next, chunk) = is
            .scan("svc", "api", ns.meta.clone(), None, cursor, 1)
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
            .scan("svc", "api", ns.meta.clone(), None, cursor, 1)
            .await
            .unwrap();
        r2.extend(chunk);
        cursor = next;

        if cursor == 0 {
            break;
        }
    }

    let mut r3: Vec<String> = Vec::new();
    loop {
        let (next, chunk) = is
            .scan("svc", "api", ns.meta.clone(), None, cursor, 1)
            .await
            .unwrap();
        r3.extend(chunk);
        cursor = next;

        if cursor == 0 {
            break;
        }
    }

    let mut all = Vec::new();
    all.extend(r1.clone());
    all.extend(r2.clone());
    all.extend(r3.clone());
    all.sort();

    // Note: Redis does not guarantee to return the asked number of items, it is just a hint.
    // check!(r1.len() == 1);
    // check!(r2.len() == 1);
    assert!(all.contains(&key1.to_string()));
    assert!(all.contains(&key2.to_string()));
    assert!(all.contains(&key3.to_string()));
}

/// `scan_stable` must not skip keys when the caller deletes each page it is handed. The keys belong
/// to six agents because the multi-SQLite backend keeps a file per agent, and a second namespace is
/// drained alongside, as an archive step drains the layer below.
#[test]
#[tracing::instrument]
async fn scan_stable_resumes_past_deleted_keys(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let is = is.get_indexed_storage().await;
    let swept_meta = IndexedStorageMetaNamespace::CompressedOplog {
        agent_mode: AgentMode::Durable,
        level: SCAN_STABLE_LEVEL,
    };

    // One agent per key, so a backend that shards by agent has six shards to walk.
    let planted: Vec<(IndexedStorageNamespace, IndexedStorageNamespace, String)> = (0..6)
        .map(|i| {
            let component_id = ComponentId::new();
            let swept = IndexedStorageNamespace::CompressedOpLog {
                agent_id: AgentId {
                    component_id,
                    agent_id: format!("stable-{i}"),
                },
                agent_mode: AgentMode::Durable,
                level: SCAN_STABLE_LEVEL,
            };
            let below = IndexedStorageNamespace::CompressedOpLog {
                agent_id: AgentId {
                    component_id,
                    agent_id: format!("stable-{i}"),
                },
                agent_mode: AgentMode::Durable,
                level: SCAN_STABLE_LEVEL + 1,
            };
            (swept, below, format!("{}-swept-{i}", Uuid::new_v4()))
        })
        .collect();

    for (swept, below, key) in &planted {
        is.append(
            "svc",
            "api",
            "entity",
            swept.clone(),
            key,
            1,
            b"v".to_vec(),
            None,
        )
        .await
        .unwrap();
        is.append(
            "svc",
            "api",
            "entity",
            below.clone(),
            key,
            1,
            b"v".to_vec(),
            None,
        )
        .await
        .unwrap();
    }

    // Take a page, delete its keys here and in the layer below, and resume from the token.
    let mut seen: Vec<String> = Vec::new();
    let mut resume = None;
    let mut terminated = false;
    for _ in 0..256 {
        let (next, chunk) = is
            .with("svc", "api")
            .scan_stable(swept_meta.clone(), None, resume, 2)
            .await
            .unwrap();
        for key in &chunk {
            if let Some((swept, below, _)) = planted.iter().find(|(_, _, k)| k == key) {
                is.delete("svc", "api", swept.clone(), key).await.unwrap();
                is.delete("svc", "api", below.clone(), key).await.unwrap();
            }
        }
        seen.extend(chunk);
        match next {
            Some(next) => resume = Some(next),
            None => {
                terminated = true;
                break;
            }
        }
    }

    assert!(
        terminated,
        "the walk never reported exhaustion and was cut off by the iteration cap"
    );

    for (_, _, key) in &planted {
        assert!(
            seen.contains(key),
            "key {key} was skipped: the walk moved past something nothing had examined"
        );
    }

    // Redis may return a key more than once, so check membership rather than an exact sequence.
    for key in &seen {
        assert!(
            planted.iter().any(|(_, _, planted)| planted == key),
            "the walk returned {key}, which belongs to no namespace it was pointed at"
        );
    }
}

/// A drained multi-SQLite namespace keeps its files, so the walk still crosses them a page budget
/// at a time.
#[test]
#[tracing::instrument]
async fn multi_sqlite_scan_stable_crosses_its_files_a_page_at_a_time() {
    async fn walk(
        is: &MultiSqliteIndexedStorage,
        meta: &IndexedStorageMetaNamespace,
    ) -> (usize, Vec<String>) {
        let mut resume = None;
        let mut seen: Vec<String> = Vec::new();
        for page in 1..=32 {
            let (next, chunk) = is
                .scan_stable("svc", "api", meta.clone(), None, resume, 2)
                .await
                .unwrap();
            seen.extend(chunk);
            match next {
                Some(next) => resume = Some(next),
                None => return (page, seen),
            }
        }
        panic!("the walk never reported exhaustion and was cut off by the iteration cap");
    }

    let tempdir = TempDir::new().unwrap();
    let is = MultiSqliteIndexedStorage::new(tempdir.path(), 10, true);
    let meta = IndexedStorageMetaNamespace::Oplog {
        agent_mode: AgentMode::Durable,
    };

    // An odd number of agents, so the last page opens fewer files than its budget allows.
    let planted: Vec<(IndexedStorageNamespace, String)> = (0..5)
        .map(|i| {
            let namespace = IndexedStorageNamespace::OpLog {
                agent_id: AgentId {
                    component_id: ComponentId::new(),
                    agent_id: format!("file-{i}"),
                },
                agent_mode: AgentMode::Durable,
            };
            (namespace, format!("key-{i}"))
        })
        .collect();

    for (namespace, key) in &planted {
        is.append(
            "svc",
            "api",
            "entity",
            namespace.clone(),
            key,
            1,
            b"v".to_vec(),
            None,
        )
        .await
        .unwrap();
    }

    let (pages, mut seen) = walk(&is, &meta).await;
    let mut expected: Vec<String> = planted.iter().map(|(_, key)| key.clone()).collect();
    seen.sort();
    expected.sort();
    assert_eq!(seen, expected);
    assert_eq!(pages, 3, "two files to a page, and the fifth ends the walk");

    for (namespace, key) in &planted {
        is.delete("svc", "api", namespace.clone(), key)
            .await
            .unwrap();
    }

    let (pages, seen) = walk(&is, &meta).await;
    assert!(seen.is_empty(), "every key was deleted");
    assert_eq!(
        pages, 3,
        "the drained files are still crossed two at a time, not opened all at once"
    );
}

/// A file created after a listing was cached still shows up in the next walk, because creating a
/// file clears the listing cache.
#[test]
#[tracing::instrument]
async fn multi_sqlite_scan_stable_sees_files_created_after_a_walk() {
    async fn walk(
        is: &MultiSqliteIndexedStorage,
        meta: &IndexedStorageMetaNamespace,
    ) -> Vec<String> {
        let mut resume = None;
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..32 {
            let (next, chunk) = is
                .scan_stable("svc", "api", meta.clone(), None, resume, 2)
                .await
                .unwrap();
            seen.extend(chunk);
            match next {
                Some(next) => resume = Some(next),
                None => {
                    seen.sort();
                    return seen;
                }
            }
        }
        panic!("the walk never reported exhaustion and was cut off by the iteration cap");
    }

    async fn plant(is: &MultiSqliteIndexedStorage, name: &str) -> String {
        let namespace = IndexedStorageNamespace::OpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: name.to_string(),
            },
            agent_mode: AgentMode::Durable,
        };
        let key = format!("key-{name}");
        is.append(
            "svc",
            "api",
            "entity",
            namespace,
            &key,
            1,
            b"v".to_vec(),
            None,
        )
        .await
        .unwrap();
        key
    }

    let tempdir = TempDir::new().unwrap();
    let is = MultiSqliteIndexedStorage::new(tempdir.path(), 10, true);
    let meta = IndexedStorageMetaNamespace::Oplog {
        agent_mode: AgentMode::Durable,
    };

    let mut expected = vec![plant(&is, "first").await, plant(&is, "second").await];
    expected.sort();
    assert_eq!(walk(&is, &meta).await, expected);

    // Straight after a walk, so the listing the walk took is still inside its window.
    expected.push(plant(&is, "third").await);
    expected.sort();
    assert_eq!(
        walk(&is, &meta).await,
        expected,
        "a walk served a cached listing missed a file created after it was taken"
    );
}

/// `last_id` must answer without moving the payload, and must agree with `last` when it does.
#[test]
#[tracing::instrument]
async fn last_id_matches_last_without_the_value(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = format!("{}-last-id", Uuid::new_v4());

    assert_eq!(
        is.with_entity("svc", "api", "entity")
            .last_id(ns.ns.clone(), &key)
            .await
            .unwrap(),
        None,
        "an index with no entries has no last id"
    );

    for id in [1u64, 7, 42] {
        is.append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            &key,
            id,
            format!("value-{id}").into_bytes(),
            None,
        )
        .await
        .unwrap();
    }

    let last = is
        .last("svc", "api", "entity", ns.ns.clone(), &key)
        .await
        .unwrap();
    let last_id = is
        .with_entity("svc", "api", "entity")
        .last_id(ns.ns.clone(), &key)
        .await
        .unwrap();

    assert_eq!(last_id, Some(42));
    assert_eq!(last_id, last.map(|(id, _)| id));
}

#[test]
#[tracing::instrument]
async fn scan_with_prefix_pattern_single_paged(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "other2";
    let key3 = "key3";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();
    let value3 = "value3".as_bytes().to_vec();

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1, None)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key2, 1, value2, None)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key3, 1, value3, None)
        .await
        .unwrap();

    let mut result: Vec<String> = Vec::new();
    let mut cursor = ScanCursor::default();
    loop {
        let (next, chunk) = is
            .scan("svc", "api", ns.meta.clone(), Some("key"), cursor, 10)
            .await
            .unwrap();
        result.extend(chunk);
        cursor = next;
        if next == 0 {
            break;
        }
    }

    result.sort();
    assert!(result.contains(&key1.to_string()));
    assert!(result.contains(&key3.to_string()));
}

#[test]
#[tracing::instrument]
async fn scan_with_prefix_pattern_paginated(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let key2 = "other2";
    let key3 = "key3";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();
    let value3 = "value3".as_bytes().to_vec();

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1, None)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key2, 1, value2, None)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key3, 1, value3, None)
        .await
        .unwrap();

    let mut r1: Vec<String> = Vec::new();
    let mut cursor = ScanCursor::default();
    loop {
        let (next, chunk) = is
            .scan("svc", "api", ns.meta.clone(), Some("key"), cursor, 1)
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
            .scan("svc", "api", ns.meta.clone(), Some("key"), cursor, 1)
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
    assert!(all.contains(&key1.to_string()));
    assert!(all.contains(&key3.to_string()));
}

#[test]
#[tracing::instrument]
async fn exists_append_delete(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();

    let result1 = is.exists("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1, None)
        .await
        .unwrap();
    is.delete("svc", "api", ns.ns.clone(), key1).await.unwrap();
    let result2 = is.exists("svc", "api", ns.ns.clone(), key1).await.unwrap();

    assert_eq!(result1, false);
    assert_eq!(result2, false);
}

#[test]
#[tracing::instrument]
async fn delete_is_per_namespace(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns1: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns1.ns.clone(),
        key1,
        1,
        value1,
        None,
    )
    .await
    .unwrap();
    is.delete("svc", "api", ns2.ns.clone(), key1).await.unwrap();
    let result = is.exists("svc", "api", ns1.ns.clone(), key1).await.unwrap();

    assert_eq!(result, true);
}

#[test]
#[tracing::instrument]
async fn delete_non_existing(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";

    let result = is.delete("svc", "api", ns.ns.clone(), key1).await;

    assert!(result.is_ok());
}

#[test]
#[tracing::instrument]
async fn first(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is
        .first("svc", "api", "entity", ns.ns.clone(), key1)
        .await
        .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        5,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        7,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    let result2 = is
        .first("svc", "api", "entity", ns.ns.clone(), key1)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, Some((5, value1)));
}

#[test]
#[tracing::instrument]
async fn last(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is
        .last("svc", "api", "entity", ns.ns.clone(), key1)
        .await
        .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        5,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        7,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    let result2 = is
        .last("svc", "api", "entity", ns.ns.clone(), key1)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, Some((7, value2)));
}

#[test]
#[tracing::instrument]
async fn closest_low(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 3)
        .await
        .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        5,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        7,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 3)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, Some((5, value1)));
}

#[test]
#[tracing::instrument]
async fn closest_match(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 5)
        .await
        .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        5,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        7,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 5)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, Some((5, value1)));
}

#[test]
#[tracing::instrument]
async fn closest_mid(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 6)
        .await
        .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        5,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        7,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 6)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, Some((7, value2)));
}

#[test]
#[tracing::instrument]
async fn closest_high(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 10)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 5, value1, None)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 7, value2, None)
        .await
        .unwrap();
    let result2 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 10)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, None);
}

#[test]
#[tracing::instrument]
async fn drop_prefix_no_match(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();
    let value3 = "value3".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        10,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        11,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        12,
        value3.clone(),
        None,
    )
    .await
    .unwrap();

    is.drop_prefix("svc", "api", ns.ns.clone(), key1, 5)
        .await
        .unwrap();
    let result = is
        .read("svc", "api", "entity", ns.ns.clone(), key1, 1, 100)
        .await
        .unwrap();

    assert_eq!(result, vec![(10, value1), (11, value2), (12, value3)]);
}

#[test]
#[tracing::instrument]
async fn drop_prefix_partial(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();
    let value3 = "value3".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        10,
        value1.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        11,
        value2.clone(),
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        12,
        value3.clone(),
        None,
    )
    .await
    .unwrap();

    is.drop_prefix("svc", "api", ns.ns.clone(), key1, 10)
        .await
        .unwrap();
    let result = is
        .read("svc", "api", "entity", ns.ns.clone(), key1, 1, 100)
        .await
        .unwrap();

    assert_eq!(result, vec![(11, value2), (12, value3)]);
}

#[test]
#[tracing::instrument]
async fn drop_prefix_full(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
    #[tagged_as("ns2")] ns2: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;

    let key1 = "key1";
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();
    let value3 = "value3".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        10,
        value1,
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        11,
        value2,
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        12,
        value3,
        None,
    )
    .await
    .unwrap();

    is.drop_prefix("svc", "api", ns.ns.clone(), key1, 20)
        .await
        .unwrap();
    let result = is
        .read("svc", "api", "entity", ns.ns.clone(), key1, 1, 100)
        .await
        .unwrap();

    assert_eq!(result, vec![]);
}

// ---------------------------------------------------------------------------------------------
// The shard-epoch fence.
//
// Every test below runs against all five backends. The ones that cannot fence (redis, in-memory)
// must behave exactly as they did before the epoch argument existed - accept the write and ignore
// the epoch - so each test asserts both halves rather than being skipped for them.
// ---------------------------------------------------------------------------------------------

fn assert_fenced(
    result: Result<(), IndexedStorageError>,
    expected_epoch: u64,
    actual_epoch: Option<u64>,
) {
    match result {
        Err(IndexedStorageError::Fenced {
            expected, actual, ..
        }) => {
            assert_eq!(expected, ShardEpoch(expected_epoch), "expected epoch");
            assert_eq!(actual, actual_epoch.map(ShardEpoch), "stored epoch");
        }
        other => panic!("expected a Fenced error, got {other:?}"),
    }
}

#[test]
#[tracing::instrument]
async fn append_with_the_recorded_epoch_is_accepted(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-match";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7))
        .await
        .unwrap();
    is.append_many(
        "svc",
        "api",
        "entity",
        &ns.ns,
        key,
        Arc::from([
            (1, Bytes::from_static(b"a")),
            (2, Bytes::from_static(b"b")),
            (3, Bytes::from_static(b"c")),
        ]),
        Some(ShardEpoch(7)),
    )
    .await
    .unwrap();

    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        3
    );
}

#[test]
#[tracing::instrument]
async fn a_stale_epoch_append_is_refused_and_writes_nothing(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-stale";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8))
        .await
        .unwrap();
    let result = is
        .append_many(
            "svc",
            "api",
            "entity",
            &ns.ns,
            key,
            Arc::from([
                (1, Bytes::from_static(b"a")),
                (2, Bytes::from_static(b"b")),
                (3, Bytes::from_static(b"c")),
            ]),
            Some(ShardEpoch(7)),
        )
        .await;

    let length = is.length("svc", "api", ns.ns.clone(), key).await.unwrap();
    assert_fenced(result, 7, Some(8));
    // The whole batch is rolled back, not the tail of it.
    assert_eq!(length, 0, "a refused batch must leave no entry behind");
}

#[test]
#[tracing::instrument]
async fn another_writer_at_the_same_epoch_is_refused(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // A shard manager that lost its state mints from zero again and can hand a live owner's epoch
    // to somebody else. The epoch alone cannot separate them, so the row's writer does: the owner
    // holds it, and the newcomer is refused at the open rather than sharing the generation.
    let (owner, newcomer) = is.get_two_writers().await;
    let key = "fence-two-writers";

    owner
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4))
        .await
        .unwrap();

    let claim = newcomer
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4))
        .await;

    match claim {
        Err(IndexedStorageError::Fenced {
            expected,
            actual,
            writer_conflict,
            ..
        }) => {
            assert_eq!(expected, ShardEpoch(4), "expected epoch");
            assert_eq!(actual, Some(ShardEpoch(4)), "stored epoch");
            assert!(
                writer_conflict,
                "the epochs match, so the refusal has to name the writer as the reason - that is \
                 what tells the shard manager to mint past this epoch rather than leave it shared"
            );
        }
        other => panic!("expected a Fenced error, got {other:?}"),
    }

    // And the newcomer cannot write behind the owner's back either.
    let append = newcomer
        .append_many(
            "svc",
            "api",
            "entity",
            &ns.ns,
            key,
            Arc::from([(1, Bytes::from_static(b"a"))]),
            Some(ShardEpoch(4)),
        )
        .await;
    match append {
        Err(IndexedStorageError::Fenced {
            writer_conflict, ..
        }) => assert!(writer_conflict),
        other => panic!("expected a Fenced error, got {other:?}"),
    }
    assert_eq!(
        owner
            .length("svc", "api", ns.ns.clone(), key)
            .await
            .unwrap(),
        0,
        "a refused append writes nothing"
    );

    // The owner is untouched by the attempt.
    owner
        .append_many(
            "svc",
            "api",
            "entity",
            &ns.ns,
            key,
            Arc::from([(1, Bytes::from_static(b"a"))]),
            Some(ShardEpoch(4)),
        )
        .await
        .unwrap();
}

#[test]
#[tracing::instrument]
async fn the_same_writer_re_opens_at_the_same_epoch(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // The ordinary case the writer column must not break: one process re-opening an oplog it
    // already holds, at the epoch it already holds, which happens on every cache eviction.
    let is = is.get_indexed_storage().await;
    let key = "fence-reopen";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4))
        .await
        .unwrap();
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4))
        .await
        .unwrap();
    is.append_many(
        "svc",
        "api",
        "entity",
        &ns.ns,
        key,
        Arc::from([(1, Bytes::from_static(b"a"))]),
        Some(ShardEpoch(4)),
    )
    .await
    .unwrap();
}

#[test]
#[tracing::instrument]
async fn a_newcomer_minted_above_the_collision_takes_the_oplog_over(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // The repair the refusal above sets off: the newcomer reports the collision, the shard manager
    // mints past it, and the higher epoch takes the oplog over - at which point the old owner is
    // the one being refused.
    let (owner, newcomer) = is.get_two_writers().await;
    let key = "fence-re-mint";

    owner
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4))
        .await
        .unwrap();
    newcomer
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5))
        .await
        .unwrap();
    newcomer
        .append_many(
            "svc",
            "api",
            "entity",
            &ns.ns,
            key,
            Arc::from([(1, Bytes::from_static(b"a"))]),
            Some(ShardEpoch(5)),
        )
        .await
        .unwrap();

    let refused = owner
        .append_many(
            "svc",
            "api",
            "entity",
            &ns.ns,
            key,
            Arc::from([(2, Bytes::from_static(b"b"))]),
            Some(ShardEpoch(4)),
        )
        .await;
    match refused {
        Err(IndexedStorageError::Fenced {
            actual,
            writer_conflict,
            ..
        }) => {
            assert_eq!(actual, Some(ShardEpoch(5)));
            assert!(
                !writer_conflict,
                "this one is an ordinary takeover, not two writers on one epoch"
            );
        }
        other => panic!("expected a Fenced error, got {other:?}"),
    }
}

#[test]
#[tracing::instrument]
async fn an_append_ahead_of_the_recorded_epoch_is_refused(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-ahead";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5))
        .await
        .unwrap();

    // The check is equality, not "at least": a record behind the asserted epoch means the open
    // that should have raised it never ran, so the write is not ours to make. Both call shapes, so
    // a single-entry path that parts from the batch path cannot quietly relax the check.
    let batch = is
        .append_many(
            "svc",
            "api",
            "entity",
            &ns.ns,
            key,
            Arc::from([
                (1, Bytes::from_static(b"a")),
                (2, Bytes::from_static(b"b")),
                (3, Bytes::from_static(b"c")),
            ]),
            Some(ShardEpoch(6)),
        )
        .await;
    let batch_length = is.length("svc", "api", ns.ns.clone(), key).await.unwrap();
    let single = is
        .append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            key,
            4,
            b"d".to_vec(),
            Some(ShardEpoch(6)),
        )
        .await;
    let length = is.length("svc", "api", ns.ns.clone(), key).await.unwrap();

    assert_fenced(batch, 6, Some(5));
    assert_eq!(
        batch_length, 0,
        "a refused batch must leave no entry behind"
    );
    assert_fenced(single, 6, Some(5));
    assert_eq!(length, 0, "a refused append must leave no entry behind");
}

#[test]
#[tracing::instrument]
async fn an_append_without_a_recorded_epoch_is_refused(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-absent";

    // No upsert. Epoch 0 is a perfectly valid epoch, so this also pins that an absent row is not
    // silently treated as zero.
    let result = is
        .append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            key,
            1,
            b"a".to_vec(),
            Some(ShardEpoch(0)),
        )
        .await;

    let length = is.length("svc", "api", ns.ns.clone(), key).await.unwrap();
    assert_fenced(result, 0, None);
    assert_eq!(length, 0);
}

#[test]
#[tracing::instrument]
async fn an_unfenced_append_ignores_the_recorded_epoch(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-none";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(9))
        .await
        .unwrap();
    // `None` asserts nothing: it is what the archive layers and generic callers pass.
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key,
        1,
        b"a".to_vec(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        1
    );
}

#[test]
#[tracing::instrument]
async fn the_recorded_epoch_only_ever_climbs(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-monotonic";

    // Rising and repeated epochs are accepted ...
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5))
        .await
        .unwrap();
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5))
        .await
        .unwrap();
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(9))
        .await
        .unwrap();

    // ... a falling one is not, or a zombie could re-open at its stale epoch and un-fence itself
    // against the current owner.
    let result = is
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8))
        .await;

    assert_fenced(result, 8, Some(9));
    // and the rejected upsert left the record alone
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key,
        1,
        b"a".to_vec(),
        Some(ShardEpoch(9)),
    )
    .await
    .unwrap();
    assert_fenced(
        is.append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            key,
            2,
            b"b".to_vec(),
            Some(ShardEpoch(8)),
        )
        .await,
        8,
        Some(9),
    );
}

#[test]
#[tracing::instrument]
async fn deleting_the_recorded_epoch_fences_later_writes(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-deleted";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(3))
        .await
        .unwrap();
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, None)
        .await
        .unwrap();
    // Idempotent: deleting again is not an error.
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, None)
        .await
        .unwrap();

    let result = is
        .append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            key,
            1,
            b"a".to_vec(),
            Some(ShardEpoch(3)),
        )
        .await;

    assert_fenced(result, 3, None);
}

#[test]
#[tracing::instrument]
async fn a_deleted_record_does_not_remember_its_epoch(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-forgotten";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(9))
        .await
        .unwrap();
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, None)
        .await
        .unwrap();

    // The documented limit of the fence: the forward-only rule lives on the record, so once the
    // record is gone a lower epoch than the one it held is recorded and written through. Closing
    // this needs the delete to keep the epoch, which changes this test on purpose.
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8))
        .await
        .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key,
        1,
        b"a".to_vec(),
        Some(ShardEpoch(8)),
    )
    .await
    .unwrap();

    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        1
    );
}

async fn append_fenced(
    is: &Arc<dyn IndexedStorage + Send + Sync>,
    ns: &IndexedStorageNamespace,
    key: &str,
    ids: &[u64],
    epoch: Option<ShardEpoch>,
) -> Result<(), IndexedStorageError> {
    let pairs: Vec<(u64, Bytes)> = ids
        .iter()
        .map(|id| (*id, Bytes::from(id.to_string())))
        .collect();
    is.append_many("svc", "api", "entity", ns, key, Arc::from(pairs), epoch)
        .await
}

#[test]
#[tracing::instrument]
async fn the_recorded_writer_deletes_the_key_and_its_record(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-delete-owner";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5))
        .await
        .unwrap();
    append_fenced(&is, &ns.ns, key, &[1, 2], Some(ShardEpoch(5)))
        .await
        .unwrap();

    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(5)))
        .await
        .unwrap();

    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        0
    );
    // The record went with the entries, so the old epoch writes nothing back.
    assert_fenced(
        append_fenced(&is, &ns.ns, key, &[3], Some(ShardEpoch(5))).await,
        5,
        None,
    );
}

#[test]
#[tracing::instrument]
async fn a_delete_by_a_writer_that_lost_the_key_deletes_nothing(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // A deletion that outlived its writer's hold on the key: the new holder took it at a higher
    // epoch and wrote to it. The stale delete must leave both the record and the entries alone.
    let (stale, owner) = is.get_two_writers().await;
    let key = "fence-delete-stale";

    stale
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5))
        .await
        .unwrap();
    append_fenced(&stale, &ns.ns, key, &[1], Some(ShardEpoch(5)))
        .await
        .unwrap();
    owner
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(6))
        .await
        .unwrap();
    append_fenced(&owner, &ns.ns, key, &[2], Some(ShardEpoch(6)))
        .await
        .unwrap();

    assert_fenced(
        stale
            .delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(5)))
            .await,
        5,
        Some(6),
    );

    assert_eq!(
        owner
            .length("svc", "api", ns.ns.clone(), key)
            .await
            .unwrap(),
        2,
        "a refused delete removes no entry"
    );
    append_fenced(&owner, &ns.ns, key, &[3], Some(ShardEpoch(6)))
        .await
        .expect("a refused delete leaves the new holder's record in place");
}

#[test]
#[tracing::instrument]
async fn a_delete_asserting_an_epoch_on_a_key_without_a_record_deletes_nothing(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = "fence-delete-unrecorded";

    append_fenced(&is, &ns.ns, key, &[1], None).await.unwrap();

    assert_fenced(
        is.delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(1)))
            .await,
        1,
        None,
    );
    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        1
    );
}

#[test]
#[tracing::instrument]
async fn an_unfenced_delete_removes_the_key_and_its_record(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let (first, second) = is.get_two_writers().await;
    let key = "fence-delete-unfenced";

    first
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7))
        .await
        .unwrap();
    append_fenced(&first, &ns.ns, key, &[1, 2], Some(ShardEpoch(7)))
        .await
        .unwrap();

    // Asserting nothing, so it does not matter that another writer holds the record.
    second
        .delete_with_epoch("svc", "api", ns.ns.clone(), key, None)
        .await
        .unwrap();

    assert_eq!(
        first
            .length("svc", "api", ns.ns.clone(), key)
            .await
            .unwrap(),
        0
    );
    assert_fenced(
        append_fenced(&first, &ns.ns, key, &[3], Some(ShardEpoch(7))).await,
        7,
        None,
    );
}

#[test]
#[tracing::instrument]
async fn an_empty_batch_is_accepted_whatever_epoch_it_asserts(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // Nothing to write is nothing to fence: every backend agrees, stale epoch or not.
    let is = is.get_indexed_storage().await;
    let key = "fence-empty-batch";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5))
        .await
        .unwrap();

    append_fenced(&is, &ns.ns, key, &[], Some(ShardEpoch(4)))
        .await
        .expect("an empty batch writes nothing, so it is not refused");
    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        0
    );
}

#[test]
#[tracing::instrument]
async fn a_repeated_id_in_a_staged_batch_is_a_conflict(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    // A stage is an oplog insert like the visible oplog, in a batch as in a single append: a batch
    // re-sent after an indeterminate write collides on its first id.
    let is = is.get_indexed_storage().await;
    let staged = IndexedStorageNamespace::StagedOpLog {
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "staged-conflict".into(),
        },
        agent_mode: AgentMode::Durable,
    };
    let key = "staged-conflict";

    append_fenced(&is, &staged, key, &[1, 2], None)
        .await
        .unwrap();

    // Only the classification is asserted: an unfenced Redis batch is a pipeline, and its callers
    // reconcile a partial one by reading it back.
    match append_fenced(&is, &staged, key, &[2, 3], None).await {
        Err(IndexedStorageError::Conflict(_)) => {}
        other => panic!("expected a Conflict, got {other:?}"),
    }
}

#[test]
#[tracing::instrument]
async fn a_failed_batch_leaves_no_partial_write(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // What makes "the fence is checked once per batch" true rather than "once per entry": a
    // backend that loops single appends would leave the entries before the failure behind.
    let is = is.get_indexed_storage().await;
    let key = "batch-atomicity";

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(1))
        .await
        .unwrap();
    is.append_many(
        "svc",
        "api",
        "entity",
        &ns.ns,
        key,
        Arc::from([(1, Bytes::from_static(b"a")), (2, Bytes::from_static(b"b"))]),
        Some(ShardEpoch(1)),
    )
    .await
    .unwrap();

    // id 1 already exists, so the second entry of this batch violates the primary key.
    let result = is
        .append_many(
            "svc",
            "api",
            "entity",
            &ns.ns,
            key,
            Arc::from([
                (3, Bytes::from_static(b"c")),
                (1, Bytes::from_static(b"dup")),
                (4, Bytes::from_static(b"d")),
            ]),
            Some(ShardEpoch(1)),
        )
        .await;

    assert!(result.is_err(), "a duplicate id must fail the batch");
    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        2,
        "the failed batch must not have written its first entry"
    );
}
