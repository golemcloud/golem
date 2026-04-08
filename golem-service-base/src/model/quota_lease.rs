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

use chrono::{DateTime, Utc};
use golem_common::model::Pod;
use golem_common::model::quota::LeaseEpoch;
use golem_common::model::quota::{EnforcementAction, ResourceDefinitionId, ResourceLimit};

/// A lease granted by the shard manager to a worker executor.
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaLease {
    /// The resource definition exists and the executor has a tracked lease.
    Bounded {
        resource_definition_id: ResourceDefinitionId,
        pod: Pod,
        epoch: LeaseEpoch,
        allocation: u64,
        expires_at: DateTime<Utc>,
        resource_limit: ResourceLimit,
        enforcement_action: EnforcementAction,
        /// Total available capacity for this resource across all executors
        /// (remaining pool + all current lease allocations).
        total_available_amount: u64,
    },
    /// The resource definition does not exist or was deleted.
    /// The executor may use unlimited capacity but must renew
    /// to detect if a limit is later imposed.
    Unlimited { pod: Pod, expires_at: DateTime<Utc> },
}

/// A pending reservation request from an agent on an executor.
/// Sent to the shard manager during lease renewal so it can weight
/// allocations across executors based on demand.
#[derive(Debug, Clone, desert_rust::BinaryCodec)]
pub struct PendingReservation {
    pub amount: u64,
    pub priority: f64,
}

mod protobuf {
    use super::*;
    use applying::Apply;
    use golem_api_grpc::proto::golem::common::{QuotaLease as GrpcQuotaLease, quota_lease};
    use std::time::SystemTime;

    impl TryFrom<GrpcQuotaLease> for QuotaLease {
        type Error = String;

        fn try_from(value: GrpcQuotaLease) -> Result<Self, Self::Error> {
            match value.kind.ok_or("QuotaLease.kind missing")? {
                quota_lease::Kind::Bounded(b) => {
                    let pod: Pod = b.pod.ok_or("BoundedQuotaLease.pod missing")?.try_into()?;
                    let resource_definition_id = b
                        .resource_definition_id
                        .ok_or("BoundedQuotaLease.resource_definition_id missing")?
                        .try_into()?;

                    let expires_at = b
                        .expires_at
                        .ok_or("missing expires_at")?
                        .apply(SystemTime::try_from)
                        .map_err(|_| "Failed to convert timestamp".to_string())?
                        .into();

                    let resource_limit = b
                        .resource_limit
                        .ok_or("BoundedQuotaLease.resource_limit missing")?
                        .try_into()?;

                    let enforcement_action =
                        golem_api_grpc::proto::golem::common::EnforcementAction::try_from(
                            b.enforcement_action,
                        )
                        .map_err(|_| "Unknown enforcement_action value")?
                        .try_into()?;

                    Ok(Self::Bounded {
                        resource_definition_id,
                        pod,
                        epoch: LeaseEpoch(b.epoch),
                        allocation: b.allocation,
                        expires_at,
                        resource_limit,
                        enforcement_action,
                        total_available_amount: b.total_available_amount,
                    })
                }
                quota_lease::Kind::Unlimited(u) => {
                    let pod: Pod = u.pod.ok_or("UnlimitedQuotaLease.pod missing")?.try_into()?;

                    let expires_at = u
                        .expires_at
                        .ok_or("missing expires_at")?
                        .apply(SystemTime::try_from)
                        .map_err(|_| "Failed to convert timestamp".to_string())?
                        .into();

                    Ok(Self::Unlimited { pod, expires_at })
                }
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::shardmanager::v1::PendingReservation>
        for PendingReservation
    {
        fn from(value: golem_api_grpc::proto::golem::shardmanager::v1::PendingReservation) -> Self {
            Self {
                amount: value.amount,
                priority: value.priority,
            }
        }
    }

    impl From<PendingReservation>
        for golem_api_grpc::proto::golem::shardmanager::v1::PendingReservation
    {
        fn from(value: PendingReservation) -> Self {
            Self {
                amount: value.amount,
                priority: value.priority,
            }
        }
    }
}
