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
use cloud_service::config::CloudServiceConfig;
use cloud_service::CloudService;
use golem_common::config::DbConfig;
use golem_common::config::DbSqliteConfig;
use golem_common::model::RetryConfig;
use golem_component_service::config::ComponentServiceConfig;
use golem_component_service::ComponentService;
use golem_service_base::clients::RemoteServiceConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::config::LocalFileSystemBlobStorageConfig;
use golem_service_base::service::routing_table::RoutingTableConfig;
use golem_shard_manager::shard_manager_config::ShardManagerConfig;
use golem_worker_executor::services::golem_config::{
    GolemConfig as WorkerExecutorConfig, ProjectServiceConfig, ProjectServiceGrpcConfig,
};
use golem_worker_service::config::WorkerServiceConfig;
use golem_worker_service::WorkerService;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use prometheus::Registry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::Instrument;
use uuid::{uuid, Uuid};

const ADMIN_TOKEN: Uuid = golem_cli::config::LOCAL_WELL_KNOWN_TOKEN;

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

    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(Registry::default())
        .build()?;

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
    let cloud_service = run_cloud_service(cloud_service_config(args), join_set).await?;

    let shard_manager = run_shard_manager(shard_manager_config(args), join_set).await?;

    let component_compilation_service =
        run_component_compilation_service(component_compilation_service_config(args), join_set)
            .await?;
    let component_service = run_component_service(
        component_service_config(args, &component_compilation_service, &cloud_service),
        join_set,
    )
    .await?;
    let worker_executor = {
        let config =
            worker_executor_config(args, &shard_manager, &component_service, &cloud_service);
        run_worker_executor(config, join_set).await?
    };
    let worker_service = run_worker_service(
        worker_service_config(args, &shard_manager, &component_service, &cloud_service),
        join_set,
    )
    .await?;

    Ok(StartedComponents {
        cloud_service,
        shard_manager,
        worker_executor,
        component_service,
        worker_service,
        prometheus_registry: prometheus::default_registry().clone(),
    })
}

fn blob_storage_config(args: &LaunchArgs) -> BlobStorageConfig {
    BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
        root: args.data_dir.join("blobs"),
    })
}

fn cloud_service_config(args: &LaunchArgs) -> CloudServiceConfig {
    use cloud_service::config::{AccountConfig, AccountsConfig};
    use golem_common::model::auth::Role;

    let mut accounts = HashMap::new();
    {
        let root_account = AccountConfig {
            id: uuid!("51de7d7d-f286-49aa-b79a-96022f7e2df9").to_string(),
            name: "Initial User".to_string(),
            email: "initial@user".to_string(),
            token: ADMIN_TOKEN,
            role: Role::Admin,
        };
        accounts.insert(root_account.id.clone(), root_account);
    }

    CloudServiceConfig {
        grpc_port: 0,
        http_port: 0,
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: args.data_dir.join("cloud.db").to_string_lossy().to_string(),
            max_connections: 4,
        }),
        accounts: AccountsConfig { accounts },
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
    use golem_component_compilation_service::config::DynamicComponentServiceConfig;
    use golem_worker_executor::services::golem_config::{
        CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig,
    };

    golem_component_compilation_service::config::ServerConfig {
        component_service:
            golem_component_compilation_service::config::ComponentServiceConfig::Dynamic(
                DynamicComponentServiceConfig {
                    access_token: ADMIN_TOKEN,
                },
            ),
        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        blob_storage: blob_storage_config(args),
        grpc_port: 0,
        http_port: 0,
        ..Default::default()
    }
}

