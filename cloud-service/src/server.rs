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

use chrono::{DateTime, Utc};
use cloud_service::api::make_open_api_service;
use cloud_service::auth::AccountAuthorisation;
use cloud_service::bootstrap::Services;
use cloud_service::config::{
    make_config_loader, AccountConfig, AccountsConfig, CloudServiceConfig,
};
use cloud_service::model::AccountData;
use cloud_service::service::account::AccountService;
use cloud_service::service::account_grant::AccountGrantService;
use cloud_service::service::token::TokenService;
use cloud_service::{api, grpcapi, metrics};
use golem_common::config::DbConfig;
use golem_common::model::auth::TokenSecret;
use golem_common::model::AccountId;
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
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tracing::error;
use tracing::info;

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

    create_all_initial_accounts(
        &config.accounts,
        &services.account_service,
        &services.account_grant_service,
        &services.token_service,
    )
    .await;

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

async fn create_all_initial_accounts(
    accounts_config: &AccountsConfig,
    account_service: &Arc<dyn AccountService>,
    grant_service: &Arc<dyn AccountGrantService>,
    token_service: &Arc<dyn TokenService>,
) {
    for account_config in accounts_config.accounts.values() {
        create_initial_account(
            account_config,
            account_service,
            grant_service,
            token_service,
        )
        .await
    }
}

async fn create_initial_account(
    account_config: &AccountConfig,
    account_service: &Arc<dyn AccountService>,
    grant_service: &Arc<dyn AccountGrantService>,
    token_service: &Arc<dyn TokenService>,
) {
    info!(
        "Creating initial account({}, {}).",
        account_config.id, account_config.name
    );
    // This unwrap is infallible.
    let account_id = AccountId::from_str(&account_config.id).unwrap();

    account_service
        .create(
            &account_id,
            &AccountData {
                name: account_config.name.clone(),
                email: account_config.email.clone(),
            },
            &AccountAuthorisation::admin(),
        )
        .await
        .ok();

    grant_service
        .add(
            &account_id,
            &account_config.role,
            &AccountAuthorisation::admin(),
        )
        .await
        .ok();

    token_service
        .create_known_secret(
            &account_id,
            &DateTime::<Utc>::MAX_UTC,
            &TokenSecret::new(account_config.token),
            &AccountAuthorisation::admin(),
        )
        .await
        .ok();
}
