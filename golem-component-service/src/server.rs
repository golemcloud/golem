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

use golem_component_service::api::make_open_api_service;
use golem_component_service::config::ComponentServiceConfig;
use golem_component_service::service::Services;
use golem_component_service::{api, grpcapi, metrics};
use golem_service_base::config::DbConfig;
use golem_service_base::db;
use opentelemetry::global;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::EndpointExt;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use tokio::select;
use tracing::{error, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), std::io::Error> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        let service = make_open_api_service(&Services::noop());
        println!("{}", service.spec_yaml());
        Ok(())
    } else {
        let prometheus = metrics::register_all();
        let config = ComponentServiceConfig::new();

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
            opentelemetry_sdk::metrics::MeterProviderBuilder::default()
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
    config: &ComponentServiceConfig,
    prometheus_registry: Registry,
) -> Result<(), std::io::Error> {
    let grpc_port = config.grpc_port;
    let http_port = config.http_port;

    info!(
        "Starting cloud server on ports: http: {}, grpc: {}",
        http_port, grpc_port
    );

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

    let services = Services::new(config).await.map_err(|e| {
        error!("Services - init error: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, e)
    })?;

    let http_services = services.clone();
    let grpc_services = services.clone();

    let http_server = tokio::spawn(async move {
        let prometheus_registry = Arc::new(prometheus_registry);
        let app = api::combined_routes(prometheus_registry, &http_services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

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
