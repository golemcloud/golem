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

use crate::config::ComponentServiceConfig;
use crate::service::Services;
use golem_common::config::DbConfig;
use golem_common::golem_version;
use golem_service_base::db;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::EndpointExt;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use tokio::select;
use tracing::{error, info};

pub mod api;
pub mod config;
pub mod grpcapi;
pub mod metrics;
pub mod service;

const VERSION: &str = golem_version!();

#[cfg(test)]
test_r::enable!();

pub async fn async_main(
    config: &ComponentServiceConfig,
    prometheus_registry: Registry,
    db_migration_path: &Path
) -> Result<(), std::io::Error> {
    let grpc_port = config.grpc_port;
    let http_port = config.http_port;

    info!(
        "Starting cloud server on ports: http: {}, grpc: {}",
        http_port, grpc_port
    );

    match config.db.clone() {
        DbConfig::Postgres(c) => {
            db::postgres_migrate(&c, &db_migration_path.join("postgres"))
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, "Init error (Postgres DB migration)")
                })?;
        }
        DbConfig::Sqlite(c) => {
            db::sqlite_migrate(&c, &db_migration_path.join("sqlite"))
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, "Init error (Sqlite DB migration)")
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
