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
use futures::future::BoxFuture;
use golem_common::config::{DbPostgresConfig, RedisConfig};
use golem_common::model::ShardEpoch;
use golem_common::model::agent::AgentMode;
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
    IndexedStorageNamespace, ScanResume,
};
use golem_worker_executor_test_utils::WorkerExecutorTestDependencies;
use pretty_assertions::assert_eq;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use url::Url;
use uuid::Uuid;

type OpenIndexedStorage =
    Box<dyn Fn() -> BoxFuture<'static, Arc<dyn IndexedStorage + Send + Sync>> + Send + Sync>;

#[test]
#[test_r::timeout("120s")]
async fn absent_lifecycle_claim_removes_epoch(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    let state = sealed_index_state(AgentMode::Ephemeral);
    let key = state.owned_agent_id.storage_key();
    let ns = state.primary_namespace();
    assert!(
        !storage
            .set_key_epoch(
                "test",
                "observe",
                ns.clone(),
                &key,
                ShardEpoch(8),
                &[],
                true
            )
            .await
            .unwrap()
    );
    let write = storage
        .append(
            "test",
            "write",
            "entry",
            ns,
            &key,
            1,
            vec![9],
            Some(ShardEpoch(8)),
            None,
        )
        .await;
    assert!(
        matches!(write, Err(IndexedStorageError::Fenced { actual: None, .. })),
        "an absent observation-only claim must remove its epoch, got {write:?}"
    );
}

#[test]
#[test_r::timeout("120s")]
async fn lifecycle_claim_reports_explicit_namespace_presence(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    let state = sealed_index_state(AgentMode::Ephemeral);
    let key = state.owned_agent_id.storage_key();
    let extra = state.manifest_namespace(99);
    storage
        .append(
            "test",
            "seed",
            "entry",
            extra.clone(),
            &key,
            1,
            vec![9],
            None,
            None,
        )
        .await
        .unwrap();
    let present = storage
        .set_key_epoch(
            "test",
            "claim",
            state.primary_namespace(),
            &key,
            ShardEpoch(8),
            &[extra],
            true,
        )
        .await
        .unwrap();
    assert!(present, "the explicit presence namespace contains data");
}

#[async_trait]
trait GetIndexedStorage: Debug {
    async fn get_indexed_storage(&self) -> Arc<dyn IndexedStorage + Send + Sync> {
        let open = self.opener().await;
        open().await
    }

    /// Opens clients onto one store. Persistent backends create independent clients;
    /// the in-memory backend retains and returns the same store.
    async fn opener(&self) -> OpenIndexedStorage;

    /// Two handles onto the SAME store, as two executors racing over one key have.
    async fn get_two_writers(
        &self,
    ) -> (
        Arc<dyn IndexedStorage + Send + Sync>,
        Arc<dyn IndexedStorage + Send + Sync>,
    ) {
        let open = self.opener().await;
        (open().await, open().await)
    }
}

struct InMemoryIndexedStorageWrapper;

impl Debug for InMemoryIndexedStorageWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InMemoryIndexedStorageWrapper")
    }
}

