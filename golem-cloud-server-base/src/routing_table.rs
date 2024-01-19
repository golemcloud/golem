use crate::model::RoutingTable;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::shard_manager_service_client;
use golem_common::cache::*;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Clone)]
pub enum RoutingTableError {
    Unexpected(String),
}

#[derive(Clone, Debug, Deserialize)]
pub struct RoutingTableConfig {
    host: String,
    port: u16,
}

impl RoutingTableConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse shard manager URL")
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

pub struct RoutingTableServiceDefault {
    cache: Cache<(), (), RoutingTable, RoutingTableError>,
    routing_table_config: RoutingTableConfig,
}

impl RoutingTableServiceDefault {
    pub fn new(routing_table_config: RoutingTableConfig) -> Self {
        Self {
            cache: Cache::new(
                Some(1),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "routing_table",
            ),
            routing_table_config,
        }
    }
}

#[async_trait]
impl RoutingTableService for RoutingTableServiceDefault {
    async fn get_routing_table(&self) -> Result<RoutingTable, RoutingTableError> {
        let uri: hyper::Uri = self.routing_table_config.url().to_string().parse().unwrap();
        self.cache
            .get_or_insert_simple(&(), || {
                Box::pin(async move {
                    let mut shard_manager_client =
                        shard_manager_service_client::ShardManagerServiceClient::connect(uri)
                            .await
                            .map_err(|err| {
                                RoutingTableError::unexpected(format!("Connecting to shard manager failed with {}", err))
                            })?;
                    let response = shard_manager_client
                        .get_routing_table(shardmanager::GetRoutingTableRequest {})
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
