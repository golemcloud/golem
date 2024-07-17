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

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use tonic::transport::Channel;

use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::shard_manager_service_client::ShardManagerServiceClient;
use golem_common::cache::*;
use golem_common::client::GrpcClient;
use golem_common::model::RoutingTable;

#[derive(Debug, Clone)]
pub enum RoutingTableError {
    Unexpected(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingTableConfig {
    host: String,
    port: u16,
}

impl RoutingTableConfig {
    pub fn url(&self) -> http::Uri {
        format!("http://{}:{}", self.host, self.port)
            .parse()
            .expect("Failed to parse shard manager URL")
    }
}

impl Default for RoutingTableConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9002,
        }
    }
}

impl RoutingTableError {
    pub fn unexpected(details: impl Into<String>) -> Self {
        RoutingTableError::Unexpected(details.into())
    }
}

#[async_trait]
pub trait RoutingTableService {
    async fn get_routing_table(&self) -> Result<RoutingTable, RoutingTableError>;
    async fn invalidate_routing_table(&self);
}

pub trait HasRoutingTableService {
    fn routing_table_service(&self) -> &Arc<dyn RoutingTableService + Send + Sync>;
}

pub struct RoutingTableServiceDefault {
    cache: Cache<(), (), RoutingTable, RoutingTableError>,
    client: GrpcClient<ShardManagerServiceClient<Channel>>,
}

impl RoutingTableServiceDefault {
    pub fn new(routing_table_config: RoutingTableConfig) -> Self {
        let client = GrpcClient::new(
            ShardManagerServiceClient::new,
            routing_table_config.url(),
            Default::default(), // TODO
        );
        Self {
            cache: Cache::new(
                Some(1),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "routing_table",
            ),
            client,
        }
    }
}

#[async_trait]
impl RoutingTableService for RoutingTableServiceDefault {
    async fn get_routing_table(&self) -> Result<RoutingTable, RoutingTableError> {
        let client = self.client.clone();
        self.cache
            .get_or_insert_simple(&(), || {
                Box::pin(async move {
                    let response =
                        client.call(|client| Box::pin(client
                        .get_routing_table(shardmanager::GetRoutingTableRequest {})))
                        .await
                        .map_err(|err| {
                            RoutingTableError::unexpected(format!(
                                "Getting routing table from shard manager failed with {}",
                                err
                            ))
                        })?;
                    match response.into_inner() {
                        shardmanager::GetRoutingTableResponse {
                            result:
                            Some(shardmanager::get_routing_table_response::Result::Success(routing_table)),
                        } => Ok(routing_table.into()),
                        shardmanager::GetRoutingTableResponse {
                            result: Some(shardmanager::get_routing_table_response::Result::Failure(failure)),
                        } => Err(RoutingTableError::unexpected(format!(
                            "Getting routing table from shard manager failed with shard manager error {:?}",
                            failure
                        ))),
                        shardmanager::GetRoutingTableResponse { result: None } => {
                            Err(RoutingTableError::unexpected(
                                "Getting routing table from shard manager failed with unknown error",
                            ))
                        }
                    }
                })
            })
            .await
    }

    async fn invalidate_routing_table(&self) {
        self.cache.remove(&());
    }
}

pub struct RoutingTableServiceNoop {}

#[async_trait]
impl RoutingTableService for RoutingTableServiceNoop {
    async fn get_routing_table(&self) -> Result<RoutingTable, RoutingTableError> {
        Err(RoutingTableError::unexpected(
            "Routing table service is not configured",
        ))
    }

    async fn invalidate_routing_table(&self) {}
}
