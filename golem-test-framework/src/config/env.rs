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

use crate::components;
use crate::components::component_compilation_service::docker::DockerComponentCompilationService;
use crate::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use crate::components::component_compilation_service::ComponentCompilationService;
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
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::Level;

pub struct EnvBasedTestDependenciesConfig {
    pub worker_executor_cluster_size: usize,
    pub number_of_shards_override: Option<usize>,
    pub shared_client: bool,
    pub db_type: DbType,
    pub quiet: bool,
    pub golem_docker_services: bool,
    pub keep_docker_containers: bool,
    pub redis_host: String,
    pub redis_port: u16,
    pub redis_key_prefix: String,
    pub golem_test_components: PathBuf,
}

impl EnvBasedTestDependenciesConfig {
    pub fn new() -> Self {
        Self::default().with_env_overrides()
    }

    pub fn with_env_overrides(mut self) -> Self {
        if opt_env_var("GOLEM_TEST_DB").as_deref() == Some("sqlite") {
            self.db_type = DbType::Sqlite;
        }

        if let Some(quiet) = opt_env_var_bool("QUIET") {
            self.quiet = quiet;
        }

        if let Some(golem_docker_services) = opt_env_var_bool("GOLEM_DOCKER_SERVICES") {
            self.golem_docker_services = golem_docker_services;
        }

        if let Some(keep_docker_containers) = opt_env_var_bool("KEEP_DOCKER_CONTAINERS") {
            self.keep_docker_containers = keep_docker_containers
        }

        if let Some(redis_port) = opt_env_var("REDIS_KEY_PREFIX") {
            self.redis_port = redis_port.parse().expect("Failed to parse REDIS_PORT");
        }

        if let Some(redis_key_prefix) = opt_env_var("REDIS_KEY_PREFIX") {
            self.redis_key_prefix = redis_key_prefix;
        }

        if let Some(redis_prefix) = opt_env_var("REDIS_PREFIX") {
            self.redis_key_prefix = redis_prefix;
        }

        if let Some(golem_test_components) = opt_env_var("GOLEM_TEST_COMPONENTS") {
            self.golem_test_components = golem_test_components.into();
        }

        self
    }

    pub fn default_stdout_level(&self) -> Level {
        if self.quiet {
            Level::TRACE
        } else {
            Level::INFO
        }
    }

    pub fn default_stderr_level(&self) -> Level {
        if self.quiet {
            Level::TRACE
        } else {
            Level::ERROR
        }
    }

    pub fn default_verbosity(&self) -> Level {
        if self.quiet {
            Level::INFO
        } else {
            Level::DEBUG
        }
    }

    pub fn redis_monitor_stdout_level(&self) -> Level {
        Level::TRACE
    }

    pub fn redis_monitor_stderr_level(&self) -> Level {
        Level::ERROR
    }
}

impl Default for EnvBasedTestDependenciesConfig {
    fn default() -> Self {
        Self {
            worker_executor_cluster_size: 4,
            number_of_shards_override: None,
            shared_client: false,
            db_type: DbType::Postgres,
            quiet: false,
            golem_docker_services: false,
            keep_docker_containers: false,
            redis_host: "localhost".to_string(),
            redis_port: 6379,
            redis_key_prefix: "".to_string(),
            golem_test_components: Path::new("../test-components").to_path_buf(),
        }
    }
}

#[derive(Clone)]
pub struct EnvBasedTestDependencies {
    config: Arc<EnvBasedTestDependenciesConfig>,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    component_compilation_service: Arc<dyn ComponentCompilationService + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>,
}

impl Debug for EnvBasedTestDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EnvBasedTestDependencies")
    }
}

