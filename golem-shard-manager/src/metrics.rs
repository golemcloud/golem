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
//! A standby never binds its gRPC port, so over its one remaining listener it is indistinguishable
//! from a wedged process. The gauges say who the leader is - `sum(shard_manager_is_leader) != 1`
//! is the alert - and the failure counter separates a replica queued behind a live leader (flat)
//! from one that cannot reach etcd (rising). It counts the read before the campaign too, because a
//! replica wedged there never reaches the campaign to fail one.

use prometheus::{Gauge, IntCounter, register_gauge, register_int_counter};
use std::sync::LazyLock;

static IS_LEADER: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "shard_manager_is_leader",
        "1 while this replica holds the etcd leadership lease, 0 while it is standing by"
    )
    .expect("Cannot register the shard_manager_is_leader gauge")
});

static LEADER_SINCE: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "shard_manager_leader_since_epoch",
        "Unix seconds at which this replica was elected, or 0 while it is standing by"
    )
    .expect("Cannot register the shard_manager_leader_since_epoch gauge")
});

static CAMPAIGN_ATTEMPT_FAILURES: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "shard_manager_campaign_attempt_failures_total",
        "Attempts to reach etcd on the way to leadership that failed retriably and were retried"
    )
    .expect("Cannot register the shard_manager_campaign_attempt_failures_total counter")
});

/// Registers the leadership metrics at their standing-by values.
pub fn record_standing_by() {
    IS_LEADER.set(0.0);
    LEADER_SINCE.set(0.0);
    LazyLock::force(&CAMPAIGN_ATTEMPT_FAILURES);
}

/// Counts one retriable failure on the way to leadership.
pub fn record_campaign_attempt_failure() {
    CAMPAIGN_ATTEMPT_FAILURES.inc();
}

pub fn record_elected(elected_at_epoch_secs: f64) {
    IS_LEADER.set(1.0);
    LEADER_SINCE.set(elected_at_epoch_secs);
}
