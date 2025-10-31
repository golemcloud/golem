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

use crate::components::cloud_service::provided::ProvidedCloudService;
use crate::components::cloud_service::spawned::SpawnedCloudService;
use crate::components::cloud_service::CloudService;

use crate::components::component_compilation_service::provided::ProvidedComponentCompilationService;
use crate::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use crate::components::component_compilation_service::ComponentCompilationService;

use crate::components::component_service::provided::ProvidedComponentService;
use crate::components::component_service::spawned::SpawnedComponentService;
use crate::components::component_service::ComponentService;
use crate::components::rdb::docker_postgres::DockerPostgresRdb;

use crate::components::rdb::provided_postgres::ProvidedPostgresRdb;
use crate::components::rdb::{PostgresInfo, Rdb};

use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis::Redis;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::service::spawned::SpawnedService;
use crate::components::service::Service;

use crate::components::shard_manager::provided::ProvidedShardManager;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::shard_manager::ShardManager;

use crate::components::worker_executor_cluster::provided::ProvidedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;

use crate::benchmark::BenchmarkConfig;
use crate::components::worker_service::provided::ProvidedWorkerService;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::components::worker_service::WorkerService;
use crate::config::{GolemClientProtocol, TestDependencies, TestService};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use golem_common::tracing::directive::warn;
use golem_common::tracing::{init_tracing, TracingConfig};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tracing::Level;
use uuid::Uuid;

/// Test dependencies created from command line arguments
///
/// To be used when a single executable with an async entry point requires
/// setting up the test infrastructure, for example, in benchmarks.
#[allow(dead_code)]
#[derive(Clone)]
pub struct BenchmarkTestDependencies {
    rdb: Arc<dyn Rdb>,
    redis: Arc<dyn Redis>,
    redis_monitor: Arc<dyn RedisMonitor>,
    shard_manager: Arc<dyn ShardManager>,
    component_service: Arc<dyn ComponentService>,
    component_compilation_service: Arc<dyn ComponentCompilationService>,
    worker_service: Arc<dyn WorkerService>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster>,
    blob_storage: Arc<dyn BlobStorage>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    cloud_service: Arc<dyn CloudService>,
    component_directory: PathBuf,
    component_temp_directory: Arc<TempDir>,
}

#[derive(Parser, Debug, Clone)]
#[command()]
pub struct BenchmarkCliParameters {
    #[clap(subcommand)]
    pub benchmark_config: BenchmarkConfig,

    #[arg(long, default_value = "false")]
    pub quiet: bool,
    #[arg(long, default_value = "false")]
    pub verbose: bool,
    #[arg(long, default_value = "false")]
    pub json: bool,

    /// Only display the primary benchmark results (no per-worker measurements, for example)
    #[arg(long, default_value = "false")]
    pub primary_only: bool,

    #[arg(long, default_value = "false")]
    pub otlp: bool,
}

