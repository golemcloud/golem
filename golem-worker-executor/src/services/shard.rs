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
use golem_common::model::{AgentId, ShardAssignment, ShardEpoch, ShardId};
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
    /// Full replace (plan D2): hold exactly `shard_epochs`, drop everything
    /// else, and adopt the lease expiry that came with them.
    fn assign_shards(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), WorkerExecutorError>;
    /// Pure set membership. Routing decisions only — never fenced, because a
    /// caller that reads "not mine" routes the call to another executor and a
    /// fenced answer here would route it straight back.
    fn check_worker(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError>;
    /// Set membership **and** a live lease: the self-fence. Admission sites
    /// only — a lapsed lease refuses new work but never interrupts work that
    /// is already running.
    fn check_admission(&self, agent_id: &AgentId) -> Result<(), WorkerExecutorError>;
    fn register(
        &self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
    );
    fn revoke_shards(&self, shard_ids: &HashSet<ShardId>) -> Result<(), WorkerExecutorError>;
    /// A granted lease renewal: the same shard set, at a new expiry.
    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), WorkerExecutorError>;
    /// Drops every shard. Used when the shard manager no longer knows this
    /// executor's lease, so it owns nothing until it re-registers.
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
        if guard.is_none() {
            *guard = Some(ShardAssignment::default())
        }
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
    ) -> Result<(), WorkerExecutorError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    number_of_shards,
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    shard_ids_to_assign = shard_epochs.keys().join(", "),
                    "ShardService.assign_shards"
                );
                shard_assignment.set_shards(number_of_shards, shard_epochs, expires_at);
                let assigned_shard_count = shard_assignment.len();
                record_assigned_shard_count(assigned_shard_count);
                Ok(())
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
    ) {
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
                shard_ids_current = shard_assignment.shard_ids().join(", "),
                shard_ids_to_assign = shard_epochs.keys().join(", "),
                "ShardService.register"
            );
            shard_assignment.set_shards(number_of_shards, shard_epochs, expires_at);
            let assigned_shard_count = shard_assignment.len();
            record_assigned_shard_count(assigned_shard_count);
        })
    }

    fn revoke_shards(&self, shard_ids: &HashSet<ShardId>) -> Result<(), WorkerExecutorError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    shard_ids_to_revoke = shard_ids.iter().join(", "),
                    "ShardService.revoke_shards"
                );
                shard_assignment.revoke_shards(shard_ids);
                let assigned_shard_count = shard_assignment.len();
                record_assigned_shard_count(assigned_shard_count);
                Ok(())
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn update_lease(
        &self,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), WorkerExecutorError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    shard_ids_renewed = shard_epochs.keys().join(", "),
                    "ShardService.update_lease"
                );
                shard_assignment.update_lease(shard_epochs, expires_at);
                let assigned_shard_count = shard_assignment.len();
                record_assigned_shard_count(assigned_shard_count);
                Ok(())
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn clear_assignment(&self) {
        self.with_write_shard_assignment(|shard_assignment| {
            if let Some(shard_assignment) = shard_assignment {
                debug!(
                    shard_ids_current = shard_assignment.shard_ids().join(", "),
                    "ShardService.clear_assignment"
                );
                shard_assignment.clear();
                record_assigned_shard_count(0);
            }
        })
    }

    fn try_get_current_assignment(&self) -> Option<ShardAssignment> {
        self.shard_assignment.read().unwrap().clone()
    }
}

fn sharding_not_ready_error() -> WorkerExecutorError {
    WorkerExecutorError::Unknown {
        details: "Sharding is not ready".to_string(),
    }
}

/// The self-fence. Surfaced as `ShardingNotReady` because the worker service
/// already answers that arm by refreshing its routing table and retrying
/// (`golem-worker-service/src/service/worker/routing_logic.rs:388`), which is
/// exactly what a caller should do when an executor's shard lease has lapsed.
fn shard_lease_expired_error() -> WorkerExecutorError {
    WorkerExecutorError::ShardingNotReady
}
