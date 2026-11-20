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

use super::create;
use crate::Bootstrap;
use crate::bootstrap::ServerBootstrap;
use crate::derive_disjoint_sqlite_config;
use crate::services::NoAdditionalDeps;
use crate::services::golem_config::{
    GolemConfig, IndexedStorageConfig, IndexedStorageInMemoryConfig,
    IndexedStorageKVStoreMultiSqliteConfig, IndexedStorageKVStoreSqliteConfig,
    KeyValueStorageCacheConfig, KeyValueStorageConfig, KeyValueStorageInMemoryConfig,
    KeyValueStorageInnerConfig, KeyValueStorageNamespaceRoutedConfig, SchedulerStorageConfig,
    SchedulerStorageInMemoryConfig,
};
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use crate::storage::indexed::{IndexedStorage, IndexedStorageNamespace};
use crate::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use crate::storage::keyvalue::sqlite::SqliteKeyValueStorage;
use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use crate::storage::scheduler::SchedulerStorage;
use crate::storage::scheduler::memory::InMemorySchedulerStorage;
use crate::storage::scheduler::sqlite::SqliteSchedulerStorage;
use crate::workerctx::default::Context;
use bytes::Bytes;
use chrono::Utc;
use futures::{TryStreamExt, stream};
use golem_common::config::{DbSqliteConfig, RedisConfig};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFingerprint, AgentId, ScheduleId, ShardAssignment, ShardId};
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::config::{
    BlobStorageConfig, InMemoryBlobStorageConfig, KVStoreSqliteBlobStorageConfig,
};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::task::JoinSet;

fn sqlite_config(path: &Path) -> DbSqliteConfig {
    DbSqliteConfig {
        database: path.to_str().unwrap().to_owned(),
        max_connections: 1,
        foreign_keys: false,
    }
}

fn memory() -> KeyValueStorageInnerConfig {
    KeyValueStorageInnerConfig::InMemory(KeyValueStorageInMemoryConfig {})
}

fn storage_config(key_value_storage: KeyValueStorageConfig) -> GolemConfig {
    GolemConfig {
        key_value_storage,
        scheduler_storage: SchedulerStorageConfig::InMemory(SchedulerStorageInMemoryConfig {}),
        indexed_storage: IndexedStorageConfig::InMemory(IndexedStorageInMemoryConfig {}),
        blob_storage: BlobStorageConfig::InMemory(InMemoryBlobStorageConfig {}),
        ..Default::default()
    }
}

fn routed_config(
    cache: KeyValueStorageInnerConfig,
    persistent: KeyValueStorageInnerConfig,
) -> GolemConfig {
    storage_config(KeyValueStorageConfig::NamespaceRouted(
        KeyValueStorageNamespaceRoutedConfig {
            cache: KeyValueStorageCacheConfig::Storage(cache),
            persistent,
        },
    ))
}

struct StorageWrappingBootstrap {
    key_value: Arc<dyn KeyValueStorage + Send + Sync>,
    scheduler: Arc<dyn SchedulerStorage + Send + Sync>,
    blob: Arc<dyn BlobStorage>,
}

#[async_trait::async_trait]
impl Bootstrap<Context> for StorageWrappingBootstrap {
    fn create_additional_deps(
        &self,
        _registry_service: Arc<dyn RegistryService>,
    ) -> NoAdditionalDeps {
        NoAdditionalDeps::new()
    }

