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

use super::*;
use std::time::{Duration, Instant};
use test_r::test;
use uuid::Uuid;

test_r::enable!();

/// Big enough that distinct agent ids land on distinct shards, small enough
/// that the search below always finds one.
const SHARDS: usize = 8;

fn epochs(entries: impl IntoIterator<Item = (i64, u64)>) -> HashMap<ShardId, ShardEpoch> {
    entries
        .into_iter()
        .map(|(shard_id, epoch)| (ShardId::new(shard_id), ShardEpoch(epoch)))
        .collect()
}

/// The manager process behind every delivery in the tests that do not care which one sent it.
const MANAGER: Uuid = Uuid::from_u128(0x5eed);

fn revision_of(incarnation: Uuid, number: u64) -> ShardLeaseRevision {
    ShardLeaseRevision {
        incarnation,
        number,
    }
}

/// An agent id that routes to `shard`, found by search because
/// `ShardId::from_agent_id` is a hash.
fn agent_on_shard(shard: i64) -> AgentId {
    for candidate in 0..10_000 {
        let agent_id = crate::test_agent_id(&format!("agent-{candidate}"));
        if ShardId::from_agent_id(&agent_id, SHARDS) == ShardId::new(shard) {
            return agent_id;
        }
    }
    panic!("no agent id in the search space routes to shard {shard}");
}

fn service_holding(
    shard_epochs: &HashMap<ShardId, ShardEpoch>,
    lease: Option<ShardLeaseClock>,
) -> ShardServiceDefault {
    let service = ShardServiceDefault::new();
    service.register(
        SHARDS,
        shard_epochs,
        lease,
        revision_of(MANAGER, 0),
        NonZeroUsize::MIN,
    );
    service
}

/// A lease that expires now is not live at any later check: `lease_is_live` is strict.
fn lapsed() -> ShardLeaseClock {
    let wall_anchor = SystemTime::now();
    ShardLeaseClock {
        sent_at: Instant::now(),
        wall_anchor,
        ttl: Duration::ZERO,
    }
}

fn live() -> ShardLeaseClock {
    ShardLeaseClock {
        ttl: Duration::from_secs(60),
        ..lapsed()
    }
}

#[test]
fn executor_count_follows_the_revision_not_delivery_order() {
    let service = ShardServiceDefault::new();
    let held = epochs([(0, 1)]);
    assert_eq!(service.executor_count(), None);

    service.register(
        SHARDS,
        &held,
        Some(lapsed()),
        revision_of(MANAGER, 4),
        NonZeroUsize::new(2).unwrap(),
    );
    assert_eq!(service.executor_count(), Some(2));
    let snapshot = service.try_get_current_assignment().unwrap();
    assert_eq!(snapshot.executor_count.get(), 2);
    assert_eq!(snapshot.shard_epochs, held);
    service
        .assign_shards(
            SHARDS,
            &held,
            revision_of(MANAGER, 6),
            NonZeroUsize::new(3).unwrap(),
        )
        .unwrap();
    assert_eq!(service.executor_count(), Some(3));
    assert_eq!(
        service
            .assign_shards(
                SHARDS,
                &held,
                revision_of(MANAGER, 6),
                NonZeroUsize::new(4).unwrap()
            )
            .unwrap(),
        ShardDeliveryOutcome::Applied { set_changed: false }
    );
    assert_eq!(service.executor_count(), Some(4));

    let expiry = live();
    service
        .update_lease(
            &held,
            expiry,
            revision_of(MANAGER, 5),
            NonZeroUsize::new(2).unwrap(),
        )
        .unwrap();
    assert_eq!(service.executor_count(), Some(4));
    assert_eq!(
        service.current_assignment().unwrap().expires_at(),
        Some(expiry.expires_at())
    );

    service
        .update_lease(
            &held,
            expiry,
            revision_of(MANAGER, 7),
            NonZeroUsize::new(4).unwrap(),
        )
        .unwrap();
    service
        .assign_shards(
            SHARDS,
            &held,
            revision_of(MANAGER, 6),
            NonZeroUsize::new(3).unwrap(),
        )
        .unwrap();
    assert_eq!(service.executor_count(), Some(4));

    let next_manager = Uuid::from_u128(0xbeef);
    service
        .assign_shards(
            SHARDS,
            &held,
            revision_of(next_manager, 100),
            NonZeroUsize::new(9).unwrap(),
        )
        .unwrap();
    assert_eq!(service.executor_count(), Some(4));
    service.register(
        SHARDS,
        &held,
        Some(expiry),
        revision_of(next_manager, 1),
        NonZeroUsize::new(2).unwrap(),
    );
    assert_eq!(service.executor_count(), Some(2));
    service
        .assign_shards(
            SHARDS,
            &held,
            revision_of(MANAGER, 200),
            NonZeroUsize::new(8).unwrap(),
        )
        .unwrap();
    assert_eq!(service.executor_count(), Some(2));

    service
        .revoke_shards(
            &HashSet::from([ShardId::new(0)]),
            revision_of(next_manager, 2),
            NonZeroUsize::new(5).unwrap(),
        )
        .unwrap();
    assert_eq!(service.executor_count(), Some(5));
    service.install_unexpiring(SHARDS, &held);
    assert_eq!(service.executor_count(), Some(1));
}

