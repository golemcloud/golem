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

use crate::benchmark::BenchmarkConfig;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::component_compilation_service::provided::ProvidedComponentCompilationService;
use crate::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use crate::components::component_compilation_service::unavailable::UnavailableComponentCompilationService;
use crate::components::rdb::PostgresInfo;
use crate::components::rdb::Rdb;
use crate::components::rdb::docker_postgres::DockerPostgresRdb;
use crate::components::rdb::provided_postgres::ProvidedPostgresRdb;
use crate::components::rdb::unavailable::UnavailableRdb;
use crate::components::redis::Redis;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis::unavailable::UnavailableRedis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::redis_monitor::unavailable::UnavailableRedisMonitor;
use crate::components::registry_service::RegistryService;
use crate::components::registry_service::cloud::CloudRegistryService;
use crate::components::registry_service::provided::ProvidedRegistryService;
use crate::components::registry_service::spawned::SpawnedRegistryService;
use crate::components::service::Service;
use crate::components::service::spawned::SpawnedService;
use crate::components::shard_manager::ShardManager;
use crate::components::shard_manager::provided::ProvidedShardManager;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::shard_manager::unavailable::UnavailableShardManager;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_executor_cluster::provided::ProvidedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::unavailable::UnavailableWorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use crate::components::worker_service::cloud::CloudWorkerService;
use crate::components::worker_service::provided::ProvidedWorkerService;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::config::TestDependencies;
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use golem_common::tracing::directive::warn;
use golem_common::tracing::{TracingConfig, init_tracing_returning_provider};
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tempfile::TempDir;
use tracing::Level;
use url::Url;
use uuid::Uuid;

/// Process-level UUID generated on the first cloud-mode benchmark context
/// creation. All cloud contexts within the same binary invocation share this
/// run-id, which is used to prefix account/app/env names
/// (`bench-{run_id}-…`) and written into result JSON metadata.
static CLOUD_BENCH_RUN_ID: OnceLock<Uuid> = OnceLock::new();

/// Returns the suite-level run-id if any cloud benchmark context has been
/// created in this process, `None` otherwise.
pub fn cloud_bench_run_id() -> Option<Uuid> {
    CLOUD_BENCH_RUN_ID.get().copied()
}

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
    component_compilation_service: Arc<dyn ComponentCompilationService>,
    worker_service: Arc<dyn WorkerService>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster>,
    blob_storage: Arc<dyn BlobStorage>,
    initial_agent_files_service: Arc<InitialAgentFilesService>,
    component_directory: PathBuf,
    component_temp_directory: Arc<TempDir>,
    registry_service: Arc<dyn RegistryService>,
    /// Set to `Some` in cloud mode. Used to prefix account/app/env names with
    /// `bench-{run_id}-` so that orphaned state is traceable.
    run_id: Option<Uuid>,
    /// The apps base domain for cloud mode (e.g. `apps.golem.cloud`). Used to
    /// construct HTTP API deployment domains as `{env_id}.{apps_base_domain}`.
    apps_base_domain: Option<String>,
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
            Level::INFO
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
        registry_service_host: String,
        #[arg(long, default_value = "8081")]
        registry_service_http_port: u16,
        #[arg(long, default_value = "9091")]
        registry_service_grpc_port: u16,
        #[arg(long)]
        registry_service_admin_account_id: Uuid,
        #[arg(long)]
        registry_service_admin_account_email: String,
        #[arg(long)]
        registry_service_admin_account_token: String,
        #[arg(long)]
        registry_service_builtin_plugin_owner_account_id: Uuid,
        #[arg(long)]
        registry_service_default_plan_id: Uuid,
        #[arg(long)]
        registry_service_low_fuel_plan_id: Uuid,
        #[arg(long, default_value = "a2f3b4c5-d6e7-8901-abcd-ef0123456789")]
        registry_service_low_disk_space_plan_id: Uuid,
        #[arg(long, default_value = "b3c4d5e6-f7a8-9012-bcde-f01234567890")]
        registry_service_low_http_calls_plan_id: Uuid,
        #[arg(long, default_value = "c4d5e6f7-a8b9-0123-cdef-012345678901")]
        registry_service_low_rpc_calls_plan_id: Uuid,
        #[arg(long, default_value = "localhost")]
        component_compilation_service_host: String,
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
        #[arg(long, default_value = "9100")]
        worker_executor_grpc_port: u16,
        #[arg(long, default_value = "9095")]
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
        registry_service_http_port: u16,
        #[arg(long, default_value = "9091")]
        registry_service_grpc_port: u16,
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
        #[arg(long)]
        environment_state_cache_capacity: Option<usize>,
        #[arg(long, default_value = "false")]
        mute_child: bool,
        #[arg(long, default_value = "test-components")]
        component_directory: String,
    },
    /// Cloud mode: run benchmarks against a deployed Golem environment via
    /// Gateway-API hostnames. No local service processes are spawned.
    ///
    /// All management API calls (registry-service, worker-service, agents) go
    /// through a single Gateway hostname (`--api-url`). HTTP API deployment
    /// access (code-first HTTP APIs) goes through `{env_id}.{apps_base_domain}`.
    ///
    /// For `golem-dev`:
    ///   `--api-url https://release.dev-api.golem.cloud`
    ///   `--apps-base-domain apps.dev.golem.cloud`
    #[command()]
    Cloud {
        /// Base URL of the deployed Golem API Gateway. Both registry-service
        /// and worker-service paths are routed internally by the Gateway.
        ///
        /// For the `golem-dev` environment this is
        /// `https://release.dev-api.golem.cloud`.
        #[arg(long)]
        api_url: Url,
        /// Wildcard base domain used to build per-environment HTTP API
        /// deployment hostnames: `{env_id}.{apps_base_domain}`.
        ///
        /// For the `golem-dev` environment this is `apps.dev.golem.cloud`.
        #[arg(long)]
        apps_base_domain: String,
        /// Bearer token for the admin account. Used to create a fresh user
        /// account for each benchmark run, which then owns all benchmark state.
        #[arg(long)]
        admin_account_token: String,
        /// UUID of the builtin-plugin-owner account.
        /// Only needed for environment-plugin-grant tests; benchmarks do not
        /// use it so the default (nil UUID) is fine for benchmark runs.
        #[arg(long, default_value_t = Uuid::nil())]
        builtin_plugin_owner_account_id: Uuid,
        /// UUID of the default plan on the target cluster.
        /// Only needed for environment-plugin-grant tests; benchmarks do not
        /// use it so the default (nil UUID) is fine for benchmark runs.
        #[arg(long, default_value_t = Uuid::nil())]
        default_plan_id: Uuid,
        /// Optional shard-manager gRPC hostname for a kubectl port-forward
        /// (e.g. `localhost`). When set together with
        /// `--shard-manager-grpc-port`, the throughput benchmark fetches the
        /// routing table and labels RPC pairs as local/remote.
        #[arg(long)]
        shard_manager_grpc_host: Option<String>,
        /// Optional shard-manager gRPC port (e.g. `9090`).
        #[arg(long)]
        shard_manager_grpc_port: Option<u16>,
        /// Directory containing test WASM component files.
        #[arg(long, default_value = "test-components")]
        component_directory: String,
    },
}

