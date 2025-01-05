// Copyright 2024-2025 Golem Cloud
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

use async_trait::async_trait;
use clap::{Parser, Subcommand};
use golem_common::tracing::{init_tracing, TracingConfig};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use itertools::Itertools;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::Level;

use crate::components::component_compilation_service::docker::DockerComponentCompilationService;
use crate::components::component_compilation_service::k8s::K8sComponentCompilationService;
use crate::components::component_compilation_service::provided::ProvidedComponentCompilationService;
use crate::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::component_service::docker::DockerComponentService;
use crate::components::component_service::k8s::K8sComponentService;
use crate::components::component_service::provided::ProvidedComponentService;
use crate::components::component_service::spawned::SpawnedComponentService;
use crate::components::component_service::ComponentService;
use crate::components::k8s::{aws_nlb_service_annotations, K8sNamespace, K8sRoutingType};
use crate::components::rdb::docker_postgres::DockerPostgresRdb;
use crate::components::rdb::k8s_postgres::K8sPostgresRdb;
use crate::components::rdb::provided_postgres::ProvidedPostgresRdb;
use crate::components::rdb::{PostgresInfo, Rdb};
use crate::components::redis::docker::DockerRedis;
use crate::components::redis::k8s::K8sRedis;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis::Redis;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::service::spawned::SpawnedService;
use crate::components::service::Service;
use crate::components::shard_manager::docker::DockerShardManager;
use crate::components::shard_manager::k8s::K8sShardManager;
use crate::components::shard_manager::provided::ProvidedShardManager;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::docker::DockerWorkerExecutorCluster;
use crate::components::worker_executor_cluster::k8s::K8sWorkerExecutorCluster;
use crate::components::worker_executor_cluster::provided::ProvidedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::docker::DockerWorkerService;
use crate::components::worker_service::k8s::K8sWorkerService;
use crate::components::worker_service::provided::ProvidedWorkerService;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::components::worker_service::WorkerService;
use crate::config::{TestDependencies, TestService};
use crate::dsl::benchmark::{BenchmarkConfig, RunConfig};

/// Test dependencies created from command line arguments
///
/// To be used when a single executable with an async entry point requires
/// setting up the test infrastructure, for example in benchmarks.
#[allow(dead_code)]
#[derive(Clone)]
pub struct CliTestDependencies {
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    component_compilation_service: Arc<dyn ComponentCompilationService + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync + 'static>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    component_directory: PathBuf,
}

#[derive(Parser, Debug, Clone)]
#[command()]
pub struct CliParams {
    #[command(subcommand)]
    pub mode: TestMode,

    #[arg(long, default_value = "test-components")]
    pub component_directory: String,

    #[command(flatten)]
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
    pub keep_containers: bool,
}

impl CliParams {
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

    pub fn runs(&self) -> Vec<RunConfig> {
        let cluster_size: Vec<usize> = match self.mode {
            TestMode::Provided { .. } => {
                vec![0]
            }
            _ => self
                .benchmark_config
                .cluster_size
                .iter()
                .copied()
                .unique()
                .sorted()
                .collect(),
        };

        let size = self
            .benchmark_config
            .size
            .iter()
            .copied()
            .unique()
            .sorted()
            .collect::<Vec<_>>();
        let length = self
            .benchmark_config
            .length
            .iter()
            .copied()
            .unique()
            .sorted()
            .collect::<Vec<_>>();

        let mut res = Vec::new();

        for cluster_size in cluster_size {
            for &size in &size {
                for &length in &length {
                    res.push(RunConfig {
                        cluster_size,
                        size,
                        length,
                    })
                }
            }
        }

        res
    }
}

