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

use crate::ShardLeaseState;
use crate::error::ShardManagerTraceErrorKind;
use crate::quota::QuotaService;
use crate::sharding::error::ShardManagerError;
use crate::sharding::shard_management::ShardManagement;
use crate::sharding::{ExecutorAddr, ExecutorId, RegisterAck, ShardEpoch};
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::ShardManagerService;
use golem_common::model::{Pod, ShardId};
use golem_common::recorded_grpc_api_request;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::num::TryFromIntError;
use std::sync::Arc;
use std::time::SystemTime;
use tonic::Response;
use tracing::{Instrument, debug};
use uuid::Uuid;

pub struct ShardManagerServiceImpl {
    shard_management: Arc<ShardManagement>,
    quota_service: Arc<QuotaService>,
}

impl ShardManagerServiceImpl {
    pub fn new(shard_management: Arc<ShardManagement>, quota_service: Arc<QuotaService>) -> Self {
        Self {
            shard_management,
            quota_service,
        }
    }

    async fn get_routing_table_internal(&self) -> ShardLeaseState {
        let shard_state = self.shard_management.current_snapshot().await;
        debug!("Providing routing table: {}", shard_state);
        shard_state
    }

    async fn register_internal(
        &self,
        executor_id: ExecutorId,
        pod: Pod,
        pod_name: Option<String>,
    ) -> Result<RegisterAck, ShardManagerError> {
        debug!(executor_id = %executor_id, "Received request to register executor at: {}", pod);
        let ack = self
            .shard_management
            .register_executor(executor_id, ExecutorAddr::from(pod), pod_name)
            .await?;
        debug!(executor_id = %executor_id, addr = %pod, "Registered executor");
        Ok(ack)
    }
}

#[tonic::async_trait]
impl ShardManagerService for ShardManagerServiceImpl {
    async fn get_routing_table(
        &self,
        _request: tonic::Request<golem::shardmanager::v1::GetRoutingTableRequest>,
    ) -> Result<Response<golem::shardmanager::v1::GetRoutingTableResponse>, tonic::Status> {
        let record = recorded_grpc_api_request!("get_routing_table",);

        let response = self
            .get_routing_table_internal()
            .instrument(record.span.clone())
            .await;

        Ok(Response::new(
            golem::shardmanager::v1::GetRoutingTableResponse {
                result: Some(
                    golem::shardmanager::v1::get_routing_table_response::Result::Success(
                        response.into(),
                    ),
                ),
            },
        ))
    }

    async fn register(
        &self,
        request: tonic::Request<golem::shardmanager::v1::RegisterRequest>,
    ) -> Result<Response<golem::shardmanager::v1::RegisterResponse>, tonic::Status> {
        let source_ip = request
            .remote_addr()
            .ok_or_else(|| tonic::Status::invalid_argument("missing source IP"))?
            .ip();

        let request = request.into_inner();

        // Before anything touches the state: an executor that cannot name itself has no identity to
        // renew or deregister a lease with.
        let executor_id = parse_executor_id(&request.executor_id)?;

        let record = recorded_grpc_api_request!(
            "register",
            source_ip = source_ip.to_string(),
            port = &request.port.to_string(),
            pod_name = request.pod_name(),
            executor_id = executor_id.to_string(),
        );

        let pod = make_pod(source_ip, request.port)?;

        let response = self
            .register_internal(executor_id, pod, request.pod_name)
            .instrument(record.span.clone())
            .await;

        let result = match response {
            Ok(ack) => record.succeed(golem::shardmanager::v1::register_response::Result::Success(
                golem::shardmanager::v1::RegisterSuccess {
                    number_of_shards: ack.number_of_shards as u32,
                    shard_epochs: shard_epoch_entries(&ack.grant.shard_epochs),
                    expires_at: Some(prost_types::Timestamp::from(SystemTime::from(
                        ack.grant.expires_at,
                    ))),
                },
            )),
            Err(error) => {
                let error: golem::shardmanager::v1::ShardManagerError = error.into();
                record.fail(
                    golem::shardmanager::v1::register_response::Result::Failure(error.clone()),
                    &mut ShardManagerTraceErrorKind(&error),
                )
            }
        };

        Ok(Response::new(golem::shardmanager::v1::RegisterResponse {
            result: Some(result),
        }))
    }