impl EnvBasedTestDependencies {
    async fn make_rdb(
        config: Arc<EnvBasedTestDependenciesConfig>,
    ) -> Arc<dyn Rdb + Send + Sync + 'static> {
        match config.db_type {
            DbType::Sqlite => {
                let sqlite_path = Path::new("../target/golem_test_db");
                Arc::new(SqliteRdb::new(sqlite_path))
            }
            DbType::Postgres => Arc::new(
                DockerPostgresRdb::new(
                    !config.golem_docker_services,
                    config.keep_docker_containers,
                )
                .await,
            ),
        }
    }

    async fn make_redis(
        config: Arc<EnvBasedTestDependenciesConfig>,
    ) -> Arc<dyn Redis + Send + Sync + 'static> {
        let prefix = config.redis_key_prefix.clone();
        if config.golem_docker_services {
            Arc::new(DockerRedis::new(prefix, config.keep_docker_containers).await)
        } else {
            let host = config.redis_host.clone();
            let port = config.redis_port;

            if components::redis::check_if_running(&host, port) {
                Arc::new(ProvidedRedis::new(host, port, prefix))
            } else {
                Arc::new(SpawnedRedis::new(
                    port,
                    prefix,
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                ))
            }
        }
    }

    async fn make_redis_monitor(
        config: Arc<EnvBasedTestDependenciesConfig>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        Arc::new(SpawnedRedisMonitor::new(
            redis,
            config.redis_monitor_stdout_level(),
            config.redis_monitor_stderr_level(),
        ))
    }

    async fn make_shard_manager(
        config: Arc<EnvBasedTestDependenciesConfig>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        if config.golem_docker_services {
            Arc::new(
                DockerShardManager::new(
                    redis,
                    config.number_of_shards_override,
                    config.default_verbosity(),
                    config.keep_docker_containers,
                )
                .await,
            )
        } else {
            Arc::new(
                SpawnedShardManager::new(
                    Path::new("../target/debug/golem-shard-manager"),
                    Path::new("../golem-shard-manager"),
                    config.number_of_shards_override,
                    9021,
                    9020,
                    redis,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_component_service(
        config: Arc<EnvBasedTestDependenciesConfig>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    ) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        if config.golem_docker_services {
            Arc::new(
                DockerComponentService::new(
                    Some((
                        DockerComponentCompilationService::NAME,
                        DockerComponentCompilationService::GRPC_PORT.as_u16(),
                    )),
                    rdb,
                    config.default_verbosity(),
                    config.shared_client,
                    config.keep_docker_containers,
                )
                .await,
            )
        } else {
            Arc::new(
                SpawnedComponentService::new(
                    Path::new("../target/debug/golem-component-service"),
                    Path::new("../golem-component-service"),
                    8081,
                    9091,
                    Some(9094),
                    rdb,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                    config.shared_client,
                )
                .await,
            )
        }
    }

    async fn make_component_compilation_service(
        config: Arc<EnvBasedTestDependenciesConfig>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        if config.golem_docker_services {
            Arc::new(
                DockerComponentCompilationService::new(
                    component_service,
                    config.keep_docker_containers,
                    config.default_verbosity(),
                )
                .await,
            )
        } else {
            Arc::new(
                SpawnedComponentCompilationService::new(
                    Path::new("../target/debug/golem-component-compilation-service"),
                    Path::new("../golem-component-compilation-service"),
                    8083,
                    9094,
                    component_service,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_worker_service(
        config: Arc<EnvBasedTestDependenciesConfig>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    ) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        if config.golem_docker_services {
            Arc::new(
                DockerWorkerService::new(
                    component_service,
                    shard_manager,
                    rdb,
                    config.default_verbosity(),
                    config.shared_client,
                    config.keep_docker_containers,
                )
                .await,
            )
        } else {
            Arc::new(
                SpawnedWorkerService::new(
                    Path::new("../target/debug/golem-worker-service"),
                    Path::new("../golem-worker-service"),
                    8082,
                    9092,
                    9093,
                    component_service,
                    shard_manager,
                    rdb,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                    config.shared_client,
                )
                .await,
            )
        }
    }

    async fn make_worker_executor_cluster(
        config: Arc<EnvBasedTestDependenciesConfig>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        if config.golem_docker_services {
            Arc::new(
                DockerWorkerExecutorCluster::new(
                    config.worker_executor_cluster_size,
                    9000,
                    9100,
                    redis,
                    component_service,
                    shard_manager,
                    worker_service,
                    config.default_verbosity(),
                    config.shared_client,
                    config.keep_docker_containers,
                )
                .await,
            )
        } else {
            Arc::new(
                SpawnedWorkerExecutorCluster::new(
                    config.worker_executor_cluster_size,
                    9000,
                    9100,
                    Path::new("../target/debug/worker-executor"),
                    Path::new("../golem-worker-executor"),
                    redis,
                    component_service,
                    shard_manager,
                    worker_service,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                    config.shared_client,
                )
                .await,
            )
        }
    }

    pub async fn new(config: EnvBasedTestDependenciesConfig) -> Self {
        let config = Arc::new(config);

        let redis = Self::make_redis(config.clone()).await;
        {
            let mut connection = redis.get_connection(0);
            redis::cmd("FLUSHALL").execute(&mut connection);
        }

        let rdb_and_component_service_join = {
            let config = config.clone();

            tokio::spawn(async move {
                let rdb = Self::make_rdb(config.clone()).await;
                let component_service =
                    Self::make_component_service(config.clone(), rdb.clone()).await;
                let component_compilation_service = Self::make_component_compilation_service(
                    config.clone(),
                    component_service.clone(),
                )
                .await;
                (rdb, component_service, component_compilation_service)
            })
        };

        let redis_monitor_join =
            tokio::spawn(Self::make_redis_monitor(config.clone(), redis.clone()));
        let shard_manager_join =
            tokio::spawn(Self::make_shard_manager(config.clone(), redis.clone()));

        let (rdb, component_service, component_compilation_service) =
            rdb_and_component_service_join
                .await
                .expect("Failed to join.");

        let shard_manager = shard_manager_join.await.expect("Failed to join");

        let worker_service = Self::make_worker_service(
            config.clone(),
            component_service.clone(),
            shard_manager.clone(),
            rdb.clone(),
        )
        .await;
        let worker_executor_cluster = Self::make_worker_executor_cluster(
            config.clone(),
            component_service.clone(),
            shard_manager.clone(),
            worker_service.clone(),
            redis.clone(),
        )
        .await;

        let redis_monitor = redis_monitor_join.await.expect("Failed to join");

        Self {
            config: config.clone(),
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
        }
    }
}

#[async_trait]
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
        self.config.golem_test_components.clone()
    }

    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        self.component_compilation_service.clone()
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        self.worker_executor_cluster.clone()
    }
}

fn opt_env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn opt_env_var_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|value| match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
}
