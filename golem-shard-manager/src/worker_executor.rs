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

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::time::error::Elapsed;
use tokio::time::timeout;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Response;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::{HealthCheckRequest, HealthCheckResponse};
use tracing::info;

use crate::error::{HealthCheckError, ShardManagerError};
use crate::model::{pod_shard_assignments_to_string, Assignments, Pod, Unassignments};
use crate::shard_manager_config::WorkerExecutorServiceConfig;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::client::{GrpcClientConfig, MultiTargetGrpcClient};
use golem_common::model::error::{GolemError, GolemErrorUnknown};
use golem_common::model::ShardId;
use golem_common::retries::with_retriable_errors;

#[async_trait]
pub trait WorkerExecutorService {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError>;

    async fn health_check(&self, pod: &Pod) -> Result<(), HealthCheckError>;

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
            let worker_executors = worker_executors.clone();
            Box::pin(async move {
                match worker_executors.revoke_shards(pod, shard_ids).await {
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
            let worker_executors = worker_executors.clone();
            Box::pin(async move {
                match worker_executors.assign_shards(pod, shard_ids).await {
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
            assigned_shards = pod_shard_assignments_to_string(pod, shard_ids.iter()),
            "Assigning shards",
        );

        with_retriable_errors(
            "worker_executor",
            "assign_shards",
            Some(format!("{pod}")),
            &self.config.retries,
            &(pod, shard_ids),
            |(pod, shard_ids)| Box::pin(self.assign_shards_internal(pod, shard_ids)),
        )
        .await
    }

    async fn health_check(&self, pod: &Pod) -> Result<(), HealthCheckError> {
        // NOTE: retries are handled in healthcheck.rs
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
                            let status = health_check_serving_status(response);
                            (status == ServingStatus::Serving)
                                .then_some(())
                                .ok_or_else(|| HealthCheckError::GrpcOther(status.as_str_name()))
                        }
                        Err(status) => Err(HealthCheckError::GrpcError(status)),
                    }
                }
                Err(err) => Err(HealthCheckError::GrpcTransportError(err)),
            },
            Err(_) => Err(HealthCheckError::GrpcOther("connect timeout")),
        }
    }

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!(
            revoked_shards = pod_shard_assignments_to_string(pod, shard_ids.iter()),
            "Revoking shards",
        );

        with_retriable_errors(
            "worker_executor",
            "revoke_shards",
            Some(format!("{pod}")),
            &self.config.retries,
            &(pod, shard_ids),
            |(pod, shard_ids)| Box::pin(self.revoke_shards_internal(pod, shard_ids)),
        )
        .await
    }
}

impl WorkerExecutorServiceDefault {
    pub fn new(config: WorkerExecutorServiceConfig) -> Self {
        let client = MultiTargetGrpcClient::new(
            "worker_executor",
            |channel| {
                WorkerExecutorClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            GrpcClientConfig {
                retries_on_unavailable: config.retries.clone(),
                connect_timeout: config.connect_timeout,
            },
        );
        Self { config, client }
    }

    async fn assign_shards_internal(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        let assign_shards_request = golem::workerexecutor::v1::AssignShardsRequest {
            shard_ids: shard_ids
                .clone()
                .into_iter()
                .map(|shard_id| shard_id.into())
                .collect(),
        };

        let assign_shards_response = timeout(
            self.config.assign_shards_timeout,
            self.client.call("assign_shards", pod.uri(), move |client| {
                let assign_shards_request = assign_shards_request.clone();
                Box::pin(client.assign_shards(assign_shards_request))
            }),
        )
        .await
        .map_err(|_: Elapsed| ShardManagerError::Timeout)?
        .map_err(ShardManagerError::GrpcError)?;

        match assign_shards_response.into_inner() {
            golem::workerexecutor::v1::AssignShardsResponse {
                result: Some(golem::workerexecutor::v1::assign_shards_response::Result::Success(_)),
            } => Ok(()),
            golem::workerexecutor::v1::AssignShardsResponse {
                result:
                    Some(golem::workerexecutor::v1::assign_shards_response::Result::Failure(failure)),
            } => Err(ShardManagerError::WorkerExecutionError(
                failure
                    .try_into()
                    .unwrap_or_else(|err| GolemError::Unknown(GolemErrorUnknown { details: err })),
            )),
            golem::workerexecutor::v1::AssignShardsResponse { result: None } => {
                Err(ShardManagerError::NoResult)
            }
        }
    }

    async fn revoke_shards_internal(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        let revoke_shards_request = golem::workerexecutor::v1::RevokeShardsRequest {
            shard_ids: shard_ids
                .clone()
                .into_iter()
                .map(|shard_id| shard_id.into())
                .collect(),
        };

        let revoke_shards_response = timeout(
            self.config.revoke_shards_timeout,
            self.client.call("revoke_shards", pod.uri(), move |client| {
                let revoke_shards_request = revoke_shards_request.clone();
                Box::pin(client.revoke_shards(revoke_shards_request))
            }),
        )
        .await
        .map_err(|_: Elapsed| ShardManagerError::Timeout)?
        .map_err(ShardManagerError::GrpcError)?;

        match revoke_shards_response.into_inner() {
            golem::workerexecutor::v1::RevokeShardsResponse {
                result: Some(golem::workerexecutor::v1::revoke_shards_response::Result::Success(_)),
            } => Ok(()),
            golem::workerexecutor::v1::RevokeShardsResponse {
                result:
                    Some(golem::workerexecutor::v1::revoke_shards_response::Result::Failure(failure)),
            } => Err(ShardManagerError::WorkerExecutionError(
                failure
                    .try_into()
                    .unwrap_or_else(|err| GolemError::Unknown(GolemErrorUnknown { details: err })),
            )),
            golem::workerexecutor::v1::RevokeShardsResponse { result: None } => {
                Err(ShardManagerError::NoResult)
            }
        }
    }
}

fn health_check_serving_status(response: Response<HealthCheckResponse>) -> ServingStatus {
    response
        .into_inner()
        .status
        .try_into()
        .unwrap_or(ServingStatus::Unknown)
}
