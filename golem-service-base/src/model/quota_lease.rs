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
use golem_common::model::resource_definition::{
    EnforcementAction, ResourceDefinitionId, ResourceLimit,
};
use std::fmt;
use std::time::Duration;

/// Monotonically increasing identifier for a lease on a (resource, pod) pair.
/// Used for fencing: an executor must reject operations from a stale epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseEpoch(pub u64);

impl LeaseEpoch {
    pub fn initial() -> Self {
        Self(0)
    }

    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("LeaseEpoch overflow"))
    }
}

impl fmt::Display for LeaseEpoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The allocation granted to an executor within a bounded lease.
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaAllocation {
    /// A fixed budget of units the executor may consume before
    /// renewing the lease or requesting a new.
    Budget { amount: u64 },
    /// No capacity is currently available. The executor should
    /// wait for the suggested duration before requesting a new lease.
    /// Agents should be suspended (throttle) or rejected depending
    /// on the enforcement action.
    Exhausted { retry_after: Duration },
}

impl QuotaAllocation {
    pub fn amount(&self) -> u64 {
        match self {
            Self::Budget { amount } => *amount,
            _ => 0,
        }
    }
}

/// A lease granted by the shard manager to a worker executor.
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaLease {
    /// The resource definition exists and the executor has a tracked lease.
    Bounded {
        resource_definition_id: ResourceDefinitionId,
        pod: Pod,
        epoch: LeaseEpoch,
        allocation: QuotaAllocation,
        expires_at: DateTime<Utc>,
        resource_limit: ResourceLimit,
        enforcement_action: EnforcementAction,
    },
    /// The resource definition does not exist or was deleted.
    /// The executor may use unlimited capacity but must renew
    /// to detect if a limit is later imposed.
    Unlimited { pod: Pod, expires_at: DateTime<Utc> },
}

mod protobuf {
    use super::*;
    use applying::Apply;
    use golem_api_grpc::proto::golem::common::{
        QuotaAllocation as GrpcQuotaAllocation, QuotaLease as GrpcQuotaLease, quota_allocation,
        quota_lease,
    };
    use std::time::SystemTime;

    impl TryFrom<GrpcQuotaAllocation> for QuotaAllocation {
        type Error = String;

        fn try_from(value: GrpcQuotaAllocation) -> Result<Self, Self::Error> {
            match value.kind.ok_or("QuotaAllocation.kind missing")? {
                quota_allocation::Kind::Budget(b) => Ok(Self::Budget { amount: b.amount }),
                quota_allocation::Kind::Exhausted(e) => Ok(Self::Exhausted {
                    retry_after: Duration::from_nanos(e.retry_after_nanos),
                }),
            }
        }
    }

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
                    let allocation: QuotaAllocation = b
                        .allocation
                        .ok_or("BoundedQuotaLease.allocation missing")?
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
                        allocation,
                        expires_at,
                        resource_limit,
                        enforcement_action,
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
}
