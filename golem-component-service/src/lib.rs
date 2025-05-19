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

use crate::api::{make_open_api_service, ApiServices};
use crate::config::ComponentServiceConfig;
use crate::service::Services;
use anyhow::{anyhow, Context};
use golem_common::config::DbConfig;
use golem_common::golem_version;
use golem_service_base::db;
use golem_service_base::migration::Migrations;
use include_dir::{include_dir, Dir};
use poem::endpoint::BoxEndpoint;
use poem::listener::Acceptor;
use poem::listener::Listener;
use poem::{EndpointExt, IntoEndpoint};
use poem_openapi::OpenApiService;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::task::JoinSet;
use tracing::{info, Instrument};

pub mod api;
pub mod config;
pub mod grpcapi;
pub mod metrics;
pub mod service;

const VERSION: &str = golem_version!();

static DB_MIGRATIONS: Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

#[cfg(test)]
test_r::enable!();

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
        migrations: impl Migrations,
    ) -> Result<Self, anyhow::Error> {
        info!(
            "Starting cloud server on ports: http: {}, grpc: {}",
            config.http_port, config.grpc_port
        );

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
        let http_port = self.start_standalone_http_server(join_set).await?;
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
        let endpoint = self.main_endpoint();
        self.services
            .compilation_service
            .set_self_grpc_port(grpc_port);
        Ok(TrafficReadyEndpoints {
            grpc_port,
            endpoint,
        })
    }

    pub fn db_migrations() -> Dir<'static> {
        DB_MIGRATIONS.clone()
    }

    pub fn http_service(&self) -> OpenApiService<ApiServices, ()> {
        make_open_api_service(&self.services)
    }

    async fn start_grpc_server(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> Result<u16, anyhow::Error> {
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), self.config.grpc_port).into(),
            self.services.clone(),
            join_set,
        )
        .await
        .map_err(|err| anyhow!(err).context("gRPC server failed"))
    }

    fn main_endpoint(&self) -> BoxEndpoint<'static> {
        api::make_open_api_service(&self.services)
            .into_endpoint()
            .boxed()
    }

    async fn start_standalone_http_server(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> Result<u16, anyhow::Error> {
        let prometheus_registry = self.prometheus_registry.clone();

        let app = api::combined_routes(prometheus_registry, &self.services);

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