    fn wrap_key_value_storage(
        &self,
        _key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Arc<dyn KeyValueStorage + Send + Sync> {
        self.key_value.clone()
    }

    fn wrap_scheduler_storage(
        &self,
        _scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    ) -> Arc<dyn SchedulerStorage + Send + Sync> {
        self.scheduler.clone()
    }

    fn wrap_blob_storage(&self, _blob_storage: Arc<dyn BlobStorage>) -> Arc<dyn BlobStorage> {
        self.blob.clone()
    }
}

#[test]
#[test_r::timeout("30s")]
async fn bootstrap_wrappers_supply_the_returned_handles_above_key_value_retries()
-> anyhow::Result<()> {
    let key_value = Arc::new(InMemoryKeyValueStorage::new());
    let namespace = KeyValueStorageNamespace::RunningWorkers;
    key_value
        .set(
            "test",
            "set",
            "value",
            namespace.clone(),
            "key",
            b"wrapped-kv",
        )
        .await?;
    let faults = KeyValueStorageFaults::default();
    let failure = KeyValueStorageError::Transient("bootstrap wrapper refusal".into());
    faults.fail("get", 1, failure.clone());
    let scheduler = Arc::new(InMemorySchedulerStorage::new());
    let now = Utc::now();
    scheduler
        .insert(ScheduleId::fresh(), now, ShardId::new(0), b"wrapped-action")
        .await?;
    let blob = Arc::new(InMemoryBlobStorage::new());
    let blob_namespace = BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId::new(),
    };
    let path = Path::new("wrapped-blob");
    blob.put_raw("test", "put", blob_namespace.clone(), path, b"wrapped-blob")
        .await?;
    let bootstrap = StorageWrappingBootstrap {
        key_value: Arc::new(FaultInjectingKeyValueStorage::new(key_value, faults)),
        scheduler,
        blob,
    };
    let mut config = routed_config(memory(), memory());
    config.key_value_storage_retry.max_attempts = 3;
    config.key_value_storage_retry.min_delay = Duration::from_millis(1);
    config.key_value_storage_retry.max_delay = Duration::from_millis(1);
    let mut tasks = JoinSet::new();
    let stores = create::<Context, _>(&config, &bootstrap, &mut tasks).await?;

    assert_eq!(
        stores
            .key_value_storage
            .get("test", "get", "value", namespace.clone(), "key")
            .await,
        Err(failure)
    );
    assert_eq!(
        stores
            .key_value_storage
            .get("test", "get", "value", namespace, "key")
            .await?,
        Some(Bytes::from_static(b"wrapped-kv"))
    );
    assert_eq!(
        stores
            .scheduler_storage
            .count_due(now, &ShardAssignment::unexpiring(1, [ShardId::new(0)]))
            .await?,
        1
    );
    assert_eq!(
        stores
            .blob_storage
            .get_raw("test", "get", blob_namespace, path)
            .await?,
        Some(b"wrapped-blob".to_vec())
    );
    Ok(())
}

fn namespace_cases() -> [(KeyValueStorageNamespace, bool); 10] {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "construction".into(),
    };
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(1));
    [
        (
            KeyValueStorageNamespace::Worker {
                agent_id: Arc::new(agent_id.clone()),
            },
            true,
        ),
        (
            KeyValueStorageNamespace::AgentStatus {
                agent_id: Arc::new(agent_id.clone()),
                fingerprint,
            },
            true,
        ),
        (
            KeyValueStorageNamespace::AgentInvocationResultIndex {
                agent_id: agent_id.clone(),
                fingerprint,
            },
            true,
        ),
        (
            KeyValueStorageNamespace::AgentStatusCheckpoint {
                agent_id: Arc::new(agent_id.clone()),
                fingerprint,
            },
            true,
        ),
        (
            KeyValueStorageNamespace::AgentDurableStreamSessionIndex {
                agent_id: agent_id.clone(),
                fingerprint,
            },
            true,
        ),
        (KeyValueStorageNamespace::RunningWorkers, false),
        (
            KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
                agent_id: agent_id.clone(),
            },
            false,
        ),
        (
            KeyValueStorageNamespace::Promise {
                agent_id,
                fingerprint,
            },
            false,
        ),
        (KeyValueStorageNamespace::Schedule, false),
        (
            KeyValueStorageNamespace::UserDefined {
                environment_id: EnvironmentId::new(),
                bucket: "construction".into(),
            },
            false,
        ),
    ]
}

