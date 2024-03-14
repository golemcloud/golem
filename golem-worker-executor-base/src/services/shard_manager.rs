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

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::shard_manager_service_client;
use golem_common::model::{ShardAssignment, ShardId};
use golem_common::retries::with_retries;

use crate::error::GolemError;
use crate::grpc::UriBackConversion;
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
        let pod_name = std::env::var_os("POD_NAME").map(|s| s.to_string_lossy().to_string());
        let desc = format!(
            "Registering worker executor with shard manager at {uri} using pod name {pod_name:?}"
        );
        with_retries(
            &desc,
            "shard_manager",
            "register",
            &self.config.retries,
            &(host, port),
            |(host, port)| {
                let uri = uri.clone();
                let pod_name = pod_name.clone();
                Box::pin(async move {
                    let mut shard_manager_client =
                        shard_manager_service_client::ShardManagerServiceClient::connect(
                            uri.as_http_02(),
                        )
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
                            pod_name: pod_name.clone(),
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
                                    shardmanager::RegisterSuccess { number_of_shards },
                                )),
                        } => Ok(ShardAssignment {
                            number_of_shards: number_of_shards as usize,
                            shard_ids: HashSet::new(),
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