#[async_trait]
impl GetIndexedStorage for InMemoryIndexedStorageWrapper {
    async fn opener(&self) -> OpenIndexedStorage {
        let store: Arc<dyn IndexedStorage + Send + Sync> = Arc::new(InMemoryIndexedStorage::new());
        Box::new(move || {
            let store = store.clone();
            Box::pin(async move { store })
        })
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
    async fn opener(&self) -> OpenIndexedStorage {
        let prefix = Uuid::new_v4().to_string();
        let redis = self.redis.clone();
        Box::new(move || {
            let wrapper = RedisIndexedStorageWrapper {
                redis: redis.clone(),
            };
            let prefix = prefix.clone();
            Box::pin(async move {
                Arc::new(RedisIndexedStorage::new(wrapper.pool(&prefix).await))
                    as Arc<dyn IndexedStorage + Send + Sync>
            })
        })
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
    async fn opener(&self) -> OpenIndexedStorage {
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
        Box::new(move || {
            let config = config.clone();
            Box::pin(async move {
                Arc::new(SqliteIndexedStorage::configured(&config).await.unwrap())
                    as Arc<dyn IndexedStorage + Send + Sync>
            })
        })
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
    async fn opener(&self) -> OpenIndexedStorage {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().to_path_buf();
        self.tempdirs.lock().unwrap().push(tempdir);
        Box::new(move || {
            let path = path.clone();
            Box::pin(async move {
                Arc::new(MultiSqliteIndexedStorage::new(&path, 10, true))
                    as Arc<dyn IndexedStorage + Send + Sync>
            })
        })
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
    /// second storage can be opened onto the same database.
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
    async fn opener(&self) -> OpenIndexedStorage {
        let config = self.fresh_database().await;
        Box::new(move || {
            let config = config.clone();
            Box::pin(async move {
                Arc::new(
                    PostgresIndexedStorage::configured(&config)
                        .await
                        .expect("Cannot create postgres indexed storage"),
                ) as Arc<dyn IndexedStorage + Send + Sync>
            })
        })
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

const KEY1: &str = "12345678-0000-0000-0000-000000000000/00000001-0000-8000-8000-000000000001";
const KEY2: &str = "12345678-0000-0000-0000-000000000000/00000002-0000-8000-8000-000000000002";
const KEY3: &str = "12345678-0000-0000-0000-000000000000/00000001-0000-8000-8000-000000000003";
const KEY_PREFIX: &str = "12345678-0000-0000-0000-000000000000/00000001-";

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
            owned_agent_id: crate::fresh_owned_agent_id(),
            agent_mode: AgentMode::Durable,
        },
        ns_other: IndexedStorageNamespace::OpLog {
            owned_agent_id: crate::fresh_owned_agent_id(),
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
            owned_agent_id: crate::fresh_owned_agent_id(),
            agent_mode: AgentMode::Durable,
            level: 1,
        },
        ns_other: IndexedStorageNamespace::CompressedOpLog {
            owned_agent_id: crate::fresh_owned_agent_id(),
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
define_matrix_dimension!(sql_is: Arc<dyn GetIndexedStorage + Send + Sync> -> "sqlite", "postgres");
define_matrix_dimension!(persistent_is: Arc<dyn GetIndexedStorage + Send + Sync> -> "redis", "sqlite", "multi_sqlite", "postgres");

fn test_agent_metadata() -> golem_common::model::AgentMetadata {
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::OwnerKind;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::oplog::{CreateRequest, CreateRequestKind};
    use golem_common::model::{AgentFingerprint, AgentMetadata, AgentStatusRecord, Timestamp};

    let component_id = golem_common::model::component::ComponentId::new();
    AgentMetadata {
        component_id,
        agent_type_name: None,
        canonical_key: None,
        agent_id: crate::fresh_owned_agent_id().agent_id,
        owner_kind: OwnerKind::ComponentAgent,
        env: vec![],
        environment_id: EnvironmentId::new(),
        created_by: AccountId::new(),
        created_by_email: AccountEmail::new("test@golem"),
        config: vec![],
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord::default(),
        constructor_parameters: None,
        fingerprint: AgentFingerprint::new(),
        agent_mode: AgentMode::Durable,
        standalone_tool: None,
        create_request: CreateRequest::new(
            CreateRequestKind::GetOrCreate,
            None,
            golem_worker_executor_test_utils::callers::anonymous(),
            &InvocationContextStack::fresh_rounded(),
        ),
    }
}

async fn primary_oplog_service(
    storage: Arc<dyn IndexedStorage + Send + Sync>,
) -> golem_worker_executor::services::oplog::PrimaryOplogService {
    use golem_common::model::RetryConfig;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_worker_executor::services::oplog::PrimaryOplogService;

    PrimaryOplogService::new(
        storage,
        Arc::new(InMemoryBlobStorage::new()),
        100,
        100,
        1024,
        32,
        RetryConfig::default(),
    )
    .await
}

async fn read_records(
    storage: &dyn IndexedStorage,
    namespace: &IndexedStorageNamespace,
    key: &str,
) -> Vec<(u64, Vec<u8>)> {
    storage
        .read("test", "reopen", "entry", namespace.clone(), key, 1, 10)
        .await
        .unwrap()
}

async fn reserve_test_stage(storage: &dyn IndexedStorage) -> (String, i64) {
    let owned = crate::fresh_owned_agent_id();
    let stage = Uuid::new_v4();
    let token = storage
        .reserve_stage("test", "reserve", &owned, stage, None)
        .await
        .unwrap()
        .unwrap();
    (format!("{}#{stage}", owned.storage_key()), token)
}

#[test]
#[test_r::timeout("120s")]
async fn stage_reservation_admits_only_an_agent_without_any_data(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for family in 0..4 {
            let owned = crate::fresh_owned_agent_id();
            let key = owned.storage_key();
            let namespace = match family {
                0 => IndexedStorageNamespace::OpLog {
                    owned_agent_id: owned.clone(),
                    agent_mode: mode,
                },
                1 => IndexedStorageNamespace::CompressedOpLog {
                    owned_agent_id: owned.clone(),
                    agent_mode: mode,
                    level: 99,
                },
                2 => IndexedStorageNamespace::BlobOplogManifest {
                    owned_agent_id: owned.clone(),
                    agent_mode: mode,
                    level: 99,
                },
                _ => IndexedStorageNamespace::SealedPresent {
                    owned_agent_id: owned.clone(),
                    agent_mode: mode,
                },
            };
            let id = if family == 3 { 0 } else { 17 };
            storage
                .append(
                    "test",
                    "seed_presence",
                    "entry",
                    namespace.clone(),
                    &key,
                    id,
                    vec![71],
                    None,
                    None,
                )
                .await
                .unwrap();
            let stage_id = Uuid::new_v4();
            assert!(
                storage
                    .reserve_stage("test", "refuse", &owned, stage_id, Some(ShardEpoch(8)))
                    .await
                    .unwrap()
                    .is_none()
            );
            let staged = IndexedStorageNamespace::StagedOpLog {
                owned_agent_id: owned.clone(),
                agent_mode: AgentMode::Durable,
            };
            let stage_key = format!("{key}#{stage_id}");
            assert!(
                !storage
                    .exists("test", "no_reservation", staged, &stage_key)
                    .await
                    .unwrap()
            );
            assert_eq!(
                storage
                    .read(
                        "test",
                        "unchanged",
                        "entry",
                        namespace.clone(),
                        &key,
                        id,
                        id
                    )
                    .await
                    .unwrap(),
                vec![(id, vec![71])]
            );
            // Even refused admission claims the primary epoch. Lower epochs cannot erase it.
            assert!(matches!(
                storage
                    .reserve_stage("test", "stale", &owned, stage_id, Some(ShardEpoch(7)))
                    .await,
                Err(IndexedStorageError::Fenced {
                    actual: Some(ShardEpoch(8)),
                    ..
                })
            ));
            storage
                .delete("test", "clear", namespace, &key)
                .await
                .unwrap();
            assert!(
                storage
                    .reserve_stage("test", "admit", &owned, stage_id, Some(ShardEpoch(8)))
                    .await
                    .unwrap()
                    .is_some()
            );
        }
    }

    let owned = crate::fresh_owned_agent_id();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let (a, b) = tokio::join!(
        storage.reserve_stage("test", "race", &owned, first, None),
        storage.reserve_stage("test", "race", &owned, second, None),
    );
    let a = a.unwrap().unwrap();
    let b = b.unwrap().unwrap();
    assert_ne!(a, b);
    let staged = IndexedStorageNamespace::StagedOpLog {
        owned_agent_id: owned.clone(),
        agent_mode: AgentMode::Durable,
    };
    let stage_key = format!("{}#{first}", owned.storage_key());
    assert_eq!(
        storage
            .read(
                "test",
                "reservation",
                "entry",
                staged.clone(),
                &stage_key,
                0,
                99
            )
            .await
            .unwrap(),
        vec![]
    );
    assert!(
        !storage
            .observe_agent("test", "presence", &owned)
            .await
            .unwrap()
            .has_data
    );
    assert!(
        storage
            .reserve_stage("test", "repeat", &owned, first, None)
            .await
            .unwrap()
            .is_some()
    );
    // None must not claim a primary epoch. Tokens are not indexed data.
    let primary = IndexedStorageNamespace::OpLog {
        owned_agent_id: owned.clone(),
        agent_mode: AgentMode::Durable,
    };
    assert!(matches!(
        storage
            .append(
                "test",
                "no_epoch",
                "entry",
                primary,
                &owned.storage_key(),
                1,
                vec![1],
                Some(ShardEpoch(0)),
                None
            )
            .await,
        Err(IndexedStorageError::Fenced { actual: None, .. })
    ));
}

#[test]
#[test_r::timeout("120s")]
async fn discarded_and_published_stages_cannot_be_recreated_by_old_writers(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    for publish in [false, true] {
        let owned = crate::fresh_owned_agent_id();
        let stage_id = Uuid::new_v4();
        let key = owned.storage_key();
        let stage_key = format!("{key}#{stage_id}");
        let staged = IndexedStorageNamespace::StagedOpLog {
            owned_agent_id: owned.clone(),
            agent_mode: AgentMode::Durable,
        };
        let primary = IndexedStorageNamespace::OpLog {
            owned_agent_id: owned.clone(),
            agent_mode: AgentMode::Durable,
        };
        assert!(matches!(
            storage
                .append(
                    "test",
                    "unreserved",
                    "entry",
                    staged.clone(),
                    &stage_key,
                    1,
                    vec![1],
                    None,
                    None
                )
                .await,
            Err(IndexedStorageError::StageInvalidated { .. })
        ));
        let token = storage
            .reserve_stage("test", "reserve", &owned, stage_id, Some(ShardEpoch(8)))
            .await
            .unwrap()
            .unwrap();
        storage
            .append(
                "test",
                "stage",
                "entry",
                staged.clone(),
                &stage_key,
                1,
                vec![71],
                None,
                Some(token),
            )
            .await
            .unwrap();
        if publish {
            assert!(
                storage
                    .move_if_absent(
                        "test",
                        "publish",
                        staged.clone(),
                        &stage_key,
                        primary.clone(),
                        &key,
                        1,
                        Some(token)
                    )
                    .await
                    .unwrap()
            );
            assert_eq!(
                storage
                    .read("test", "published", "entry", primary.clone(), &key, 0, 99)
                    .await
                    .unwrap(),
                vec![(1, vec![71])]
            );
        } else {
            storage
                .discard_stage("test", "discard", staged.clone(), &stage_key, Some(token))
                .await
                .unwrap();
        }
        for epoch in [None, Some(ShardEpoch(8))] {
            assert!(
                matches!(storage.append("test", "late_append", "entry", staged.clone(), &stage_key, 2, vec![99], epoch, Some(token)).await,
                Err(IndexedStorageError::StageInvalidated { key: invalidated }) if invalidated == stage_key)
            );
            let batches: [Arc<[(u64, Bytes)]>; 2] = [
                Arc::from([]),
                Arc::from([
                    (2, Bytes::from_static(b"late")),
                    (3, Bytes::from_static(b"batch")),
                ]),
            ];
            for pairs in batches {
                assert!(matches!(
                    storage
                        .append_many(
                            "test",
                            "late_batch",
                            "entry",
                            &staged,
                            &stage_key,
                            pairs,
                            epoch,
                            Some(token)
                        )
                        .await,
                    Err(IndexedStorageError::StageInvalidated { .. })
                ));
            }
        }
        assert!(matches!(
            storage
                .move_if_absent(
                    "test",
                    "late_publish",
                    staged.clone(),
                    &stage_key,
                    primary,
                    &key,
                    1,
                    Some(token)
                )
                .await,
            Err(IndexedStorageError::StageInvalidated { .. })
        ));
        assert!(
            !storage
                .exists("test", "not_recreated", staged, &stage_key)
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .observe_agent("test", "remaining_presence", &owned)
                .await
                .unwrap()
                .has_data,
            publish
        );
    }
}

#[test]
#[test_r::timeout("120s")]
async fn stage_discard_only_mutates_the_current_admission(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        let owned = crate::fresh_owned_agent_id();
        let staged = IndexedStorageNamespace::StagedOpLog {
            owned_agent_id: owned.clone(),
            agent_mode: AgentMode::Durable,
        };
        let key = format!("{}#{}", owned.storage_key(), Uuid::new_v4());
        storage
            .append(
                "test",
                "stage",
                "entry",
                staged.clone(),
                &key,
                1,
                vec![1],
                None,
                None,
            )
            .await
            .unwrap();
        storage
            .discard_stage("test", "discard", staged.clone(), &key, None)
            .await
            .unwrap();
        assert!(
            !storage
                .exists("test", "discarded", staged, &key)
                .await
                .unwrap()
        );
        return;
    }
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let owned = crate::fresh_owned_agent_id();
        let key = owned.storage_key();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let a_key = format!("{key}#{a}");
        let b_key = format!("{key}#{b}");
        let staged = IndexedStorageNamespace::StagedOpLog {
            owned_agent_id: owned.clone(),
            agent_mode: mode,
        };
        let primary = IndexedStorageNamespace::OpLog {
            owned_agent_id: owned.clone(),
            agent_mode: mode,
        };
        let a_token = storage
            .reserve_stage("test", "admit_a", &owned, a, None)
            .await
            .unwrap()
            .unwrap();
        storage
            .append(
                "test",
                "a_commit",
                "entry",
                staged.clone(),
                &a_key,
                1,
                vec![71],
                None,
                Some(a_token),
            )
            .await
            .unwrap();
        assert!(
            storage
                .reserve_stage("test", "refuse_b", &owned, b, None)
                .await
                .unwrap()
                .is_none()
        );
        storage
            .delete("test", "generic_delete_keeps_token", staged.clone(), &b_key)
            .await
            .unwrap();
        // Unadmitted cleanup owns neither the token nor rows, even if it reuses A's UUID.
        for cleanup_key in [&b_key, &a_key] {
            storage
                .with("test", "unadmitted_cleanup")
                .discard_stage(staged.clone(), cleanup_key, None)
                .await
                .unwrap();
        }
        assert_eq!(
            storage
                .read("test", "a_preserved", "entry", staged.clone(), &a_key, 0, 9)
                .await
                .unwrap(),
            vec![(1, vec![71])]
        );
        storage
            .append(
                "test",
                "a_still_writable",
                "entry",
                staged.clone(),
                &a_key,
                2,
                vec![72],
                None,
                Some(a_token),
            )
            .await
            .unwrap();
        assert!(
            storage
                .move_if_absent(
                    "test",
                    "a_publishes",
                    staged,
                    &a_key,
                    primary.clone(),
                    &key,
                    2,
                    Some(a_token)
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .read("test", "published", "entry", primary, &key, 0, 9)
                .await
                .unwrap(),
            vec![(1, vec![71]), (2, vec![72])]
        );

        let owned = crate::fresh_owned_agent_id();
        let key = owned.storage_key();
        let stage = Uuid::new_v4();
        let stage_key = format!("{key}#{stage}");
        let staged = IndexedStorageNamespace::StagedOpLog {
            owned_agent_id: owned.clone(),
            agent_mode: mode,
        };
        let a_token = storage
            .reserve_stage("test", "empty_a", &owned, stage, None)
            .await
            .unwrap()
            .unwrap();
        let b_token = storage
            .reserve_stage("test", "replacement_b", &owned, stage, None)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(a_token, b_token);
        storage
            .append(
                "test",
                "b_commit",
                "entry",
                staged.clone(),
                &stage_key,
                1,
                vec![81],
                None,
                Some(b_token),
            )
            .await
            .unwrap();
        storage
            .discard_stage(
                "test",
                "stale_a_cleanup",
                staged.clone(),
                &stage_key,
                Some(a_token),
            )
            .await
            .unwrap();
        storage
            .append(
                "test",
                "b_still_writable",
                "entry",
                staged.clone(),
                &stage_key,
                2,
                vec![82],
                None,
                Some(b_token),
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .read(
                    "test",
                    "replacement_rows_preserved",
                    "entry",
                    staged.clone(),
                    &stage_key,
                    0,
                    9
                )
                .await
                .unwrap(),
            vec![(1, vec![81]), (2, vec![82])]
        );
        storage
            .discard_stage(
                "test",
                "current_b_cleanup",
                staged.clone(),
                &stage_key,
                Some(b_token),
            )
            .await
            .unwrap();
        assert!(
            !storage
                .exists("test", "b_discarded", staged.clone(), &stage_key)
                .await
                .unwrap()
        );
        assert!(matches!(
            storage
                .append(
                    "test",
                    "b_invalidated",
                    "entry",
                    staged,
                    &stage_key,
                    3,
                    vec![83],
                    None,
                    Some(b_token)
                )
                .await,
            Err(IndexedStorageError::StageInvalidated { .. })
        ));
    }
}

#[test]
#[test_r::timeout("120s")]
async fn stage_tokens_replace_bare_admissions_and_require_exact_matches(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    let owned = crate::fresh_owned_agent_id();
    let key = owned.storage_key();
    let stage_id = Uuid::new_v4();
    let stage_key = format!("{key}#{stage_id}");
    let staged = IndexedStorageNamespace::StagedOpLog {
        owned_agent_id: owned.clone(),
        agent_mode: AgentMode::Ephemeral,
    };
    let primary = IndexedStorageNamespace::OpLog {
        owned_agent_id: owned.clone(),
        agent_mode: AgentMode::Durable,
    };
    let first = storage
        .reserve_stage("test", "first", &owned, stage_id, None)
        .await
        .unwrap()
        .unwrap();
    storage
        .append_many(
            "test",
            "empty",
            "entry",
            &staged,
            &stage_key,
            Arc::from([]),
            None,
            Some(first),
        )
        .await
        .unwrap();
    assert!(
        !storage
            .observe_agent("test", "token_not_data", &owned)
            .await
            .unwrap()
            .has_data
    );
    // The same stage UUID gets a fresh token; it cannot identify the admission by itself.
    let second = storage
        .reserve_stage("test", "replace", &owned, stage_id, None)
        .await
        .unwrap()
        .unwrap();
    assert!(first >= 0 && second >= 0);
    assert_ne!(first, second);
    assert!(
        !storage
            .remove_stage_reservation("test", "old_cleanup", &owned, Some(first))
            .await
            .unwrap()
    );
    for token in [None, Some(first)] {
        assert!(matches!(
            storage
                .append(
                    "test",
                    "stale",
                    "entry",
                    staged.clone(),
                    &stage_key,
                    1,
                    vec![1],
                    None,
                    token
                )
                .await,
            Err(IndexedStorageError::StageInvalidated { .. })
        ));
        assert!(matches!(
            storage
                .append_many(
                    "test",
                    "stale_empty",
                    "entry",
                    &staged,
                    &stage_key,
                    Arc::from([]),
                    None,
                    token
                )
                .await,
            Err(IndexedStorageError::StageInvalidated { .. })
        ));
        assert!(matches!(
            storage
                .move_if_absent(
                    "test",
                    "stale_publish",
                    staged.clone(),
                    &stage_key,
                    primary.clone(),
                    &key,
                    1,
                    token
                )
                .await,
            Err(IndexedStorageError::StageInvalidated { .. })
        ));
    }
    storage
        .append(
            "test",
            "current",
            "entry",
            staged.clone(),
            &stage_key,
            1,
            vec![71],
            None,
            Some(second),
        )
        .await
        .unwrap();
    assert!(
        storage
            .reserve_stage("test", "staged_data_refuses", &owned, Uuid::new_v4(), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        storage
            .move_if_absent(
                "test",
                "publish",
                staged.clone(),
                &stage_key,
                primary.clone(),
                &key,
                1,
                Some(second)
            )
            .await
            .unwrap()
    );
    assert_eq!(
        storage
            .read(
                "test",
                "no_token_row",
                "entry",
                primary.clone(),
                &key,
                0,
                99
            )
            .await
            .unwrap(),
        vec![(1, vec![71])]
    );
    assert!(
        !storage
            .remove_stage_reservation("test", "already_published", &owned, Some(second))
            .await
            .unwrap()
    );
    assert!(matches!(
        storage
            .move_if_absent(
                "test",
                "late_publish",
                staged.clone(),
                &stage_key,
                primary,
                &key,
                1,
                Some(second)
            )
            .await,
        Err(IndexedStorageError::StageInvalidated { .. })
    ));

    let owned = crate::fresh_owned_agent_id();
    let stage_key = format!("{}#{stage_id}", owned.storage_key());
    let token = storage
        .reserve_stage("test", "cleanup", &owned, stage_id, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        storage
            .with("test", "cleanup")
            .remove_stage_reservation(&owned, Some(token))
            .await
            .unwrap()
    );
    assert!(
        !storage
            .remove_stage_reservation("test", "cleanup_retry", &owned, None)
            .await
            .unwrap()
    );
    assert!(matches!(
        storage
            .append(
                "test",
                "cleaned_writer",
                "entry",
                staged,
                &stage_key,
                1,
                vec![1],
                None,
                Some(token)
            )
            .await,
        Err(IndexedStorageError::StageInvalidated { .. })
    ));
}

#[test]
#[test_r::timeout("120s")]
async fn initial_append_invalidates_the_stage_token_atomically(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for epoch in [None, Some(ShardEpoch(8))] {
            for batch in [false, true] {
                let owned = crate::fresh_owned_agent_id();
                let key = owned.storage_key();
                let stage_id = Uuid::new_v4();
                let stage_key = format!("{key}#{stage_id}");
                let staged = IndexedStorageNamespace::StagedOpLog {
                    owned_agent_id: owned.clone(),
                    agent_mode: AgentMode::Durable,
                };
                let primary = IndexedStorageNamespace::OpLog {
                    owned_agent_id: owned.clone(),
                    agent_mode: mode,
                };
                let token = storage
                    .reserve_stage("test", "reserve", &owned, stage_id, epoch)
                    .await
                    .unwrap()
                    .unwrap();
                if let Some(epoch) = epoch {
                    storage
                        .set_key_epoch("test", "claim", primary.clone(), &key, epoch, &[], false)
                        .await
                        .unwrap();
                }
                if batch {
                    storage
                        .append_many(
                            "test",
                            "initial_batch",
                            "entry",
                            &primary,
                            &key,
                            Arc::from([
                                (1, Bytes::from_static(b"initial")),
                                (2, Bytes::from_static(b"next")),
                            ]),
                            epoch,
                            None,
                        )
                        .await
                        .unwrap();
                } else {
                    storage
                        .append(
                            "test",
                            "initial",
                            "entry",
                            primary.clone(),
                            &key,
                            1,
                            b"initial".to_vec(),
                            epoch,
                            None,
                        )
                        .await
                        .unwrap();
                }
                assert!(
                    !storage
                        .remove_stage_reservation("test", "invalidated", &owned, Some(token))
                        .await
                        .unwrap()
                );
                assert!(matches!(
                    storage
                        .append(
                            "test",
                            "late_stage",
                            "entry",
                            staged.clone(),
                            &stage_key,
                            1,
                            vec![1],
                            None,
                            Some(token)
                        )
                        .await,
                    Err(IndexedStorageError::StageInvalidated { .. })
                ));
                assert!(matches!(
                    storage
                        .move_if_absent(
                            "test",
                            "late_publish",
                            staged,
                            &stage_key,
                            primary,
                            &key,
                            1,
                            Some(token)
                        )
                        .await,
                    Err(IndexedStorageError::StageInvalidated { .. })
                ));
            }
        }
    }
}

#[test]
#[test_r::timeout("120s")]
async fn stranded_stage_tokens_are_not_shard_epochs_or_transition_presence(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    use golem_worker_executor::storage::indexed::{ReleaseAgentResult, UnsealAgentResult};
    use std::time::{Duration, SystemTime};
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let state = sealed_index_state(mode);
        let owned = &state.owned_agent_id;
        let stage_id = Uuid::new_v4();
        let token = storage
            .reserve_stage("test", "stranded", owned, stage_id, None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !storage
                .observe_agent("test", "absent", owned)
                .await
                .unwrap()
                .has_data
        );
        assert_eq!(
            storage
                .release_agent("test", "empty_release", &state, ShardEpoch(6))
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
        assert!(
            !storage
                .remove_stage_reservation("test", "removed_by_release", owned, Some(token))
                .await
                .unwrap()
        );

        let token = storage
            .reserve_stage("test", "stranded_again", owned, stage_id, None)
            .await
            .unwrap()
            .unwrap();
        let deadline = SystemTime::now() + Duration::from_secs(30);
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "unseal",
                    &state,
                    ShardEpoch(8),
                    ShardEpoch(6),
                    deadline
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        assert!(
            !storage
                .remove_stage_reservation("test", "removed_by_unseal", owned, Some(token))
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "retry",
                    &state,
                    ShardEpoch(8),
                    ShardEpoch(6),
                    deadline
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        assert_eq!(
            storage
                .release_agent("test", "release", &state, ShardEpoch(8))
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
    }
}

async fn append_record(
    storage: &dyn IndexedStorage,
    namespace: &IndexedStorageNamespace,
    key: &str,
    id: u64,
    value: &[u8],
    epoch: Option<ShardEpoch>,
    stage_token: Option<i64>,
) -> Result<(), IndexedStorageError> {
    storage
        .append(
            "test",
            "append",
            "entry",
            namespace.clone(),
            key,
            id,
            value.to_vec(),
            epoch,
            stage_token,
        )
        .await
}

async fn publish_staged_records(
    storage: &dyn IndexedStorage,
    staged: &IndexedStorageNamespace,
    source_key: &str,
    visible: &IndexedStorageNamespace,
    target_key: &str,
    last_id: u64,
    stage_token: Option<i64>,
) -> bool {
    storage
        .with("test", "publish")
        .move_if_absent(
            staged.clone(),
            source_key,
            visible.clone(),
            target_key,
            last_id,
            stage_token,
        )
        .await
        .unwrap()
}

#[test]
#[test_r::timeout("120s")]
async fn committed_oplog_records_survive_cold_reopen(
    #[dimension(persistent_is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    use golem_common::model::oplog::{OplogEntry, OplogIndex};
    use golem_worker_executor::services::oplog::{CommitLevel, OplogService};

    let open = is.opener().await;
    let metadata = test_agent_metadata();
    let owned = metadata.owned_agent_id();
    let stage_id = Uuid::new_v4();
    let target_key = owned.storage_key();
    let key = format!("{target_key}#{stage_id}");
    let staged = IndexedStorageNamespace::StagedOpLog {
        owned_agent_id: owned.clone(),
        agent_mode: AgentMode::Durable,
    };
    let visible = IndexedStorageNamespace::OpLog {
        owned_agent_id: owned.clone(),
        agent_mode: AgentMode::Durable,
    };
    let first = OplogEntry::suspend().rounded();
    let second = OplogEntry::restart().rounded();
    let stage_token;
    {
        let service = primary_oplog_service(open().await).await;
        let writer = service
            .create_staged(&owned, AgentMode::Durable, stage_id, metadata)
            .await
            .unwrap();
        stage_token = writer.stage_token();
        writer.add(first.clone()).await.unwrap();
        writer.add(second.clone()).await.unwrap();
        assert_eq!(writer.commit(CommitLevel::Always).await.unwrap().len(), 2);
        let uncommitted = writer.add(OplogEntry::suspend().rounded()).await.unwrap();
        assert_eq!(uncommitted, OplogIndex::from_u64(3));
        let closed = writer.closed();
        drop(writer);
        closed.await.unwrap();
    }

    let storage = open().await;
    let expected = vec![
        (1, golem_common::serialization::serialize(&first).unwrap()),
        (2, golem_common::serialization::serialize(&second).unwrap()),
    ];
    assert_eq!(
        read_records(storage.as_ref(), &staged, &key).await,
        expected
    );
    assert!(
        !storage
            .exists("test", "hidden", visible.clone(), &target_key)
            .await
            .unwrap()
    );

    // Independent clients race to publish different complete stages into the same target.
    let competitor = open().await;
    let (competitor_key, competitor_token) = reserve_test_stage(competitor.as_ref()).await;
    append_record(
        competitor.as_ref(),
        &staged,
        &competitor_key,
        1,
        b"other",
        None,
        Some(competitor_token),
    )
    .await
    .unwrap();
    let (published, other) = tokio::join!(
        publish_staged_records(
            storage.as_ref(),
            &staged,
            &key,
            &visible,
            &target_key,
            2,
            stage_token
        ),
        publish_staged_records(
            competitor.as_ref(),
            &staged,
            &competitor_key,
            &visible,
            &target_key,
            1,
            Some(competitor_token)
        ),
    );
    assert_ne!(published, other, "exactly one publication must win");
    let expected = if published {
        expected
    } else {
        vec![(1, b"other".to_vec())]
    };
    let (loser, tip, loser_token) = if published {
        (competitor_key.as_str(), 1, Some(competitor_token))
    } else {
        (key.as_str(), 2, stage_token)
    };
    assert!(
        !publish_staged_records(
            competitor.as_ref(),
            &staged,
            loser,
            &visible,
            &target_key,
            tip,
            loser_token
        )
        .await
    );
    assert!(
        competitor
            .exists("test", "loser", staged, loser)
            .await
            .unwrap()
    );
    storage
        .set_key_epoch(
            "test",
            "owner",
            visible.clone(),
            &target_key,
            ShardEpoch(7),
            &[],
            false,
        )
        .await
        .unwrap();
    competitor
        .set_key_epoch(
            "test",
            "takeover",
            visible.clone(),
            &target_key,
            ShardEpoch(8),
            &[],
            false,
        )
        .await
        .unwrap();
    assert_fenced(
        append_record(
            storage.as_ref(),
            &visible,
            &target_key,
            3,
            b"stale",
            Some(ShardEpoch(7)),
            None,
        )
        .await,
        7,
        Some(8),
    );
    drop(storage);
    drop(competitor);

    let reopened = open().await;
    assert_eq!(
        read_records(reopened.as_ref(), &visible, &target_key).await,
        expected
    );
    assert_fenced(
        append_record(
            reopened.as_ref(),
            &visible,
            &target_key,
            3,
            b"stale",
            Some(ShardEpoch(7)),
            None,
        )
        .await,
        7,
        Some(8),
    );
    append_record(
        reopened.as_ref(),
        &visible,
        &target_key,
        3,
        b"current",
        Some(ShardEpoch(8)),
        None,
    )
    .await
    .unwrap();
    drop(reopened);
    let mut expected = expected;
    expected.push((3, b"current".to_vec()));
    assert_eq!(
        read_records(open().await.as_ref(), &visible, &target_key).await,
        expected
    );
}

#[test]
async fn staged_publication_preserves_atomic_visibility(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    let agent = crate::fresh_owned_agent_id();
    let mode = AgentMode::Durable;
    let staged = IndexedStorageNamespace::StagedOpLog {
        owned_agent_id: agent.clone(),
        agent_mode: mode,
    };
    let visible = IndexedStorageNamespace::OpLog {
        owned_agent_id: agent.clone(),
        agent_mode: mode,
    };
    for (_case, ids, expected) in [
        ("missing", vec![], 1),
        ("gap", vec![1, 3], 3),
        ("shifted", vec![2, 3], 2),
        ("wrong-tip", vec![1, 2, 3], 2),
        ("zero-tip", vec![1], 0),
    ] {
        let (key, token) = reserve_test_stage(storage.as_ref()).await;
        let key = key.as_str();
        let target = crate::fresh_owned_agent_id().storage_key();
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
                    Some(token),
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
                    &target,
                    expected,
                    Some(token),
                )
                .await
                .is_err()
        );
        assert!(
            !storage
                .exists("test", "visible", visible.clone(), &target)
                .await
                .unwrap()
        );
    }
    let (first_stage, first_token) = reserve_test_stage(storage.as_ref()).await;
    let (second_stage, second_token) = reserve_test_stage(storage.as_ref()).await;
    for (key, token, value) in [
        (first_stage.as_str(), first_token, 17u8),
        (second_stage.as_str(), second_token, 39),
    ] {
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
                Some(token),
            )
            .await
            .unwrap();
    }
    assert!(
        storage
            .scan_stable(
                "test",
                "scan",
                IndexedStorageMetaNamespace::Oplog { agent_mode: mode },
                None,
                None,
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
            &first_stage,
            visible.clone(),
            KEY1,
            3,
            Some(first_token),
        ),
        storage.move_if_absent(
            "test",
            "publish",
            staged.clone(),
            &second_stage,
            visible.clone(),
            KEY1,
            3,
            Some(second_token),
        ),
    );
    let first = first.unwrap();
    assert_ne!(first, second.unwrap());
    let (winner, loser, loser_token, value) = if first {
        (
            first_stage.as_str(),
            second_stage.as_str(),
            second_token,
            17,
        )
    } else {
        (second_stage.as_str(), first_stage.as_str(), first_token, 39)
    };
    let expected: Vec<_> = (1..=3).map(|id| (id, vec![value, id as u8])).collect();
    assert_eq!(
        storage
            .read("test", "read", "entry", visible.clone(), KEY1, 0, 3)
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
                KEY1,
                3,
                Some(loser_token),
            )
            .await
            .unwrap()
    );
    storage
        .discard_stage("test", "discard", staged.clone(), loser, Some(loser_token))
        .await
        .unwrap();
    assert_eq!(
        storage
            .read("test", "read", "entry", visible.clone(), KEY1, 1, 3)
            .await
            .unwrap(),
        expected
    );

    for (key, first_id) in [(KEY2, 1), (KEY3, 17)] {
        let (stage, token) = reserve_test_stage(storage.as_ref()).await;
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
                None,
            )
            .await
            .unwrap();
        storage
            .drop_prefix("test", "archive", visible.clone(), key, first_id, None)
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
                &stage,
                1,
                vec![23],
                None,
                Some(token),
            )
            .await
            .unwrap();
        assert!(
            !storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged.clone(),
                    &stage,
                    visible.clone(),
                    key,
                    1,
                    Some(token),
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
                    &stage,
                    visible.clone(),
                    key,
                    1,
                    Some(token),
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
    let absent = crate::fresh_owned_agent_id().storage_key();
    storage
        .drop_prefix("test", "archive", visible.clone(), &absent, 50, None)
        .await
        .unwrap();
    assert!(
        !storage
            .exists("test", "exists", visible.clone(), &absent)
            .await
            .unwrap()
    );

    let (race, token) = reserve_test_stage(storage.as_ref()).await;
    let ordinary_key = crate::fresh_owned_agent_id().storage_key();
    storage
        .append(
            "test",
            "stage",
            "entry",
            staged.clone(),
            &race,
            1,
            vec![51],
            None,
            Some(token),
        )
        .await
        .unwrap();
    let (published, ordinary) = tokio::join!(
        storage.move_if_absent(
            "test",
            "publish",
            staged,
            &race,
            visible.clone(),
            &ordinary_key,
            1,
            Some(token),
        ),
        storage.append(
            "test",
            "create",
            "entry",
            visible.clone(),
            &ordinary_key,
            1,
            vec![77],
            None,
            None,
        ),
    );
    let published = published.unwrap();
    assert_ne!(published, ordinary.is_ok());
    assert_eq!(
        storage
            .read("test", "read", "entry", visible, &ordinary_key, 1, 2)
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
    for (key, shard_epoch) in [(KEY1, None), (KEY2, Some(ShardEpoch(1)))] {
        for ns in [primary, compressed] {
            if let Some(epoch) = shard_epoch {
                storage
                    .set_key_epoch("svc", "api", ns.ns.clone(), key, epoch, &[], false)
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
                    None,
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
                    None,
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
                .scan_stable("svc", "api", ns.meta.clone(), Some(key), None, 10)
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
                        None,
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
                    None,
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
    for count in [1024, 1025] {
        for epoch in [None, Some(ShardEpoch(7))] {
            let key = crate::fresh_owned_agent_id().storage_key();
            if let Some(epoch) = epoch {
                storage
                    .set_key_epoch("svc", "api", ns.ns.clone(), &key, epoch, &[], false)
                    .await
                    .unwrap();
            }
            storage
                .append(
                    "svc",
                    "api",
                    "entity",
                    ns.ns.clone(),
                    &key,
                    count,
                    b"original".to_vec(),
                    epoch,
                    None,
                )
                .await
                .unwrap();
            let pairs: Arc<[(u64, Bytes)]> = (1..=count)
                .map(|id| (id, Bytes::from_static(b"new")))
                .collect::<Vec<_>>()
                .into();
            assert!(matches!(
                storage
                    .append_many("svc", "api", "entity", &ns.ns, &key, pairs, epoch, None)
                    .await,
                Err(IndexedStorageError::Conflict(_))
            ));
            assert_eq!(
                storage
                    .read("svc", "api", "entity", ns.ns.clone(), &key, 0, 2000)
                    .await
                    .unwrap(),
                vec![(count, b"original".to_vec())]
            );
        }
    }
}

#[test]
#[test_r::timeout("60s")]
async fn postgres_fenced_append_observes_locked_epoch_and_commit_errors(
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let wrapper = PostgresIndexedStorageWrapper {
        postgres: DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await,
    };
    let config = wrapper.fresh_database().await;
    let storage = Arc::new(PostgresIndexedStorage::configured(&config).await.unwrap());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect_with(config.postgres.connect_options())
        .await
        .unwrap();

    for (key, actual) in [(KEY1, Some(8i64)), (KEY2, None), (KEY3, Some(-1))] {
        storage
            .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
            .await
            .unwrap();
        assert_fenced(
            storage
                .append(
                    "svc",
                    "api",
                    "entity",
                    ns.ns.clone(),
                    key,
                    1,
                    vec![17],
                    Some(ShardEpoch(u64::MAX)),
                    None,
                )
                .await,
            u64::MAX,
            Some(7),
        );
        let owned = golem_common::model::OwnedAgentId::from_storage_key(key).unwrap();
        let mut tx = pool.begin().await.unwrap();
        if let Some(epoch) = actual {
            sqlx::query("UPDATE indexed_key_epoch SET epoch = $1 WHERE namespace = 'durable-worker-oplog' AND env = $2 AND agent = $3")
                .bind(epoch)
                .bind(owned.environment_id.0)
                .bind(owned.agent_id.as_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
        } else {
            sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = 'durable-worker-oplog' AND env = $1 AND agent = $2")
                .bind(owned.environment_id.0)
                .bind(owned.agent_id.as_uuid())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let writer = storage.clone();
        let namespace = ns.ns.clone();
        let append = tokio::spawn(async move {
            writer
                .append(
                    "svc",
                    "api",
                    "entity",
                    namespace,
                    key,
                    1,
                    vec![23],
                    Some(ShardEpoch(7)),
                    None,
                )
                .await
        });
        // Observe the actual row-lock wait before releasing the epoch change.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let (blocked,): (bool,) = sqlx::query_as(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND cardinality(pg_blocking_pids(pid)) > 0)",
                ).fetch_one(&pool).await.unwrap();
                if blocked { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("append did not wait for the epoch lock");
        tx.commit().await.unwrap();
        let result = append.await.unwrap();
        if actual == Some(-1) {
            assert!(
                matches!(result, Err(IndexedStorageError::Other(ref message)) if message.contains("negative epoch -1"))
            );
        } else {
            assert_fenced(result, 7, actual.map(|epoch| epoch as u64));
        }
        assert_eq!(
            storage
                .length("svc", "api", ns.ns.clone(), key)
                .await
                .unwrap(),
            0
        );
    }

    sqlx::raw_sql(
        "CREATE FUNCTION reject_append_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'deferred append conflict' USING ERRCODE = '23505'; END $$;
         CREATE CONSTRAINT TRIGGER reject_append AFTER INSERT ON index_storage DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_append_at_commit();",
    ).execute(&pool).await.unwrap();
    let commit_error_key = crate::fresh_owned_agent_id().storage_key();
    storage
        .set_key_epoch(
            "svc",
            "api",
            ns.ns.clone(),
            &commit_error_key,
            ShardEpoch(7),
            &[],
            false,
        )
        .await
        .unwrap();
    assert!(matches!(
        storage
            .append(
                "svc",
                "api",
                "entity",
                ns.ns.clone(),
                &commit_error_key,
                1,
                vec![31],
                Some(ShardEpoch(7)),
                None
            )
            .await,
        Err(IndexedStorageError::Conflict(_))
    ));
    assert_eq!(
        storage
            .length("svc", "api", ns.ns.clone(), &commit_error_key)
            .await
            .unwrap(),
        0
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

    let key1 = KEY1;
    let value1 = "value1".as_bytes().to_vec();

    let result1 = is.exists("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        1,
        value1,
        None,
        None,
    )
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

    let key1 = KEY1;
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

    let key1 = KEY1;
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

    let key1 = KEY1;
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        1,
        value1,
        None,
        None,
    )
    .await
    .unwrap();
    let result1 = is
        .append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            key1,
            1,
            value2,
            None,
            None,
        )
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

    let key1 = KEY1;
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

    let key1 = KEY1;
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is.length("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        4,
        value1,
        None,
        None,
    )
    .await
    .unwrap();
    let result2 = is.length("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        8,
        value2,
        None,
        None,
    )
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
    let mut resume = None;
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), None, resume, 10)
            .await
            .unwrap();
        result.extend(chunk);
        if next.is_none() {
            break;
        }
        resume = next;
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

    let key1 = KEY1;
    let key2 = KEY2;
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        1,
        value1,
        None,
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key2,
        1,
        value2,
        None,
        None,
    )
    .await
    .unwrap();

    let mut result: Vec<String> = Vec::new();
    let mut resume = None;
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), None, resume, 10)
            .await
            .unwrap();
        result.extend(chunk);
        if next.is_none() {
            break;
        }
        resume = next;
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

    let key1 = KEY1;
    let key2 = KEY2;
    let key3 = KEY2;
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
        None,
    )
    .await
    .unwrap();

    let mut r1: Vec<String> = Vec::new();
    let mut resume = None;
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), None, resume, 1)
            .await
            .unwrap();
        r1.extend(chunk);
        resume = next;

        if !r1.is_empty() || resume.is_none() {
            break;
        }
    }

    let mut r2: Vec<String> = Vec::new();
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), None, resume, 1)
            .await
            .unwrap();
        r2.extend(chunk);
        resume = next;

        if resume.is_none() {
            break;
        }
    }

    let mut r3: Vec<String> = Vec::new();
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), None, resume, 1)
            .await
            .unwrap();
        r3.extend(chunk);
        resume = next;

        if resume.is_none() {
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
        .map(|_| {
            let agent = crate::fresh_owned_agent_id();
            let key = agent.storage_key();
            let swept = IndexedStorageNamespace::CompressedOpLog {
                owned_agent_id: agent.clone(),
                agent_mode: AgentMode::Durable,
                level: SCAN_STABLE_LEVEL,
            };
            let below = IndexedStorageNamespace::CompressedOpLog {
                owned_agent_id: agent,
                agent_mode: AgentMode::Durable,
                level: SCAN_STABLE_LEVEL + 1,
            };
            (swept, below, key)
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

/// Draining multi-SQLite removes its database files, so later walks do no per-agent database work.
#[test]
#[tracing::instrument]
async fn multi_sqlite_scan_stable_does_not_reopen_drained_files() {
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
                owned_agent_id: crate::fresh_owned_agent_id(),
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
        pages, 1,
        "the drained files were removed, not traversed again"
    );
    assert!(std::fs::read_dir(tempdir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("durable-oplog-")
    }));
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
            owned_agent_id: crate::fresh_owned_agent_id(),
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
    let key = crate::fresh_owned_agent_id().storage_key();

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

    let key1 = KEY1;
    let key2 = KEY2;
    let key3 = KEY3;
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
        value1,
        None,
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key2,
        1,
        value2,
        None,
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key3,
        1,
        value3,
        None,
        None,
    )
    .await
    .unwrap();

    let mut result: Vec<String> = Vec::new();
    let mut resume = None;
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), Some(KEY_PREFIX), resume, 10)
            .await
            .unwrap();
        result.extend(chunk);
        if next.is_none() {
            break;
        }
        resume = next;
    }

    result.sort();
    assert!(result.contains(&key1.to_string()));
    assert!(result.contains(&key3.to_string()));
}