/// Nothing is installed until a registration: a push, a renewal or a
/// revoke that arrives first is refused, never applied to a placeholder
/// whose shard count of zero the routing hash would divide by.
#[test]
fn a_delivery_before_registration_is_refused() {
    let service = ShardServiceDefault::new();

    // The variant is the contract, not merely that it failed: the worker service answers
    // `ShardingNotReady` by refreshing its routing table and retrying, and answers an opaque
    // error by failing the call.
    for refused in [
        service.assign_shards(
            SHARDS,
            &epochs([(0, 1)]),
            revision_of(MANAGER, 1),
            NonZeroUsize::MIN,
        ),
        service.update_lease(
            &epochs([(0, 1)]),
            live(),
            revision_of(MANAGER, 1),
            NonZeroUsize::MIN,
        ),
        service.revoke_shards(
            &HashSet::from([ShardId::new(0)]),
            revision_of(MANAGER, 1),
            NonZeroUsize::MIN,
        ),
    ] {
        assert!(
            matches!(refused, Err(WorkerExecutorError::ShardingNotReady)),
            "a delivery before registration must be refused as ShardingNotReady, got {refused:?}"
        );
    }
    assert!(!service.is_ready());
    assert!(service.try_get_current_assignment().is_none());
}

/// The revision gate is silent by design - dropping the older of two crossed deliveries is
/// correct, so it only warns. That makes a *sustained* drop rate invisible in logs, which is
/// what the counter is for.
#[test]
fn a_dropped_delivery_is_counted_and_not_only_logged() {
    let service = ShardServiceDefault::new();
    service.register(
        SHARDS,
        &epochs([(0, 1), (1, 1)]),
        Some(live()),
        revision_of(MANAGER, 5),
        NonZeroUsize::MIN,
    );

    let before = stale_shard_delivery_count(ShardDelivery::Renewal);
    let outcome = service
        .update_lease(
            &epochs([(0, 1)]),
            live(),
            revision_of(MANAGER, 3),
            NonZeroUsize::MIN,
        )
        .expect("a registered executor can be renewed");

    assert!(
        matches!(outcome, ShardDeliveryOutcome::Stale { .. }),
        "a renewal below the applied revision is dropped"
    );
    assert!(
        stale_shard_delivery_count(ShardDelivery::Renewal) > before,
        "and dropping it moves the counter for its own delivery kind"
    );
}

