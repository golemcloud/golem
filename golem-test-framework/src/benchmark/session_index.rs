//! Fail-closed controls for a derived index in the spawned PostgreSQL benchmark backend.

use crate::components::rdb::{DbInfo, PostgresInfo};
use crate::config::benchmark::TestMode;
use crate::config::{BenchmarkTestDependencies, TestDependencies};
use anyhow::{Context, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::{AgentStatusRecord, IdempotencyKey, OwnedAgentId};
use golem_common::serialization::try_deserialize;
use golem_service_base::storage::blob::{BlobStorageNamespace, ExistsResult};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

type IndexRows = Vec<(String, Vec<u8>)>;

/// Read-only observation of exact fields, usable after a live producer's completion barrier.
/// Status is an asynchronously persisted baseline; callers must check `oplog_idx` before
/// interpreting absence from its bounded recent-session set as eviction.
pub struct LiveSessionIndexInspection {
    pub status: Option<AgentStatusRecord>,
    pub coverage_present: bool,
    pub sessions_present: BTreeSet<IdempotencyKey>,
}

pub async fn inspect_live_session_index(
    mode: &TestMode,
    deps: &BenchmarkTestDependencies,
    id: &OwnedAgentId,
    sessions: &[IdempotencyKey],
) -> anyhow::Result<LiveSessionIndexInspection> {
    let info = live_postgres(matches!(mode, TestMode::Spawned { .. }), || {
        deps.rdb().info()
    })?;
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut connection = PgConnection::connect_with(&info.to_connect_options()).await?;
        let mut keys = vec!["coverage".to_string()];
        keys.extend(sessions.iter().map(|key| format!("session:{}", key.value)));
        let rows: IndexRows = sqlx::query_as(
            "SELECT key, value FROM golem_worker_executor.kv_storage WHERE namespace = $1 AND key = ANY($2)",
        )
        .bind(id.agent_id.durable_stream_session_index_namespace())
        .bind(keys)
        .fetch_all(&mut connection)
        .await?;
        let status: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT value FROM golem_worker_executor.kv_storage WHERE namespace = $1 AND key = $2",
        )
        .bind(format!("agent-status:{}", id.agent_id.to_redis_key()))
        .bind("core")
        .fetch_optional(&mut connection)
        .await?;
        decode_live_inspection(rows, status.map(|(bytes,)| bytes))
    }).await?
}

fn live_postgres(spawned: bool, info: impl FnOnce() -> DbInfo) -> anyhow::Result<PostgresInfo> {
    ensure!(spawned, "live index inspection requires spawned mode");
    let DbInfo::Postgres(info) = info() else {
        anyhow::bail!("live index inspection requires PostgreSQL");
    };
    Ok(info)
}

fn decode_live_inspection(
    rows: IndexRows,
    status: Option<Vec<u8>>,
) -> anyhow::Result<LiveSessionIndexInspection> {
    ensure!(
        rows.iter().all(|(_, value)| !value.is_empty()),
        "empty index value"
    );
    Ok(LiveSessionIndexInspection {
        coverage_present: rows.iter().any(|(key, _)| key == "coverage"),
        sessions_present: rows
            .iter()
            .filter_map(|(key, _)| {
                key.strip_prefix("session:")
                    .map(|key| IdempotencyKey::new(key.to_string()))
            })
            .collect(),
        status: status
            .map(|bytes| {
                try_deserialize(&bytes)
                    .map_err(anyhow::Error::msg)?
                    .context("invalid status serialization")
            })
            .transpose()?,
    })
}

pub struct SpawnedSessionIndexControl<'a> {
    deps: &'a BenchmarkTestDependencies,
    connection: PgConnection,
    _reaped: Vec<Box<dyn Send + Sync>>,
}

