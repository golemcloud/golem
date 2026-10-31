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

//! Monotonic lease, refresh, hold and wall-clock skew calculations.

use golem_common::model::Timestamp;
use std::time::Duration;

/// How far the clock of another host can be ahead of the local clock. A time that is further
/// ahead counts as missing.
pub(super) const CLOCK_SKEW_MARGIN: Duration = Duration::from_secs(2 * 60);

/// Tells whether the grace period passed at `now` since the time.
pub(super) fn passed_since(time: Timestamp, now: Timestamp, grace: Duration) -> bool {
    now.to_millis()
        >= time
            .to_millis()
            .saturating_add(u64::try_from(grace.as_millis()).unwrap_or(u64::MAX))
}

/// Tells whether the time is more than [`CLOCK_SKEW_MARGIN`] after `now`.
pub(super) fn beyond_margin(time: Timestamp, now: Timestamp) -> bool {
    time.to_millis()
        > now
            .to_millis()
            .saturating_add(u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap_or(u64::MAX))
}

/// The unit of the time in the name of a marker.
pub(super) const MARKER_TIME_UNIT: Duration = Duration::from_millis(1);

/// Gives the time from the start of a marker write to the end of the lease that the marker gives,
/// as the other deletes count it from the time in the name: the grace period less one storage call
/// deadline, but at least one deadline.
fn lease_bound(grace: Duration, deadline: Duration) -> Duration {
    grace.saturating_sub(deadline).max(deadline)
}

/// Gives how long a marker write that succeeds lets the prune of its claim go on: one refresh
/// period and two storage call deadlines, but at most the lease bound less one millisecond. The
/// name of a marker keeps its time in whole milliseconds, so the time in the name can be up to one
/// millisecond before the time that the delete read. So the lease is above zero for each deadline
/// above one millisecond. A prune stops its reads and deletes of snapshot data at most one span
/// after the start of its newest marker write that succeeded, also when its later refresh writes
/// fail.
pub(in crate::filesystem_snapshot::backend::rustic) fn lease_span(
    grace: Duration,
    deadline: Duration,
) -> Duration {
    refresh_period(grace, deadline)
        .saturating_add(deadline.saturating_mul(2))
        .min(lease_bound(grace, deadline).saturating_sub(MARKER_TIME_UNIT))
}

/// Gives how long a marker holds the claims of its ledger: the lease bound, then one margin for
/// clock skew, then two storage call deadlines. Another delete sees the marker time with up to one
/// margin of skew. A marker write that the storage received can still land up to one deadline after
/// the call gave up. A prune that its lease stopped writes its final marker after the lease ran
/// out, and that write can land up to one more deadline later. So a prune whose lease ran out stops
/// before another delete can take its claim, and the next prune waits a full hold from its final
/// marker.
pub(super) fn claim_hold(grace: Duration, deadline: Duration) -> Duration {
    lease_bound(grace, deadline)
        .saturating_add(CLOCK_SKEW_MARGIN)
        .saturating_add(deadline.saturating_mul(2))
}

/// Gives the time between two markers of a live claim: a fourth of the grace period, or a fourth
/// of the margin for clock skew when the grace period is zero, but at most a fourth of the lease
/// bound less one millisecond. It comes from the lease bound and not from the lease span, which
/// comes from it.
pub(in crate::filesystem_snapshot::backend::rustic) fn refresh_period(
    grace: Duration,
    deadline: Duration,
) -> Duration {
    let base = if grace.is_zero() {
        CLOCK_SKEW_MARGIN
    } else {
        grace
    };
    (base / 4).min(lease_bound(grace, deadline).saturating_sub(MARKER_TIME_UNIT) / 4)
}

#[cfg(test)]
mod tests;
