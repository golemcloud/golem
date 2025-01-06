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

use crate::health;
use crate::migration::IncludedMigrationsDir;
use crate::AllRunDetails;
use anyhow::Context;
use golem_common::config::DbConfig;
use golem_common::config::DbSqliteConfig;
use golem_common::model::Empty;
use golem_component_service::config::ComponentServiceConfig;
use golem_component_service::ComponentService;
use golem_component_service_base::config::{ComponentStoreConfig, ComponentStoreLocalConfig};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::config::LocalFileSystemBlobStorageConfig;
use golem_service_base::service::routing_table::RoutingTableConfig;
use golem_shard_manager::shard_manager_config::{
    FileSystemPersistenceConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_worker_executor_base::services::golem_config::CompiledComponentServiceConfig;
use golem_worker_executor_base::services::golem_config::ComponentServiceGrpcConfig;
use golem_worker_executor_base::services::golem_config::ShardManagerServiceConfig;
use golem_worker_executor_base::services::golem_config::ShardManagerServiceGrpcConfig;
use golem_worker_executor_base::services::golem_config::{
    GolemConfig, IndexedStorageConfig, KeyValueStorageConfig,
};
use golem_worker_service::WorkerService;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use include_dir::include_dir;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use prometheus::{default_registry, Registry};
use std::path::PathBuf;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::Instrument;

use crate::proxy;

pub struct LaunchArgs {
    pub router_host: String,
    pub router_port: u16,
    pub custom_request_port: u16,
    pub data_dir: PathBuf,
}

pub async fn launch_golem_services(args: &LaunchArgs) -> Result<(), anyhow::Error> {
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

    let mut join_set = JoinSet::new();

    tokio::fs::create_dir_all(&args.data_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create data directory at {}",
                args.data_dir.display()
            )
        })?;

    let shard_manager = run_shard_manager(shard_manager_config(args), &mut join_set).await?;
    let component_service =
        run_component_service(component_service_config(args), &mut join_set).await?;
    let worker_executor = run_worker_executor(
        worker_executor_config(args, &shard_manager, &component_service),
        &mut join_set,
    )
    .await?;
    let worker_service = run_worker_service(
        worker_service_config(args, &shard_manager, &component_service),
        &mut join_set,
    )
    .await?;

    let all_run_details = AllRunDetails {
        shard_manager,
        worker_executor,
        component_service,
        worker_service,
    };

    let healthcheck_port = health::start_healthcheck_server(
        all_run_details.healthcheck_ports(),
        prometheus::default_registry().clone(),
        &mut join_set,
    )
    .await?;

    // Don't drop the channel, it will cause the proxy to fail
    let _proxy_command_channel = proxy::start_proxy(
        &args.router_host,
        args.router_port,
        healthcheck_port,
        &all_run_details,
        &mut join_set,
    )?;

    while let Some(res) = join_set.join_next().await {
        res??;
    }

    Ok(())
}

fn blob_storage_config(args: &LaunchArgs) -> BlobStorageConfig {
    BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
        root: args.data_dir.join("blobs"),
    })
}

fn shard_manager_config(args: &LaunchArgs) -> ShardManagerConfig {
    ShardManagerConfig {
        grpc_port: 0,
        http_port: 0,
        persistence: PersistenceConfig::FileSystem(FileSystemPersistenceConfig {
            path: args.data_dir.join("sharding.bin"),
        }),
        ..Default::default()
    }
}

fn component_service_config(args: &LaunchArgs) -> ComponentServiceConfig {
    ComponentServiceConfig {
        http_port: 0,
        grpc_port: 0,
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("components.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 32,
        }),
        component_store: ComponentStoreConfig::Local(ComponentStoreLocalConfig {
            root_path: args
                .data_dir
                .join("components")
                .to_string_lossy()
                .to_string(),
            object_prefix: "".to_string(),
        }),
        blob_storage: blob_storage_config(args),
        compilation: golem_component_service_base::config::ComponentCompilationConfig::Disabled(
            Empty {},
        ),
        ..Default::default()
    }
}

