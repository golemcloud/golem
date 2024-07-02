use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::EndpointExt;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use prometheus::Registry;
use tokio::select;
use tracing::error;

use golem_service_base::config::DbConfig;
use golem_service_base::db;
use golem_worker_service::api;
use golem_worker_service::api::make_open_api_service;
use golem_worker_service::config::make_config_loader;
use golem_worker_service::grpcapi;
use golem_worker_service::service::Services;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use golem_worker_service_base::metrics;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        let services = Services::noop();
        let api_service = make_open_api_service(&services);
        println!("{}", api_service.spec_yaml());
        Ok(())
    } else if let Some(config) = make_config_loader().load_or_dump_config() {
        let prometheus = metrics::register_all();
        app(&config, prometheus).await
    } else {
        Ok(())
    }
}

pub async fn app(
    config: &WorkerServiceBaseConfig,
    prometheus_registry: Registry,
) -> std::io::Result<()> {
    let config = config.clone();

    init_tracing(&config, prometheus_registry.clone());

    match config.db.clone() {
        DbConfig::Postgres(c) => {
            db::postgres_migrate(&c, "./db/migration/postgres")
                .await
                .map_err(|e| {
                    dbg!("DB - init error: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, "Init error")
                })?;
        }
        DbConfig::Sqlite(c) => {
            db::sqlite_migrate(&c, "./db/migration/sqlite")
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, "Init error")
                })?;
        }
    };

    let services: Services = Services::new(&config)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let http_service1 = services.clone();
    let http_service2 = services.clone();
    let grpc_services = services.clone();

    let custom_request_server = tokio::spawn(async move {
        let route = api::custom_request_route(http_service1)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(poem::listener::TcpListener::bind((
            "0.0.0.0",
            config.custom_request_port,
        )))
        .name("gateway")
        .run(route)
        .await
        .expect("Custom Request server failed")
    });

    let worker_server = tokio::spawn(async move {
        let prometheus_registry = Arc::new(prometheus_registry);
        let app = api::combined_routes(prometheus_registry, &http_service2)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", config.port)))
            .run(app)
            .await
            .expect("HTTP server failed");
    });

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
    }
    Ok(())
}

fn init_tracing(config: &WorkerServiceBaseConfig, prometheus_registry: Registry) {
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(prometheus_registry.clone())
        .build()
        .unwrap();

    global::set_meter_provider(
        MeterProviderBuilder::default()
            .with_reader(exporter)
            .build(),
    );

    golem_common::tracing::init(
        &config.tracing,
        golem_common::tracing::filter::for_all_outputs::DEFAULT_ENV,
    );
}
