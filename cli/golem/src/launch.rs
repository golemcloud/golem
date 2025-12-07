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

use crate::router::start_router;
use crate::StartedComponents;
use anyhow::Context;
use golem_common::config::DbConfig;
use golem_common::config::DbSqliteConfig;
use golem_common::model::account::AccountId;
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::model::plan::{PlanId, PlanName};
use golem_common::model::Empty;
use golem_registry_service::config::{
    ComponentCompilationEnabledConfig, LoginConfig, PrecreatedAccount, PrecreatedPlan,
    RegistryServiceConfig,
};
use golem_registry_service::RegistryService;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::config::LocalFileSystemBlobStorageConfig;
use golem_service_base::service::compiled_component::{
    CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig,
};
use golem_service_base::service::routing_table::RoutingTableConfig;
use golem_shard_manager::shard_manager_config::ShardManagerConfig;
use golem_worker_executor::services::golem_config::{
    AgentTypesServiceConfig, GolemConfig as WorkerExecutorConfig, IndexedStorageConfig,
    IndexedStorageKVStoreSqliteConfig, KeyValueStorageConfig, ResourceLimitsConfig,
    ResourceLimitsGrpcConfig, ShardManagerServiceConfig, ShardManagerServiceGrpcConfig,
    WorkerServiceGrpcConfig,
};
use golem_worker_service::config::{RouteResolverConfig, WorkerServiceConfig};
use golem_worker_service::WorkerService;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::Instrument;
use uuid::uuid;

const ADMIN_TOKEN: &str = golem_cli::config::LOCAL_WELL_KNOWN_TOKEN;

pub struct LaunchArgs {
    pub router_addr: String,
    pub router_port: u16,
    pub custom_request_port: u16,
    pub data_dir: PathBuf,
}

pub async fn launch_golem_services(
    args: &LaunchArgs,
) -> anyhow::Result<JoinSet<anyhow::Result<()>>> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    let exporter = opentelemetry_prometheus_text_exporter::ExporterBuilder::default()
        .without_counter_suffixes()
        .without_units()
        .build();

    global::set_meter_provider(
        MeterProviderBuilder::default()
            .with_reader(exporter)
            .build(),
    );

    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();

    tokio::fs::create_dir_all(&args.data_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create data directory at {}",
                args.data_dir.display()
            )
        })?;

    let started_components = start_components(args, &mut join_set).await?;

    start_router(
        &args.router_addr,
        args.router_port,
        started_components,
        &mut join_set,
    )?;

    Ok(join_set)
}

async fn start_components(
    args: &LaunchArgs,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<StartedComponents, anyhow::Error> {
    let component_compilation_service =
        run_component_compilation_service(component_compilation_service_config(args), join_set)
            .await?;

    let registry_service = run_registry_service(
        registry_service_config(args, &component_compilation_service),
        join_set,
    )
    .await?;

    let shard_manager = run_shard_manager(shard_manager_config(args), join_set).await?;

    let worker_service = run_worker_service(
        worker_service_config(args, &shard_manager, &registry_service),
        join_set,
    )
    .await?;

    let worker_executor = {
        let config =
            worker_executor_config(args, &shard_manager, &registry_service, &worker_service);
        run_worker_executor(config, join_set).await?
    };

    Ok(StartedComponents {
        registry_service,
        shard_manager,
        worker_executor,
        worker_service,
        prometheus_registry: prometheus::default_registry().clone(),
    })
}

fn blob_storage_config(args: &LaunchArgs) -> BlobStorageConfig {
    BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
        root: args.data_dir.join("blobs"),
    })
}

