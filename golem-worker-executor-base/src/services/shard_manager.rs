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

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_client::ShardManagerServiceClient;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{ShardAssignment, ShardId};
use golem_common::retries::with_retries;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;

use crate::error::GolemError;
use crate::services::golem_config::{ShardManagerServiceConfig, ShardManagerServiceGrpcConfig};

/// Service providing access to the shard manager service
#[async_trait]
pub trait ShardManagerService: Send + Sync {
    async fn register(&self, host: String, port: u16) -> Result<ShardAssignment, GolemError>;
}

pub fn configured(config: &ShardManagerServiceConfig) -> Arc<dyn ShardManagerService> {
    match config {
        ShardManagerServiceConfig::Grpc(config) => {
            Arc::new(ShardManagerServiceGrpc::new(config.clone()))
        }
        ShardManagerServiceConfig::SingleShard(_) => {
            Arc::new(ShardManagerServiceSingleShard::new())
        }
    }
}

pub struct ShardManagerServiceGrpc {
    config: ShardManagerServiceGrpcConfig,
    client: GrpcClient<ShardManagerServiceClient<Channel>>,
}

impl ShardManagerServiceGrpc {
    pub fn new(config: ShardManagerServiceGrpcConfig) -> Self {
        let client = GrpcClient::new(
            "shard_manager",
            |channel| {
                ShardManagerServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            config.uri(),
            GrpcClientConfig {
                retries_on_unavailable: config.retries.clone(),
                ..Default::default()
            },
        );
        Self { config, client }
    }
}

#[async_trait]
impl ShardManagerService for ShardManagerServiceGrpc {
    async fn register(&self, host: String, port: u16) -> Result<ShardAssignment, GolemError> {
        let pod_name = std::env::var_os("POD_NAME").map(|s| s.to_string_lossy().to_string());
        with_retries(
            "shard_manager",
            "register",
            Some(format!("{:?}", pod_name)),
            &self.config.retries,
            &(host, port),
            |(host, port)| {
                let client = self.client.clone();
                let pod_name = pod_name.clone();
                Box::pin(async move {
                    let response = client
                        .call("register", move |client| {
                            Box::pin(client.register(shardmanager::v1::RegisterRequest {
                                host: host.clone(),
                                port: *port as i32,
                                pod_name: pod_name.clone(),
                            }))
                        })
                        .await
                        .map_err(|err| {
                            GolemError::unknown(format!(
                                "Registering with shard manager failed with {}",
                                err
                            ))
                        })?;
                    match response.into_inner() {
                        shardmanager::v1::RegisterResponse {
                            result:
                                Some(shardmanager::v1::register_response::Result::Success(
                                    shardmanager::v1::RegisterSuccess { number_of_shards },
                                )),
                        } => Ok(ShardAssignment {
                            number_of_shards: number_of_shards as usize,
                            shard_ids: HashSet::new(),
                        }),
                        shardmanager::v1::RegisterResponse {
                            result:
                                Some(shardmanager::v1::register_response::Result::Failure(failure)),
                        } => Err(GolemError::unknown(format!(
                            "Registering with shard manager failed with shard manager error {:?}",
                            failure
                        ))),
                        shardmanager::v1::RegisterResponse { .. } => Err(GolemError::unknown(
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
