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

mod migration;
mod proxy;

use anyhow::anyhow;
use bytes::Bytes;
use futures::future::BoxFuture;
use golem_common::config::DbConfig;
use golem_common::tracing::init_tracing_with_default_debug_env_filter;
use golem_common::{
    config::DbSqliteConfig,
    tracing::{init_tracing_with_default_env_filter, TracingConfig},
};
use golem_component_service::config::ComponentServiceConfig;
use golem_component_service::ComponentService;
use golem_component_service_base::config::{ComponentStoreConfig, ComponentStoreLocalConfig};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::migration::Migrations;
use golem_shard_manager::shard_manager_config::{
    FileSystemPersistenceConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_worker_executor_base::services::golem_config::{
    GolemConfig, IndexedStorageConfig, KeyValueStorageConfig,
};
use golem_worker_service::WorkerService;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use include_dir::Dir;
use include_dir::include_dir;
use migration::{IncludedMigrationsDir};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::http::StatusCode;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::{Body, Endpoint, EndpointExt, IntoEndpoint, Request, Response};
use prometheus::{default_registry, Registry};
use regex::Regex;
use sqlx::error::BoxDynError;
use sqlx::migrate::{Migration, MigrationSource};
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinSet;

fn main() -> Result<(), anyhow::Error> {
    // TODO: root dir configuration for all sqlite / filesystem paths
    // TODO: serve command, otherwise delegate to CLI
    // TODO: verbose flag
    // TODO: start component compilation service
    // TODO: connect endpoint needs to be explicitly added to the combined Route
    let verbose: bool = true;

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    let tracing_config = tracing_config();
    if verbose {
        init_tracing_with_default_debug_env_filter(&tracing_config);
    } else {
        init_tracing_with_default_env_filter(&tracing_config);
    }

    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(Registry::default())
        .build()?;

    global::set_meter_provider(
        MeterProviderBuilder::default()
            .with_reader(exporter)
            .build(),
    );

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    runtime.block_on(run_all())
}

async fn run_all() -> Result<(), anyhow::Error> {
    let mut join_set = JoinSet::new();

    run_worker_executor(&mut join_set).await?;
    run_shard_manager(&mut join_set).await?;
    run_component_service(&mut join_set).await?;
    run_worker_service(&mut join_set).await?;

    // Don't drop the channel, it will cause the proxy to fail
    let _proxy_command_channel = proxy::start_proxy(&proxy::Ports {
        listener_port: 9882,
        component_service_port: 8083,
        worker_service_port: 9005,
    }, &mut join_set)?;

    while let Some(res) = join_set.join_next().await {
        let result = res?;
        result?;
    }

    Ok(())
}

fn tracing_config() -> TracingConfig {
    TracingConfig::test_pretty_without_time("golem")
}

const BLOB_STORAGE_DB: &str = "blob-storage.db";

fn worker_executor_config() -> GolemConfig {
    let mut config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::Sqlite(DbSqliteConfig {
            database: BLOB_STORAGE_DB.to_string(),
            max_connections: 32,
        }),
        indexed_storage: IndexedStorageConfig::KVStoreSqlite,
        blob_storage: BlobStorageConfig::KVStoreSqlite,
        ..Default::default()
    };

    config.add_port_to_tracing_file_name_if_enabled();
    config
}

fn shard_manager_config() -> ShardManagerConfig {
    ShardManagerConfig {
        persistence: PersistenceConfig::FileSystem(FileSystemPersistenceConfig {
            path: Path::new("sharding.bin").to_path_buf(),
        }),
        ..Default::default()
    }
}

fn component_service_config() -> ComponentServiceConfig {
    ComponentServiceConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: "components.db".to_string(),
            max_connections: 32,
        }),
        component_store: ComponentStoreConfig::Local(ComponentStoreLocalConfig {
            root_path: "components".to_string(),
            object_prefix: "".to_string(),
        }),
        blob_storage: BlobStorageConfig::Sqlite(DbSqliteConfig {
            database: BLOB_STORAGE_DB.to_string(),
            max_connections: 32,
        }),
        ..Default::default()
    }
}

fn worker_service_config() -> WorkerServiceBaseConfig {
    WorkerServiceBaseConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: "apis.db".to_string(),
            max_connections: 32,
        }),
        blob_storage: BlobStorageConfig::Sqlite(DbSqliteConfig {
            database: BLOB_STORAGE_DB.to_string(),
            max_connections: 32,
        }),
        ..Default::default()
    }
}

async fn run_worker_executor(
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let golem_config = worker_executor_config();
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();

    let _server = join_set.spawn(async move { golem_worker_executor::run(golem_config, prometheus_registry, Handle::current()).await });
    Ok(())
}

async fn run_shard_manager(
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = shard_manager_config();
    let prometheus_registry = default_registry().clone();
    let _server = join_set.spawn(async move { golem_shard_manager::async_main(&config, prometheus_registry).await });
    Ok(())
}

async fn run_component_service(
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = component_service_config();
    let prometheus_registry = golem_component_service::metrics::register_all();
    let migration_path = IncludedMigrationsDir::new(include_dir!("$CARGO_MANIFEST_DIR/../golem-component-service/db/migration"));

    let component_service = ComponentService::new(config, prometheus_registry, migration_path).await?;
    let _server = join_set.spawn(async move { component_service.run().await });
    Ok(())
}

async fn run_worker_service(
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = worker_service_config();
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();
    let migration_path = IncludedMigrationsDir::new(include_dir!("$CARGO_MANIFEST_DIR/../golem-worker-service/db/migration"));

    let worker_service = WorkerService::new(config, prometheus_registry, migration_path).await?;
    let _server = join_set.spawn(async move { worker_service.run().await });
    Ok(())
}