    async fn renew_shard_lease(
        &self,
        request: tonic::Request<golem::shardmanager::v1::RenewShardLeaseRequest>,
    ) -> Result<Response<golem::shardmanager::v1::RenewShardLeaseResponse>, tonic::Status> {
        let request = request.into_inner();

        // Both before any state is touched: an executor that cannot name itself has no lease to
        // renew, and a claim the manager cannot decode is not one it can validate.
        let executor_id = parse_executor_id(&request.executor_id)?;
        let claimed = parse_shard_epochs(request.shard_epochs)?;

        let result = match self
            .shard_management
            .renew_shard_lease(executor_id, &claimed)
            .await
        {
            Ok(grant) => golem::shardmanager::v1::renew_shard_lease_response::Result::Success(
                golem::shardmanager::v1::ShardLease {
                    shard_epochs: shard_epoch_entries(&grant.shard_epochs),
                    expires_at: Some(prost_types::Timestamp::from(SystemTime::from(
                        grant.expires_at,
                    ))),
                },
            ),
            Err(error) => {
                golem::shardmanager::v1::renew_shard_lease_response::Result::Failure(error.into())
            }
        };

        Ok(Response::new(
            golem::shardmanager::v1::RenewShardLeaseResponse {
                result: Some(result),
            },
        ))
    }

    async fn deregister(
        &self,
        request: tonic::Request<golem::shardmanager::v1::DeregisterRequest>,
    ) -> Result<Response<golem::shardmanager::v1::DeregisterResponse>, tonic::Status> {
        let request = request.into_inner();

        let executor_id = parse_executor_id(&request.executor_id)?;
        let claimed = parse_shard_epochs(request.shard_epochs)?;

        let result = match self
            .shard_management
            .deregister_executor(executor_id, &claimed)
            .await
        {
            Ok(()) => golem::shardmanager::v1::deregister_response::Result::Success(
                golem::common::Empty {},
            ),
            Err(error) => {
                golem::shardmanager::v1::deregister_response::Result::Failure(error.into())
            }
        };

        Ok(Response::new(golem::shardmanager::v1::DeregisterResponse {
            result: Some(result),
        }))
    }

    async fn acquire_quota_lease(
        &self,
        request: tonic::Request<golem::shardmanager::v1::AcquireQuotaLeaseRequest>,
    ) -> Result<Response<golem::shardmanager::v1::AcquireQuotaLeaseResponse>, tonic::Status> {
        let source_ip = request
            .remote_addr()
            .ok_or_else(|| tonic::Status::invalid_argument("missing source IP"))?
            .ip();

        let request = request.into_inner();

        let environment_id = request
            .environment_id
            .ok_or_else(|| tonic::Status::invalid_argument("missing environment_id"))?
            .try_into()
            .map_err(|e: String| tonic::Status::invalid_argument(e))?;

        let pod = make_pod(source_ip, request.port)?;

        let name = golem_common::model::quota::ResourceName(request.resource_name);

        match self
            .quota_service
            .acquire_lease(environment_id, name, pod)
            .await
        {
            Ok(lease) => {
                let grpc_lease: golem_api_grpc::proto::golem::common::QuotaLease = lease.into();
                Ok(Response::new(
                    golem::shardmanager::v1::AcquireQuotaLeaseResponse {
                        result: Some(
                            golem::shardmanager::v1::acquire_quota_lease_response::Result::Success(
                                golem::shardmanager::v1::AcquireQuotaLeaseSuccessResponse {
                                    lease: Some(grpc_lease),
                                },
                            ),
                        ),
                    },
                ))
            }
            Err(err) => Ok(Response::new(
                golem::shardmanager::v1::AcquireQuotaLeaseResponse {
                    result: Some(
                        golem::shardmanager::v1::acquire_quota_lease_response::Result::Error(
                            err.into(),
                        ),
                    ),
                },
            )),
        }
    }