async fn assert_namespace_reconstruction(
    config: GolemConfig,
    cache_survives: bool,
    persistent_survives: bool,
) -> anyhow::Result<()> {
    let cases = namespace_cases();
    let mut tasks = JoinSet::new();
    let stores = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await?;
    let key_value = &stores.key_value_storage;
    stream::iter(cases.iter().map(Ok))
        .try_for_each(|(namespace, _)| async move {
            key_value
                .set(
                    "test",
                    "set",
                    "value",
                    namespace.clone(),
                    "key",
                    b"retained",
                )
                .await?;
            assert_eq!(
                key_value
                    .get("test", "get", "value", namespace.clone(), "key")
                    .await?,
                Some(Bytes::from_static(b"retained"))
            );
            Ok::<(), anyhow::Error>(())
        })
        .await?;
    drop(stores);
    let stores = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await?;
    let key_value = &stores.key_value_storage;
    stream::iter(cases.iter().map(Ok))
        .try_for_each(|(namespace, cache)| async move {
            let survives = if *cache {
                cache_survives
            } else {
                persistent_survives
            };
            assert_eq!(
                key_value
                    .get("test", "get", "value", namespace.clone(), "key")
                    .await?,
                survives.then(|| Bytes::from_static(b"retained")),
                "{namespace:?}"
            );
            Ok::<(), anyhow::Error>(())
        })
        .await?;
    Ok(())
}

#[test]
#[test_r::timeout("30s")]
async fn memory_cache_is_lost_but_sqlite_persistence_survives() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    assert_namespace_reconstruction(
        routed_config(
            memory(),
            KeyValueStorageInnerConfig::Sqlite(sqlite_config(
                &directory.path().join("persistent.db"),
            )),
        ),
        false,
        true,
    )
    .await
}

#[test]
#[test_r::timeout("30s")]
async fn sqlite_cache_survives_but_memory_persistence_is_lost() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    assert_namespace_reconstruction(
        routed_config(
            KeyValueStorageInnerConfig::Sqlite(sqlite_config(&directory.path().join("cache.db"))),
            memory(),
        ),
        true,
        false,
    )
    .await
}

#[test]
#[test_r::timeout("30s")]
async fn router_reads_and_writes_the_two_configured_sqlite_destinations() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let cache_config = sqlite_config(&directory.path().join("cache.db"));
    let persistent_config = sqlite_config(&directory.path().join("persistent.db"));
    let cache = &SqliteKeyValueStorage::configured(&cache_config).await?;
    let persistent = &SqliteKeyValueStorage::configured(&persistent_config).await?;
    let cases = namespace_cases();
    stream::iter(cases.iter().map(Ok))
        .try_for_each(|(namespace, _)| async move {
            cache
                .set(
                    "test",
                    "set",
                    "value",
                    namespace.clone(),
                    "seed",
                    b"cache-source",
                )
                .await?;
            persistent
                .set(
                    "test",
                    "set",
                    "value",
                    namespace.clone(),
                    "seed",
                    b"persistent-source",
                )
                .await?;
            Ok::<(), anyhow::Error>(())
        })
        .await?;
    let config = routed_config(
        KeyValueStorageInnerConfig::Sqlite(cache_config),
        KeyValueStorageInnerConfig::Sqlite(persistent_config),
    );
    let mut tasks = JoinSet::new();
    let stores = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await?;
    let key_value = &stores.key_value_storage;
    stream::iter(cases.iter().map(Ok))
        .try_for_each(|(namespace, expected_cache)| async move {
            let source = if *expected_cache {
                b"cache-source".as_slice()
            } else {
                b"persistent-source".as_slice()
            };
            assert_eq!(
                key_value
                    .get("test", "get", "value", namespace.clone(), "seed")
                    .await?,
                Some(Bytes::copy_from_slice(source)),
                "{namespace:?}"
            );
            key_value
                .set(
                    "test",
                    "set",
                    "value",
                    namespace.clone(),
                    "written",
                    b"routed-write",
                )
                .await?;
            assert_eq!(
                cache
                    .get("test", "get", "value", namespace.clone(), "written")
                    .await?,
                (*expected_cache).then(|| Bytes::from_static(b"routed-write")),
                "cache destination for {namespace:?}"
            );
            assert_eq!(
                persistent
                    .get("test", "get", "value", namespace.clone(), "written")
                    .await?,
                (!*expected_cache).then(|| Bytes::from_static(b"routed-write")),
                "persistent destination for {namespace:?}"
            );
            Ok::<(), anyhow::Error>(())
        })
        .await?;
    Ok(())
}

