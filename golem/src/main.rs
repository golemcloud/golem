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

use golem_common::config::DbConfig;
use golem_common::tracing::init_tracing_with_default_debug_env_filter;
use golem_common::{
    config::DbSqliteConfig,
    tracing::{init_tracing_with_default_env_filter, TracingConfig},
};
use golem_component_service::config::ComponentServiceConfig;
use golem_service_base::config::{
    BlobStorageConfig, ComponentStoreConfig, ComponentStoreLocalConfig,
};
use golem_shard_manager::shard_manager_config::{
    FileSystemPersistenceConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_worker_executor_base::services::golem_config::{
    GolemConfig, IndexedStorageConfig, KeyValueStorageConfig,
};
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use prometheus::{default_registry, Registry};
use std::path::Path;
use std::sync::Arc;
use tokio::{join, runtime::Runtime};

fn main() {
    // TODO: root dir configuration for all sqlite / filesystem paths
    // TODO: serve command, otherwise delegate to CLI
    // TODO: verbose flag
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
        .build()
        .unwrap();

    global::set_meter_provider(
        MeterProviderBuilder::default()
            .with_reader(exporter)
            .build(),
    );

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap(),
    );

    runtime.block_on(run_all(runtime.clone()))
}

async fn run_all(runtime: Arc<Runtime>) {
    let worker_executor = tokio::spawn({
        let runtime = runtime.clone();
        async move { run_worker_executor(runtime).await }
    });
    let shard_manager = tokio::spawn(async { run_shard_manager().await });
    let component_service = tokio::spawn(async { run_component_service().await });
    let worker_service = tokio::spawn(async { run_worker_service().await });

    let (
        worker_executor_result,
        shard_manager_result,
        component_service_result,
        worker_service_result,
    ) = join!(
        worker_executor,
        shard_manager,
        component_service,
        worker_service
    );

    let _ = worker_executor_result.expect("Worker executor failed");
    let _ = shard_manager_result.expect("Shard manager failed");
    let _ = component_service_result.expect("Component service failed");
    let _ = worker_service_result.expect("Worker service failed");
}

fn tracing_config() -> TracingConfig {
    TracingConfig::test_pretty_without_time("golem")
}

const BLOB_STORAGE_DB: &'static str = "blob-storage.db";

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

async fn run_worker_executor(runtime: Arc<Runtime>) {
    let golem_config = worker_executor_config();
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();

    golem_worker_executor::run(golem_config, prometheus_registry, runtime.handle().clone())
        .await
        .expect("Worker executor failed")
}

async fn run_shard_manager() {
    let config = shard_manager_config();
    let prometheus_registry = default_registry().clone();
    golem_shard_manager::async_main(&config, prometheus_registry)
        .await
        .expect("Shard manager failed")
}

async fn run_component_service() {
    let config = component_service_config();
    let prometheus_registry = golem_component_service::metrics::register_all();
    let migration_path = Path::new("/Users/vigoo/projects/golem/golem-component-service/db/migration"); // TODO: this needs to be embedded in the final binary

    golem_component_service::async_main(&config, prometheus_registry, migration_path)
        .await
        .expect("Component service failed")
}

async fn run_worker_service() {
    let config = worker_service_config();
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();
    let migration_path = Path::new("/Users/vigoo/projects/golem/golem-worker-service/db/migration"); // TODO: this needs to be embedded in the final binary

    golem_worker_service::app(&config, prometheus_registry, migration_path)
        .await
        .expect("Worker service failed")
}
