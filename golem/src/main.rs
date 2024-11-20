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
use golem_common::config::DbConfig;
use golem_common::tracing::init_tracing_with_default_debug_env_filter;
use golem_common::{
    config::DbSqliteConfig,
    tracing::{init_tracing_with_default_env_filter, TracingConfig},
};
use golem_component_service::config::ComponentServiceConfig;
use golem_component_service::ComponentService;
use golem_service_base::config::{
    BlobStorageConfig, ComponentStoreConfig, ComponentStoreLocalConfig,
};
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
use hyper::http::uri::Authority;
use hyper_util::rt::TokioIo;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::http::StatusCode;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::{Body, Endpoint, EndpointExt, IntoEndpoint, Middleware, Request, Response};
use prometheus::{default_registry, Registry};
use regex::Regex;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::task::JoinSet;

fn main() -> Result<(), anyhow::Error> {
    // TODO: root dir configuration for all sqlite / filesystem paths
    // TODO: serve command, otherwise delegate to CLI
    // TODO: verbose flag
    // TODO: start component compilation service
    // TODO: connect endpoint needs to be explicitly added to the combined Route
    let verbose: bool = true;

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    let tracing_config = tracing_config();
    if verbose {
        init_tracing_with_default_debug_env_filter(&tracing_config);
    } else {
        init_tracing_with_default_env_filter(&tracing_config);
    }

    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(Registry::default())
        .build()?;

    global::set_meter_provider(
        MeterProviderBuilder::default()
            .with_reader(exporter)
            .build(),
    );

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    runtime.block_on(run_all(runtime.clone()))
}

async fn run_all(runtime: Arc<Runtime>) -> Result<(), anyhow::Error> {
    let mut join_set = JoinSet::new();

    let _worker_executor = join_set.spawn({
        let runtime = runtime.clone();
        async move { run_worker_executor(runtime).await }
    });
    let _shard_manager = join_set.spawn(async { run_shard_manager().await });
    let _component_service = run_component_service(&mut join_set).await?;
    let _worker_service = run_worker_service(&mut join_set).await?;

    let prometheus_registry = Registry::default();
    let metrics = PrometheusExporter::new(prometheus_registry.clone());

    let proxy = Proxy::new(8083, 9005)?
        .with(OpenTelemetryMetrics::new())
        .with(Tracing);

    join_set.spawn(async move {
        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", 9881)))
            .run(proxy)
            .await
            .map_err(|err| anyhow!(err).context("HTTP server failed"))
    });

    while let Some(res) = join_set.join_next().await {
        let result = res?;
        result?;
    }

    Ok(())
}

fn tracing_config() -> TracingConfig {
    TracingConfig::test_pretty_without_time("golem")
}

const BLOB_STORAGE_DB: &'static str = "blob-storage.db";

fn worker_executor_config() -> GolemConfig {
    let mut config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::Sqlite(DbSqliteConfig {
            database: BLOB_STORAGE_DB.to_string(),
            max_connections: 32,
        }),
        indexed_storage: IndexedStorageConfig::KVStoreSqlite,
        blob_storage: BlobStorageConfig::KVStoreSqlite,
        ..Default::default()
    };

    config.add_port_to_tracing_file_name_if_enabled();
    config
}

fn shard_manager_config() -> ShardManagerConfig {
    ShardManagerConfig {
        persistence: PersistenceConfig::FileSystem(FileSystemPersistenceConfig {
            path: Path::new("sharding.bin").to_path_buf(),
        }),
        ..Default::default()
    }
}

fn component_service_config() -> ComponentServiceConfig {
    ComponentServiceConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: "components.db".to_string(),
            max_connections: 32,
        }),
        component_store: ComponentStoreConfig::Local(ComponentStoreLocalConfig {
            root_path: "components".to_string(),
            object_prefix: "".to_string(),
        }),
        blob_storage: BlobStorageConfig::Sqlite(DbSqliteConfig {
            database: BLOB_STORAGE_DB.to_string(),
            max_connections: 32,
        }),
        ..Default::default()
    }
}

fn worker_service_config() -> WorkerServiceBaseConfig {
    WorkerServiceBaseConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: "apis.db".to_string(),
            max_connections: 32,
        }),
        blob_storage: BlobStorageConfig::Sqlite(DbSqliteConfig {
            database: BLOB_STORAGE_DB.to_string(),
            max_connections: 32,
        }),
        ..Default::default()
    }
}