#[test]
#[test_r::timeout("30s")]
async fn blob_pool_uses_cache_sqlite_before_persistent_sqlite() -> anyhow::Result<()> {
    stream::iter(
        [
            (true, true, true),
            (true, false, true),
            (false, true, false),
        ]
        .map(Ok),
    )
    .try_for_each(
        |(cache_sqlite, persistent_sqlite, expected_cache)| async move {
            let directory = tempfile::tempdir()?;
            let cache_config = sqlite_config(&directory.path().join("cache.db"));
            let persistent_config = sqlite_config(&directory.path().join("persistent.db"));
            let mut config = routed_config(
                if cache_sqlite {
                    KeyValueStorageInnerConfig::Sqlite(cache_config.clone())
                } else {
                    memory()
                },
                if persistent_sqlite {
                    KeyValueStorageInnerConfig::Sqlite(persistent_config.clone())
                } else {
                    memory()
                },
            );
            config.blob_storage =
                BlobStorageConfig::KVStoreSqlite(KVStoreSqliteBlobStorageConfig {});
            let mut tasks = JoinSet::new();
            let stores = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await?;
            let namespace = BlobStorageNamespace::CustomStorage {
                environment_id: EnvironmentId::new(),
            };
            let path = Path::new("blob");
            stores
                .blob_storage
                .put_raw("test", "put", namespace.clone(), path, b"selected-pool")
                .await?;
            assert_eq!(
                stores
                    .blob_storage
                    .get_raw("test", "get", namespace.clone(), path)
                    .await?,
                Some(b"selected-pool".to_vec())
            );
            drop(stores);
            let cache =
                SqliteBlobStorage::new(SqlitePool::configured(&cache_config).await?).await?;
            let persistent =
                SqliteBlobStorage::new(SqlitePool::configured(&persistent_config).await?).await?;
            assert_eq!(
                cache
                    .get_raw("test", "get", namespace.clone(), path)
                    .await?,
                expected_cache.then(|| b"selected-pool".to_vec())
            );
            assert_eq!(
                persistent.get_raw("test", "get", namespace, path).await?,
                (!expected_cache).then(|| b"selected-pool".to_vec())
            );
            Ok::<(), anyhow::Error>(())
        },
    )
    .await
}

#[test]
#[test_r::timeout("30s")]
#[should_panic(
    expected = "Sqlite must be configured as key-value storage when using KVStoreSqlite"
)]
async fn blob_kvstore_sqlite_without_either_pool_still_panics() {
    let mut config = routed_config(memory(), memory());
    config.blob_storage = BlobStorageConfig::KVStoreSqlite(KVStoreSqliteBlobStorageConfig {});
    let mut tasks = JoinSet::new();
    let _ = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await;
}

