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

use crate::services::golem_config::{
    GolemConfig, IndexedStorageConfig, KeyValueStorageCacheConfig, KeyValueStorageConfig,
    KeyValueStorageInnerConfig, SchedulerStorageConfig,
};
use crate::storage;
use crate::storage::indexed::IndexedStorage;
use crate::storage::indexed::multi_sqlite::MultiSqliteIndexedStorage;
use crate::storage::indexed::postgres::PostgresIndexedStorage;
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use crate::storage::keyvalue::KeyValueStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use crate::storage::keyvalue::multi_sqlite::MultiSqliteKeyValueStorage;
use crate::storage::keyvalue::namespace_routed::NamespaceRoutedKeyValueStorage;
use crate::storage::keyvalue::postgres::PostgresKeyValueStorage;
use crate::storage::keyvalue::redis::RedisKeyValueStorage;
use crate::storage::keyvalue::retrying::RetryingKeyValueStorage;
use crate::storage::keyvalue::sqlite::SqliteKeyValueStorage;
use crate::storage::scheduler::SchedulerStorage;
use crate::storage::scheduler::memory::InMemorySchedulerStorage;
use crate::storage::scheduler::postgres::PostgresSchedulerStorage;
use crate::storage::scheduler::sqlite::SqliteSchedulerStorage;
use crate::workerctx::WorkerCtx;
use crate::{Bootstrap, derive_disjoint_sqlite_config};
use anyhow::anyhow;
use golem_common::redis::RedisPool;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::s3::S3BlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use std::sync::Arc;
use tokio::task::JoinSet;

pub(crate) struct ConfiguredStorage {
    pub(crate) key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    pub(crate) scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    pub(crate) indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    pub(crate) blob_storage: Arc<dyn BlobStorage>,
}

