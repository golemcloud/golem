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

use crate::metrics::sharding::*;
use crate::model::ShardAssignmentCheck;
use golem_common::model::sharding::{
    ShardAssignmentEvent, ShardAssignmentState, transition_shard_assignment,
};
use golem_common::model::{
    AgentId, ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId, ShardLeaseRevision,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, HashSet};
use std::convert::identity;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tracing::debug;

/// Service for assigning shards to worker executors.
pub trait ShardService: Send + Sync {
    /// True when an assignment exists and its lease is live.
    fn is_ready(&self) -> bool;
    /// Full replace from an `AssignShards` push: hold exactly `shard_epochs`,
    /// drop everything else. Ignored whole if `revision` is older than the last
    /// delivery applied, so a push and a renewal reply that cross on the
    /// network cannot leave the older set in place.
    fn assign_shards(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError>;
    /// Checks shard membership without checking the lease.
    fn check_worker(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError>;
    /// Checks shard membership and a live lease. A lapsed lease refuses new
    /// work but never interrupts work that is already running.
    fn check_admission(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError>;
    /// Installs the first assignment, from a registration's grant. Creates the
    /// assignment if none exists yet. The set gates on `revision` like every
    /// delivery; the lease clock always moves. `None` never expires.
    fn register(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<Instant>,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> ShardDeliveryOutcome;
    /// Installs the single-shard executor's assignment, which no shard manager
    /// delivers: it never expires, names no revision, and replaces whatever is
    /// held.
    fn install_unexpiring(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
    ) -> ShardDeliveryOutcome;
    /// A revoke: the shards named are dropped, everything else stays. The one
    /// delta among the deliveries, gated on `revision` like the rest; the
    /// shard manager only ever sends it for shards its store has already moved
    /// away, at the revision that move was stored under.
    fn revoke_shards(
        &self,
        shard_ids: &HashSet<ShardId>,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError>;
    /// Applies the manager's complete shard set and a new expiry anchored where
    /// the renewal was sent. `set_changed` reports a change to the held set.
    /// The set gates on `revision`; the lease clock always moves, even when
    /// the set is stale.
    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError>;
    /// Drops every shard and lapses the lease. Retains the manager lineage,
    /// shard count and executor count. A later registration or renewal can
    /// restore readiness.
    fn clear_assignment(&self);
    fn current_assignment(&self) -> Result<ShardAssignment, WorkerExecutorError>;
    fn try_get_current_assignment(&self) -> Option<ShardAssignment>;
    /// The last accepted executor count, or `None` when no assignment is installed.
    fn executor_count(&self) -> Option<usize>;
}

pub struct ShardServiceDefault {
    shard_assignment: Arc<RwLock<ShardAssignmentState>>,
}

impl Default for ShardServiceDefault {
    fn default() -> Self {
        Self::new()
    }
}

impl ShardServiceDefault {
    pub fn new() -> Self {
        Self {
            shard_assignment: Arc::new(RwLock::new(ShardAssignmentState::default())),
        }
    }

    fn with_read_shard_assignment<F, O>(&self, f: F) -> Result<O, WorkerExecutorError>
    where
        F: Fn(&ShardAssignment) -> O,
    {
        let guard = self.shard_assignment.read().unwrap();
        match guard.as_ref() {
            Some(shard_assignment) => Ok(f(shard_assignment)),
            None => Err(sharding_not_ready_error()),
        }
    }

    fn apply(&self, event: ShardAssignmentEvent<'_>) -> Option<ShardDeliveryOutcome> {
        let mut guard = self.shard_assignment.write().unwrap();
        debug!(?event, current = ?guard.as_ref(), "Applying shard assignment event");
        let delivery = match event {
            ShardAssignmentEvent::Register { .. }
            | ShardAssignmentEvent::InstallUnexpiring { .. } => Some(ShardDelivery::Register),
            ShardAssignmentEvent::Assign { .. } => Some(ShardDelivery::Assign),
            ShardAssignmentEvent::Renew { .. } => Some(ShardDelivery::Renewal),
            ShardAssignmentEvent::Revoke { .. } => Some(ShardDelivery::Revoke),
            ShardAssignmentEvent::Clear { .. } => None,
        };
        let (next, outcome) = transition_shard_assignment(std::mem::take(&mut *guard), event);
        *guard = next;
        match (delivery, guard.as_ref(), outcome.as_ref()) {
            (Some(delivery), Some(current), Some(outcome)) => {
                record_delivery(delivery, outcome, current);
            }
            (None, Some(_), _) => {
                if let ShardAssignmentEvent::Clear { now } = event {
                    record_assigned_shard_count(0);
                    record_shard_lease_expiry(now);
                }
            }
            _ => {}
        }
        outcome
    }
}

impl ShardService for ShardServiceDefault {
    fn is_ready(&self) -> bool {
        let now = Instant::now();
        self.shard_assignment
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|shard_assignment| shard_assignment.lease_is_live(now))
    }

    fn assign_shards(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.apply(ShardAssignmentEvent::Assign {
            number_of_shards,
            shard_epochs,
            revision,
            executor_count,
        })
        .ok_or_else(sharding_not_ready_error)
    }

    fn check_worker(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError> {
        self.with_read_shard_assignment(|shard_assignment: &ShardAssignment| {
            shard_assignment.check_worker(agent_id)
        })
        .and_then(identity)
    }

    fn check_admission(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError> {
        let now = Instant::now();
        let guard = self.shard_assignment.read().unwrap();
        let Some(shard_assignment) = guard.as_ref() else {
            record_shard_admission_refusal(ShardAdmissionRefusal::NotRegistered);
            return Err(sharding_not_ready_error());
        };
        if !shard_assignment.lease_is_live(now) {
            record_shard_admission_refusal(ShardAdmissionRefusal::LeaseLapsed);
            return Err(shard_lease_expired_error());
        }
        shard_assignment
            .check_worker(agent_id)
            .inspect_err(|_| record_shard_admission_refusal(ShardAdmissionRefusal::NotOwned))
    }

    fn current_assignment(&self) -> Result<ShardAssignment, WorkerExecutorError> {
        self.with_read_shard_assignment(|shard_assignment| shard_assignment.clone())
    }

    fn register(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<Instant>,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> ShardDeliveryOutcome {
        self.apply(ShardAssignmentEvent::Register {
            number_of_shards,
            shard_epochs,
            expires_at,
            revision,
            executor_count,
        })
        .expect("registration always delivers an assignment")
    }

    fn install_unexpiring(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
    ) -> ShardDeliveryOutcome {
        self.apply(ShardAssignmentEvent::InstallUnexpiring {
            number_of_shards,
            shard_epochs,
        })
        .expect("standalone installation always delivers an assignment")
    }

    fn revoke_shards(
        &self,
        shard_ids: &HashSet<ShardId>,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.apply(ShardAssignmentEvent::Revoke {
            shard_ids,
            revision,
            executor_count,
        })
        .ok_or_else(sharding_not_ready_error)
    }

    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
        revision: ShardLeaseRevision,
        executor_count: NonZeroUsize,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.apply(ShardAssignmentEvent::Renew {
            shard_epochs,
            expires_at,
            revision,
            executor_count,
        })
        .ok_or_else(sharding_not_ready_error)
    }

    fn clear_assignment(&self) {
        self.apply(ShardAssignmentEvent::Clear {
            now: Instant::now(),
        });
    }

    fn try_get_current_assignment(&self) -> Option<ShardAssignment> {
        self.shard_assignment.read().unwrap().as_ref().cloned()
    }

    fn executor_count(&self) -> Option<usize> {
        self.shard_assignment
            .read()
            .unwrap()
            .as_ref()
            .map(|assignment| assignment.executor_count)
    }
}

/// Records the resulting shard and executor counts.
/// Records the lease expiry when present.
/// Records a stale-revision drop when the outcome is `Stale`.
fn record_delivery(
    delivery: ShardDelivery,
    outcome: &ShardDeliveryOutcome,
    shard_assignment: &ShardAssignment,
) {
    record_assigned_shard_count(shard_assignment.len());
    record_executor_count(shard_assignment.executor_count);
    if let Some(expires_at) = shard_assignment.expires_at {
        record_shard_lease_expiry(expires_at);
    }
    if matches!(outcome, ShardDeliveryOutcome::Stale { .. }) {
        record_stale_shard_delivery(delivery);
    }
}

fn sharding_not_ready_error() -> WorkerExecutorError {
    WorkerExecutorError::ShardingNotReady
}

/// A lapsed shard lease refuses admission with the same retryable routing error.
fn shard_lease_expired_error() -> WorkerExecutorError {
    WorkerExecutorError::ShardingNotReady
}

#[cfg(test)]
mod tests {
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
        expires_at: Option<Instant>,
    ) -> ShardServiceDefault {
        let service = ShardServiceDefault::new();
        service.register(
            SHARDS,
            shard_epochs,
            expires_at,
            revision_of(MANAGER, 0),
            NonZeroUsize::new(1).unwrap(),
        );
        service
    }

    /// A lease that expires now is not live at any later check: `lease_is_live` is strict.
    fn lapsed() -> Instant {
        Instant::now()
    }

    fn live() -> Instant {
        Instant::now() + Duration::from_secs(60)
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
            service.current_assignment().unwrap().expires_at,
            Some(expiry)
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
    #[test_r::always_capture]
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
                NonZeroUsize::new(1).unwrap(),
            ),
            service.update_lease(
                &epochs([(0, 1)]),
                live(),
                revision_of(MANAGER, 1),
                NonZeroUsize::new(1).unwrap(),
            ),
            service.revoke_shards(
                &HashSet::from([ShardId::new(0)]),
                revision_of(MANAGER, 1),
                NonZeroUsize::new(1).unwrap(),
            ),
        ] {
            assert!(
                matches!(refused, Err(WorkerExecutorError::ShardingNotReady)),
                "a delivery before registration must be refused as ShardingNotReady, got {refused:?}"
            );
        }
        assert!(!service.is_ready());
        assert!(service.try_get_current_assignment().is_none());
        service.clear_assignment();
        assert!(service.try_get_current_assignment().is_none());
        assert_eq!(
            service.register(
                SHARDS,
                &epochs([]),
                Some(live()),
                revision_of(MANAGER, 1),
                NonZeroUsize::new(2).unwrap()
            ),
            ShardDeliveryOutcome::Applied { set_changed: false }
        );
        let standalone = ShardServiceDefault::new();
        assert_eq!(
            standalone.install_unexpiring(SHARDS, &epochs([])),
            ShardDeliveryOutcome::Applied { set_changed: true }
        );
        assert_eq!(standalone.executor_count(), Some(1));
        assert_eq!(standalone.current_assignment().unwrap().revision, None);
        assert_eq!(standalone.current_assignment().unwrap().expires_at, None);
    }

    #[test]
    fn registration_gates_the_set_but_always_adopts_its_expiry() {
        let service = ShardServiceDefault::new();
        let held = epochs([(0, 4)]);
        service.register(
            SHARDS,
            &held,
            Some(live()),
            revision_of(MANAGER, 6),
            NonZeroUsize::new(2).unwrap(),
        );
        let another_manager = Uuid::from_u128(0xbeef);
        assert_eq!(
            service.register(
                8,
                &epochs([(1, 1)]),
                None,
                revision_of(another_manager, 1),
                NonZeroUsize::new(9).unwrap()
            ),
            ShardDeliveryOutcome::FromAnotherManager {
                delivered: revision_of(another_manager, 1),
                applied: revision_of(MANAGER, 6),
            }
        );
        let assignment = service.current_assignment().unwrap();
        assert_eq!(assignment.shard_epochs, held);
        assert_eq!(assignment.number_of_shards, SHARDS);
        assert_eq!(assignment.executor_count, 2);
        assert_eq!(assignment.expires_at, None);
        assert_eq!(assignment.revision, Some(revision_of(MANAGER, 6)));

        service.clear_assignment();
        let expiry = live();
        assert!(matches!(
            service.register(
                8,
                &held,
                Some(expiry),
                revision_of(MANAGER, 5),
                NonZeroUsize::new(7).unwrap()
            ),
            ShardDeliveryOutcome::Stale { .. }
        ));
        let assignment = service.current_assignment().unwrap();
        assert!(assignment.is_empty());
        assert_eq!(assignment.number_of_shards, SHARDS);
        assert_eq!(assignment.executor_count, 2);
        assert_eq!(assignment.revision, Some(revision_of(MANAGER, 6)));
        assert_eq!(assignment.expires_at, Some(expiry));
    }

    /// The revision gate is silent by design - dropping the older of two crossed deliveries is
    /// correct, so it only warns. That makes a *sustained* drop rate invisible in logs, which is
    /// what the counter is for.
    #[test]
    #[test_r::always_capture]
    fn a_dropped_delivery_is_counted_and_not_only_logged() {
        let service = ShardServiceDefault::new();
        service.register(
            SHARDS,
            &epochs([(0, 1), (1, 1)]),
            Some(live()),
            revision_of(MANAGER, 5),
            NonZeroUsize::new(1).unwrap(),
        );

        let before = stale_shard_delivery_count(ShardDelivery::Renewal);
        let outcome = service
            .update_lease(
                &epochs([(0, 1)]),
                live(),
                revision_of(MANAGER, 3),
                NonZeroUsize::new(1).unwrap(),
            )
            .expect("a registered executor can be renewed");

        assert!(
            matches!(outcome, ShardDeliveryOutcome::Stale { .. }),
            "a renewal below the applied revision is dropped"
        );
        assert_eq!(
            stale_shard_delivery_count(ShardDelivery::Renewal),
            before + 1.0,
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
            NonZeroUsize::new(1).unwrap(),
        );

        let outcome = service
            .revoke_shards(
                &HashSet::from([ShardId::new(0)]),
                revision_of(MANAGER, 3),
                NonZeroUsize::new(2).unwrap(),
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
                NonZeroUsize::new(4).unwrap(),
            )
            .expect("a registered executor can be revoked from");
        assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
        assert_eq!(
            service.current_assignment().unwrap().shard_id_set(),
            HashSet::from([ShardId::new(1)])
        );
        assert_eq!(
            service
                .assign_shards(
                    SHARDS,
                    &epochs([(1, 1), (2, 3)]),
                    revision_of(MANAGER, 5),
                    NonZeroUsize::new(3).unwrap()
                )
                .unwrap(),
            ShardDeliveryOutcome::Applied { set_changed: true }
        );
        assert_eq!(service.executor_count(), Some(3));
        assert_eq!(
            service.current_assignment().unwrap().shard_epochs,
            epochs([(1, 1), (2, 3)])
        );
    }

    /// `AssignShards` says "your shards are exactly these". This test checks
    /// shard membership and refuses agents on shards absent from the new set.
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
                NonZeroUsize::new(1).unwrap(),
            )
            .unwrap();

        assert_eq!(
            service.current_assignment().unwrap().shard_id_set(),
            HashSet::from([ShardId::new(1)]),
            "a shard absent from the push must be dropped, not merged"
        );
        assert!(
            service.check_worker(&on_dropped).is_err(),
            "an agent on a dropped shard must fail the membership check"
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
                NonZeroUsize::new(1).unwrap(),
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

    /// `None` means "never expires": the single binary must not fence itself at boot.
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
    #[test_r::always_capture]
    fn clearing_the_assignment_leaves_the_lease_lapsed() {
        let agent = agent_on_shard(0);
        let service = service_holding(&epochs([(0, 3)]), Some(live()));

        service.clear_assignment();

        // Capture isolates the process-global gauge from concurrent tests.
        let metrics = prometheus::gather();
        let assigned = metrics
            .iter()
            .find(|metric| metric.name() == "assigned_shard_count")
            .unwrap();
        assert_eq!(assigned.get_metric()[0].get_gauge().value(), 0.0);
        let assignment = service.current_assignment().unwrap();
        assert!(assignment.is_empty(), "every shard is dropped");
        assert!(
            assignment.expires_at.is_some(),
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
            NonZeroUsize::new(1).unwrap(),
        );
        assert!(!service.is_ready());

        let outcome = service
            .update_lease(
                &epochs([(0, 1)]),
                live(),
                revision_of(MANAGER, 3),
                NonZeroUsize::new(1).unwrap(),
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

        let shorter_expiry = lapsed();
        assert!(matches!(
            service
                .update_lease(
                    &epochs([]),
                    shorter_expiry,
                    revision_of(MANAGER, 2),
                    NonZeroUsize::new(9).unwrap()
                )
                .unwrap(),
            ShardDeliveryOutcome::Stale { .. }
        ));
        let assignment = service.current_assignment().unwrap();
        assert_eq!(assignment.expires_at, Some(shorter_expiry));
        assert_eq!(assignment.executor_count, 1);
        assert_eq!(assignment.shard_epochs, epochs([(0, 1), (1, 1)]));
        assert!(!service.is_ready());
    }
}
