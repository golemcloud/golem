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
use crate::migration::{IncludedMigrationsDir};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::http::StatusCode;
use poem::listener::TcpListener;
use prometheus::{default_registry, Registry};
use regex::Regex;
use sqlx::error::BoxDynError;
use sqlx::migrate::{Migration, MigrationSource};
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::task::JoinSet;
use std::{collections::BTreeMap, env, io::stdout, thread};
use anyhow::Context;
use sozu_command_lib::{
    channel::Channel,
    config::ListenerBuilder,
    logging::setup_default_logging,
    proto::command::{
        request::RequestType, AddBackend, Cluster, LoadBalancingAlgorithms, LoadBalancingParams,
        PathRule, Request, RequestHttpFrontend, RulePosition, SocketAddress,WorkerRequest,
    },
};


struct Proxy {
    metrics_port: u16,
    component_service_port: u16,
    worker_service_port: u16
}

impl Proxy {
    pub fn new(
        metrics_port: u16,
        component_service_port: u16,
        worker_service_port: u16,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self {
            metrics_port,
            component_service_port,
            worker_service_port,
        })
    }

    pub async fn start(&self) -> Result<(), anyhow::Error> {
        let listener_address = SocketAddress::new_v4(127,0,0,1,8080);

        let http_listener = ListenerBuilder::new_http(listener_address).to_http(None)?;

        let (mut command_channel, proxy_channel) = Channel::generate(1000, 10000).with_context(|| "should create a channel")?;

        let _worker_thread_join_handle = thread::spawn(move || {
            let max_buffers = 500;
            let buffer_size = 16384;
            sozu_lib::http::testing::start_http_worker(http_listener, proxy_channel, max_buffers, buffer_size)
                .expect("The worker could not be started, or shut down");
        });

        let metrics_backend = "golem-metrics";
        let component_backend = "golem-component";
        let worker_backend = "golem-worker";

        // set up the clusters. We'll have one per service with a single backend per cluster
        {

            let mut add_backend = |(name, port): (&str, u16)| {
                command_channel
                  .write_message(&WorkerRequest {
                        id: format!("add-{name}-cluster"),
                        content: RequestType::AddCluster(
                            Cluster {
                                cluster_id: name.to_string(),
                                sticky_session: false,
                                https_redirect: false,
                                load_balancing: LoadBalancingAlgorithms::Random as i32,
                                ..Default::default()
                            }
                        ).into(),
                    })?;

                command_channel
                    .write_message(&WorkerRequest {
                        id: format!("add-{name}-backend"),
                        content: RequestType::AddBackend(
                            AddBackend {
                                cluster_id: name.to_string(),
                                backend_id: name.to_string(),
                                address: SocketAddress::new_v4(127,0,0,1, port),
                                ..Default::default()
                            }
                        ).into(),
                    })
            };

            add_backend((metrics_backend, self.metrics_port))?;
            add_backend((component_backend, self.component_service_port))?;
            add_backend((worker_backend, self.worker_service_port))?;

        }

        // set up routing
        {
            let mut route_counter = -1;
            let mut add_route = |(path, cluster_id): (PathRule, &str)| {
                route_counter += 1;
                command_channel.write_message(&WorkerRequest {
                        id: format!("add-golem-frontend-${route_counter}"),
                        content: RequestType::AddHttpFrontend(
                            RequestHttpFrontend {
                                cluster_id: Some(cluster_id.to_string()),
                                address: SocketAddress::new_v4(127,0,0,1,8080),
                                hostname: "*".to_string(),
                                path,
                                position: RulePosition::Post.into(),
                                ..Default::default()
                            }
                        ).into(),
                    })
            };

            add_route((PathRule::regex("/v1/components/[^/]+/workers/[^/]+/connect$"), worker_backend))?;
            add_route((PathRule::prefix("/v1/api"), worker_backend))?;
            add_route((PathRule::regex("/v1/components/[^/]+/workers"), worker_backend))?;
            add_route((PathRule::regex("/v1/components/[^/]+/invoke"), worker_backend))?;
            add_route((PathRule::regex("/v1/components/[^/]+/invoke-and-await"), worker_backend))?;
            add_route((PathRule::prefix("/v1/components"), component_backend))?;
            add_route((PathRule::prefix("/"), component_backend))?;
        }

        Ok(())

        // uncomment to let it run in the background
        // let _ = worker_thread_join_handle.join();
    }
}