#[derive(Subcommand, Debug, Clone)]
#[command()]
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
        #[arg(long, default_value = "9021")]
        shard_manager_http_port: u16,
        #[arg(long, default_value = "9020")]
        shard_manager_grpc_port: u16,
        #[arg(long, default_value = "localhost")]
        component_service_host: String,
        #[arg(long, default_value = "8081")]
        component_service_http_port: u16,
        #[arg(long, default_value = "9091")]
        component_service_grpc_port: u16,
        #[arg(long, default_value = "localhost")]
        component_compilation_service_host: String,
        #[arg(long, default_value = "8083")]
        component_compilation_service_http_port: u16,
        #[arg(long, default_value = "9094")]
        component_compilation_service_grpc_port: u16,
        #[arg(long, default_value = "localhost")]
        worker_service_host: String,
        #[arg(long, default_value = "8082")]
        worker_service_http_port: u16,
        #[arg(long, default_value = "9092")]
        worker_service_grpc_port: u16,
        #[arg(long, default_value = "9093")]
        worker_service_custom_request_port: u16,
        #[arg(long, default_value = "localhost")]
        worker_executor_host: String,
        #[arg(long, default_value = "9000")]
        worker_executor_http_port: u16,
        #[arg(long, default_value = "9100")]
        worker_executor_grpc_port: u16,
    },
    #[command()]
    Docker {
        #[arg(long, default_value = "")]
        redis_prefix: String,
        #[arg(long, default_value = "9000")]
        worker_executor_base_http_port: u16,
        #[arg(long, default_value = "9100")]
        worker_executor_base_grpc_port: u16,
        #[arg(long, default_value = "false")]
        compilation_service_disabled: bool,
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
        #[arg(long, default_value = "9021")]
        shard_manager_http_port: u16,
        #[arg(long, default_value = "9020")]
        shard_manager_grpc_port: u16,
        #[arg(long, default_value = "8081")]
        component_service_http_port: u16,
        #[arg(long, default_value = "9091")]
        component_service_grpc_port: u16,
        #[arg(long, default_value = "8083")]
        component_compilation_service_http_port: u16,
        #[arg(long, default_value = "9094")]
        component_compilation_service_grpc_port: u16,
        #[arg(long, default_value = "false")]
        compilation_service_disabled: bool,
        #[arg(long, default_value = "8082")]
        worker_service_http_port: u16,
        #[arg(long, default_value = "9092")]
        worker_service_grpc_port: u16,
        #[arg(long, default_value = "9093")]
        worker_service_custom_request_port: u16,
        #[arg(long, default_value = "9000")]
        worker_executor_base_http_port: u16,
        #[arg(long, default_value = "9100")]
        worker_executor_base_grpc_port: u16,
        #[arg(long, default_value = "false")]
        mute_child: bool,
    },
    #[command()]
    Minikube {
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value = "")]
        redis_prefix: String,
        #[arg(long, default_value = "false")]
        compilation_service_disabled: bool,
    },
    #[command()]
    Aws {
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value = "")]
        redis_prefix: String,
        #[arg(long, default_value = "false")]
        compilation_service_disabled: bool,
    },
}

impl TestMode {
    pub fn compilation_service_disabled(&self) -> bool {
        match self {
            TestMode::Provided { .. } => false,
            TestMode::Docker {
                compilation_service_disabled,
                ..
            } => *compilation_service_disabled,
            TestMode::Minikube {
                compilation_service_disabled,
                ..
            } => *compilation_service_disabled,
            TestMode::Aws {
                compilation_service_disabled,
                ..
            } => *compilation_service_disabled,
            TestMode::Spawned {
                compilation_service_disabled,
                ..
            } => *compilation_service_disabled,
        }
    }
}

impl CliTestDependencies {
    pub fn init_logging(params: &CliParams) {
        init_tracing(&TracingConfig::test("cli-tests"), |_output| {
            golem_common::tracing::filter::boxed::env_with_directives(
                params
                    .default_log_level()
                    .parse()
                    .expect("Failed to parse log cli test log level"),
                golem_common::tracing::directive::default_deps(),
            )
        });
    }

