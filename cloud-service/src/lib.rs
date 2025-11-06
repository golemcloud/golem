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
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod grpcapi;
pub mod login;
pub mod metrics;
pub mod model;
pub mod repo;
pub mod service;

use self::config::{AccountConfig, AccountsConfig};
use self::service::account::{AccountError, AccountService};
use self::service::account_grant::AccountGrantService;
use self::service::token::{TokenService, TokenServiceError};
use crate::api::Apis;
use crate::bootstrap::Services;
use crate::config::CloudServiceConfig;
use crate::model::AccountData;
use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use golem_common::config::DbConfig;
use golem_common::model::auth::TokenSecret;
use golem_common::model::AccountId;
use golem_common::poem::LazyEndpointExt;
use golem_service_base::db;
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::{include_dir, Dir};
use opentelemetry_sdk::trace::SdkTracer;
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::listener::Acceptor;
use poem::listener::Listener;
use poem::middleware::{CookieJarManager, Cors, OpenTelemetryTracing};
use poem::{EndpointExt, Route};
use poem_openapi::OpenApiService;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::str::FromStr;
use std::sync::Arc;
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
pub struct CloudService {
    config: CloudServiceConfig,
    prometheus_registry: Registry,
    services: Services,
}

impl CloudService {
    pub async fn new(
        config: CloudServiceConfig,
        prometheus_registry: Registry,
    ) -> Result<Self, anyhow::Error> {
        debug!("Initializing cloud service");

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

        services
            .plan_service
            .create_initial_plan()
            .await
            .context("initial plan creation")?;

        create_all_initial_accounts(
            &config.accounts,
            &services.account_service,
            &services.account_grant_service,
            &services.token_service,
        )
        .await?;

        Ok(Self {
            config,
            prometheus_registry,
            services,
        })
    }

    pub async fn run(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
        tracer: Option<SdkTracer>,
    ) -> Result<RunDetails, anyhow::Error> {
        let grpc_port = self.start_grpc_server(join_set).await?;
        let http_port = self.start_http_server(join_set, tracer).await?;

        info!(
            "Started cloud service on ports: http: {}, grpc: {}",
            http_port, grpc_port
        );

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
        tracer: Option<SdkTracer>,
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
            .with_if_lazy(tracer.is_some(), || {
                OpenTelemetryTracing::new(tracer.unwrap())
            });

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

async fn create_all_initial_accounts(
    accounts_config: &AccountsConfig,
    account_service: &Arc<dyn AccountService>,
    grant_service: &Arc<dyn AccountGrantService>,
    token_service: &Arc<dyn TokenService>,
) -> anyhow::Result<()> {
    for account_config in accounts_config.accounts.values() {
        create_initial_account(
            account_config,
            account_service,
            grant_service,
            token_service,
        )
        .await?
    }
    Ok(())
}

async fn create_initial_account(
    account_config: &AccountConfig,
    account_service: &Arc<dyn AccountService>,
    grant_service: &Arc<dyn AccountGrantService>,
    token_service: &Arc<dyn TokenService>,
) -> anyhow::Result<()> {
    info!(
        "Creating initial account({}, {}).",
        account_config.id, account_config.name
    );
    // This unwrap is infallible.
    let account_id = AccountId::from_str(&account_config.id).unwrap();

    // Check if the user exists. Trying to create it and catching the error leads to ugly Error level repo logs
    let user_exists = match account_service.get(&account_id).await {
        Ok(_) => true,
        Err(AccountError::AccountNotFound(_)) => false,
        Err(other) => Err(other)?,
    };

    if !user_exists {
        account_service
            .create(
                &account_id,
                &AccountData {
                    name: account_config.name.clone(),
                    email: account_config.email.clone(),
                },
            )
            .await
            .ok();
    }

    // idempotent / will not fail on already existing role grant
    grant_service.add(&account_id, &account_config.role).await?;

    token_service
        .create_known_secret(
            &account_id,
            &DateTime::<Utc>::MAX_UTC,
            &TokenSecret::new(account_config.token),
        )
        .await
        .or_else(|e| match e {
            TokenServiceError::InternalSecretAlreadyExists {
                existing_account_id,
                ..
            } if existing_account_id == account_id => Ok(()),
            _ => Err(e),
        })?;

    Ok(())
}
