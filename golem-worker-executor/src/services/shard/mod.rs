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
use golem_common::model::{
    AgentId, ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId, ShardLeaseRevision,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::convert::identity;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tracing::debug;

/// Service for assigning shards to worker executors.
pub trait ShardService: Send + Sync {
    /// True once an assignment exists **and** its lease is still live. Gates
    /// the scheduler's poll loop, which admits work without going through
    /// `check_admission`.
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
    /// assignment if none exists yet. The set gates on `revision` like every
    /// delivery; the lease clock always moves. `None` never expires.
    fn register(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<Instant>,
        revision: ShardLeaseRevision,
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
    ) -> Result<ShardDeliveryOutcome, WorkerExecutorError>;
    /// A granted lease renewal: the shard manager's set for this executor, at
    /// a new expiry anchored where the renewal was sent. Normally the set that
    /// was held; when it is not, it is the manager correcting a push this
    /// executor never received, and `set_changed` tells the caller to sweep
    /// and recover agents exactly as it would for a push. The set gates on
    /// `revision`; the lease clock always moves, even when the set is stale.
    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
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
                let outcome = shard_assignment.set_shards(number_of_shards, shard_epochs, revision);
                record_delivery(ShardDelivery::Assign, &outcome, shard_assignment);
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
            let outcome = match expires_at {
                Some(expires_at) => shard_assignment.adopt_grant(
                    Some(number_of_shards),
                    shard_epochs,
                    expires_at,
                    revision,
                ),
                // The never-expires door, which no wire decode can open.
                None => {
                    shard_assignment.expires_at = None;
                    shard_assignment.set_shards(number_of_shards, shard_epochs, revision)
                }
            };
            record_delivery(ShardDelivery::Register, &outcome, shard_assignment);
            outcome
        })
    }

    fn install_unexpiring(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
    ) -> ShardDeliveryOutcome {
        self.with_write_shard_assignment(|shard_assignment| {
            debug!(
                number_of_shards,
                shard_ids_to_assign = shard_epochs.keys().join(", "),
                "ShardService.install_unexpiring"
            );
            let set_changed = shard_assignment
                .as_ref()
                .is_none_or(|current| current.shard_epochs != *shard_epochs);
            let installed = shard_assignment.insert(ShardAssignment {
                number_of_shards,
                shard_epochs: shard_epochs.clone(),
                expires_at: None,
                revision: None,
            });
            let outcome = ShardDeliveryOutcome::Applied { set_changed };
            record_delivery(ShardDelivery::Register, &outcome, installed);
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
                record_delivery(ShardDelivery::Revoke, &outcome, shard_assignment);
                Ok(outcome)
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
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
                let outcome =
                    shard_assignment.adopt_grant(None, shard_epochs, expires_at, revision);
                record_delivery(ShardDelivery::Renewal, &outcome, shard_assignment);
                Ok(outcome)
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn clear_assignment(&self) {
        let now = Instant::now();
        self.with_write_shard_assignment(|shard_assignment| {
            if let Some(shard_assignment) = shard_assignment {
                debug!(
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    "ShardService.clear_assignment"
                );
                // Lapsed as of now, not "never expires".
                shard_assignment.clear(now);
                record_assigned_shard_count(0);
                record_shard_lease_expiry(now);
            }
        })
    }

    fn try_get_current_assignment(&self) -> Option<ShardAssignment> {
        self.shard_assignment.read().unwrap().clone()
    }
}

/// Records what every delivery updates: the resulting shard count, the lease expiry when the
/// lease expires, and, when the delivery was dropped - older than the last applied, or pushed by a
/// manager this executor does not follow - the drop itself.
///
/// One function rather than a line per delivery, so a delivery added later cannot record the
/// count and silently forget the staleness or the expiry.
fn record_delivery(
    delivery: ShardDelivery,
    outcome: &ShardDeliveryOutcome,
    shard_assignment: &ShardAssignment,
) {
    record_assigned_shard_count(shard_assignment.len());
    if let Some(expires_at) = shard_assignment.expires_at {
        record_shard_lease_expiry(expires_at);
    }
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
mod tests;
