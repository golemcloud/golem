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

//! Leadership metrics for distributed mode.
//!
//! A standby serves no call, so over its listeners it is indistinguishable from a wedged process.
//! The gauges say who holds the leadership - `sum(golem_shard_manager_is_leader) != 1` is the alert - and
//! which replica serves, `golem_shard_manager_is_serving`. The failure counter separates a replica queued
//! behind a live leader (flat) from one that cannot reach etcd (rising). It counts the read before
//! the campaign too, because a replica wedged there never reaches the campaign to fail one.
//!
//! The lease expiry counter is the one view of an executor that stopped renewing which does not
//! depend on that executor still answering a scrape.

use golem_common::metrics::catalog;
use prometheus::{Gauge, IntCounter};
use std::sync::LazyLock;

static IS_LEADER: LazyLock<Gauge> = LazyLock::new(|| catalog::SHARD_MANAGER_IS_LEADER.register());

static IS_SERVING: LazyLock<Gauge> = LazyLock::new(|| catalog::SHARD_MANAGER_IS_SERVING.register());

static LEADER_SINCE: LazyLock<Gauge> =
    LazyLock::new(|| catalog::SHARD_MANAGER_LEADER_SINCE_TIMESTAMP_SECONDS.register());

static CAMPAIGN_ATTEMPT_FAILURES: LazyLock<IntCounter> =
    LazyLock::new(|| catalog::SHARD_MANAGER_CAMPAIGN_ATTEMPT_FAILURES_TOTAL.register());

/// Registers the leadership metrics at their standing-by values.
pub fn record_standing_by() {
    IS_LEADER.set(0.0);
    IS_SERVING.set(0.0);
    LEADER_SINCE.set(0.0);
    LazyLock::force(&CAMPAIGN_ATTEMPT_FAILURES);
    LazyLock::force(&EXECUTOR_LEASES_EXPIRED);
}

/// Counts one retriable failure on the way to leadership.
pub fn record_campaign_attempt_failure() {
    CAMPAIGN_ATTEMPT_FAILURES.inc();
}

/// This replica takes calls: set once its startup has completed and, in distributed mode, its
/// election value is marked ready. `golem_shard_manager_is_leader` turns on earlier, at the election.
pub fn record_serving() {
    IS_SERVING.set(1.0);
}

/// This replica no longer takes calls, though it may not have given up the leadership yet.
pub fn record_not_serving() {
    IS_SERVING.set(0.0);
}

pub fn record_elected(elected_at_epoch_secs: f64) {
    IS_LEADER.set(1.0);
    LEADER_SINCE.set(elected_at_epoch_secs);
}

static EXECUTOR_LEASES_EXPIRED: LazyLock<IntCounter> =
    LazyLock::new(|| catalog::SHARD_MANAGER_EXECUTOR_LEASE_EXPIRED_TOTAL.register());

/// Counts executor leases that lapsed and were removed from the stored state.
///
/// Only the leader removes leases, so the sum over all replicas is the cluster total. A standby
/// exports 0 from [`record_standing_by`].
pub fn record_executor_leases_expired(count: usize) {
    EXECUTOR_LEASES_EXPIRED.inc_by(count as u64);
}
