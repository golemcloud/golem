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

use self::config::GrpcApiConfig;
use crate::config::RegistryServiceConfig;
use anyhow::anyhow;
use config::ServerConfig;
use futures::TryFutureExt;
use golem_api_grpc::proto::golem::componentcompilation::v1::component_compilation_service_server::ComponentCompilationServiceServer;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::service::compiled_component;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::s3::{
    ConfiguredS3BlobStorageFactory, S3BlobStorageFactory, S3StartupNamespace,
};
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::tls::server::GrpcServerTls;
use grpc::CompileGrpcService;
use prometheus::Registry;
use service::ComponentCompilationService;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic_tracing_opentelemetry::middleware;
use tonic_tracing_opentelemetry::middleware::filters;
use tracing::{Instrument, info};

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
    run_with_blob_storage_factory(
        config,
        prometheus,
        join_set,
        ConfiguredS3BlobStorageFactory::default(),
    )
    .await
}

async fn run_with_blob_storage_factory(
    config: ServerConfig,
    prometheus: Registry,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    blob_storage_factory: impl S3BlobStorageFactory,
) -> anyhow::Result<RunDetails> {
    config.blob_storage.validate()?;
    let blob_storage: Arc<dyn BlobStorage + Send + Sync> = match &config.blob_storage {
        BlobStorageConfig::S3(config) => {
            blob_storage_factory
                .create(config.clone(), S3StartupNamespace::CompilationCache)
                .await?
        }
        BlobStorageConfig::LocalFileSystem(config) => Arc::new(
            golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                .await
                .expect("Failed to create file system blob storage"),
        ),
        BlobStorageConfig::InMemory(_) => {
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
        }
        BlobStorageConfig::KVStoreSqlite(_) => Err(anyhow!(
            "KVStoreSqlite configuration option is not supported - use an explicit Sqlite configuration instead"
        ))?,
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

    let engine = wasmtime::Engine::new(&create_wasmtime_config(&config.engine))
        .expect("Failed to create engine");

    // Start metrics and healthcheck server.
    let http_port = golem_service_base::observability::start_health_and_metrics_server(
        config.http_addr(),
        prometheus,
        "Component Compilation Service is running",
        join_set,
    )
    .await?;

    let grpc_addr = config.grpc_addr();
    let compilation_service = ComponentCompilationService::new(
        config.compile_worker,
        config.registry_service.clone(),
        engine,
        compiled_component,
    )
    .await;

    let compilation_service = Arc::new(compilation_service);

    let grpc_port = start_grpc_server(
        grpc_addr,
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
    addr: SocketAddr,
    config: &GrpcApiConfig,
    service: Arc<ComponentCompilationService>,
    registry_service_config: RegistryServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<u16> {
    let (health_reporter, health_service) = tonic_health::server::health_reporter();

    let listener = golem_service_base::net::bind_tcp_listener(addr)?;

    let grpc_port = listener.local_addr()?.port();

    health_reporter
        .set_serving::<ComponentCompilationServiceServer<CompileGrpcService>>()
        .await;

    join_set.spawn({
        let tls = GrpcServerTls::from_config(&config.tls, "grpc server")?;
        let server = tonic::transport::Server::builder();

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
            .serve_with_incoming(GrpcServerTls::incoming(
                tls.as_ref(),
                TcpListenerStream::new(listener),
            ))
            .map_err(anyhow::Error::from)
            .in_current_span()
    });

    Ok(grpc_port)
}

fn create_wasmtime_config(engine_config: &config::EngineConfig) -> wasmtime::Config {
    golem_common::wasmtime_config::create_wasmtime_config(engine_config.enable_fs_cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_service_base::config::S3BlobStorageConfig;
    use golem_service_base::storage::blob::memory::InMemoryS3BlobStorageFactory;
    use test_r::test;

    #[test]
    #[test_r::timeout("5s")]
    async fn s3_startup_failure_stops_compilation_before_listeners() {
        let config = ServerConfig {
            blob_storage: BlobStorageConfig::S3(S3BlobStorageConfig {
                region: "us-east-1".to_string(),
                aws_endpoint_url: Some("http://s3.test".to_string()),
                compilation_cache_bucket: "compilation-startup-only".to_string(),
                object_prefix: "objects-not-the-bucket".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let failure = aws_sdk_s3::error::SdkError::dispatch_failure(
            aws_sdk_s3::error::ConnectorError::io(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "selected bucket access denied",
            ))),
        );
        let factory = InMemoryS3BlobStorageFactory::failed(failure);
        let mut tasks = JoinSet::new();
        let error = run_with_blob_storage_factory(config, Registry::new(), &mut tasks, factory)
            .await
            .err()
            .expect("S3 failure must stop service construction");
        assert_eq!(
            error.to_string(),
            "S3 startup check failed for bucket compilation-startup-only"
        );
        assert_eq!(
            error
                .chain()
                .find_map(|cause| cause.downcast_ref::<std::io::Error>())
                .unwrap()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            error.root_cause().to_string(),
            "selected bucket access denied"
        );
        assert!(tasks.is_empty(), "listeners must not start after S3 fails");
    }
}