impl<'a> SpawnedSessionIndexControl<'a> {
    /// Keep this guard alive only during storage inspection/mutation, and drop
    /// it before restarting executors. Acquisition is bounded and fail-closed.
    pub async fn acquire(
        mode: &TestMode,
        deps: &'a BenchmarkTestDependencies,
    ) -> anyhow::Result<Self> {
        ensure!(
            matches!(mode, TestMode::Spawned { .. }),
            "missing-index control requires spawned mode"
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            let DbInfo::Postgres(info) = deps.rdb().info() else {
                anyhow::bail!("missing-index control requires PostgreSQL");
            };
            let executors = deps.worker_executor_cluster().to_vec();
            ensure!(
                !executors.is_empty(),
                "missing-index control requires executors"
            );
            let mut reaped = Vec::new();
            for executor in executors {
                reaped.push(executor.lock_reaped().await?);
            }
            let connection = PgConnection::connect_with(&info.to_connect_options()).await?;
            Ok(Self {
                deps,
                connection,
                _reaped: reaped,
            })
        })
        .await?
    }

    /// Require coverage and exactly the caller's seeded session keys. Other
    /// derived fields (control, binding, recovery pages) may also be present.
    pub async fn validate(
        &mut self,
        id: &OwnedAgentId,
        sessions: &[IdempotencyKey],
    ) -> anyhow::Result<usize> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let rows = index_rows(
                &mut self.connection,
                &id.agent_id.durable_stream_session_index_namespace(),
            )
            .await?;
            validate_rows(&rows, sessions)?;
            Ok(rows.len())
        })
        .await?
    }

    /// Delete only the target namespace in one transaction, verifying the
    /// selected rows and untouched KV, target oplog, and payload bytes before
    /// committing. No Redis, status, oplog or blob writes are performed.
    pub async fn delete(
        &mut self,
        id: &OwnedAgentId,
        sessions: &[IdempotencyKey],
    ) -> anyhow::Result<usize> {
        tokio::time::timeout(Duration::from_secs(120), async {
            let namespace = id.agent_id.durable_stream_session_index_namespace();
            let payloads = payload_snapshot(self.deps, id).await?;
            let mut tx = self.connection.begin().await?;
            sqlx::query("LOCK TABLE golem_worker_executor.kv_storage IN EXCLUSIVE MODE")
                .execute(&mut *tx)
                .await?;
            let rows = index_rows(&mut tx, &namespace).await?;
            validate_rows(&rows, sessions)?;
            let unrelated = unrelated_rows(&mut tx, &namespace).await?;
            let status_namespace = format!("agent-status:{}", id.agent_id.to_redis_key());
            ensure!(
                unrelated.iter().any(
                    |(namespace, _, value)| namespace == &status_namespace && !value.is_empty()
                ),
                "target status rows are absent"
            );
            let oplog = oplog_rows(&mut tx, id).await?;
            ensure!(!oplog.is_empty(), "target oplog is absent");
            let deleted =
                sqlx::query("DELETE FROM golem_worker_executor.kv_storage WHERE namespace = $1")
                    .bind(&namespace)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected();
            ensure!(deleted == rows.len() as u64, "index row set changed");
            ensure!(
                index_rows(&mut tx, &namespace).await?.is_empty(),
                "target index was not removed"
            );
            ensure!(
                unrelated_rows(&mut tx, &namespace).await? == unrelated,
                "unrelated KV state changed"
            );
            ensure!(
                oplog_rows(&mut tx, id).await? == oplog,
                "target oplog changed"
            );
            ensure!(
                payload_snapshot(self.deps, id).await? == payloads,
                "target payload state changed"
            );
            tx.commit().await?;
            Ok(rows.len())
        })
        .await?
    }
}

async fn index_rows(connection: &mut PgConnection, namespace: &str) -> anyhow::Result<IndexRows> {
    Ok(sqlx::query_as(
        "SELECT key, value FROM golem_worker_executor.kv_storage WHERE namespace = $1 ORDER BY key",
    )
    .bind(namespace)
    .fetch_all(connection)
    .await?)
}

async fn unrelated_rows(
    connection: &mut PgConnection,
    namespace: &str,
) -> anyhow::Result<Vec<(String, String, Vec<u8>)>> {
    Ok(sqlx::query_as("SELECT namespace, key, value FROM golem_worker_executor.kv_storage WHERE namespace <> $1 ORDER BY namespace, key").bind(namespace).fetch_all(connection).await?)
}

async fn oplog_rows(
    connection: &mut PgConnection,
    id: &OwnedAgentId,
) -> anyhow::Result<Vec<(String, i64, Vec<u8>)>> {
    Ok(sqlx::query_as("SELECT namespace, id, value FROM golem_worker_executor_indexed.index_storage WHERE key = $1 ORDER BY namespace, id").bind(id.agent_id.to_redis_key()).fetch_all(connection).await?)
}

