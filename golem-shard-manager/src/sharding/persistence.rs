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

use super::error::ShardManagerError;
use super::model::RoutingTable;
use anyhow::anyhow;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::db::Pool;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::repo::RepoError;
use sqlx::Row;

const PERSISTENCE_SVC: &str = "persistence";

#[async_trait]
pub trait RoutingTablePersistence: Send + Sync {
    async fn write(&self, routing_table: &RoutingTable) -> Result<(), ShardManagerError>;
    async fn read(&self) -> Result<RoutingTable, ShardManagerError>;
}

pub struct DbRoutingTablePersistence<DBP: Pool> {
    pool: DBP,
    number_of_shards: usize,
}

impl<DBP: Pool> DbRoutingTablePersistence<DBP> {
    pub fn new(pool: DBP, number_of_shards: usize) -> Self {
        Self {
            pool,
            number_of_shards,
        }
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl RoutingTablePersistence for DbRoutingTablePersistence<PostgresPool> {
    async fn write(&self, routing_table: &RoutingTable) -> Result<(), ShardManagerError> {
        let encoded = serialize(&routing_table).map_err(ShardManagerError::SerializationError)?;

        self.pool
            .with_rw(PERSISTENCE_SVC, "write")
            .execute(
                sqlx::query(
                    "INSERT INTO shard_manager_state (id, state) VALUES (1, $1) \
                     ON CONFLICT (id) DO UPDATE SET state = EXCLUDED.state",
                )
                .bind(encoded),
            )
            .await
            .map_err(ShardManagerError::RepoError)?;

        Ok(())
    }

    async fn read(&self) -> Result<RoutingTable, ShardManagerError> {
        let row = self
            .pool
            .with_ro(PERSISTENCE_SVC, "read")
            .fetch_optional(sqlx::query(
                "SELECT state FROM shard_manager_state WHERE id = 1",
            ))
            .await
            .map_err(ShardManagerError::RepoError)?;

        if let Some(row) = row {
            let bytes: Vec<u8> = row
                .try_get("state")
                .map_err(|err| RepoError::InternalError(anyhow!(err)))?;
            let routing_table: RoutingTable =
                deserialize(&bytes).map_err(ShardManagerError::SerializationError)?;
            Ok(routing_table)
        } else {
            Ok(RoutingTable::new(self.number_of_shards))
        }
    }
}
