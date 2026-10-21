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
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use test_r::test;
use uuid::Uuid;

use super::{ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId, ShardLeaseRevision};

test_r::enable!();

fn epochs(entries: impl IntoIterator<Item = (i64, u64)>) -> HashMap<ShardId, ShardEpoch> {
    entries
        .into_iter()
        .map(|(shard_id, epoch)| (ShardId::new(shard_id), ShardEpoch(epoch)))
        .collect()
}

fn in_secs(seconds: u64) -> Instant {
    Instant::now() + Duration::from_secs(seconds)
}

/// The manager process behind every delivery in the tests that do not care which one sent it.
const MANAGER: Uuid = Uuid::from_u128(0x5eed);

/// The push says "exactly these"; anything absent is dropped.
#[test]
fn set_shards_replaces_the_set_rather_than_merging_into_it() {
    let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0), ShardId::new(1)]);

    let outcome = assignment.set_shards(8, &epochs([(1, 4)]), revision_of(MANAGER, 1));

    assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
    assert!(!assignment.contains(&ShardId::new(0)));
    assert_eq!(assignment.epoch_of(&ShardId::new(1)), Some(ShardEpoch(4)));
    assert_eq!(assignment.len(), 1);
    assert_eq!(assignment.revision, Some(revision_of(MANAGER, 1)));
}

/// Two deliveries can cross on the network. A renewal reply read from an
/// older state than a push must not undo the push when it arrives second:
/// ordering by revision is what keeps both delivery paths safe. The lease
/// clock still moves, because the grant answers this executor's own
/// request, anchored before that request went out, and a push carries no
/// lease that could have made this one redundant.
#[test]
fn a_stale_grant_keeps_the_set_but_still_moves_the_lease_clock() {
    let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);
    assignment.set_shards(8, &epochs([(0, 1), (5, 2)]), revision_of(MANAGER, 7));

    let granted = in_secs(60);
    let outcome = assignment.adopt_grant(None, &epochs([(0, 1)]), granted, revision_of(MANAGER, 6));

    assert_eq!(
        outcome,
        ShardDeliveryOutcome::Stale {
            delivered: revision_of(MANAGER, 6),
            applied: revision_of(MANAGER, 7),
        }
    );
    assert_eq!(
        assignment.shard_epochs,
        epochs([(0, 1), (5, 2)]),
        "the older delivery narrowed the set the newer one had just widened"
    );
    assert_eq!(assignment.revision, Some(revision_of(MANAGER, 7)));
    assert_eq!(
        assignment.expires_at,
        Some(granted),
        "a grant's lease is about time, not about the set; a stale set must not cost the lease"
    );
}

/// Equal revisions come from the same persisted state and carry the same
/// set, so a renewal at the revision of the last push applies rather than
/// being dropped.
#[test]
fn a_delivery_at_the_same_revision_is_applied() {
    let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);
    assignment.set_shards(8, &epochs([(0, 1)]), revision_of(MANAGER, 7));

    let refreshed = in_secs(60);
    let outcome =
        assignment.adopt_grant(None, &epochs([(0, 1)]), refreshed, revision_of(MANAGER, 7));

    assert_eq!(
        outcome,
        ShardDeliveryOutcome::Applied { set_changed: false }
    );
    assert_eq!(assignment.expires_at, Some(refreshed));
}

/// A push has no request of this executor's to anchor a lease to, so it
/// carries none: neither a full replace nor a revoke moves the lease clock,
/// whatever revision they carry.
#[test]
fn a_push_never_moves_the_lease_clock() {
    let mut assignment = ShardAssignment::default();
    let granted = in_secs(60);
    assignment.adopt_grant(
        Some(8),
        &epochs([(0, 1), (1, 1)]),
        granted,
        revision_of(MANAGER, 1),
    );

    assignment.set_shards(
        8,
        &epochs([(0, 1), (1, 1), (2, 1)]),
        revision_of(MANAGER, 2),
    );
    assert_eq!(
        assignment.expires_at,
        Some(granted),
        "a full-replace push must leave the lease where the grant put it"
    );

    assignment.revoke_shards(&HashSet::from([ShardId::new(2)]), revision_of(MANAGER, 3));
    assert_eq!(
        assignment.expires_at,
        Some(granted),
        "a revoke must leave the lease where the grant put it"
    );
}

