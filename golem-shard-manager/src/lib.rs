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

mod error;
mod healthcheck;
mod http_server;
mod model;
mod persistence;
mod rebalancing;
mod shard_management;
mod shard_manager_config;
mod worker_executor;

use std::env;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use crate::healthcheck::{get_unhealthy_pods, GrpcHealthCheck, HealthCheck};
use error::ShardManagerError;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::shard_manager_service_server::{
    ShardManagerService, ShardManagerServiceServer,
};
use model::{Pod, RoutingTable};
use persistence::{PersistenceService, PersistenceServiceDefault};
use prometheus::{default_registry, Registry};
use shard_management::ShardManagement;
use shard_manager_config::ShardManagerConfig;
use tonic::transport::Server;
use tonic::Response;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use worker_executor::{WorkerExecutorService, WorkerExecutorServiceDefault};

use crate::http_server::HttpServerImpl;
use crate::shard_manager_config::{HealthCheckK8sConfig, HealthCheckMode};

pub struct ShardManagerServiceImpl {
    shard_management: ShardManagement,
    shard_manager_config: Arc<ShardManagerConfig>,
    health_check: Arc<dyn HealthCheck + Send + Sync>,
}

impl ShardManagerServiceImpl {
    async fn new(
        persistence_service: Arc<dyn PersistenceService + Send + Sync>,
        worker_executor_service: Arc<dyn WorkerExecutorService + Send + Sync>,
        shard_manager_config: Arc<ShardManagerConfig>,
        health_check: Arc<dyn HealthCheck + Send + Sync>,
    ) -> Result<ShardManagerServiceImpl, ShardManagerError> {
        let shard_management = ShardManagement::new(
            persistence_service.clone(),
            worker_executor_service,
            health_check.clone(),
            shard_manager_config.rebalance_threshold,
        )
        .await?;

        let shard_manager_service = ShardManagerServiceImpl {
            shard_management,
            shard_manager_config,
            health_check,
        };

        info!("Starting health check process...");
        shard_manager_service.start_health_check();
        info!("Shard Manager is fully operational.");

        Ok(shard_manager_service)
    }

    async fn get_routing_table_internal(&self) -> RoutingTable {
        let routing_table = self.shard_management.current_snapshot().await;
        info!("Shard Manager providing routing table: {}", routing_table);
        routing_table
    }

    async fn register_internal(
        &self,
        request: tonic::Request<golem::shardmanager::RegisterRequest>,
    ) -> Result<(), ShardManagerError> {
        let pod = Pod::from_register_request(request)?;
        info!("Shard Manager received request to register pod: {}", pod);
        self.shard_management.register_pod(pod).await;
        Ok(())
    }

    fn start_health_check(&self) {
        let delay = self.shard_manager_config.health_check.delay;
        let shard_management = self.shard_management.clone();
        let health_check = self.health_check.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(delay).await;
                Self::health_check(shard_management.clone(), health_check.clone()).await
            }
        });
    }

    async fn health_check(
        shard_management: ShardManagement,
        health_check: Arc<dyn HealthCheck + Send + Sync>,
    ) {
        debug!("Shard Manager scheduled to conduct health check");
        let routing_table = shard_management.current_snapshot().await;
        debug!("Shard Manager checking health of registered pods...");
        let failed_pods = get_unhealthy_pods(health_check, &routing_table.get_pods()).await;
        if failed_pods.is_empty() {
            debug!("All registered pods are healthy")
        } else {
            warn!(
                "The following pods were found to be unhealthy: {:?}",
                failed_pods
            );
            for failed_pod in failed_pods {
                shard_management.unregister_pod(failed_pod).await;
            }
        }

        debug!("Golem Shard Manager finished checking health of registered pods");
    }
}

#[tonic::async_trait]
impl ShardManagerService for ShardManagerServiceImpl {
    async fn get_routing_table(
        &self,
        _request: tonic::Request<golem::shardmanager::GetRoutingTableRequest>,
    ) -> Result<tonic::Response<golem::shardmanager::GetRoutingTableResponse>, tonic::Status> {
        Ok(Response::new(
            golem::shardmanager::GetRoutingTableResponse {
                result: Some(
                    golem::shardmanager::get_routing_table_response::Result::Success(
                        self.get_routing_table_internal().await.into(),
                    ),
                ),
            },
        ))
    }

