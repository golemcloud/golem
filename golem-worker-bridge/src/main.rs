use std::net::{Ipv4Addr, SocketAddrV4};
use golem_worker_bridge::api;
use golem_worker_bridge::app_config::WorkerBridgeConfig;
use golem_worker_bridge::register::{RegisterApiDefinition};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProvider;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::{EndpointExt};
use std::sync::Arc;
use poem::listener::TcpListener;
use prometheus::Registry;
use golem_worker_bridge::service::Services;
use golem_worker_bridge::grpcapi;
use golem_worker_bridge::metrics;
use tokio::select;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let prometheus = metrics::register_all();

    let config = WorkerBridgeConfig::default();
    app(&config, prometheus).await
}

pub async fn app(config: &WorkerBridgeConfig, prometheus_registry: Registry) -> std::io::Result<()> {
    init_tracing_metrics();

    let services: Services = Services::new(config).await?;

    let http_services = services.clone();

    let worker_server = tokio::spawn(async move {
        let prometheus_registry = Arc::new(prometheus_registry);
        let app = api::combined_routes(prometheus_registry, &http_services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", config.port)))
            .run(app)
            .await
            .expect("HTTP server failed");
    });

    let custom_request_server = {
        let route = api::custom_request_route(http_services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(poem::listener::TcpListener::bind(("0.0.0.0", config.custom_request_port)))
            .name("gateway")
            .run(route)
            .await
            .expect("Custom Request server failed")
    };

    let grpc_services = services.clone();

    let grpc_server = tokio::spawn(async move {
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), config.worker_grpc_port).into(),
            &grpc_services,
        )
            .await
            .expect("gRPC server failed");
    });

    select! {
        _ = worker_server => {},
        _ = custom_request_server => {},
        _ = grpc_server => {},

    Ok(())
}

fn init_tracing_metrics() {
    let prometheus = prometheus::default_registry();
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(prometheus.clone())
        .build()
        .unwrap();

    global::set_meter_provider(MeterProvider::builder().with_reader(exporter).build());

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(true)
        .init();
}
