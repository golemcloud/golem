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

use test_r::test;

use super::*;
use golem_common::model::ShardId;
use std::net::Ipv4Addr;

const TTL: Duration = Duration::from_secs(60);

fn t0() -> Timestamp {
    Timestamp::new(1_700_000_000, 0).unwrap()
}

fn executor(idx: u128) -> ExecutorId {
    ExecutorId(Uuid::from_u128(idx))
}

fn addr(idx: u8) -> ExecutorAddr {
    ExecutorAddr {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, idx)),
        port: 9000 + idx as u16,
    }
}

fn shard(id: i64) -> ShardId {
    ShardId::new(id)
}

fn shards(ids: &[i64]) -> BTreeSet<ShardId> {
    ids.iter().copied().map(ShardId::new).collect()
}

fn shard_state_with(number_of_shards: usize, executors: &[(u128, u8, &[i64])]) -> ShardLeaseState {
    let mut shard_state = ShardLeaseState::new(number_of_shards);
    for (idx, addr_idx, shard_ids) in executors {
        shard_state.add_executor(executor(*idx), addr(*addr_idx), None, t0(), TTL);
        for shard_id in *shard_ids {
            shard_state.assign_shard(executor(*idx), shard(*shard_id));
        }
    }
    shard_state
}

#[test]
fn deliveries_count_all_executor_leases_after_join_and_leave() {
    let mut state = shard_state_with(2, &[(1, 1, &[0]), (2, 2, &[1])]);
    let check = |state: &ShardLeaseState, expected| {
        state.get_executors().for_each(|executor_id| {
            let grant = state
                .lease_grant_for(*executor_id)
                .unwrap()
                .stamp(state.revision);
            let push = state.assignment_push_for(*executor_id).unwrap();
            assert_eq!(grant.executor_count, expected);
            assert_eq!(push.executor_count, expected);
            let wire = crate::sharding::worker_executor::assignment_to_proto(
                &push,
                "test-incarnation".to_string(),
            )
            .unwrap();
            assert_eq!(wire.executor_count, expected as u32);
            assert_eq!(wire.incarnation_id, "test-incarnation");
        });
    };

    check(&state, 2);
    state.add_executor(executor(3), addr(3), None, t0(), TTL);
    state.bump_revision().unwrap();
    check(&state, 3);
    state.remove_executor(executor(1));
    state.bump_revision().unwrap();
    check(&state, 2);
}

#[test]
fn new_state_is_empty_with_initial_revision() {
    let shard_state = ShardLeaseState::new(8);
    assert_eq!(shard_state.number_of_shards, 8);
    assert_eq!(shard_state.revision, ShardLeaseRevision::INITIAL);
    assert!(shard_state.shard_assignments.is_empty());
    assert!(shard_state.shard_epochs.is_empty());
    assert!(shard_state.executor_leases.is_empty());
    assert!(shard_state.pending_rebalance.is_empty());
    assert_eq!(shard_state.executor_count(), 0);
    assert_eq!(
        shard_state.get_unassigned_shards(),
        shards(&[0, 1, 2, 3, 4, 5, 6, 7])
    );
    assert!(shard_state.check_invariants().is_ok());
}

#[test]
fn generated_executor_ids_are_unique_and_time_ordered() {
    let ids: Vec<ExecutorId> = (0..64).map(|_| ExecutorId::generate()).collect();
    let unique: BTreeSet<ExecutorId> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len());
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn add_executor_grants_lease_with_expected_window() {
    let mut shard_state = ShardLeaseState::new(4);
    let replaced =
        shard_state.add_executor(executor(1), addr(1), Some("pod-1".to_string()), t0(), TTL);
    assert_eq!(replaced, None);

    let lease = &shard_state.executor_leases[&executor(1)];
    assert_eq!(lease.addr, addr(1));
    assert_eq!(lease.granted_at, t0());
    assert_eq!(lease.expires_at, t0() + jiff::SignedDuration::from_secs(60));
    assert_eq!(lease.pod_name.as_deref(), Some("pod-1"));

    assert!(shard_state.has_executor(executor(1)));
    assert_eq!(shard_state.addr_for(executor(1)), Some(addr(1)));
    assert_eq!(shard_state.executor_for_addr(addr(1)), Some(executor(1)));
    assert_eq!(
        shard_state.shards_for_executor(executor(1)),
        Some(BTreeSet::new())
    );
    assert_eq!(shard_state.shards_for_executor(executor(2)), None);
    assert_eq!(
        shard_state.get_executors_with_addrs(),
        vec![(executor(1), addr(1), Some("pod-1".to_string()))]
    );
}

