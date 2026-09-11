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

//! The timing contract of the shard lease, shared by the shard manager and the worker executor.
//!
//! The two run in separate processes and their crates cannot see each other, so the durations that
//! have to be ordered against one another live here, with the ordering stated as compile-time
//! assertions. The ladder, from the inside out:
//!
//! ```text
//! state write budget  <  per-attempt RPC deadline  <=  renewal cadence  <  lease duration
//! ```
//!
//! Each link exists for a different failure:
//!
//! - **write budget < deadline** - the manager persists a renewal on a task of its own, so a write
//!   it started always lands. If the executor could give up first, the manager would extend a lease
//!   the executor never learned about: the executor fences itself while the manager keeps it in the
//!   routing table, and the shards are neither served nor re-homed.
//! - **deadline <= cadence / 2** - one unanswered call must not consume the whole gap between two
//!   renewals, or a lease survives fewer attempts than [`SHARD_LEASE_RENEWAL_ATTEMPTS`] promises.
//! - **cadence x attempts = lease** - a lease has to outlive more than one lost response.

use std::time::Duration;

/// How many renewals an executor gets per lease: the cadence is the lease divided by this.
pub const SHARD_LEASE_RENEWAL_ATTEMPTS: u32 = 3;

/// How much of the gap between two renewals a single attempt may consume.
pub const SHARD_LEASE_RPC_DEADLINE_DIVISOR: u32 = 2;

/// How long the shard manager may spend storing the shard lease state, once.
///
/// It bounds a write that holds the state lock while an executor waits on the RPC that triggered
/// it, so it is the number the executor's deadline is built to outlast. Exceeding it stops the
/// shard manager: a standby takes over from the persisted state rather than a wedged leader
/// serving a routing table it can no longer update.
pub const SHARD_LEASE_STATE_WRITE_BUDGET_MILLIS: u64 = 4_000;

/// The shortest per-attempt deadline an executor will ever use, however little of its lease is
/// left.
///
/// The cadence follows the time *remaining* on the lease, so a push arriving late in one would
/// otherwise shorten the deadline below the manager's write budget. The floor is what keeps the
/// innermost link of the ladder true for every lease length and every delivery.
pub const SHARD_LEASE_RPC_DEADLINE_FLOOR_MILLIS: u64 = 10_000;

/// Floor on the derived renewal cadence, so a very short lease cannot turn the renewal loop into a
/// busy loop. Also the first step of the retry backoff.
pub const SHARD_LEASE_MIN_RENEWAL_INTERVAL_MILLIS: u64 = 1_000;

/// The shortest lease the protocol can serve.
///
/// Derived, not chosen: it is the lease at which the deadline floor equals the whole cadence, so a
/// single attempt fills the gap between two renewals and nothing is left for a retry.
pub const MIN_SHARD_LEASE_DURATION_MILLIS: u64 =
    SHARD_LEASE_RPC_DEADLINE_FLOOR_MILLIS * SHARD_LEASE_RENEWAL_ATTEMPTS as u64;

/// The shortest lease that still delivers the full [`SHARD_LEASE_RENEWAL_ATTEMPTS`] budget.
///
/// Also derived: it is the lease at which the deadline stops being held up by the floor and
/// resumes tracking the cadence.
pub const RECOMMENDED_MIN_SHARD_LEASE_DURATION_MILLIS: u64 =
    MIN_SHARD_LEASE_DURATION_MILLIS * SHARD_LEASE_RPC_DEADLINE_DIVISOR as u64;

// The manager's renewal handler reaps lapsed leases and then stores the grant, so one renewal is
// two writes, and both have to finish inside the shortest deadline the executor will ever use.
// Were it the other way round, a manager working within its own budget would still be abandoned
// mid-write, and the write would land anyway: the executor fences itself against a lease the
// manager goes on refreshing.
const _: () =
    assert!(SHARD_LEASE_RPC_DEADLINE_FLOOR_MILLIS >= 2 * SHARD_LEASE_STATE_WRITE_BUDGET_MILLIS);
// A divisor of one means the deadline is the whole cadence, so one unanswered call costs the
// entire gap between two renewals rather than half of it.
const _: () = assert!(SHARD_LEASE_RPC_DEADLINE_DIVISOR >= 2);
// Two attempts means one lost response loses the shards.
const _: () = assert!(SHARD_LEASE_RENEWAL_ATTEMPTS >= 3);
// The busy-loop floor must leave room inside one attempt, or the backoff outlasts the deadline it
// is retrying behind.
const _: () =
    assert!(SHARD_LEASE_MIN_RENEWAL_INTERVAL_MILLIS < SHARD_LEASE_RPC_DEADLINE_FLOOR_MILLIS);

pub const fn state_write_budget() -> Duration {
    Duration::from_millis(SHARD_LEASE_STATE_WRITE_BUDGET_MILLIS)
}

pub const fn rpc_deadline_floor() -> Duration {
    Duration::from_millis(SHARD_LEASE_RPC_DEADLINE_FLOOR_MILLIS)
}

pub const fn min_renewal_interval() -> Duration {
    Duration::from_millis(SHARD_LEASE_MIN_RENEWAL_INTERVAL_MILLIS)
}

pub const fn min_shard_lease_duration() -> Duration {
    Duration::from_millis(MIN_SHARD_LEASE_DURATION_MILLIS)
}

pub const fn recommended_min_shard_lease_duration() -> Duration {
    Duration::from_millis(RECOMMENDED_MIN_SHARD_LEASE_DURATION_MILLIS)
}