    async fn renew_quota_lease(
        &self,
        request: tonic::Request<golem::shardmanager::v1::RenewQuotaLeaseRequest>,
    ) -> Result<Response<golem::shardmanager::v1::RenewQuotaLeaseResponse>, tonic::Status> {
        let source_ip = request
            .remote_addr()
            .ok_or_else(|| tonic::Status::invalid_argument("missing source IP"))?
            .ip();
        let request = request.into_inner();

        let resource_definition_id = request
            .resource_definition_id
            .ok_or_else(|| tonic::Status::invalid_argument("missing resource_definition_id"))?
            .try_into()
            .map_err(|e: String| tonic::Status::invalid_argument(e))?;

        let pod = make_pod(source_ip, request.port)?;

        let epoch = golem_common::model::quota::LeaseEpoch(request.epoch);

        let pending_reservations = request
            .pending_reservations
            .into_iter()
            .map(Into::into)
            .collect();

        match self
            .quota_service
            .renew_lease(
                resource_definition_id,
                pod,
                epoch,
                request.unused,
                pending_reservations,
            )
            .await
        {
            Ok(lease) => {
                let grpc_lease: golem_api_grpc::proto::golem::common::QuotaLease = lease.into();
                Ok(Response::new(
                    golem::shardmanager::v1::RenewQuotaLeaseResponse {
                        result: Some(
                            golem::shardmanager::v1::renew_quota_lease_response::Result::Success(
                                golem::shardmanager::v1::RenewQuotaLeaseSuccessResponse {
                                    lease: Some(grpc_lease),
                                },
                            ),
                        ),
                    },
                ))
            }
            Err(err) => Ok(Response::new(
                golem::shardmanager::v1::RenewQuotaLeaseResponse {
                    result: Some(
                        golem::shardmanager::v1::renew_quota_lease_response::Result::Error(
                            err.into(),
                        ),
                    ),
                },
            )),
        }
    }

    async fn batch_renew_quota_leases(
        &self,
        request: tonic::Request<golem::shardmanager::v1::BatchRenewQuotaLeasesRequest>,
    ) -> Result<Response<golem::shardmanager::v1::BatchRenewQuotaLeasesResponse>, tonic::Status>
    {
        let source_ip = request
            .remote_addr()
            .ok_or_else(|| tonic::Status::invalid_argument("missing source IP"))?
            .ip();
        let request = request.into_inner();

        let mut renewals = Vec::with_capacity(request.renewals.len());
        for r in request.renewals {
            let resource_definition_id = r
                .resource_definition_id
                .ok_or_else(|| tonic::Status::invalid_argument("missing resource_definition_id"))?
                .try_into()
                .map_err(|e: String| tonic::Status::invalid_argument(e))?;
            let pod = make_pod(source_ip, r.port)?;
            let epoch = golem_common::model::quota::LeaseEpoch(r.epoch);
            let pending_reservations = r.pending_reservations.into_iter().map(Into::into).collect();
            renewals.push((
                resource_definition_id,
                pod,
                epoch,
                r.unused,
                pending_reservations,
            ));
        }

        let lease_results = self.quota_service.batch_renew_leases(renewals).await;

        let results = lease_results
            .into_iter()
            .map(|result| match result {
                Ok(lease) => {
                    let grpc_lease: golem_api_grpc::proto::golem::common::QuotaLease = lease.into();
                    golem::shardmanager::v1::RenewQuotaLeaseResult {
                        result: Some(
                            golem::shardmanager::v1::renew_quota_lease_result::Result::Success(
                                golem::shardmanager::v1::RenewQuotaLeaseSuccessResponse {
                                    lease: Some(grpc_lease),
                                },
                            ),
                        ),
                    }
                }
                Err(err) => golem::shardmanager::v1::RenewQuotaLeaseResult {
                    result: Some(
                        golem::shardmanager::v1::renew_quota_lease_result::Result::Error(
                            err.into(),
                        ),
                    ),
                },
            })
            .collect();

        Ok(Response::new(
            golem::shardmanager::v1::BatchRenewQuotaLeasesResponse { results },
        ))
    }