#[test]
fn add_executor_at_reused_address_replaces_lease_and_transfers_shards() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[2])]);
    let mut later = t0() + jiff::SignedDuration::from_secs(10);

    // bump shard 1 once so we can tell "advanced" from "reset": moving it to executor 2 and
    // back advances its epoch to 2
    shard_state.assign_shard(executor(2), shard(1));
    shard_state.assign_shard(executor(1), shard(1));
    assert_eq!(shard_state.epoch_for_shard(shard(1)), Some(ShardEpoch(2)));

    let replaced = shard_state.add_executor(executor(3), addr(1), None, later, TTL);
    assert_eq!(replaced, Some(executor(1)));

    assert!(!shard_state.has_executor(executor(1)));
    assert!(shard_state.has_executor(executor(3)));
    assert_eq!(shard_state.executor_for_addr(addr(1)), Some(executor(3)));
    assert_eq!(shard_state.executor_count(), 2);

    assert_eq!(
        shard_state.shards_for_executor(executor(3)),
        Some(shards(&[0, 1]))
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
    assert_eq!(shard_state.epoch_for_shard(shard(1)), Some(ShardEpoch(3)));
    assert_eq!(
        shard_state.shard_epochs.get(&shard(1)),
        Some(&ShardEpoch(3))
    );
    assert_eq!(
        shard_state.shards_for_executor(executor(2)),
        Some(shards(&[2]))
    );
    assert_eq!(shard_state.epoch_for_shard(shard(2)), Some(ShardEpoch(0)));
    assert!(shard_state.pending_rebalance.is_empty());
    assert_eq!(shard_state.executor_leases[&executor(3)].granted_at, later);

    // a second registration of the same id at the same address only refreshes the lease
    later += jiff::SignedDuration::from_secs(10);
    let replaced = shard_state.add_executor(executor(3), addr(1), None, later, TTL);
    assert_eq!(replaced, None);
    assert_eq!(
        shard_state.shards_for_executor(executor(3)),
        Some(shards(&[0, 1]))
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
    assert_eq!(shard_state.executor_leases[&executor(3)].granted_at, later);
    assert!(shard_state.check_invariants().is_ok());
}

#[test]
fn remove_executor_orphans_shards_into_pending_rebalance() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[2])]);

    let orphaned = shard_state.remove_executor(executor(1));
    assert_eq!(orphaned, shards(&[0, 1]));
    assert!(!shard_state.has_executor(executor(1)));
    assert_eq!(shard_state.pending_rebalance, shards(&[0, 1]));
    assert_eq!(shard_state.get_unassigned_shards(), shards(&[0, 1, 3]));
    assert_eq!(shard_state.shards_for_executor(executor(1)), None);
    assert_eq!(
        shard_state.shards_for_executor(executor(2)),
        Some(shards(&[2]))
    );
    assert!(shard_state.check_invariants().is_ok());

    // unknown ids are a no-op
    assert_eq!(shard_state.remove_executor(executor(42)), BTreeSet::new());
    assert_eq!(shard_state.executor_count(), 1);
}

