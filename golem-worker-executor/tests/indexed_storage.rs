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
use golem_common::config::{DbPostgresConfig, RedisConfig};
use golem_common::model::AgentId;
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
    IndexedStorage, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace, ScanCursor,
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
            tls: false,
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
}

#[test_dep(tagged_as = "sqlite")]
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
}

#[test_dep(tagged_as = "multi_sqlite")]
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

#[async_trait]
impl GetIndexedStorage for PostgresIndexedStorageWrapper {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
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

        let config = IndexedStoragePostgresConfig {
            postgres,
            drop_prefix_delete_batch_size: 1024,
            max_concurrent_ops: None,
        };

        let storage = PostgresIndexedStorage::configured(&config)
            .await
            .expect("Cannot create postgres indexed storage");

        Arc::new(storage)
    }
}

#[test_dep(tagged_as = "postgres")]
async fn postgres_storage(
    _deps: &WorkerExecutorTestDependencies,
) -> Arc<dyn GetIndexedStorage + Send + Sync> {
    let unique_network_id = Uuid::new_v4().to_string();
    let postgres = DockerPostgresRdb::new(&unique_network_id, false).await;
    Arc::new(PostgresIndexedStorageWrapper { postgres })
}

/// A compressed level nothing else writes to, so a walk over it has a fixed set of keys to be
/// right about.
const SCAN_STABLE_LEVEL: usize = 97;

#[derive(Debug, Clone)]
struct IndexedStorageNamespaces {
    ns: IndexedStorageNamespace,
    ns_other: IndexedStorageNamespace,
    meta: IndexedStorageMetaNamespace,
}

#[test_dep(tagged_as = "ns1")]
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

#[test_dep(tagged_as = "ns2")]
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
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1)
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

    is.append("svc", "api", "entity", ns1.ns.clone(), key1, 1, value1)
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

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    let result1 = is
        .append("svc", "api", "entity", ns.ns.clone(), key1, 1, value2)
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
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 4, value1)
        .await
        .unwrap();
    let result2 = is.length("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 8, value2)
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

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key2, 1, value2)
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

/// Pins the contract `IndexedStorage::scan_stable` exists for.
///
/// `scan` cannot be paged by a caller that deletes what it is handed: its cursor is a position on
/// every backend that has one, so a delete behind the cursor shifts the rest down and the next page
/// steps over that many keys nothing has looked at. `scan_stable` resumes by seeking instead, and
/// every backend has to honour that, whether it seeks on a key, walks its files, or falls back to
/// an iteration protocol that already tolerates deletion.
///
/// The keys are spread over six agents rather than one, because the multi-SQLite backend puts each
/// agent in its own file and walks files rather than keys. With one agent its whole walk is a
/// single file and none of that is exercised.
///
/// A second namespace is deleted from alongside the first. It stands for the lower oplog layers an
/// archive step drains on its way down: the caller removes keys there too, and none of that may
/// disturb the walk in progress.
///
/// The walk needs a meta-namespace nothing else writes to, so it takes a compressed level of its
/// own. The shared one carries whatever the tests running beside this are appending and deleting,
/// which leaves the walk with no fixed set of keys to be right about.
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
        is.append("svc", "api", "entity", swept.clone(), key, 1, b"v".to_vec())
            .await
            .unwrap();
        is.append("svc", "api", "entity", below.clone(), key, 1, b"v".to_vec())
            .await
            .unwrap();
    }

    // Take a page, delete what it handed back along with that key's entry in the layer below, carry
    // on from the token. The sweep's tick in miniature.
    let mut seen: Vec<String> = Vec::new();
    let mut resume = None;
    let mut terminated = false;
    for _ in 0..256 {
        // Through the labelled wrapper, which is how every caller reaches this.
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

    // Redis may hand a key back more than once, so this is the strongest thing every backend
    // owes: nothing but the planted keys, and all of them.
    for key in &seen {
        assert!(
            planted.iter().any(|(_, _, planted)| planted == key),
            "the walk returned {key}, which belongs to no namespace it was pointed at"
        );
    }
}

/// The multi-SQLite walk pages over files rather than keys: its token names the last file it
/// finished, and it is done when the file list runs out. A drained namespace keeps every one of
/// its files, since nothing removes them, so that list stays as long as it ever was and the walk
/// still has to cross it a budget at a time.
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

/// A file created after a listing was cached still shows up in the next walk.
///
/// The listing is cached because re-reading and re-sorting the whole directory per page made a walk
/// quadratic in the file count, and a directory holds one file per agent that has ever had entries.
/// Nothing may go missing for that: this process is the only writer to its own directory, so
/// creating a file has to drop what the cache holds.
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
        is.append("svc", "api", "entity", namespace, &key, 1, b"v".to_vec())
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
        )
        .await
        .unwrap();
    }

    let last = is
        .last("svc", "api", "entity", ns.ns.clone(), &key)
        .await
        .unwrap();
    // Through the labelled wrapper, which is how every caller reaches this.
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

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key2, 1, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key3, 1, value3)
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

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key2, 1, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key3, 1, value3)
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
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 1, value1)
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

    is.append("svc", "api", "entity", ns1.ns.clone(), key1, 1, value1)
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
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 5, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 7, value2)
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

    is.append("svc", "api", "entity", ns.ns.clone(), key1, 10, value1)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 11, value2)
        .await
        .unwrap();
    is.append("svc", "api", "entity", ns.ns.clone(), key1, 12, value3)
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