#[test]
#[tracing::instrument]
async fn sql_scan_prefix_is_literal_bounded_and_deletion_safe(
    #[tagged_as("sqlite")] storage: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let storage = storage.get_indexed_storage().await;
    let keys = [
        "A-prefix",
        "a",
        "a%",
        "a%tail",
        "a\\tail",
        "a_tail",
        "aa",
        "ae\u{301}",
        "aé",
        "a😀",
        "b",
        "á",
        "\u{10ffff}",
        "\u{10ffff}tail",
    ];
    for key in keys {
        storage
            .append(
                "svc",
                "api",
                "entity",
                ns.ns.clone(),
                key,
                1,
                vec![1],
                None,
                None,
            )
            .await
            .unwrap();
    }
    storage
        .append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            "aa",
            2,
            vec![2],
            None,
            None,
        )
        .await
        .unwrap();

    let expected: Vec<String> = keys
        .into_iter()
        .filter(|key| key.starts_with('a'))
        .map(str::to_string)
        .collect();
    let (_, literal_wildcard) = storage
        .scan_stable("svc", "api", ns.meta.clone(), Some("a%"), None, 10)
        .await
        .unwrap();
    assert_eq!(literal_wildcard, ["a%", "a%tail"]);

    let (resume, first) = storage
        .scan_stable("svc", "api", ns.meta.clone(), Some("a"), None, 2)
        .await
        .unwrap();
    assert_eq!(first, expected[..2]);

    let deleted_marker = first.last().unwrap().clone();
    storage
        .delete("svc", "api", ns.ns.clone(), &deleted_marker)
        .await
        .unwrap();

    let mut actual = first;
    let mut resume = resume;
    loop {
        let (next, page) = storage
            .scan_stable("svc", "api", ns.meta.clone(), Some("a"), resume, 2)
            .await
            .unwrap();
        actual.extend(page);
        resume = next;
        if resume.is_none() {
            break;
        }
    }
    assert_eq!(actual, expected);

    let (_, from_before_prefix) = storage
        .scan_stable(
            "svc",
            "api",
            ns.meta.clone(),
            Some("a"),
            Some(ScanResume::Marker("A".to_string())),
            20,
        )
        .await
        .unwrap();
    assert_eq!(
        from_before_prefix,
        expected
            .iter()
            .filter(|key| **key != deleted_marker)
            .cloned()
            .collect::<Vec<_>>()
    );

    let (_, at_upper_bound) = storage
        .scan_stable(
            "svc",
            "api",
            ns.meta.clone(),
            Some("a"),
            Some(ScanResume::Marker("b".to_string())),
            20,
        )
        .await
        .unwrap();
    assert!(at_upper_bound.is_empty());

    let (_, maximum_prefix) = storage
        .scan_stable("svc", "api", ns.meta.clone(), Some("\u{10ffff}"), None, 20)
        .await
        .unwrap();
    assert_eq!(maximum_prefix, ["\u{10ffff}", "\u{10ffff}tail"]);

    let (_, resumed_maximum_prefix) = storage
        .scan_stable(
            "svc",
            "api",
            ns.meta.clone(),
            Some("\u{10ffff}"),
            Some(ScanResume::Marker("\u{10ffff}".to_string())),
            20,
        )
        .await
        .unwrap();
    assert_eq!(resumed_maximum_prefix, ["\u{10ffff}tail"]);
}