fn worker_executor_config(
    args: &LaunchArgs,
    shard_manager_run_details: &golem_shard_manager::RunDetails,
    component_service_run_details: &golem_component_service::RunDetails,
) -> GolemConfig {
    let mut config = GolemConfig {
        port: 0,
        http_port: 0,
        key_value_storage: KeyValueStorageConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("kv-store.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 32,
        }),
        indexed_storage: IndexedStorageConfig::KVStoreSqlite,
        blob_storage: blob_storage_config(args),
        component_service: golem_worker_executor_base::services::golem_config::ComponentServiceConfig::Grpc(
            ComponentServiceGrpcConfig {
                host: args.router_host.clone(),
                port: component_service_run_details.grpc_port,
                ..ComponentServiceGrpcConfig::default()
            }
        ),
        compiled_component_service: CompiledComponentServiceConfig::Disabled(golem_worker_executor_base::services::golem_config::CompiledComponentServiceDisabledConfig {  }),
        shard_manager_service: ShardManagerServiceConfig::Grpc(ShardManagerServiceGrpcConfig {
            host: args.router_host.clone(),
            port: shard_manager_run_details.grpc_port,
            ..ShardManagerServiceGrpcConfig::default()
        }),
        ..Default::default()
    };

    config.add_port_to_tracing_file_name_if_enabled();
    config
}

fn worker_service_config(
    args: &LaunchArgs,
    shard_manager_run_details: &golem_shard_manager::RunDetails,
    component_service_run_details: &golem_component_service::RunDetails,
) -> WorkerServiceBaseConfig {
    WorkerServiceBaseConfig {
        port: 0,
        worker_grpc_port: 0,
        custom_request_port: args.custom_request_port,
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("workers.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 32,
        }),
        gateway_session_storage:
            golem_worker_service_base::app_config::GatewaySessionStorageConfig::Sqlite(
                DbSqliteConfig {
                    database: args
                        .data_dir
                        .join("gateway-sessions.db")
                        .to_string_lossy()
                        .to_string(),
                    max_connections: 32,
                },
            ),
        blob_storage: blob_storage_config(args),
        component_service: golem_worker_service_base::app_config::ComponentServiceConfig {
            host: args.router_host.clone(),
            port: component_service_run_details.grpc_port,
            ..golem_worker_service_base::app_config::ComponentServiceConfig::default()
        },
        routing_table: RoutingTableConfig {
            host: args.router_host.clone(),
            port: shard_manager_run_details.grpc_port,
            ..RoutingTableConfig::default()
        },
        ..Default::default()
    }
}

async fn run_shard_manager(
    config: ShardManagerConfig,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<golem_shard_manager::RunDetails, anyhow::Error> {
    let prometheus_registry = default_registry().clone();
    let span = tracing::info_span!("shard-manager");

    golem_shard_manager::run(&config, prometheus_registry, join_set)
        .instrument(span)
        .await
}

async fn run_component_service(
    config: ComponentServiceConfig,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<golem_component_service::RunDetails, anyhow::Error> {
    let prometheus_registry = golem_component_service::metrics::register_all();
    let migration_path = IncludedMigrationsDir::new(include_dir!(
        "$CARGO_MANIFEST_DIR/../golem-component-service/db/migration"
    ));

    let span = tracing::info_span!("component-service", component = "component-service");
    ComponentService::new(config, prometheus_registry, migration_path)
        .instrument(span.clone())
        .await?
        .run(join_set)
        .instrument(span)
        .await
}

async fn run_worker_executor(
    config: GolemConfig,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<golem_worker_executor_base::RunDetails, anyhow::Error> {
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();

    let span = tracing::info_span!("worker-executor");
    golem_worker_executor::run(config, prometheus_registry, Handle::current(), join_set)
        .instrument(span)
        .await
}

async fn run_worker_service(
    config: WorkerServiceBaseConfig,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<golem_worker_service::RunDetails, anyhow::Error> {
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();
    let migration_path = IncludedMigrationsDir::new(include_dir!(
        "$CARGO_MANIFEST_DIR/../golem-worker-service/db/migration"
    ));

    let span = tracing::info_span!("worker-service");

    WorkerService::new(config, prometheus_registry, migration_path)
        .instrument(span.clone())
        .await?
        .run(join_set)
        .instrument(span)
        .await
}
