use grpc::CompileGrpcService;
use prometheus::Registry;
use service::CompilationService;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use crate::service::compile_service::ComponentCompilationServiceImpl;
use config::ServerConfig;
use golem_api_grpc::proto::golem::componentcompilation::component_compilation_service_server::ComponentCompilationServiceServer;
use golem_worker_executor_base::{http_server::HttpServerImpl, services::compiled_component};
use tracing_subscriber::EnvFilter;

mod config;
mod grpc;
mod metrics;
mod model;
mod service;

fn main() {
    let prometheus = metrics::register_all();
    let config = crate::config::ServerConfig::new();

    if config.enable_tracing_console {
        // NOTE: also requires RUSTFLAGS="--cfg tokio_unstable" cargo build
        console_subscriber::init();
    } else if config.enable_json_log {
        tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            // .with_span_events(FmtSpan::FULL) // NOTE: enable to see span events
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_ansi(true)
            .init();
    }

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.block_on(run(config, prometheus))
}

async fn run(config: ServerConfig, prometheus: Registry) {
    let compiled_component =
        compiled_component::configured(&config.compiled_component_service).await;
    let engine = wasmtime::Engine::new(&create_wasmtime_config()).expect("Failed to create engine");

    // Start metrics and healthcheck server.
    let address = config.http_addr().expect("Invalid HTTP address");
    let http_server = HttpServerImpl::new(
        address,
        prometheus,
        "Component Compilation Service is running",
    );

    let compilation_service = ComponentCompilationServiceImpl::new(
        config.upload_worker,
        config.compile_worker,
        config.component_service,
        engine,
        compiled_component,
    );

    let compilation_service = Arc::new(compilation_service);

    let ipv4_address: Ipv4Addr = config.grpc_host.parse().expect("Invalid IP address");
    let address = SocketAddr::new(ipv4_address.into(), config.grpc_port);

    start_grpc_server(address, compilation_service)
        .await
        .expect("gRPC server failed");

    drop(http_server)
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
        .add_service(ComponentCompilationServiceServer::new(
            CompileGrpcService::new(service),
        ))
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