#[test]
async fn sql_canonical_key_prefixes_and_resume_match_text_order(
    #[dimension(sql_is)] storage: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let storage = storage.get_indexed_storage().await;
    let keys = [
        "00000000-0000-0000-0000-000000000000/00000000-0000-8000-8000-000000000000",
        KEY1,
        KEY3,
        KEY2,
        "12345678-0000-0000-0000-000000000000/ffffffff-ffff-8fff-bfff-ffffffffffff",
        "ffffffff-ffff-ffff-ffff-ffffffffffff/ffffffff-ffff-8fff-bfff-ffffffffffff",
    ];
    for key in keys {
        append_fenced(&storage, &ns.ns, key, &[1, 3], None)
            .await
            .unwrap();
    }
    let mut prefixes = vec![
        "".to_string(),
        "%".into(),
        "_".into(),
        "\\".into(),
        "é".into(),
        "\u{10ffff}".into(),
    ];
    for key in keys {
        prefixes.extend((1..=key.len()).map(|end| key[..end].to_string()));
        prefixes.push(format!("{key}x"));
        prefixes.push(key.to_uppercase());
    }
    prefixes.sort();
    prefixes.dedup();
    for prefix in prefixes {
        for marker in [
            None,
            Some(""),
            Some(KEY1),
            Some(KEY2),
            Some("z"),
            // Bounds on either side of the environment/agent separator.
            Some("12345678-0000-0000-0000-000000000000."),
            Some("12345678-0000-0000-0000-0000000000000"),
            // Carry past an agent separator, a hex digit, and an entire agent UUID.
            Some("12345678-0000-0000-0000-000000000000/00000001."),
            Some("12345678-0000-0000-0000-000000000000/00000001-0000-8000-8000-00000000000g"),
            Some("12345678-0000-0000-0000-000000000000/00000001-ffff-ffff-ffff-ffffffffffffx"),
            // A suffix must exclude an exact key; carrying past the largest agent advances env.
            Some("12345678-0000-0000-0000-000000000000/00000001-0000-8000-8000-000000000001x"),
            Some("12345678-0000-0000-0000-000000000000/ffffffff-ffff-ffff-ffff-ffffffffffffx"),
            Some("ffffffff-ffff-ffff-ffff-ffffffffffff/ffffffff-ffff-ffff-ffff-ffffffffffff"),
            Some("ffffffff-ffff-ffff-ffff-ffffffffffff/ffffffff-ffff-ffff-ffff-ffffffffffffx"),
        ] {
            let mut expected: Vec<_> = keys
                .iter()
                .filter(|key| {
                    key.starts_with(&prefix) && marker.is_none_or(|marker| **key > marker)
                })
                .map(|key| key.to_string())
                .collect();
            expected.sort();
            let (resume, actual) = storage
                .scan_stable(
                    "svc",
                    "api",
                    ns.meta.clone(),
                    Some(&prefix),
                    marker.map(|marker| ScanResume::Marker(marker.into())),
                    20,
                )
                .await
                .unwrap();
            assert_eq!(actual, expected, "prefix={prefix:?}, marker={marker:?}");
            assert!(resume.is_none());
        }
    }
    let (resume, first) = storage
        .scan_stable("svc", "api", ns.meta.clone(), Some(KEY_PREFIX), None, 1)
        .await
        .unwrap();
    assert_eq!(first, [KEY1]);
    storage
        .delete("svc", "api", ns.ns.clone(), KEY1)
        .await
        .unwrap();
    let (_, second) = storage
        .scan_stable("svc", "api", ns.meta.clone(), Some(KEY_PREFIX), resume, 1)
        .await
        .unwrap();
    assert_eq!(second, [KEY3]);
    assert_eq!(
        storage
            .scan_stable("svc", "api", ns.meta.clone(), None, None, 0)
            .await
            .unwrap(),
        (None, vec![])
    );
    for resume in [ScanResume::Cursor(0), ScanResume::Marker("\0".into())] {
        assert!(matches!(
            storage
                .scan_stable("svc", "api", ns.meta.clone(), None, Some(resume), 1)
                .await,
            Err(IndexedStorageError::InvalidResume(_))
        ));
    }
}

#[test]
#[test_r::timeout("120s")]
async fn postgres_binary_key_migration_refuses_populated_storage() {
    let wrapper = PostgresIndexedStorageWrapper {
        postgres: DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await,
    };
    for seed in [None, Some("entry"), Some("presence"), Some("epoch")] {
        let config = wrapper.fresh_database().await;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with(config.postgres.connect_options())
            .await
            .unwrap();
        let mut old = sqlx::migrate::Migrator::new(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-worker-executor/db/migration/indexed/postgres"
        )))
        .await
        .unwrap();
        old.migrations.to_mut().truncate(3);
        old.run(&pool).await.unwrap();
        match seed {
            Some("epoch") => {
                sqlx::query("INSERT INTO indexed_key_epoch (namespace, key, epoch) VALUES ('durable-worker-oplog', $1, 7)")
                    .bind(KEY1).execute(&pool).await.unwrap();
            }
            Some(kind) => {
                sqlx::query("INSERT INTO index_storage (namespace, key, id, value) VALUES ($1, $2, 0, 'payload'::bytea)")
                    .bind(if kind == "presence" { "durable-worker-oplog-present" } else { "durable-worker-oplog" })
                    .bind(KEY1).execute(&pool).await.unwrap();
            }
            None => {}
        }
        let result = PostgresIndexedStorage::configured(&config).await;
        if seed.is_some() {
            assert!(result.unwrap_err().contains("requires empty tables"));
            let (version,): (i64,) = sqlx::query_as("SELECT MAX(version) FROM _sqlx_migrations")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(version, 3);
            if seed == Some("epoch") {
                let (key, epoch): (String, i64) =
                    sqlx::query_as("SELECT key, epoch FROM indexed_key_epoch")
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(key, KEY1);
                assert_eq!(epoch, 7);
            } else {
                let (key, value): (String, Vec<u8>) =
                    sqlx::query_as("SELECT key, value FROM index_storage")
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(key, KEY1);
                assert_eq!(value, b"payload");
            }
        } else {
            let storage = result.unwrap();
            let namespace = ns();
            append_fenced(
                &(Arc::new(storage) as Arc<dyn IndexedStorage + Send + Sync>),
                &namespace.ns,
                KEY1,
                &[1],
                None,
            )
            .await
            .unwrap();
            let (env, agent): (Uuid, Uuid) = sqlx::query_as("SELECT env, agent FROM index_storage")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(format!("{env}/{agent}"), KEY1);
        }
    }
}