/// How long to wait before renewing a lease with `remaining` left on it.
///
/// Both sides derive their cadence from this: the executor to schedule its next renewal, the shard
/// manager to size the pass that reaps whatever did not renew.
pub fn renewal_interval(remaining: Duration) -> Duration {
    (remaining / SHARD_LEASE_RENEWAL_ATTEMPTS).max(min_renewal_interval())
}

/// How long one lease RPC may take, given the cadence the last grant implied.
///
/// Half the cadence, so an unanswered call costs half the gap to the next renewal rather than all
/// of it - and never below `floor`, so it always outlasts the manager's write budget. `None` is an
/// executor that has not been granted a lease yet, which has no cadence to divide.
pub fn rpc_deadline(cadence: Option<Duration>, floor: Duration) -> Duration {
    cadence
        .map(|cadence| cadence / SHARD_LEASE_RPC_DEADLINE_DIVISOR)
        .unwrap_or(floor)
        .max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// Replays the renewal schedule for a lease of `ttl` in which every attempt hangs until its
    /// deadline, and counts the attempts that finish before the lease lapses. Mirrors the loop in
    /// the executor: renew at the cadence, wait out the deadline, back off, try again.
    fn full_attempts_under_a_dead_manager(ttl: Duration) -> usize {
        let cadence = renewal_interval(ttl);
        let deadline = rpc_deadline(Some(cadence), rpc_deadline_floor());
        let cap = cadence.min(Duration::from_secs(30));

        let mut at = cadence;
        let mut backoff = min_renewal_interval();
        let mut full = 0;
        while at + deadline <= ttl {
            full += 1;
            at += deadline + backoff;
            backoff = (backoff * 2).min(cap);
        }
        full
    }

    /// The ladder the compile-time assertions in this module state, checked against the schedule
    /// it is supposed to produce. The counts are the point: at the shipped lease an executor gets
    /// the three attempts `SHARD_LEASE_RENEWAL_ATTEMPTS` promises, and it gets them because the
    /// deadline is half the cadence rather than all of it.
    #[test]
    fn a_lease_delivers_the_promised_attempts_even_when_every_renewal_hangs() {
        // (lease, expected full attempts)
        let cases = [
            (Duration::from_secs(30), 1),
            (Duration::from_secs(60), 3),
            (Duration::from_secs(90), 3),
            (Duration::from_secs(300), 3),
        ];
        for (ttl, expected) in cases {
            assert_eq!(
                full_attempts_under_a_dead_manager(ttl),
                expected,
                "a {ttl:?} lease should survive {expected} hung renewals"
            );
        }
    }

    /// The innermost link, and the one whose failure is not self-correcting: a manager working
    /// inside its own write budget must never be abandoned mid-write, whatever the lease looks
    /// like - including a lease with almost nothing left, which is what an `AssignShards` push
    /// arriving late produces.
    #[test]
    fn a_deadline_always_outlasts_the_managers_write_budget() {
        let floor = rpc_deadline_floor();
        for remaining_secs in [1u64, 2, 5, 6, 30, 60, 90, 300, 600] {
            let cadence = renewal_interval(Duration::from_secs(remaining_secs));
            let deadline = rpc_deadline(Some(cadence), floor);
            assert!(
                deadline > state_write_budget(),
                "a {remaining_secs}s remaining lease gave a {deadline:?} deadline, which does not \
                 outlast the {:?} write budget",
                state_write_budget()
            );
        }
        assert!(
            rpc_deadline(None, floor) > state_write_budget(),
            "an executor with no lease yet must still outlast a write"
        );
    }

    /// Above the recommended minimum the deadline tracks the cadence and takes half of it; below,
    /// the floor holds it up. Either way it never exceeds the cadence except where the floor says
    /// the lease was too short to serve properly - which is what the startup validation refuses.
    #[test]
    fn a_deadline_is_half_the_cadence_once_the_lease_is_long_enough() {
        let floor = rpc_deadline_floor();
        let recommended = recommended_min_shard_lease_duration();

        let cadence = renewal_interval(recommended);
        assert_eq!(rpc_deadline(Some(cadence), floor), cadence / 2);
        assert_eq!(
            rpc_deadline(Some(cadence), floor),
            floor,
            "and meets the floor exactly here"
        );

        let cadence = renewal_interval(recommended * 4);
        assert_eq!(rpc_deadline(Some(cadence), floor), cadence / 2);
    }

    /// The minimum is derived, not chosen: it is exactly the lease at which one attempt fills the
    /// whole gap between two renewals. A millisecond less and the deadline outruns the cadence.
    #[test]
    fn the_minimum_lease_is_where_one_attempt_fills_the_whole_cadence() {
        let floor = rpc_deadline_floor();
        let minimum = min_shard_lease_duration();

        let cadence = renewal_interval(minimum);
        assert_eq!(rpc_deadline(Some(cadence), floor), cadence);

        let shorter = renewal_interval(minimum - Duration::from_millis(1));
        assert!(
            rpc_deadline(Some(shorter), floor) > shorter,
            "below the minimum a single attempt outlasts the gap to the next renewal"
        );
    }

    /// A lease so short that the divisor would schedule a renewal every few milliseconds is held
    /// at the floor instead, so the loop cannot spin.
    #[test]
    fn a_tiny_lease_cannot_spin_the_renewal_loop() {
        assert_eq!(
            renewal_interval(Duration::from_millis(3)),
            min_renewal_interval()
        );
    }
}
