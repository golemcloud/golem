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

use crate::RoutingTable;
use crate::error::ShardManagerTraceErrorKind;
use crate::quota::QuotaService;
use crate::sharding::error::ShardManagerError;
use crate::sharding::shard_management::ShardManagement;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::ShardManagerService;
use golem_common::model::Pod;
use golem_common::recorded_grpc_api_request;
use std::net::IpAddr;
use std::num::TryFromIntError;
use std::sync::Arc;
use tonic::Response;
use tracing::{Instrument, debug};

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

    async fn get_routing_table_internal(&self) -> RoutingTable {
        let routing_table = self.shard_management.current_snapshot().await;
        debug!("Providing routing table: {}", routing_table);
        routing_table
    }

    async fn register_internal(
        &self,
        pod: Pod,
        pod_name: Option<String>,
    ) -> Result<(), ShardManagerError> {
        debug!("Received request to register pod: {}", pod);
        self.shard_management.register_pod(pod, pod_name).await;
        Ok(())
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

        let record = recorded_grpc_api_request!(
            "register",
            source_ip = source_ip.to_string(),
            port = &request.port.to_string(),
            pod_name = request.pod_name(),
        );

        let pod = make_pod(source_ip, request.port)?;

        let response = self
            .register_internal(pod, request.pod_name)
            .instrument(record.span.clone())
            .await;

        let result = match response {
            Ok(_) => record.succeed(golem::shardmanager::v1::register_response::Result::Success(
                golem::shardmanager::v1::RegisterSuccess {
                    number_of_shards: self.get_routing_table_internal().await.number_of_shards
                        as u32,
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

        let name = golem_common::model::resource_definition::ResourceName(request.resource_name);

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

        let epoch = golem_service_base::model::quota_lease::LeaseEpoch(request.epoch);

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

        let epoch = golem_service_base::model::quota_lease::LeaseEpoch(request.epoch);

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

fn make_pod(ip: IpAddr, port: i32) -> Result<Pod, tonic::Status> {
    Ok(Pod {
        ip,
        port: port
            .try_into()
            .map_err(|e: TryFromIntError| tonic::Status::invalid_argument(e.to_string()))?,
    })
}