#[test]
async fn postgres_rejects_noncanonical_keys_and_isolates_staged_epochs(
    #[tagged_as("postgres")] storage: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let storage = storage.get_indexed_storage().await;
    for key in [
        "key",
        &KEY1.replacen("12345678", "ABCDEFAB", 1),
        &KEY1.replace('-', ""),
        &format!("{KEY1}#{}", Uuid::new_v4()),
    ] {
        assert!(matches!(
            append_fenced(&storage, &ns.ns, key, &[1], None).await,
            Err(IndexedStorageError::Other(_))
        ));
    }
    let staged = IndexedStorageNamespace::StagedOpLog {
        owned_agent_id: crate::fresh_owned_agent_id(),
        agent_mode: AgentMode::Durable,
    };
    let owned = crate::fresh_owned_agent_id();
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let first = format!("{}#{first_id}", owned.storage_key());
    let second = format!("{}#{second_id}", owned.storage_key());
    let first_token = storage
        .reserve_stage("test", "reserve", &owned, first_id, None)
        .await
        .unwrap()
        .unwrap();
    storage
        .set_key_epoch(
            "svc",
            "api",
            staged.clone(),
            &first,
            ShardEpoch(7),
            &[],
            false,
        )
        .await
        .unwrap();
    storage
        .set_key_epoch(
            "svc",
            "api",
            staged.clone(),
            &second,
            ShardEpoch(11),
            &[],
            false,
        )
        .await
        .unwrap();
    append_record(
        storage.as_ref(),
        &staged,
        &first,
        1,
        b"1",
        Some(ShardEpoch(7)),
        Some(first_token),
    )
    .await
    .unwrap();
    storage
        .delete_with_epoch(
            "svc",
            "api",
            staged.clone(),
            &first,
            Some(ShardEpoch(7)),
            false,
        )
        .await
        .unwrap();
    let second_token = storage
        .reserve_stage("test", "reserve", &owned, second_id, None)
        .await
        .unwrap()
        .unwrap();
    assert_fenced(
        append_record(
            storage.as_ref(),
            &staged,
            &second,
            1,
            b"1",
            Some(ShardEpoch(7)),
            Some(second_token),
        )
        .await,
        7,
        Some(11),
    );
    append_record(
        storage.as_ref(),
        &staged,
        &second,
        1,
        b"1",
        Some(ShardEpoch(11)),
        Some(second_token),
    )
    .await
    .unwrap();
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

    let key1 = KEY1;
    let key2 = KEY2;
    let key3 = KEY3;
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
        value1,
        None,
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key2,
        1,
        value2,
        None,
        None,
    )
    .await
    .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key3,
        1,
        value3,
        None,
        None,
    )
    .await
    .unwrap();

    let mut r1: Vec<String> = Vec::new();
    let mut resume = None;
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), Some(KEY_PREFIX), resume, 1)
            .await
            .unwrap();
        r1.extend(chunk);
        resume = next;

        if r1.len() == 1 || resume.is_none() {
            break;
        }
    }

    let mut r2: Vec<String> = Vec::new();
    loop {
        let (next, chunk) = is
            .scan_stable("svc", "api", ns.meta.clone(), Some(KEY_PREFIX), resume, 1)
            .await
            .unwrap();
        r2.extend(chunk);
        resume = next;

        if resume.is_none() {
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

    let key1 = KEY1;
    let value1 = "value1".as_bytes().to_vec();

    let result1 = is.exists("svc", "api", ns.ns.clone(), key1).await.unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        1,
        value1,
        None,
        None,
    )
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

    let key1 = KEY1;
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

    let key1 = KEY1;

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

    let key1 = KEY1;
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

    let key1 = KEY1;
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

    let key1 = KEY1;
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

    let key1 = KEY1;
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

    let key1 = KEY1;
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

    let key1 = KEY1;
    let value1 = "value1".as_bytes().to_vec();
    let value2 = "value2".as_bytes().to_vec();

    let result1 = is
        .closest("svc", "api", "entity", ns.ns.clone(), key1, 10)
        .await
        .unwrap();
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key1,
        5,
        value1,
        None,
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
        value2,
        None,
        None,
    )
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

    let key1 = KEY1;
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
        None,
    )
    .await
    .unwrap();

    is.drop_prefix("svc", "api", ns.ns.clone(), key1, 5, None)
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

    let key1 = KEY1;
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
        None,
    )
    .await
    .unwrap();

    is.drop_prefix("svc", "api", ns.ns.clone(), key1, 10, None)
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

    let key1 = KEY1;
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
        None,
    )
    .await
    .unwrap();

    is.drop_prefix("svc", "api", ns.ns.clone(), key1, 20, None)
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
// Every test below runs against all five backends, and every backend fences, so each expects the
// same refusal from all of them.
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
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
        None,
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8), &[], false)
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
            None,
        )
        .await;

    let length = is.length("svc", "api", ns.ns.clone(), key).await.unwrap();
    assert_fenced(result, 7, Some(8));
    // The whole batch is rolled back, not the tail of it.
    assert_eq!(length, 0, "a refused batch must leave no entry behind");
}

#[test]
#[tracing::instrument]
async fn an_owner_re_opens_at_the_epoch_it_holds(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // One process re-opening an oplog it already holds, at the epoch it already holds, which
    // happens on every cache eviction.
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4), &[], false)
        .await
        .unwrap();
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4), &[], false)
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
        None,
    )
    .await
    .unwrap();
}

#[test]
#[tracing::instrument]
async fn a_higher_epoch_takes_the_oplog_over(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // A new owner at a higher epoch takes the oplog over, at which point the old owner is the one
    // being refused.
    let (owner, newcomer) = is.get_two_writers().await;
    let key = KEY1;

    owner
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(4), &[], false)
        .await
        .unwrap();
    newcomer
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5), &[], false)
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
            None,
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
            None,
        )
        .await;
    match refused {
        Err(IndexedStorageError::Fenced { actual, .. }) => {
            assert_eq!(actual, Some(ShardEpoch(5)));
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5), &[], false)
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
            None,
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
            None,
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
    let key = KEY1;

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
            None,
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(9), &[], false)
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
    let key = KEY1;

    // Rising and repeated epochs are accepted ...
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5), &[], false)
        .await
        .unwrap();
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5), &[], false)
        .await
        .unwrap();
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(9), &[], false)
        .await
        .unwrap();

    // ... a falling one is not, or a zombie could re-open at its stale epoch and un-fence itself
    // against the current owner.
    let result = is
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8), &[], false)
        .await;

    assert_fenced(result.map(|_| ()), 8, Some(9));
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
        None,
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
            None,
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(3), &[], false)
        .await
        .unwrap();
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, None, false)
        .await
        .unwrap();
    // Idempotent: deleting again is not an error.
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, None, false)
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
            None,
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(9), &[], false)
        .await
        .unwrap();
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, None, false)
        .await
        .unwrap();

    // The documented limit of the fence: the forward-only rule lives on the record, so once the
    // record is gone a lower epoch than the one it held is recorded and written through. Closing
    // this needs the delete to keep the epoch, which changes this test on purpose.
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8), &[], false)
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
        None,
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
    is.append_many(
        "svc",
        "api",
        "entity",
        ns,
        key,
        Arc::from(pairs),
        epoch,
        None,
    )
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5), &[], false)
        .await
        .unwrap();
    append_fenced(&is, &ns.ns, key, &[1, 2], Some(ShardEpoch(5)))
        .await
        .unwrap();

    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(5)), false)
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
    let key = KEY1;

    stale
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5), &[], false)
        .await
        .unwrap();
    append_fenced(&stale, &ns.ns, key, &[1], Some(ShardEpoch(5)))
        .await
        .unwrap();
    owner
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(6), &[], false)
        .await
        .unwrap();
    append_fenced(&owner, &ns.ns, key, &[2], Some(ShardEpoch(6)))
        .await
        .unwrap();

    assert_fenced(
        stale
            .delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(5)), false)
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
    let key = KEY1;

    append_fenced(&is, &ns.ns, key, &[1], None).await.unwrap();

    assert_fenced(
        is.delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(1)), false)
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
async fn a_delete_asserting_an_epoch_on_a_key_that_is_already_gone_is_accepted(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    // Neither a record nor entries: nothing is left for a writer that lost the key to destroy, so
    // a deletion retried after its own earlier attempt removed the key can finish.
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(3), &[], false)
        .await
        .unwrap();
    append_fenced(&is, &ns.ns, key, &[1, 2], Some(ShardEpoch(3)))
        .await
        .unwrap();
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(3)), false)
        .await
        .unwrap();
    is.delete_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(3)), false)
        .await
        .expect("a repeated delete of a key that is already gone is accepted");
    is.delete_with_epoch(
        "svc",
        "api",
        ns.ns.clone(),
        KEY2,
        Some(ShardEpoch(3)),
        false,
    )
    .await
    .expect("a delete of a key that was never written is accepted");

    assert!(!is.exists("svc", "api", ns.ns.clone(), key).await.unwrap());
    // Accepting the delete recorded nothing: the old epoch still writes nothing back.
    assert_fenced(
        append_fenced(&is, &ns.ns, key, &[3], Some(ShardEpoch(3))).await,
        3,
        None,
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
    let key = KEY1;

    first
        .set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
        .await
        .unwrap();
    append_fenced(&first, &ns.ns, key, &[1, 2], Some(ShardEpoch(7)))
        .await
        .unwrap();

    // Asserting nothing, so it does not matter that another writer holds the record.
    second
        .delete_with_epoch("svc", "api", ns.ns.clone(), key, None, false)
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(5), &[], false)
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
        owned_agent_id: crate::fresh_owned_agent_id(),
        agent_mode: AgentMode::Durable,
    };
    let (key, token) = reserve_test_stage(is.as_ref()).await;
    let key = key.as_str();

    is.append_many(
        "svc",
        "api",
        "entity",
        &staged,
        key,
        Arc::from([(1, Bytes::from_static(b"1")), (2, Bytes::from_static(b"2"))]),
        None,
        Some(token),
    )
    .await
    .unwrap();

    // Only the classification is asserted: an unfenced Redis batch is a pipeline, and its callers
    // reconcile a partial one by reading it back.
    match is
        .append_many(
            "svc",
            "api",
            "entity",
            &staged,
            key,
            Arc::from([(2, Bytes::from_static(b"2")), (3, Bytes::from_static(b"3"))]),
            None,
            Some(token),
        )
        .await
    {
        Err(IndexedStorageError::Conflict(_)) => {}
        other => panic!("expected a Conflict, got {other:?}"),
    }
}

#[test]
#[tracing::instrument]
async fn a_held_index_in_the_blob_manifest_is_a_conflict(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    // The blob layer deletes the object of a manifest append that gets this answer. So it must be
    // the storage's own answer that the index is held, with or without an asserted epoch, for one
    // entry as for a batch.
    let is = is.get_indexed_storage().await;
    let manifest = IndexedStorageNamespace::BlobOplogManifest {
        owned_agent_id: crate::fresh_owned_agent_id(),
        agent_mode: AgentMode::Durable,
        level: BLOB_MANIFEST_LEVEL,
    };
    let key = crate::fresh_owned_agent_id().storage_key();

    is.set_key_epoch(
        "svc",
        "api",
        manifest.clone(),
        &key,
        ShardEpoch(6),
        &[],
        false,
    )
    .await
    .unwrap();
    append_fenced(&is, &manifest, &key, &[1], Some(ShardEpoch(6)))
        .await
        .unwrap();

    for epoch in [Some(ShardEpoch(6)), None] {
        let single = is
            .append(
                "svc",
                "api",
                "entity",
                manifest.clone(),
                &key,
                1,
                b"again".to_vec(),
                epoch,
                None,
            )
            .await;
        assert!(
            matches!(single, Err(IndexedStorageError::Conflict(_))),
            "an append asserting {epoch:?} returned {single:?}"
        );
        let batch = append_fenced(&is, &manifest, &key, &[1], epoch).await;
        assert!(
            matches!(batch, Err(IndexedStorageError::Conflict(_))),
            "a batch asserting {epoch:?} returned {batch:?}"
        );
    }
    assert_eq!(
        is.length("svc", "api", manifest.clone(), &key)
            .await
            .unwrap(),
        1
    );
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
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(1), &[], false)
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
        None,
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
            None,
        )
        .await;

    assert!(result.is_err(), "a duplicate id must fail the batch");
    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        2,
        "the failed batch must not have written its first entry"
    );
}

async fn append_three_fenced(
    is: &Arc<dyn IndexedStorage + Send + Sync>,
    ns: &IndexedStorageNamespaces,
    key: &str,
    epoch: u64,
) {
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
        Some(ShardEpoch(epoch)),
        None,
    )
    .await
    .unwrap();
}

#[test]
#[tracing::instrument]
async fn a_trim_with_the_recorded_epoch_removes_the_prefix(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
        .await
        .unwrap();
    append_three_fenced(&is, ns, key, 7).await;

    is.drop_prefix("svc", "api", ns.ns.clone(), key, 1, Some(ShardEpoch(7)))
        .await
        .unwrap();

    let survivors = is
        .read("svc", "api", "entity", ns.ns.clone(), key, 1, 100)
        .await
        .unwrap();
    assert_eq!(
        survivors.into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
        vec![2, 3]
    );
}

#[test]
#[tracing::instrument]
async fn a_stale_epoch_trim_is_refused_and_removes_nothing(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
        .await
        .unwrap();
    append_three_fenced(&is, ns, key, 7).await;
    // Another writer takes the key over.
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8), &[], false)
        .await
        .unwrap();

    let result = is
        .drop_prefix("svc", "api", ns.ns.clone(), key, 3, Some(ShardEpoch(7)))
        .await;

    assert_fenced(result, 7, Some(8));
    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        3,
        "a refused trim must remove nothing"
    );
}

#[test]
#[tracing::instrument]
async fn a_trim_asserting_an_epoch_on_a_key_without_a_record_is_refused(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    for (id, value) in [(1, b"a"), (2, b"b"), (3, b"c")] {
        is.append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            key,
            id,
            value.to_vec(),
            None,
            None,
        )
        .await
        .unwrap();
    }

    let result = is
        .drop_prefix("svc", "api", ns.ns.clone(), key, 3, Some(ShardEpoch(7)))
        .await;

    assert_fenced(result, 7, None);
    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        3
    );
}

#[test]
#[tracing::instrument]
async fn an_unfenced_trim_ignores_the_recorded_epoch(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8), &[], false)
        .await
        .unwrap();
    append_three_fenced(&is, ns, key, 8).await;

    is.drop_prefix("svc", "api", ns.ns.clone(), key, 1, None)
        .await
        .unwrap();

    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        2
    );
}

#[test]
#[tracing::instrument]
async fn deleting_an_emptied_key_keeps_its_recorded_epoch(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
        .await
        .unwrap();
    append_three_fenced(&is, ns, key, 7).await;
    is.drop_prefix("svc", "api", ns.ns.clone(), key, 3, Some(ShardEpoch(7)))
        .await
        .unwrap();

    let deleted = is
        .delete_empty_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(7)))
        .await
        .unwrap();
    assert!(deleted, "an emptied key is deleted");
    assert!(!is.exists("svc", "api", ns.ns.clone(), key).await.unwrap());

    // The record survives: the owner keeps writing, a stale writer is still refused.
    is.append(
        "svc",
        "api",
        "entity",
        ns.ns.clone(),
        key,
        4,
        b"d".to_vec(),
        Some(ShardEpoch(7)),
        None,
    )
    .await
    .unwrap();
    let stale = is
        .append(
            "svc",
            "api",
            "entity",
            ns.ns.clone(),
            key,
            5,
            b"e".to_vec(),
            Some(ShardEpoch(6)),
            None,
        )
        .await;
    assert_fenced(stale, 6, Some(7));
}

#[test]
#[tracing::instrument]
async fn a_key_that_still_holds_entries_is_not_deleted(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
        .await
        .unwrap();
    append_three_fenced(&is, ns, key, 7).await;

    let deleted = is
        .delete_empty_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(7)))
        .await
        .unwrap();

    assert!(!deleted);
    assert_eq!(
        is.length("svc", "api", ns.ns.clone(), key).await.unwrap(),
        3
    );
}

