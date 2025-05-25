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

use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use http::Uri;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Status;

use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_error::Error;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_client::ShardManagerServiceClient;
use golem_api_grpc::proto::golem::shardmanager::v1::ShardManagerError;
use golem_common::cache::*;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{RetryConfig, RoutingTable};
use golem_common::retriable_error::IsRetriableError;

#[derive(Debug, Clone)]
pub enum RoutingTableError {
    ShardManagerGrpcError(Status),
    ShardManagerError(ShardManagerError),
    NoResult,
}

impl Display for RoutingTableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            RoutingTableError::ShardManagerGrpcError(status) => {
                write!(
                    f,
                    "Shard Manager gRPC error: {} {}",
                    status.code(),
                    status.message()
                )
            }
            RoutingTableError::ShardManagerError(error) => {
                let detail = match &error.error {
                    Some(error) => match error {
                        Error::InvalidRequest(e) => format!("Invalid Request: {}", e.error),
                        Error::Timeout(e) => format!("Timeout: {}", e.error),
                        Error::Unknown(e) => format!("Unknown: {}", e.error),
                    },
                    None => "Unknown".to_string(),
                };

                write!(f, "Shard Manager error: {}", detail)
            }
            RoutingTableError::NoResult => write!(f, "No Result"),
        }
    }
}

impl IsRetriableError for RoutingTableError {
    fn is_retriable(&self) -> bool {
        match &self {
            RoutingTableError::ShardManagerGrpcError(status) => status.is_retriable(),
            RoutingTableError::ShardManagerError(error) => match &error.error {
                Some(error) => match error {
                    Error::InvalidRequest(_) => false,
                    Error::Timeout(_) => true,
                    Error::Unknown(_) => true,
                },
                None => true,
            },
            RoutingTableError::NoResult => true,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingTableConfig {
    pub host: String,
    pub port: u16,
    #[serde(with = "humantime_serde")]
    pub invalidation_min_delay: Duration,
    pub retries: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
}

impl RoutingTableConfig {
    pub fn url(&self) -> Uri {
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
            invalidation_min_delay: Duration::from_millis(500),
            retries: RetryConfig::default(),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

#[async_trait]
pub trait RoutingTableService {
    async fn get_routing_table(&self) -> Result<RoutingTable, RoutingTableError>;
    // Returns false in case of skipped (throttled) invalidation
    async fn try_invalidate_routing_table(&self) -> bool;
}

pub trait HasRoutingTableService {
    fn routing_table_service(&self) -> &Arc<dyn RoutingTableService + Send + Sync>;
}

pub struct RoutingTableServiceDefault {
    config: RoutingTableConfig,
    cache: Cache<(), (), RoutingTable, RoutingTableError>,
    last_invalidated_at: RwLock<Option<Instant>>,
    client: GrpcClient<ShardManagerServiceClient<Channel>>,
}

impl RoutingTableServiceDefault {
    pub fn new(config: RoutingTableConfig) -> Self {
        let client = GrpcClient::new(
            "shard-manager",
            |channel| {
                ShardManagerServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            config.url(),
            GrpcClientConfig {
                retries_on_unavailable: config.retries.clone(),
                connect_timeout: config.connect_timeout,
            },
        );
        Self {
            config,
            cache: Cache::new(
                Some(1),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "routing_table",
            ),
            last_invalidated_at: RwLock::new(None),
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
                    let response = client
                        .call("get_routing_table", |client| {
                            Box::pin(
                                client
                                    .get_routing_table(shardmanager::v1::GetRoutingTableRequest {}),
                            )
                        })
                        .await
                        .map_err(RoutingTableError::ShardManagerGrpcError)?;
                    match response.into_inner() {
                        shardmanager::v1::GetRoutingTableResponse {
                            result:
                                Some(shardmanager::v1::get_routing_table_response::Result::Success(
                                    routing_table,
                                )),
                        } => Ok(routing_table.into()),
                        shardmanager::v1::GetRoutingTableResponse {
                            result:
                                Some(shardmanager::v1::get_routing_table_response::Result::Failure(
                                    failure,
                                )),
                        } => Err(RoutingTableError::ShardManagerError(failure)),
                        shardmanager::v1::GetRoutingTableResponse { result: None } => {
                            Err(RoutingTableError::NoResult)
                        }
                    }
                })
            })
            .await
    }

    async fn try_invalidate_routing_table(&self) -> bool {
        let now = Instant::now();

        let skip_invalidate = |last_invalidated_at: &Option<Instant>| {
            matches!(
                last_invalidated_at,
                Some(last_invalidated_at)
                    if now.saturating_duration_since(last_invalidated_at.to_owned()) < self.config.invalidation_min_delay
            )
        };

        if skip_invalidate(self.last_invalidated_at.read().await.deref()) {
            return false;
        }

        let mut last_invalidated_at = self.last_invalidated_at.write().await;
        if skip_invalidate(last_invalidated_at.deref()) {
            return false;
        }
        self.cache.remove(&());
        *last_invalidated_at = Some(Instant::now());
        true
    }
}

pub struct RoutingTableServiceNoop {}

#[async_trait]
impl RoutingTableService for RoutingTableServiceNoop {
    async fn get_routing_table(&self) -> Result<RoutingTable, RoutingTableError> {
        Err(RoutingTableError::NoResult)
    }

    async fn try_invalidate_routing_table(&self) -> bool {
        return false;
    }
}