    async fn register(
        &self,
        request: tonic::Request<golem::shardmanager::RegisterRequest>,
    ) -> Result<tonic::Response<golem::shardmanager::RegisterResponse>, tonic::Status> {
        match self.register_internal(request).await {
            Ok(_) => Ok(Response::new(golem::shardmanager::RegisterResponse {
                result: Some(golem::shardmanager::register_response::Result::Success(
                    golem::shardmanager::RegisterSuccess {
                        number_of_shards: self.shard_manager_config.number_of_shards as u32,
                    },
                )),
            })),
            Err(error) => Ok(Response::new(golem::shardmanager::RegisterResponse {
                result: Some(golem::shardmanager::register_response::Result::Failure(
                    error.into(),
                )),
            })),
        }
    }
}

pub fn server_main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ShardManagerConfig::new();
    let registry = default_registry().clone();

    if config.enable_json_log {
        tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            // .with_span_events(FmtSpan::FULL) // NOTE: enable to see span events
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_ansi(true)
            .init();
    }

    // NOTE: to enable tokio-console, comment the lines above and uncomment the lines below,
    // and compile with RUSTFLAGS="--cfg tokio_unstable" cargo build
    // TODO: make tracing subscription configurable
    // console_subscriber::init();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(&config, registry))
}

async fn async_main(
    shard_manager_config: &ShardManagerConfig,
    registry: Registry,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build()
        .unwrap();

    info!("Golem Shard Manager starting up...");

    let _ = HttpServerImpl::new(
        SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), shard_manager_config.http_port),
        registry,
    );

    info!("Using Redis at {}", shard_manager_config.redis.url());
    let pool = golem_common::redis::RedisPool::configured(&shard_manager_config.redis).await?;

    let shard_manager_config = Arc::new(shard_manager_config.clone());

    let persistence_service = Arc::new(PersistenceServiceDefault::new(
        &pool,
        &shard_manager_config.number_of_shards,
    ));
    let worker_executors = Arc::new(WorkerExecutorServiceDefault::new(
        shard_manager_config.worker_executors.clone(),
    ));

    let shard_manager_port_str = env::var("GOLEM_SHARD_MANAGER_PORT")?;
    info!("The port read from env is {}", shard_manager_port_str);
    let shard_manager_port = shard_manager_port_str.parse::<u16>()?;
    let shard_manager_addr = format!("0.0.0.0:{}", shard_manager_port);

    info!("Listening on port {}", shard_manager_port);

    let addr = shard_manager_addr.parse()?;

    let health_check: Arc<dyn HealthCheck + Send + Sync> =
        match &shard_manager_config.health_check.mode {
            HealthCheckMode::Grpc(_) => Arc::new(GrpcHealthCheck::new(
                worker_executors.clone(),
                shard_manager_config.worker_executors.retries.clone(),
            )),
            #[cfg(feature = "kubernetes")]
            HealthCheckMode::K8s(HealthCheckK8sConfig { namespace }) => Arc::new(
                crate::healthcheck::kubernetes::KubernetesHealthCheck::new(
                    namespace.clone(),
                    shard_manager_config.worker_executors.retries.clone(),
                )
                .await
                .expect("Failed to initialize K8s health checker"),
            ),
        };

    let shard_manager = ShardManagerServiceImpl::new(
        persistence_service,
        worker_executors,
        shard_manager_config,
        health_check,
    )
    .await?;

    let service = ShardManagerServiceServer::new(shard_manager);

    // TODO: configurable limits
    Server::builder()
        .concurrency_limit_per_connection(1024)
        .max_concurrent_streams(Some(1024))
        .add_service(reflection_service)
        .add_service(service)
        .add_service(health_service)
        .serve(addr)
        .await?;

    info!("Server started on port {}", shard_manager_port);

    Ok(())
}
