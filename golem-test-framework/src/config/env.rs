// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::rdb::docker_postgres::DockerPostgresRdb;
use crate::components::rdb::sqlite::SqliteRdb;
use crate::components::rdb::Rdb;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis::Redis;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::registry_service::spawned::SpawnedRegistryService;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::components::worker_service::WorkerService;
use crate::config::{DbType, TestDependencies};
use async_trait::async_trait;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tracing::Level;
use uuid::Uuid;

#[derive(Clone)]
pub struct EnvBasedTestDependenciesConfig {
    pub worker_executor_cluster_size: usize,
    pub number_of_shards_override: Option<usize>,
    pub shared_client: bool,
    pub db_type: DbType,
    pub quiet: bool,
    pub redis_host: String,
    pub redis_port: u16,
    pub redis_key_prefix: String,
    pub golem_repo_root: PathBuf,
    pub unique_network_id: String,
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

        if let Some(redis_port) = opt_env_var("REDIS_KEY_PREFIX") {
            self.redis_port = redis_port.parse().expect("Failed to parse REDIS_PORT");
        }

        if let Some(redis_key_prefix) = opt_env_var("REDIS_KEY_PREFIX") {
            self.redis_key_prefix = redis_key_prefix;
        }

        if let Some(redis_prefix) = opt_env_var("REDIS_PREFIX") {
            self.redis_key_prefix = redis_prefix;
        }

        if let Some(golem_repo_root) = opt_env_var("GOLEM_REPO_ROOT") {
            self.golem_repo_root = golem_repo_root.into();
        }

        self
    }

    pub fn default_stdout_level(&self) -> Level {
        if self.quiet {
            Level::DEBUG
        } else {
            Level::INFO
        }
    }

    pub fn default_stderr_level(&self) -> Level {
        if self.quiet {
            Level::DEBUG
        } else {
            Level::ERROR
        }
    }

    pub fn default_verbosity(&self) -> Level {
        if self.quiet {
            Level::WARN
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

    fn test_components_dir(&self) -> PathBuf {
        self.golem_repo_root.join("test-components")
    }

    fn debug_targets_dirs(&self) -> PathBuf {
        self.golem_repo_root.join("target/debug")
    }

    fn storage_dir(&self) -> PathBuf {
        self.golem_repo_root.join("target/test_dependencies")
    }

    async fn canonicalize_dirs(&mut self) -> anyhow::Result<()> {
        self.golem_repo_root = tokio::fs::canonicalize(&self.golem_repo_root).await?;
        Ok(())
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
            redis_host: "localhost".to_string(),
            redis_port: 6379,
            redis_key_prefix: "".to_string(),
            golem_repo_root: PathBuf::from(".."),
            unique_network_id: Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Clone)]
pub struct EnvBasedTestDependencies {
    rdb: Arc<dyn Rdb>,
    redis: Arc<dyn Redis>,
    redis_monitor: Arc<dyn RedisMonitor>,
    shard_manager: Arc<dyn ShardManager>,
    component_compilation_service: Arc<dyn ComponentCompilationService>,
    worker_service: Arc<dyn WorkerService>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster>,
    blob_storage: Arc<dyn BlobStorage>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    temp_directory: Arc<TempDir>,
    registry_service: Arc<dyn RegistryService>,
    test_component_dir: PathBuf,
}

impl Debug for EnvBasedTestDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EnvBasedTestDependencies")
    }
}

impl EnvBasedTestDependencies {
    async fn make_rdb(config: &EnvBasedTestDependenciesConfig) -> Arc<dyn Rdb> {
        match config.db_type {
            DbType::Sqlite => {
                let sqlite_path = &config.storage_dir().join("golem_test_db");
                Arc::new(SqliteRdb::new(sqlite_path))
            }
            DbType::Postgres => {
                Arc::new(DockerPostgresRdb::new(&config.unique_network_id, false).await)
            }
        }
    }

