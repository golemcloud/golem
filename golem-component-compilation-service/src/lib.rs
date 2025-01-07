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

use crate::service::compile_service::ComponentCompilationServiceImpl;
use config::ServerConfig;
use golem_api_grpc::proto::golem::componentcompilation::v1::component_compilation_service_server::ComponentCompilationServiceServer;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::storage::blob::s3::S3BlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::sqlite::SqlitePool;
use golem_worker_executor_base::services::compiled_component;
use grpc::CompileGrpcService;
use prometheus::Registry;
use service::CompilationService;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tracing::{info, Instrument};
use wasmtime::component::__internal::anyhow;
use wasmtime::component::__internal::anyhow::anyhow;
use wasmtime::WasmBacktraceDetails;

pub mod config;
mod grpc;
pub mod metrics;
mod model;
mod service;

#[cfg(test)]
test_r::enable!();

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
}

pub async fn run(
    config: ServerConfig,
    prometheus: Registry,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> anyhow::Result<RunDetails> {
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
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await
                    .expect("Failed to create file system blob storage"),
            )
        }
        BlobStorageConfig::InMemory => {
            info!("Using in-memory blob storage");
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
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
    let http_port = golem_service_base::observability::start_health_and_metrics_server(
        address,
        prometheus,
        "Component Compilation Service is running",
        join_set,
    )
    .await?;

    let compilation_service = ComponentCompilationServiceImpl::new(
        config.compile_worker,
        config.component_service,
        engine,
        compiled_component,
    );

    let compilation_service = Arc::new(compilation_service);

    let ipv4_address: Ipv4Addr = config.grpc_host.parse().expect("Invalid IP address");
    let address = SocketAddr::new(ipv4_address.into(), config.grpc_port);

    let grpc_port = start_grpc_server(address, compilation_service, join_set).await?;

    info!("Server started on port {}", config.grpc_port);

    Ok(RunDetails {
        http_port,
        grpc_port,
    })
}

async fn start_grpc_server(
    addr: SocketAddr,
    service: Arc<dyn CompilationService + Send + Sync>,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<u16> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

    let listener = TcpListener::bind(addr).await?;
    let grpc_port = listener.local_addr()?.port();

    health_reporter
        .set_serving::<ComponentCompilationServiceServer<CompileGrpcService>>()
        .await;

    join_set.spawn(
        async move {
            tonic::transport::Server::builder()
                .add_service(health_service)
                .add_service(
                    ComponentCompilationServiceServer::new(CompileGrpcService::new(service))
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip),
                )
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .map_err(|e| anyhow!(e).context("gRPC server failed"))
        }
        .in_current_span(),
    );

    Ok(grpc_port)
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
