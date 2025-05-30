// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::collections::HashSet;
use std::convert::identity;
use std::sync::{Arc, RwLock};

use itertools::Itertools;
use tracing::debug;

use golem_common::model::{ShardAssignment, ShardId, WorkerId};

use crate::error::GolemError;
use crate::metrics::sharding::*;
use crate::model::ShardAssignmentCheck;

/// Service for assigning shards to worker executors
pub trait ShardService: Send + Sync {
    fn is_ready(&self) -> bool;
    fn assign_shards(&self, shard_ids: &HashSet<ShardId>) -> Result<(), GolemError>;
    fn check_worker(&self, worker_id: &WorkerId) -> Result<(), GolemError>;
    fn register(&self, number_of_shards: usize, shard_ids: &HashSet<ShardId>);
    fn revoke_shards(&self, shard_ids: &HashSet<ShardId>) -> Result<(), GolemError>;
    fn current_assignment(&self) -> Result<ShardAssignment, GolemError>;
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

    pub fn with_read_shard_assignment<F, O>(&self, f: F) -> Result<O, GolemError>
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
        self.shard_assignment.read().unwrap().is_some()
    }

    fn assign_shards(&self, shard_ids: &HashSet<ShardId>) -> Result<(), GolemError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
                    shard_ids_to_assign = shard_ids.iter().join(", "),
                    "ShardService.assign_shards"
                );
                shard_assignment.assign_shards(shard_ids);
                let assigned_shard_count = shard_assignment.shard_ids.len();
                record_assigned_shard_count(assigned_shard_count);
                Ok(())
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn check_worker(&self, worker_id: &WorkerId) -> Result<(), GolemError> {
        self.with_read_shard_assignment(|shard_assignment: &ShardAssignment| {
            shard_assignment.check_worker(worker_id)
        })
        .and_then(identity)
    }

    fn current_assignment(&self) -> Result<ShardAssignment, GolemError> {
        self.with_read_shard_assignment(|shard_assignment| shard_assignment.clone())
    }

    fn register(&self, number_of_shards: usize, shard_ids: &HashSet<ShardId>) {
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
                shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
                shard_ids_to_assign = shard_ids.iter().join(", "),
                "ShardService.register"
            );
            shard_assignment.register(number_of_shards, shard_ids);
            let assigned_shard_count = shard_assignment.shard_ids.len();
            record_assigned_shard_count(assigned_shard_count);
        })
    }

    fn revoke_shards(&self, shard_ids: &HashSet<ShardId>) -> Result<(), GolemError> {
        self.with_write_shard_assignment(|shard_assignment| match shard_assignment {
            Some(shard_assignment) => {
                debug!(
                    shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
                    shard_ids_to_revoke = shard_ids.iter().join(", "),
                    "ShardService.revoke_shards"
                );
                shard_assignment.revoke_shards(shard_ids);
                let assigned_shard_count = shard_assignment.shard_ids.len();
                record_assigned_shard_count(assigned_shard_count);
                Ok(())
            }
            None => Err(sharding_not_ready_error()),
        })
    }

    fn try_get_current_assignment(&self) -> Option<ShardAssignment> {
        self.shard_assignment.read().unwrap().clone()
    }
}

fn sharding_not_ready_error() -> GolemError {
    GolemError::Unknown {
        details: "Sharding is not ready".to_string(),
    }
}