#[test]
#[test_r::timeout("30s")]
async fn sqlite_handles_reconstruct_with_a_disjoint_indexed_database() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let base = sqlite_config(&directory.path().join("state.db"));
    let scheduler = sqlite_config(&directory.path().join("scheduler.db"));
    let mut config = storage_config(KeyValueStorageConfig::Sqlite(base.clone()));
    config.scheduler_storage = SchedulerStorageConfig::Sqlite(scheduler.clone());
    config.indexed_storage =
        IndexedStorageConfig::KVStoreSqlite(IndexedStorageKVStoreSqliteConfig {});
    config.blob_storage = BlobStorageConfig::KVStoreSqlite(KVStoreSqliteBlobStorageConfig {});
    let mut tasks = JoinSet::new();
    let stores = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await?;
    let kv_namespace = KeyValueStorageNamespace::RunningWorkers;
    let indexed_namespace = IndexedStorageNamespace::OpLog {
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "sqlite-construction".into(),
        },
        agent_mode: AgentMode::Durable,
    };
    let blob_namespace = BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId::new(),
    };
    let path = Path::new("blob");
    let now = Utc::now();
    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    stores
        .key_value_storage
        .set(
            "test",
            "set",
            "value",
            kv_namespace.clone(),
            "key",
            b"key-value",
        )
        .await?;
    stores
        .indexed_storage
        .append(
            "test",
            "append",
            "value",
            indexed_namespace.clone(),
            "key",
            1,
            b"indexed".to_vec(),
            None,
        )
        .await?;
    stores
        .blob_storage
        .put_raw("test", "put", blob_namespace.clone(), path, b"blob-value")
        .await?;
    stores
        .scheduler_storage
        .insert(
            ScheduleId::fresh(),
            now,
            ShardId::new(0),
            b"scheduled-action",
        )
        .await?;
    assert_eq!(
        stores
            .key_value_storage
            .get("test", "get", "value", kv_namespace.clone(), "key")
            .await?,
        Some(Bytes::from_static(b"key-value"))
    );
    assert_eq!(
        stores
            .indexed_storage
            .read(
                "test",
                "read",
                "value",
                indexed_namespace.clone(),
                "key",
                1,
                1
            )
            .await?,
        vec![(1, b"indexed".to_vec())]
    );
    assert_eq!(
        stores
            .blob_storage
            .get_raw("test", "get", blob_namespace.clone(), path)
            .await?,
        Some(b"blob-value".to_vec())
    );
    assert_eq!(
        stores.scheduler_storage.count_due(now, &assignment).await?,
        1
    );
    drop(stores);
    let stores = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await?;
    assert_eq!(
        stores
            .key_value_storage
            .get("test", "get", "value", kv_namespace, "key")
            .await?,
        Some(Bytes::from_static(b"key-value"))
    );
    assert_eq!(
        stores
            .indexed_storage
            .read(
                "test",
                "read",
                "value",
                indexed_namespace.clone(),
                "key",
                1,
                1
            )
            .await?,
        vec![(1, b"indexed".to_vec())]
    );
    assert_eq!(
        stores
            .blob_storage
            .get_raw("test", "get", blob_namespace.clone(), path)
            .await?,
        Some(b"blob-value".to_vec())
    );
    assert_eq!(
        stores.scheduler_storage.count_due(now, &assignment).await?,
        1
    );
    let indexed =
        SqliteIndexedStorage::configured(&derive_disjoint_sqlite_config(&base, "indexed")).await?;
    assert_eq!(
        indexed
            .read("test", "read", "value", indexed_namespace, "key", 1, 1)
            .await?,
        vec![(1, b"indexed".to_vec())]
    );
    let blobs = SqliteBlobStorage::new(SqlitePool::configured(&base).await?).await?;
    assert_eq!(
        blobs.get_raw("test", "get", blob_namespace, path).await?,
        Some(b"blob-value".to_vec())
    );
    let scheduler = SqliteSchedulerStorage::configured(&scheduler)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(scheduler.count_due(now, &assignment).await?, 1);
    Ok(())
}

#[test]
#[test_r::timeout("30s")]
#[should_panic(
    expected = "Invalid configuration: sqlite must be used as key-value storage when using KVStoreSqlite"
)]
async fn namespace_routed_sqlite_remains_invalid_for_kvstore_sqlite_indexed() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = routed_config(
        memory(),
        KeyValueStorageInnerConfig::Sqlite(sqlite_config(&directory.path().join("state.db"))),
    );
    config.indexed_storage =
        IndexedStorageConfig::KVStoreSqlite(IndexedStorageKVStoreSqliteConfig {});
    let mut tasks = JoinSet::new();
    let _ = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await;
}