    async fn make_docker(
        params: CliParams,
        cluster_size: usize,
        redis_prefix: &str,
        worker_executor_base_http_port: u16,
        worker_executor_base_grpc_port: u16,
        compilation_service_disabled: bool,
    ) -> Self {
        let params_clone = params.clone();

        let rdb_and_component_service_join = tokio::spawn(async move {
            let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                Arc::new(DockerPostgresRdb::new(true, params.keep_containers).await);

            let component_compilation_service = if !compilation_service_disabled {
                Some((
                    DockerComponentCompilationService::NAME,
                    DockerComponentCompilationService::GRPC_PORT.as_u16(),
                ))
            } else {
                None
            };

            let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
                DockerComponentService::new(
                    component_compilation_service,
                    rdb.clone(),
                    params_clone.service_verbosity(),
                    true,
                    params.keep_containers,
                )
                .await,
            );

            let component_compilation_service: Arc<
                dyn ComponentCompilationService + Send + Sync + 'static,
            > = Arc::new(
                DockerComponentCompilationService::new(
                    component_service.clone(),
                    params.keep_containers,
                    params_clone.service_verbosity(),
                )
                .await,
            );

            (rdb, component_service, component_compilation_service)
        });

        let redis: Arc<dyn Redis + Send + Sync + 'static> =
            Arc::new(DockerRedis::new(redis_prefix.to_string(), params.keep_containers).await);
        let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
            SpawnedRedisMonitor::new(redis.clone(), Level::DEBUG, Level::ERROR),
        );
        let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> = Arc::new(
            DockerShardManager::new(
                redis.clone(),
                None,
                params.service_verbosity(),
                params.keep_containers,
            )
            .await,
        );

        let (rdb, component_service, component_compilation_service) =
            rdb_and_component_service_join
                .await
                .expect("Failed to join");

        let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
            DockerWorkerService::new(
                component_service.clone(),
                shard_manager.clone(),
                rdb.clone(),
                params.service_verbosity(),
                true,
                params.keep_containers,
            )
            .await,
        );
        let worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> =
            Arc::new(
                DockerWorkerExecutorCluster::new(
                    cluster_size,
                    worker_executor_base_http_port,
                    worker_executor_base_grpc_port,
                    redis.clone(),
                    component_service.clone(),
                    shard_manager.clone(),
                    worker_service.clone(),
                    params.service_verbosity(),
                    true,
                    params.keep_containers,
                )
                .await,
            );

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(&PathBuf::from("/tmp/ittest-local-object-store/golem"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
            blob_storage,
            initial_component_files_service,
            component_directory: Path::new(&params.component_directory).to_path_buf(),
        }
    }

    async fn make_spawned(
        params: CliParams,
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
        mute_child: bool,
    ) -> Self {
        let workspace_root = Path::new(workspace_root).canonicalize().unwrap();
        let build_root = workspace_root.join(build_target);

        let out_level = if mute_child {
            Level::TRACE
        } else {
            Level::INFO
        };

        let rdb_and_component_service_join = {
            let params = params.clone();
            let workspace_root = workspace_root.clone();
            let build_root = build_root.clone();

            tokio::spawn(async move {
                let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                    Arc::new(DockerPostgresRdb::new(true, params.keep_containers).await);

                let component_compilation_service_port = if !compilation_service_disabled {
                    Some(component_compilation_service_grpc_port)
                } else {
                    None
                };
                let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
                    SpawnedComponentService::new(
                        &build_root.join("golem-component-service"),
                        &workspace_root.join("golem-component-service"),
                        component_service_http_port,
                        component_service_grpc_port,
                        component_compilation_service_port,
                        rdb.clone(),
                        params.service_verbosity(),
                        out_level,
                        Level::ERROR,
                        true,
                    )
                    .await,
                );
                let component_compilation_service: Arc<
                    dyn ComponentCompilationService + Send + Sync + 'static,
                > = Arc::new(
                    SpawnedComponentCompilationService::new(
                        &build_root.join("golem-component-compilation-service"),
                        &workspace_root.join("golem-component-compilation-service"),
                        component_compilation_service_http_port,
                        component_compilation_service_grpc_port,
                        component_service.clone(),
                        params.service_verbosity(),
                        out_level,
                        Level::ERROR,
                    )
                    .await,
                );

                (rdb, component_service, component_compilation_service)
            })
        };

        let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(SpawnedRedis::new(
            redis_port,
            redis_prefix.to_string(),
            out_level,
            Level::ERROR,
        ));
        let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
            SpawnedRedisMonitor::new(redis.clone(), Level::DEBUG, Level::ERROR),
        );
        let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> = Arc::new(
            SpawnedShardManager::new(
                &build_root.join("golem-shard-manager"),
                &workspace_root.join("golem-shard-manager"),
                None,
                shard_manager_http_port,
                shard_manager_grpc_port,
                redis.clone(),
                params.service_verbosity(),
                out_level,
                Level::ERROR,
            )
            .await,
        );

        let (rdb, component_service, component_compilation_service) =
            rdb_and_component_service_join
                .await
                .expect("Failed to join.");

        let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
            SpawnedWorkerService::new(
                &build_root.join("golem-worker-service"),
                &workspace_root.join("golem-worker-service"),
                worker_service_http_port,
                worker_service_grpc_port,
                worker_service_custom_request_port,
                component_service.clone(),
                shard_manager.clone(),
                rdb.clone(),
                params.service_verbosity(),
                out_level,
                Level::ERROR,
                true,
            )
            .await,
        );
        let worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> =
            Arc::new(
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
                    params.service_verbosity(),
                    out_level,
                    Level::ERROR,
                    true,
                )
                .await,
            );

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(&PathBuf::from("/tmp/ittest-local-object-store/golem"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
            component_directory: Path::new(&params.component_directory).to_path_buf(),
            blob_storage,
            initial_component_files_service,
        }
    }

    async fn make_minikube(
        params: CliParams,
        cluster_size: usize,
        namespace: &str,
        redis_prefix: &str,
        compilation_service_disabled: bool,
    ) -> Self {
        let routing_type = K8sRoutingType::Minikube;
        let namespace = K8sNamespace(namespace.to_string());
        let timeout = Duration::from_secs(90);

        let rdb_and_component_service_join = {
            let namespace = namespace.clone();
            let routing_type = routing_type.clone();
            tokio::spawn(async move {
                let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                    Arc::new(K8sPostgresRdb::new(&namespace, &routing_type, timeout, None).await);

                let component_compilation_service = if !compilation_service_disabled {
                    Some((
                        K8sComponentCompilationService::NAME,
                        K8sComponentCompilationService::GRPC_PORT,
                    ))
                } else {
                    None
                };
                let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
                    K8sComponentService::new(
                        &namespace,
                        &routing_type,
                        Level::INFO,
                        component_compilation_service,
                        rdb.clone(),
                        timeout,
                        None,
                        true,
                    )
                    .await,
                );
                let component_compilation_service: Arc<
                    dyn ComponentCompilationService + Send + Sync + 'static,
                > = Arc::new(
                    K8sComponentCompilationService::new(
                        &namespace,
                        &routing_type,
                        Level::INFO,
                        component_service.clone(),
                        timeout,
                        None,
                    )
                    .await,
                );

                (rdb, component_service, component_compilation_service)
            })
        };

        let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(
            K8sRedis::new(
                &namespace,
                &routing_type,
                redis_prefix.to_string(),
                timeout,
                None,
            )
            .await,
        );
        let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
            SpawnedRedisMonitor::new(redis.clone(), Level::DEBUG, Level::ERROR),
        );
        let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> = Arc::new(
            K8sShardManager::new(
                &namespace,
                &routing_type,
                Level::INFO,
                redis.clone(),
                timeout,
                None,
            )
            .await,
        );

        let (rdb, component_service, component_compilation_service) =
            rdb_and_component_service_join
                .await
                .expect("Failed to join.");

        let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
            K8sWorkerService::new(
                &namespace,
                &routing_type,
                Level::INFO,
                component_service.clone(),
                shard_manager.clone(),
                rdb.clone(),
                timeout,
                None,
                true,
            )
            .await,
        );
        let worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> =
            Arc::new(
                K8sWorkerExecutorCluster::new(
                    cluster_size,
                    &namespace,
                    &routing_type,
                    redis.clone(),
                    component_service.clone(),
                    shard_manager.clone(),
                    worker_service.clone(),
                    Level::INFO,
                    timeout,
                    None,
                    true,
                )
                .await,
            );

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(&PathBuf::from("/tmp/ittest-local-object-store/golem"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
            blob_storage,
            initial_component_files_service,
            component_directory: Path::new(&params.component_directory).to_path_buf(),
        }
    }

    async fn make_aws(
        params: CliParams,
        cluster_size: usize,
        namespace: &str,
        redis_prefix: &str,
        compilation_service_disabled: bool,
    ) -> Self {
        let routing_type = K8sRoutingType::Service;
        let namespace = K8sNamespace(namespace.to_string());
        let service_annotations = Some(aws_nlb_service_annotations());
        let timeout = Duration::from_secs(900);

        let rdb_and_component_service_join = {
            let namespace = namespace.clone();
            let routing_type = routing_type.clone();
            let service_annotations = service_annotations.clone();

            tokio::spawn(async move {
                let rdb: Arc<dyn Rdb + Send + Sync + 'static> = Arc::new(
                    K8sPostgresRdb::new(
                        &namespace,
                        &routing_type,
                        timeout,
                        service_annotations.clone(),
                    )
                    .await,
                );

                let component_compilation_service = if !compilation_service_disabled {
                    Some((
                        K8sComponentCompilationService::NAME,
                        K8sComponentCompilationService::GRPC_PORT,
                    ))
                } else {
                    None
                };
                let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
                    K8sComponentService::new(
                        &namespace,
                        &routing_type,
                        Level::INFO,
                        component_compilation_service,
                        rdb.clone(),
                        timeout,
                        service_annotations.clone(),
                        true,
                    )
                    .await,
                );
                let component_compilation_service: Arc<
                    dyn ComponentCompilationService + Send + Sync + 'static,
                > = Arc::new(
                    K8sComponentCompilationService::new(
                        &namespace,
                        &routing_type,
                        Level::INFO,
                        component_service.clone(),
                        timeout,
                        service_annotations.clone(),
                    )
                    .await,
                );

                (rdb, component_service, component_compilation_service)
            })
        };

        let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(
            K8sRedis::new(
                &namespace,
                &routing_type,
                redis_prefix.to_string(),
                timeout,
                service_annotations.clone(),
            )
            .await,
        );
        let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
            SpawnedRedisMonitor::new(redis.clone(), Level::DEBUG, Level::ERROR),
        );
        let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> = Arc::new(
            K8sShardManager::new(
                &namespace,
                &routing_type,
                Level::INFO,
                redis.clone(),
                timeout,
                service_annotations.clone(),
            )
            .await,
        );

        let (rdb, component_service, component_compilation_service) =
            rdb_and_component_service_join
                .await
                .expect("Failed to join.");

        let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
            K8sWorkerService::new(
                &namespace,
                &routing_type,
                Level::INFO,
                component_service.clone(),
                shard_manager.clone(),
                rdb.clone(),
                timeout,
                service_annotations.clone(),
                true,
            )
            .await,
        );
        let worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> =
            Arc::new(
                K8sWorkerExecutorCluster::new(
                    cluster_size,
                    &namespace,
                    &routing_type,
                    redis.clone(),
                    component_service.clone(),
                    shard_manager.clone(),
                    worker_service.clone(),
                    Level::INFO,
                    timeout,
                    service_annotations.clone(),
                    true,
                )
                .await,
            );

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(&PathBuf::from("/tmp/ittest-local-object-store/golem"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
            component_directory: Path::new(&params.component_directory).to_path_buf(),
            blob_storage,
            initial_component_files_service,
        }
    }

    pub async fn new(params: CliParams, cluster_size: usize) -> Self {
        match &params.mode {
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
            } => {
                let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                    Arc::new(ProvidedPostgresRdb::new(postgres.clone()));
                let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(ProvidedRedis::new(
                    redis_host.clone(),
                    *redis_port,
                    redis_prefix.clone(),
                ));
                let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
                    SpawnedRedisMonitor::new(redis.clone(), Level::DEBUG, Level::ERROR),
                );
                let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> =
                    Arc::new(ProvidedShardManager::new(
                        shard_manager_host.clone(),
                        *shard_manager_http_port,
                        *shard_manager_grpc_port,
                    ));
                let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
                    ProvidedComponentService::new(
                        component_service_host.clone(),
                        *component_service_http_port,
                        *component_service_grpc_port,
                        true,
                    )
                    .await,
                );
                let component_compilation_service: Arc<
                    dyn ComponentCompilationService + Send + Sync + 'static,
                > = Arc::new(ProvidedComponentCompilationService::new(
                    component_compilation_service_host.clone(),
                    *component_compilation_service_http_port,
                    *component_compilation_service_grpc_port,
                ));
                let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
                    ProvidedWorkerService::new(
                        worker_service_host.clone(),
                        *worker_service_http_port,
                        *worker_service_grpc_port,
                        *worker_service_custom_request_port,
                        true,
                    )
                    .await,
                );
                let worker_executor_cluster: Arc<
                    dyn WorkerExecutorCluster + Send + Sync + 'static,
                > = Arc::new(ProvidedWorkerExecutorCluster::new(
                    worker_executor_host.clone(),
                    *worker_executor_http_port,
                    *worker_executor_grpc_port,
                    true,
                ));

                let blob_storage = Arc::new(
                    FileSystemBlobStorage::new(&PathBuf::from(
                        "/tmp/ittest-local-object-store/golem",
                    ))
                    .await
                    .unwrap(),
                );
                let initial_component_files_service =
                    Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

                Self {
                    rdb,
                    redis,
                    redis_monitor,
                    shard_manager,
                    component_service,
                    component_compilation_service,
                    worker_service,
                    worker_executor_cluster,
                    component_directory: Path::new(&params.component_directory).to_path_buf(),
                    blob_storage,
                    initial_component_files_service,
                }
            }
            TestMode::Docker {
                redis_prefix,
                worker_executor_base_http_port,
                worker_executor_base_grpc_port,
                compilation_service_disabled,
            } => {
                Self::make_docker(
                    params.clone(),
                    cluster_size,
                    redis_prefix,
                    *worker_executor_base_http_port,
                    *worker_executor_base_grpc_port,
                    *compilation_service_disabled,
                )
                .await
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
                compilation_service_disabled,
                worker_service_http_port,
                worker_service_grpc_port,
                worker_service_custom_request_port,
                worker_executor_base_http_port,
                worker_executor_base_grpc_port,
                mute_child,
            } => {
                Self::make_spawned(
                    params.clone(),
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
                    *compilation_service_disabled,
                    *worker_service_http_port,
                    *worker_service_grpc_port,
                    *worker_service_custom_request_port,
                    *worker_executor_base_http_port,
                    *worker_executor_base_grpc_port,
                    *mute_child,
                )
                .await
            }
            TestMode::Minikube {
                namespace,
                redis_prefix,
                compilation_service_disabled,
            } => {
                Self::make_minikube(
                    params.clone(),
                    cluster_size,
                    namespace,
                    redis_prefix,
                    *compilation_service_disabled,
                )
                .await
            }
            TestMode::Aws {
                namespace,
                redis_prefix,
                compilation_service_disabled,
            } => {
                Self::make_aws(
                    params.clone(),
                    cluster_size,
                    namespace,
                    redis_prefix,
                    *compilation_service_disabled,
                )
                .await
            }
        }
    }
}

#[async_trait]
impl TestDependencies for CliTestDependencies {
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
        self.component_directory.clone()
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

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.blob_storage.clone()
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct CliTestService {
    service: Arc<dyn Service + Send + Sync + 'static>,
}

impl CliTestService {
    pub fn new(
        params: CliParams,
        name: String,
        env_vars: HashMap<String, String>,
        service_path: Option<String>,
    ) -> Self {
        match &params.mode {
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

                let service: Arc<dyn Service + Send + Sync + 'static> =
                    Arc::new(SpawnedService::new(
                        name.clone(),
                        &build_root.join(name.clone()),
                        &workspace_root.join(name.clone()),
                        env_vars,
                        params.service_verbosity(),
                        Level::INFO,
                        Level::ERROR,
                    ));

                Self { service }
            }
            _ => {
                panic!("Test mode {:?} not supported", &params.mode)
            }
        }
    }
}

impl TestService for CliTestService {
    fn service(&self) -> Arc<dyn Service + Send + Sync + 'static> {
        self.service.clone()
    }
}