    async fn make_redis(config: &EnvBasedTestDependenciesConfig) -> Arc<dyn Redis> {
        let prefix = config.redis_key_prefix.clone();
        let host = config.redis_host.clone();
        let port = config.redis_port;

        if crate::components::redis::check_if_running(&host, port) {
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

    async fn make_redis_monitor(
        config: &EnvBasedTestDependenciesConfig,
        redis: Arc<dyn Redis>,
    ) -> Arc<dyn RedisMonitor> {
        Arc::new(SpawnedRedisMonitor::new(
            redis,
            config.redis_monitor_stdout_level(),
            config.redis_monitor_stderr_level(),
        ))
    }

    async fn make_registry_service(
        config: &EnvBasedTestDependenciesConfig,
        rdb: &Arc<dyn Rdb>,
        component_compilation_service: &Arc<dyn ComponentCompilationService>,
    ) -> Arc<dyn RegistryService> {
        Arc::new(
            SpawnedRegistryService::new(
                &config.debug_targets_dirs().join("golem-registry-service"),
                &config.golem_repo_root.join("golem-registry-service"),
                8081,
                9091,
                rdb,
                Some(component_compilation_service),
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                false,
            )
            .await,
        )
    }

    async fn make_shard_manager(
        config: &EnvBasedTestDependenciesConfig,
        redis: Arc<dyn Redis>,
    ) -> Arc<dyn ShardManager> {
        Arc::new(
            SpawnedShardManager::new(
                &config.debug_targets_dirs().join("golem-shard-manager"),
                &config.golem_repo_root.join("golem-shard-manager"),
                config.number_of_shards_override,
                9021,
                9020,
                redis,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                false,
            )
            .await,
        )
    }

    async fn make_component_compilation_service(
        config: &EnvBasedTestDependenciesConfig,
    ) -> Arc<dyn ComponentCompilationService> {
        Arc::new(
            SpawnedComponentCompilationService::new(
                &config
                    .debug_targets_dirs()
                    .join("golem-component-compilation-service"),
                &config
                    .golem_repo_root
                    .join("golem-component-compilation-service"),
                8083,
                9094,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                true,
                false,
            )
            .await,
        )
    }

    async fn make_worker_service(
        config: &EnvBasedTestDependenciesConfig,
        shard_manager: &Arc<dyn ShardManager>,
        rdb: &Arc<dyn Rdb>,
        registry_service: &Arc<dyn RegistryService>,
    ) -> Arc<dyn WorkerService> {
        Arc::new(
            SpawnedWorkerService::new(
                &config.debug_targets_dirs().join("golem-worker-service"),
                &config.golem_repo_root.join("golem-worker-service"),
                8082,
                9092,
                9093,
                shard_manager,
                rdb,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                registry_service,
                true,
                false,
            )
            .await,
        )
    }

    async fn make_worker_executor_cluster(
        config: &EnvBasedTestDependenciesConfig,
        shard_manager: Arc<dyn ShardManager>,
        worker_service: Arc<dyn WorkerService>,
        redis: Arc<dyn Redis>,
        registry_service: Arc<dyn RegistryService>,
    ) -> Arc<dyn WorkerExecutorCluster> {
        Arc::new(
            SpawnedWorkerExecutorCluster::new(
                config.worker_executor_cluster_size,
                9000,
                9100,
                &config.debug_targets_dirs().join("worker-executor"),
                &config.golem_repo_root.join("golem-worker-executor"),
                redis,
                shard_manager,
                worker_service,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                registry_service,
                false,
            )
            .await,
        )
    }

    pub async fn new(mut config: EnvBasedTestDependenciesConfig) -> anyhow::Result<Self> {
        config.canonicalize_dirs().await?;

        let blob_storage_root = &config.storage_dir().join("blob_storage");
        tokio::fs::create_dir_all(&blob_storage_root).await?;

        let blob_storage = Arc::new(FileSystemBlobStorage::new(blob_storage_root).await.unwrap());

        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let plugin_wasm_files_service = Arc::new(PluginWasmFilesService::new(blob_storage.clone()));

        let redis = Self::make_redis(&config).await;
        {
            let mut connection = redis.get_connection(0);
            redis::cmd("FLUSHALL").exec(&mut connection).unwrap();
        }

        let redis_monitor = Self::make_redis_monitor(&config, redis.clone()).await;

        let rdb = Self::make_rdb(&config).await;

        let shard_manager = Self::make_shard_manager(&config, redis.clone()).await;

        let component_compilation_service = Self::make_component_compilation_service(&config).await;

        let registry_service: Arc<dyn RegistryService> =
            Self::make_registry_service(&config, &rdb, &component_compilation_service).await;

        let worker_service =
            Self::make_worker_service(&config, &shard_manager, &rdb, &registry_service).await;

        let worker_executor_cluster = Self::make_worker_executor_cluster(
            &config,
            shard_manager.clone(),
            worker_service.clone(),
            redis.clone(),
            registry_service.clone(),
        )
        .await;

        Ok(Self {
            rdb,
            redis,
            redis_monitor,
            blob_storage,
            initial_component_files_service,
            plugin_wasm_files_service,
            registry_service,
            temp_directory: Arc::new(TempDir::new().expect("Failed to create temporary directory")),
            shard_manager,
            component_compilation_service,
            worker_executor_cluster,
            worker_service,
            test_component_dir: config.test_components_dir(),
        })
    }
}

#[async_trait]
impl TestDependencies for EnvBasedTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb> {
        self.rdb.clone()
    }

    fn redis(&self) -> Arc<dyn Redis> {
        self.redis.clone()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage> {
        self.blob_storage.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager> {
        self.shard_manager.clone()
    }

    fn component_directory(&self) -> &Path {
        &self.test_component_dir
    }

    fn temp_directory(&self) -> &Path {
        self.temp_directory.path()
    }

    fn component_compilation_service(&self) -> Arc<dyn ComponentCompilationService> {
        self.component_compilation_service.clone()
    }

    fn worker_service(&self) -> Arc<dyn WorkerService> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster> {
        self.worker_executor_cluster.clone()
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }

    fn registry_service(&self) -> Arc<dyn RegistryService> {
        self.registry_service.clone()
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
