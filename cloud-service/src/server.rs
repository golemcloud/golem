use cloud_service::api::make_open_api_service;
use cloud_service::config::{make_config_loader, CloudServiceConfig};
use cloud_service::service::Services;
use cloud_service::{api, grpcapi, metrics};
use golem_common::config::DbConfig;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_service_base::db;
use golem_service_base::migration::{Migrations, MigrationsDir};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::listener::TcpListener;
use poem::middleware::{CookieJarManager, Cors};
use poem::EndpointExt;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use tokio::select;
use tracing::error;

fn main() -> Result<(), std::io::Error> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(dump_openapi_yaml())
    } else if let Some(config) = make_config_loader().load_or_dump_config() {
        init_tracing_with_default_env_filter(&config.tracing);

        let prometheus = metrics::register_all();

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
            .build()?
            .block_on(async_main(&config, prometheus))
    } else {
        Ok(())
    }
}

async fn dump_openapi_yaml() -> Result<(), std::io::Error> {
    let config = CloudServiceConfig::default();
    let services = Services::new(&config).await.map_err(|e| {
        error!("Services - init error: {}", e);
        std::io::Error::other(e)
    })?;
    let open_api_service = make_open_api_service(&services);
    println!("{}", open_api_service.spec_yaml());
    Ok(())
}

async fn async_main(
    config: &CloudServiceConfig,
    prometheus_registry: Registry,
) -> Result<(), std::io::Error> {
    let grpc_port = config.grpc_port;
    let http_port = config.http_port;

    let migrations = MigrationsDir::new(Path::new("./db/migration").to_path_buf());
    match config.db.clone() {
        DbConfig::Postgres(c) => {
            db::postgres::migrate(&c, migrations.postgres_migrations())
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::other(format!("Init error: {e:?}"))
                })?;
        }
        DbConfig::Sqlite(c) => {
            db::sqlite::migrate(&c, migrations.sqlite_migrations())
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::other(format!("Init error: {e:?}"))
                })?;
        }
    };

    let services = Services::new(config).await.map_err(|e| {
        error!("Services - init error: {}", e);
        std::io::Error::other(e)
    })?;

    services
        .plan_service
        .create_initial_plan()
        .await
        .map_err(|e| {
            error!("Plan - init error: {}", e);
            std::io::Error::other("Plan Error")
        })?;

    services
        .login_service
        .create_initial_users()
        .await
        .map_err(|e| {
            error!("Login - init error: {}", e);
            std::io::Error::other("Login Error")
        })?;

    let http_services = services.clone();
    let grpc_services = services.clone();

    let cors = Cors::new()
        .allow_origin_regex(&config.cors_origin_regex)
        .allow_credentials(true);

    let http_server = tokio::spawn(async move {
        let prometheus_registry = Arc::new(prometheus_registry);
        let app = api::combined_routes(prometheus_registry, &http_services)
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