/// A revoke is a delta rather than a full set, and it is gated on the revision like every
/// other delivery: one read from a state older than what this executor has already applied is
/// dropped whole, so a revoke that crossed a newer push on the network cannot take a shard the
/// newer push had just granted.
#[test]
fn a_revoke_older_than_the_last_delivery_is_ignored() {
    let service = ShardServiceDefault::new();
    service.register(
        SHARDS,
        &epochs([(0, 1), (1, 1)]),
        Some(live()),
        revision_of(MANAGER, 5),
        NonZeroUsize::MIN,
    );

    let outcome = service
        .revoke_shards(
            &HashSet::from([ShardId::new(0)]),
            revision_of(MANAGER, 3),
            NonZeroUsize::MIN,
        )
        .expect("a registered executor can be revoked from");

    assert_eq!(
        outcome,
        ShardDeliveryOutcome::Stale {
            delivered: revision_of(MANAGER, 3),
            applied: revision_of(MANAGER, 5),
        }
    );
    assert_eq!(
        service.current_assignment().unwrap().shard_id_set(),
        HashSet::from([ShardId::new(0), ShardId::new(1)]),
        "a stale revoke must leave the set exactly as the newer delivery left it"
    );
    assert!(
        service.check_worker(&agent_on_shard(0)).is_ok(),
        "and the shard it named is still this executor's"
    );

    // ...while one at or above the applied revision does take the shard.
    let outcome = service
        .revoke_shards(
            &HashSet::from([ShardId::new(0)]),
            revision_of(MANAGER, 5),
            NonZeroUsize::MIN,
        )
        .expect("a registered executor can be revoked from");
    assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
    assert_eq!(
        service.current_assignment().unwrap().shard_id_set(),
        HashSet::from([ShardId::new(1)])
    );
}

/// `AssignShards` says "your shards are exactly these". Anything
/// absent is dropped, and the sweep in `assign_shards_internal` retires
/// exactly the agents the new set rejects.
#[test]
fn a_full_replace_push_drops_unlisted_shards_and_their_agents() {
    let service = service_holding(&epochs([(0, 1), (1, 1)]), None);
    let on_dropped = agent_on_shard(0);
    let on_kept = agent_on_shard(1);
    assert!(service.check_worker(&on_dropped).is_ok());
    assert!(service.check_worker(&on_kept).is_ok());

    service
        .assign_shards(
            SHARDS,
            &epochs([(1, 1)]),
            revision_of(MANAGER, 1),
            NonZeroUsize::MIN,
        )
        .unwrap();

    assert_eq!(
        service.current_assignment().unwrap().shard_id_set(),
        HashSet::from([ShardId::new(1)]),
        "a shard absent from the push must be dropped, not merged"
    );
    assert!(
        service.check_worker(&on_dropped).is_err(),
        "an agent on a dropped shard is what the retirement sweep picks up"
    );
    assert!(service.check_worker(&on_kept).is_ok());
}

/// The self-fence. It surfaces as `ShardingNotReady`, which the worker
/// service answers by refreshing its routing table and retrying.
#[test]
fn admission_is_refused_once_the_lease_has_lapsed() {
    let agent = agent_on_shard(0);
    let service = service_holding(&epochs([(0, 3)]), Some(live()));
    assert!(service.is_ready());
    assert!(service.check_admission(&agent).is_ok());

    service
        .update_lease(
            &epochs([(0, 3)]),
            lapsed(),
            revision_of(MANAGER, 1),
            NonZeroUsize::MIN,
        )
        .unwrap();

    assert!(
        matches!(
            service.check_admission(&agent),
            Err(WorkerExecutorError::ShardingNotReady)
        ),
        "the fence must surface as ShardingNotReady, not as an opaque Unknown"
    );
    assert!(!service.is_ready(), "and the scheduler must stop claiming");
}

/// Routing decisions stay pure set membership: a fenced answer here would
/// send the call straight back to this executor.
#[test]
fn routing_checks_are_not_fenced_by_a_lapsed_lease() {
    let agent = agent_on_shard(0);
    let service = service_holding(&epochs([(0, 3)]), Some(lapsed()));

    assert!(
        service.check_worker(&agent).is_ok(),
        "routing must still say 'mine' for a shard this executor holds"
    );
    assert!(
        service.check_admission(&agent).is_err(),
        "while admission is fenced"
    );
}

