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

use golem_common::config::{DbPostgresConfig, RedisConfig};
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::AgentMode;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_executor::services::golem_config::{
    IndexedStorageConfig, IndexedStoragePostgresConfig, KeyValueStorageCacheConfig,
    KeyValueStorageConfig, KeyValueStorageInnerConfig, KeyValueStorageNamespaceRoutedConfig,
    KeyValueStoragePostgresConfig, OplogConfig, SchedulerStorageConfig,
    SchedulerStoragePostgresConfig,
};
use golem_worker_executor::storage::indexed::postgres::PostgresIndexedStorage;
use golem_worker_executor::storage::indexed::{IndexedStorage, IndexedStorageNamespace};
use golem_worker_executor::storage::keyvalue::KeyValueStorage;
use golem_worker_executor::storage::keyvalue::postgres::PostgresKeyValueStorage;
use golem_worker_executor::storage::keyvalue::sqlite::SqliteKeyValueStorage;
use golem_worker_executor_test_utils::{
    TestContext, TestExecutorOverrides, TestWorkerExecutor, WorkerExecutorTestDependencies,
    agent_oplog_length, sqlite_storage_config, start_with_oplog_config, start_with_overrides,
    start_with_redis_cache,
};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum PersistentStorage {
    Sqlite,
    Postgres(Box<DockerPostgresRdb>),
}

fn postgres_config(
    server: &DockerPostgresRdb,
    context: &TestContext,
    store: &str,
) -> DbPostgresConfig {
    let url = url::Url::parse(&server.public_connection_string()).unwrap();
    DbPostgresConfig {
        host: url.host_str().unwrap().to_string(),
        database: url.path().trim_start_matches('/').to_string(),
        username: url.username().to_string(),
        password: url.password().unwrap_or_default().to_string(),
        port: url.port().unwrap(),
        max_connections: 8,
        schema: Some(format!(
            "{}_{}",
            context.redis_prefix().replace([':', '-'], "_"),
            store
        )),
        acquire_timeout: None,
    }
}

impl PersistentStorage {
    pub(crate) async fn start_with_cache(
        &self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
    ) -> anyhow::Result<TestWorkerExecutor> {
        match self {
            Self::Sqlite => start_with_redis_cache(deps, context).await,
            Self::Postgres(server) => {
                let key_value = postgres_config(server, context, "kv");
                let indexed = postgres_config(server, context, "indexed");
                let scheduler = postgres_config(server, context, "scheduler");
                let cache = RedisConfig {
                    host: deps.redis.public_host(),
                    port: deps.redis.public_port(),
                    key_prefix: context.redis_prefix(),
                    ..Default::default()
                };
                start_with_overrides(
                    deps,
                    context,
                    TestExecutorOverrides {
                        configure: Some(Arc::new(move |config| {
                            config.key_value_storage = KeyValueStorageConfig::NamespaceRouted(
                                KeyValueStorageNamespaceRoutedConfig {
                                    cache: KeyValueStorageCacheConfig::Redis(cache.clone()),
                                    persistent: KeyValueStorageInnerConfig::Postgres(
                                        KeyValueStoragePostgresConfig {
                                            postgres: key_value.clone(),
                                        },
                                    ),
                                },
                            );
                            config.indexed_storage = IndexedStorageConfig::Postgres(
                                IndexedStoragePostgresConfig::new(indexed.clone()),
                            );
                            config.scheduler_storage =
                                SchedulerStorageConfig::Postgres(SchedulerStoragePostgresConfig {
                                    postgres: scheduler.clone(),
                                });
                        })),
                        ..Default::default()
                    },
                )
                .await
            }
        }
    }

    pub(crate) async fn key_value(
        &self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
    ) -> anyhow::Result<Arc<dyn KeyValueStorage + Send + Sync>> {
        match self {
            Self::Sqlite => Ok(Arc::new(
                SqliteKeyValueStorage::configured(&sqlite_storage_config(deps, context)).await?,
            )),
            Self::Postgres(server) => Ok(Arc::new(
                PostgresKeyValueStorage::configured(&KeyValueStoragePostgresConfig {
                    postgres: postgres_config(server, context, "kv"),
                })
                .await
                .map_err(anyhow::Error::msg)?,
            )),
        }
    }

    pub(crate) async fn start(
        &self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        oplog: Option<OplogConfig>,
    ) -> anyhow::Result<TestWorkerExecutor> {
        match self {
            Self::Sqlite => start_with_oplog_config(deps, context, oplog).await,
            Self::Postgres(server) => {
                let key_value = postgres_config(server, context, "kv");
                let indexed = postgres_config(server, context, "indexed");
                let scheduler = postgres_config(server, context, "scheduler");
                start_with_overrides(
                    deps,
                    context,
                    TestExecutorOverrides {
                        configure: Some(Arc::new(move |config| {
                            config.key_value_storage =
                                KeyValueStorageConfig::Postgres(KeyValueStoragePostgresConfig {
                                    postgres: key_value.clone(),
                                });
                            config.indexed_storage = IndexedStorageConfig::Postgres(
                                IndexedStoragePostgresConfig::new(indexed.clone()),
                            );
                            config.scheduler_storage =
                                SchedulerStorageConfig::Postgres(SchedulerStoragePostgresConfig {
                                    postgres: scheduler.clone(),
                                });
                            if let Some(oplog) = &oplog {
                                config.oplog = oplog.clone();
                            }
                        })),
                        ..Default::default()
                    },
                )
                .await
            }
        }
    }

    pub(crate) async fn primary_length(
        &self,
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        agent: &OwnedAgentId,
    ) -> anyhow::Result<u64> {
        match self {
            Self::Sqlite => agent_oplog_length(deps, context, agent).await,
            Self::Postgres(server) => {
                let storage = PostgresIndexedStorage::configured(
                    &IndexedStoragePostgresConfig::new(postgres_config(server, context, "indexed")),
                )
                .await
                .map_err(anyhow::Error::msg)?;
                Ok(storage
                    .length(
                        "test",
                        "primary_length",
                        IndexedStorageNamespace::OpLog {
                            owned_agent_id: agent.clone(),
                            agent_mode: AgentMode::Durable,
                        },
                        &agent.storage_key(),
                    )
                    .await?)
            }
        }
    }
}