#[test]
fn assign_shard_epoch_semantics() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[]), (2, 2, &[])]);

    // first assignment starts at the initial epoch
    assert_eq!(
        shard_state.assign_shard(executor(1), shard(0)),
        ShardEpoch::initial()
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(0)));

    // re-assigning to the same owner is idempotent
    assert_eq!(
        shard_state.assign_shard(executor(1), shard(0)),
        ShardEpoch(0)
    );

    // moving to another owner advances the epoch; stored == returned
    assert_eq!(
        shard_state.assign_shard(executor(2), shard(0)),
        ShardEpoch(1)
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
    assert_eq!(
        shard_state.shards_for_executor(executor(1)),
        Some(BTreeSet::new())
    );
    assert_eq!(
        shard_state.shards_for_executor(executor(2)),
        Some(shards(&[0]))
    );

    // after a full unassignment the next assignment continues past the highest epoch ever
    // issued for the shard - an epoch is never reused in a shard's history
    assert!(shard_state.unassign_shard(executor(2), shard(0)));
    assert_eq!(shard_state.epoch_for_shard(shard(0)), None);
    assert_eq!(
        shard_state.shard_epochs.get(&shard(0)),
        Some(&ShardEpoch(1))
    );
    assert_eq!(
        shard_state.assign_shard(executor(1), shard(0)),
        ShardEpoch(2)
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(2)));
}

#[test]
fn epochs_stay_unique_across_eviction_and_reassignment() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[]), (3, 3, &[])]);
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(0)));

    // executor 1 is evicted (health check / lease expiry): its shards are orphaned ...
    shard_state.remove_executor(executor(1));
    assert_eq!(shard_state.epoch_for_shard(shard(0)), None);
    assert_eq!(shard_state.pending_rebalance, shards(&[0, 1]));

    // ... and later handed to other executors with epochs the evicted owner never held
    assert_eq!(
        shard_state.assign_shard(executor(2), shard(0)),
        ShardEpoch(1)
    );
    assert_eq!(
        shard_state.assign_shard(executor(3), shard(1)),
        ShardEpoch(1)
    );

    // a second eviction keeps advancing
    shard_state.remove_executor(executor(2));
    assert_eq!(
        shard_state.assign_shard(executor(3), shard(0)),
        ShardEpoch(2)
    );

    // housekeep-driven eviction behaves the same way
    let expired = t0() + jiff::SignedDuration::from_secs(3600);
    assert_eq!(
        shard_state.housekeep(expired),
        vec![(executor(3), shards(&[0, 1]))]
    );
    shard_state.add_executor(executor(4), addr(4), None, expired, TTL);
    assert_eq!(
        shard_state.assign_shard(executor(4), shard(0)),
        ShardEpoch(3)
    );
    assert_eq!(
        shard_state.assign_shard(executor(4), shard(1)),
        ShardEpoch(2)
    );
    assert!(shard_state.check_invariants().is_ok());
}

#[test]
fn unassign_shard_is_guarded_by_owner() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0]), (2, 2, &[])]);

    assert!(!shard_state.unassign_shard(executor(2), shard(0)));
    assert_eq!(
        shard_state.shards_for_executor(executor(1)),
        Some(shards(&[0]))
    );

    assert!(shard_state.unassign_shard(executor(1), shard(0)));
    assert_eq!(
        shard_state.shards_for_executor(executor(1)),
        Some(BTreeSet::new())
    );
    assert!(!shard_state.unassign_shard(executor(1), shard(0)));
}

#[test]
fn assign_shard_clears_pending_rebalance() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[])]);
    shard_state.remove_executor(executor(1));
    assert_eq!(shard_state.pending_rebalance, shards(&[0, 1]));

    shard_state.assign_shard(executor(2), shard(0));
    assert_eq!(shard_state.pending_rebalance, shards(&[1]));
    assert!(shard_state.check_invariants().is_ok());
}