impl BenchmarkTestDependencies {
    /// Initializes logging/tracing. Returns the `SdkTracerProvider` if OTLP
    /// is enabled — the caller must call `provider.shutdown()` before process
    /// exit to flush pending spans.
    pub fn init_logging(
        params: &BenchmarkCliParameters,
    ) -> Option<opentelemetry_sdk::trace::SdkTracerProvider> {
        init_tracing_returning_provider(
            &TracingConfig::test_pretty("benchmarks")
                .with_env_overrides()
                .use_stderr()
                .with_otlp(params.otlp, "localhost", 4318, "benchmarks"),
            |output| match output {
                golem_common::tracing::Output::Otlp => {
                    golem_common::tracing::filter::boxed::default_otlp_env()
                }
                _ => golem_common::tracing::filter::boxed::env_with_directives(
                    params
                        .default_log_level()
                        .parse()
                        .expect("Failed to parse log cli test log level"),
                    [
                        golem_common::tracing::directive::default_deps(),
                        vec![warn("golem_client")],
                    ]
                    .concat(),
                ),
            },
        )
        .map(|(_tracer, provider)| provider)
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
        registry_service_http_port: u16,
        registry_service_grpc_port: u16,
        component_compilation_service_http_port: u16,
        component_compilation_service_grpc_port: u16,
        component_compilation_service_disabled: bool,
        worker_service_http_port: u16,
        worker_service_grpc_port: u16,
        worker_service_custom_request_port: u16,
        worker_executor_base_http_port: u16,
        worker_executor_base_grpc_port: u16,
        mute_child: bool,
        component_directory: &str,
        environment_state_cache_capacity: Option<usize>,
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
        let initial_agent_files_service =
            Arc::new(InitialAgentFilesService::new(blob_storage.clone()));

        let rdb: Arc<dyn Rdb> = {
            let unique_network_id = Uuid::new_v4().to_string();
            Arc::new(DockerPostgresRdb::new(&unique_network_id, true).await)
        };

        let component_compilation_service: Arc<dyn ComponentCompilationService> = Arc::new(
            SpawnedComponentCompilationService::new(
                &build_root.join("golem-component-compilation-service"),
                &workspace_root.join("golem-component-compilation-service"),
                component_compilation_service_http_port,
                component_compilation_service_grpc_port,
                verbosity,
                out_level,
                Level::ERROR,
                false,
                otlp,
            )
            .await,
        );

        let registry_service: Arc<dyn RegistryService> = Arc::new(
            SpawnedRegistryService::new(
                &build_root.join("golem-registry-service"),
                &workspace_root.join("golem-registry-service"),
                registry_service_http_port,
                registry_service_grpc_port,
                &rdb,
                (!component_compilation_service_disabled).then_some(&component_compilation_service),
                verbosity,
                out_level,
                Level::ERROR,
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
                rdb.clone(),
                registry_service.clone(),
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
                worker_service_custom_request_port + 2,
                &shard_manager,
                &rdb,
                &redis,
                verbosity,
                out_level,
                Level::ERROR,
                &registry_service,
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
                rdb.clone(),
                shard_manager.clone(),
                worker_service.clone(),
                verbosity,
                out_level,
                Level::ERROR,
                registry_service.clone(),
                environment_state_cache_capacity,
                None,
                otlp,
            )
            .await,
        );

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
            component_directory: Path::new(component_directory).to_path_buf(),
            blob_storage,
            initial_agent_files_service,
            component_temp_directory: Arc::new(TempDir::new().unwrap()),
            registry_service,
            run_id: None,
            apps_base_domain: None,
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
                registry_service_host,
                registry_service_http_port,
                registry_service_grpc_port,
                registry_service_admin_account_id,
                registry_service_admin_account_email,
                registry_service_admin_account_token,
                registry_service_builtin_plugin_owner_account_id,
                registry_service_default_plan_id,
                registry_service_low_fuel_plan_id,
                registry_service_low_disk_space_plan_id,
                registry_service_low_http_calls_plan_id,
                registry_service_low_rpc_calls_plan_id,
                component_compilation_service_host,
                component_compilation_service_grpc_port,
                worker_service_host,
                worker_service_http_port,
                worker_service_grpc_port,
                worker_service_custom_request_port,
                worker_executor_host,
                worker_executor_grpc_port,
                blob_storage_path,
                component_directory,
            } => {
                let blob_storage = Arc::new(
                    FileSystemBlobStorage::new(&PathBuf::from(blob_storage_path))
                        .await
                        .unwrap(),
                );
                let initial_agent_files_service =
                    Arc::new(InitialAgentFilesService::new(blob_storage.clone()));

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

                let registry_service: Arc<dyn RegistryService> = Arc::new(
                    ProvidedRegistryService::new(
                        registry_service_host.clone(),
                        *registry_service_http_port,
                        *registry_service_grpc_port,
                        AccountId(*registry_service_admin_account_id),
                        AccountEmail::new(registry_service_admin_account_email.clone()),
                        TokenSecret::trusted(registry_service_admin_account_token.clone()),
                        AccountId(*registry_service_builtin_plugin_owner_account_id),
                        PlanId(*registry_service_default_plan_id),
                        PlanId(*registry_service_low_fuel_plan_id),
                        PlanId(*registry_service_low_disk_space_plan_id),
                        PlanId(*registry_service_low_http_calls_plan_id),
                        PlanId(*registry_service_low_rpc_calls_plan_id),
                    )
                    .await,
                );

                let shard_manager: Arc<dyn ShardManager> = Arc::new(ProvidedShardManager::new(
                    shard_manager_host.clone(),
                    *shard_manager_http_port,
                    *shard_manager_grpc_port,
                ));

                let component_compilation_service: Arc<dyn ComponentCompilationService> =
                    Arc::new(ProvidedComponentCompilationService::new(
                        component_compilation_service_host.clone(),
                        *component_compilation_service_grpc_port,
                    ));

                let worker_service: Arc<dyn WorkerService> = Arc::new(
                    ProvidedWorkerService::new(
                        worker_service_host.clone(),
                        *worker_service_http_port,
                        *worker_service_grpc_port,
                        *worker_service_custom_request_port,
                        *worker_service_custom_request_port + 2,
                    )
                    .await,
                );
                let worker_executor_cluster: Arc<dyn WorkerExecutorCluster> =
                    Arc::new(ProvidedWorkerExecutorCluster::new(
                        worker_executor_host.clone(),
                        *worker_executor_grpc_port,
                    ));

                Self {
                    rdb,
                    redis,
                    redis_monitor,
                    shard_manager,
                    component_compilation_service,
                    worker_service,
                    worker_executor_cluster,
                    component_directory: Path::new(component_directory).to_path_buf(),
                    blob_storage,
                    initial_agent_files_service,
                    component_temp_directory: Arc::new(TempDir::new().unwrap()),
                    registry_service,
                    run_id: None,
                    apps_base_domain: None,
                }
            }
            TestMode::Spawned {
                workspace_root,
                build_target,
                redis_port,
                redis_prefix,
                shard_manager_http_port,
                shard_manager_grpc_port,
                registry_service_http_port,
                registry_service_grpc_port,
                component_compilation_service_http_port,
                component_compilation_service_grpc_port,
                worker_service_http_port,
                worker_service_grpc_port,
                worker_service_custom_request_port,
                worker_executor_base_http_port,
                worker_executor_base_grpc_port,
                environment_state_cache_capacity,
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
                    *registry_service_http_port,
                    *registry_service_grpc_port,
                    *component_compilation_service_http_port,
                    *component_compilation_service_grpc_port,
                    compilation_cache_disabled,
                    *worker_service_http_port,
                    *worker_service_grpc_port,
                    *worker_service_custom_request_port,
                    *worker_executor_base_http_port,
                    *worker_executor_base_grpc_port,
                    *mute_child,
                    component_directory,
                    *environment_state_cache_capacity,
                    otlp,
                )
                .await
            }
            TestMode::Cloud {
                api_url,
                apps_base_domain,
                admin_account_token,
                builtin_plugin_owner_account_id,
                default_plan_id,
                shard_manager_grpc_host,
                shard_manager_grpc_port,
                component_directory,
            } => {
                let blob_storage = Arc::new(
                    FileSystemBlobStorage::new(
                        &std::env::temp_dir().join("golem-bench-blob-storage"),
                    )
                    .await
                    .unwrap(),
                );
                let initial_agent_files_service =
                    Arc::new(InitialAgentFilesService::new(blob_storage.clone()));

                // Use the process-level run_id (shared across all cloud contexts in
                // this process so all benchmarks in a suite carry the same run ID).
                let run_id = *CLOUD_BENCH_RUN_ID.get_or_init(Uuid::new_v4);
                tracing::info!("Cloud benchmark run_id: {run_id}");

                // Both registry-service and worker-service are reachable via the
                // same Gateway hostname; routing is path-based.
                let registry_service: Arc<dyn RegistryService> =
                    Arc::new(CloudRegistryService::new(
                        api_url.clone(),
                        TokenSecret::trusted(admin_account_token.clone()),
                        AccountId(*builtin_plugin_owner_account_id),
                        PlanId(*default_plan_id),
                    ));

                let shard_manager: Arc<dyn ShardManager> =
                    match (shard_manager_grpc_host, shard_manager_grpc_port) {
                        (Some(host), Some(port)) => {
                            Arc::new(ProvidedShardManager::new(host.clone(), 0, *port))
                        }
                        _ => Arc::new(UnavailableShardManager),
                    };

                let worker_service: Arc<dyn WorkerService> =
                    Arc::new(CloudWorkerService::new(api_url.clone()));

                Self {
                    rdb: Arc::new(UnavailableRdb),
                    redis: Arc::new(UnavailableRedis),
                    redis_monitor: Arc::new(UnavailableRedisMonitor),
                    shard_manager,
                    component_compilation_service: Arc::new(UnavailableComponentCompilationService),
                    worker_service,
                    worker_executor_cluster: Arc::new(UnavailableWorkerExecutorCluster),
                    component_directory: Path::new(component_directory).to_path_buf(),
                    blob_storage,
                    initial_agent_files_service,
                    component_temp_directory: Arc::new(TempDir::new().unwrap()),
                    registry_service,
                    run_id: Some(run_id),
                    apps_base_domain: Some(apps_base_domain.clone()),
                }
            }
        }
    }

    /// Checks if all the spawned dependencies are still running, and if not, panics.
    ///
    /// This can be used as a checkpoint in benchmarks to avoid infinite retries.
    /// In cloud mode this is a no-op — the cloud cluster is assumed to be
    /// managed externally.
    pub async fn ensure_all_deps_running(&self) {
        if !self.worker_executor_cluster.is_running().await {
            panic!("Worker executor process(es) stopped");
        }
    }

    /// Returns the run-id for this benchmark context, if running in cloud mode.
    /// Used to prefix accounts/apps/envs with `bench-{run_id}-`.
    pub fn run_id(&self) -> Option<Uuid> {
        self.run_id
    }

    /// Returns the apps base domain for cloud mode (e.g. `apps.golem.cloud`).
    pub fn apps_base_domain(&self) -> Option<&str> {
        self.apps_base_domain.as_deref()
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

    fn temp_directory(&self) -> &Path {
        self.component_temp_directory.path()
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

    fn initial_agent_files_service(&self) -> Arc<InitialAgentFilesService> {
        self.initial_agent_files_service.clone()
    }

    fn registry_service(&self) -> Arc<dyn RegistryService> {
        self.registry_service.clone()
    }

    fn bench_name_prefix(&self) -> Option<String> {
        self.run_id.map(|id| format!("bench-{id}-"))
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
                panic!("Test mode {:?} not supported", mode)
            }
        }
    }
}
