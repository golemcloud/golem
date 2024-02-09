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

use async_trait::async_trait;
use bytes::Bytes;
use golem_common::redis::RedisPool;

use crate::error::ShardManagerError;
use crate::model::{Rebalance, RoutingTable, ShardManagerState};

#[async_trait]
pub trait PersistenceService {
    async fn write(
        &self,
        routing_table: &RoutingTable,
        rebalance: &Rebalance,
    ) -> Result<(), ShardManagerError>;
    async fn read(&self) -> Result<(RoutingTable, Rebalance), ShardManagerError>;
}

pub struct PersistenceServiceDefault {
    pool: RedisPool,
    number_of_shards: usize,
}

#[async_trait]
impl PersistenceService for PersistenceServiceDefault {
    async fn write(
        &self,
        routing_table: &RoutingTable,
        rebalance: &Rebalance,
    ) -> Result<(), ShardManagerError> {
        let shard_manager_state = ShardManagerState::new(routing_table, rebalance);
        let key = "shard:shard_manager_state";
        let value = self
            .pool
            .serialize(&shard_manager_state)
            .map_err(|e| ShardManagerError::unknown(e.to_string()))?;

        self.pool
            .with("persistence", "write")
            .set(key, value, None, None, false)
            .await
            .map_err(|e| ShardManagerError::unknown(e.to_string()))
    }

    async fn read(&self) -> Result<(RoutingTable, Rebalance), ShardManagerError> {
        let key = "shard:shard_manager_state";

        let value: Option<Bytes> = self
            .pool
            .with("persistence", "read")
            .get(key)
            .await
            .map_err(|e| ShardManagerError::unknown(e.to_string()))?;

        match value {
            Some(value) => {
                let shard_manager_state: ShardManagerState = self
                    .pool
                    .deserialize(&value)
                    .map_err(|e| ShardManagerError::unknown(e.to_string()))?;
                Ok((
                    shard_manager_state.get_routing_table(),
                    shard_manager_state.get_rebalance(),
                ))
            }
            None => Ok((RoutingTable::new(self.number_of_shards), Rebalance::new())),
        }
    }
}

impl PersistenceServiceDefault {
    pub fn new(pool: &RedisPool, number_of_shards: &usize) -> Self {
        Self {
            pool: pool.clone(),
            number_of_shards: *number_of_shards,
        }
    }
}
