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

use anyhow::anyhow;
use bytes::Bytes;
use futures::future::BoxFuture;
use golem_common::config::DbConfig;
use golem_common::tracing::init_tracing_with_default_debug_env_filter;
use golem_common::{
    config::DbSqliteConfig,
    tracing::{init_tracing_with_default_env_filter, TracingConfig},
};
use golem_component_service::config::ComponentServiceConfig;
use golem_component_service::ComponentService;
use golem_component_service_base::config::{ComponentStoreConfig, ComponentStoreLocalConfig};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::migration::Migrations;
use golem_shard_manager::shard_manager_config::{
    FileSystemPersistenceConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_worker_executor_base::services::golem_config::{
    GolemConfig, IndexedStorageConfig, KeyValueStorageConfig,
};
use golem_worker_service::WorkerService;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use include_dir::Dir;
use include_dir::include_dir;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::http::StatusCode;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::{Body, Endpoint, EndpointExt, IntoEndpoint, Request, Response};
use prometheus::{default_registry, Registry};
use regex::Regex;
use sqlx::error::BoxDynError;
use sqlx::migrate::{Migration, MigrationSource};
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::task::JoinSet;

#[derive(Debug)]
struct SpecificMigrationsDir<'a>(&'a Dir<'a>);

impl<'a> SpecificMigrationsDir<'a> {
    async fn resolve_impl(self) -> Result<Vec<Migration>, BoxDynError> {
        let temp_dir = tempfile::tempdir().map_err(Box::new)?;
        self.0.extract(temp_dir.path()).map_err(Box::new)?;
        temp_dir.path().resolve().await
    }
}

impl <'a> MigrationSource<'a> for SpecificMigrationsDir<'a> {
    fn resolve(self) -> BoxFuture<'a, Result<Vec<Migration>, BoxDynError>> {
        Box::pin(self.resolve_impl())
    }
}

struct MigrationsDir(Dir<'static>);

impl Migrations for MigrationsDir {
    type Output<'b> = SpecificMigrationsDir<'b>
        where Self: 'b;

    fn sqlite_migrations<'b>(&'b self) -> Self::Output<'b> {
        SpecificMigrationsDir(self.0.get_dir("sqlite").unwrap())
    }

    fn postgres_migrations<'b>(&'b self) -> Self::Output<'b> {
        SpecificMigrationsDir(self.0.get_dir("postgres").unwrap())
    }
}

macro_rules! include_migrations_dir {
    ($path:expr) => {
        MigrationsDir(include_dir!(concat!($path, "/db/migration")))
    };
}

// pub fn included_migrations_dir(path: &str) -> MigrationsDir {
//     MigrationsDir(include_dir!(path))
// }
