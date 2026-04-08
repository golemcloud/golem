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
use golem_common::model::quota::{
    EnforcementAction, LeaseEpoch, ResourceDefinitionId, ResourceLimit,
};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum QuotaLease {
    Bounded {
        resource_definition_id: ResourceDefinitionId,
        pod: Pod,
        epoch: LeaseEpoch,
        allocated_amount: u64,
        expires_at: DateTime<Utc>,
        resource_limit: ResourceLimit,
        enforcement_action: EnforcementAction,
        total_available_amount: u64,
    },
    Unlimited {
        pod: Pod,
        expires_at: DateTime<Utc>,
    },
}

impl QuotaLease {
    pub fn allocation(&self) -> u64 {
        match self {
            QuotaLease::Bounded {
                allocated_amount, ..
            } => *allocated_amount,
            QuotaLease::Unlimited { .. } => 0,
        }
    }
}

impl From<QuotaLease> for golem_api_grpc::proto::golem::common::QuotaLease {
    fn from(value: QuotaLease) -> Self {
        use golem_api_grpc::proto::golem::common::quota_lease;

        match value {
            QuotaLease::Bounded {
                resource_definition_id,
                pod,
                epoch,
                allocated_amount,
                expires_at,
                resource_limit,
                enforcement_action,
                total_available_amount,
            } => Self {
                kind: Some(quota_lease::Kind::Bounded(quota_lease::Bounded {
                    resource_definition_id: Some(resource_definition_id.into()),
                    pod: Some(pod.into()),
                    epoch: epoch.0,
                    allocation: allocated_amount,
                    expires_at: Some(prost_types::Timestamp::from(SystemTime::from(expires_at))),
                    resource_limit: Some(resource_limit.into()),
                    enforcement_action:
                        golem_api_grpc::proto::golem::common::EnforcementAction::from(
                            enforcement_action,
                        )
                        .into(),
                    total_available_amount,
                })),
            },
            QuotaLease::Unlimited { pod, expires_at } => Self {
                kind: Some(quota_lease::Kind::Unlimited(quota_lease::Unlimited {
                    pod: Some(pod.into()),
                    expires_at: Some(prost_types::Timestamp::from(SystemTime::from(expires_at))),
                })),
            },
        }
    }
}