/// The executor takes each grant as the manager's current word and keeps
/// no older, longer deadline of its own. The manager never issues a
/// shorter one, so this pins which side would ever be the one extending a
/// lease: neither.
#[test]
fn a_grant_with_a_shorter_lease_shortens_it() {
    let mut assignment = ShardAssignment::default();
    let now = Instant::now();
    assignment.adopt_grant(
        Some(8),
        &epochs([(0, 1)]),
        now + Duration::from_secs(60),
        revision_of(MANAGER, 1),
    );

    assignment.adopt_grant(
        None,
        &epochs([(0, 1)]),
        now + Duration::from_secs(30),
        revision_of(MANAGER, 2),
    );

    assert_eq!(assignment.expires_at, Some(now + Duration::from_secs(30)));
}

/// A revoke is a delta rather than a full set, but it is gated the same
/// way: once it has applied, a grant read before the shard moved is older
/// than it and cannot put the shard back.
#[test]
fn a_revoke_is_gated_by_revision_like_every_other_delivery() {
    let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0), ShardId::new(1)]);
    let expiry = in_secs(60);
    assignment.adopt_grant(
        Some(8),
        &epochs([(0, 1), (1, 1)]),
        expiry,
        revision_of(MANAGER, 3),
    );
    let revoked = HashSet::from([ShardId::new(0)]);

    let stale = assignment.revoke_shards(&revoked, revision_of(MANAGER, 2));
    assert_eq!(
        stale,
        ShardDeliveryOutcome::Stale {
            delivered: revision_of(MANAGER, 2),
            applied: revision_of(MANAGER, 3),
        }
    );
    assert!(
        assignment.contains(&ShardId::new(0)),
        "a revoke older than the last delivery applied must be ignored"
    );

    let applied = assignment.revoke_shards(&revoked, revision_of(MANAGER, 5));
    assert_eq!(applied, ShardDeliveryOutcome::Applied { set_changed: true });
    assert!(!assignment.contains(&ShardId::new(0)));
    assert_eq!(
        assignment.revision,
        Some(revision_of(MANAGER, 5)),
        "the revoke's revision is recorded like any other delivery's"
    );
    assert_eq!(
        assignment.expires_at,
        Some(expiry),
        "a revoke drops shards; it does not touch the lease"
    );

    let late_grant = assignment.adopt_grant(
        None,
        &epochs([(0, 1), (1, 1)]),
        in_secs(60),
        revision_of(MANAGER, 4),
    );
    assert!(matches!(late_grant, ShardDeliveryOutcome::Stale { .. }));
    assert!(
        !assignment.contains(&ShardId::new(0)),
        "a grant read before the shard moved, arriving after the revoke, must not restore it"
    );
}

/// The corrective delivery: a renewal that answers with a different set
/// than was held is applied like a push, and reports the set moved so
/// the caller sweeps and recovers.
#[test]
fn a_renewal_that_changes_the_set_reports_it() {
    let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0), ShardId::new(1)]);
    assignment.set_shards(8, &epochs([(0, 1), (1, 1)]), revision_of(MANAGER, 3));

    let outcome = assignment.adopt_grant(
        None,
        &epochs([(1, 1), (2, 5)]),
        in_secs(60),
        revision_of(MANAGER, 4),
    );

    assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
    assert!(
        !assignment.contains(&ShardId::new(0)),
        "the dropped shard is gone"
    );
    assert_eq!(assignment.epoch_of(&ShardId::new(2)), Some(ShardEpoch(5)));
    assert_eq!(assignment.revision, Some(revision_of(MANAGER, 4)));
}

fn revision_of(incarnation: Uuid, number: u64) -> ShardLeaseRevision {
    ShardLeaseRevision {
        incarnation,
        number,
    }
}

