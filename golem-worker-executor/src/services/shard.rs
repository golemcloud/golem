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
use chrono::{DateTime, Utc};
use golem_common::model::{
    AgentId, ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId, ShardLeaseRevision,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::convert::identity;
use std::sync::{Arc, RwLock};
use tracing::debug;

/// Service for assigning shards to worker executors
pub trait ShardService: Send + Sync {
    /// True once an assignment exists **and** its lease is still live. Gates
    /// the scheduler's poll loop, which admits work without going through
    /// `check_admission`.
    fn is_ready(&self) -> bool;
    /// Full replace from an `AssignShards` push: hold exactly `shard_epochs`,
    /// drop everything else, and adopt the lease expiry that came with them.
    /// Ignored whole if `revision` is older than the last delivery applied,
    /// so a push and a renewal response that cross on the network cannot
    /// leave the older set in place.
    fn assign_shards(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError>;
    /// Pure set membership. Routing decisions only — never fenced, because a
    /// caller that reads "not mine" routes the call to another executor and a
    /// fenced answer here would route it straight back.
    fn check_worker(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError>;
    /// Set membership **and** a live lease: the self-fence. Admission sites
    /// only — a lapsed lease refuses new work but never interrupts work that
    /// is already running.
    fn check_admission(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError>;
    /// Installs the first assignment, from a registration's grant. Creates the
    /// assignment if none exists yet; gates on `revision` like the other two.
    fn register(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome;
    /// A revoke: the shards named are dropped, everything else stays. The one
    /// delta among the deliveries, gated on `revision` like the rest; the
    /// shard manager only ever sends it for shards its store has already moved
    /// away, at the revision that move was stored under.
    fn revoke_shards(
        &self,
        shard_ids: &HashSet<ShardId>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError>;
    /// A granted lease renewal: the shard manager's set for this executor, at
    /// a new expiry. Normally the set that was claimed; when it is not, it is
    /// the manager correcting a push this executor never received, and
    /// `set_changed` tells the caller to sweep and recover agents exactly as it
    /// would for a push. Gates on `revision` like the other two.
    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError>;
    /// Drops every shard and lapses the lease. Used when the shard
    /// manager no longer knows this executor's lease: it owns nothing and is
    /// not ready until a re-registration installs a fresh grant.
    fn clear_assignment(&self);
    fn current_assignment(&self) -> Result<ShardAssignment, WorkerExecutorError>;
    fn try_get_current_assignment(&self) -> Option<ShardAssignment>;
}

pub struct ShardServiceDefault {
    shard_assignment: Arc<RwLock<Option<ShardAssignment>>>,
}

impl Default for ShardServiceDefault {
    fn default() -> Self {
        Self::new()
    }
}

impl ShardServiceDefault {
    pub fn new() -> Self {
        Self {
            shard_assignment: Arc::new(RwLock::new(None)),
        }
    }

    pub fn with_read_shard_assignment<F, O>(&self, f: F) -> Result<O, WorkerExecutorError>
    where
        F: Fn(&ShardAssignment) -> O,
    {
        let guard = self.shard_assignment.read().unwrap();
        match guard.as_ref() {
            Some(shard_assignment) => Ok(f(shard_assignment)),
            None => Err(sharding_not_ready_error()),
        }
    }

    pub fn with_write_shard_assignment<F, O>(&self, f: F) -> O
    where
        F: Fn(&mut Option<ShardAssignment>) -> O,
    {
        let mut guard = self.shard_assignment.write().unwrap();
        f(&mut guard)
    }
}

impl ShardService for ShardServiceDefault {
    fn is_ready(&self) -> bool {
        let now = Utc::now();
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
        expires_at: Option<DateTime<Utc>>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    number_of_shards,
                    %revision,
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    shard_ids_to_assign = shard_epochs.keys().join(", "),
                    "ShardService.assign_shards"
                );
                let outcome = shard_assignment.set_shards(
                    number_of_shards,
                    shard_epochs,
                    expires_at,
                    revision,
                );
                record_delivery(ShardDelivery::Assign, &outcome, shard_assignment.len());
                Ok(outcome)
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn check_worker(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError> {
        self.with_read_shard_assignment(|shard_assignment: &ShardAssignment| {
            shard_assignment.check_worker(agent_id)
        })
        .and_then(identity)
    }

    fn check_admission(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError> {
        let now = Utc::now();
        self.with_read_shard_assignment(|shard_assignment: &ShardAssignment| {
            if shard_assignment.lease_is_live(now) {
                shard_assignment.check_worker(agent_id)
            } else {
                Err(shard_lease_expired_error())
            }
        })
        .and_then(identity)
    }

    fn current_assignment(&self) -> Result<ShardAssignment, WorkerExecutorError> {
        self.with_read_shard_assignment(|shard_assignment| shard_assignment.clone())
    }

    fn register(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        self.with_write_shard_assignment(|shard_assignment| {
            let shard_assignment = match shard_assignment {
                Some(shard_assignment) => shard_assignment,
                None => {
                    *shard_assignment = Some(ShardAssignment::default());
                    shard_assignment.as_mut().unwrap()
                }
            };
            debug!(
                number_of_shards,
                %revision,
                shard_ids_current = shard_assignment.shard_ids().join(", "),
                shard_ids_to_assign = shard_epochs.keys().join(", "),
                "ShardService.register"
            );
            let outcome =
                shard_assignment.set_shards(number_of_shards, shard_epochs, expires_at, revision);
            record_delivery(ShardDelivery::Register, &outcome, shard_assignment.len());
            outcome
        })
    }

    fn revoke_shards(
        &self,
        shard_ids: &HashSet<ShardId>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    %revision,
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    shard_ids_to_revoke = shard_ids.iter().join(", "),
                    "ShardService.revoke_shards"
                );
                let outcome = shard_assignment.revoke_shards(shard_ids, revision);
                record_delivery(ShardDelivery::Revoke, &outcome, shard_assignment.len());
                Ok(outcome)
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
        revision: ShardLeaseRevision,
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    %revision,
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    shard_ids_renewed = shard_epochs.keys().join(", "),
                    "ShardService.update_lease"
                );
                let outcome = shard_assignment.update_lease(shard_epochs, expires_at, revision);
                record_delivery(ShardDelivery::Renewal, &outcome, shard_assignment.len());
                Ok(outcome)
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn clear_assignment(&self) {
        let now = Utc::now();
        self.with_write_shard_assignment(|shard_assignment| {
            if let Some(shard_assignment) = shard_assignment {
                debug!(
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    "ShardService.clear_assignment"
                );
                // Lapsed as of now, not "never expires".
                shard_assignment.clear(now);
                record_assigned_shard_count(0);
            }
        })
    }

    fn try_get_current_assignment(&self) -> Option<ShardAssignment> {
        self.shard_assignment.read().unwrap().clone()
    }
}

/// Records what every delivery updates: the resulting shard count, and, when the delivery was
/// dropped for being older than the last applied, the staleness itself.
///
/// One function rather than a line per delivery, so a delivery added later cannot record the
/// count and silently forget the staleness.
fn record_delivery(delivery: ShardDelivery, outcome: &ShardDeliveryOutcome, assigned: usize) {
    record_assigned_shard_count(assigned);
    if matches!(outcome, ShardDeliveryOutcome::Stale { .. }) {
        record_stale_shard_delivery(delivery);
    }
}

/// No assignment is installed yet, so this executor cannot answer for any shard.
///
/// The same variant as [`shard_lease_expired_error`], because a caller wants the same thing of
/// both: refresh the routing table and retry elsewhere. They stay separate names so the two
/// states read differently at the call sites, which is the only place they differ.
fn sharding_not_ready_error() -> WorkerExecutorError {
    WorkerExecutorError::ShardingNotReady
}

/// The self-fence. Surfaced as `ShardingNotReady` because the worker service
/// already answers that arm by refreshing its routing table and retrying
/// (the `ShardingNotReady` arm of the worker service's `routing_logic`), which is
/// exactly what a caller should do when an executor's shard lease has lapsed.
fn shard_lease_expired_error() -> WorkerExecutorError {
    WorkerExecutorError::ShardingNotReady
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use golem_common::model::component::ComponentId;
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

    /// An agent id that routes to `shard`, found by search because
    /// `ShardId::from_agent_id` is a hash.
    fn agent_on_shard(shard: i64) -> AgentId {
        let component_id = ComponentId(Uuid::nil());
        for candidate in 0..10_000 {
            let agent_id = AgentId {
                component_id,
                agent_id: format!("agent-{candidate}"),
            };
            if ShardId::from_agent_id(&agent_id, SHARDS) == ShardId::new(shard) {
                return agent_id;
            }
        }
        panic!("no agent id in the search space routes to shard {shard}");
    }

    fn service_holding(
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
    ) -> ShardServiceDefault {
        let service = ShardServiceDefault::new();
        service.register(
            SHARDS,
            shard_epochs,
            expires_at,
            ShardLeaseRevision::default(),
        );
        service
    }

    fn lapsed() -> Option<DateTime<Utc>> {
        Some(Utc::now() - ChronoDuration::seconds(1))
    }

    fn live() -> Option<DateTime<Utc>> {
        Some(Utc::now() + ChronoDuration::seconds(60))
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
            service.assign_shards(SHARDS, &epochs([(0, 1)]), live(), ShardLeaseRevision(1)),
            service.update_lease(&epochs([(0, 1)]), live(), ShardLeaseRevision(1)),
            service.revoke_shards(&HashSet::from([ShardId::new(0)]), ShardLeaseRevision(1)),
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
            live(),
            ShardLeaseRevision(5),
        );

        let before = stale_shard_delivery_count(ShardDelivery::Renewal);
        let outcome = service
            .update_lease(&epochs([(0, 1)]), live(), ShardLeaseRevision(3))
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
            live(),
            ShardLeaseRevision(5),
        );

        let outcome = service
            .revoke_shards(&HashSet::from([ShardId::new(0)]), ShardLeaseRevision(3))
            .expect("a registered executor can be revoked from");

        assert_eq!(
            outcome,
            ShardDeliveryOutcome::Stale {
                delivered: ShardLeaseRevision(3),
                applied: ShardLeaseRevision(5),
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
            .revoke_shards(&HashSet::from([ShardId::new(0)]), ShardLeaseRevision(5))
            .expect("a registered executor can be revoked from");
        assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
        assert_eq!(
            service.current_assignment().unwrap().shard_id_set(),
            HashSet::from([ShardId::new(1)])
        );
    }

    /// `AssignShards` says "your shards are exactly these". Anything
    /// absent is dropped, and the sweep in `assign_shards_internal` restarts
    /// exactly the agents the new set rejects.
    #[test]
    fn a_full_replace_push_drops_unlisted_shards_and_their_agents() {
        let service = service_holding(&epochs([(0, 1), (1, 1)]), None);
        let on_dropped = agent_on_shard(0);
        let on_kept = agent_on_shard(1);
        assert!(service.check_worker(&on_dropped).is_ok());
        assert!(service.check_worker(&on_kept).is_ok());

        service
            .assign_shards(SHARDS, &epochs([(1, 1)]), None, ShardLeaseRevision(1))
            .unwrap();

        assert_eq!(
            service.current_assignment().unwrap().shard_id_set(),
            HashSet::from([ShardId::new(1)]),
            "a shard absent from the push must be dropped, not merged"
        );
        assert!(
            service.check_worker(&on_dropped).is_err(),
            "an agent on a dropped shard is what the restart sweep picks up"
        );
        assert!(service.check_worker(&on_kept).is_ok());
    }

    /// The self-fence. It surfaces as `ShardingNotReady`, which the worker
    /// service answers by refreshing its routing table and retrying.
    #[test]
    fn admission_is_refused_once_the_lease_has_lapsed() {
        let agent = agent_on_shard(0);
        let service = service_holding(&epochs([(0, 3)]), live());
        assert!(service.is_ready());
        assert!(service.check_admission(&agent).is_ok());

        service
            .update_lease(&epochs([(0, 3)]), lapsed(), ShardLeaseRevision(1))
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
        let service = service_holding(&epochs([(0, 3)]), lapsed());

        assert!(
            service.check_worker(&agent).is_ok(),
            "routing must still say 'mine' for a shard this executor holds"
        );
        assert!(
            service.check_admission(&agent).is_err(),
            "while admission is fenced"
        );
    }

    /// `None` means "never expires": the single binary and the debugging
    /// service must not fence themselves at boot.
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
        let service = service_holding(&epochs([(0, 3)]), live());

        service.clear_assignment();

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
}