fn registry_service_config(
    args: &LaunchArgs,
    component_compilation_service: &golem_component_compilation_service::RunDetails,
) -> RegistryServiceConfig {
    let plan_id = PlanId(uuid!("e808bd76-a6ab-4090-ade4-8447b8e8550f"));
    let plan_name = PlanName("default".to_string());

    RegistryServiceConfig {
        http_port: 0,
        grpc_port: 0,
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("registry.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 4,
            foreign_keys: true,
        }),
        login: LoginConfig::Disabled(Empty {}),
        cors_origin_regex: ".*".to_string(),
        component_compilation: golem_registry_service::config::ComponentCompilationConfig::Enabled(
            ComponentCompilationEnabledConfig {
                host: args.router_addr.clone(),
                port: component_compilation_service.grpc_port,
                retries: Default::default(),
                connect_timeout: Default::default(),
            },
        ),
        blob_storage: blob_storage_config(args),
        initial_plans: {
            let mut plans = HashMap::new();
            plans.insert(
                plan_name.0.clone(),
                PrecreatedPlan {
                    plan_id,
                    plan_name,
                    app_limit: i64::MAX,
                    env_limit: i64::MAX,
                    component_limit: i64::MAX,
                    worker_limit: i64::MAX,
                    worker_connection_limit: i64::MAX,
                    storage_limit: i64::MAX,
                    monthly_gas_limit: i64::MAX,
                    monthly_upload_limit: i64::MAX,
                    max_memory_per_worker: u64::MAX,
                },
            );
            plans
        },
        initial_accounts: {
            let mut accounts = HashMap::new();
            accounts.insert(
                "root".to_string(),
                PrecreatedAccount {
                    id: AccountId(uuid!("51de7d7d-f286-49aa-b79a-96022f7e2df9")),
                    name: "Initial User".to_string(),
                    email: "initial@user".to_string(),
                    token: TokenSecret::trusted(ADMIN_TOKEN.to_string()),
                    plan_id,
                    role: AccountRole::Admin,
                },
            );
            accounts
        },
        ..Default::default()
    }
}

