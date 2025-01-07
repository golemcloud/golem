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
use crate::api::{make_open_api_service, ApiServices};
use crate::service::Services;
use anyhow::{anyhow, Context};
use golem_common::config::DbConfig;
use golem_service_base::db;
use golem_service_base::migration::Migrations;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use poem::listener::Acceptor;
use poem::listener::Listener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::EndpointExt;
use poem_openapi::OpenApiService;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::task::JoinSet;
use tracing::Instrument;

pub mod api;
pub mod config;
pub mod grpcapi;
pub mod service;

#[cfg(test)]
test_r::enable!();

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
    pub custom_request_port: u16,
}

#[derive(Clone)]
pub struct WorkerService {
    config: WorkerServiceBaseConfig,
    prometheus_registry: Registry,
    services: Services,
}

impl WorkerService {
    pub async fn new(
        config: WorkerServiceBaseConfig,
        prometheus_registry: Registry,
        migrations: impl Migrations,
    ) -> Result<Self, anyhow::Error> {
        match &config.db {
            DbConfig::Postgres(c) => {
                db::postgres_migrate(c, migrations.postgres_migrations())
                    .await
                    .context("Postgres DB migration")?;
            }
            DbConfig::Sqlite(c) => {
                db::sqlite_migrate(c, migrations.sqlite_migrations())
                    .await
                    .context("Sqlite DB migration")?;
            }
        };

        let services: Services = Services::new(&config)
            .await
            .map_err(|err| anyhow!(err).context("Service initialization"))?;

        Ok(Self {
            config,
            prometheus_registry,
            services,
        })
    }

    pub async fn run(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> anyhow::Result<RunDetails> {
        let grpc_port = self.start_grpc_server(join_set).await?;
        let http_port = self.start_http_server(join_set).await?;
        let custom_request_port = self.start_api_gateway_server(join_set).await?;

        Ok(RunDetails {
            http_port,
            grpc_port,
            custom_request_port,
        })
    }

    pub fn http_service(&self) -> OpenApiService<ApiServices, ()> {
        make_open_api_service(&self.services)
    }

    pub async fn start_grpc_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<u16, anyhow::Error> {
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), self.config.worker_grpc_port).into(),
            self.services.clone(),
            join_set,
        )
        .await
        .map_err(|err| anyhow!(err).context("gRPC server failed"))
    }

    async fn start_http_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<u16, anyhow::Error> {
        let prometheus_registry = self.prometheus_registry.clone();
        let app = api::combined_routes(prometheus_registry, &self.services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        let poem_listener =
            poem::listener::TcpListener::bind(format!("0.0.0.0:{}", self.config.port));
        let acceptor = poem_listener.into_acceptor().await?;
        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        join_set.spawn(
            async move {
                poem::Server::new_with_acceptor(acceptor)
                    .run(app)
                    .await
                    .map_err(|e| e.into())
            }
            .in_current_span(),
        );

        Ok(port)
    }

    pub async fn start_api_gateway_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<u16, anyhow::Error> {
        let route = api::custom_request_route(&self.services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        let poem_listener = poem::listener::TcpListener::bind(format!(
            "0.0.0.0:{}",
            self.config.custom_request_port
        ));
        let acceptor = poem_listener.into_acceptor().await?;
        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        join_set.spawn(
            async move {
                poem::Server::new_with_acceptor(acceptor)
                    .run(route)
                    .await
                    .map_err(|err| anyhow!(err).context("API Gateway server failed"))
            }
            .in_current_span(),
        );

        Ok(port)
    }
}