#[test]
fn apply_rebalance_moves_bump_epoch_once_and_unassignments_are_owner_guarded() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1, 2, 3]), (2, 2, &[])]);

    // move shards 0 and 1 from executor 1 to executor 2, as a Rebalance would express it
    let mut assignments = Assignments::new();
    assignments.assign(executor(2), shard(0));
    assignments.assign(executor(2), shard(1));
    let mut unassignments = Unassignments::new();
    unassignments.unassign(executor(1), shard(0));
    unassignments.unassign(executor(1), shard(1));
    let rebalance = Rebalance::new(assignments, unassignments);

    shard_state.apply_rebalance(&rebalance);

    assert_eq!(
        shard_state.shards_for_executor(executor(1)),
        Some(shards(&[2, 3]))
    );
    assert_eq!(
        shard_state.shards_for_executor(executor(2)),
        Some(shards(&[0, 1]))
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
    assert_eq!(shard_state.epoch_for_shard(shard(1)), Some(ShardEpoch(1)));
    assert_eq!(shard_state.epoch_for_shard(shard(2)), Some(ShardEpoch(0)));
    assert!(shard_state.check_invariants().is_ok());

    // Applying the same plan again is a no-op on the state: an assignment to the shard's
    // current owner keeps its epoch, and an unassignment is owner-guarded.
    shard_state.apply_rebalance(&rebalance);
    assert_eq!(
        shard_state.shards_for_executor(executor(2)),
        Some(shards(&[0, 1]))
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
}

#[test]
// Between the plan and the apply, a restarted instance registering at a known address can
// inherit the same shard and mint an epoch for it inline. The apply mints against the state as
// it is then, so its epoch is above that one - never a second holder of the same `(shard, epoch)`.
fn an_epoch_minted_at_apply_is_above_one_a_register_minted_in_between() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1, 2, 3]), (2, 2, &[])]);

    // the plan: shard 0 moves from executor 1 to executor 2
    let mut assignments = Assignments::new();
    assignments.assign(executor(2), shard(0));
    let mut unassignments = Unassignments::new();
    unassignments.unassign(executor(1), shard(0));
    let rebalance = Rebalance::new(assignments, unassignments);

    // ...and before it is applied, executor 1 is replaced at its own address by a restart,
    // which inherits shard 0 inline and mints epoch 1 for it
    shard_state.add_executor(executor(3), addr(1), None, t0(), TTL);
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));

    shard_state.apply_rebalance(&rebalance);

    assert_eq!(
        shard_state.epoch_for_shard(shard(0)),
        Some(ShardEpoch(2)),
        "the epoch minted at the apply must be above the one the restart minted in between"
    );
    assert_eq!(
        shard_state.shard_assignments[&shard(0)].executor_id,
        executor(2)
    );
    assert!(shard_state.check_invariants().is_ok());
}

#[test]
fn renew_lease_moves_only_the_clock() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1])]);
    let later = t0() + jiff::SignedDuration::from_secs(30);

    assert!(shard_state.renew_lease(executor(1), later, TTL));
    let lease = &shard_state.executor_leases[&executor(1)];
    assert_eq!(lease.granted_at, later);
    assert_eq!(
        lease.expires_at,
        later + jiff::SignedDuration::from_secs(60)
    );
    assert_eq!(
        shard_state.shards_for_executor(executor(1)),
        Some(shards(&[0, 1]))
    );
    assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(0)));

    assert!(!shard_state.renew_lease(executor(9), later, TTL));
}

/// The executor holds the deadline it was last told, and a renewal's reply can be lost. If a
/// renewal under a `shard_lease_duration` reduced across a restart stored the shorter deadline,
/// the executor would admit to the old one while the manager reaped from the new one - and no
/// reply the executor never receives can correct that. So the outstanding deadline is kept, and
/// the configured length takes over only once `now + length` passes it.
#[test]
fn a_renewal_never_shortens_an_outstanding_lease_because_its_reply_may_be_lost() {
    // granted at t0 for 60 s: the executor may hold t0 + 60 s
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1])]);
    let shortened = std::time::Duration::from_secs(30);

    let early = t0() + jiff::SignedDuration::from_secs(20);
    assert!(shard_state.renew_lease(executor(1), early, shortened));
    assert_eq!(
        shard_state.executor_leases[&executor(1)].expires_at,
        t0() + jiff::SignedDuration::from_secs(60),
        "an early renewal must keep the deadline the executor may still be holding"
    );

    let late = t0() + jiff::SignedDuration::from_secs(35);
    assert!(shard_state.renew_lease(executor(1), late, shortened));
    assert_eq!(
        shard_state.executor_leases[&executor(1)].expires_at,
        late + jiff::SignedDuration::from_secs(30),
        "once now + the configured length passes the outstanding deadline, it applies"
    );
}

