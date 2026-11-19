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
    static ref EXECUTOR_COUNT: Gauge = register_gauge!(
        "executor_count",
        "Number of executor leases in the last accepted shard assignment"
    )
    .unwrap();
    static ref SHARD_DELIVERY_ATTEMPTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "shard_delivery_attempts_total",
        "Number of shard deliveries started before applying the assignment transition",
        &["delivery"]
    )
    .unwrap();
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

pub fn record_executor_count(count: usize) {
    EXECUTOR_COUNT.set(count as f64);
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

/// Exports delivery attempts, renewal outcomes and admission refusals before their first use.
/// Repeated initialization preserves existing totals.
///
/// Called when the executor registers with a shard manager, so that a single-shard executor,
/// which has no lease, exports none of the lease metrics.
pub fn record_shard_lease_metrics_started() {
    ShardDelivery::ALL.into_iter().for_each(|delivery| {
        SHARD_DELIVERY_ATTEMPTS_TOTAL.with_label_values(&[delivery.label()]);
    });
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
    const ALL: [Self; 4] = [Self::Register, Self::Assign, Self::Revoke, Self::Renewal];

    fn label(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Assign => "assign",
            Self::Revoke => "revoke",
            Self::Renewal => "renewal",
        }
    }
}

/// Records delivery entry before the assignment or its count gauge can change.
pub fn record_shard_delivery_attempt(delivery: ShardDelivery) {
    SHARD_DELIVERY_ATTEMPTS_TOTAL
        .with_label_values(&[delivery.label()])
        .inc();
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

#[cfg(test)]
pub fn shard_delivery_attempt_count(delivery: ShardDelivery) -> u64 {
    SHARD_DELIVERY_ATTEMPTS_TOTAL
        .with_label_values(&[delivery.label()])
        .get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn delivery_metric_initialization_preserves_existing_totals() {
        record_shard_lease_metrics_started();
        let metrics = prometheus::gather();
        let attempts = metrics
            .iter()
            .find(|metric| metric.name() == "shard_delivery_attempts_total")
            .expect("delivery attempts must be exported before their first use");
        ShardDelivery::ALL.into_iter().for_each(|delivery| {
            assert!(attempts.get_metric().iter().any(|metric| {
                metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == "delivery" && label.value() == delivery.label())
            }));
        });
        ShardDelivery::ALL
            .into_iter()
            .for_each(record_shard_delivery_attempt);
        let before = ShardDelivery::ALL.map(shard_delivery_attempt_count);
        record_shard_lease_metrics_started();
        ShardDelivery::ALL
            .into_iter()
            .zip(before)
            .for_each(|(delivery, before)| {
                assert!(
                    shard_delivery_attempt_count(delivery) >= before,
                    "re-registration must not reset a delivery total"
                );
            });
    }
}
