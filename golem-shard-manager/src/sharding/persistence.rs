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
use super::model::ShardLeaseState;
use anyhow::anyhow;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::serialization::{serialize, try_deserialize};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::RepoError;
use sqlx::Row;

const PERSISTENCE_SVC: &str = "persistence";

#[async_trait]
pub trait RoutingTablePersistence: Send + Sync {
    async fn write(&self, shard_state: &ShardLeaseState) -> Result<(), ShardManagerError>;
    async fn read(&self) -> Result<ShardLeaseState, ShardManagerError>;
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
    async fn write(&self, shard_state: &ShardLeaseState) -> Result<(), ShardManagerError> {
        let encoded = serialize(shard_state).map_err(ShardManagerError::SerializationError)?;

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

    async fn read(&self) -> Result<ShardLeaseState, ShardManagerError> {
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
            decode_shard_state(&bytes)
        } else {
            Ok(ShardLeaseState::new(self.number_of_shards))
        }
    }
}

/// Decodes a persisted state blob and refuses to load one that violates the state invariants.
pub(crate) fn decode_shard_state(bytes: &[u8]) -> Result<ShardLeaseState, ShardManagerError> {
    let shard_state: ShardLeaseState = try_deserialize(bytes)
        .map_err(ShardManagerError::SerializationError)?
        .ok_or_else(|| {
            ShardManagerError::SerializationError(
                "persisted shard lease state is empty or has an unknown serialization version"
                    .to_string(),
            )
        })?;
    shard_state.check_invariants().map_err(|violation| {
        ShardManagerError::SerializationError(format!(
            "persisted shard lease state violates invariants: {violation}"
        ))
    })?;
    Ok(shard_state)
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::sharding::model::{ExecutorAddr, ExecutorId, ShardAssignmentEntry, ShardEpoch};
    use chrono::{DateTime, Utc};
    use golem_common::model::{Pod, ShardId};
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;
    use uuid::Uuid;

    const TTL: Duration = Duration::from_secs(60);

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn pod(last_octet: u8, port: u16) -> Pod {
        Pod {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
            port,
        }
    }

    #[test]
    fn roundtrips() {
        let mut shard_state = ShardLeaseState::new(16);
        shard_state.add_executor(
            ExecutorId(Uuid::from_u128(1)),
            ExecutorAddr::from(pod(1, 9010)),
            Some("worker-executor-0".to_string()),
            t0(),
            TTL,
        );
        shard_state.assign_shard(ExecutorId(Uuid::from_u128(1)), ShardId::new(3));
        shard_state.bump_revision().unwrap();

        let bytes = serialize(&shard_state).unwrap();
        let decoded = decode_shard_state(&bytes).unwrap();
        assert_eq!(decoded, shard_state);
    }

    #[test]
    fn state_violating_invariants_is_rejected() {
        let mut shard_state = ShardLeaseState::new(16);
        shard_state.shard_assignments.insert(
            ShardId::new(0),
            ShardAssignmentEntry {
                executor_id: ExecutorId(Uuid::from_u128(7)),
                epoch: ShardEpoch::initial(),
            },
        );
        let bytes = serialize(&shard_state).unwrap();
        match decode_shard_state(&bytes) {
            Err(ShardManagerError::SerializationError(msg)) => {
                assert!(msg.contains("violates invariants"), "{msg}");
            }
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }

    #[test]
    fn empty_blob_is_rejected() {
        match decode_shard_state(&[]) {
            Err(ShardManagerError::SerializationError(msg)) => {
                assert!(msg.contains("empty"), "{msg}");
            }
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }

    #[test]
    fn truncated_blob_is_rejected() {
        let bytes = [3u8, 0u8];
        match decode_shard_state(&bytes) {
            Err(ShardManagerError::SerializationError(_)) => {}
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }
}