/// A registration retried with the same id - the client retries a lost reply - is the one
/// other writer of a lease, and it must keep an outstanding deadline for the same reason.
#[test]
fn a_retried_registration_never_shortens_an_outstanding_lease() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1])]);
    let retried = t0() + jiff::SignedDuration::from_secs(10);

    let replaced = shard_state.add_executor(
        executor(1),
        addr(1),
        None,
        retried,
        std::time::Duration::from_secs(30),
    );

    assert_eq!(replaced, None);
    assert_eq!(
        shard_state.executor_leases[&executor(1)].expires_at,
        t0() + jiff::SignedDuration::from_secs(60)
    );
}

/// The startup re-grant is a renewal, so a shard manager coming back with a shorter
/// `shard_lease_duration` keeps every outstanding deadline too; a longer one applies at once.
#[test]
fn a_regrant_never_shortens_an_outstanding_lease() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1])]);
    let restarted = t0() + jiff::SignedDuration::from_secs(15);

    let regranted = shard_state.regrant_leases(
        &HashSet::from([executor(1)]),
        restarted,
        std::time::Duration::from_secs(30),
    );
    assert_eq!(regranted, 1);
    assert_eq!(
        shard_state.executor_leases[&executor(1)].expires_at,
        t0() + jiff::SignedDuration::from_secs(60)
    );

    shard_state.regrant_leases(
        &HashSet::from([executor(1)]),
        restarted,
        std::time::Duration::from_secs(90),
    );
    assert_eq!(
        shard_state.executor_leases[&executor(1)].expires_at,
        restarted + jiff::SignedDuration::from_secs(90)
    );
}

#[test]
fn regrant_leases_restarts_the_clock_of_the_listed_executors_only() {
    let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[2, 3])]);
    let later = t0() + jiff::SignedDuration::from_secs(3600);

    // executor 9 holds no lease and must not create one
    let regranted =
        shard_state.regrant_leases(&HashSet::from([executor(1), executor(9)]), later, TTL);

    assert_eq!(regranted, 1);
    assert_eq!(
        shard_state.executor_leases[&executor(1)].expires_at,
        later + jiff::SignedDuration::from_secs(60)
    );
    assert_eq!(
        shard_state.executor_leases[&executor(2)].expires_at,
        t0() + jiff::SignedDuration::from_secs(60)
    );
    assert!(!shard_state.has_executor(executor(9)));

    // the re-granted lease survives the housekeeping that evicts the one left behind
    let evicted = shard_state.housekeep(later);
    assert_eq!(evicted, vec![(executor(2), shards(&[2, 3]))]);
    assert!(shard_state.has_executor(executor(1)));
    assert_eq!(shard_state.pending_rebalance, shards(&[2, 3]));
}

#[test]
fn executor_shard_sets_covers_every_lease_in_id_order() {
    let shard_state = shard_state_with(6, &[(3, 3, &[4]), (1, 1, &[0, 2]), (2, 2, &[])]);
    let sets = shard_state.executor_shard_sets();
    let ids: Vec<ExecutorId> = sets.iter().map(|s| s.executor_id).collect();
    assert_eq!(ids, vec![executor(1), executor(2), executor(3)]);
    assert_eq!(sets[0].shard_ids, shards(&[0, 2]));
    assert_eq!(sets[1].shard_ids, BTreeSet::new());
    assert_eq!(sets[2].shard_ids, shards(&[4]));
    assert_eq!(shard_state.get_unassigned_shards(), shards(&[1, 3, 5]));
}