fn component_service_config(
    args: &LaunchArgs,
    component_compilation_service: &golem_component_compilation_service::RunDetails,
    cloud_service: &cloud_service::TrafficReadyEndpoints,
) -> golem_component_service::config::ComponentServiceConfig {
    use golem_component_service::config::ComponentCompilationEnabledConfig;

    ComponentServiceConfig {
        http_port: 0,
        grpc_port: 0,
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("components.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 4,
        }),
        blob_storage: blob_storage_config(args),
        compilation: golem_component_service::config::ComponentCompilationConfig::Enabled(
            ComponentCompilationEnabledConfig {
                host: args.router_addr.clone(),
                port: component_compilation_service.grpc_port,
                retries: Default::default(),
                connect_timeout: Default::default(),
            },
        ),
        cloud_service: golem_service_base::clients::RemoteServiceConfig {
            host: args.router_addr.clone(),
            port: cloud_service.grpc_port,
            access_token: ADMIN_TOKEN,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn worker_executor_config(
    args: &LaunchArgs,
    shard_manager_run_details: &golem_shard_manager::RunDetails,
    component_service_run_details: &golem_component_service::TrafficReadyEndpoints,
    cloud_service_run_details: &cloud_service::TrafficReadyEndpoints,
) -> WorkerExecutorConfig {
    use golem_worker_executor::services::golem_config::CompiledComponentServiceEnabledConfig;
    use golem_worker_executor::services::golem_config::ComponentServiceConfig;
    use golem_worker_executor::services::golem_config::{
        CompiledComponentServiceConfig, IndexedStorageKVStoreSqliteConfig,
    };
    use golem_worker_executor::services::golem_config::{
        ComponentServiceGrpcConfig, ResourceLimitsConfig, ResourceLimitsGrpcConfig,
        ShardManagerServiceConfig, ShardManagerServiceGrpcConfig,
    };
    use golem_worker_executor::services::golem_config::{
        IndexedStorageConfig, KeyValueStorageConfig,
    };
    use golem_worker_executor::services::golem_config::{
        PluginServiceConfig, PluginServiceGrpcConfig,
    };

    let mut config = WorkerExecutorConfig {
        port: 0,
        http_port: 0,
        key_value_storage: KeyValueStorageConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("kv-store.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 4,
        }),
        indexed_storage: IndexedStorageConfig::KVStoreSqlite(IndexedStorageKVStoreSqliteConfig {}),
        blob_storage: blob_storage_config(args),
        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        shard_manager_service: ShardManagerServiceConfig::Grpc(ShardManagerServiceGrpcConfig {
            host: args.router_addr.clone(),
            port: shard_manager_run_details.grpc_port,
            ..ShardManagerServiceGrpcConfig::default()
        }),
        plugin_service: PluginServiceConfig::Grpc(PluginServiceGrpcConfig {
            host: args.router_addr.clone(),
            port: component_service_run_details.grpc_port,
            access_token: ADMIN_TOKEN.to_string(),
            ..Default::default()
        }),
        component_service: ComponentServiceConfig::Grpc(ComponentServiceGrpcConfig {
            host: args.router_addr.clone(),
            port: component_service_run_details.grpc_port,
            access_token: ADMIN_TOKEN.to_string(),
            ..ComponentServiceGrpcConfig::default()
        }),
        project_service: ProjectServiceConfig::Grpc(ProjectServiceGrpcConfig {
            host: args.router_addr.clone(),
            port: cloud_service_run_details.grpc_port,
            access_token: ADMIN_TOKEN.to_string(),
            retries: RetryConfig::default(),
            ..ProjectServiceGrpcConfig::default()
        }),
        resource_limits: ResourceLimitsConfig::Grpc(ResourceLimitsGrpcConfig {
            host: args.router_addr.clone(),
            port: cloud_service_run_details.grpc_port,
            access_token: ADMIN_TOKEN.to_string(),
            batch_update_interval: Duration::from_secs(60),
            retries: RetryConfig::default(),
        }),
        ..Default::default()
    };

    config.add_port_to_tracing_file_name_if_enabled();
    config
}

fn worker_service_config(
    args: &LaunchArgs,
    shard_manager_run_details: &golem_shard_manager::RunDetails,
    component_service_run_details: &golem_component_service::TrafficReadyEndpoints,
    cloud_service_run_details: &cloud_service::TrafficReadyEndpoints,
) -> WorkerServiceConfig {
    WorkerServiceConfig {
        port: 0,
        worker_grpc_port: 0,
        custom_request_port: args.custom_request_port,
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("workers.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 4,
        }),
        gateway_session_storage: golem_worker_service::config::GatewaySessionStorageConfig::Sqlite(
            DbSqliteConfig {
                database: args
                    .data_dir
                    .join("gateway-sessions.db")
                    .to_string_lossy()
                    .to_string(),
                max_connections: 4,
            },
        ),
        blob_storage: blob_storage_config(args),
        component_service: golem_worker_service::config::ComponentServiceConfig {
            host: args.router_addr.clone(),
            port: component_service_run_details.grpc_port,
            access_token: ADMIN_TOKEN,
            ..golem_worker_service::config::ComponentServiceConfig::default()
        },
        routing_table: RoutingTableConfig {
            host: args.router_addr.clone(),
            port: shard_manager_run_details.grpc_port,
            ..RoutingTableConfig::default()
        },
        cloud_service: RemoteServiceConfig {
            host: args.router_addr.clone(),
            port: cloud_service_run_details.grpc_port,
            access_token: ADMIN_TOKEN,
            ..RemoteServiceConfig::default()
        },
        ..Default::default()
    }
}

async fn run_cloud_service(
    config: CloudServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<cloud_service::TrafficReadyEndpoints, anyhow::Error> {
    let prometheus_registry = golem_component_service::metrics::register_all();
    let span = tracing::info_span!("cloud-service", component = "cloud-service");
    CloudService::new(config, prometheus_registry)
        .instrument(span.clone())
        .await?
        .start_endpoints(join_set)
        .instrument(span)
        .await
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

async fn run_component_service(
    config: ComponentServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_component_service::TrafficReadyEndpoints, anyhow::Error> {
    let prometheus_registry = golem_component_service::metrics::register_all();
    let span = tracing::info_span!("component-service", component = "component-service");
    ComponentService::new(config, prometheus_registry)
        .instrument(span.clone())
        .await?
        .start_endpoints(join_set)
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
        .start_endpoints(join_set)
        .instrument(span)
        .await
}