fn validate_rows(rows: &IndexRows, sessions: &[IdempotencyKey]) -> anyhow::Result<()> {
    ensure!(
        rows.iter()
            .filter(|(key, value)| key == "coverage" && !value.is_empty())
            .count()
            == 1,
        "exactly one nonempty coverage row is required"
    );
    let expected: BTreeSet<_> = sessions
        .iter()
        .map(|key| format!("session:{}", key.value))
        .collect();
    ensure!(
        expected.len() == sessions.len(),
        "duplicate expected session keys"
    );
    let actual: BTreeSet<_> = rows
        .iter()
        .filter(|(key, _)| key.starts_with("session:"))
        .map(|(key, _)| key.clone())
        .collect();
    ensure!(
        actual == expected,
        "persisted session rows do not match expected sessions"
    );
    ensure!(
        rows.iter().all(|(_, value)| !value.is_empty()),
        "empty index value"
    );
    Ok(())
}

async fn payload_snapshot(
    deps: &BenchmarkTestDependencies,
    id: &OwnedAgentId,
) -> anyhow::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let storage = deps.blob_storage();
    let namespace = BlobStorageNamespace::OplogPayload {
        environment_id: id.environment_id,
        agent_id: id.agent_id.clone(),
        agent_mode: AgentMode::Durable,
    };
    let mut result = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(path) = pending.pop() {
        match storage
            .exists("benchmark", "index_control", namespace.clone(), &path)
            .await?
        {
            ExistsResult::File => {
                let bytes = storage
                    .get_raw("benchmark", "index_control", namespace.clone(), &path)
                    .await?
                    .context("payload disappeared")?;
                result.insert(path, Sha256::digest(bytes).to_vec());
            }
            ExistsResult::Directory => pending.extend(
                storage
                    .list_dir("benchmark", "index_control", namespace.clone(), &path)
                    .await?,
            ),
            ExistsResult::DoesNotExist => {
                ensure!(path.as_os_str().is_empty(), "payload disappeared");
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn live_inspection_rejects_uncontrolled_backends() {
        assert!(
            live_postgres(false, || panic!(
                "uncontrolled backend must not be accessed"
            ))
            .unwrap_err()
            .to_string()
            .contains("spawned")
        );
        assert!(
            live_postgres(true, || DbInfo::Sqlite(PathBuf::new()))
                .unwrap_err()
                .to_string()
                .contains("PostgreSQL")
        );
    }

    #[test]
    fn live_session_index_inspection_preserves_status_horizon_and_absence() {
        let status = AgentStatusRecord {
            oplog_idx: golem_common::model::oplog::OplogIndex::from_u64(127),
            ..Default::default()
        };
        let observed = decode_live_inspection(
            vec![
                ("coverage".into(), vec![1]),
                ("session:old".into(), vec![2]),
            ],
            Some(
                golem_common::serialization::serialize(&status)
                    .unwrap()
                    .to_vec(),
            ),
        )
        .unwrap();
        assert!(observed.coverage_present);
        assert_eq!(observed.status.unwrap().oplog_idx, status.oplog_idx);
        assert_eq!(
            observed.sessions_present,
            BTreeSet::from([IdempotencyKey::new("old".into())])
        );
        let absent = decode_live_inspection(vec![], None).unwrap();
        assert!(!absent.coverage_present);
        assert!(absent.status.is_none());
        assert!(decode_live_inspection(vec![("coverage".into(), vec![])], None).is_err());
        assert!(decode_live_inspection(vec![], Some(vec![255])).is_err());
    }

    #[test]
    fn missing_index_control_validates_exact_sessions_and_coverage() {
        let key = IdempotencyKey::new("old".into());
        let rows = vec![
            ("coverage".into(), vec![1]),
            ("session:old".into(), vec![2]),
        ];
        assert!(validate_rows(&rows, std::slice::from_ref(&key)).is_ok());
        assert!(validate_rows(&rows, &[]).is_err());
        assert!(validate_rows(&rows[1..].to_vec(), std::slice::from_ref(&key)).is_err());
        assert!(validate_rows(&rows, &[key.clone(), key]).is_err());
        assert!(validate_rows(&vec![("coverage".into(), vec![1])], &[]).is_ok());
    }
}
