use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::{ShardAssignment, ShardId};
use golem_common::proto::golem::shardmanager;
use golem_common::proto::golem::shardmanager::shard_manager_service_client;
use golem_common::retries::with_retries;

use crate::error::GolemError;
use crate::services::golem_config::{ShardManagerServiceConfig, ShardManagerServiceGrpcConfig};

/// Service providing access to the shard manager service
#[async_trait]
pub trait ShardManagerService {
    async fn register(&self, host: String, port: u16) -> Result<ShardAssignment, GolemError>;
}

pub fn configured(
    config: &ShardManagerServiceConfig,
) -> Arc<dyn ShardManagerService + Send + Sync> {
    match config {
        ShardManagerServiceConfig::Grpc(config) => {
            Arc::new(ShardManagerServiceGrpc::new(config.clone()))
        }
        ShardManagerServiceConfig::SingleShard => Arc::new(ShardManagerServiceSingleShard::new()),
    }
}

pub struct ShardManagerServiceGrpc {
    config: ShardManagerServiceGrpcConfig,
}

impl ShardManagerServiceGrpc {
    pub fn new(config: ShardManagerServiceGrpcConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ShardManagerService for ShardManagerServiceGrpc {
    async fn register(&self, host: String, port: u16) -> Result<ShardAssignment, GolemError> {
        let uri: hyper::Uri = self.config.url().to_string().parse().unwrap();
        let desc = format!("Registering instance server with shard manager at {}", uri);
        with_retries(
            &desc,
            "shard_manager",
            "register",
            &self.config.retries,
            &(host, port),
            |(host, port)| {
                let uri = uri.clone();
                Box::pin(async move {
                    let mut shard_manager_client =
                        shard_manager_service_client::ShardManagerServiceClient::connect(uri)
                            .await
                            .map_err(|err| {
                                GolemError::unknown(format!(
                                    "Connecting to shard manager failed with {}",
                                    err
                                ))
                            })?;
                    let response = shard_manager_client
                        .register(shardmanager::RegisterRequest {
                            host: host.clone(),
                            port: *port as i32,
                        })
                        .await
                        .map_err(|err| {
                            GolemError::unknown(format!(
                                "Registering with shard manager failed with {}",
                                err
                            ))
                        })?;
                    match response.into_inner() {
                        shardmanager::RegisterResponse {
                            result:
                                Some(shardmanager::register_response::Result::Success(
                                    shardmanager::RegisterSuccess {
                                        number_of_shards,
                                        shard_ids,
                                    },
                                )),
                        } => Ok(ShardAssignment {
                            number_of_shards: number_of_shards as usize,
                            shard_ids: shard_ids
                                .into_iter()
                                .map(|shard_id| shard_id.into())
                                .collect(),
                        }),
                        shardmanager::RegisterResponse {
                            result: Some(shardmanager::register_response::Result::Failure(failure)),
                        } => Err(GolemError::unknown(format!(
                            "Registering with shard manager failed with shard manager error {:?}",
                            failure
                        ))),
                        shardmanager::RegisterResponse { .. } => Err(GolemError::unknown(
                            "Registering with shard manager failed with unknown error",
                        )),
                    }
                })
            },
            |_| true,
        )
        .await
    }
}

pub struct ShardManagerServiceSingleShard {}

impl Default for ShardManagerServiceSingleShard {
    fn default() -> Self {
        Self::new()
    }
}

impl ShardManagerServiceSingleShard {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl ShardManagerService for ShardManagerServiceSingleShard {
    async fn register(&self, _host: String, _port: u16) -> Result<ShardAssignment, GolemError> {
        Ok(ShardAssignment::new(
            1,
            HashSet::from_iter(vec![ShardId::new(0)]),
        ))
    }
}
