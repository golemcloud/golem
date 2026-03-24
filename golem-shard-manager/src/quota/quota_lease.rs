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

use crate::model::Pod;
use golem_common::model::resource_definition::{
    EnforcementAction, ResourceDefinitionId, ResourceLimit,
};
use golem_service_base::model::quota_lease::{LeaseEpoch, QuotaAllocation};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum QuotaLease {
    Bounded {
        resource_definition_id: ResourceDefinitionId,
        pod: Pod,
        epoch: LeaseEpoch,
        allocation: QuotaAllocation,
        expires_after: Duration,
        resource_limit: ResourceLimit,
        enforcement_action: EnforcementAction,
    },
    Unlimited {
        pod: Pod,
        expires_after: Duration,
    },
}

impl From<QuotaLease> for golem_api_grpc::proto::golem::registry::QuotaLease {
    fn from(value: QuotaLease) -> Self {
        use golem_api_grpc::proto::golem::registry::{
            quota_allocation, quota_lease, BoundedQuotaLease, UnlimitedQuotaLease,
        };

        match value {
            QuotaLease::Bounded {
                resource_definition_id,
                pod,
                epoch,
                allocation,
                expires_after,
                resource_limit,
                enforcement_action,
            } => {
                let grpc_allocation = match allocation {
                    QuotaAllocation::Budget { amount } => {
                        golem_api_grpc::proto::golem::registry::QuotaAllocation {
                            kind: Some(quota_allocation::Kind::Budget(quota_allocation::Budget {
                                amount,
                            })),
                        }
                    }
                    QuotaAllocation::Exhausted { retry_after } => {
                        golem_api_grpc::proto::golem::registry::QuotaAllocation {
                            kind: Some(quota_allocation::Kind::Exhausted(
                                quota_allocation::Exhausted {
                                    retry_after_nanos: retry_after.as_nanos() as u64,
                                },
                            )),
                        }
                    }
                };
                Self {
                    kind: Some(quota_lease::Kind::Bounded(BoundedQuotaLease {
                        resource_definition_id: Some(resource_definition_id.into()),
                        pod: Some(pod.into()),
                        epoch: epoch.0,
                        allocation: Some(grpc_allocation),
                        expires_after_nanos: expires_after.as_nanos() as u64,
                        resource_limit: Some(resource_limit.into()),
                        enforcement_action:
                            golem_api_grpc::proto::golem::registry::EnforcementAction::from(
                                enforcement_action,
                            )
                            .into(),
                    })),
                }
            }
            QuotaLease::Unlimited { pod, expires_after } => Self {
                kind: Some(quota_lease::Kind::Unlimited(UnlimitedQuotaLease {
                    pod: Some(pod.into()),
                    expires_after_nanos: expires_after.as_nanos() as u64,
                })),
            },
        }
    }
}
