// Copyright 2024-2025 Golem Cloud
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

use std::path::{Path, PathBuf};

use crate::error::ShardManagerError;
use crate::model::{RoutingTable, ShardManagerState};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::redis::RedisPool;
use golem_common::serialization::{deserialize, serialize};

#[async_trait]
pub trait RoutingTablePersistence {
    async fn write(&self, routing_table: &RoutingTable) -> Result<(), ShardManagerError>;
    async fn read(&self) -> Result<RoutingTable, ShardManagerError>;
}

pub struct RoutingTableRedisPersistence {
    pool: RedisPool,
    number_of_shards: usize,
}

#[async_trait]
impl RoutingTablePersistence for RoutingTableRedisPersistence {
    async fn write(&self, routing_table: &RoutingTable) -> Result<(), ShardManagerError> {
        let shard_manager_state = ShardManagerState::new(routing_table);
        let key = "shard:shard_manager_state";
        let value = self
            .pool
            .serialize(&shard_manager_state)
            .map_err(ShardManagerError::SerializationError)?;

        self.pool
            .with("persistence", "write")
            .set(key, value, None, None, false)
            .await
            .map_err(ShardManagerError::RedisError)
    }

    async fn read(&self) -> Result<RoutingTable, ShardManagerError> {
        let key = "shard:shard_manager_state";

        let value: Option<Bytes> = self
            .pool
            .with("persistence", "read")
            .get(key)
            .await
            .map_err(ShardManagerError::RedisError)?;

        match value {
            Some(value) => {
                let shard_manager_state: ShardManagerState = self
                    .pool
                    .deserialize(&value)
                    .map_err(ShardManagerError::SerializationError)?;
                Ok(shard_manager_state.get_routing_table())
            }
            None => Ok(RoutingTable::new(self.number_of_shards)),
        }
    }
}

impl RoutingTableRedisPersistence {
    pub fn new(pool: &RedisPool, number_of_shards: usize) -> Self {
        Self {
            pool: pool.clone(),
            number_of_shards,
        }
    }
}

pub struct RoutingTableFileSystemPersistence {
    path: PathBuf,
    number_of_shards: usize,
}

impl RoutingTableFileSystemPersistence {
    pub async fn new(path: &Path, number_of_shards: usize) -> std::io::Result<Self> {
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        Ok(Self {
            path: path.to_path_buf(),
            number_of_shards,
        })
    }
}

#[async_trait]
impl RoutingTablePersistence for RoutingTableFileSystemPersistence {
    async fn write(&self, routing_table: &RoutingTable) -> Result<(), ShardManagerError> {
        let shard_manager_state = ShardManagerState::new(routing_table);
        let encoded =
            serialize(&shard_manager_state).map_err(ShardManagerError::SerializationError)?;
        tokio::fs::write(&self.path, encoded).await?;
        Ok(())
    }

    async fn read(&self) -> Result<RoutingTable, ShardManagerError> {
        if tokio::fs::try_exists(&self.path).await? {
            let bytes = tokio::fs::read(&self.path).await?;
            let shard_manager_state: ShardManagerState =
                deserialize(&bytes).map_err(ShardManagerError::SerializationError)?;
            Ok(shard_manager_state.get_routing_table())
        } else {
            Ok(RoutingTable::new(self.number_of_shards))
        }
    }
}