impl BenchmarkCliParameters {
    pub fn default_log_level(&self) -> &'static str {
        if self.quiet {
            "warn"
        } else if self.verbose {
            "debug"
        } else {
            "info"
        }
    }

    pub fn service_verbosity(&self) -> Level {
        if self.verbose {
            Level::DEBUG
        } else {
            Level::WARN
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
#[command()]
#[allow(clippy::large_enum_variant)]
pub enum TestMode {
    #[command()]
    Provided {
        #[command(flatten)]
        postgres: PostgresInfo,
        #[arg(long, default_value = "localhost")]
        redis_host: String,
        #[arg(long, default_value = "6379")]
        redis_port: u16,
        #[arg(long, default_value = "")]
        redis_prefix: String,
        #[arg(long, default_value = "localhost")]
        shard_manager_host: String,
        #[arg(long, default_value = "8080")]
        shard_manager_http_port: u16,
        #[arg(long, default_value = "9090")]
        shard_manager_grpc_port: u16,
        #[arg(long, default_value = "localhost")]
        component_service_host: String,
        #[arg(long, default_value = "8081")]
        component_service_http_port: u16,
        #[arg(long, default_value = "9091")]
        component_service_grpc_port: u16,
        #[arg(long, default_value = "localhost")]
        component_compilation_service_host: String,
        #[arg(long, default_value = "8082")]
        component_compilation_service_http_port: u16,
        #[arg(long, default_value = "9092")]
        component_compilation_service_grpc_port: u16,
        #[arg(long, default_value = "localhost")]
        worker_service_host: String,
        #[arg(long, default_value = "8083")]
        worker_service_http_port: u16,
        #[arg(long, default_value = "9093")]
        worker_service_grpc_port: u16,
        #[arg(long, default_value = "8084")]
        worker_service_custom_request_port: u16,
        #[arg(long, default_value = "localhost")]
        worker_executor_host: String,
        #[arg(long, default_value = "8100")]
        worker_executor_http_port: u16,
        #[arg(long, default_value = "9100")]
        worker_executor_grpc_port: u16,
        #[arg(long, default_value = "localhost")]
        cloud_service_host: String,
        #[arg(long, default_value = "8085")]
        cloud_service_http_port: u16,
        #[arg(long, default_value = "9095")]
        cloud_service_grpc_port: u16,
        #[arg(long, default_value = "target/data/blob")]
        blob_storage_path: PathBuf,
        #[arg(long, default_value = "test-components")]
        component_directory: String,
    },
    #[command()]
    Spawned {
        #[arg(long, default_value = ".")]
        workspace_root: String,
        #[arg(long, default_value = "target/release")]
        build_target: String,
        #[arg(long, default_value = "6379")]
        redis_port: u16,
        #[arg(long, default_value = "")]
        redis_prefix: String,
        #[arg(long, default_value = "8080")]
        shard_manager_http_port: u16,
        #[arg(long, default_value = "9096")]
        shard_manager_grpc_port: u16,
        #[arg(long, default_value = "8081")]
        component_service_http_port: u16,
        #[arg(long, default_value = "9091")]
        component_service_grpc_port: u16,
        #[arg(long, default_value = "8082")]
        component_compilation_service_http_port: u16,
        #[arg(long, default_value = "9092")]
        component_compilation_service_grpc_port: u16,
        #[arg(long, default_value = "8083")]
        worker_service_http_port: u16,
        #[arg(long, default_value = "9093")]
        worker_service_grpc_port: u16,
        #[arg(long, default_value = "8084")]
        worker_service_custom_request_port: u16,
        #[arg(long, default_value = "8100")]
        worker_executor_base_http_port: u16,
        #[arg(long, default_value = "9100")]
        worker_executor_base_grpc_port: u16,
        #[arg(long, default_value = "8085")]
        cloud_service_http_port: u16,
        #[arg(long, default_value = "9095")]
        cloud_service_grpc_port: u16,
        #[arg(long, default_value = "false")]
        mute_child: bool,
        #[arg(long, default_value = "test-components")]
        component_directory: String,
    },
}

impl BenchmarkTestDependencies {
    pub fn init_logging(params: &BenchmarkCliParameters) {
        init_tracing(
            &TracingConfig::test_pretty("benchmarks")
                .with_env_overrides()
                .use_stderr()
                .with_otlp(params.otlp, "localhost", 4318, "benchmarks"),
            |_output| {
                golem_common::tracing::filter::boxed::env_with_directives(
                    params
                        .default_log_level()
                        .parse()
                        .expect("Failed to parse log cli test log level"),
                    [
                        golem_common::tracing::directive::default_deps(),
                        vec![warn("golem_client")],
                    ]
                    .concat(),
                )
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    async fn make_spawned(
        verbosity: Level,
        cluster_size: usize,
        workspace_root: &str,
        build_target: &str,
        redis_port: u16,
        redis_prefix: &str,
        shard_manager_http_port: u16,
        shard_manager_grpc_port: u16,
        component_service_http_port: u16,
        component_service_grpc_port: u16,
        component_compilation_service_http_port: u16,
        component_compilation_service_grpc_port: u16,
        compilation_service_disabled: bool,
        worker_service_http_port: u16,
        worker_service_grpc_port: u16,
        worker_service_custom_request_port: u16,
        worker_executor_base_http_port: u16,
        worker_executor_base_grpc_port: u16,
        cloud_service_http_port: u16,
        cloud_service_grpc_port: u16,
        mute_child: bool,
        component_directory: &str,
        otlp: bool,
    ) -> Self {
        let workspace_root = Path::new(workspace_root).canonicalize().unwrap();
        let build_root = workspace_root.join(build_target);

        let out_level = if mute_child {
            Level::TRACE
        } else {
            Level::INFO
        };

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(&PathBuf::from("/tmp/ittest-local-object-store/golem"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let plugin_wasm_files_service = Arc::new(PluginWasmFilesService::new(blob_storage.clone()));

        let rdb: Arc<dyn Rdb> = {
            let unique_network_id = Uuid::new_v4().to_string();
            Arc::new(DockerPostgresRdb::new(&unique_network_id, true).await)
        };

        let cloud_service: Arc<dyn CloudService> = Arc::new(
            SpawnedCloudService::new(
                &build_root.join("cloud-service"),
                &workspace_root.join("cloud-service"),
                cloud_service_http_port,
                cloud_service_grpc_port,
                rdb.clone(),
                GolemClientProtocol::Http,
                verbosity,
                out_level,
                Level::ERROR,
                true,
                otlp,
            )
            .await,
        );

        let component_service: Arc<dyn ComponentService> = {
            Arc::new(
                SpawnedComponentService::new(
                    PathBuf::from(component_directory),
                    &build_root.join("golem-component-service"),
                    &workspace_root.join("golem-component-service"),
                    component_service_http_port,
                    component_service_grpc_port,
                    (!compilation_service_disabled)
                        .then_some(component_compilation_service_grpc_port),
                    rdb.clone(),
                    verbosity,
                    out_level,
                    Level::ERROR,
                    GolemClientProtocol::Http,
                    plugin_wasm_files_service.clone(),
                    cloud_service.clone(),
                    otlp,
                )
                .await,
            )
        };

        let component_compilation_service: Arc<dyn ComponentCompilationService> = Arc::new(
            SpawnedComponentCompilationService::new(
                &build_root.join("golem-component-compilation-service"),
                &workspace_root.join("golem-component-compilation-service"),
                component_compilation_service_http_port,
                component_compilation_service_grpc_port,
                component_service.clone(),
                verbosity,
                out_level,
                Level::ERROR,
                cloud_service.clone(),
                false,
                otlp,
            )
            .await,
        );

        let redis: Arc<dyn Redis> = Arc::new(SpawnedRedis::new(
            redis_port,
            redis_prefix.to_string(),
            out_level,
            Level::ERROR,
        ));

        let redis_monitor: Arc<dyn RedisMonitor> = Arc::new(SpawnedRedisMonitor::new(
            redis.clone(),
            Level::DEBUG,
            Level::ERROR,
        ));

        let shard_manager: Arc<dyn ShardManager> = Arc::new(
            SpawnedShardManager::new(
                &build_root.join("golem-shard-manager"),
                &workspace_root.join("golem-shard-manager"),
                None,
                shard_manager_http_port,
                shard_manager_grpc_port,
                redis.clone(),
                verbosity,
                out_level,
                Level::ERROR,
                otlp,
            )
            .await,
        );

        let worker_service: Arc<dyn WorkerService> = Arc::new(
            SpawnedWorkerService::new(
                &build_root.join("golem-worker-service"),
                &workspace_root.join("golem-worker-service"),
                worker_service_http_port,
                worker_service_grpc_port,
                worker_service_custom_request_port,
                component_service.clone(),
                shard_manager.clone(),
                rdb.clone(),
                verbosity,
                out_level,
                Level::ERROR,
                GolemClientProtocol::Http,
                cloud_service.clone(),
                false,
                otlp,
            )
            .await,
        );
        let worker_executor_cluster: Arc<dyn WorkerExecutorCluster> = Arc::new(
            SpawnedWorkerExecutorCluster::new(
                cluster_size,
                worker_executor_base_http_port,
                worker_executor_base_grpc_port,
                &build_root.join("worker-executor"),
                &workspace_root.join("golem-worker-executor"),
                redis.clone(),
                component_service.clone(),
                shard_manager.clone(),
                worker_service.clone(),
                verbosity,
                out_level,
                Level::ERROR,
                true,
                cloud_service.clone(),
                otlp,
            )
            .await,
        );

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
            component_directory: Path::new(component_directory).to_path_buf(),
            blob_storage,
            plugin_wasm_files_service,
            initial_component_files_service,
            component_temp_directory: Arc::new(TempDir::new().unwrap()),
            cloud_service,
        }
    }

    pub async fn new(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        compilation_cache_disabled: bool,
        otlp: bool,
    ) -> Self {
        match mode {
            TestMode::Provided {
                postgres,
                redis_host,
                redis_port,
                redis_prefix,
                shard_manager_host,
                shard_manager_http_port,
                shard_manager_grpc_port,
                component_service_host,
                component_service_http_port,
                component_service_grpc_port,
                component_compilation_service_host,
                component_compilation_service_http_port,
                component_compilation_service_grpc_port,
                worker_service_host,
                worker_service_http_port,
                worker_service_grpc_port,
                worker_service_custom_request_port,
                worker_executor_host,
                worker_executor_http_port,
                worker_executor_grpc_port,
                cloud_service_host,
                cloud_service_http_port,
                cloud_service_grpc_port,
                blob_storage_path,
                component_directory,
            } => {
                let blob_storage = Arc::new(
                    FileSystemBlobStorage::new(&PathBuf::from(blob_storage_path))
                        .await
                        .unwrap(),
                );
                let initial_component_files_service =
                    Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

                let plugin_wasm_files_service =
                    Arc::new(PluginWasmFilesService::new(blob_storage.clone()));

                let rdb: Arc<dyn Rdb> = Arc::new(ProvidedPostgresRdb::new(postgres.clone()));

                let redis: Arc<dyn Redis> = Arc::new(ProvidedRedis::new(
                    redis_host.clone(),
                    *redis_port,
                    redis_prefix.clone(),
                ));

                let redis_monitor: Arc<dyn RedisMonitor> = Arc::new(SpawnedRedisMonitor::new(
                    redis.clone(),
                    Level::DEBUG,
                    Level::ERROR,
                ));

                let cloud_service: Arc<dyn CloudService> = Arc::new(
                    ProvidedCloudService::new(
                        cloud_service_host.clone(),
                        *cloud_service_http_port,
                        *cloud_service_grpc_port,
                        GolemClientProtocol::Http,
                    )
                    .await,
                );

                let shard_manager: Arc<dyn ShardManager> = Arc::new(ProvidedShardManager::new(
                    shard_manager_host.clone(),
                    *shard_manager_http_port,
                    *shard_manager_grpc_port,
                ));

                let component_service: Arc<dyn ComponentService> = Arc::new(
                    ProvidedComponentService::new(
                        component_directory.clone().into(),
                        component_service_host.clone(),
                        *component_service_http_port,
                        *component_service_grpc_port,
                        GolemClientProtocol::Http,
                        plugin_wasm_files_service.clone(),
                    )
                    .await,
                );

                let component_compilation_service: Arc<dyn ComponentCompilationService> =
                    Arc::new(ProvidedComponentCompilationService::new(
                        component_compilation_service_host.clone(),
                        *component_compilation_service_http_port,
                        *component_compilation_service_grpc_port,
                    ));

                let worker_service: Arc<dyn WorkerService> = Arc::new(
                    ProvidedWorkerService::new(
                        worker_service_host.clone(),
                        *worker_service_http_port,
                        *worker_service_grpc_port,
                        *worker_service_custom_request_port,
                        GolemClientProtocol::Http,
                        component_service.clone(),
                    )
                    .await,
                );
                let worker_executor_cluster: Arc<dyn WorkerExecutorCluster> =
                    Arc::new(ProvidedWorkerExecutorCluster::new(
                        worker_executor_host.clone(),
                        *worker_executor_http_port,
                        *worker_executor_grpc_port,
                        true,
                    ));

                Self {
                    rdb,
                    redis,
                    redis_monitor,
                    shard_manager,
                    component_service,
                    component_compilation_service,
                    worker_service,
                    worker_executor_cluster,
                    component_directory: Path::new(component_directory).to_path_buf(),
                    blob_storage,
                    plugin_wasm_files_service,
                    initial_component_files_service,
                    component_temp_directory: Arc::new(TempDir::new().unwrap()),
                    cloud_service,
                }
            }
            TestMode::Spawned {
                workspace_root,
                build_target,
                redis_port,
                redis_prefix,
                shard_manager_http_port,
                shard_manager_grpc_port,
                component_service_http_port,
                component_service_grpc_port,
                component_compilation_service_http_port,
                component_compilation_service_grpc_port,
                worker_service_http_port,
                worker_service_grpc_port,
                worker_service_custom_request_port,
                worker_executor_base_http_port,
                worker_executor_base_grpc_port,
                cloud_service_http_port,
                cloud_service_grpc_port,
                mute_child,
                component_directory,
            } => {
                Self::make_spawned(
                    verbosity,
                    cluster_size,
                    workspace_root,
                    build_target,
                    *redis_port,
                    redis_prefix,
                    *shard_manager_http_port,
                    *shard_manager_grpc_port,
                    *component_service_http_port,
                    *component_service_grpc_port,
                    *component_compilation_service_http_port,
                    *component_compilation_service_grpc_port,
                    compilation_cache_disabled,
                    *worker_service_http_port,
                    *worker_service_grpc_port,
                    *worker_service_custom_request_port,
                    *worker_executor_base_http_port,
                    *worker_executor_base_grpc_port,
                    *cloud_service_http_port,
                    *cloud_service_grpc_port,
                    *mute_child,
                    component_directory,
                    otlp,
                )
                .await
            }
        }
    }
}

#[async_trait]
impl TestDependencies for BenchmarkTestDependencies {
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
        &self.component_directory
    }

    fn component_temp_directory(&self) -> &Path {
        self.component_temp_directory.path()
    }

    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
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

    fn cloud_service(&self) -> Arc<dyn CloudService> {
        self.cloud_service.clone()
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct CliTestService {
    service: Arc<dyn Service>,
}

impl CliTestService {
    pub fn new(
        mode: &TestMode,
        verbosity: Level,
        name: String,
        env_vars: HashMap<String, String>,
        service_path: Option<String>,
    ) -> Self {
        match &mode {
            TestMode::Spawned {
                workspace_root,
                build_target,
                ..
            } => {
                let workspace_root = Path::new(workspace_root).canonicalize().unwrap();

                let workspace_root = if let Some(service_path) = service_path {
                    workspace_root.join(service_path)
                } else {
                    workspace_root
                };

                let build_root = workspace_root.join(build_target);

                let service: Arc<dyn Service> = Arc::new(SpawnedService::new(
                    name.clone(),
                    &build_root.join(name.clone()),
                    &workspace_root.join(name.clone()),
                    env_vars,
                    verbosity,
                    Level::INFO,
                    Level::ERROR,
                ));

                Self { service }
            }
            _ => {
                panic!("Test mode {:?} not supported", &mode)
            }
        }
    }
}

impl TestService for CliTestService {
    fn service(&self) -> Arc<dyn Service> {
        self.service.clone()
    }
}