fn shard_manager_config(args: &LaunchArgs) -> ShardManagerConfig {
    use golem_shard_manager::shard_manager_config::{
        FileSystemPersistenceConfig, HealthCheckConfig, PersistenceConfig,
    };

    ShardManagerConfig {
        grpc_port: 0,
        http_port: 0,
        persistence: PersistenceConfig::FileSystem(FileSystemPersistenceConfig {
            path: args.data_dir.join("sharding.bin"),
        }),
        health_check: HealthCheckConfig {
            silent: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn component_compilation_service_config(
    args: &LaunchArgs,
) -> golem_component_compilation_service::config::ServerConfig {
    golem_component_compilation_service::config::ServerConfig {
        registry_service:
            golem_component_compilation_service::config::RegistryServiceConfig::Dynamic(Empty {}),
        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        blob_storage: blob_storage_config(args),
        grpc_port: 0,
        http_port: 0,
        ..Default::default()
    }
}

fn worker_executor_config(
    args: &LaunchArgs,
    shard_manager_run_details: &golem_shard_manager::RunDetails,
    registry_service_run_details: &golem_registry_service::SingleExecutableRunDetails,
    worker_service_run_details: &golem_worker_service::TrafficReadyEndpoints,
) -> WorkerExecutorConfig {
    let mut config = WorkerExecutorConfig {
        port: 0,
        http_port: 0,
        key_value_storage:
        KeyValueStorageConfig::Sqlite(
            DbSqliteConfig {
                database: args
                    .data_dir
                    .join("kv-store.db")
                    .to_string_lossy()
                    .to_string(),
                max_connections: 4,
                foreign_keys: false,
            },
        ),
        indexed_storage:
        IndexedStorageConfig::KVStoreSqlite(
            IndexedStorageKVStoreSqliteConfig {},
        ),
        blob_storage: blob_storage_config(args),
        compiled_component_service: golem_service_base::service::compiled_component::CompiledComponentServiceConfig::Enabled(
            golem_service_base::service::compiled_component::CompiledComponentServiceEnabledConfig {},
        ),
        shard_manager_service: ShardManagerServiceConfig::Grpc(ShardManagerServiceGrpcConfig {
            host: args.router_addr.clone(),
            port: shard_manager_run_details.grpc_port,
            ..ShardManagerServiceGrpcConfig::default()
        }),
        registry_service: golem_service_base::clients::RegistryServiceConfig {
            host: args.router_addr.clone(),
            port: registry_service_run_details.grpc_port,
            ..Default::default()
        },
        resource_limits: ResourceLimitsConfig::Grpc(ResourceLimitsGrpcConfig {
            batch_update_interval: Duration::from_secs(60),
        }),
        agent_types_service: AgentTypesServiceConfig::Grpc(
            golem_worker_executor::services::golem_config::AgentTypesServiceGrpcConfig {
                ..Default::default()
            },
        ),
        public_worker_api: WorkerServiceGrpcConfig {
            host: args.router_addr.clone(),
            port: worker_service_run_details.grpc_port,
            retries: Default::default(),
            connect_timeout: Default::default(),
        },
        ..Default::default()
    };

    config.add_port_to_tracing_file_name_if_enabled();
    config
}

fn worker_service_config(
    args: &LaunchArgs,
    shard_manager_run_details: &golem_shard_manager::RunDetails,
    registry_service_run_details: &golem_registry_service::SingleExecutableRunDetails,
) -> WorkerServiceConfig {
    WorkerServiceConfig {
        port: 0,
        worker_grpc_port: 0,
        custom_request_port: args.custom_request_port,
        gateway_session_storage: golem_worker_service::config::GatewaySessionStorageConfig::Sqlite(
            DbSqliteConfig {
                database: args
                    .data_dir
                    .join("gateway-sessions.db")
                    .to_string_lossy()
                    .to_string(),
                max_connections: 4,
                foreign_keys: false,
            },
        ),
        blob_storage: blob_storage_config(args),
        routing_table: RoutingTableConfig {
            host: args.router_addr.clone(),
            port: shard_manager_run_details.grpc_port,
            ..RoutingTableConfig::default()
        },
        registry_service: golem_service_base::clients::RegistryServiceConfig {
            host: args.router_addr.clone(),
            port: registry_service_run_details.grpc_port,
            ..Default::default()
        },
        route_resolver: RouteResolverConfig {
            router_cache_max_capacity: 0,
            router_cache_ttl: Default::default(),
            router_cache_eviction_period: Default::default(),
        },
        ..Default::default()
    }
}

async fn run_shard_manager(
    config: ShardManagerConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_shard_manager::RunDetails, anyhow::Error> {
    let prometheus_registry = prometheus::default_registry().clone();
    let span = tracing::info_span!("shard-manager");
    golem_shard_manager::run(&config, prometheus_registry, join_set)
        .instrument(span)
        .await
}

async fn run_component_compilation_service(
    config: golem_component_compilation_service::config::ServerConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_component_compilation_service::RunDetails, anyhow::Error> {
    let prometheus_registry = golem_component_compilation_service::metrics::register_all();
    let span = tracing::info_span!("component-compilation-service");
    golem_component_compilation_service::run(config, prometheus_registry, join_set)
        .instrument(span)
        .await
}

async fn run_registry_service(
    config: RegistryServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_registry_service::SingleExecutableRunDetails, anyhow::Error> {
    let prometheus_registry = golem_registry_service::metrics::register_all();
    let span = tracing::info_span!("registry-service", component = "registry-service");
    RegistryService::new(config, prometheus_registry)
        .instrument(span.clone())
        .await?
        .start_for_single_executable(join_set)
        .instrument(span)
        .await
}

async fn run_worker_executor(
    config: WorkerExecutorConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_worker_executor::RunDetails, anyhow::Error> {
    let prometheus_registry = golem_worker_executor::metrics::register_all();

    let span = tracing::info_span!("worker-executor");
    golem_worker_executor::bootstrap::run(config, prometheus_registry, Handle::current(), join_set)
        .instrument(span)
        .await
}

async fn run_worker_service(
    config: WorkerServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_worker_service::TrafficReadyEndpoints, anyhow::Error> {
    let prometheus_registry = golem_worker_executor::metrics::register_all();
    let span = tracing::info_span!("worker-service");
    WorkerService::new(config, prometheus_registry)
        .instrument(span.clone())
        .await?
        .start_endpoints(join_set, None)
        .instrument(span)
        .await
}