/// `None` means "never expires": the single-shard adapter has no lease to renew.
#[test]
fn a_lease_without_an_expiry_never_fences() {
    let agent = agent_on_shard(0);
    let service = service_holding(&epochs([(0, 0)]), None);

    assert!(service.is_ready());
    assert!(service.check_admission(&agent).is_ok());
    assert!(service.check_worker(&agent).is_ok());
}

/// Clearing after `LeaseNotFound` leaves the lease lapsed, not
/// never-expiring, so admission keeps refusing until a re-registration
/// installs a fresh grant.
#[test]
fn clearing_the_assignment_leaves_the_lease_lapsed() {
    let agent = agent_on_shard(0);
    let service = service_holding(&epochs([(0, 3)]), Some(live()));

    service.clear_assignment();

    let assignment = service.current_assignment().unwrap();
    assert!(assignment.is_empty(), "every shard is dropped");
    assert!(
        assignment.expires_at().is_some(),
        "a cleared lease is lapsed, never 'never expires'"
    );
    assert!(!service.is_ready());
    assert!(matches!(
        service.check_admission(&agent),
        Err(WorkerExecutorError::ShardingNotReady)
    ));
}

/// A renewal reply that crossed a newer push is stale about the set, not about time: it
/// answers this executor's own request, so its lease is adopted even though its set is not.
/// Refusing it would leave an executor the manager keeps renewing fenced by its own clock.
#[test]
fn a_stale_renewal_still_moves_the_lease_clock() {
    let service = ShardServiceDefault::new();
    service.register(
        SHARDS,
        &epochs([(0, 1), (1, 1)]),
        Some(lapsed()),
        revision_of(MANAGER, 5),
        NonZeroUsize::MIN,
    );
    assert!(!service.is_ready());

    let outcome = service
        .update_lease(
            &epochs([(0, 1)]),
            live(),
            revision_of(MANAGER, 3),
            NonZeroUsize::MIN,
        )
        .expect("a registered executor can be renewed");

    assert_eq!(
        outcome,
        ShardDeliveryOutcome::Stale {
            delivered: revision_of(MANAGER, 3),
            applied: revision_of(MANAGER, 5),
        }
    );
    assert_eq!(
        service.current_assignment().unwrap().shard_id_set(),
        HashSet::from([ShardId::new(0), ShardId::new(1)]),
        "the stale set is not adopted"
    );
    assert!(service.is_ready(), "but the lease it granted is");
}

#[test]
fn write_deadlines_require_the_held_epoch_and_a_bounded_expiring_lease() {
    let service = ShardServiceDefault::with_max_lease_ttl(Duration::from_secs(60));
    let agent = agent_on_shard(0);
    let margin = Duration::from_secs(3);
    assert_eq!(
        service.lease_valid_until(&agent, ShardEpoch(5), margin),
        None
    );

    let wall_anchor = SystemTime::now() - Duration::from_secs(1);
    let lease = ShardLeaseClock {
        sent_at: Instant::now(),
        wall_anchor,
        ttl: Duration::from_secs(300),
    };
    service.register(
        SHARDS,
        &epochs([(0, 5), (1, 8)]),
        Some(lease),
        revision_of(MANAGER, 1),
        NonZeroUsize::MIN,
    );
    assert_eq!(
        service.current_assignment().unwrap().expires_at(),
        Some(lease.sent_at + Duration::from_secs(60))
    );
    assert_eq!(
        service.lease_valid_until(&agent, ShardEpoch(5), margin),
        Some(wall_anchor + Duration::from_secs(57))
    );
    assert_eq!(
        service.lease_valid_until(&agent, ShardEpoch(4), margin),
        None
    );

    service
        .update_lease(
            &epochs([(1, 8)]),
            lease,
            revision_of(MANAGER, 2),
            NonZeroUsize::MIN,
        )
        .unwrap();
    assert_eq!(
        service.lease_valid_until(&agent, ShardEpoch(5), margin),
        None
    );
    service.install_unexpiring(SHARDS, &epochs([(0, 5)]));
    assert_eq!(
        service.lease_valid_until(&agent, ShardEpoch(5), margin),
        None
    );
}