#[test]
fn a_lease_counts_as_expired_from_its_expiry_on() {
    let mut shard_state = ShardLeaseState::new(4);
    shard_state.add_executor(executor(1), addr(1), None, t0(), Duration::from_secs(60));

    assert!(!shard_state.has_expired_lease(t0()));
    assert!(!shard_state.has_expired_lease(t0() + jiff::SignedDuration::from_secs(59)));
    assert!(shard_state.has_expired_lease(t0() + jiff::SignedDuration::from_secs(60)));
}

#[test]
fn housekeep_evicts_only_expired_leases() {
    let mut shard_state = ShardLeaseState::new(4);
    shard_state.add_executor(executor(1), addr(1), None, t0(), Duration::from_secs(10));
    shard_state.add_executor(executor(2), addr(2), None, t0(), Duration::from_secs(60));
    shard_state.assign_shard(executor(1), shard(0));
    shard_state.assign_shard(executor(2), shard(1));

    let check_delivered_count = |state: &ShardLeaseState, expected| {
        let grant = state
            .lease_grant_for(executor(2))
            .unwrap()
            .stamp(state.revision);
        let push = state.assignment_push_for(executor(2)).unwrap();
        assert_eq!(grant.executor_count, expected);
        assert_eq!(push.executor_count, expected);
        assert_eq!(
            crate::sharding::worker_executor::assignment_to_proto(
                &push,
                "test-incarnation".to_string(),
            )
            .unwrap()
            .executor_count,
            expected as u32
        );
    };

    assert!(
        shard_state
            .housekeep(t0() + jiff::SignedDuration::from_secs(9))
            .is_empty()
    );
    assert_eq!(shard_state.executor_count(), 2);
    check_delivered_count(&shard_state, 2);

    // expiry is inclusive
    let evicted = shard_state.housekeep(t0() + jiff::SignedDuration::from_secs(10));
    assert_eq!(evicted, vec![(executor(1), shards(&[0]))]);
    assert!(!shard_state.has_executor(executor(1)));
    assert!(shard_state.has_executor(executor(2)));
    assert_eq!(shard_state.pending_rebalance, shards(&[0]));
    check_delivered_count(&shard_state, 1);
    assert_eq!(
        shard_state.shards_for_executor(executor(2)),
        Some(shards(&[1]))
    );

    assert_eq!(shard_state.take_pending_rebalance(), shards(&[0]));
    assert!(shard_state.pending_rebalance.is_empty());
    assert!(shard_state.check_invariants().is_ok());
}

#[test]
fn bump_revision_increments_and_reports_overflow() {
    let mut shard_state = ShardLeaseState::new(1);
    assert_eq!(shard_state.bump_revision().unwrap(), ShardLeaseRevision(1));
    assert_eq!(shard_state.bump_revision().unwrap(), ShardLeaseRevision(2));
    assert_eq!(shard_state.revision, ShardLeaseRevision(2));

    shard_state.revision = ShardLeaseRevision(u64::MAX);
    match shard_state.bump_revision() {
        Err(ShardManagerError::Internal(msg)) => assert!(msg.contains("overflow")),
        other => panic!("expected Internal error, got {other:?}"),
    }
    assert_eq!(shard_state.revision, ShardLeaseRevision(u64::MAX));
}

#[test]
fn binary_codec_roundtrip_is_exact() {
    let mut shard_state = shard_state_with(8, &[(1, 1, &[0, 1, 2]), (2, 2, &[5])]);
    shard_state
        .executor_leases
        .get_mut(&executor(2))
        .unwrap()
        .pod_name = Some("worker-executor-1".to_string());
    shard_state
        .executor_leases
        .get_mut(&executor(2))
        .unwrap()
        .granted_at = Timestamp::new(1_700_000_000, 123_456_789).unwrap();
    shard_state.assign_shard(executor(2), shard(0));
    shard_state.remove_executor(executor(1));
    shard_state.bump_revision().unwrap();

    let bytes = golem_common::serialization::serialize(&shard_state).unwrap();
    let decoded: ShardLeaseState = golem_common::serialization::deserialize(&bytes).unwrap();
    assert_eq!(decoded, shard_state);
}