async fn run_worker_executor(runtime: Arc<Runtime>) -> Result<(), anyhow::Error> {
    let golem_config = worker_executor_config();
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();

    golem_worker_executor::run(golem_config, prometheus_registry, runtime.handle().clone()).await
}

async fn run_shard_manager() -> Result<(), anyhow::Error> {
    let config = shard_manager_config();
    let prometheus_registry = default_registry().clone();
    golem_shard_manager::async_main(&config, prometheus_registry).await
}

async fn run_component_service(
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = component_service_config();
    let prometheus_registry = golem_component_service::metrics::register_all();
    let migration_path = Path::new(
        "/Users/vigoo/projects/ziverge/golem-services/golem-component-service/db/migration",
    ); // TODO: this needs to be embedded in the final binary

    let component_service =
        ComponentService::new(config, prometheus_registry, migration_path).await?;

    let _server = join_set.spawn(async move { component_service.run().await });
    Ok(())
}

async fn run_worker_service(
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let config = worker_service_config();
    let prometheus_registry = golem_worker_executor_base::metrics::register_all();
    let migration_path =
        Path::new("/Users/vigoo/projects/ziverge/golem-services/golem-worker-service/db/migration"); // TODO: this needs to be embedded in the final binary

    let worker_service = WorkerService::new(config, prometheus_registry, migration_path).await?;

    let _server = join_set.spawn({ async move { worker_service.run().await } });
    Ok(())
}

struct Proxy {
    component_service_port: u16,
    worker_service_port: u16,
    exporter: BoxEndpoint<'static>,
    re_workers1: Regex,
    re_workers2: Regex,
    re_workers3: Regex,
}

impl Proxy {
    pub fn new(
        component_service_port: u16,
        worker_service_port: u16,
    ) -> Result<Self, anyhow::Error> {
        let prometheus_registry = Registry::default();
        let exporter = PrometheusExporter::new(prometheus_registry.clone())
            .into_endpoint()
            .boxed();

        let re_workers1 = Regex::new(r#"/v1/components/[^/]+/workers(.*)$"#)?;
        let re_workers2 = Regex::new(r#"/v1/components/[^/]+/invoke$"#)?;
        let re_workers3 = Regex::new(r#"/v1/components/[^/]+/invoke-and-await$"#)?;

        Ok(Self {
            component_service_port,
            worker_service_port,
            exporter,
            re_workers1,
            re_workers2,
            re_workers3,
        })
    }

    async fn proxy_to(&self, req: Request, port: u16) -> poem::Result<Response> {
        let address = format!("localhost:{}", port);
        let mut request: hyper::Request<BoxBody<Bytes, std::io::Error>> = req.into();

        let mut uri_parts = request.uri().clone().into_parts();
        uri_parts.authority = Some(address.parse().expect("Failed to parse authority"));
        *request.uri_mut() =
            hyper::Uri::from_parts(uri_parts).expect("Failed to construct modified URI");

        let stream = TcpStream::connect(address).await.map_err(|err| {
            poem::Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
        })?;

        let io = TokioIo::new(stream);
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(io)
                .await
                .map_err(|err| {
                    poem::Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
                })?;

        let response = sender.send_request(request).await.map_err(|err| {
            poem::Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
        })?;
        let mut builder = Response::builder();
        builder = builder.status(response.status());
        for (name, value) in response.headers() {
            builder = builder.header(name, value);
        }
        let body_stream = response
            .into_body()
            .map_err(|err| std::io::Error::other(err.to_string()))
            .into_data_stream();
        let poem_response = builder.body(Body::from_bytes_stream(body_stream));
        Ok(poem_response)
    }

    async fn proxy_to_worker_service(&self, req: Request) -> poem::Result<Response> {
        self.proxy_to(req, self.worker_service_port).await
    }

    async fn proxy_to_component_service(&self, req: Request) -> poem::Result<Response> {
        self.proxy_to(req, self.component_service_port).await
    }

    async fn proxy(&self, req: Request) -> poem::Result<Response> {
        let uri = req.uri();
        let path = uri.path();

        if path == "/metrics" {
            self.exporter.call(req).await
        } else if path.starts_with("/v1/api")
            || self.re_workers1.is_match(path)
            || self.re_workers2.is_match(path)
            || self.re_workers3.is_match(path)
        {
            self.proxy_to_worker_service(req).await
        } else {
            self.proxy_to_component_service(req).await
        }
    }
}

impl Endpoint for Proxy {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        async move { self.proxy(req).await }
    }
}
