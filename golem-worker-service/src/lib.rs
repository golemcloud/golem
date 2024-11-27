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
use crate::api::{make_open_api_service, ApiServices};
use crate::service::Services;
use anyhow::{anyhow, Context};
use golem_common::config::DbConfig;
use golem_service_base::db;
use golem_service_base::migration::Migrations;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::EndpointExt;
use poem_openapi::OpenApiService;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::task::JoinSet;

pub mod api;
pub mod config;
pub mod grpcapi;
pub mod service;

#[cfg(test)]
test_r::enable!();

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

    pub async fn run(&self) -> Result<(), anyhow::Error> {
        let mut join_set = JoinSet::new();

        let _grpc_server = join_set.spawn({
            let self_ = self.clone();
            async move { self_.start_grpc_server().await }
        });
        let _http_server = join_set.spawn({
            let self_ = self.clone();
            async move { self_.start_http_server().await }
        });
        let _api_gateway_server = join_set.spawn({
            let self_ = self.clone();
            async move { self_.start_api_gateway_server().await }
        });

        while let Some(res) = join_set.join_next().await {
            let result = res?;
            result?;
        }

        Ok(())
    }

    pub fn http_service(&self) -> OpenApiService<ApiServices, ()> {
        make_open_api_service(&self.services)
    }

    pub async fn start_grpc_server(&self) -> Result<(), anyhow::Error> {
        let grpc_services = self.services.clone();
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), self.config.worker_grpc_port).into(),
            &grpc_services,
        )
        .await
        .map_err(|err| anyhow!(err).context("gRPC server failed"))
    }

    async fn start_http_server(&self) -> Result<(), anyhow::Error> {
        let prometheus_registry = self.prometheus_registry.clone();
        let app = api::combined_routes(prometheus_registry, &self.services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", self.config.port)))
            .run(app)
            .await
            .map_err(|err| anyhow!(err).context("HTTP server failed"))
    }

    pub async fn start_api_gateway_server(&self) -> Result<(), anyhow::Error> {
        let route = api::custom_request_route(&self.services)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(TcpListener::bind((
            "0.0.0.0",
            self.config.custom_request_port,
        )))
        .name("gateway")
        .run(route)
        .await
        .map_err(|err| anyhow!(err).context("API Gateway server failed"))
    }
}