#[test]
#[tracing::instrument]
async fn a_stale_epoch_delete_of_an_emptied_key_deletes_nothing(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
    #[tagged_as("ns1")] ns: &IndexedStorageNamespaces,
) {
    let is = is.get_indexed_storage().await;
    let key = KEY1;

    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(7), &[], false)
        .await
        .unwrap();
    append_three_fenced(&is, ns, key, 7).await;
    is.drop_prefix("svc", "api", ns.ns.clone(), key, 3, Some(ShardEpoch(7)))
        .await
        .unwrap();
    is.set_key_epoch("svc", "api", ns.ns.clone(), key, ShardEpoch(8), &[], false)
        .await
        .unwrap();

    let result = is
        .delete_empty_with_epoch("svc", "api", ns.ns.clone(), key, Some(ShardEpoch(7)))
        .await;

    assert_fenced(result.map(|_| ()), 7, Some(8));
    assert!(
        is.exists("svc", "api", ns.ns.clone(), key).await.unwrap(),
        "a refused delete leaves the emptied key in place"
    );
}

/// A blob manifest level no other test writes to, so a walk over it sees only this test's keys.
const BLOB_MANIFEST_LEVEL: usize = 95;

async fn scan_all(
    is: &Arc<dyn IndexedStorage + Send + Sync>,
    namespace: IndexedStorageMetaNamespace,
) -> Vec<String> {
    let mut keys = Vec::new();
    let mut resume = None;
    loop {
        let (next, chunk) = is
            .with("svc", "api")
            .scan_stable(namespace.clone(), None, resume, 100)
            .await
            .unwrap();
        keys.extend(chunk);
        match next {
            Some(next) => resume = Some(next),
            None => return keys,
        }
    }
}

/// The blob manifest is fenced like every oplog namespace, and its entries, epoch record and scan
/// are its own: the compressed level with the same agent and level number shares none of them.
#[test]
#[tracing::instrument]
async fn the_blob_manifest_is_fenced_apart_from_the_compressed_level(
    deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let is = is.get_indexed_storage().await;
    let agent_id = crate::fresh_owned_agent_id();
    let manifest = IndexedStorageNamespace::BlobOplogManifest {
        owned_agent_id: agent_id.clone(),
        agent_mode: AgentMode::Durable,
        level: BLOB_MANIFEST_LEVEL,
    };
    let compressed = IndexedStorageNamespace::CompressedOpLog {
        owned_agent_id: agent_id,
        agent_mode: AgentMode::Durable,
        level: BLOB_MANIFEST_LEVEL,
    };
    let key = crate::fresh_owned_agent_id().storage_key();

    // The compressed level's record does not stand in for the manifest's.
    is.set_key_epoch(
        "svc",
        "api",
        compressed.clone(),
        &key,
        ShardEpoch(9),
        &[],
        false,
    )
    .await
    .unwrap();
    assert_fenced(
        append_fenced(&is, &manifest, &key, &[1], Some(ShardEpoch(9))).await,
        9,
        None,
    );

    is.set_key_epoch(
        "svc",
        "api",
        manifest.clone(),
        &key,
        ShardEpoch(6),
        &[],
        false,
    )
    .await
    .unwrap();
    assert_fenced(
        append_fenced(&is, &manifest, &key, &[1], Some(ShardEpoch(5))).await,
        5,
        Some(6),
    );
    append_fenced(&is, &manifest, &key, &[1, 2], Some(ShardEpoch(6)))
        .await
        .unwrap();
    assert_eq!(
        is.length("svc", "api", manifest.clone(), &key)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        is.length("svc", "api", compressed.clone(), &key)
            .await
            .unwrap(),
        0
    );

    let manifest_keys = scan_all(
        &is,
        IndexedStorageMetaNamespace::BlobOplogManifest {
            agent_mode: AgentMode::Durable,
            level: BLOB_MANIFEST_LEVEL,
        },
    )
    .await;
    assert!(manifest_keys.contains(&key), "{manifest_keys:?}");
    let compressed_keys = scan_all(
        &is,
        IndexedStorageMetaNamespace::CompressedOplog {
            agent_mode: AgentMode::Durable,
            level: BLOB_MANIFEST_LEVEL,
        },
    )
    .await;
    assert!(!compressed_keys.contains(&key), "{compressed_keys:?}");

    // The recorded writer's delete takes the record with the entries.
    is.delete_with_epoch(
        "svc",
        "api",
        manifest.clone(),
        &key,
        Some(ShardEpoch(6)),
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        is.length("svc", "api", manifest.clone(), &key)
            .await
            .unwrap(),
        0
    );
    assert_fenced(
        append_fenced(&is, &manifest, &key, &[3], Some(ShardEpoch(6))).await,
        6,
        None,
    );
}

fn sealed_index_state(
    mode: AgentMode,
) -> golem_worker_executor::storage::indexed::AgentIndexedState {
    golem_worker_executor::storage::indexed::AgentIndexedState {
        owned_agent_id: crate::fresh_owned_agent_id(),
        agent_mode: mode,
        compressed_layers: 2,
        manifests: std::collections::BTreeMap::from([
            (2, Arc::from([(13, Bytes::from_static(b"chunk-thirteen"))])),
            (
                4,
                Arc::from([(29, Bytes::from_static(b"chunk-twenty-nine"))]),
            ),
        ]),
    }
}

async fn seed_sealed_index(
    storage: &Arc<dyn IndexedStorage + Send + Sync>,
    state: &golem_worker_executor::storage::indexed::AgentIndexedState,
    epoch: u64,
) {
    let key = state.owned_agent_id.storage_key();
    for namespace in state.epoch_namespaces() {
        storage
            .set_key_epoch(
                "test",
                "seed",
                namespace,
                &key,
                ShardEpoch(epoch),
                &[],
                false,
            )
            .await
            .unwrap();
    }
    for (level, entries) in &state.manifests {
        storage
            .append_many(
                "test",
                "seed",
                "manifest",
                &state.manifest_namespace(*level),
                &key,
                entries.clone(),
                Some(ShardEpoch(epoch)),
                None,
            )
            .await
            .unwrap();
    }
    storage
        .mark_sealed("test", "seed", state, ShardEpoch(epoch))
        .await
        .unwrap();
}

#[test]
#[test_r::timeout("120s")]
async fn indexed_agent_observation_tracks_read_generations(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    use golem_worker_executor::storage::indexed::{
        AgentDataObservation, ReleaseAgentResult, UnsealAgentResult,
    };
    use std::time::{Duration, SystemTime};

    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        let id = crate::fresh_owned_agent_id();
        assert!(
            storage
                .observe_agent("test", "unsupported", &id)
                .await
                .is_err()
        );
        return;
    }
    let absent = AgentDataObservation {
        has_data: false,
        durable_generation: None,
        ephemeral_generation: None,
    };
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let state = sealed_index_state(mode);
        let id = &state.owned_agent_id;
        let key = id.storage_key();
        let observation = storage.with("test", "generation");
        assert_eq!(observation.observe_agent(id).await.unwrap(), absent);
        storage
            .set_key_epoch(
                "test",
                "bare_epoch",
                state.fence_namespace(),
                &key,
                ShardEpoch(6),
                &[],
                false,
            )
            .await
            .unwrap();
        assert_eq!(observation.observe_agent(id).await.unwrap(), absent);
        seed_sealed_index(&storage, &state, 6).await;
        let marked = observation.observe_agent(id).await.unwrap();
        assert!(marked.has_data);
        let generation = match mode {
            AgentMode::Durable => {
                assert_eq!(marked.ephemeral_generation, None);
                marked.durable_generation.unwrap()
            }
            AgentMode::Ephemeral => {
                assert_eq!(marked.durable_generation, None);
                marked.ephemeral_generation.unwrap()
            }
        };
        storage
            .mark_sealed("test", "repeat_mark", &state, ShardEpoch(6))
            .await
            .unwrap();
        assert_eq!(observation.observe_agent(id).await.unwrap(), marked);
        assert_eq!(
            storage
                .release_agent("test", "release", &state, ShardEpoch(6))
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
        assert_eq!(observation.observe_agent(id).await.unwrap(), absent);
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "restore",
                    &state,
                    ShardEpoch(7),
                    ShardEpoch(6),
                    SystemTime::now() + Duration::from_secs(30)
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        let restored = observation.observe_agent(id).await.unwrap();
        let restored_generation = match mode {
            AgentMode::Durable => restored.durable_generation.unwrap(),
            AgentMode::Ephemeral => restored.ephemeral_generation.unwrap(),
        };
        assert_ne!(restored_generation, generation);
        let mut wrong = state.clone();
        wrong
            .manifests
            .insert(2, Arc::from([(13, Bytes::from_static(b"wrong-manifest"))]));
        for (retry_state, epoch) in [(&wrong, ShardEpoch(7)), (&state, ShardEpoch(8))] {
            assert_eq!(
                storage
                    .unseal_agent(
                        "test",
                        "non_exact_retry",
                        retry_state,
                        epoch,
                        ShardEpoch(6),
                        SystemTime::UNIX_EPOCH
                    )
                    .await
                    .unwrap(),
                UnsealAgentResult::AlreadyPresent
            );
            assert_eq!(observation.observe_agent(id).await.unwrap(), restored);
        }
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "expired_retry",
                    &state,
                    ShardEpoch(7),
                    ShardEpoch(6),
                    SystemTime::UNIX_EPOCH
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        assert_eq!(observation.observe_agent(id).await.unwrap(), restored);
        storage
            .mark_sealed("test", "restored_mark", &state, ShardEpoch(7))
            .await
            .unwrap();
        assert_eq!(observation.observe_agent(id).await.unwrap(), restored);

        let mut opposite = state.clone();
        opposite.agent_mode = match mode {
            AgentMode::Durable => AgentMode::Ephemeral,
            AgentMode::Ephemeral => AgentMode::Durable,
        };
        storage
            .set_key_epoch(
                "test",
                "other_mode",
                opposite.fence_namespace(),
                &key,
                ShardEpoch(7),
                &[],
                false,
            )
            .await
            .unwrap();
        storage
            .mark_sealed("test", "other_mode", &opposite, ShardEpoch(7))
            .await
            .unwrap();
        let both = observation.observe_agent(id).await.unwrap();
        assert!(both.has_data);
        assert!(both.durable_generation.is_some() && both.ephemeral_generation.is_some());
        assert_ne!(both.durable_generation, both.ephemeral_generation);
        match mode {
            AgentMode::Durable => assert_eq!(both.durable_generation, restored.durable_generation),
            AgentMode::Ephemeral => {
                assert_eq!(both.ephemeral_generation, restored.ephemeral_generation)
            }
        }
        assert_eq!(
            storage
                .release_agent("test", "release_both", &state, ShardEpoch(7))
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
        assert_eq!(observation.observe_agent(id).await.unwrap(), absent);

        // A malformed marker is a data error, not a missing generation or an old format.
        storage
            .set_key_epoch(
                "test",
                "corrupt_fence",
                state.fence_namespace(),
                &key,
                ShardEpoch(8),
                &[],
                false,
            )
            .await
            .unwrap();
        storage
            .append(
                "test",
                "corrupt_marker",
                "marker",
                state.marker_namespace(),
                &key,
                0,
                vec![1, 2, 3],
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            observation.observe_agent(id).await,
            Err(IndexedStorageError::Other(_))
        ));
        assert!(matches!(
            storage
                .mark_sealed("test", "corrupt_repeat", &state, ShardEpoch(8))
                .await,
            Err(IndexedStorageError::Other(_))
        ));
        assert!(matches!(
            storage
                .unseal_agent(
                    "test",
                    "corrupt_retry",
                    &state,
                    ShardEpoch(8),
                    ShardEpoch(6),
                    SystemTime::UNIX_EPOCH
                )
                .await,
            Err(IndexedStorageError::Other(_))
        ));
    }
}

