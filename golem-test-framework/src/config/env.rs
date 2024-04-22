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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::Level;

use crate::components;
use crate::components::component_service::docker::DockerComponentService;
use crate::components::component_service::spawned::SpawnedComponentService;
use crate::components::component_service::ComponentService;
use crate::components::rdb::docker_postgres::DockerPostgresRdb;
use crate::components::rdb::sqlite::SqliteRdb;
use crate::components::rdb::Rdb;
use crate::components::redis::docker::DockerRedis;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis::Redis;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::shard_manager::docker::DockerShardManager;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::docker::DockerWorkerExecutorCluster;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::docker::DockerWorkerService;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::components::worker_service::WorkerService;
use crate::config::{DbType, TestDependencies};

pub struct EnvBasedTestDependencies {
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>,
}

impl EnvBasedTestDependencies {
    pub fn blocking_new(worker_executor_cluster_size: usize) -> Self {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move { Self::new(worker_executor_cluster_size).await })
    }

    pub async fn new(worker_executor_cluster_size: usize) -> Self {
        let rdb = match Self::db_type() {
            DbType::Sqlite => {
                let sqlite_path = Path::new("../target/golem_test_db");
                let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                    Arc::new(SqliteRdb::new(sqlite_path));
                rdb
            }
            DbType::Postgres => {
                let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                    Arc::new(DockerPostgresRdb::new(!Self::use_docker()).await);
                rdb
            }
        };
        let redis = {
            let prefix = Self::redis_prefix().unwrap_or("".to_string());
            let redis: Arc<dyn Redis + Send + Sync + 'static> = if Self::use_docker() {
                Arc::new(DockerRedis::new(prefix))
            } else {
                let host = Self::redis_host().unwrap_or("localhost".to_string());
                let port = Self::redis_port().unwrap_or(6379);

                if components::redis::check_if_running(&host, port) {
                    Arc::new(ProvidedRedis::new(host, port, prefix))
                } else {
                    Arc::new(SpawnedRedis::new(
                        port,
                        prefix,
                        Self::default_stdout_level(),
                        Self::default_stderr_level(),
                    ))
                }
            };
            redis
        };
        let redis_monitor = {
            let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> =
                Arc::new(SpawnedRedisMonitor::new(
                    redis.clone(),
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                ));
            redis_monitor
        };
        let shard_manager = {
            let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> = if Self::use_docker()
            {
                Arc::new(DockerShardManager::new(
                    redis.clone(),
                    Self::default_verbosity(),
                ))
            } else {
                Arc::new(
                    SpawnedShardManager::new(
                        Path::new("../target/debug/golem-shard-manager"),
                        Path::new("../golem-shard-manager"),
                        9021,
                        9020,
                        redis.clone(),
                        Self::default_verbosity(),
                        Self::default_stdout_level(),
                        Self::default_stderr_level(),
                    )
                    .await,
                )
            };
            shard_manager
        };
        let component_service = {
            let component_service: Arc<dyn ComponentService + Send + Sync + 'static> =
                if Self::use_docker() {
                    Arc::new(DockerComponentService::new(
                        rdb.clone(),
                        Self::default_verbosity(),
                    ))
                } else {
                    Arc::new(
                        SpawnedComponentService::new(
                            Path::new("../target/debug/golem-component-service"),
                            Path::new("../golem-component-service"),
                            8081,
                            9091,
                            rdb.clone(),
                            Self::default_verbosity(),
                            Self::default_stdout_level(),
                            Self::default_stderr_level(),
                        )
                        .await,
                    )
                };
            component_service
        };
        let worker_service = {
            let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> =
                if Self::use_docker() {
                    Arc::new(DockerWorkerService::new(
                        component_service.clone(),
                        shard_manager.clone(),
                        rdb.clone(),
                        redis.clone(),
                        Self::default_verbosity(),
                    ))
                } else {
                    Arc::new(
                        SpawnedWorkerService::new(
                            Path::new("../target/debug/golem-worker-service"),
                            Path::new("../golem-worker-service"),
                            8082,
                            9092,
                            9093,
                            component_service.clone(),
                            shard_manager.clone(),
                            rdb.clone(),
                            redis.clone(),
                            Self::default_verbosity(),
                            Self::default_stdout_level(),
                            Self::default_stderr_level(),
                        )
                        .await,
                    )
                };
            worker_service
        };
        let worker_executor_cluster = {
            let worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> =
                if Self::use_docker() {
                    Arc::new(DockerWorkerExecutorCluster::new(
                        worker_executor_cluster_size,
                        9000,
                        9100,
                        redis.clone(),
                        component_service.clone(),
                        shard_manager.clone(),
                        worker_service.clone(),
                        Self::default_verbosity(),
                    ))
                } else {
                    Arc::new(
                        SpawnedWorkerExecutorCluster::new(
                            worker_executor_cluster_size,
                            9000,
                            9100,
                            Path::new("../target/debug/worker-executor"),
                            Path::new("../golem-worker-executor"),
                            redis.clone(),
                            component_service.clone(),
                            shard_manager.clone(),
                            worker_service.clone(),
                            Self::default_verbosity(),
                            Self::default_stdout_level(),
                            Self::default_stderr_level(),
                        )
                        .await,
                    )
                };
            worker_executor_cluster
        };

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            worker_service,
            worker_executor_cluster,
        }
    }

    fn db_type() -> DbType {
        let db_type_str = std::env::var("GOLEM_TEST_DB")
            .unwrap_or("".to_string())
            .to_lowercase();
        if db_type_str == "sqlite" {
            DbType::Sqlite
        } else {
            DbType::Postgres
        }
    }

    fn is_quiet() -> bool {
        std::env::var("QUIET").is_ok()
    }

    fn use_docker() -> bool {
        std::env::var("GOLEM_DOCKER_SERVICES").is_ok()
    }

    fn redis_host() -> Option<String> {
        std::env::var("REDIS_HOST").ok()
    }

    fn redis_port() -> Option<u16> {
        std::env::var("REDIS_PORT")
            .ok()
            .map(|port| port.parse().expect("Failed to parse REDIS_PORT"))
    }

    fn redis_prefix() -> Option<String> {
        std::env::var("REDIS_KEY_PREFIX").ok()
    }

    fn default_stdout_level() -> Level {
        if Self::is_quiet() {
            Level::TRACE
        } else {
            Level::INFO
        }
    }

    fn default_stderr_level() -> Level {
        if Self::is_quiet() {
            Level::TRACE
        } else {
            Level::ERROR
        }
    }

    fn default_verbosity() -> Level {
        if Self::is_quiet() {
            Level::INFO
        } else {
            Level::DEBUG
        }
    }

    fn golem_test_components() -> Option<PathBuf> {
        std::env::var("GOLEM_TEST_COMPONENTS")
            .ok()
            .map(|s| Path::new(&s).to_path_buf())
    }
}

impl TestDependencies for EnvBasedTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        self.rdb.clone()
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.redis.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        self.shard_manager.clone()
    }

    fn component_directory(&self) -> PathBuf {
        Self::golem_test_components().unwrap_or(Path::new("../test-components").to_path_buf())
    }

    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        self.component_service.clone()
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        self.worker_executor_cluster.clone()
    }
}