#[test]
fn proto_conversion_emits_one_entry_per_routable_shard() {
    let mut shard_state = shard_state_with(8, &[(1, 1, &[0, 1]), (2, 2, &[5])]);
    // violate invariant 1 on purpose: an assignment whose executor holds no lease
    shard_state.shard_assignments.insert(
        shard(7),
        ShardAssignmentEntry {
            executor_id: executor(99),
            epoch: ShardEpoch(3),
        },
    );
    assert!(shard_state.check_invariants().is_err());

    let proto: golem::shardmanager::RoutingTable = shard_state.into();
    assert_eq!(proto.number_of_shards, 8);
    assert_eq!(proto.shard_assignments.len(), 3);
    for entry in &proto.shard_assignments {
        assert!(entry.pod.is_some());
        assert!(entry.shard_id.is_some());
    }
    let mut routed: Vec<i64> = proto
        .shard_assignments
        .iter()
        .map(|entry| entry.shard_id.unwrap().value)
        .collect();
    routed.sort();
    assert_eq!(routed, vec![0, 1, 5]);
}

#[test]
fn check_invariants_detects_each_violation() {
    let good = shard_state_with(4, &[(1, 1, &[0]), (2, 2, &[1])]);
    assert!(good.check_invariants().is_ok());

    let mut lease_less_owner = good.clone();
    lease_less_owner.shard_assignments.insert(
        shard(2),
        ShardAssignmentEntry {
            executor_id: executor(99),
            epoch: ShardEpoch(0),
        },
    );
    assert!(lease_less_owner.check_invariants().is_err());

    let mut duplicate_addr = good.clone();
    duplicate_addr
        .executor_leases
        .get_mut(&executor(2))
        .unwrap()
        .addr = addr(1);
    assert!(duplicate_addr.check_invariants().is_err());

    let mut out_of_range = good.clone();
    out_of_range.shard_assignments.insert(
        shard(4),
        ShardAssignmentEntry {
            executor_id: executor(1),
            epoch: ShardEpoch(0),
        },
    );
    assert!(out_of_range.check_invariants().is_err());

    let mut pending_and_assigned = good.clone();
    pending_and_assigned.pending_rebalance.insert(shard(0));
    assert!(pending_and_assigned.check_invariants().is_err());

    let mut pending_out_of_range = good.clone();
    pending_out_of_range.pending_rebalance.insert(shard(9));
    assert!(pending_out_of_range.check_invariants().is_err());

    let mut epoch_mismatch = good.clone();
    epoch_mismatch.shard_epochs.insert(shard(0), ShardEpoch(7));
    assert!(epoch_mismatch.check_invariants().is_err());

    let mut epoch_missing = good.clone();
    epoch_missing.shard_epochs.remove(&shard(0));
    assert!(epoch_missing.check_invariants().is_err());

    let mut epoch_out_of_range = good;
    epoch_out_of_range
        .shard_epochs
        .insert(shard(9), ShardEpoch(0));
    assert!(epoch_out_of_range.check_invariants().is_err());
}

#[test]
fn display_is_readable() {
    let mut shard_state = shard_state_with(8, &[(1, 1, &[0, 1, 2, 5])]);
    shard_state
        .executor_leases
        .get_mut(&executor(1))
        .unwrap()
        .pod_name = Some("worker-executor-0".to_string());
    let rendered = shard_state.to_string();
    assert!(rendered.contains("number_of_shards: 8"));
    assert!(
        rendered.contains("10.0.0.1:9001 worker-executor-0: [<0>..<2>, <5>]"),
        "{rendered}"
    );
    assert!(rendered.contains("pending_rebalance: []"));
}