#[test]
#[test_r::timeout("120s")]
async fn indexed_agent_transitions_round_trip_and_lost_reply_retries(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    use golem_worker_executor::storage::indexed::{ReleaseAgentResult, UnsealAgentResult};
    use std::time::{Duration, SystemTime};
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let state = sealed_index_state(mode);
        let key = state.owned_agent_id.storage_key();
        let stage_id = Uuid::new_v4();
        let stage_token = storage
            .reserve_stage(
                "test",
                "reserve",
                &state.owned_agent_id,
                stage_id,
                Some(ShardEpoch(6)),
            )
            .await
            .unwrap()
            .unwrap();
        seed_sealed_index(&storage, &state, 6).await;
        let stage = IndexedStorageNamespace::StagedOpLog {
            owned_agent_id: state.owned_agent_id.clone(),
            agent_mode: AgentMode::Durable,
        };
        let stage_key = format!("{key}#{stage_id}");
        storage
            .append(
                "test",
                "stage",
                "entry",
                stage.clone(),
                &stage_key,
                1,
                vec![71],
                None,
                Some(stage_token),
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .release_agent("test", "release", &state, ShardEpoch(6))
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
        assert_eq!(
            storage
                .release_agent("test", "release_retry", &state, ShardEpoch(6))
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
        for namespace in state
            .epoch_namespaces()
            .into_iter()
            .chain([state.marker_namespace()])
        {
            assert!(
                !storage
                    .exists("test", "absent", namespace.clone(), &key)
                    .await
                    .unwrap()
            );
            assert_fenced(
                storage
                    .append(
                        "test",
                        "old_owner",
                        "entry",
                        namespace,
                        &key,
                        45,
                        vec![99],
                        Some(ShardEpoch(6)),
                        None,
                    )
                    .await,
                6,
                None,
            );
        }
        assert!(
            !storage
                .exists("test", "stage_absent", stage.clone(), &stage_key)
                .await
                .unwrap()
        );
        assert!(matches!(
            storage
                .append(
                    "test",
                    "old_stage",
                    "entry",
                    stage,
                    &stage_key,
                    2,
                    vec![99],
                    None,
                    Some(stage_token)
                )
                .await,
            Err(IndexedStorageError::StageInvalidated { .. })
        ));
        assert_fenced(
            storage
                .unseal_agent(
                    "test",
                    "stale",
                    &state,
                    ShardEpoch(5),
                    ShardEpoch(6),
                    SystemTime::now() + Duration::from_secs(30),
                )
                .await
                .map(|_| ()),
            5,
            Some(6),
        );
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "expired",
                    &state,
                    ShardEpoch(7),
                    ShardEpoch(6),
                    SystemTime::UNIX_EPOCH
                )
                .await
                .unwrap(),
            UnsealAgentResult::Expired
        );
        assert!(
            !storage
                .exists("test", "still_absent", state.primary_namespace(), &key)
                .await
                .unwrap()
        );
        // A crashed create can leave a bare epoch. It must not count as data presence.
        assert!(
            !storage
                .set_key_epoch(
                    "test",
                    "bare_epoch",
                    state.fence_namespace(),
                    &key,
                    ShardEpoch(7),
                    &state.epoch_namespaces(),
                    false
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "unseal",
                    &state,
                    ShardEpoch(7),
                    ShardEpoch(6),
                    SystemTime::now() + Duration::from_secs(30)
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "unseal_retry",
                    &state,
                    ShardEpoch(7),
                    ShardEpoch(6),
                    SystemTime::UNIX_EPOCH
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        assert!(
            storage
                .set_key_epoch(
                    "test",
                    "presence",
                    state.fence_namespace(),
                    &key,
                    ShardEpoch(7),
                    &state.epoch_namespaces(),
                    false
                )
                .await
                .unwrap()
        );
        for (level, entries) in &state.manifests {
            assert_eq!(
                storage
                    .read(
                        "test",
                        "restored",
                        "manifest",
                        state.manifest_namespace(*level),
                        &key,
                        0,
                        u64::MAX / 2
                    )
                    .await
                    .unwrap(),
                entries
                    .iter()
                    .map(|(id, bytes)| (*id, bytes.to_vec()))
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            !storage
                .remove_key_epoch_if_empty(
                    "test",
                    "gc",
                    state.fence_namespace(),
                    &key,
                    ShardEpoch(7)
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .release_agent("test", "release_again", &state, ShardEpoch(7))
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
        assert!(
            !storage
                .set_key_epoch(
                    "test",
                    "ephemeral_observe",
                    state.fence_namespace(),
                    &key,
                    ShardEpoch(8),
                    &state.epoch_namespaces(),
                    true
                )
                .await
                .unwrap()
        );
        assert_fenced(
            storage
                .append(
                    "test",
                    "no_observer_epoch",
                    "entry",
                    state.fence_namespace(),
                    &key,
                    1,
                    vec![1],
                    Some(ShardEpoch(8)),
                    None,
                )
                .await,
            8,
            None,
        );
    }
}

#[test]
#[test_r::timeout("120s")]
async fn indexed_agent_transitions_refuse_without_removing_history(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    use golem_worker_executor::storage::indexed::{ReleaseAgentResult, UnsealAgentResult};
    use std::time::{Duration, SystemTime};
    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let state = sealed_index_state(mode);
        let key = state.owned_agent_id.storage_key();
        seed_sealed_index(&storage, &state, 6).await;
        assert_fenced(
            storage
                .release_agent("test", "stale", &state, ShardEpoch(5))
                .await
                .map(|_| ()),
            5,
            Some(6),
        );
        let mut wrong = state.clone();
        wrong
            .manifests
            .insert(2, Arc::from([(13, Bytes::from_static(b"wrong-object"))]));
        assert_eq!(
            storage
                .release_agent("test", "wrong_bytes", &wrong, ShardEpoch(6))
                .await
                .unwrap(),
            ReleaseAgentResult::NotReleased
        );
        wrong = state.clone();
        wrong.manifests.remove(&4);
        assert_eq!(
            storage
                .release_agent("test", "extra_rows", &wrong, ShardEpoch(6))
                .await
                .unwrap(),
            ReleaseAgentResult::NotReleased
        );
        let opposite = IndexedStorageNamespace::CompressedOpLog {
            owned_agent_id: state.owned_agent_id.clone(),
            agent_mode: if mode == AgentMode::Durable {
                AgentMode::Ephemeral
            } else {
                AgentMode::Durable
            },
            level: 91,
        };
        storage
            .set_key_epoch(
                "test",
                "opposite",
                opposite.clone(),
                &key,
                ShardEpoch(9),
                &[],
                false,
            )
            .await
            .unwrap();
        assert_fenced(
            storage
                .release_agent("test", "higher_other_mode", &state, ShardEpoch(6))
                .await
                .map(|_| ()),
            6,
            Some(9),
        );
        assert_fenced(
            storage
                .unseal_agent(
                    "test",
                    "higher_other_mode",
                    &state,
                    ShardEpoch(8),
                    ShardEpoch(6),
                    SystemTime::now() + Duration::from_secs(30),
                )
                .await
                .map(|_| ()),
            8,
            Some(9),
        );
        storage
            .delete_with_epoch(
                "test",
                "remove_other_epoch",
                opposite.clone(),
                &key,
                Some(ShardEpoch(9)),
                false,
            )
            .await
            .unwrap();
        storage
            .append(
                "test",
                "nonempty",
                "entry",
                opposite.clone(),
                &key,
                3,
                vec![19],
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .release_agent("test", "nonempty", &state, ShardEpoch(6))
                .await
                .unwrap(),
            ReleaseAgentResult::NotReleased
        );
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "present",
                    &state,
                    ShardEpoch(6),
                    ShardEpoch(6),
                    SystemTime::now() + Duration::from_secs(30)
                )
                .await
                .unwrap(),
            UnsealAgentResult::AlreadyPresent
        );
        for (level, entries) in &state.manifests {
            assert_eq!(
                storage
                    .read(
                        "test",
                        "unchanged",
                        "manifest",
                        state.manifest_namespace(*level),
                        &key,
                        0,
                        i64::MAX as u64
                    )
                    .await
                    .unwrap(),
                entries
                    .iter()
                    .map(|(id, bytes)| (*id, bytes.to_vec()))
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            storage
                .read("test", "unchanged", "entry", opposite, &key, 0, 3)
                .await
                .unwrap(),
            vec![(3, vec![19])]
        );
    }
}

#[test]
#[test_r::timeout("120s")]
async fn indexed_unseal_restores_over_retained_bare_epochs(
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    use golem_worker_executor::storage::indexed::UnsealAgentResult;
    use std::time::{Duration, SystemTime};

    let storage = is.get_indexed_storage().await;
    if !storage.supports_release() {
        return;
    }
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for retained in [ShardEpoch(6), ShardEpoch(7), ShardEpoch(8)] {
            for retain_all in [false, true] {
                let state = sealed_index_state(mode);
                let key = state.owned_agent_id.storage_key();
                let extra = IndexedStorageNamespace::CompressedOpLog {
                    owned_agent_id: state.owned_agent_id.clone(),
                    agent_mode: if mode == AgentMode::Durable {
                        AgentMode::Ephemeral
                    } else {
                        AgentMode::Durable
                    },
                    level: 99,
                };
                let configured = state.epoch_namespaces();
                let retained_namespaces: Vec<_> = configured
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| retain_all || index % 2 == 0)
                    .map(|(_, namespace)| namespace.clone())
                    .chain([extra.clone()])
                    .collect();
                for namespace in &retained_namespaces {
                    storage
                        .delete_with_epoch(
                            "test",
                            "retain_bare",
                            namespace.clone(),
                            &key,
                            Some(retained),
                            true,
                        )
                        .await
                        .unwrap();
                }
                assert!(
                    !storage
                        .observe_agent("test", "bare_is_absent", &state.owned_agent_id)
                        .await
                        .unwrap()
                        .has_data
                );
                let result = storage
                    .unseal_agent(
                        "test",
                        "restore_retained",
                        &state,
                        ShardEpoch(7),
                        ShardEpoch(6),
                        SystemTime::now() + Duration::from_secs(30),
                    )
                    .await;
                if retained > ShardEpoch(7) {
                    assert_fenced(result.map(|_| ()), 7, Some(8));
                    assert!(
                        !storage
                            .observe_agent("test", "refused_without_data", &state.owned_agent_id)
                            .await
                            .unwrap()
                            .has_data
                    );
                    for namespace in retained_namespaces {
                        assert_fenced(
                            storage
                                .set_key_epoch(
                                    "test",
                                    "retained_unchanged",
                                    namespace,
                                    &key,
                                    ShardEpoch(7),
                                    &[],
                                    false,
                                )
                                .await
                                .map(|_| ()),
                            7,
                            Some(8),
                        );
                    }
                    continue;
                }
                assert_eq!(result.unwrap(), UnsealAgentResult::Unsealed);
                for (level, entries) in &state.manifests {
                    assert_eq!(
                        storage
                            .read(
                                "test",
                                "restored_history",
                                "manifest",
                                state.manifest_namespace(*level),
                                &key,
                                0,
                                99
                            )
                            .await
                            .unwrap(),
                        entries
                            .iter()
                            .map(|(id, bytes)| (*id, bytes.to_vec()))
                            .collect::<Vec<_>>()
                    );
                }
                let restored = storage
                    .observe_agent("test", "restored", &state.owned_agent_id)
                    .await
                    .unwrap();
                assert!(restored.has_data);
                assert!(
                    restored.durable_generation.is_some()
                        || restored.ephemeral_generation.is_some()
                );
                assert_eq!(
                    storage
                        .unseal_agent(
                            "test",
                            "committed_retry",
                            &state,
                            ShardEpoch(7),
                            ShardEpoch(6),
                            SystemTime::UNIX_EPOCH
                        )
                        .await
                        .unwrap(),
                    UnsealAgentResult::Unsealed
                );
                assert_eq!(
                    storage
                        .observe_agent("test", "same_generation", &state.owned_agent_id)
                        .await
                        .unwrap(),
                    restored
                );
                // Both previously retained rows and newly inserted configured rows now fence at 7.
                for namespace in configured.into_iter().chain([extra]) {
                    assert_fenced(
                        storage
                            .set_key_epoch(
                                "test",
                                "raised_or_inserted",
                                namespace,
                                &key,
                                ShardEpoch(6),
                                &[],
                                false,
                            )
                            .await
                            .map(|_| ()),
                        6,
                        Some(7),
                    );
                }
            }
        }
    }
}

#[test]
#[test_r::timeout("120s")]
async fn postgres_unseal_updates_locked_epochs_and_aborts_missing_row_conflicts() {
    use golem_worker_executor::storage::indexed::UnsealAgentResult;
    use std::time::{Duration, SystemTime};

    let wrapper = PostgresIndexedStorageWrapper {
        postgres: DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await,
    };
    let config = wrapper.fresh_database().await;
    let storage = PostgresIndexedStorage::configured(&config).await.unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_with(config.postgres.connect_options())
        .await
        .unwrap();
    let state = sealed_index_state(AgentMode::Durable);
    let key = state.owned_agent_id.storage_key();
    storage
        .delete_with_epoch(
            "test",
            "retain",
            state.primary_namespace(),
            &key,
            Some(ShardEpoch(6)),
            true,
        )
        .await
        .unwrap();
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION reject_retained_epoch_replacement() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
         IF OLD.agent = '{}'::uuid AND OLD.namespace = 'durable-worker-oplog' THEN
         RAISE EXCEPTION 'retained epoch must be updated in place'; END IF; RETURN OLD; END $$;
         CREATE TRIGGER reject_retained_epoch_replacement BEFORE DELETE ON indexed_key_epoch FOR EACH ROW EXECUTE FUNCTION reject_retained_epoch_replacement();",
        state.owned_agent_id.agent_id.as_uuid()
    )).execute(&pool).await.unwrap();
    assert_eq!(
        storage
            .unseal_agent(
                "test",
                "update_retained",
                &state,
                ShardEpoch(7),
                ShardEpoch(6),
                SystemTime::now() + Duration::from_secs(30)
            )
            .await
            .unwrap(),
        UnsealAgentResult::Unsealed
    );

    let state = sealed_index_state(AgentMode::Durable);
    let key = state.owned_agent_id.storage_key();
    let env = state.owned_agent_id.environment_id.0;
    let agent = state.owned_agent_id.agent_id.as_uuid();
    storage
        .delete_with_epoch(
            "test",
            "retain_for_conflict",
            state.primary_namespace(),
            &key,
            Some(ShardEpoch(6)),
            true,
        )
        .await
        .unwrap();
    // Inject a configured namespace absent from the locked snapshot while raising a retained row.
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION insert_missing_epoch_during_raise() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
         IF NEW.agent = '{}'::uuid AND NEW.namespace = 'durable-worker-oplog' THEN
         INSERT INTO indexed_key_epoch(namespace, env, agent, epoch) VALUES ('durable-worker-c1-oplog', NEW.env, NEW.agent, 9);
         END IF; RETURN NEW; END $$;
         CREATE TRIGGER insert_missing_epoch_during_raise BEFORE UPDATE ON indexed_key_epoch FOR EACH ROW EXECUTE FUNCTION insert_missing_epoch_during_raise();",
        agent
    )).execute(&pool).await.unwrap();
    assert!(
        storage
            .unseal_agent(
                "test",
                "missing_conflict",
                &state,
                ShardEpoch(7),
                ShardEpoch(6),
                SystemTime::now() + Duration::from_secs(30)
            )
            .await
            .is_err()
    );
    let epochs: Vec<(String, i64)> = sqlx::query_as("SELECT namespace, epoch FROM indexed_key_epoch WHERE env = $1 AND agent = $2 ORDER BY namespace;")
        .bind(env).bind(agent).fetch_all(&pool).await.unwrap();
    assert_eq!(epochs, vec![("durable-worker-oplog".to_string(), 6)]);
    assert!(
        !storage
            .observe_agent("test", "rolled_back", &state.owned_agent_id)
            .await
            .unwrap()
            .has_data
    );
}

#[test]
#[test_r::timeout("120s")]
async fn indexed_agent_transitions_retained_deletion_and_conditional_collection(
    _deps: &WorkerExecutorTestDependencies,
    #[dimension(is)] is: &Arc<dyn GetIndexedStorage + Send + Sync>,
) {
    let storage = is.get_indexed_storage().await;
    // Retained deletion is valid on unsupported seal backends too.
    let state = sealed_index_state(AgentMode::Durable);
    let key = state.owned_agent_id.storage_key();
    storage
        .set_key_epoch(
            "test",
            "owner",
            state.primary_namespace(),
            &key,
            ShardEpoch(12),
            &[],
            false,
        )
        .await
        .unwrap();
    storage
        .append(
            "test",
            "write",
            "entry",
            state.primary_namespace(),
            &key,
            1,
            vec![81],
            Some(ShardEpoch(12)),
            None,
        )
        .await
        .unwrap();
    storage
        .delete_with_epoch(
            "test",
            "delete",
            state.primary_namespace(),
            &key,
            Some(ShardEpoch(12)),
            true,
        )
        .await
        .unwrap();
    let other_epoch = IndexedStorageNamespace::CompressedOpLog {
        owned_agent_id: state.owned_agent_id.clone(),
        agent_mode: AgentMode::Ephemeral,
        level: 41,
    };
    storage
        .set_key_epoch(
            "test",
            "other_epoch",
            other_epoch,
            &key,
            ShardEpoch(13),
            &[],
            false,
        )
        .await
        .unwrap();
    storage
        .delete_with_epoch(
            "test",
            "delete_retry",
            state.primary_namespace(),
            &key,
            Some(ShardEpoch(12)),
            true,
        )
        .await
        .unwrap();
    assert_fenced(
        storage
            .append(
                "test",
                "stale",
                "entry",
                state.primary_namespace(),
                &key,
                1,
                vec![82],
                Some(ShardEpoch(11)),
                None,
            )
            .await,
        11,
        Some(12),
    );
    if storage.supports_release() {
        assert!(
            !storage
                .remove_key_epoch_if_empty(
                    "test",
                    "wrong_collector",
                    state.primary_namespace(),
                    &key,
                    ShardEpoch(11)
                )
                .await
                .unwrap()
        );
        assert!(
            storage
                .remove_key_epoch_if_empty(
                    "test",
                    "collector",
                    state.primary_namespace(),
                    &key,
                    ShardEpoch(12)
                )
                .await
                .unwrap()
        );
        assert!(
            !storage
                .remove_key_epoch_if_empty(
                    "test",
                    "collector_retry",
                    state.primary_namespace(),
                    &key,
                    ShardEpoch(12)
                )
                .await
                .unwrap()
        );
        assert_fenced(
            storage
                .append(
                    "test",
                    "collected",
                    "entry",
                    state.primary_namespace(),
                    &key,
                    1,
                    vec![83],
                    Some(ShardEpoch(12)),
                    None,
                )
                .await,
            12,
            None,
        );
    }
}