/// A manager that came back on a wiped or restored store counts its revisions from far below
/// the last one this executor applied. Its reply is still the manager in charge speaking.
#[test]
fn a_reply_from_another_manager_process_starts_the_revisions_over() {
    let (old_manager, new_manager) = (Uuid::new_v4(), Uuid::new_v4());
    let mut assignment = ShardAssignment::default();
    assignment.adopt_grant(
        Some(8),
        &epochs([(0, 4)]),
        in_secs(60),
        revision_of(old_manager, 10_000),
    );
    // what a lost lease does before the re-registration
    assignment.clear(Instant::now());

    let outcome = assignment.adopt_grant(
        Some(8),
        &epochs([(0, 5)]),
        in_secs(60),
        revision_of(new_manager, 3),
    );

    assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
    assert_eq!(assignment.epoch_of(&ShardId::new(0)), Some(ShardEpoch(5)));
    assert_eq!(assignment.revision, Some(revision_of(new_manager, 3)));
}

/// Starting the revisions over must not be a way back in for the manager that was left: its
/// delayed push carries a revision far above the new manager's, and would win on the number.
#[test]
fn a_push_from_a_manager_process_that_is_not_followed_is_ignored() {
    let (old_manager, new_manager) = (Uuid::new_v4(), Uuid::new_v4());
    let mut assignment = ShardAssignment::default();
    assignment.adopt_grant(
        Some(8),
        &epochs([(0, 4)]),
        in_secs(60),
        revision_of(old_manager, 10_000),
    );

    // The new manager's push arrives before any reply from it: nothing says yet that it is
    // the one in charge.
    let early = assignment.set_shards(8, &epochs([(1, 1)]), revision_of(new_manager, 2));
    assert_eq!(
        early,
        ShardDeliveryOutcome::FromAnotherManager {
            delivered: revision_of(new_manager, 2),
            applied: revision_of(old_manager, 10_000),
        }
    );
    assert_eq!(assignment.shard_epochs, epochs([(0, 4)]));

    assignment.adopt_grant(
        None,
        &epochs([(0, 5)]),
        in_secs(60),
        revision_of(new_manager, 3),
    );

    let delayed_push = assignment.set_shards(
        8,
        &epochs([(0, 4), (1, 4)]),
        revision_of(old_manager, 10_001),
    );
    let delayed_revoke = assignment.revoke_shards(
        &HashSet::from([ShardId::new(0)]),
        revision_of(old_manager, 10_002),
    );
    for delayed in [delayed_push, delayed_revoke] {
        assert!(
            matches!(delayed, ShardDeliveryOutcome::FromAnotherManager { .. }),
            "got {delayed:?}"
        );
    }
    assert_eq!(assignment.shard_epochs, epochs([(0, 5)]));
    assert_eq!(assignment.revision, Some(revision_of(new_manager, 3)));

    // The followed manager's own pushes are ordered by number as ever.
    let push = assignment.set_shards(8, &epochs([(0, 5), (2, 1)]), revision_of(new_manager, 4));
    assert_eq!(push, ShardDeliveryOutcome::Applied { set_changed: true });
}

/// Every delivery names the process that sent it. One that names none is malformed, not
/// ordered by its number alone: that would let a deposed manager's delayed push back in.
#[test]
fn a_delivery_must_name_its_manager_process() {
    let manager = Uuid::new_v4();
    assert_eq!(
        ShardLeaseRevision::from_wire(&manager.to_string(), 3),
        Ok(revision_of(manager, 3))
    );
    for malformed in ["", "not-a-uuid"] {
        assert!(
            ShardLeaseRevision::from_wire(malformed, 3).is_err(),
            "{malformed:?} must not decode"
        );
    }
}

/// `clear()` lapses the lease as of `now`. `None` would mean
/// "never expires", which would leave a fenced executor reading as ready.
#[test]
fn clear_lapses_the_lease_instead_of_making_it_unexpiring() {
    let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);
    let now = Instant::now();
    assert!(assignment.lease_is_live(now));

    assignment.clear(now);

    assert!(assignment.is_empty());
    assert_eq!(assignment.expires_at, Some(now));
    assert!(
        !assignment.lease_is_live(now),
        "a cleared assignment is lapsed, not never-expiring"
    );
    assert!(!assignment.lease_is_live(now + Duration::from_secs(1)));
}

/// The single-shard implementations run with no expiry at all and must
/// never fence themselves.
#[test]
fn a_lease_without_an_expiry_is_always_live() {
    let assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);

    assert!(assignment.lease_is_live(Instant::now()));
    assert!(assignment.lease_is_live(in_secs(365 * 24 * 3600)));
}
