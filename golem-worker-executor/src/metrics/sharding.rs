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

use lazy_static::lazy_static;
use prometheus::*;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

lazy_static! {
    static ref ASSIGNED_SHARD_COUNT: Gauge =
        register_gauge!("assigned_shard_count", "Current number of assigned shards").unwrap();
    static ref STALE_SHARD_DELIVERY_TOTAL: CounterVec = register_counter_vec!(
        "stale_shard_delivery_total",
        "Number of shard deliveries dropped for naming a revision older than the last applied",
        &["delivery"]
    )
    .unwrap();
    static ref SHARD_LEASE_EXPIRY_TIMESTAMP_SECONDS: Gauge = register_gauge!(
        "shard_lease_expiry_timestamp_seconds",
        "Unix time at which the shard lease of this executor lapses unless it is renewed"
    )
    .unwrap();
    static ref SHARD_LEASE_RENEWALS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "shard_lease_renewals_total",
        "Number of shard lease renewal attempts, by outcome",
        &["outcome"]
    )
    .unwrap();
    static ref SHARD_ADMISSION_REFUSALS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "shard_admission_refusals_total",
        "Number of agent admissions refused by the shard assignment, by reason",
        &["reason"]
    )
    .unwrap();
}

pub fn record_assigned_shard_count(size: usize) {
    ASSIGNED_SHARD_COUNT.set(size as f64);
}

/// The outcome of one shard lease renewal attempt, as the `outcome` label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardLeaseRenewalOutcome {
    /// The shard manager granted a new lease.
    Granted,
    /// No assignment is installed yet, so there is no lease to renew.
    NotRegistered,
    /// The shard manager did not answer within the RPC deadline.
    Timeout,
    /// The shard manager no longer knows this executor, which then re-registers.
    LeaseNotFound,
    /// Any other failure of the RPC.
    Failed,
}

impl ShardLeaseRenewalOutcome {
    const ALL: [Self; 5] = [
        Self::Granted,
        Self::NotRegistered,
        Self::Timeout,
        Self::LeaseNotFound,
        Self::Failed,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::NotRegistered => "not_registered",
            Self::Timeout => "timeout",
            Self::LeaseNotFound => "lease_not_found",
            Self::Failed => "failed",
        }
    }
}

/// Why the shard assignment refused to admit an agent, as the `reason` label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardAdmissionRefusal {
    /// No assignment is installed yet.
    NotRegistered,
    /// The shard lease has lapsed: the self-fence.
    LeaseLapsed,
    /// The agent is on a shard this executor does not hold, which is normal during a
    /// rebalance.
    NotOwned,
}

impl ShardAdmissionRefusal {
    const ALL: [Self; 3] = [Self::NotRegistered, Self::LeaseLapsed, Self::NotOwned];

    fn label(self) -> &'static str {
        match self {
            Self::NotRegistered => "not_registered",
            Self::LeaseLapsed => "lease_lapsed",
            Self::NotOwned => "not_owned",
        }
    }
}

/// Exports every renewal outcome and admission refusal at zero, so that a dashboard or an
/// alert reads 0 instead of no data before the first one happens.
///
/// Called when the executor registers with a shard manager, so that a single-shard executor,
/// which has no lease, exports none of the lease metrics.
pub fn record_shard_lease_metrics_started() {
    for outcome in ShardLeaseRenewalOutcome::ALL {
        SHARD_LEASE_RENEWALS_TOTAL.with_label_values(&[outcome.label()]);
    }
    for refusal in ShardAdmissionRefusal::ALL {
        SHARD_ADMISSION_REFUSALS_TOTAL.with_label_values(&[refusal.label()]);
    }
}

/// Records the expiry of the held lease as wall-clock time.
///
/// Wall-clock time and not the remaining seconds, so that `expiry - time()` keeps counting
/// down in Prometheus when the renewal loop stops while the metrics endpoint still answers.
pub fn record_shard_lease_expiry(expires_at: Instant) {
    let remaining = expires_at.saturating_duration_since(Instant::now());
    let expires_at = (SystemTime::now() + remaining)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    SHARD_LEASE_EXPIRY_TIMESTAMP_SECONDS.set(expires_at.as_secs_f64());
}

pub fn record_shard_lease_renewal(outcome: ShardLeaseRenewalOutcome) {
    SHARD_LEASE_RENEWALS_TOTAL
        .with_label_values(&[outcome.label()])
        .inc();
}

pub fn record_shard_admission_refusal(refusal: ShardAdmissionRefusal) {
    SHARD_ADMISSION_REFUSALS_TOTAL
        .with_label_values(&[refusal.label()])
        .inc();
}

/// Which of the four shard deliveries a measurement is about, as the `delivery` label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardDelivery {
    Register,
    Assign,
    Revoke,
    Renewal,
}

impl ShardDelivery {
    fn label(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Assign => "assign",
            Self::Revoke => "revoke",
            Self::Renewal => "renewal",
        }
    }
}

/// A delivery was dropped for arriving older than the last one applied.
///
/// One of these is not a fault: a push and a renewal response that cross on the network
/// always produce one, and dropping the older is the point of the revision gate. A sustained
/// rate is the fault, and a warning per occurrence does not make a rate visible.
pub fn record_stale_shard_delivery(delivery: ShardDelivery) {
    STALE_SHARD_DELIVERY_TOTAL
        .with_label_values(&[delivery.label()])
        .inc();
}

/// How many deliveries of `delivery` have been dropped as stale.
#[cfg(test)]
pub fn stale_shard_delivery_count(delivery: ShardDelivery) -> f64 {
    STALE_SHARD_DELIVERY_TOTAL
        .with_label_values(&[delivery.label()])
        .get()
}
