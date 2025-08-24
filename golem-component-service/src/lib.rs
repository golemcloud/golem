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
pub mod authed;
pub mod bootstrap;
pub mod config;
pub mod error;
pub mod grpcapi;
pub mod metrics;
pub mod model;
pub mod repo;
pub mod service;

use crate::api::Apis;
use crate::bootstrap::Services;
use crate::config::ComponentServiceConfig;
use anyhow::{anyhow, Context};
use golem_common::config::DbConfig;
use golem_service_base::db;
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::{include_dir, Dir};
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::listener::Acceptor;
use poem::listener::Listener;
use poem::middleware::{CookieJarManager, Cors, Tracing};
use poem::{EndpointExt, Route};
use poem_openapi::OpenApiService;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::task::JoinSet;
use tracing::{debug, info, Instrument};

#[cfg(test)]
test_r::enable!();

static DB_MIGRATIONS: Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

pub struct RunDetails {
    pub grpc_port: u16,
    pub http_port: u16,
}

pub struct TrafficReadyEndpoints {
    pub grpc_port: u16,
    pub endpoint: BoxEndpoint<'static>,
}

#[derive(Clone)]
pub struct ComponentService {
    config: ComponentServiceConfig,
    prometheus_registry: Registry,
    services: Services,
}

impl ComponentService {
    pub async fn new(
        config: ComponentServiceConfig,
        prometheus_registry: Registry,
    ) -> Result<Self, anyhow::Error> {
        debug!("Initializing component service");

        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);

        match config.db.clone() {
            DbConfig::Postgres(c) => {
                db::postgres::migrate(&c, migrations.postgres_migrations())
                    .await
                    .context("Postgres DB migration")?;
            }
            DbConfig::Sqlite(c) => {
                db::sqlite::migrate(&c, migrations.sqlite_migrations())
                    .await
                    .context("SQLite DB migration")?;
            }
        };

        let services = Services::new(&config)
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
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> Result<RunDetails, anyhow::Error> {
        let grpc_port = self.start_grpc_server(join_set).await?;
        let http_port = self.start_http_server(join_set).await?;

        info!("Started component service on ports: http: {http_port}, grpc: {grpc_port}");

        self.services
            .compilation_service
            .set_self_grpc_port(grpc_port);
        Ok(RunDetails {
            http_port,
            grpc_port,
        })
    }

    /// Endpoints are only valid until joinset is dropped
    pub async fn start_endpoints(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> Result<TrafficReadyEndpoints, anyhow::Error> {
        let grpc_port = self.start_grpc_server(join_set).await?;
        let endpoint = api::make_open_api_service(&self.services).boxed();
        self.services
            .compilation_service
            .set_self_grpc_port(grpc_port);
        Ok(TrafficReadyEndpoints {
            grpc_port,
            endpoint,
        })
    }

    pub fn http_service(&self) -> OpenApiService<Apis, ()> {
        api::make_open_api_service(&self.services)
    }

    async fn start_grpc_server(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> Result<u16, anyhow::Error> {
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), self.config.grpc_port).into(),
            &self.services,
            join_set,
        )
        .await
        .map_err(|err| anyhow!(err).context("gRPC server failed"))
    }

    async fn start_http_server(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> Result<u16, anyhow::Error> {
        let prometheus_registry = self.prometheus_registry.clone();

        let api_service = api::make_open_api_service(&self.services);

        let ui = api_service.swagger_ui();
        let spec = api_service.spec_endpoint_yaml();
        let metrics = PrometheusExporter::new(prometheus_registry.clone());

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
            .with(Tracing);

        let poem_listener =
            poem::listener::TcpListener::bind(format!("0.0.0.0:{}", self.config.http_port));
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
}