#[test]
#[test_r::timeout("120s")]
async fn postgres_agent_lookup_plans_use_agent_index_for_present_and_absent_rows() {
    let wrapper = PostgresIndexedStorageWrapper {
        postgres: DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await,
    };
    let config = wrapper.fresh_database().await;
    let storage = PostgresIndexedStorage::configured(&config).await.unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(config.postgres.connect_options())
        .await
        .unwrap();
    let state = sealed_index_state(AgentMode::Durable);
    let env = state.owned_agent_id.environment_id.0;
    let agent = state.owned_agent_id.agent_id.as_uuid();
    sqlx::query(
        "INSERT INTO index_storage (namespace, env, agent, id, value) SELECT 'unconfigured-' || ns, CASE WHEN a % 2 = 0 THEN $1 ELSE md5('environment-' || (a % 4))::uuid END, md5('agent-' || a)::uuid, 1, decode('01', 'hex') FROM generate_series(1, 2048) a CROSS JOIN generate_series(1, 16) ns;",
    ).bind(env).execute(&pool).await.unwrap();
    // The environment is part of identity, even when another environment has the same agent.
    sqlx::query("INSERT INTO index_storage (namespace, env, agent, id, value) VALUES ('other-environment', $1, $2, 1, decode('01', 'hex'));")
        .bind(Uuid::new_v4()).bind(agent).execute(&pool).await.unwrap();
    let absent_agent = Uuid::new_v4();
    for namespace in ["unconfigured-level", "durable-worker-staged-oplog"] {
        sqlx::query("INSERT INTO index_storage (namespace, env, agent, id, value) VALUES ($1, $2, $3, 1, decode('01', 'hex'));")
            .bind(namespace).bind(env).bind(agent).execute(&pool).await.unwrap();
        sqlx::query("ANALYZE index_storage;")
            .execute(&pool)
            .await
            .unwrap();
        for (lookup, expected) in [(agent, true), (absent_agent, false)] {
            let (present, durable, ephemeral): (bool, Option<Vec<u8>>, Option<Vec<u8>>) =
                sqlx::query_as(PostgresIndexedStorage::OBSERVE_AGENT_QUERY)
                    .bind(env)
                    .bind(lookup)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(present, expected);
            assert_eq!(durable, None);
            assert_eq!(ephemeral, None);
            for query in [
                PostgresIndexedStorage::OBSERVE_AGENT_QUERY,
                PostgresIndexedStorage::AGENT_ROWS_QUERY,
                PostgresIndexedStorage::DELETE_AGENT_ROWS_QUERY,
            ] {
                let mut tx = pool.begin().await.unwrap();
                let explain = format!("EXPLAIN (ANALYZE, BUFFERS) {query}");
                let plan: Vec<(String,)> = sqlx::query_as(&explain)
                    .bind(env)
                    .bind(lookup)
                    .fetch_all(&mut *tx)
                    .await
                    .unwrap();
                let details = plan
                    .iter()
                    .map(|row| row.0.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    details.contains("idx_index_storage_agent"),
                    "{query}: {details}"
                );
                assert!(
                    !details.contains("Seq Scan on index_storage"),
                    "{query}: {details}"
                );
                assert!(
                    details.contains("Index Cond:")
                        && details.contains("env =")
                        && details.contains("agent ="),
                    "{query}: {details}"
                );
                tx.rollback().await.unwrap();
            }
        }
        assert!(
            storage
                .observe_agent("test", "public_presence", &state.owned_agent_id)
                .await
                .unwrap()
                .has_data
        );
        let rows: Vec<(String, i64, Vec<u8>)> =
            sqlx::query_as(PostgresIndexedStorage::AGENT_ROWS_QUERY)
                .bind(env)
                .bind(agent)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, vec![(namespace.to_string(), 1, vec![1])]);
        let deleted = sqlx::query(PostgresIndexedStorage::DELETE_AGENT_ROWS_QUERY)
            .bind(env)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(deleted.rows_affected(), 1);
        assert!(
            !storage
                .observe_agent("test", "absent_after_release", &state.owned_agent_id)
                .await
                .unwrap()
                .has_data
        );
    }

    // A committed retry must not issue writes to either table, even with an expired deadline.
    storage
        .unseal_agent(
            "test",
            "restore_for_retry",
            &state,
            ShardEpoch(7),
            ShardEpoch(6),
            std::time::SystemTime::now() + std::time::Duration::from_secs(30),
        )
        .await
        .unwrap();
    let before = storage
        .observe_agent("test", "before_retry", &state.owned_agent_id)
        .await
        .unwrap();
    assert!(before.durable_generation.is_some());
    sqlx::raw_sql(
        "CREATE FUNCTION reject_committed_retry_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'committed retry must not write'; END $$;
         CREATE TRIGGER reject_index_retry_write BEFORE INSERT OR UPDATE OR DELETE ON index_storage FOR EACH STATEMENT EXECUTE FUNCTION reject_committed_retry_write();
         CREATE TRIGGER reject_epoch_retry_write BEFORE INSERT OR UPDATE OR DELETE ON indexed_key_epoch FOR EACH STATEMENT EXECUTE FUNCTION reject_committed_retry_write();",
    ).execute(&pool).await.unwrap();
    assert_eq!(
        storage
            .unseal_agent(
                "test",
                "read_only_retry",
                &state,
                ShardEpoch(7),
                ShardEpoch(6),
                std::time::SystemTime::UNIX_EPOCH
            )
            .await
            .unwrap(),
        golem_worker_executor::storage::indexed::UnsealAgentResult::Unsealed
    );
    assert_eq!(
        storage
            .observe_agent("test", "after_retry", &state.owned_agent_id)
            .await
            .unwrap(),
        before
    );
}

async fn wait_for_indexed_sql_wait(pool: &sqlx::PgPool, query_fragment: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let (blocked,): (bool,) = sqlx::query_as(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() AND query LIKE '%' || $1 || '%' AND cardinality(pg_blocking_pids(pid)) > 0)",
            ).bind(query_fragment).fetch_one(pool).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("indexed operation did not reach the expected lock wait");
}

#[test]
#[test_r::timeout("120s")]
async fn postgres_agent_transitions_serialize_fenced_writes_without_blocking_other_agents() {
    use golem_worker_executor::storage::indexed::ReleaseAgentResult;
    use std::time::Duration;
    let wrapper = PostgresIndexedStorageWrapper {
        postgres: DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await,
    };
    let config = wrapper.fresh_database().await;
    let storage: Arc<dyn IndexedStorage + Send + Sync> =
        Arc::new(PostgresIndexedStorage::configured(&config).await.unwrap());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect_with(config.postgres.connect_options())
        .await
        .unwrap();
    let state = sealed_index_state(AgentMode::Durable);
    let key = state.owned_agent_id.storage_key();
    seed_sealed_index(&storage, &state, 6).await;

    // A database trigger pauses the real fenced append after it owns the epoch row.
    sqlx::raw_sql(&format!(
        "CREATE TABLE append_gate (id INT PRIMARY KEY); INSERT INTO append_gate VALUES (1);
         CREATE FUNCTION hold_test_append() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
         IF NEW.agent = '{}'::uuid AND NEW.namespace = 'durable-worker-oplog' THEN
         PERFORM id FROM append_gate WHERE id = 1 FOR UPDATE; END IF; RETURN NEW; END $$;
         CREATE TRIGGER hold_test_append BEFORE INSERT ON index_storage FOR EACH ROW EXECUTE FUNCTION hold_test_append();",
        state.owned_agent_id.agent_id.as_uuid()
    )).execute(&pool).await.unwrap();
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM append_gate WHERE id = 1 FOR UPDATE")
        .execute(&mut *gate)
        .await
        .unwrap();
    let writer = storage.clone();
    let ns = state.primary_namespace();
    let append_key = key.clone();
    let append = tokio::spawn(async move {
        writer
            .append(
                "test",
                "append",
                "entry",
                ns,
                &append_key,
                2,
                vec![42],
                Some(ShardEpoch(6)),
                None,
            )
            .await
    });
    wait_for_indexed_sql_wait(&pool, "WITH locked_epoch").await;
    let releaser = storage.clone();
    let release_state = state.clone();
    let release = tokio::spawn(async move {
        releaser
            .release_agent("test", "release", &release_state, ShardEpoch(6))
            .await
    });
    wait_for_indexed_sql_wait(&pool, "ORDER BY namespace FOR UPDATE").await;
    // The blocked transition may hold its agent lock, but cannot serialize another agent.
    let other = sealed_index_state(AgentMode::Durable);
    tokio::time::timeout(
        Duration::from_secs(2),
        seed_sealed_index(&storage, &other, 19),
    )
    .await
    .expect("unrelated agent was blocked");
    gate.commit().await.unwrap();
    append.await.unwrap().unwrap();
    assert_eq!(
        release.await.unwrap().unwrap(),
        ReleaseAgentResult::NotReleased
    );
    assert_eq!(
        storage
            .read(
                "test",
                "preserved",
                "entry",
                state.primary_namespace(),
                &key,
                2,
                2
            )
            .await
            .unwrap(),
        vec![(2, vec![42])]
    );
    storage
        .drop_prefix(
            "test",
            "archive",
            state.primary_namespace(),
            &key,
            2,
            Some(ShardEpoch(6)),
        )
        .await
        .unwrap();

    // Lock the last namespace so release locks the primary first, then pauses.
    sqlx::query("INSERT INTO indexed_key_epoch (namespace, env, agent, epoch) VALUES ('ephemeral-worker-zz-test-gate', $1, $2, 6)")
        .bind(state.owned_agent_id.environment_id.0).bind(state.owned_agent_id.agent_id.as_uuid()).execute(&pool).await.unwrap();
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT epoch FROM indexed_key_epoch WHERE namespace = 'ephemeral-worker-zz-test-gate' AND env = $1 AND agent = $2 FOR UPDATE")
        .bind(state.owned_agent_id.environment_id.0).bind(state.owned_agent_id.agent_id.as_uuid()).execute(&mut *gate).await.unwrap();
    let releaser = storage.clone();
    let release_state = state.clone();
    let release = tokio::spawn(async move {
        releaser
            .release_agent("test", "release_first", &release_state, ShardEpoch(6))
            .await
    });
    wait_for_indexed_sql_wait(&pool, "ORDER BY namespace FOR UPDATE").await;
    let writer = storage.clone();
    let ns = state.primary_namespace();
    let append_key = key.clone();
    let append = tokio::spawn(async move {
        writer
            .append(
                "test",
                "late_append",
                "entry",
                ns,
                &append_key,
                2,
                vec![43],
                Some(ShardEpoch(6)),
                None,
            )
            .await
    });
    wait_for_indexed_sql_wait(&pool, "WITH locked_epoch").await;
    // An epoch claim on a namespace that does not exist must wait for release, then report absent.
    let owner = storage.clone();
    let new_ns = IndexedStorageNamespace::BlobOplogManifest {
        owned_agent_id: state.owned_agent_id.clone(),
        agent_mode: AgentMode::Ephemeral,
        level: 73,
    };
    let claim_key = key.clone();
    let claim_presence = state.epoch_namespaces();
    let claim = tokio::spawn(async move {
        owner
            .set_key_epoch(
                "test",
                "new_namespace",
                new_ns,
                &claim_key,
                ShardEpoch(7),
                &claim_presence,
                false,
            )
            .await
    });
    wait_for_indexed_sql_wait(&pool, "pg_advisory_xact_lock").await;
    gate.commit().await.unwrap();
    assert_eq!(
        release.await.unwrap().unwrap(),
        ReleaseAgentResult::Released
    );
    assert_fenced(append.await.unwrap(), 6, None);
    assert!(
        !claim.await.unwrap().unwrap(),
        "the new owner must take the absent/unseal path"
    );
    let (rows,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM index_storage WHERE env = $1 AND agent = $2")
            .bind(state.owned_agent_id.environment_id.0)
            .bind(state.owned_agent_id.agent_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 0);

    // The deadline is checked on the server after an epoch-lock wait, not at transaction start.
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT epoch FROM indexed_key_epoch WHERE env = $1 AND agent = $2 FOR UPDATE")
        .bind(state.owned_agent_id.environment_id.0)
        .bind(state.owned_agent_id.agent_id.as_uuid())
        .execute(&mut *gate)
        .await
        .unwrap();
    let deadline = std::time::SystemTime::now() + Duration::from_millis(400);
    let unsealer = storage.clone();
    let unseal_state = state.clone();
    let unseal = tokio::spawn(async move {
        unsealer
            .unseal_agent(
                "test",
                "expired_after_wait",
                &unseal_state,
                ShardEpoch(7),
                ShardEpoch(6),
                deadline,
            )
            .await
    });
    wait_for_indexed_sql_wait(&pool, "ORDER BY namespace FOR UPDATE").await;
    tokio::time::sleep(
        deadline
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            + Duration::from_millis(20),
    )
    .await;
    gate.commit().await.unwrap();
    assert_eq!(
        unseal.await.unwrap().unwrap(),
        golem_worker_executor::storage::indexed::UnsealAgentResult::Expired
    );
    let (rows,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM index_storage WHERE env = $1 AND agent = $2")
            .bind(state.owned_agent_id.environment_id.0)
            .bind(state.owned_agent_id.agent_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 0);

    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT epoch FROM indexed_key_epoch WHERE env = $1 AND agent = $2 FOR UPDATE")
        .bind(state.owned_agent_id.environment_id.0)
        .bind(state.owned_agent_id.agent_id.as_uuid())
        .execute(&mut *gate)
        .await
        .unwrap();
    let claim_namespace = IndexedStorageNamespace::BlobOplogManifest {
        owned_agent_id: state.owned_agent_id.clone(),
        agent_mode: AgentMode::Ephemeral,
        level: 73,
    };
    let timeout_result = storage
        .set_key_epoch(
            "test",
            "lock_timeout",
            claim_namespace,
            &key,
            ShardEpoch(8),
            &[],
            false,
        )
        .await;
    assert!(
        matches!(timeout_result, Err(IndexedStorageError::Transient(_))),
        "lock contention must retry, got {timeout_result:?}"
    );
    gate.rollback().await.unwrap();
}
