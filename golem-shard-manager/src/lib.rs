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

mod error;
mod healthcheck;
mod model;
mod persistence;
mod rebalancing;
mod shard_management;
pub mod shard_manager_config;
mod worker_executor;

use crate::error::ShardManagerTraceErrorKind;
use crate::healthcheck::{get_unhealthy_pods, GrpcHealthCheck, HealthCheck};
use crate::persistence::RoutingTableFileSystemPersistence;
use crate::shard_manager_config::{HealthCheckK8sConfig, HealthCheckMode, PersistenceConfig};
use error::ShardManagerError;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::{
    ShardManagerService, ShardManagerServiceServer,
};
use golem_common::recorded_grpc_api_request;
use model::{Pod, RoutingTable};
use persistence::{RoutingTablePersistence, RoutingTableRedisPersistence};
use prometheus::Registry;
use shard_management::ShardManagement;
use shard_manager_config::ShardManagerConfig;
use std::env;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tonic::Response;
use tracing::Instrument;
use tracing::{debug, info, warn};
use worker_executor::{WorkerExecutorService, WorkerExecutorServiceDefault};

#[cfg(test)]
test_r::enable!();

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
}

pub struct ShardManagerServiceImpl {
    shard_management: ShardManagement,
    shard_manager_config: Arc<ShardManagerConfig>,
    health_check: Arc<dyn HealthCheck + Send + Sync>,
}

impl ShardManagerServiceImpl {
    async fn new(
        persistence_service: Arc<dyn RoutingTablePersistence + Send + Sync>,
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
        source_ip: Option<SocketAddr>,
        request: golem::shardmanager::v1::RegisterRequest,
    ) -> Result<(), ShardManagerError> {
        let source_ip = source_ip.ok_or(ShardManagerError::NoSourceIpForPod)?.ip();

        let pod = Pod::from_register_request(source_ip, request)?;
        info!("Shard Manager received request to register pod: {}", pod);
        self.shard_management.register_pod(pod).await;
        Ok(())
    }

    fn start_health_check(&self) {
        let delay = self.shard_manager_config.health_check.delay;
        let shard_management = self.shard_management.clone();
        let health_check = self.health_check.clone();

        tokio::spawn(
            async move {
                loop {
                    tokio::time::sleep(delay).await;
                    Self::health_check(shard_management.clone(), health_check.clone()).await
                }
            }
            .in_current_span(),
        );
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
        _request: tonic::Request<golem::shardmanager::v1::GetRoutingTableRequest>,
    ) -> Result<Response<golem::shardmanager::v1::GetRoutingTableResponse>, tonic::Status> {
        let record = recorded_grpc_api_request!("get_routing_table",);

        let response = self
            .get_routing_table_internal()
            .instrument(record.span.clone())
            .await;

        Ok(Response::new(
            golem::shardmanager::v1::GetRoutingTableResponse {
                result: Some(
                    golem::shardmanager::v1::get_routing_table_response::Result::Success(
                        response.into(),
                    ),
                ),
            },
        ))
    }

    async fn register(
        &self,
        request: tonic::Request<golem::shardmanager::v1::RegisterRequest>,
    ) -> Result<Response<golem::shardmanager::v1::RegisterResponse>, tonic::Status> {
        let source_ip = request.remote_addr();
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "register",
            source_ip = source_ip.map(|ip| ip.to_string()),
            host = &request.host,
            port = &request.port.to_string(),
        );

        let response = self
            .register_internal(source_ip, request)
            .instrument(record.span.clone())
            .await;

        let result = match response {
            Ok(_) => record.succeed(golem::shardmanager::v1::register_response::Result::Success(
                golem::shardmanager::v1::RegisterSuccess {
                    number_of_shards: self.shard_manager_config.number_of_shards as u32,
                },
            )),
            Err(error) => {
                let error: golem::shardmanager::v1::ShardManagerError = error.into();
                record.fail(
                    golem::shardmanager::v1::register_response::Result::Failure(error.clone()),
                    &ShardManagerTraceErrorKind(&error),
                )
            }
        };

        Ok(Response::new(golem::shardmanager::v1::RegisterResponse {
            result: Some(result),
        }))
    }
}

pub async fn run(
    shard_manager_config: &ShardManagerConfig,
    registry: Registry,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<RunDetails> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    info!("Golem Shard Manager starting up...");

    let http_port = golem_service_base::observability::start_health_and_metrics_server(
        SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), shard_manager_config.http_port),
        registry,
        "shard manager is running",
        join_set,
    )
    .await?;

    let shard_manager_config = Arc::new(shard_manager_config.clone());

    let persistence_service: Arc<dyn RoutingTablePersistence + Send + Sync> =
        match &shard_manager_config.persistence {
            PersistenceConfig::Redis(redis) => {
                info!("Using Redis at {}", redis.url());
                let pool = golem_common::redis::RedisPool::configured(redis).await?;
                Arc::new(RoutingTableRedisPersistence::new(
                    &pool,
                    shard_manager_config.number_of_shards,
                ))
            }
            PersistenceConfig::FileSystem(fs) => {
                info!("Using sharding file {:?}", fs.path);
                Arc::new(
                    RoutingTableFileSystemPersistence::new(
                        &fs.path,
                        shard_manager_config.number_of_shards,
                    )
                    .await?,
                )
            }
        };
    let worker_executors = Arc::new(WorkerExecutorServiceDefault::new(
        shard_manager_config.worker_executors.clone(),
    ));

    let health_check: Arc<dyn HealthCheck + Send + Sync> =
        match &shard_manager_config.health_check.mode {
            HealthCheckMode::Grpc(_) => Arc::new(GrpcHealthCheck::new(
                worker_executors.clone(),
                shard_manager_config.worker_executors.retries.clone(),
                shard_manager_config.health_check.silent,
            )),
            #[cfg(feature = "kubernetes")]
            HealthCheckMode::K8s(HealthCheckK8sConfig { namespace }) => Arc::new(
                healthcheck::kubernetes::KubernetesHealthCheck::new(
                    namespace.clone(),
                    shard_manager_config.worker_executors.retries.clone(),
                    shard_manager_config.health_check.silent,
                )
                .await
                .expect("Failed to initialize K8s health checker"),
            ),
        };

    let shard_manager = ShardManagerServiceImpl::new(
        persistence_service,
        worker_executors,
        shard_manager_config.clone(),
        health_check,
    )
    .await?;

    let service = ShardManagerServiceServer::new(shard_manager);

    let shard_manager_port_str =
        env::var("GOLEM_SHARD_MANAGER_PORT").unwrap_or(shard_manager_config.grpc_port.to_string());
    info!("The port read from env is {}", shard_manager_port_str);
    let configured_port = shard_manager_port_str.parse::<u16>()?;
    let listener = TcpListener::bind(SocketAddrV4::new(
        Ipv4Addr::new(0, 0, 0, 0),
        configured_port,
    ))
    .await?;
    let grpc_port = listener.local_addr()?.port();

    join_set.spawn(
        async move {
            Server::builder()
                .add_service(reflection_service)
                .add_service(
                    service
                        .accept_compressed(CompressionEncoding::Gzip)
                        .send_compressed(CompressionEncoding::Gzip),
                )
                .add_service(health_service)
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .map_err(|e| anyhow::anyhow!(e).context("gRPC server failed"))
        }
        .in_current_span(),
    );

    info!("Server started on port {}", grpc_port);

    Ok(RunDetails {
        http_port,
        grpc_port,
    })
}
