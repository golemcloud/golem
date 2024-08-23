// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use itertools::Itertools;
use tracing::debug;

use golem_common::model::{ShardAssignment, ShardId, WorkerId};

use crate::error::GolemError;
use crate::metrics::sharding::*;
use crate::model::ShardAssignmentCheck;

/// Service for assigning shards to worker executors
pub trait ShardService {
    fn assign_shards(&self, shard_ids: &HashSet<ShardId>);
    fn check_worker(&self, worker_id: &WorkerId) -> Result<(), GolemError>;
    fn register(&self, number_of_shards: usize, shard_ids: &HashSet<ShardId>);
    fn revoke_shards(&self, shard_ids: &HashSet<ShardId>);
    fn current_assignment(&self) -> ShardAssignment;
}

pub struct ShardServiceDefault {
    shard_assignment: Arc<RwLock<ShardAssignment>>,
}

impl Default for ShardServiceDefault {
    fn default() -> Self {
        Self::new()
    }
}

impl ShardServiceDefault {
    pub fn new() -> Self {
        Self {
            shard_assignment: Arc::new(RwLock::new(ShardAssignment::default())),
        }
    }
}

impl ShardService for ShardServiceDefault {
    fn assign_shards(&self, shard_ids: &HashSet<ShardId>) {
        let mut shard_assignment = self.shard_assignment.write().unwrap();
        debug!(
            shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
            shard_ids_to_assign = shard_ids.iter().join(", "),
            "ShardService.assign_shards"
        );
        shard_assignment.assign_shards(shard_ids);
        let assigned_shard_count = shard_assignment.shard_ids.len();
        record_assigned_shard_count(assigned_shard_count);
    }

    fn check_worker(&self, worker_id: &WorkerId) -> Result<(), GolemError> {
        self.shard_assignment
            .read()
            .unwrap()
            .check_worker(worker_id)
    }

    fn current_assignment(&self) -> ShardAssignment {
        self.shard_assignment.read().unwrap().clone()
    }

    fn register(&self, number_of_shards: usize, shard_ids: &HashSet<ShardId>) {
        let mut shard_assignment = self.shard_assignment.write().unwrap();
        debug!(
            number_of_shards,
            shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
            shard_ids_to_assign = shard_ids.iter().join(", "),
            "ShardService.register"
        );
        shard_assignment.register(number_of_shards, shard_ids);
        let assigned_shard_count = shard_assignment.shard_ids.len();
        record_assigned_shard_count(assigned_shard_count);
    }

    fn revoke_shards(&self, shard_ids: &HashSet<ShardId>) {
        let mut shard_assignment = self.shard_assignment.write().unwrap();
        debug!(
            shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
            shard_ids_to_revoke = shard_ids.iter().join(", "),
            "ShardService.revoke_shards"
        );
        shard_assignment.revoke_shards(shard_ids);
        let assigned_shard_count = shard_assignment.shard_ids.len();
        record_assigned_shard_count(assigned_shard_count);
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct ShardServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for ShardServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl ShardServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
impl ShardService for ShardServiceMock {
    fn assign_shards(&self, shard_ids: &HashSet<ShardId>) {
        tracing::info!("ShardServiceMock::assign_shards {:?}", shard_ids)
    }
    fn check_worker(&self, worker_id: &WorkerId) -> Result<(), GolemError> {
        tracing::info!("ShardServiceMock::check_worker {:?}", worker_id);
        Ok(())
    }
    fn register(&self, number_of_shards: usize, shard_ids: &HashSet<ShardId>) {
        tracing::info!(
            "ShardServiceMock::register {} {:?}",
            number_of_shards,
            shard_ids
        )
    }
    fn revoke_shards(&self, shard_ids: &HashSet<ShardId>) {
        tracing::info!("ShardServiceMock::revoke_shards {:?}", shard_ids)
    }

    fn current_assignment(&self) -> ShardAssignment {
        ShardAssignment::default()
    }
}
