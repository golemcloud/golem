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

use self::config::GrpcApiConfig;
use crate::config::RegistryServiceConfig;
use anyhow::anyhow;
use config::ServerConfig;
use futures::TryFutureExt;
use golem_api_grpc::proto::golem::componentcompilation::v1::component_compilation_service_server::ComponentCompilationServiceServer;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use golem_service_base::service::compiled_component;
use golem_service_base::storage::blob::s3::S3BlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use grpc::CompileGrpcService;
use prometheus::Registry;
use service::ComponentCompilationService;
use std::net::SocketAddrV4;
use std::{net::Ipv4Addr, sync::Arc};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic_tracing_opentelemetry::middleware;
use tonic_tracing_opentelemetry::middleware::filters;
use tracing::{info, Instrument};
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
            Arc::new(S3BlobStorage::new(config.clone()).await)
        }
        BlobStorageConfig::LocalFileSystem(config) => {
            Arc::new(
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await
                    .expect("Failed to create file system blob storage"),
            )
        }
        BlobStorageConfig::InMemory(_) => {
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
        }
        BlobStorageConfig::KVStoreSqlite(_) => {
            Err(anyhow!("KVStoreSqlite configuration option is not supported - use an explicit Sqlite configuration instead"))?
        }
        BlobStorageConfig::Sqlite(sqlite) => {
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

    let compilation_service = ComponentCompilationService::new(
        config.compile_worker,
        config.registry_service.clone(),
        engine,
        compiled_component,
    )
    .await;

    let compilation_service = Arc::new(compilation_service);

    let grpc_port = start_grpc_server(
        &config.grpc,
        compilation_service,
        config.registry_service,
        join_set,
    )
    .await?;

    info!("Started component service on ports: grpc: {grpc_port}");

    Ok(RunDetails {
        http_port,
        grpc_port,
    })
}

async fn start_grpc_server(
    config: &GrpcApiConfig,
    service: Arc<ComponentCompilationService>,
    registry_service_config: RegistryServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<u16> {
    let (health_reporter, health_service) = tonic_health::server::health_reporter();

    let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), config.port);
    let listener = TcpListener::bind(addr).await?;

    let grpc_port = listener.local_addr()?.port();

    health_reporter
        .set_serving::<ComponentCompilationServiceServer<CompileGrpcService>>()
        .await;

    join_set.spawn({
        let mut server = tonic::transport::Server::builder();

        if let GrpcServerTlsConfig::Enabled(tls) = &config.tls {
            server = server.tls_config(tls.to_tonic())?;
        };

        server
            .layer(middleware::server::OtelGrpcLayer::default().filter(filters::reject_healthcheck))
            .add_service(health_service)
            .add_service(
                ComponentCompilationServiceServer::new(CompileGrpcService::new(
                    service,
                    registry_service_config,
                ))
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Gzip),
            )
            .serve_with_incoming(TcpListenerStream::new(listener))
            .map_err(anyhow::Error::from)
            .in_current_span()
    });

    Ok(grpc_port)
}

fn create_wasmtime_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::default();

    config.wasm_multi_value(true);
    config.wasm_component_model(true);
    config.epoch_interruption(true);
    config.consume_fuel(true);
    config.wasm_backtrace_details(WasmBacktraceDetails::Enable);

    config
}