#[test]
#[test_r::timeout("30s")]
#[should_panic(
    expected = "Invalid configuration: multi-sqlite must be used as key-value storage when using KVStoreMultiSqlite"
)]
async fn memory_remains_invalid_for_kvstore_multi_sqlite_indexed() {
    let mut config = storage_config(KeyValueStorageConfig::InMemory(
        KeyValueStorageInMemoryConfig {},
    ));
    config.indexed_storage =
        IndexedStorageConfig::KVStoreMultiSqlite(IndexedStorageKVStoreMultiSqliteConfig {});
    let mut tasks = JoinSet::new();
    let _ = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await;
}

async fn assert_sqlite_failure(cache_fails: bool) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let invalid = KeyValueStorageInnerConfig::Sqlite(sqlite_config(directory.path()));
    let config = if cache_fails {
        routed_config(invalid, memory())
    } else {
        routed_config(memory(), invalid)
    };
    let mut tasks = JoinSet::new();
    let error = match create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await {
        Err(error) => error,
        Ok(_) => panic!("an invalid SQLite destination must not fall back"),
    };
    assert!(
        matches!(error.downcast_ref::<KeyValueStorageError>(), Some(KeyValueStorageError::Other(message)) if message.contains("Sqlite key-value storage migration failed") && message.contains("unable to open database file")),
        "wrong failure source: {error:#}"
    );
    Ok(())
}

#[test]
#[test_r::timeout("30s")]
async fn cache_sqlite_initialization_failure_is_returned_without_fallback() -> anyhow::Result<()> {
    assert_sqlite_failure(true).await
}

#[test]
#[test_r::timeout("30s")]
async fn persistent_sqlite_initialization_failure_is_returned_without_fallback()
-> anyhow::Result<()> {
    assert_sqlite_failure(false).await
}

#[test]
#[test_r::timeout("30s")]
async fn lazy_redis_construction_allows_persistent_operations_without_a_server()
-> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut config = routed_config(
        memory(),
        KeyValueStorageInnerConfig::Sqlite(sqlite_config(&directory.path().join("persistent.db"))),
    );
    let KeyValueStorageConfig::NamespaceRouted(routed) = &mut config.key_value_storage else {
        unreachable!()
    };
    routed.cache = KeyValueStorageCacheConfig::Redis(RedisConfig {
        host: "127.0.0.1".into(),
        port: 1,
        tls: false,
        pool_size: 1,
        ..Default::default()
    });
    let mut tasks = JoinSet::new();
    let stores = create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await?;
    stores
        .key_value_storage
        .set(
            "test",
            "set",
            "value",
            KeyValueStorageNamespace::RunningWorkers,
            "key",
            b"persistent",
        )
        .await?;
    assert_eq!(
        stores
            .key_value_storage
            .get(
                "test",
                "get",
                "value",
                KeyValueStorageNamespace::RunningWorkers,
                "key"
            )
            .await?,
        Some(Bytes::from_static(b"persistent"))
    );
    Ok(())
}

#[test]
#[test_r::timeout("30s")]
async fn redis_pool_construction_error_is_returned_without_fallback() {
    let config = storage_config(KeyValueStorageConfig::NamespaceRouted(
        KeyValueStorageNamespaceRoutedConfig {
            cache: KeyValueStorageCacheConfig::Redis(RedisConfig {
                host: "127.0.0.1".into(),
                port: 1,
                tls: false,
                pool_size: 0,
                ..Default::default()
            }),
            persistent: memory(),
        },
    ));
    let mut tasks = JoinSet::new();
    let error = match create::<Context, _>(&config, &ServerBootstrap, &mut tasks).await {
        Err(error) => error,
        Ok(_) => panic!("an empty Redis pool must not fall back"),
    };
    let redis_error = error
        .downcast_ref::<fred::error::Error>()
        .expect("the Redis constructor error");
    assert_eq!(redis_error.kind(), &fred::error::ErrorKind::Config);
    assert_eq!(redis_error.details(), "Pool cannot be empty.");
}
