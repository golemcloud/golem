// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

pub mod api;
pub mod aws_config;
pub mod aws_load_balancer;
pub mod config;
pub mod gateway_api_definition;
pub mod gateway_api_definition_transformer;
pub mod gateway_api_deployment;
pub mod gateway_binding;
pub mod gateway_execution;
pub mod gateway_middleware;
pub mod gateway_request;
pub mod gateway_rib_compiler;
pub mod gateway_rib_interpreter;
pub mod gateway_security;
pub mod getter;
pub mod grpcapi;
pub mod headers;
pub mod http_invocation_context;
pub mod metrics;
pub mod model;
pub mod path;
pub mod repo;
pub mod service;

use crate::config::WorkerServiceConfig;
use crate::service::Services;
use anyhow::{anyhow, Context};
use golem_common::config::DbConfig;
use golem_service_base::db;
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::{include_dir, Dir};
use opentelemetry_sdk::trace::SdkTracer;
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::listener::Acceptor;
use poem::listener::Listener;
use poem::middleware::{CookieJarManager, Cors, OpenTelemetryMetrics, OpenTelemetryTracing};
use poem::{EndpointExt, Route};
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::task::JoinSet;
use tracing::{info, Instrument};

#[cfg(test)]
test_r::enable!();

static DB_MIGRATIONS: Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
    pub custom_request_port: u16,
}

pub struct TrafficReadyEndpoints {
    pub grpc_port: u16,
    pub custom_request_port: u16,
    pub api_endpoint: BoxEndpoint<'static>,
}

#[derive(Clone)]
pub struct WorkerService {
    config: WorkerServiceConfig,
    prometheus_registry: Registry,
    services: Services,
}

impl WorkerService {
    pub async fn new(
        config: WorkerServiceConfig,
        prometheus_registry: Registry,
    ) -> anyhow::Result<Self> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);

        match &config.db {
            DbConfig::Postgres(c) => {
                db::postgres::migrate(c, migrations.postgres_migrations())
                    .await
                    .context("Postgres DB migration")?;
            }
            DbConfig::Sqlite(c) => {
                db::sqlite::migrate(c, migrations.sqlite_migrations())
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
        tracer: Option<SdkTracer>,
    ) -> anyhow::Result<RunDetails> {
        let grpc_port = self.start_grpc_server(join_set).await?;
        let http_port = self.start_http_server(join_set, tracer.clone()).await?;
        let custom_request_port = self.start_api_gateway_server(join_set, tracer).await?;

        info!(
            "Started worker service on ports: http: {}, grpc: {}, gateway: {}",
            http_port, grpc_port, custom_request_port
        );

        Ok(RunDetails {
            http_port,
            grpc_port,
            custom_request_port,
        })
    }

    /// Endpoints are only valid until joinset is dropped
    pub async fn start_endpoints(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
        tracer: Option<SdkTracer>,
    ) -> Result<TrafficReadyEndpoints, anyhow::Error> {
        let grpc_port = self.start_grpc_server(join_set).await?;
        let custom_request_port = self.start_api_gateway_server(join_set, tracer).await?;
        let api_endpoint = api::make_open_api_service(&self.services).boxed();
        Ok(TrafficReadyEndpoints {
            grpc_port,
            api_endpoint,
            custom_request_port,
        })
    }

    async fn start_grpc_server(
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
        tracer: Option<SdkTracer>,
    ) -> Result<u16, anyhow::Error> {
        let api_service = api::make_open_api_service(&self.services);

        let ui = api_service.swagger_ui();
        let spec = api_service.spec_endpoint_yaml();
        let metrics = PrometheusExporter::new(self.prometheus_registry.clone());

        let cors = Cors::new()
            .allow_origin_regex(&self.config.cors_origin_regex)
            .allow_credentials(true);

        let app = Route::new()
            .nest("/", api_service)
            .nest("/docs", ui)
            .nest("/specs", spec)
            .nest("/metrics", metrics)
            .with(CookieJarManager::new())
            .with(cors)
            .with_if(tracer.is_some(), OpenTelemetryTracing::new(tracer.unwrap()));

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

    async fn start_api_gateway_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        tracer: Option<SdkTracer>,
    ) -> Result<u16, anyhow::Error> {
        let route = Route::new()
            .nest("/", api::custom_http_request_api(&self.services))
            .with(OpenTelemetryMetrics::new())
            .with_if(tracer.is_some(), OpenTelemetryTracing::new(tracer.unwrap()));

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
