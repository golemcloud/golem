// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::error::{HealthCheckError, ShardManagerError};
use super::model::{
    Assignments, ExecutorAddrs, ExecutorId, Unassignments, shard_assignments_to_string,
};
use crate::config::WorkerExecutorServiceConfig;
use async_trait::async_trait;
use futures::future::BoxFuture;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::model::Pod;
use golem_common::model::ShardId;
use golem_common::retries::with_retriable_errors;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::grpc::client::MultiTargetGrpcClient;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::time::error::Elapsed;
use tokio::time::timeout;
use tonic::Response;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::{HealthCheckRequest, HealthCheckResponse};
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;
use tracing::{info, warn};

#[async_trait]
pub trait WorkerExecutorService: Send + Sync {
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

    async fn set_shard_assignment(
        &self,
        pod: &Pod,
        number_of_shards: usize,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError>;
}

/// Sends revoke requests to all worker executors based on an `Unassignments` plan
pub async fn revoke_shards(
    worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
    unassignments: &Unassignments,
    addrs: &ExecutorAddrs,
) -> Vec<(ExecutorId, BTreeSet<ShardId>)> {
    fan_out(
        &unassignments.unassignments,
        addrs,
        "revoke_shards",
        |pod, shard_ids| {
            let worker_executors = worker_executors.clone();
            Box::pin(async move { worker_executors.revoke_shards(&pod, shard_ids).await })
        },
    )
    .await
}

/// Sends assign requests to all worker executors based on an `Assignments` plan.
pub async fn assign_shards(
    worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
    assignments: &Assignments,
    addrs: &ExecutorAddrs,
) -> Vec<(ExecutorId, BTreeSet<ShardId>)> {
    fan_out(
        &assignments.assignments,
        addrs,
        "assign_shards",
        |pod, shard_ids| {
            let worker_executors = worker_executors.clone();
            Box::pin(async move { worker_executors.assign_shards(&pod, shard_ids).await })
        },
    )
    .await
}

/// Reconciles executors to the authoritative shard assignments.
pub async fn set_shard_assignments(
    worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
    number_of_shards: usize,
    assignments: &Assignments,
    addrs: &ExecutorAddrs,
) -> Vec<(ExecutorId, BTreeSet<ShardId>)> {
    fan_out(
        &assignments.assignments,
        addrs,
        "set_shard_assignment",
        |pod, shard_ids| {
            let worker_executors = worker_executors.clone();
            Box::pin(async move {
                worker_executors
                    .set_shard_assignment(&pod, number_of_shards, shard_ids)
                    .await
            })
        },
    )
    .await
}

async fn fan_out<'a, F>(
    plan: &'a BTreeMap<ExecutorId, BTreeSet<ShardId>>,
    addrs: &ExecutorAddrs,
    operation: &'static str,
    call: F,
) -> Vec<(ExecutorId, BTreeSet<ShardId>)>
where
    F: Fn(Pod, &'a BTreeSet<ShardId>) -> BoxFuture<'a, Result<(), ShardManagerError>>,
{
    let futures: Vec<_> = plan
        .iter()
        .map(|(executor_id, shard_ids)| {
            let call = addrs
                .get(executor_id)
                .map(|addr| call(Pod::from(*addr), shard_ids));
            async move {
                match call {
                    None => {
                        warn!(
                            executor_id = %executor_id,
                            operation,
                            "Executor has no known address; reporting the operation as failed"
                        );
                        Some((*executor_id, shard_ids.clone()))
                    }
                    Some(call) => match call.await {
                        Ok(_) => None,
                        Err(_) => Some((*executor_id, shard_ids.clone())),
                    },
                }
            }
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
    client: MultiTargetGrpcClient<WorkerExecutorClient<OtelGrpcService<Channel>>>,
}

#[async_trait]
impl WorkerExecutorService for WorkerExecutorServiceDefault {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!(
            assigned_shards = shard_assignments_to_string(pod, None, shard_ids.iter()),
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
        let endpoint = pod.endpoint(self.config.client_config.tls_enabled());
        // The deadline covers the check RPC as well as the connect: an executor that accepts
        // connections but never answers would otherwise hold this call open forever.
        let checked = timeout(self.config.health_check_timeout, async {
            let conn = endpoint
                .connect()
                .await
                .map_err(HealthCheckError::GrpcTransportError)?;
            let request = HealthCheckRequest {
                service: "".to_string(),
            };
            let response = HealthClient::new(conn)
                .check(request)
                .await
                .map_err(HealthCheckError::GrpcError)?;
            let status = health_check_serving_status(response);
            (status == ServingStatus::Serving)
                .then_some(())
                .ok_or_else(|| HealthCheckError::GrpcOther(status.as_str_name()))
        })
        .await;

        match checked {
            Ok(result) => result,
            Err(_) => Err(HealthCheckError::GrpcOther("health check timeout")),
        }
    }

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!(
            revoked_shards = shard_assignments_to_string(pod, None, shard_ids.iter()),
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

    async fn set_shard_assignment(
        &self,
        pod: &Pod,
        number_of_shards: usize,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        info!(
            assigned_shards = shard_assignments_to_string(pod, None, shard_ids.iter()),
            number_of_shards, "Setting authoritative shard assignment",
        );

        with_retriable_errors(
            "worker_executor",
            "set_shard_assignment",
            Some(format!("{pod}")),
            &self.config.retries,
            &(pod, number_of_shards, shard_ids),
            |(pod, number_of_shards, shard_ids)| {
                Box::pin(self.set_shard_assignment_internal(pod, *number_of_shards, shard_ids))
            },
        )
        .await
    }
}

impl WorkerExecutorServiceDefault {
    pub fn new(config: WorkerExecutorServiceConfig) -> Self {
        let client = MultiTargetGrpcClient::new(
            "worker_executor",
            |channel, max_message_size| {
                WorkerExecutorClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(max_message_size)
                    .max_encoding_message_size(max_message_size)
            },
            config.client_config.clone(),
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
            self.client.call(
                "assign_shards",
                pod.uri(self.config.client_config.tls_enabled()),
                move |client| {
                    let assign_shards_request = assign_shards_request.clone();
                    Box::pin(client.assign_shards(assign_shards_request))
                },
            ),
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
                    .unwrap_or_else(WorkerExecutorError::unknown),
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
            self.client.call(
                "revoke_shards",
                pod.uri(self.config.client_config.tls_enabled()),
                move |client| {
                    let revoke_shards_request = revoke_shards_request.clone();
                    Box::pin(client.revoke_shards(revoke_shards_request))
                },
            ),
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
                    .unwrap_or_else(WorkerExecutorError::unknown),
            )),
            golem::workerexecutor::v1::RevokeShardsResponse { result: None } => {
                Err(ShardManagerError::NoResult)
            }
        }
    }

    async fn set_shard_assignment_internal(
        &self,
        pod: &Pod,
        number_of_shards: usize,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        let set_shard_assignment_request = golem::workerexecutor::v1::SetShardAssignmentRequest {
            number_of_shards: number_of_shards as u32,
            shard_ids: shard_ids
                .clone()
                .into_iter()
                .map(|shard_id| shard_id.into())
                .collect(),
        };

        let set_shard_assignment_response = timeout(
            self.config.assign_shards_timeout,
            self.client.call(
                "set_shard_assignment",
                pod.uri(self.config.client_config.tls_enabled()),
                move |client| {
                    let set_shard_assignment_request = set_shard_assignment_request.clone();
                    Box::pin(client.set_shard_assignment(set_shard_assignment_request))
                },
            ),
        )
        .await
        .map_err(|_: Elapsed| ShardManagerError::Timeout)?
        .map_err(ShardManagerError::GrpcError)?;

        match set_shard_assignment_response.into_inner() {
            golem::workerexecutor::v1::SetShardAssignmentResponse {
                result:
                    Some(golem::workerexecutor::v1::set_shard_assignment_response::Result::Success(_)),
            } => Ok(()),
            golem::workerexecutor::v1::SetShardAssignmentResponse {
                result:
                    Some(golem::workerexecutor::v1::set_shard_assignment_response::Result::Failure(
                        failure,
                    )),
            } => Err(ShardManagerError::WorkerExecutionError(
                failure
                    .try_into()
                    .unwrap_or_else(WorkerExecutorError::unknown),
            )),
            golem::workerexecutor::v1::SetShardAssignmentResponse { result: None } => {
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

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::config::WorkerExecutorServiceConfig;
    use std::time::Duration;
    use tokio::net::TcpListener;

    #[test]
    // The executor accepts the connection and then never answers, so only a deadline covering the
    // check RPC - not just the connect - lets this return.
    async fn a_health_check_against_a_silent_executor_gives_up() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local address");
        let _silent_executor = tokio::spawn(async move {
            let mut accepted = Vec::new();
            while let Ok((connection, _)) = listener.accept().await {
                // Held open and never answered.
                accepted.push(connection);
            }
        });

        let health_check_timeout = Duration::from_millis(300);
        let service = WorkerExecutorServiceDefault::new(WorkerExecutorServiceConfig {
            health_check_timeout,
            ..Default::default()
        });
        let pod = Pod {
            ip: addr.ip(),
            port: addr.port(),
        };

        let checked = timeout(health_check_timeout * 2, service.health_check(&pod))
            .await
            .expect("health_check outlived twice its own timeout");
        assert!(
            checked.is_err(),
            "an executor that never answers must not be reported healthy"
        );
    }
}
