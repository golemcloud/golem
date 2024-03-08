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
mod http_server;
mod model;
mod persistence;
mod rebalancing;
mod shard_management;
mod shard_manager_config;
mod worker_executor;

use std::collections::{BTreeSet, HashSet};
use std::env;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use error::ShardManagerError;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::shard_manager_service_server::{
    ShardManagerService, ShardManagerServiceServer,
};
use golem_common::model::ShardId;
use model::{Assignments, Pod, RoutingTable, Unassignments};
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
use crate::rebalancing::Rebalance;

pub struct ShardManagerServiceImpl {
    shard_management: ShardManagement,
    persistence_service: Arc<dyn PersistenceService + Send + Sync>,
    worker_executor_service: Arc<dyn WorkerExecutorService + Send + Sync>,
    shard_manager_config: Arc<ShardManagerConfig>,
}

impl ShardManagerServiceImpl {
    async fn new(
        persistence_service: Arc<dyn PersistenceService + Send + Sync>,
        instance_server_service: Arc<dyn WorkerExecutorService + Send + Sync>,
        shard_manager_config: Arc<ShardManagerConfig>,
    ) -> Result<ShardManagerServiceImpl, ShardManagerError> {
        info!("Reading routing table from persistent storage");
        let (routing_table, rebalance) = persistence_service.read().await.unwrap();
        info!(
            "Routing table read from persistent storage: {}",
            routing_table
        );

        if rebalance.is_empty() {
            info!("No rebalance was in progress.");
        } else {
            info!("A rebalance was in progress: {}", rebalance);
            // ShardManagerServiceImpl::rebalance(
            //     &mut routing_table,
            //     rebalance,
            //     instance_server_service.clone(),
            //     persistence_service.clone(),
            // )
            // .await?;
            // info!("In progress rebalance completed: {}", routing_table);
        }

        let shard_management = ShardManagement::new(routing_table, rebalance).await;

        let shard_manager_service = ShardManagerServiceImpl {
            shard_management,
            persistence_service,
            worker_executor_service: instance_server_service,
            shard_manager_config,
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
        // let mut routing_table = self.routing_table.write().await;
        // info!("Shard Manager registering pod: {}", pod);
        // match routing_table.get_shards(&pod) {
        //     Some(shard_ids) => {
        //         let number_of_shards = routing_table.number_of_shards;
        //         let shard_assignment = ShardAssignment::new(number_of_shards, shard_ids);
        //         info!("Pod already registered and assigned: {}", shard_assignment);
        //         Ok(shard_assignment)
        //     }
        //     None => {
        //         routing_table.add_pod(&pod);
        //         let number_of_shards = routing_table.number_of_shards;
        //         let shard_ids = routing_table.get_shards(&pod).unwrap();
        //         let shard_assignment = ShardAssignment::new(number_of_shards, shard_ids);
        //         info!("Pod registered and assigned: {}", shard_assignment);
        //         Ok(shard_assignment)
        //     }
        // }
    }

    fn start_health_check(&self) {
        let delay = self.shard_manager_config.health_check.delay;
        let shard_management = self.shard_management.clone();
        let worker_executor_service = self.worker_executor_service.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(delay).await;
                Self::health_check(shard_management.clone(), worker_executor_service.clone()).await
            }
        });
    }

    async fn health_check(
        shard_management: ShardManagement,
        worker_executor_service: Arc<dyn WorkerExecutorService + Send + Sync>,
    ) {
        debug!("Shard Manager scheduled to conduct health check");
        let routing_table = shard_management.current_snapshot().await;
        debug!("Shard Manager checking health of registered pods...");
        let failed_pods =
            Self::health_check_pods(&routing_table.get_pods(), worker_executor_service.clone())
                .await;
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
        // let rebalance = Rebalance::from_routing_table(&routing_table);
        // if !rebalance.is_empty() {
        //     Self::rebalance(
        //         &mut routing_table,
        //         rebalance,
        //         worker_executor_service.clone(),
        //         persistence_service.clone(),
        //     )
        //     .await
        //     .ok();
        // }
        debug!("Golem Shard Manager finished checking health of registered pods");
    }

    async fn health_check_pods(
        pods: &HashSet<Pod>,
        worker_executor: Arc<dyn WorkerExecutorService + Send + Sync>,
    ) -> HashSet<Pod> {
        let futures: Vec<_> = pods
            .iter()
            .map(|pod| {
                let worker_executor = worker_executor.clone();
                Box::pin(async move {
                    match worker_executor.health_check(pod).await {
                        true => None,
                        false => Some(pod.clone()),
                    }
                })
            })
            .collect();
        futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    async fn revoke_shards(
        unassignments: &Unassignments,
        worker_executor: Arc<dyn WorkerExecutorService + Send + Sync>,
    ) -> Vec<(Pod, BTreeSet<ShardId>)> {
        let futures: Vec<_> = unassignments
            .unassignments
            .iter()
            .map(|(pod, shard_ids)| {
                let worker_executor = worker_executor.clone();
                Box::pin(async move {
                    match worker_executor.revoke_shards(pod, shard_ids).await {
                        Ok(_) => None,
                        Err(_) => Some((pod.clone(), shard_ids.clone())),
                    }
                })
            })
            .collect();
        futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    async fn assign_shards(
        assignments: &Assignments,
        instance_server_service: Arc<dyn WorkerExecutorService + Send + Sync>,
    ) -> Vec<(Pod, BTreeSet<ShardId>)> {
        let futures: Vec<_> = assignments
            .assignments
            .iter()
            .map(|(pod, shard_ids)| {
                let instance_server_service = instance_server_service.clone();
                Box::pin(async move {
                    match instance_server_service.assign_shards(pod, shard_ids).await {
                        Ok(_) => None,
                        Err(_) => Some((pod.clone(), shard_ids.clone())),
                    }
                })
            })
            .collect();
        futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    async fn rebalance(
        routing_table: &mut RoutingTable,
        mut rebalance: Rebalance,
        instance_server_service: Arc<dyn WorkerExecutorService + Send + Sync>,
        persistence_service: Arc<dyn PersistenceService + Send + Sync>,
    ) -> Result<(), ShardManagerError> {
        info!("Shard manager beginning rebalance...");
        let pods = rebalance.get_pods();
        info!("The following pods are involved in rebalance: {:?}", pods);

        info!("Conducting health check of pods involved in rebalance");
        let unhealthy_pods = Self::health_check_pods(&pods, instance_server_service.clone()).await;
        rebalance.remove_pods(&unhealthy_pods);
        info!("The following pods were found to be unhealthy and have been removed from rebalance: {:?}", unhealthy_pods);

        info!(
            "Writing planned rebalance: {} to persistent storage",
            rebalance
        );
        persistence_service.write(routing_table, &rebalance).await?;
        info!("Planned rebalance written to persistent storage");

        info!(
            "Executing shard unassignments: {}",
            rebalance.get_unassignments()
        );
        let failed_unassignments = Self::revoke_shards(
            rebalance.get_unassignments(),
            instance_server_service.clone(),
        )
        .await;
        let failed_shards = failed_unassignments
            .iter()
            .flat_map(|(_, shard_ids)| shard_ids.clone())
            .collect();
        rebalance.remove_shards(&failed_shards);
        info!("The following shards could not be unassigned and have been removed from rebalance: {:?}", failed_shards);

        info!(
            "Executing shard assignments: {}",
            rebalance.get_assignments()
        );
        Self::assign_shards(rebalance.get_assignments(), instance_server_service.clone()).await;
        routing_table.rebalance(rebalance);
        info!("Executed shard assignments");

        info!(
            "Writing update routing table: {} to persistent storage",
            routing_table
        );
        persistence_service
            .write(routing_table, &Rebalance::empty())
            .await?;
        info!("Updated routing table written to persistent storage");

        Ok(())
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    let instance_server_service = Arc::new(WorkerExecutorServiceDefault::new(
        shard_manager_config.instance_server_service.clone(),
    ));

    let shard_manager_port_str = env::var("GOLEM_SHARD_MANAGER_PORT")?;
    info!("The port read from env is {}", shard_manager_port_str);
    let shard_manager_port = shard_manager_port_str.parse::<u16>()?;
    let shard_manager_addr = format!("0.0.0.0:{}", shard_manager_port);

    info!("Listening on port {}", shard_manager_port);

    let addr = shard_manager_addr.parse()?;

    let shard_manager = ShardManagerServiceImpl::new(
        persistence_service,
        instance_server_service,
        shard_manager_config,
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
