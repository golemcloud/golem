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

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use config::ServerConfig;
use golem_api_grpc::proto::golem::componentcompilation::v1::component_compilation_service_server::ComponentCompilationServiceServer;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor_base::services::golem_config::BlobStorageConfig;
use golem_worker_executor_base::storage::blob::s3::S3BlobStorage;
use golem_worker_executor_base::storage::blob::BlobStorage;
use golem_worker_executor_base::{
    http_server::HttpServerImpl, services::compiled_component, storage,
};
use grpc::CompileGrpcService;
use prometheus::Registry;
use service::CompilationService;
use tonic::codec::CompressionEncoding;
use tracing::info;
use wasmtime::component::__internal::anyhow::anyhow;

use crate::config::make_config_loader;
use crate::service::compile_service::ComponentCompilationServiceImpl;
use golem_worker_executor_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_worker_executor_base::storage::sqlite::SqlitePool;
use wasmtime::WasmBacktraceDetails;

mod config;
mod grpc;
mod metrics;
mod model;
mod service;

#[cfg(test)]
test_r::enable!();

pub fn server_main() -> Result<(), Box<dyn std::error::Error>> {
    match make_config_loader().load_or_dump_config() {
        Some(config) => {
            init_tracing_with_default_env_filter(&config.tracing);
            let prometheus = metrics::register_all();

            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(run(config, prometheus))
        }
        None => Ok(()),
    }
}

async fn run(config: ServerConfig, prometheus: Registry) -> Result<(), Box<dyn std::error::Error>> {
    let blob_storage: Arc<dyn BlobStorage + Send + Sync> = match &config.blob_storage {
        BlobStorageConfig::S3(config) => {
            info!("Using S3 for blob storage");
            Arc::new(S3BlobStorage::new(config.clone()).await)
        }
        BlobStorageConfig::LocalFileSystem(config) => {
            info!(
                "Using local file system for blob storage at {:?}",
                config.root
            );
            Arc::new(
                storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await
                    .expect("Failed to create file system blob storage"),
            )
        }
        BlobStorageConfig::InMemory => {
            info!("Using in-memory blob storage");
            Arc::new(storage::blob::memory::InMemoryBlobStorage::new())
        }
        BlobStorageConfig::KVStoreSqlite => {
            Err(anyhow!("KVStoreSqlite configuration option is not supported - use an explicit Sqlite configuration instead"))?
        }
        BlobStorageConfig::Sqlite(sqlite) => {
            info!("Using Sqlite for blob storage at {}", sqlite.database);
            let pool = SqlitePool::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            Arc::new(
                SqliteBlobStorage::new(pool.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
    };
    let compiled_component =
        compiled_component::configured(&config.compiled_component_service, blob_storage.clone());
    let engine = wasmtime::Engine::new(&create_wasmtime_config()).expect("Failed to create engine");

    // Start metrics and healthcheck server.
    let address = config.http_addr().expect("Invalid HTTP address");
    let http_server = HttpServerImpl::new(
        address,
        prometheus,
        "Component Compilation Service is running",
    );

    let compilation_service = ComponentCompilationServiceImpl::new(
        config.compile_worker,
        config.component_service,
        engine,
        compiled_component,
    );

    let compilation_service = Arc::new(compilation_service);

    let ipv4_address: Ipv4Addr = config.grpc_host.parse().expect("Invalid IP address");
    let address = SocketAddr::new(ipv4_address.into(), config.grpc_port);

    start_grpc_server(address, compilation_service).await?;

    info!("Server started on port {}", config.grpc_port);

    drop(http_server); // explicitly keeping it alive until the end

    Ok(())
}

async fn start_grpc_server(
    addr: SocketAddr,
    service: Arc<dyn CompilationService + Send + Sync>,
) -> Result<(), tonic::transport::Error> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

    health_reporter
        .set_serving::<ComponentCompilationServiceServer<CompileGrpcService>>()
        .await;

    tonic::transport::Server::builder()
        .add_service(health_service)
        .add_service(
            ComponentCompilationServiceServer::new(CompileGrpcService::new(service))
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await
}

fn create_wasmtime_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::default();

    config.wasm_multi_value(true);
    config.async_support(true);
    config.wasm_component_model(true);
    config.epoch_interruption(true);
    config.consume_fuel(true);
    config.wasm_backtrace_details(WasmBacktraceDetails::Enable);

    config
}

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
