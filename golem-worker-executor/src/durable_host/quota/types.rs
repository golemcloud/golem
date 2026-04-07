// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use crate::services::quota::LeaseInterest;
use chrono::{DateTime, Utc};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::Reservation;
use golem_common::model::quota::ResourceName;
use wasmtime::component::Resource;

/// Parameters needed to perform a deferred (lazy) lease acquire during replay.
pub struct PendingAcquire {
    pub environment_id: EnvironmentId,
    pub resource_name: ResourceName,
    pub expected_use: u64,
    /// Most-recently replayed credit value (starts at 0 from the acquire entry,
    /// then updated by each replayed reserve/commit entry).
    pub last_credit: i64,
    /// Timestamp at which `last_credit` was recorded (from the replayed entry).
    pub last_credit_at: DateTime<Utc>,
}

/// State of the lease held by a `QuotaTokenEntry`.
pub enum LeaseInterestHandle {
    /// Live mode: the acquire already happened and the `LeaseInterest` is live.
    Live(LeaseInterest),
    /// Replay mode: the acquire is deferred until the first `reserve` call.
    /// Credit state is kept up to date by each replayed reserve/commit entry.
    Pending(PendingAcquire),
}

/// Resource table entry for a `quota-token` WIT resource.
pub struct QuotaTokenEntry {
    pub lease: LeaseInterestHandle,
}

impl QuotaTokenEntry {
    pub fn live(interest: LeaseInterest) -> Self {
        Self {
            lease: LeaseInterestHandle::Live(interest),
        }
    }

    pub fn pending(
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        expected_use: u64,
        last_credit: i64,
        last_credit_at: DateTime<Utc>,
    ) -> Self {
        Self {
            lease: LeaseInterestHandle::Pending(PendingAcquire {
                environment_id,
                resource_name,
                expected_use,
                last_credit,
                last_credit_at,
            }),
        }
    }

    pub fn environment_id(&self) -> EnvironmentId {
        match &self.lease {
            LeaseInterestHandle::Live(interest) => interest.environment_id,
            LeaseInterestHandle::Pending(p) => p.environment_id,
        }
    }

    pub fn resource_name(&self) -> &ResourceName {
        match &self.lease {
            LeaseInterestHandle::Live(interest) => &interest.resource_name,
            LeaseInterestHandle::Pending(p) => &p.resource_name,
        }
    }

    pub fn expected_use(&self) -> u64 {
        match &self.lease {
            LeaseInterestHandle::Live(interest) => interest.expected_use,
            LeaseInterestHandle::Pending(p) => p.expected_use,
        }
    }

    pub fn set_expected_use(&mut self, expected_use: u64) {
        match &mut self.lease {
            LeaseInterestHandle::Live(interest) => {
                interest.expected_use = expected_use;
                interest.credit_rate =
                    expected_use as f64 * crate::services::quota::CREDIT_RATE_FACTOR;
                interest.max_credit = crate::services::quota::max_credit_for(expected_use);
            }
            LeaseInterestHandle::Pending(p) => {
                p.expected_use = expected_use;
            }
        }
    }

    /// Returns the current credit value (0 if acquire is still pending and no
    /// reserve/commit entries have been replayed yet).
    pub fn last_credit(&self) -> i64 {
        match &self.lease {
            LeaseInterestHandle::Live(interest) => interest.last_credit_value,
            LeaseInterestHandle::Pending(p) => p.last_credit,
        }
    }

    /// Update the replayed credit state. Used during replay of reserve/commit entries.
    pub fn update_replayed_credit(&mut self, credit: i64, at: DateTime<Utc>) {
        match &mut self.lease {
            LeaseInterestHandle::Live(_) => {
                panic!("update replayed credit may not be called with a live lease interest")
            }
            LeaseInterestHandle::Pending(p) => {
                p.last_credit = credit;
                p.last_credit_at = at;
            }
        }
    }
}

/// Resource table entry for a `reservation` WIT resource.
pub struct ReservationEntry {
    pub reservation: Reservation,
    pub token: Resource<QuotaTokenEntry>,
}
