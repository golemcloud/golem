use golem_service_base::config::DbConfig;
use golem_service_base::db;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use cloud_service::api::make_open_api_service;
use cloud_service::config::CloudServiceConfig;
use cloud_service::service::Services;
use cloud_service::{api, grpcapi, metrics};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::listener::TcpListener;
use poem::middleware::{CookieJarManager, Cors, OpenTelemetryMetrics, Tracing};
use poem::EndpointExt;
use prometheus::Registry;
use tokio::select;
use tracing::error;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), std::io::Error> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        let service = make_open_api_service(&Services::noop());
        println!("{}", service.spec_yaml());
        Ok(())
    } else {
        let prometheus = metrics::register_all();
        let config = CloudServiceConfig::new();

        if config.enable_tracing_console {
            // NOTE: also requires RUSTFLAGS="--cfg tokio_unstable" cargo build
            console_subscriber::init();
        } else if config.enable_json_log {
            tracing_subscriber::fmt()
                .json()
                .flatten_event(true)
                .with_span_events(FmtSpan::FULL) // NOTE: enable to see span events
                .with_env_filter(EnvFilter::from_default_env())
                .init();
        } else {
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::from_default_env())
                .with_ansi(true)
                .init();
        }

        let exporter = opentelemetry_prometheus::exporter()
            .with_registry(prometheus.clone())
            .build()
            .unwrap();

        global::set_meter_provider(
            MeterProviderBuilder::default()
                .with_reader(exporter)
                .build(),
        );

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async_main(&config, prometheus))
    }
}

async fn async_main(
    config: &CloudServiceConfig,
    prometheus_registry: Registry,
) -> Result<(), std::io::Error> {
    let grpc_port = config.grpc_port;
    let http_port = config.http_port;

    dbg!("Starting cloud server", http_port, grpc_port);

    match config.db.clone() {
        DbConfig::Postgres(c) => {
            db::postgres_migrate(&c, "./db/migration/postgres")
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, format!("Init error: {e:?}"))
                })?;
        }
        DbConfig::Sqlite(c) => {
            db::sqlite_migrate(&c, "./db/migration/sqlite")
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, format!("Init error: {e:?}"))
                })?;
        }
    };

    let services = Services::new(config).await.map_err(|e| {
        error!("Services - init error: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, e)
    })?;

    services
        .plan_service
        .create_initial_plan()
        .await
        .map_err(|e| {
            error!("Plan - init error: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Plan Error")
        })?;

    services
        .login_service
        .create_initial_users()
        .await
        .map_err(|e| {
            error!("Login - init error: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Login Error")
        })?;

    let http_services = services.clone();
    let grpc_services = services.clone();

    let cors = Cors::new()
        .allow_origin_regex("https://*.golem.cloud")
        .allow_credentials(true);

    let http_server = tokio::spawn(async move {
        let prometheus_registry = Arc::new(prometheus_registry);
        let app = api::combined_routes(prometheus_registry, &http_services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing)
            .with(CookieJarManager::new())
            .with(cors);

        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", http_port)))
            .run(app)
            .await
            .expect("HTTP server failed");
    });

    let grpc_server = tokio::spawn(async move {
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), grpc_port).into(),
            &grpc_services,
        )
        .await
        .expect("gRPC server failed");
    });

    select! {
        _ = http_server => {},
        _ = grpc_server => {},
    }

    Ok(())
}
