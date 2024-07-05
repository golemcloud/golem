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

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::error::ShardManagerError;
use crate::model::{pod_shard_assignments_to_string, Assignments, Pod, Unassignments};
use crate::shard_manager_config::WorkerExecutorServiceConfig;
use async_trait::async_trait;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::client::{GrpcClientConfig, MultiTargetGrpcClient};
use golem_common::model::ShardId;
use tokio::time::timeout;
use tonic::transport::Channel;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;
use tracing::{debug, info, warn};

#[async_trait]
pub trait WorkerExecutorService {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError>;

    async fn health_check(&self, pod: &Pod) -> bool;

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError>;
}

/// Sends revoke requests to all worker executors based on an `Unassignments` plan
pub async fn revoke_shards(
    worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
    unassignments: &Unassignments,
) -> Vec<(Pod, BTreeSet<ShardId>)> {
    let futures: Vec<_> = unassignments
        .unassignments
        .iter()
        .map(|(pod, shard_ids)| {
            let worker_executor = worker_executors.clone();
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

/// Sends assign requests to all worker executors based on an `Assignments` plan
pub async fn assign_shards(
    worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
    assignments: &Assignments,
) -> Vec<(Pod, BTreeSet<ShardId>)> {
    let futures: Vec<_> = assignments
        .assignments
        .iter()
        .map(|(pod, shard_ids)| {
            let instance_server_service = worker_executors.clone();
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

pub struct WorkerExecutorServiceDefault {
    config: WorkerExecutorServiceConfig,
    client: MultiTargetGrpcClient<WorkerExecutorClient<Channel>>,
}

#[async_trait]
impl WorkerExecutorService for WorkerExecutorServiceDefault {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!(
            "Assigning shards: {}",
            pod_shard_assignments_to_string(pod, shard_ids.iter())
        );

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

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!(
            "Revoking shards: {}",
            pod_shard_assignments_to_string(pod, shard_ids.iter())
        );

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

    async fn health_check(&self, pod: &Pod) -> bool {
        debug!("Health checking pod {pod}");
        let endpoint = pod.endpoint();
        let conn = timeout(self.config.health_check_timeout, endpoint.connect()).await;
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
}

impl WorkerExecutorServiceDefault {
    pub fn new(config: WorkerExecutorServiceConfig) -> Self {
        let client = MultiTargetGrpcClient::new(
            WorkerExecutorClient::new,
            GrpcClientConfig {
                retries_on_unavailable: config.retries.clone(),
                ..Default::default() // TODO: configure
            },
        );
        Self { config, client }
    }

    async fn assign_shards_internal(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        let assign_shards_request = golem::workerexecutor::AssignShardsRequest {
            shard_ids: shard_ids
                .clone()
                .into_iter()
                .map(|shard_id| shard_id.into())
                .collect(),
        };

        let assign_shards_response = timeout(
            self.config.assign_shards_timeout,
            self.client.call(pod.uri(), move |client| {
                let assign_shards_request = assign_shards_request.clone();
                Box::pin(client.assign_shards(assign_shards_request))
            }),
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

    async fn revoke_shards_internal(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        let revoke_shards_request = golem::workerexecutor::RevokeShardsRequest {
            shard_ids: shard_ids
                .clone()
                .into_iter()
                .map(|shard_id| shard_id.into())
                .collect(),
        };

        let revoke_shards_response = timeout(
            self.config.revoke_shards_timeout,
            self.client.call(pod.uri(), move |client| {
                let revoke_shards_request = revoke_shards_request.clone();
                Box::pin(client.revoke_shards(revoke_shards_request))
            }),
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
