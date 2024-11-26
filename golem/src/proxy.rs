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
    worker_service_port: u16,
    // exporter: BoxEndpoint<'static>,
    // routes: Vec<(Regex, u16)>,
}

impl Proxy {
    pub fn new(
        metrics_port: u16,
        component_service_port: u16,
        worker_service_port: u16,
    ) -> Result<Self, anyhow::Error> {
        // let prometheus_registry = Registry::default();
        // let exporter = PrometheusExporter::new(prometheus_registry.clone())
        //     .into_endpoint()
        //     .boxed();

        // let routes = vec![
        //     (Regex::new(r#"^/v1/api$"#)?, component_service_port),
        //     (Regex::new(r#"^/v1/components/[^/]+/workers.*"#)?, worker_service_port),
        //     (Regex::new(r#"^/v1/components/[^/]+/invoke$"#)?, worker_service_port),
        //     (Regex::new(r#"^/v1/components/[^/]+/invoke-and-await$"#)?, worker_service_port),
        //     (Regex::new(r#"^/v1/components.*"#)?, component_service_port),
        //     (Regex::new(r#"^/.*"#)?, worker_service_port),
        // ];

        Ok(Self {
            metrics_port,
            component_service_port,
            worker_service_port,
            // exporter,
            // routes,
        })
    }

    async fn start(&self) -> Result<(), anyhow::Error> {

        let http_listener = ListenerBuilder::new_http(SocketAddress::new_v4(127,0,0,1,8080)).to_http(None)?;

        let (mut command_channel, proxy_channel) = Channel::generate(1000, 10000).with_context(|| "should create a channel")?;

        let worker_thread_join_handle = thread::spawn(move || {
            let max_buffers = 500;
            let buffer_size = 16384;
            sozu_lib::http::testing::start_http_worker(http_listener, proxy_channel, max_buffers, buffer_size)
                .expect("The worker could not be started, or shut down");
        });

        let cluster = Cluster {
            cluster_id: "my-cluster".to_string(),
            sticky_session: false,
            https_redirect: false,
            load_balancing: LoadBalancingAlgorithms::RoundRobin as i32,
            answer_503: Some("A custom forbidden message".to_string()),
            ..Default::default()
        };

        let http_front = RequestHttpFrontend {
            cluster_id: Some("my-cluster".to_string()),
            address: SocketAddress::new_v4(127,0,0,1,8080),
            hostname: "example.com".to_string(),
            path: PathRule::prefix(String::from("/")),
            position: RulePosition::Pre.into(),
            tags: BTreeMap::from([
                ("owner".to_owned(), "John".to_owned()),
                ("id".to_owned(), "my-own-http-front".to_owned()),
            ]),
            ..Default::default()
        };

        let http_backend = AddBackend {
            cluster_id: "my-cluster".to_string(),
            backend_id: "test-backend".to_string(),
            address: SocketAddress::new_v4(127,0,0,1,8000),
            load_balancing_parameters: Some(LoadBalancingParams::default()),
            ..Default::default()
        };

        command_channel
            .write_message(&WorkerRequest {
                id: String::from("add-the-cluster"),
                content: RequestType::AddCluster(cluster).into(),
            })?;

        command_channel
            .write_message(&WorkerRequest {
                id: String::from("add-the-frontend"),
                content: RequestType::AddHttpFrontend(http_front).into(),
            })?;

        command_channel
            .write_message(&WorkerRequest {
                id: String::from("add-the-backend"),
                content: RequestType::AddBackend(http_backend).into(),
            })?;

        println!("HTTP -> {:?}", command_channel.read_message());
        println!("HTTP -> {:?}", command_channel.read_message());
        Ok(())

        // uncomment to let it run in the background
        // let _ = worker_thread_join_handle.join();
    }
}
