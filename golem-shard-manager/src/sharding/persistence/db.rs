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

use super::{
    ExternalRevision, NO_REVISION, RoutingTablePersistence, check_prev_revision,
    check_state_for_write, check_stored_revision, decode_shard_state,
};
use crate::sharding::error::ShardManagerError;
use crate::sharding::model::ShardLeaseState;
use anyhow::anyhow;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::serialization::serialize;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{Blob, RepoError, SqlDateTime};
use indoc::indoc;
use sqlx::{QueryBuilder, Row};
use std::net::IpAddr;
use uuid::Uuid;

const PERSISTENCE_SVC: &str = "persistence";

/// Rows per multi-row `INSERT` into the mirror tables. Six binds per lease row, against SQLite's
/// 32766 bind-parameter limit - the smaller of the two dialects'.
const MIRROR_INSERT_CHUNK_SIZE: usize = 1000;
const _: () = assert!(MIRROR_INSERT_CHUNK_SIZE * 6 <= 32766);

/// Creates the single state row. Succeeds only while the row is absent: `DO NOTHING` turns the
/// primary-key clash into "0 rows affected" instead of a unique-violation error, identically on
/// Postgres and SQLite.
const INSERT_STATE_SQL: &str = indoc! { r#"
    INSERT INTO shard_manager_state (id, state, revision)
    VALUES (1, $1, $2)
    ON CONFLICT (id) DO NOTHING
"#};

/// Replaces the single state row. Succeeds only while the row is present and still carries `$3`;
/// an absent row matches nothing and yields 0 rows, which is the same answer etcd gives for a
/// compare against a deleted key.
const UPDATE_STATE_SQL: &str = indoc! { r#"
    UPDATE shard_manager_state
    SET state = $1, revision = $2
    WHERE id = 1 AND revision = $3
"#};

const SELECT_STATE_SQL: &str = indoc! { r#"
    SELECT state, revision
    FROM shard_manager_state
    WHERE id = 1
"#};

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
    async fn read(&self) -> Result<(ShardLeaseState, ExternalRevision), ShardManagerError> {
        // NOTE: `with_ro` and `with_rw` address the same Postgres pool. If a read replica is
        // ever put behind `with_ro`, a stale read here yields a stale revision and every
        // subsequent compare-and-swap fails - a livelock, not corruption - and this read has to
        // move to `with_rw` at that point.
        let row = self
            .pool
            .with_ro(PERSISTENCE_SVC, "read")
            .fetch_optional(sqlx::query(SELECT_STATE_SQL))
            .await
            .map_err(ShardManagerError::RepoError)?;

        let Some(row) = row else {
            return Ok((ShardLeaseState::new(self.number_of_shards), NO_REVISION));
        };

        let bytes: Vec<u8> = row
            .try_get("state")
            .map_err(|err| RepoError::InternalError(anyhow!(err)))?;
        let revision: ExternalRevision = row
            .try_get("revision")
            .map_err(|err| RepoError::InternalError(anyhow!(err)))?;

        if revision < 1 {
            return Err(ShardManagerError::Internal(format!(
                "persisted shard lease state carries revision {revision}, which is reserved for \
                 absent state"
            )));
        }

        Ok((decode_shard_state(&bytes)?, revision))
    }

    async fn write(
        &self,
        shard_state: &ShardLeaseState,
        prev_revision: ExternalRevision,
    ) -> Result<ExternalRevision, ShardManagerError> {
        check_prev_revision(prev_revision)?;
        check_state_for_write(shard_state)?;
        let next_revision = prev_revision.checked_add(1).ok_or_else(|| {
            ShardManagerError::Internal("shard state storage revision overflow".to_string())
        })?;
        let encoded = serialize(shard_state).map_err(ShardManagerError::SerializationError)?;
        let leases = lease_rows(shard_state);
        let assignments = assignment_rows(shard_state)?;

        // One transaction, so the mirror tables follow the compare-and-swap or, if it is rejected,
        // are left untouched with it.
        let revision = self
            .pool
            .with_tx_err(PERSISTENCE_SVC, "write", |tx| {
                async move {
                    Self::compare_and_swap_state_in_tx(tx, encoded, prev_revision, next_revision)
                        .await?;
                    Self::replace_mirror_rows_in_tx(tx, &leases, &assignments).await?;
                    Ok::<ExternalRevision, ShardManagerError>(next_revision)
                }
                .boxed()
            })
            .await?;

        check_stored_revision(revision, prev_revision)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbRoutingTablePersistence<PostgresPool> {
    async fn compare_and_swap_state_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        encoded: Vec<u8>,
        prev_revision: ExternalRevision,
        next_revision: ExternalRevision,
    ) -> Result<(), ShardManagerError> {
        // `prev_revision == NO_REVISION` asserts the row does not exist, which is an
        // insert-if-absent, not an update. A single `INSERT ... ON CONFLICT DO UPDATE ... WHERE
        // revision = $prev` cannot express it: its INSERT branch is unguarded, so it would happily
        // resurrect a row that was deleted underneath a writer holding `prev > NO_REVISION`, while
        // etcd refuses the same write. Two statements keep both backends honest.
        let query = if prev_revision == NO_REVISION {
            sqlx::query(INSERT_STATE_SQL)
                .bind(encoded) // $1
                .bind(next_revision) // $2
        } else {
            sqlx::query(UPDATE_STATE_SQL)
                .bind(encoded) // $1
                .bind(next_revision) // $2
                .bind(prev_revision) // $3
        };

        let result = tx.execute(query).await?;
        if result.rows_affected() == 0 {
            return Err(ShardManagerError::ConcurrentModification);
        }
        Ok(())
    }

    /// Rewrites the mirror tables from scratch. They are a projection of the blob, so replacing
    /// them wholesale is both simpler and safer than diffing: nothing can be left behind.
    async fn replace_mirror_rows_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        leases: &[ExecutorLeaseRow],
        assignments: &[ShardAssignmentRow],
    ) -> Result<(), ShardManagerError> {
        // Assignments reference leases, so they go first on delete and last on insert.
        tx.execute(sqlx::query("DELETE FROM shard_assignments"))
            .await?;
        tx.execute(sqlx::query("DELETE FROM executor_leases"))
            .await?;

        for chunk in leases.chunks(MIRROR_INSERT_CHUNK_SIZE) {
            let mut query = QueryBuilder::<<PostgresPool as Pool>::Db>::new(
                "INSERT INTO executor_leases \
                 (executor_id, ip, port, granted_at, expires_at, pod_name) ",
            );
            query.push_values(chunk, |mut row, lease| {
                row.push_bind(lease.executor_id)
                    .push_bind(lease.ip.clone())
                    .push_bind(lease.port)
                    .push_bind(lease.granted_at.clone())
                    .push_bind(lease.expires_at.clone())
                    .push_bind(lease.pod_name.clone());
            });
            tx.execute(query.build()).await?;
        }

        for chunk in assignments.chunks(MIRROR_INSERT_CHUNK_SIZE) {
            let mut query = QueryBuilder::<<PostgresPool as Pool>::Db>::new(
                "INSERT INTO shard_assignments (shard_id, executor_id, epoch) ",
            );
            query.push_values(chunk, |mut row, assignment| {
                row.push_bind(assignment.shard_id)
                    .push_bind(assignment.executor_id)
                    .push_bind(assignment.epoch);
            });
            tx.execute(query.build()).await?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ExecutorLeaseRow {
    executor_id: Uuid,
    ip: Blob<IpAddr>,
    port: i32,
    granted_at: SqlDateTime,
    expires_at: SqlDateTime,
    pod_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShardAssignmentRow {
    shard_id: i32,
    executor_id: Uuid,
    epoch: i64,
}

fn lease_rows(shard_state: &ShardLeaseState) -> Vec<ExecutorLeaseRow> {
    shard_state
        .executor_leases
        .iter()
        .map(|(executor_id, lease)| ExecutorLeaseRow {
            executor_id: executor_id.0,
            ip: Blob::new(lease.addr.ip),
            port: i32::from(lease.addr.port),
            granted_at: SqlDateTime::new(lease.granted_at),
            expires_at: SqlDateTime::new(lease.expires_at),
            pod_name: lease.pod_name.clone(),
        })
        .collect()
}

fn assignment_rows(
    shard_state: &ShardLeaseState,
) -> Result<Vec<ShardAssignmentRow>, ShardManagerError> {
    shard_state
        .shard_assignments
        .iter()
        .map(|(shard_id, entry)| {
            Ok(ShardAssignmentRow {
                shard_id: i32::try_from(shard_id.value()).map_err(|_| {
                    ShardManagerError::Internal(format!(
                        "shard id {} does not fit the shard_assignments.shard_id column",
                        shard_id.value()
                    ))
                })?,
                executor_id: entry.executor_id.0,
                epoch: i64::try_from(entry.epoch.0).map_err(|_| {
                    ShardManagerError::Internal(format!(
                        "shard epoch {} does not fit the shard_assignments.epoch column",
                        entry.epoch.0
                    ))
                })?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::sharding::model::{ExecutorAddr, ExecutorId, ShardAssignmentEntry, ShardEpoch};
    use chrono::{DateTime, Utc};
    use golem_common::model::ShardId;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    const TTL: Duration = Duration::from_secs(60);

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn addr(last_octet: u8, port: u16) -> ExecutorAddr {
        ExecutorAddr {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
            port,
        }
    }

    #[test]
    fn mirror_rows_project_the_state() {
        let mut shard_state = ShardLeaseState::new(8);
        shard_state.add_executor(
            ExecutorId(Uuid::from_u128(1)),
            addr(1, 9010),
            Some("worker-executor-0".to_string()),
            t0(),
            TTL,
        );
        shard_state.add_executor(
            ExecutorId(Uuid::from_u128(2)),
            addr(2, 9011),
            None,
            t0(),
            TTL,
        );
        shard_state.assign_shard(ExecutorId(Uuid::from_u128(1)), ShardId::new(0));
        shard_state.assign_shard(ExecutorId(Uuid::from_u128(2)), ShardId::new(1));
        // moving a shard bumps its epoch
        shard_state.assign_shard(ExecutorId(Uuid::from_u128(2)), ShardId::new(0));

        let leases = lease_rows(&shard_state);
        assert_eq!(leases.len(), 2);
        assert_eq!(leases[0].executor_id, Uuid::from_u128(1));
        assert_eq!(
            *leases[0].ip.value(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
        assert_eq!(leases[0].port, 9010);
        assert_eq!(leases[0].granted_at, SqlDateTime::new(t0()));
        assert_eq!(
            leases[0].expires_at,
            SqlDateTime::new(t0() + chrono::Duration::from_std(TTL).unwrap())
        );
        assert_eq!(leases[0].pod_name.as_deref(), Some("worker-executor-0"));
        assert_eq!(leases[1].pod_name, None);

        let assignments = assignment_rows(&shard_state).unwrap();
        assert_eq!(
            assignments,
            vec![
                ShardAssignmentRow {
                    shard_id: 0,
                    executor_id: Uuid::from_u128(2),
                    epoch: 1,
                },
                ShardAssignmentRow {
                    shard_id: 1,
                    executor_id: Uuid::from_u128(2),
                    epoch: 0,
                },
            ]
        );
        assert_eq!(
            shard_state.epoch_for_shard(ShardId::new(0)),
            Some(ShardEpoch(1))
        );
    }

    #[test]
    fn mirror_rows_refuse_values_the_columns_cannot_hold() {
        let executor_id = ExecutorId(Uuid::from_u128(1));

        let mut shard_state = ShardLeaseState::new(8);
        shard_state.add_executor(executor_id, addr(1, 9010), None, t0(), TTL);
        shard_state.shard_assignments.insert(
            ShardId::new(0),
            ShardAssignmentEntry {
                executor_id,
                epoch: ShardEpoch(u64::MAX),
            },
        );
        match assignment_rows(&shard_state) {
            Err(ShardManagerError::Internal(msg)) => assert!(msg.contains("epoch"), "{msg}"),
            other => panic!("expected Internal, got {other:?}"),
        }

        let mut shard_state = ShardLeaseState::new(8);
        shard_state.add_executor(executor_id, addr(1, 9010), None, t0(), TTL);
        shard_state.shard_assignments.insert(
            ShardId::new(i64::from(i32::MAX) + 1),
            ShardAssignmentEntry {
                executor_id,
                epoch: ShardEpoch::initial(),
            },
        );
        match assignment_rows(&shard_state) {
            Err(ShardManagerError::Internal(msg)) => assert!(msg.contains("shard id"), "{msg}"),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn mirror_rows_of_an_empty_state_are_empty() {
        let shard_state = ShardLeaseState::new(8);
        assert!(lease_rows(&shard_state).is_empty());
        assert!(assignment_rows(&shard_state).unwrap().is_empty());
    }
}
