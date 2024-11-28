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

use crate::migration::IncludedMigrationsDir;
use anyhow::Context;
use golem_common::config::DbConfig;
use golem_common::config::DbSqliteConfig;
use golem_common::model::Empty;
use golem_component_service::config::ComponentServiceConfig;
use golem_component_service::ComponentService;
use golem_component_service_base::config::{ComponentStoreConfig, ComponentStoreLocalConfig};
use golem_service_base::config::BlobStorageConfig;
use golem_shard_manager::shard_manager_config::{
    FileSystemPersistenceConfig, PersistenceConfig, ShardManagerConfig,
};
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
    pub port: u16,
    pub data_dir: PathBuf,
}

struct ServiceArgs {
    data_dir: PathBuf,
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

    let service_args = ServiceArgs {
        data_dir: args.data_dir.clone(),
    };

    tokio::fs::create_dir_all(&service_args.data_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create data directory at {}",
                service_args.data_dir.display()
            )
        })?;

    run_worker_executor(&service_args, &mut join_set).await?;
    run_shard_manager(&service_args, &mut join_set).await?;
    run_component_service(&service_args, &mut join_set).await?;
    run_worker_service(&service_args, &mut join_set).await?;

    // Don't drop the channel, it will cause the proxy to fail
    let _proxy_command_channel = proxy::start_proxy(
        &proxy::Ports {
            listener_port: args.port,
            component_service_port: 8083,
            worker_service_port: 9005,
        },
        &mut join_set,
    )?;

    while let Some(res) = join_set.join_next().await {
        let result = res?;
        result?;
    }

    Ok(())
}

fn blob_storage_config(args: &ServiceArgs) -> BlobStorageConfig {
    BlobStorageConfig::Sqlite(DbSqliteConfig {
        database: args
            .data_dir
            .join("blob-storage.db")
            .to_string_lossy()
            .to_string(),
        max_connections: 32,
    })
}

fn worker_executor_config(args: &ServiceArgs) -> GolemConfig {
    let mut config = GolemConfig {
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
        ..Default::default()
    };

    config.add_port_to_tracing_file_name_if_enabled();
    config
}

fn shard_manager_config(args: &ServiceArgs) -> ShardManagerConfig {
    ShardManagerConfig {
        persistence: PersistenceConfig::FileSystem(FileSystemPersistenceConfig {
            path: args.data_dir.join("sharding.bin"),
        }),
        ..Default::default()
    }
}

fn component_service_config(args: &ServiceArgs) -> ComponentServiceConfig {
    ComponentServiceConfig {
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

fn worker_service_config(args: &ServiceArgs) -> WorkerServiceBaseConfig {
    WorkerServiceBaseConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: args
                .data_dir
                .join("workers.db")
                .to_string_lossy()
                .to_string(),
            max_connections: 32,
        }),
        blob_storage: blob_storage_config(args),
        ..Default::default()
    }
}

async fn run_worker_executor(
    args: &ServiceArgs,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let golem_config = worker_executor_config(args);
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();

    let span = tracing::info_span!("worker-executor");
    let _server = join_set.spawn(async move {
        golem_worker_executor::run(golem_config, prometheus_registry, Handle::current())
            .instrument(span)
            .await
    });
    Ok(())
}

async fn run_shard_manager(
    args: &ServiceArgs,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = shard_manager_config(args);
    let prometheus_registry = default_registry().clone();
    let span = tracing::info_span!("shard-manager");
    let _server = join_set.spawn(async move {
        golem_shard_manager::async_main(&config, prometheus_registry)
            .instrument(span)
            .await
    });
    Ok(())
}

async fn run_component_service(
    args: &ServiceArgs,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = component_service_config(args);
    let prometheus_registry = golem_component_service::metrics::register_all();
    let migration_path = IncludedMigrationsDir::new(include_dir!(
        "$CARGO_MANIFEST_DIR/../golem-component-service/db/migration"
    ));

    let component_service =
        ComponentService::new(config, prometheus_registry, migration_path).await?;
    let span = tracing::info_span!("component-service", component = "component-service");
    let _server = join_set.spawn(async move { component_service.run().instrument(span).await });
    Ok(())
}

async fn run_worker_service(
    args: &ServiceArgs,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = worker_service_config(args);
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();
    let migration_path = IncludedMigrationsDir::new(include_dir!(
        "$CARGO_MANIFEST_DIR/../golem-worker-service/db/migration"
    ));

    let worker_service = WorkerService::new(config, prometheus_registry, migration_path).await?;
    let span = tracing::info_span!("worker-service");
    let _server = join_set.spawn(async move { worker_service.run().instrument(span).await });
    Ok(())
}
