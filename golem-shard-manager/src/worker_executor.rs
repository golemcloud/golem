use std::collections::HashSet;

use async_trait::async_trait;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::model::ShardId;
use tokio::time::timeout;
use tonic::transport::Uri;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;
use tracing::{debug, info, warn};

use crate::error::ShardManagerError;
use crate::model::Pod;
use crate::shard_manager_config::WorkerExecutorServiceConfig;

#[async_trait]
pub trait WorkerExecutorService {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &HashSet<ShardId>,
    ) -> Result<(), ShardManagerError>;

    async fn health_check(&self, pod: &Pod) -> bool;

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &HashSet<ShardId>,
    ) -> Result<(), ShardManagerError>;
}

pub struct WorkerExecutorServiceDefault {
    config: WorkerExecutorServiceConfig,
}

#[async_trait]
impl WorkerExecutorService for WorkerExecutorServiceDefault {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &HashSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!("Assigning shards {:?} to pod {:?}", shard_ids, pod);

        let retry_max_attempts = self.config.retries.max_attempts;
        let retry_min_delay = self.config.retries.min_delay;
        let retry_max_delay = self.config.retries.max_delay;
        let retry_multiplier = self.config.retries.multiplier;

        let mut attempts = 0;
        let mut delay = retry_min_delay;

        loop {
            match self.assign_shards_internal(pod, shard_ids).await {
                Ok(shard_ids) => return Ok(shard_ids),
                Err(e) => {
                    if attempts >= retry_max_attempts {
                        return Err(e);
                    }
                    tokio::time::sleep(delay).await;
                    attempts += 1;
                    delay = std::cmp::min(delay * retry_multiplier, retry_max_delay);
                }
            }
        }
    }

    async fn health_check(&self, pod: &Pod) -> bool {
        let retry_max_attempts = self.config.retries.max_attempts;
        let retry_min_delay = self.config.retries.min_delay;
        let retry_max_delay = self.config.retries.max_delay;
        let retry_multiplier = self.config.retries.multiplier;

        let mut attempts = 0;
        let mut delay = retry_min_delay;

        loop {
            match self.health_check_pod(pod).await {
                true => return true,
                false => {
                    if attempts >= retry_max_attempts {
                        debug!("Health check for {pod} failed {attempts}, marking as unhealthy");
                        return false;
                    }
                    tokio::time::sleep(delay).await;
                    attempts += 1;
                    delay = std::cmp::min(delay * retry_multiplier, retry_max_delay);
                }
            }
        }
    }

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &HashSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!("Revoking shards {:?} from pod {:?}", shard_ids, pod);

        let retry_max_attempts = self.config.retries.max_attempts;
        let retry_min_delay = self.config.retries.min_delay;
        let retry_max_delay = self.config.retries.max_delay;
        let retry_multiplier = self.config.retries.multiplier;

        let mut attempts = 0;
        let mut delay = retry_min_delay;

        loop {
            match self.revoke_shards_internal(pod, shard_ids).await {
                Ok(shard_ids) => return Ok(shard_ids),
                Err(e) => {
                    if attempts >= retry_max_attempts {
                        return Err(e);
                    }
                    tokio::time::sleep(delay).await;
                    attempts += 1;
                    delay = std::cmp::min(delay * retry_multiplier, retry_max_delay);
                }
            }
        }
    }
}

impl WorkerExecutorServiceDefault {
    pub fn new(config: WorkerExecutorServiceConfig) -> Self {
        Self { config }
    }

    async fn assign_shards_internal(
        &self,
        pod: &Pod,
        shard_ids: &HashSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        let assign_shards_request = golem::workerexecutor::AssignShardsRequest {
            shard_ids: shard_ids
                .clone()
                .into_iter()
                .map(|shard_id| shard_id.into())
                .collect(),
        };

        let mut worker_executor_client = WorkerExecutorClient::connect(pod.address())
            .await
            .map_err(|e| ShardManagerError::unknown(e.to_string()))?;

        let assign_shards_response = timeout(
            self.config.assign_shards_timeout,
            worker_executor_client.assign_shards(assign_shards_request),
        )
        .await
        .map_err(|e| ShardManagerError::unknown(e.to_string()))?
        .map_err(|_| ShardManagerError::timeout("assign_shards"))?;

        match assign_shards_response.into_inner() {
            golem::workerexecutor::AssignShardsResponse {
                result: Some(golem::workerexecutor::assign_shards_response::Result::Success(_)),
            } => Ok(()),
            golem::workerexecutor::AssignShardsResponse {
                result:
                    Some(golem::workerexecutor::assign_shards_response::Result::Failure(failure)),
            } => Err(ShardManagerError::unknown(format!(
                "unknown : {:#?}",
                failure
            ))),
            golem::workerexecutor::AssignShardsResponse { result: None } => {
                Err(ShardManagerError::unknown("unknown"))
            }
        }
    }

    async fn health_check_pod(&self, pod: &Pod) -> bool {
        debug!("Health checking pod {pod}");
        match pod.address().parse::<Uri>() {
            Ok(uri) => {
                let conn = timeout(
                    self.config.health_check_timeout,
                    tonic::transport::Endpoint::from(uri).connect(),
                )
                .await;
                match conn {
                    Ok(conn) => match conn {
                        Ok(conn) => {
                            let request = HealthCheckRequest {
                                service: "".to_string(),
                            };
                            match HealthClient::new(conn).check(request).await {
                                Ok(response) => {
                                    response.into_inner().status == ServingStatus::Serving as i32
                                }
                                Err(err) => {
                                    warn!("Health request returned with an error: {:?}", err);
                                    false
                                }
                            }
                        }
                        Err(err) => {
                            warn!("Failed to connect to pod {pod}: {:?}", err);
                            false
                        }
                    },
                    Err(_) => {
                        warn!("Connection to pod {pod} timed out");
                        false
                    }
                }
            }
            Err(_) => {
                warn!("Pod has an invalid URI: {pod}");
                false
            }
        }
    }

    async fn revoke_shards_internal(
        &self,
        pod: &Pod,
        shard_ids: &HashSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        let revoke_shards_request = golem::workerexecutor::RevokeShardsRequest {
            shard_ids: shard_ids
                .clone()
                .into_iter()
                .map(|shard_id| shard_id.into())
                .collect(),
        };

        let mut worker_executor_client = WorkerExecutorClient::connect(pod.address())
            .await
            .map_err(|e| ShardManagerError::unknown(e.to_string()))?;

        let revoke_shards_response = timeout(
            self.config.revoke_shards_timeout,
            worker_executor_client.revoke_shards(revoke_shards_request),
        )
        .await
        .map_err(|e| ShardManagerError::unknown(e.to_string()))?
        .map_err(|_| ShardManagerError::timeout("revoke_shards"))?;

        match revoke_shards_response.into_inner() {
            golem::workerexecutor::RevokeShardsResponse {
                result: Some(golem::workerexecutor::revoke_shards_response::Result::Success(_)),
            } => Ok(()),
            golem::workerexecutor::RevokeShardsResponse {
                result:
                    Some(golem::workerexecutor::revoke_shards_response::Result::Failure(failure)),
            } => Err(ShardManagerError::unknown(format!(
                "unknown : {:#?}",
                failure
            ))),
            golem::workerexecutor::RevokeShardsResponse { result: None } => {
                Err(ShardManagerError::unknown("unknown"))
            }
        }
    }
}