pub(crate) async fn create<Ctx: WorkerCtx, BootstrapImpl: Bootstrap<Ctx> + ?Sized + Send + Sync>(
    golem_config: &GolemConfig,
    bootstrap: &BootstrapImpl,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<ConfiguredStorage, anyhow::Error> {
    let (sqlite, key_value_storage): (Option<SqlitePool>, Arc<dyn KeyValueStorage + Send + Sync>) =
        match &golem_config.key_value_storage {
            KeyValueStorageConfig::Postgres(postgres) => {
                let kv = PostgresKeyValueStorage::configured(postgres)
                    .await
                    .map_err(|err| anyhow!(err))?;
                let kv_metrics = kv.clone();
                join_set
                    .spawn(async move { kv_metrics.run_metrics_loop("key_value_storage").await });
                let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(kv);
                (None, key_value_storage)
            }
            KeyValueStorageConfig::NamespaceRouted(namespace_routed) => {
                let (cache_sqlite, cache_storage) = match &namespace_routed.cache {
                    KeyValueStorageCacheConfig::Redis(redis) => {
                        let pool = RedisPool::configured(redis)
                            .await
                            .map_err(|err| anyhow!(err))?;
                        let storage: Arc<dyn KeyValueStorage + Send + Sync> =
                            Arc::new(RedisKeyValueStorage::new(pool));
                        (None, storage)
                    }
                    KeyValueStorageCacheConfig::Storage(config) => {
                        build_inner_key_value_storage(config, "key_value_storage_cache", join_set)
                            .await?
                    }
                };
                let (persistent_sqlite, persistent_storage) = build_inner_key_value_storage(
                    &namespace_routed.persistent,
                    "key_value_storage_persistent",
                    join_set,
                )
                .await?;

                let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
                    NamespaceRoutedKeyValueStorage::new(cache_storage, persistent_storage),
                );

                (cache_sqlite.or(persistent_sqlite), key_value_storage)
            }
            KeyValueStorageConfig::InMemory(_) => (None, Arc::new(InMemoryKeyValueStorage::new())),
            KeyValueStorageConfig::Sqlite(sqlite) => {
                let storage = SqliteKeyValueStorage::configured(sqlite)
                    .await
                    .map_err(|err| anyhow!(err))?;
                let pool = storage.pool();
                let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(storage);
                (Some(pool), key_value_storage)
            }
            KeyValueStorageConfig::MultiSqlite(multi_sqlite) => {
                let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                    Arc::new(MultiSqliteKeyValueStorage::new(
                        &multi_sqlite.root_dir,
                        multi_sqlite.max_connections,
                        multi_sqlite.foreign_keys,
                    ));
                (None, key_value_storage)
            }
        };

    // Applied outermost, above the namespace router, so every backend - and every namespace - gets
    // the identical retry policy for transient failures.
    let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = bootstrap
        .wrap_key_value_storage(Arc::new(RetryingKeyValueStorage::new(
            key_value_storage,
            golem_config.key_value_storage_retry.clone(),
        )));

    let scheduler_storage = bootstrap
        .wrap_scheduler_storage(build_scheduler_storage(&golem_config.scheduler_storage).await?);

    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> = match &golem_config.indexed_storage
    {
        IndexedStorageConfig::Postgres(postgres) => {
            let is = PostgresIndexedStorage::configured(postgres)
                .await
                .map_err(|err| anyhow!(err))?;
            let is_metrics = is.clone();
            join_set.spawn(async move { is_metrics.run_metrics_loop().await });
            Arc::new(is)
        }
        IndexedStorageConfig::KVStoreSqlite(_) => {
            let kv_sqlite_config = match &golem_config.key_value_storage {
                KeyValueStorageConfig::Sqlite(sqlite_config) => sqlite_config,
                _ => panic!(
                    "Invalid configuration: sqlite must be used as key-value storage when using KVStoreSqlite"
                ),
            };
            // The indexed storage uses its own SQLite DB file, disjoint from the
            // key-value storage's one, so each module has an independent
            // `_sqlx_migrations` table.
            let indexed_sqlite_config = derive_disjoint_sqlite_config(kv_sqlite_config, "indexed");
            Arc::new(
                SqliteIndexedStorage::configured(&indexed_sqlite_config)
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        IndexedStorageConfig::KVStoreMultiSqlite(_) => match &golem_config.key_value_storage {
            KeyValueStorageConfig::MultiSqlite(multi_sqlite) => {
                Arc::new(MultiSqliteIndexedStorage::new(
                    &multi_sqlite.root_dir,
                    multi_sqlite.max_connections,
                    multi_sqlite.foreign_keys,
                ))
            }
            _ => panic!(
                "Invalid configuration: multi-sqlite must be used as key-value storage when using KVStoreMultiSqlite"
            ),
        },
        IndexedStorageConfig::Sqlite(sqlite) => Arc::new(
            SqliteIndexedStorage::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?,
        ),
        IndexedStorageConfig::MultiSqlite(multi_sqlite) => {
            Arc::new(MultiSqliteIndexedStorage::new(
                &multi_sqlite.root_dir,
                multi_sqlite.max_connections,
                multi_sqlite.foreign_keys,
            ))
        }
        IndexedStorageConfig::InMemory(_) => {
            Arc::new(storage::indexed::memory::InMemoryIndexedStorage::new())
        }
    };

    let blob_storage: Arc<dyn BlobStorage> = match &golem_config.blob_storage {
        BlobStorageConfig::S3(config) => Arc::new(S3BlobStorage::new(config.clone()).await),
        BlobStorageConfig::LocalFileSystem(config) => Arc::new(
            golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                .await
                .map_err(|err| anyhow!(err))?,
        ),
        BlobStorageConfig::KVStoreSqlite(_) => {
            let sqlite = sqlite
                .expect("Sqlite must be configured as key-value storage when using KVStoreSqlite");
            Arc::new(
                SqliteBlobStorage::new(sqlite.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        BlobStorageConfig::Sqlite(sqlite) => {
            let pool = SqlitePool::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            Arc::new(
                SqliteBlobStorage::new(pool.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        BlobStorageConfig::InMemory(_) => {
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
        }
    };
    let blob_storage = bootstrap.wrap_blob_storage(blob_storage);

    Ok(ConfiguredStorage {
        key_value_storage,
        scheduler_storage,
        indexed_storage,
        blob_storage,
    })
}

async fn build_scheduler_storage(
    config: &SchedulerStorageConfig,
) -> Result<Arc<dyn SchedulerStorage + Send + Sync>, anyhow::Error> {
    match config {
        SchedulerStorageConfig::Postgres(postgres) => Ok(Arc::new(
            PostgresSchedulerStorage::configured(postgres)
                .await
                .map_err(|err| anyhow!(err))?,
        )),
        SchedulerStorageConfig::Sqlite(sqlite) => Ok(Arc::new(
            SqliteSchedulerStorage::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?,
        )),
        SchedulerStorageConfig::InMemory(_) => Ok(Arc::new(InMemorySchedulerStorage::new())),
    }
}

async fn build_inner_key_value_storage(
    config: &KeyValueStorageInnerConfig,
    svc_name: &'static str,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(Option<SqlitePool>, Arc<dyn KeyValueStorage + Send + Sync>), anyhow::Error> {
    match config {
        KeyValueStorageInnerConfig::Postgres(postgres) => {
            let kv = PostgresKeyValueStorage::configured(postgres)
                .await
                .map_err(|err| anyhow!(err))?;
            let kv_metrics = kv.clone();
            join_set.spawn(async move { kv_metrics.run_metrics_loop(svc_name).await });
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(kv);
            Ok((None, key_value_storage))
        }
        KeyValueStorageInnerConfig::InMemory(_) => {
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(InMemoryKeyValueStorage::new());
            Ok((None, key_value_storage))
        }
        KeyValueStorageInnerConfig::Sqlite(sqlite) => {
            let storage = SqliteKeyValueStorage::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            let pool = storage.pool();
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(storage);
            Ok((Some(pool), key_value_storage))
        }
        KeyValueStorageInnerConfig::MultiSqlite(multi_sqlite) => {
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(MultiSqliteKeyValueStorage::new(
                    &multi_sqlite.root_dir,
                    multi_sqlite.max_connections,
                    multi_sqlite.foreign_keys,
                ));
            Ok((None, key_value_storage))
        }
    }
}

#[cfg(test)]
mod tests;