    async fn release_quota_lease(
        &self,
        request: tonic::Request<golem::shardmanager::v1::ReleaseQuotaLeaseRequest>,
    ) -> Result<Response<golem::shardmanager::v1::ReleaseQuotaLeaseResponse>, tonic::Status> {
        let source_ip = request
            .remote_addr()
            .ok_or_else(|| tonic::Status::invalid_argument("missing source IP"))?
            .ip();
        let request = request.into_inner();

        let resource_definition_id = request
            .resource_definition_id
            .ok_or_else(|| tonic::Status::invalid_argument("missing resource_definition_id"))?
            .try_into()
            .map_err(|e: String| tonic::Status::invalid_argument(e))?;

        let pod = make_pod(source_ip, request.port)?;

        let epoch = golem_common::model::quota::LeaseEpoch(request.epoch);

        match self
            .quota_service
            .release_lease(resource_definition_id, pod, epoch, request.unused)
            .await
        {
            Ok(()) => Ok(Response::new(
                golem::shardmanager::v1::ReleaseQuotaLeaseResponse {
                    result: Some(
                        golem::shardmanager::v1::release_quota_lease_response::Result::Success(
                            golem::shardmanager::v1::ReleaseQuotaLeaseSuccessResponse {},
                        ),
                    ),
                },
            )),
            Err(err) => Ok(Response::new(
                golem::shardmanager::v1::ReleaseQuotaLeaseResponse {
                    result: Some(
                        golem::shardmanager::v1::release_quota_lease_response::Result::Error(
                            err.into(),
                        ),
                    ),
                },
            )),
        }
    }
}

/// The executor-generated UUID that identifies a shard lease. Empty or malformed is a client
/// error, refused before any state is touched.
fn parse_executor_id(raw: &str) -> Result<ExecutorId, tonic::Status> {
    if raw.is_empty() {
        return Err(tonic::Status::invalid_argument("missing executor_id"));
    }
    Uuid::parse_str(raw)
        .map(ExecutorId)
        .map_err(|err| tonic::Status::invalid_argument(format!("invalid executor_id: {err}")))
}

/// The shard set an executor claims, decoded from the wire.
///
/// A `ShardEpochEntry` without a shard id names no shard, so it cannot be validated against
/// anything: that is a malformed request, refused before any state is touched rather than silently
/// dropped from a claim whose whole point is to be exact.
fn parse_shard_epochs(
    entries: Vec<golem::shardmanager::ShardEpochEntry>,
) -> Result<BTreeMap<ShardId, ShardEpoch>, tonic::Status> {
    entries
        .into_iter()
        .map(|entry| {
            let shard_id: ShardId = entry
                .shard_id
                .ok_or_else(|| tonic::Status::invalid_argument("missing shard_id"))?
                .into();
            Ok((shard_id, ShardEpoch(entry.epoch)))
        })
        .collect()
}

fn shard_epoch_entries(
    shard_epochs: &BTreeMap<ShardId, ShardEpoch>,
) -> Vec<golem::shardmanager::ShardEpochEntry> {
    shard_epochs
        .iter()
        .map(|(shard_id, epoch)| golem::shardmanager::ShardEpochEntry {
            shard_id: Some((*shard_id).into()),
            epoch: epoch.0,
        })
        .collect()
}

fn make_pod(ip: IpAddr, port: i32) -> Result<Pod, tonic::Status> {
    Ok(Pod {
        ip,
        port: port
            .try_into()
            .map_err(|e: TryFromIntError| tonic::Status::invalid_argument(e.to_string()))?,
    })
}
