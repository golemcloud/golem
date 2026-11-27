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
    AgentDataObservation, AgentIndexedState, FencedTxError, IndexedStorage, IndexedStorageError,
    IndexedStorageMetaNamespace, IndexedStorageNamespace, KeyEpochChange, KeyEpochClaim,
    ReleaseAgentResult, ScanResume, UnsealAgentResult,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use golem_common::SafeDisplay;
use golem_common::config::DbSqliteConfig;
use golem_common::metrics::db::record_db_serialized_size;
use golem_common::model::{OwnedAgentId, ShardEpoch};
use golem_service_base::db::sqlite::{SqliteLabelledTransaction, SqlitePool};
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use golem_service_base::repo::RepoError;
use include_dir::include_dir;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DB_TYPE: &str = "sqlite";
const OBSERVE_AGENT_QUERY: &str = "SELECT EXISTS(SELECT 1 FROM index_storage WHERE key = ?1 OR (key >= ?2 || '#' AND key < ?3 || '$')), (SELECT value FROM index_storage WHERE namespace = 'durable-worker-sealed-present' AND key = ?1 AND id = 0), (SELECT value FROM index_storage WHERE namespace = 'ephemeral-worker-sealed-present' AND key = ?1 AND id = 0);";
const AGENT_HAS_DATA_QUERY: &str = "SELECT EXISTS(SELECT 1 FROM index_storage WHERE key = ? OR (key >= ? || '#' AND key < ? || '$'));";
const AGENT_ROWS_QUERY: &str = "SELECT namespace, key, id, value FROM index_storage WHERE key = ? OR (key >= ? || '#' AND key < ? || '$');";
const DELETE_AGENT_ROWS_QUERY: &str =
    "DELETE FROM index_storage WHERE key = ? OR (key >= ? || '#' AND key < ? || '$');";
const SCAN_INCLUSIVE_BOUNDED_QUERY: &str = "SELECT key FROM index_storage WHERE namespace = ? AND key >= ? AND key < ? UNION SELECT key FROM index_storage WHERE namespace = ? AND key >= ? AND key < ? ORDER BY key LIMIT ?;";
const SCAN_EXCLUSIVE_BOUNDED_QUERY: &str = "SELECT key FROM index_storage WHERE namespace = ? AND key > ? AND key < ? UNION SELECT key FROM index_storage WHERE namespace = ? AND key > ? AND key < ? ORDER BY key LIMIT ?;";
const SCAN_INCLUSIVE_UNBOUNDED_QUERY: &str = "SELECT key FROM index_storage WHERE namespace = ? AND key >= ? UNION SELECT key FROM index_storage WHERE namespace = ? AND key >= ? ORDER BY key LIMIT ?;";
const SCAN_EXCLUSIVE_UNBOUNDED_QUERY: &str = "SELECT key FROM index_storage WHERE namespace = ? AND key > ? UNION SELECT key FROM index_storage WHERE namespace = ? AND key > ? ORDER BY key LIMIT ?;";

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration/indexed");

#[derive(Debug)]
enum UnsealTxError {
    Storage(FencedTxError),
    Expired,
}

impl From<RepoError> for UnsealTxError {
    fn from(error: RepoError) -> Self {
        Self::Storage(error.into())
    }
}

impl From<FencedTxError> for UnsealTxError {
    fn from(error: FencedTxError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Debug, Clone)]
pub struct SqliteIndexedStorage {
    pool: SqlitePool,
    require_stage_reservation: bool,
}

impl SqliteIndexedStorage {
    pub(super) async fn configured_managed(
        config: &DbSqliteConfig,
    ) -> Result<Self, IndexedStorageError> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::sqlite::migrate_managed(config, migrations.sqlite_migrations())
            .await
            .map_err(|error| {
                IndexedStorageError::initialization_failed("Managed SQLite migration failed", error)
            })?;
        let pool = SqlitePool::configured_managed(config)
            .await
            .map_err(|error| {
                IndexedStorageError::initialization_failed(
                    "Managed SQLite pool initialization failed",
                    error,
                )
            })?;
        Ok(Self {
            pool,
            require_stage_reservation: false,
        })
    }

    pub async fn configured(config: &DbSqliteConfig) -> Result<Self, IndexedStorageError> {
        Self::migrate(config).await?;

        let pool = SqlitePool::configured(config).await.map_err(|err| {
            IndexedStorageError::initialization_failed(
                "Sqlite indexed storage pool initialization failed",
                err,
            )
        })?;

        Ok(Self {
            pool,
            require_stage_reservation: true,
        })
    }

    /// Apply the indexed storage migrations on the given sqlite config without
    /// creating a pool.
    pub async fn migrate(config: &DbSqliteConfig) -> Result<(), IndexedStorageError> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::sqlite::migrate(config, migrations.sqlite_migrations())
            .await
            .map_err(|err| {
                IndexedStorageError::initialization_failed(
                    "Sqlite indexed storage migration failed",
                    err,
                )
            })
    }

    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            require_stage_reservation: true,
        }
    }

    pub(super) async fn close(&self) {
        self.pool.close().await;
    }

    #[cfg(test)]
    pub(super) async fn hold_connection_for_test(&self) -> SqliteLabelledTransaction {
        use golem_service_base::db::LabelledPoolApi;
        self.pool
            .with_ro("test", "retirement_barrier")
            .begin()
            .await
            .unwrap()
    }

    /// Presence markers and staged entries keep a database alive, even without visible oplogs.
    pub(super) async fn empty_database_epochs(
        &self,
    ) -> Result<Option<Vec<(String, String, i64)>>, IndexedStorageError> {
        // The caller holds exclusive file admission across both reads.
        let mut reader = self.pool.with_ro("multi_sqlite", "empty_database");
        let (present,): (bool,) = reader
            .fetch_one_as(sqlx::query_as(
                "SELECT EXISTS(SELECT 1 FROM index_storage);",
            ))
            .await
            .map_err(Self::classify_repo_error)?;
        if present {
            return Ok(None);
        }
        reader
            .fetch_all_as(sqlx::query_as(
                "SELECT namespace, key, epoch FROM indexed_key_epoch;",
            ))
            .await
            .map(Some)
            .map_err(Self::classify_repo_error)
    }

    pub(super) async fn restore_database_epochs(
        &self,
        epochs: Vec<(String, String, i64)>,
    ) -> Result<(), IndexedStorageError> {
        self.pool
            .with_tx::<_, _>("multi_sqlite", "restore_epochs", |tx| {
                Box::pin(async move {
                    for (namespace, key, epoch) in epochs {
                        tx.execute(
                            sqlx::query(
                                "INSERT INTO indexed_key_epoch (namespace, key, epoch) VALUES (?, ?, ?);",
                            )
                            .bind(namespace)
                            .bind(key)
                            .bind(epoch),
                        )
                        .await?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(Self::classify_repo_error)
    }

    // A record in the shared metadata database transfers authority away from the per-agent file.
    // Remove it after restoring all writer generations, or after unlink if no generations remain.
    pub(super) async fn database_epoch_snapshot(
        &self,
        file: &str,
    ) -> Result<Option<Vec<(String, String, i64)>>, IndexedStorageError> {
        let row: Option<(Vec<u8>,)> = self.pool.with_ro("multi_sqlite", "epoch_snapshot")
            .fetch_optional_as(sqlx::query_as(
                "SELECT value FROM index_storage WHERE namespace = 'database-epochs' AND key = ? AND id = 0;",
            ).bind(file)).await.map_err(Self::classify_repo_error)?;
        row.map(|(bytes,)| {
            serde_json::from_slice(&bytes).map_err(|error| {
                IndexedStorageError::Other(format!("Invalid epoch snapshot for {file}: {error}"))
            })
        })
        .transpose()
    }

    pub(super) async fn save_database_epoch_snapshot(
        &self,
        file: &str,
        epochs: Vec<(String, String, i64)>,
    ) -> Result<(), IndexedStorageError> {
        let bytes = serde_json::to_vec(&epochs).map_err(|error| {
            IndexedStorageError::Other(format!("Failed to encode epochs for {file}: {error}"))
        })?;
        self.pool.with_rw("multi_sqlite", "save_epochs").execute(sqlx::query(
            "INSERT INTO index_storage (namespace, key, id, value) VALUES ('database-epochs', ?, 0, ?) ON CONFLICT(namespace, key, id) DO UPDATE SET value = excluded.value;",
        ).bind(file).bind(bytes)).await.map(|_| ()).map_err(Self::classify_repo_error)
    }

    pub(super) async fn delete_database_epoch_snapshot(
        &self,
        file: &str,
    ) -> Result<(), IndexedStorageError> {
        self.pool
            .with_rw("multi_sqlite", "delete_epochs")
            .execute(
                sqlx::query(
                    "DELETE FROM index_storage WHERE namespace = 'database-epochs' AND key = ?;",
                )
                .bind(file),
            )
            .await
            .map(|_| ())
            .map_err(Self::classify_repo_error)
    }

    pub(super) fn namespace(namespace: IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::SealedPresent { agent_mode, .. } => {
                format!(
                    "{}-worker-sealed-present",
                    super::agent_mode_prefix(agent_mode)
                )
            }
            IndexedStorageNamespace::OpLog {
                owned_agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-oplog")
            }
            IndexedStorageNamespace::StagedOpLog {
                owned_agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-staged-oplog")
            }
            IndexedStorageNamespace::CompressedOpLog {
                owned_agent_id: _,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-c{level}-oplog")
            }
            IndexedStorageNamespace::BlobOplogManifest {
                owned_agent_id: _,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-b{level}-oplog")
            }
        }
    }

    fn meta_namespace(namespace: IndexedStorageMetaNamespace) -> String {
        match namespace {
            IndexedStorageMetaNamespace::Oplog { agent_mode } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-oplog")
            }
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-c{level}-oplog")
            }
            IndexedStorageMetaNamespace::BlobOplogManifest { agent_mode, level } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-b{level}-oplog")
            }
        }
    }

    /// sqlx has no `Encode<Sqlite>` for `u64`, so a value that must stay integer-bound (rather
    /// than go through `Json`, which encodes as TEXT - see [`Self::set_key_epoch`]) has to
    /// cross to `i64` first. Checked, like Postgres's own `to_i64`: an unchecked `as i64` on a
    /// value above `i64::MAX` wraps to negative, and reading that back `as u64` produces a
    /// spuriously huge epoch instead of failing loudly.
    pub(super) fn to_i64(value: u64, field_name: &'static str) -> Result<i64, IndexedStorageError> {
        i64::try_from(value).map_err(|_| {
            IndexedStorageError::Other(format!(
                "SQLite indexed storage cannot represent {field_name}={value} as i64"
            ))
        })
    }

    /// A stored epoch that will not fit a `u64` is corruption, not a fence: `to_i64` refuses to
    /// write one, so a negative column value came from outside this code, and reading it back as
    /// `u64` would wrap it into a spuriously huge epoch.
    pub(super) fn negative_epoch_message(value: i64, key: &str) -> String {
        format!("SQLite indexed storage read a negative epoch {value} for key '{key}'")
    }

    /// The writer generation recorded for `key`, read inside `tx`, which the single-connection write pool keeps current until `tx` ends (see `append_many`).
    async fn stored_epoch(
        tx: &mut SqliteLabelledTransaction,
        namespace: &str,
        key: &str,
    ) -> Result<Option<i64>, RepoError> {
        let stored: Option<(i64,)> = tx
            .fetch_optional_as(
                sqlx::query_as(
                    "SELECT epoch FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                )
                .bind(namespace.to_string())
                .bind(key.to_string()),
            )
            .await?;
        Ok(stored.map(|(epoch,)| epoch))
    }

    /// Refuses the write `tx` makes unless `expected` is the generation recorded for `key`.
    async fn check_epoch(
        tx: &mut SqliteLabelledTransaction,
        namespace: &str,
        key: &str,
        expected: ShardEpoch,
    ) -> Result<(), FencedTxError> {
        let stored = Self::stored_epoch(tx, namespace, key).await?;
        FencedTxError::check_record(key, expected, stored, Self::negative_epoch_message)
    }

    async fn agent_epochs(
        tx: &mut SqliteLabelledTransaction,
        key: &str,
        expected: ShardEpoch,
    ) -> Result<(), FencedTxError> {
        let rows: Vec<(String, i64)> = tx.fetch_all_as(sqlx::query_as(
            "SELECT key, epoch FROM indexed_key_epoch WHERE (namespace GLOB 'durable-worker-*' OR namespace GLOB 'ephemeral-worker-*') AND (key = ? OR substr(key, 1, length(?) + 1) = ? || '#');",
        ).bind(key).bind(key).bind(key)).await?;
        for (key, stored) in rows {
            if stored < 0 || u64::try_from(stored).is_ok_and(|value| value > expected.0) {
                FencedTxError::check_record(
                    &key,
                    expected,
                    Some(stored),
                    Self::negative_epoch_message,
                )?;
            }
        }
        Ok(())
    }

    async fn agent_has_data_in_tx(
        tx: &mut SqliteLabelledTransaction,
        key: &str,
    ) -> Result<bool, RepoError> {
        let (present,): (bool,) = tx
            .fetch_one_as(
                sqlx::query_as(AGENT_HAS_DATA_QUERY)
                    .bind(key)
                    .bind(key)
                    .bind(key),
            )
            .await?;
        Ok(present)
    }

    pub(super) fn classify_repo_error(err: RepoError) -> IndexedStorageError {
        if err.is_transient() {
            IndexedStorageError::Transient(err.to_string())
        } else {
            IndexedStorageError::Other(err.to_safe_string())
        }
    }

    fn classify_repo_error_primary_oplog_insert(err: RepoError) -> IndexedStorageError {
        if err.is_pool_timeout() {
            IndexedStorageError::Transient(err.to_string())
        } else if err.is_transient() {
            IndexedStorageError::Indeterminate(err.to_string())
        } else if err.is_unique_violation() {
            IndexedStorageError::Conflict(format!(
                "possible shard ownership mismatch while writing oplog: {}",
                err.to_safe_string()
            ))
        } else {
            IndexedStorageError::Other(err.to_safe_string())
        }
    }

    /// A blob manifest insert under an index the key already holds is a conflict, which the blob
    /// layer acts on. Its other failures are classified as any other write's are.
    fn classify_repo_error_manifest_insert(err: RepoError) -> IndexedStorageError {
        if err.is_unique_violation() {
            IndexedStorageError::Conflict(format!(
                "the blob oplog manifest already holds the index: {}",
                err.to_safe_string()
            ))
        } else {
            Self::classify_repo_error(err)
        }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl IndexedStorage for SqliteIndexedStorage {
    fn supports_release(&self) -> bool {
        true
    }

    async fn reserve_stage(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
        token: i64,
    ) -> Result<Option<i64>, IndexedStorageError> {
        let key = owned_agent_id.storage_key();
        self.pool.with_tx_err::<Option<i64>, FencedTxError, _>(svc_name, api_name, |tx| {
            Box::pin(async move {
                if Self::agent_has_data_in_tx(tx, &key).await? {
                    return Ok(None);
                }
                tx.execute(sqlx::query("INSERT INTO indexed_key_epoch (namespace, key, epoch) VALUES (?, ?, ?) ON CONFLICT(namespace, key) DO UPDATE SET epoch = excluded.epoch;")
                    .bind(super::STAGE_RESERVATION_NAMESPACE).bind(&key).bind(token)).await?;
                Ok(Some(token))
            })
        }).await.map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error))
    }

    async fn remove_stage_reservation(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
        token: Option<i64>,
    ) -> Result<bool, IndexedStorageError> {
        let key = owned_agent_id.storage_key();
        self.pool.with_tx(svc_name, api_name, |tx| Box::pin(async move {
            let result = tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ? AND (? IS NULL OR epoch = ?);")
                .bind(super::STAGE_RESERVATION_NAMESPACE).bind(key).bind(token).bind(token)).await?;
            Ok(result.rows_affected() > 0)
        })).await.map_err(Self::classify_repo_error)
    }

    async fn observe_agent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<AgentDataObservation, IndexedStorageError> {
        let key = owned_agent_id.storage_key();
        self.pool
            .with_ro(svc_name, api_name)
            .fetch_one_as(
                sqlx::query_as::<_, (bool, Option<Vec<u8>>, Option<Vec<u8>>)>(OBSERVE_AGENT_QUERY)
                    .bind(&key)
                    .bind(&key)
                    .bind(&key),
            )
            .await
            .map_err(Self::classify_repo_error)
            .and_then(|(has_data, durable, ephemeral)| {
                AgentDataObservation::from_raw(has_data, durable, ephemeral)
            })
    }

    async fn mark_sealed(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        state: &AgentIndexedState,
        epoch: ShardEpoch,
        fingerprint: golem_common::model::AgentFingerprint,
        superseded_marker: Option<super::SealMarker>,
    ) -> Result<(), IndexedStorageError> {
        let key = state.owned_agent_id.storage_key();
        let fence = Self::namespace(state.fence_namespace());
        let marker = Self::namespace(state.marker_namespace());
        self.pool.with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
            Box::pin(async move {
                Self::check_epoch(tx, &fence, &key, epoch).await?;
                let existing: Option<(Vec<u8>,)> = tx.fetch_optional_as(sqlx::query_as(
                    "SELECT value FROM index_storage WHERE namespace = ? AND key = ? AND id = 0;",
                ).bind(&marker).bind(&key)).await?;
                let replacement = match existing {
                    Some((value,)) => super::SealMarker::decode(&value).and_then(|marker| marker.replacement(fingerprint, superseded_marker)).map_err(FencedTxError::Corrupt)?,
                    None => Some(super::SealMarker::fresh(fingerprint)),
                };
                if let Some(replacement) = replacement {
                    tx.execute(sqlx::query("INSERT INTO index_storage (namespace, key, id, value) VALUES (?, ?, 0, ?) ON CONFLICT (namespace, key, id) DO UPDATE SET value = excluded.value;")
                        .bind(&marker).bind(&key).bind(replacement.encode().map_err(FencedTxError::Corrupt)?)).await?;
                }
                Ok(())
            })
        }).await.map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error))
    }

    async fn release_agent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        state: &AgentIndexedState,
        epoch: ShardEpoch,
    ) -> Result<ReleaseAgentResult, IndexedStorageError> {
        let key = state.owned_agent_id.storage_key();
        let fence = Self::namespace(state.fence_namespace());
        let mut expected = BTreeMap::new();
        for (level, pairs) in &state.manifests {
            let namespace = Self::namespace(state.manifest_namespace(*level));
            for (id, value) in pairs.iter() {
                let id = Self::to_i64(*id, "manifest id")?;
                if expected
                    .insert((namespace.clone(), id), value.to_vec())
                    .is_some()
                {
                    return Ok(ReleaseAgentResult::NotReleased);
                }
            }
        }
        self.pool.with_tx_err::<_, FencedTxError, _>(svc_name, api_name, |tx| {
            Box::pin(async move {
                if !Self::agent_has_data_in_tx(tx, &key).await? {
                    let (has_epochs,): (bool,) = tx.fetch_one_as(sqlx::query_as(
                        "SELECT EXISTS(SELECT 1 FROM indexed_key_epoch WHERE (namespace GLOB 'durable-worker-*' OR namespace GLOB 'ephemeral-worker-*') AND (key = ? OR substr(key, 1, length(?) + 1) = ? || '#'));",
                    ).bind(&key).bind(&key).bind(&key)).await?;
                    if !has_epochs {
                        tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(&key)).await?;
                        return Ok(ReleaseAgentResult::Released);
                    }
                }
                Self::check_epoch(tx, &fence, &key, epoch).await?;
                Self::agent_epochs(tx, &key, epoch).await?;
                let rows: Vec<(String, String, i64, Vec<u8>)> = tx.fetch_all_as(sqlx::query_as(
                    AGENT_ROWS_QUERY,
                ).bind(&key).bind(&key).bind(&key)).await?;
                for (namespace, row_key, id, value) in rows {
                    if namespace == "durable-worker-sealed-present" || namespace == "ephemeral-worker-sealed-present" {
                        super::SealMarker::decode(&value).map_err(FencedTxError::Corrupt)?;
                    }
                    if namespace.ends_with("-worker-staged-oplog") || namespace.ends_with("-present") {
                        continue;
                    }
                    if row_key != key || expected.remove(&(namespace, id)).as_ref() != Some(&value) {
                        return Ok(ReleaseAgentResult::NotReleased);
                    }
                }
                if !expected.is_empty() {
                    return Ok(ReleaseAgentResult::NotReleased);
                }
                tx.execute(sqlx::query(DELETE_AGENT_ROWS_QUERY)
                    .bind(&key).bind(&key).bind(&key)).await?;
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE (namespace GLOB 'durable-worker-*' OR namespace GLOB 'ephemeral-worker-*') AND (key = ? OR substr(key, 1, length(?) + 1) = ? || '#');")
                    .bind(&key).bind(&key).bind(&key)).await?;
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(&key)).await?;
                Ok(ReleaseAgentResult::Released)
            })
        }).await.map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error))
    }

    async fn unseal_agent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        state: &AgentIndexedState,
        epoch: ShardEpoch,
        record_epoch: ShardEpoch,
        valid_until: SystemTime,
        fingerprint: golem_common::model::AgentFingerprint,
    ) -> Result<UnsealAgentResult, IndexedStorageError> {
        let key = state.owned_agent_id.storage_key();
        if epoch < record_epoch {
            return Err(IndexedStorageError::Fenced {
                key,
                expected: epoch,
                actual: Some(record_epoch),
            });
        }
        let epoch_value = Self::to_i64(epoch.0, "epoch")?;
        let write_epoch = Self::to_i64(epoch.max(record_epoch).0, "epoch")?;
        let deadline = valid_until
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok());
        let fence = Self::namespace(state.fence_namespace());
        let primary = Self::namespace(state.primary_namespace());
        let marker = Self::namespace(state.marker_namespace());
        let namespaces: Vec<_> = state
            .epoch_namespaces()
            .into_iter()
            .map(Self::namespace)
            .collect();
        let mut manifests = Vec::new();
        for (level, pairs) in &state.manifests {
            for (id, value) in pairs.iter() {
                manifests.push((
                    Self::namespace(state.manifest_namespace(*level)),
                    Self::to_i64(*id, "manifest id")?,
                    value.clone(),
                ));
            }
        }
        let result = self.pool.with_tx_err::<_, UnsealTxError, _>(svc_name, api_name, |tx| {
            Box::pin(async move {
                Self::agent_epochs(tx, &key, epoch).await?;
                let epochs: Vec<(String, String, i64)> = tx.fetch_all_as(sqlx::query_as(
                    "SELECT namespace, key, epoch FROM indexed_key_epoch WHERE (namespace GLOB 'durable-worker-*' OR namespace GLOB 'ephemeral-worker-*') AND (key = ? OR substr(key, 1, length(?) + 1) = ? || '#');",
                ).bind(&key).bind(&key).bind(&key)).await?;
                if Self::agent_has_data_in_tx(tx, &key).await? {
                    let rows: Vec<(String, String, i64, Vec<u8>)> = tx.fetch_all_as(sqlx::query_as(
                        AGENT_ROWS_QUERY,
                    ).bind(&key).bind(&key).bind(&key)).await?;
                    let mut expected: BTreeMap<_, _> = manifests.iter().map(|(namespace, id, value)| {
                        ((namespace.clone(), *id), value.to_vec())
                    }).collect();
                    expected.insert((format!("{primary}-present"), 0), Vec::new());
                    expected.insert((marker.clone(), 0), Vec::new());
                    for (namespace, row_key, id, value) in &rows {
                        if namespace == "durable-worker-sealed-present" || namespace == "ephemeral-worker-sealed-present" {
                            let decoded = super::SealMarker::decode(value).map_err(FencedTxError::Corrupt)?;
                            if namespace == &marker && row_key == &key && *id == 0 && decoded.fingerprint == fingerprint {
                                expected.insert((marker.clone(), 0), value.clone());
                            }
                        }
                    }
                    let exact_rows = rows.len() == expected.len() && rows.into_iter().all(|(namespace, row_key, id, value)| {
                        row_key == key && expected.get(&(namespace, id)) == Some(&value)
                    });
                    if exact_rows {
                        let mut required: BTreeSet<_> = namespaces.iter().cloned().collect();
                        required.insert(fence.clone());
                        let exact_epochs = epochs.into_iter().all(|(namespace, row_key, stored)| {
                            if row_key == key {
                                required.remove(&namespace);
                            }
                            stored == epoch_value
                        });
                        if exact_epochs && required.is_empty() {
                            tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(&key)).await?;
                            return Ok(UnsealAgentResult::Unsealed);
                        }
                    }
                    return Ok(UnsealAgentResult::AlreadyPresent);
                }
                let Some(deadline) = deadline else {
                    return Ok(UnsealAgentResult::Expired);
                };
                // SQLite's clock has millisecond precision. Compare its next millisecond to
                // the floored deadline so rounding cannot admit a write after expiry.
                for (namespace, row_key, _) in &epochs {
                    let updated = tx.execute(sqlx::query(
                        "UPDATE indexed_key_epoch SET epoch = ? WHERE namespace = ? AND key = ? AND CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER) + 1 <= ?;",
                    ).bind(write_epoch).bind(namespace).bind(row_key).bind(deadline)).await?;
                    if updated.rows_affected() != 1 {
                        return Err(UnsealTxError::Expired);
                    }
                }
                let mut required: BTreeSet<_> = namespaces.into_iter().collect();
                required.insert(fence);
                for namespace in required {
                    if epochs.iter().any(|(stored_namespace, row_key, _)| stored_namespace == &namespace && row_key == &key) {
                        continue;
                    }
                    let inserted = tx.execute(sqlx::query(
                        "INSERT INTO indexed_key_epoch (namespace, key, epoch) SELECT ?, ?, ? WHERE CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER) + 1 <= ?;",
                    ).bind(namespace).bind(&key).bind(write_epoch).bind(deadline)).await?;
                    if inserted.rows_affected() != 1 {
                        return Err(UnsealTxError::Expired);
                    }
                }
                for (namespace, id, value) in manifests {
                    tx.execute(sqlx::query("INSERT INTO index_storage (namespace, key, id, value) VALUES (?, ?, ?, ?);")
                        .bind(namespace).bind(&key).bind(id).bind(value.as_ref())).await?;
                }
                for (namespace, value) in [(format!("{primary}-present"), Vec::new()), (marker, super::SealMarker::fresh(fingerprint).encode().map_err(FencedTxError::Corrupt)?)] {
                    tx.execute(sqlx::query("INSERT INTO index_storage (namespace, key, id, value) VALUES (?, ?, 0, ?);")
                        .bind(namespace).bind(&key).bind(value)).await?;
                }
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(&key)).await?;
                let (in_time,): (bool,) = tx.fetch_one_as(sqlx::query_as(
                    "SELECT CAST(strftime('%s', 'now') AS INTEGER) * 1000 + CAST(substr(strftime('%f', 'now'), 4, 3) AS INTEGER) + 1 <= ?;",
                ).bind(deadline)).await?;
                if !in_time {
                    return Err(UnsealTxError::Expired);
                }
                Ok(UnsealAgentResult::Unsealed)
            })
        }).await;
        match result {
            Ok(result) => Ok(result),
            Err(UnsealTxError::Expired) => Ok(UnsealAgentResult::Expired),
            Err(UnsealTxError::Storage(error)) => {
                Err(error.into_indexed_storage_error(Self::classify_repo_error))
            }
        }
    }

    async fn remove_key_epoch_if_empty(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        epoch: ShardEpoch,
    ) -> Result<bool, IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        let epoch = Self::to_i64(epoch.0, "epoch")?;
        let key = key.to_string();
        let agent_key = key.split('#').next().unwrap_or(&key).to_string();
        self.pool.with_tx(svc_name, api_name, |tx| {
            Box::pin(async move {
                let removed = tx.execute(sqlx::query(
                    "DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ? AND epoch = ? AND NOT EXISTS(SELECT 1 FROM index_storage WHERE key = ? OR (key >= ? || '#' AND key < ? || '$'));",
                ).bind(namespace).bind(key).bind(epoch).bind(&agent_key).bind(&agent_key).bind(&agent_key)).await?;
                Ok(removed.rows_affected() > 0)
            })
        }).await.map_err(Self::classify_repo_error)
    }

    async fn number_of_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn wait_for_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        let query = sqlx::query_as::<_, (bool,)>(
            "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace IN (?, ?) AND key = ?);",
        )
        .bind(format!("{namespace}-present"))
        .bind(namespace)
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as(query)
            .await
            .map(|row| row.unwrap_or((false,)).0)
            .map_err(Self::classify_repo_error)
    }

    async fn scan_stable(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<ScanResume>,
        count: u64,
    ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError> {
        let bounds = super::stable_scan_key_bounds(prefix, resume, "SQLite")?;
        let namespace = Self::meta_namespace(namespace);
        let count_param = sqlx::types::Json(count);
        let query = match (bounds.inclusive, bounds.upper) {
            (true, Some(upper)) => sqlx::query_as(SCAN_INCLUSIVE_BOUNDED_QUERY)
                .bind(namespace.clone())
                .bind(bounds.lower.clone())
                .bind(upper.clone())
                .bind(format!("{namespace}-present"))
                .bind(bounds.lower)
                .bind(upper)
                .bind(count_param),
            (false, Some(upper)) => sqlx::query_as(SCAN_EXCLUSIVE_BOUNDED_QUERY)
                .bind(namespace.clone())
                .bind(bounds.lower.clone())
                .bind(upper.clone())
                .bind(format!("{namespace}-present"))
                .bind(bounds.lower)
                .bind(upper)
                .bind(count_param),
            (true, None) => sqlx::query_as(SCAN_INCLUSIVE_UNBOUNDED_QUERY)
                .bind(namespace.clone())
                .bind(bounds.lower.clone())
                .bind(format!("{namespace}-present"))
                .bind(bounds.lower)
                .bind(count_param),
            (false, None) => sqlx::query_as(SCAN_EXCLUSIVE_UNBOUNDED_QUERY)
                .bind(namespace.clone())
                .bind(bounds.lower.clone())
                .bind(format!("{namespace}-present"))
                .bind(bounds.lower)
                .bind(count_param),
        };

        let keys = self
            .pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<(String,), _>(query)
            .await
            .map(|keys| keys.into_iter().map(|k| k.0).collect::<Vec<String>>())
            .map_err(Self::classify_repo_error)?;

        Ok((super::last_key_resume(&keys, count), keys))
    }

    /// Delegates to [`Self::append_many`] so there is exactly one fenced write path: the epoch
    /// check has to happen in the same transaction as the insert.
    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
        expected_epoch: Option<ShardEpoch>,
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        self.append_many(
            svc_name,
            api_name,
            entity_name,
            &namespace,
            key,
            vec![(id, Bytes::from(value))].into(),
            expected_epoch,
            stage_token,
        )
        .await
    }

    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
        expected_epoch: Option<ShardEpoch>,
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        let staged = self.require_stage_reservation
            && matches!(namespace, IndexedStorageNamespace::StagedOpLog { .. });
        let initial = self.require_stage_reservation
            && matches!(namespace, IndexedStorageNamespace::OpLog { .. })
            && pairs.iter().any(|(id, _)| *id == 1);
        if pairs.is_empty() && !staged {
            return Ok(());
        }

        let classify: fn(RepoError) -> IndexedStorageError = match namespace {
            IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. } => {
                Self::classify_repo_error_primary_oplog_insert
            }
            IndexedStorageNamespace::BlobOplogManifest { .. } => {
                Self::classify_repo_error_manifest_insert
            }
            IndexedStorageNamespace::CompressedOpLog { .. }
            | IndexedStorageNamespace::SealedPresent { .. } => Self::classify_repo_error,
        };
        let namespace = Self::namespace((*namespace).clone());
        let key = key.to_string();
        for (_, value) in pairs.iter() {
            record_db_serialized_size(DB_TYPE, svc_name, entity_name, value.len());
        }

        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                Box::pin(async move {
                    // SQLite has no `SELECT ... FOR UPDATE`, so labelled write transactions use
                    // `BEGIN IMMEDIATE` (golem-service-base db/sqlite.rs). The reservation covers
                    // this epoch check and insert atomically, including when another process uses
                    // the same SQLite file; a competing writer waits via SQLite's busy timeout.
                    if staged {
                        let stored = Self::stored_epoch(tx, super::STAGE_RESERVATION_NAMESPACE, key.split('#').next().unwrap_or(&key)).await?;
                        FencedTxError::check_stage_token(&key, stage_token, stored)?;
                    }
                    if let Some(expected) = expected_epoch {
                        Self::check_epoch(tx, &namespace, &key, expected).await?;
                    }

                    for (id, value) in pairs.iter() {
                        tx.execute(
                            sqlx::query(
                                "INSERT INTO index_storage (namespace, key, id, value) VALUES (?,?,?,?);",
                            )
                            .bind(namespace.as_str())
                            .bind(key.as_str())
                            .bind(sqlx::types::Json(*id))
                            .bind(value.as_ref()),
                        )
                        .await?;
                    }

                    if initial {
                        tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;")
                            .bind(super::STAGE_RESERVATION_NAMESPACE).bind(&key)).await?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(classify))
    }

    /// SQLite's half of [`IndexedStorage::set_key_epoch`], which states the rule this
    /// enforces. The unqualified `epoch` in the `WHERE` is the existing row's, and `excluded` is
    /// the row being written.
    async fn set_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        new_epoch: ShardEpoch,
        presence_namespaces: &[IndexedStorageNamespace],
        remove_if_absent: bool,
    ) -> Result<KeyEpochClaim, IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        // `i64`, not `u64`: sqlx has no `Encode<Sqlite>` for `u64`, which is why ids elsewhere in
        // this file go through `Json`. That encodes as TEXT, and comparison affinity is applied
        // per operand, so this column stays integer-bound everywhere. Checked (see `to_i64`)
        // rather than `as i64`, which would silently wrap an out-of-range epoch to negative.
        let epoch = Self::to_i64(new_epoch.0, "epoch")?;
        let presence_namespaces: BTreeSet<_> = presence_namespaces
            .iter()
            .cloned()
            .map(Self::namespace)
            .chain(std::iter::once(namespace.clone()))
            .collect();
        let key = key.to_string();
        self.pool.with_tx_err::<KeyEpochClaim, FencedTxError, _>(svc_name, api_name, |tx| {
            Box::pin(async move {
                let stored: Option<(i64,)> = tx.fetch_optional_as(sqlx::query_as(
                    "SELECT epoch FROM indexed_key_epoch WHERE namespace = ? AND key = ?;"
                ).bind(&namespace).bind(&key)).await?;
                let change = match stored {
                    None => KeyEpochChange::Inserted,
                    Some((stored,)) if stored < epoch => KeyEpochChange::Raised,
                    Some(_) => KeyEpochChange::Unchanged,
                };
                let result = tx.execute(sqlx::query(
                    r#"INSERT INTO indexed_key_epoch (namespace, key, epoch) VALUES (?, ?, ?)
                       ON CONFLICT(namespace, key) DO UPDATE SET epoch = excluded.epoch
                       WHERE epoch <= excluded.epoch;"#,
                ).bind(&namespace).bind(&key).bind(epoch)).await?;
                if result.rows_affected() == 0 {
                    Self::check_epoch(tx, &namespace, &key, new_epoch).await?;
                }
                for presence_namespace in presence_namespaces {
                    let (present,): (bool,) = tx.fetch_one_as(sqlx::query_as(
                        "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace IN (?, ?) AND key = ?);",
                    ).bind(format!("{presence_namespace}-present")).bind(presence_namespace).bind(&key)).await?;
                    if present {
                        return Ok(KeyEpochClaim { has_data: true, change });
                    }
                }
                if remove_if_absent && change == KeyEpochChange::Inserted {
                    tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ? AND epoch = ?;")
                        .bind(namespace).bind(key).bind(epoch)).await?;
                }
                Ok(KeyEpochClaim { has_data: false, change })
            })
        }).await.map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error))
    }

    async fn delete_empty_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<bool, IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        let key = key.to_string();
        self.pool
            .with_tx_err::<bool, FencedTxError, _>(svc_name, api_name, |tx| {
                Box::pin(async move {
                    // The single-connection write pool makes the check, the emptiness test and
                    // the delete one step, as it does for an append (see `append_many`).
                    if let Some(expected) = expected_epoch {
                        Self::check_epoch(tx, &namespace, &key, expected).await?;
                    }
                    let (has_entries,): (bool,) = tx
                        .fetch_one_as(
                            sqlx::query_as(
                                "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace = ? AND key = ?);",
                            )
                            .bind(namespace.clone())
                            .bind(key.clone()),
                        )
                        .await?;
                    if has_entries {
                        return Ok(false);
                    }
                    tx.execute(
                        sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ?;")
                            .bind(format!("{namespace}-present"))
                            .bind(key.clone()),
                    )
                    .await?;
                    Ok(true)
                })
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error))
    }

    async fn delete_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
        retain_epoch: bool,
    ) -> Result<(), IndexedStorageError> {
        if retain_epoch && expected_epoch.is_none() {
            return Err(IndexedStorageError::Other(
                "Retaining a SQLite indexed epoch requires an asserted epoch".into(),
            ));
        }
        let retained_epoch = expected_epoch
            .filter(|_| retain_epoch)
            .map(|epoch| Self::to_i64(epoch.0, "epoch"))
            .transpose()?;
        let mode = match &namespace {
            IndexedStorageNamespace::SealedPresent { agent_mode, .. }
            | IndexedStorageNamespace::OpLog { agent_mode, .. }
            | IndexedStorageNamespace::StagedOpLog { agent_mode, .. }
            | IndexedStorageNamespace::CompressedOpLog { agent_mode, .. }
            | IndexedStorageNamespace::BlobOplogManifest { agent_mode, .. } => *agent_mode,
        };
        let seal_marker = format!("{}-worker-sealed-present", super::agent_mode_prefix(mode));
        let namespace = Self::namespace(namespace);
        let key = key.to_string();
        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                Box::pin(async move {
                    // The single-connection write pool makes the check and the deletes one
                    // step, as it does for an append (see `append_many`).
                    if let Some(expected) = expected_epoch {
                        let stored = Self::stored_epoch(tx, &namespace, &key).await?;
                        // Neither a record nor entries: the key is already gone, most often taken
                        // by an earlier attempt of this same deletion whose later steps failed, and
                        // a writer that lost the key has nothing here to destroy. Refusing would
                        // leave that deletion unable to finish.
                        if stored.is_none() {
                            let (has_entries,): (bool,) = tx
                                .fetch_one_as(
                                    sqlx::query_as(
                                        "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace IN (?, ?) AND key = ?);",
                                    )
                                    .bind(format!("{namespace}-present"))
                                    .bind(namespace.clone())
                                    .bind(key.clone()),
                                )
                                .await?;
                            if !has_entries {
                                if !retain_epoch {
                                    return Ok(());
                                }
                                let agent_key = key.split('#').next().unwrap_or(&key);
                                Self::agent_epochs(tx, agent_key, expected).await?;
                            } else {
                                FencedTxError::check_record(
                                    &key,
                                    expected,
                                    stored,
                                    Self::negative_epoch_message,
                                )?;
                            }
                        } else {
                            FencedTxError::check_record(
                                &key,
                                expected,
                                stored,
                                Self::negative_epoch_message,
                            )?;
                        }
                    }
                    tx.execute(
                        sqlx::query(
                            "DELETE FROM index_storage WHERE namespace IN (?, ?) AND key = ?;",
                        )
                        .bind(format!("{namespace}-present"))
                        .bind(namespace.clone())
                        .bind(key.clone()),
                    )
                    .await?;
                    if let Some(epoch) = retained_epoch {
                        tx.execute(sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ?;")
                            .bind(seal_marker).bind(key.split('#').next().unwrap_or(&key))).await?;
                        tx.execute(sqlx::query("INSERT INTO indexed_key_epoch (namespace, key, epoch) VALUES (?, ?, ?) ON CONFLICT(namespace, key) DO UPDATE SET epoch = excluded.epoch;")
                            .bind(namespace).bind(key).bind(epoch)).await?;
                    } else {
                        tx.execute(
                            sqlx::query(
                                "DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                            )
                            .bind(namespace)
                            .bind(key),
                        )
                        .await?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error))
    }

    async fn move_if_absent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        source_namespace: IndexedStorageNamespace,
        source_key: &str,
        target_namespace: IndexedStorageNamespace,
        target_key: &str,
        expected_last_id: u64,
        stage_token: Option<i64>,
    ) -> Result<bool, IndexedStorageError> {
        if expected_last_id == 0 {
            return Err(IndexedStorageError::Other(
                "source index expected tip must be greater than zero".to_string(),
            ));
        }
        let expected = i64::try_from(expected_last_id).map_err(|_| {
            IndexedStorageError::Other("source index tip exceeds storage range".into())
        })?;
        let staged = self.require_stage_reservation
            && matches!(
                &source_namespace,
                IndexedStorageNamespace::StagedOpLog { .. }
            );
        let source_namespace = Self::namespace(source_namespace);
        let target_namespace = Self::namespace(target_namespace);
        let source_key = source_key.to_string();
        let target_key = target_key.to_string();
        let result = self.pool
            .with_tx_err::<bool, FencedTxError, _>(svc_name, api_name, |tx| {
                async move {
                    // Acquire the write lock before reading the source. A read-first
                    // transaction cannot upgrade its snapshot after a concurrent write.
                    if staged {
                        let stored = Self::stored_epoch(tx, super::STAGE_RESERVATION_NAMESPACE, source_key.split('#').next().unwrap_or(&source_key)).await?;
                        FencedTxError::check_stage_token(&source_key, stage_token, stored)?;
                    }
                    let claimed = tx
                        .execute(
                            sqlx::query("INSERT INTO index_storage (namespace, key, id, value) SELECT ?, ?, 0, x'' WHERE NOT EXISTS(SELECT 1 FROM index_storage WHERE namespace IN (?, ?) AND key = ?);")
                                .bind(format!("{target_namespace}-present"))
                                .bind(&target_key)
                                .bind(&target_namespace)
                                .bind(format!("{target_namespace}-present"))
                                .bind(&target_key),
                        )
                        .await?;
                    if claimed.rows_affected() == 0 {
                        return Ok(false);
                    }
                    let source: (i64, Option<i64>, Option<i64>) = tx
                        .fetch_one_as(
                            sqlx::query_as("SELECT COUNT(*), MIN(id), MAX(id) FROM index_storage WHERE namespace = ? AND key = ?;")
                                .bind(&source_namespace)
                                .bind(&source_key),
                        )
                        .await?;
                    if source != (expected, Some(1), Some(expected)) {
                        return Err(FencedTxError::Corrupt("source index is missing, empty, gapped, or has an unexpected tip".into()));
                    }
                    tx.execute(
                        sqlx::query("UPDATE index_storage SET namespace = ?, key = ? WHERE namespace = ? AND key = ?;")
                            .bind(&target_namespace)
                            .bind(&target_key)
                            .bind(&source_namespace)
                            .bind(&source_key),
                    ).await?;
                    tx.execute(sqlx::query("DELETE FROM index_storage WHERE namespace IN (?, ?) AND key = ?;")
                        .bind(&source_namespace).bind(format!("{source_namespace}-present")).bind(&source_key)).await?;
                    if staged {
                        tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;")
                            .bind(super::STAGE_RESERVATION_NAMESPACE).bind(source_key.split('#').next().unwrap_or(&source_key))).await?;
                    }
                    Ok(true)
                }.boxed()
            })
            .await;
        match result {
            Err(FencedTxError::Repo(err)) if err.is_unique_violation() => Ok(false),
            result => result.map_err(|err| {
                err.into_indexed_storage_error(Self::classify_repo_error_primary_oplog_insert)
            }),
        }
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        let query = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM index_storage WHERE namespace = ? AND key = ?;",
        )
        .bind(Self::namespace(namespace))
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as(query)
            .await
            .map(|row| row.map(|r| r.0 as u64).unwrap_or(0))
            .map_err(Self::classify_repo_error)
    }

    async fn discard_stage(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        if !self.require_stage_reservation {
            return self.delete(svc_name, api_name, namespace, key).await;
        }
        let namespace = Self::namespace(namespace);
        let key = key.to_string();
        self.pool
            .with_tx(svc_name, api_name, |tx| {
                Box::pin(async move {
                    let agent_key = key.split('#').next().unwrap_or(&key);
                    let stored =
                        Self::stored_epoch(tx, super::STAGE_RESERVATION_NAMESPACE, agent_key)
                            .await?;
                    if stage_token.is_some() && stored == stage_token {
                        tx.execute(
                            sqlx::query(
                                "DELETE FROM index_storage WHERE namespace IN (?, ?) AND key = ?;",
                            )
                            .bind(format!("{namespace}-present"))
                            .bind(namespace)
                            .bind(&key),
                        )
                        .await?;
                        tx.execute(
                            sqlx::query(
                                "DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                            )
                            .bind(super::STAGE_RESERVATION_NAMESPACE)
                            .bind(agent_key),
                        )
                        .await?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(Self::classify_repo_error)
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let staged = self.require_stage_reservation
            && matches!(&namespace, IndexedStorageNamespace::StagedOpLog { .. });
        let namespace = Self::namespace(namespace);
        if staged {
            let key = key.to_string();
            return self
                .pool
                .with_tx(svc_name, api_name, |tx| {
                    Box::pin(async move {
                        tx.execute(
                            sqlx::query(
                                "DELETE FROM index_storage WHERE namespace IN (?, ?) AND key = ?;",
                            )
                            .bind(format!("{namespace}-present"))
                            .bind(&namespace)
                            .bind(&key),
                        )
                        .await?;
                        Ok(())
                    })
                })
                .await
                .map_err(Self::classify_repo_error);
        }
        let query = sqlx::query("DELETE FROM index_storage WHERE namespace IN (?, ?) AND key = ?;")
            .bind(format!("{namespace}-present"))
            .bind(namespace)
            .bind(key);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(Self::classify_repo_error)
    }

    async fn read(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
        let query = sqlx::query_as(
            "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? AND id BETWEEN ? AND ?;",
        )
            .bind(Self::namespace(namespace))
            .bind(key)
            .bind(sqlx::types::Json(start_id))
            .bind(sqlx::types::Json(end_id));

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<DBIdValue, _>(query)
            .await
            .map(|vec| vec.into_iter().map(|row| row.into_pair()).collect())
            .map_err(Self::classify_repo_error)
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let query = sqlx::query_as(
                    "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? ORDER BY id ASC LIMIT 1;",
                )
                    .bind(Self::namespace(namespace))
                    .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<DBIdValue, _>(query)
            .await
            .map(|op| op.map(|row| row.into_pair()))
            .map_err(Self::classify_repo_error)
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let query = sqlx::query_as(
                    "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? ORDER BY id DESC LIMIT 1;",
                )
                .bind(Self::namespace(namespace))
                .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<DBIdValue, _>(query)
            .await
            .map(|op| op.map(|row| row.into_pair()))
            .map_err(Self::classify_repo_error)
    }

    async fn last_id(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        let query = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM index_storage WHERE namespace = ? AND key = ? ORDER BY id DESC LIMIT 1;",
        )
        .bind(Self::namespace(namespace))
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<(i64,), _>(query)
            .await
            .map(|op| op.map(|row| row.0 as u64))
            .map_err(Self::classify_repo_error)
    }

    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let query = sqlx::query_as(
            "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? AND id >= ? ORDER BY id ASC LIMIT 1;",
        )
            .bind(Self::namespace(namespace))
            .bind(key)
            .bind(sqlx::types::Json(id));

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<DBIdValue, _>(query)
            .await
            .map(|op| op.map(|row| row.into_pair()))
            .map_err(Self::classify_repo_error)
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let delete_if_empty = matches!(
            &namespace,
            IndexedStorageNamespace::CompressedOpLog { .. }
                | IndexedStorageNamespace::BlobOplogManifest { .. }
        );
        let namespace = Self::namespace(namespace);
        let key = key.to_string();
        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                Box::pin(async move {
                    // The single-connection write pool makes the check and the trim one step,
                    // as it does for an append (see `append_many`).
                    if let Some(expected) = expected_epoch {
                        Self::check_epoch(tx, &namespace, &key, expected).await?;
                    }
                    tx.execute(sqlx::query(
                        "INSERT OR IGNORE INTO index_storage (namespace, key, id, value) SELECT ?, ?, 0, x'' WHERE EXISTS (SELECT 1 FROM index_storage WHERE namespace = ? AND key = ?);")
                        .bind(format!("{namespace}-present")).bind(&key).bind(&namespace).bind(&key)).await?;
                    tx.execute(sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ? AND id <= ?;")
                        .bind(&namespace).bind(&key).bind(sqlx::types::Json(last_dropped_id))).await?;
                    if delete_if_empty {
                        tx.execute(sqlx::query(
                            "DELETE FROM index_storage WHERE namespace = ? AND key = ? AND NOT EXISTS (SELECT 1 FROM index_storage WHERE namespace = ? AND key = ?);",
                        )
                        .bind(format!("{namespace}-present"))
                        .bind(&key)
                        .bind(&namespace)
                        .bind(&key)).await?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error))
    }
}

#[derive(sqlx::FromRow, Debug)]
struct DBIdValue {
    pub id: i64,
    value: Vec<u8>,
}

impl DBIdValue {
    fn into_pair(self) -> (u64, Vec<u8>) {
        (self.id as u64, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentId, OwnedAgentId};
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{Connection, Executor};
    use test_r::test;

    fn oplog_namespace(agent_id: &str) -> IndexedStorageNamespace {
        IndexedStorageNamespace::OpLog {
            owned_agent_id: OwnedAgentId::new(
                EnvironmentId(uuid::Uuid::from_u128(1)),
                &crate::test_agent_id(agent_id),
            ),
            agent_mode: AgentMode::Durable,
        }
    }

    async fn sqlite_storage(database: String) -> SqliteIndexedStorage {
        SqliteIndexedStorage::configured(&DbSqliteConfig {
            database,
            max_connections: 1,
            foreign_keys: false,
        })
        .await
        .unwrap()
    }

    fn sealed_fingerprint() -> golem_common::model::AgentFingerprint {
        golem_common::model::AgentFingerprint(uuid::Uuid::from_u128(42))
    }

    fn sealed_state(agent_mode: AgentMode) -> AgentIndexedState {
        AgentIndexedState {
            owned_agent_id: OwnedAgentId::new(
                EnvironmentId(uuid::Uuid::from_u128(1)),
                &AgentId::from_parts(7, 1),
            ),
            agent_mode,
            compressed_layers: 1,
            manifests: BTreeMap::from([(2, vec![(9, Bytes::from_static(b"manifest"))].into())]),
        }
    }

    #[test]
    async fn agent_lookup_plans_use_agent_index_for_present_and_absent_rows() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(
            tempdir
                .path()
                .join("agent-plan.db")
                .to_string_lossy()
                .into(),
        )
        .await;
        storage.pool.with_rw("test", "seed_plan").execute(sqlx::query(
            "WITH RECURSIVE agents(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM agents WHERE n < 2048), namespaces(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM namespaces WHERE n < 16) INSERT INTO index_storage (namespace, key, id, value) SELECT 'unconfigured-' || namespaces.n, 'other-agent-' || agents.n, 1, x'01' FROM agents CROSS JOIN namespaces;",
        )).await.unwrap();
        let state = sealed_state(AgentMode::Durable);
        let key = state.owned_agent_id.storage_key();
        // Neither a staged-only row nor an unconfigured namespace may be hidden by the lookup.
        for (namespace, row_key, present) in [
            ("unconfigured-level", key.clone(), true),
            (
                "durable-worker-staged-oplog",
                format!("{key}#00000000-0000-0000-0000-000000000001"),
                true,
            ),
            ("unconfigured-level", format!("{key}$neighbor"), false),
        ] {
            storage.pool.with_rw("test", "seed_target").execute(sqlx::query(
                "INSERT INTO index_storage (namespace, key, id, value) VALUES (?, ?, 1, x'01');",
            ).bind(namespace).bind(&row_key)).await.unwrap();
            storage
                .pool
                .with_rw("test", "analyze")
                .execute(sqlx::query("ANALYZE index_storage;"))
                .await
                .unwrap();
            for (lookup, expected) in [(key.as_str(), present), ("absent-agent", false)] {
                let (found, durable, ephemeral): (bool, Option<Vec<u8>>, Option<Vec<u8>>) = storage
                    .pool
                    .with_ro("test", "presence")
                    .fetch_one_as(
                        sqlx::query_as(OBSERVE_AGENT_QUERY)
                            .bind(lookup)
                            .bind(lookup)
                            .bind(lookup),
                    )
                    .await
                    .unwrap();
                assert_eq!(found, expected);
                assert_eq!(durable, None);
                assert_eq!(ephemeral, None);
                for query in [
                    OBSERVE_AGENT_QUERY,
                    AGENT_ROWS_QUERY,
                    DELETE_AGENT_ROWS_QUERY,
                ] {
                    let explain = format!("EXPLAIN QUERY PLAN {query}");
                    let plan: Vec<(i64, i64, i64, String)> = storage
                        .pool
                        .with_ro("test", "agent_plan")
                        .fetch_all_as(
                            sqlx::query_as(&explain)
                                .bind(lookup)
                                .bind(lookup)
                                .bind(lookup),
                        )
                        .await
                        .unwrap();
                    let details = plan
                        .iter()
                        .map(|row| row.3.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    assert!(
                        details.contains("idx_index_storage_agent"),
                        "{query}: {details}"
                    );
                    assert!(
                        !details.contains("SCAN index_storage"),
                        "{query}: {details}"
                    );
                    assert_eq!(
                        details.matches("idx_index_storage_agent").count(),
                        2,
                        "both the exact key and staged range must use the index: {details}"
                    );
                }
            }
            assert_eq!(
                storage
                    .observe_agent("test", "public_presence", &state.owned_agent_id)
                    .await
                    .unwrap()
                    .has_data,
                present
            );
            let rows: Vec<(String, String, i64, Vec<u8>)> = storage
                .pool
                .with_ro("test", "unseal_rows")
                .fetch_all_as(
                    sqlx::query_as(AGENT_ROWS_QUERY)
                        .bind(&key)
                        .bind(&key)
                        .bind(&key),
                )
                .await
                .unwrap();
            assert_eq!(rows.len(), usize::from(present));
            let deleted = storage
                .pool
                .with_rw("test", "release_delete")
                .execute(
                    sqlx::query(DELETE_AGENT_ROWS_QUERY)
                        .bind(&key)
                        .bind(&key)
                        .bind(&key),
                )
                .await
                .unwrap();
            assert_eq!(deleted.rows_affected(), u64::from(present));
            storage
                .pool
                .with_rw("test", "cleanup_target")
                .execute(sqlx::query("DELETE FROM index_storage WHERE key = ?;").bind(row_key))
                .await
                .unwrap();
        }
    }

    #[test]
    async fn committed_unseal_retry_preserves_generation_without_writes_after_deadline() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(
            tempdir
                .path()
                .join("read-only-retry.db")
                .to_string_lossy()
                .into(),
        )
        .await;
        let state = sealed_state(AgentMode::Durable);
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "restore",
                    &state,
                    ShardEpoch(7),
                    ShardEpoch(6),
                    SystemTime::now() + Duration::from_secs(30),
                    sealed_fingerprint(),
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        let before = storage
            .observe_agent("test", "before", &state.owned_agent_id)
            .await
            .unwrap();
        assert!(before.has_data && before.durable_marker.is_some());
        for table in ["index_storage", "indexed_key_epoch"] {
            for operation in ["INSERT", "UPDATE", "DELETE"] {
                let sql = format!(
                    "CREATE TRIGGER reject_{table}_{operation} BEFORE {operation} ON {table} BEGIN SELECT RAISE(ABORT, 'committed retry must not write'); END;"
                );
                storage
                    .pool
                    .with_rw("test", "reject_writes")
                    .execute(sqlx::query(&sql))
                    .await
                    .unwrap();
            }
        }
        storage
            .mark_sealed(
                "test",
                "read_only_mark_retry",
                &state,
                ShardEpoch(7),
                sealed_fingerprint(),
                None,
            )
            .await
            .unwrap();
        for deadline in [UNIX_EPOCH, UNIX_EPOCH - Duration::from_secs(1)] {
            assert_eq!(
                storage
                    .unseal_agent(
                        "test",
                        "retry",
                        &state,
                        ShardEpoch(7),
                        ShardEpoch(6),
                        deadline,
                        sealed_fingerprint(),
                    )
                    .await
                    .unwrap(),
                UnsealAgentResult::Unsealed
            );
            assert_eq!(
                storage
                    .observe_agent("test", "after", &state.owned_agent_id)
                    .await
                    .unwrap(),
                before
            );
        }
    }

    #[test]
    async fn release_checks_all_modes_and_levels_and_removes_crash_left_stages() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage =
            sqlite_storage(tempdir.path().join("release.db").to_string_lossy().into()).await;
        let state = sealed_state(AgentMode::Durable);
        let key = state.owned_agent_id.storage_key();
        let epoch = ShardEpoch(5);
        let stage_token = storage
            .reserve_stage("test", "reserve", &state.owned_agent_id, 55)
            .await
            .unwrap()
            .unwrap();
        for namespace in state.epoch_namespaces() {
            storage
                .set_key_epoch("test", "seed", namespace, &key, epoch, &[], false)
                .await
                .unwrap();
        }
        for (level, entries) in &state.manifests {
            storage
                .append_many(
                    "test",
                    "seed",
                    "manifest",
                    &state.manifest_namespace(*level),
                    &key,
                    entries.clone(),
                    Some(epoch),
                    None,
                )
                .await
                .unwrap();
        }
        storage
            .mark_sealed("test", "seed", &state, epoch, sealed_fingerprint(), None)
            .await
            .unwrap();
        let extra = IndexedStorageNamespace::CompressedOpLog {
            owned_agent_id: state.owned_agent_id.clone(),
            agent_mode: AgentMode::Ephemeral,
            level: 99,
        };
        storage
            .set_key_epoch(
                "test",
                "epoch",
                extra.clone(),
                &key,
                ShardEpoch(6),
                &[],
                false,
            )
            .await
            .unwrap();
        assert!(matches!(
            storage
                .release_agent("test", "release", &state, epoch)
                .await,
            Err(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(6)),
                ..
            })
        ));
        assert!(
            storage
                .exists("test", "exists", state.marker_namespace(), &key)
                .await
                .unwrap()
        );
        storage
            .delete_with_epoch(
                "test",
                "delete",
                extra.clone(),
                &key,
                Some(ShardEpoch(6)),
                false,
            )
            .await
            .unwrap();
        storage
            .append(
                "test",
                "append",
                "entry",
                extra.clone(),
                &key,
                1,
                vec![1],
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .release_agent("test", "release", &state, epoch)
                .await
                .unwrap(),
            ReleaseAgentResult::NotReleased
        );
        storage.delete("test", "delete", extra, &key).await.unwrap();

        let extra_manifest = IndexedStorageNamespace::BlobOplogManifest {
            owned_agent_id: state.owned_agent_id.clone(),
            agent_mode: AgentMode::Ephemeral,
            level: 99,
        };
        storage
            .append(
                "test",
                "append",
                "manifest",
                extra_manifest.clone(),
                &key,
                9,
                b"manifest".to_vec(),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .release_agent("test", "release", &state, epoch)
                .await
                .unwrap(),
            ReleaseAgentResult::NotReleased
        );
        storage
            .delete("test", "delete", extra_manifest, &key)
            .await
            .unwrap();

        let stage = IndexedStorageNamespace::StagedOpLog {
            owned_agent_id: state.owned_agent_id.clone(),
            agent_mode: AgentMode::Ephemeral,
        };
        let stage_key = format!("{key}#{}", uuid::Uuid::from_u128(55));
        storage
            .set_key_epoch(
                "test",
                "epoch",
                stage.clone(),
                &stage_key,
                ShardEpoch(4),
                &[],
                false,
            )
            .await
            .unwrap();
        storage
            .append(
                "test",
                "stage",
                "entry",
                stage,
                &stage_key,
                1,
                vec![1],
                Some(ShardEpoch(4)),
                Some(stage_token),
            )
            .await
            .unwrap();
        let mut mismatched = state.clone();
        mismatched
            .manifests
            .insert(2, vec![(9, Bytes::from_static(b"wrong"))].into());
        assert_eq!(
            storage
                .release_agent("test", "release", &mismatched, epoch)
                .await
                .unwrap(),
            ReleaseAgentResult::NotReleased
        );
        assert_eq!(
            storage
                .release_agent("test", "release", &state, epoch)
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
        assert_eq!(storage.empty_database_epochs().await.unwrap(), Some(vec![]));
        assert_eq!(
            storage
                .release_agent("test", "retry_release", &state, epoch)
                .await
                .unwrap(),
            ReleaseAgentResult::Released
        );
    }

    #[test]
    async fn unseal_raises_bare_epochs_and_retained_delete_fences_old_owners() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage =
            sqlite_storage(tempdir.path().join("unseal.db").to_string_lossy().into()).await;
        let state = sealed_state(AgentMode::Ephemeral);
        let key = state.owned_agent_id.storage_key();
        let old_namespace = IndexedStorageNamespace::CompressedOpLog {
            owned_agent_id: state.owned_agent_id.clone(),
            agent_mode: AgentMode::Durable,
            level: 99,
        };
        storage
            .set_key_epoch(
                "test",
                "epoch",
                old_namespace.clone(),
                &key,
                ShardEpoch(2),
                &[],
                false,
            )
            .await
            .unwrap();
        let deadline = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "restore",
                    &state,
                    ShardEpoch(5),
                    ShardEpoch(4),
                    deadline,
                    sealed_fingerprint(),
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        storage
            .mark_sealed(
                "test",
                "mark",
                &state,
                ShardEpoch(5),
                sealed_fingerprint(),
                None,
            )
            .await
            .unwrap();
        let (raised,): (i64,) = storage
            .pool
            .with_ro("test", "raised_epoch")
            .fetch_one_as(
                sqlx::query_as(
                    "SELECT epoch FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                )
                .bind(SqliteIndexedStorage::namespace(old_namespace.clone()))
                .bind(&key),
            )
            .await
            .unwrap();
        assert_eq!(raised, 5);
        assert!(matches!(
            storage
                .mark_sealed(
                    "test",
                    "mark",
                    &state,
                    ShardEpoch(4),
                    sealed_fingerprint(),
                    None
                )
                .await,
            Err(IndexedStorageError::Fenced { .. })
        ));
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "restore",
                    &state,
                    ShardEpoch(5),
                    ShardEpoch(4),
                    UNIX_EPOCH,
                    sealed_fingerprint(),
                )
                .await
                .unwrap(),
            UnsealAgentResult::Unsealed
        );
        assert!(
            !storage
                .remove_key_epoch_if_empty("test", "gc", old_namespace.clone(), &key, ShardEpoch(5))
                .await
                .unwrap()
        );
        storage
            .delete_with_epoch(
                "test",
                "delete_seal",
                state.fence_namespace(),
                &key,
                Some(ShardEpoch(5)),
                true,
            )
            .await
            .unwrap();
        assert!(
            !storage
                .exists("test", "exists", state.marker_namespace(), &key)
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .unseal_agent(
                    "test",
                    "changed_rows",
                    &state,
                    ShardEpoch(5),
                    ShardEpoch(4),
                    deadline,
                    sealed_fingerprint(),
                )
                .await
                .unwrap(),
            UnsealAgentResult::AlreadyPresent
        );
        storage
            .release_agent("test", "release", &state, ShardEpoch(5))
            .await
            .unwrap();
        storage
            .delete_with_epoch(
                "test",
                "delete",
                state.fence_namespace(),
                &key,
                Some(ShardEpoch(6)),
                true,
            )
            .await
            .unwrap();
        storage
            .delete_with_epoch(
                "test",
                "retry",
                state.fence_namespace(),
                &key,
                Some(ShardEpoch(6)),
                true,
            )
            .await
            .unwrap();
        assert!(matches!(
            storage
                .unseal_agent(
                    "test",
                    "restore",
                    &state,
                    ShardEpoch(5),
                    ShardEpoch(4),
                    deadline,
                    sealed_fingerprint(),
                )
                .await,
            Err(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(6)),
                ..
            })
        ));
        assert!(
            !storage
                .remove_key_epoch_if_empty(
                    "test",
                    "gc",
                    state.fence_namespace(),
                    &key,
                    ShardEpoch(5)
                )
                .await
                .unwrap()
        );
        assert!(
            storage
                .remove_key_epoch_if_empty(
                    "test",
                    "gc",
                    state.fence_namespace(),
                    &key,
                    ShardEpoch(6)
                )
                .await
                .unwrap()
        );
        assert_eq!(storage.empty_database_epochs().await.unwrap(), Some(vec![]));
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn unseal_deadline_is_checked_after_waiting_for_the_writer() {
        use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction};

        let tempdir = tempfile::tempdir().unwrap();
        let storage =
            sqlite_storage(tempdir.path().join("deadline.db").to_string_lossy().into()).await;
        let state = sealed_state(AgentMode::Durable);
        let key = state.owned_agent_id.storage_key();
        storage
            .set_key_epoch(
                "test",
                "epoch",
                state.fence_namespace(),
                &key,
                ShardEpoch(2),
                &[],
                false,
            )
            .await
            .unwrap();
        let held = storage
            .pool
            .with_rw("test", "hold_writer")
            .begin()
            .await
            .unwrap();
        let deadline = SystemTime::now() + Duration::from_millis(25);
        let (result, ()) = tokio::join!(
            storage.unseal_agent(
                "test",
                "restore",
                &state,
                ShardEpoch(5),
                ShardEpoch(4),
                deadline,
                sealed_fingerprint(),
            ),
            async move {
                tokio::time::sleep(Duration::from_millis(75)).await;
                held.rollback().await.unwrap();
            }
        );
        assert_eq!(result.unwrap(), UnsealAgentResult::Expired);
        assert_eq!(
            storage.empty_database_epochs().await.unwrap(),
            Some(vec![(
                SqliteIndexedStorage::namespace(state.fence_namespace()),
                key,
                2
            )])
        );
    }

    #[test]
    async fn staged_publication_serializes_independent_sqlite_connections() {
        let tempdir = tempfile::tempdir().unwrap();
        let database = tempdir
            .path()
            .join("indexed.db")
            .to_string_lossy()
            .into_owned();
        let mut stores = Vec::new();
        for _ in 0..8 {
            stores.push(sqlite_storage(database.clone()).await);
        }
        let agent = OwnedAgentId::new(
            EnvironmentId(uuid::Uuid::from_u128(1)),
            &AgentId::from_parts(7, 1),
        );
        let mode = AgentMode::Durable;
        let staged = IndexedStorageNamespace::StagedOpLog {
            owned_agent_id: agent.clone(),
            agent_mode: mode,
        };
        let visible = IndexedStorageNamespace::OpLog {
            owned_agent_id: agent.clone(),
            agent_mode: mode,
        };
        for same_target in [true, false] {
            let mut requests = Vec::new();
            for (index, store) in stores.iter().enumerate() {
                let owned = OwnedAgentId::new(
                    EnvironmentId(uuid::Uuid::new_v4()),
                    &AgentId::from_parts(7, 1),
                );
                let stage_id = uuid::Uuid::new_v4();
                let token = store
                    .reserve_stage("test", "reserve", &owned, index as i64)
                    .await
                    .unwrap()
                    .unwrap();
                let stage = format!("{}#{stage_id}", owned.storage_key());
                let target = format!(
                    "target-{same_target}-{}",
                    if same_target { 0 } else { index }
                );
                store
                    .append(
                        "test",
                        "stage",
                        "entry",
                        staged.clone(),
                        &stage,
                        1,
                        vec![index as u8],
                        None,
                        Some(token),
                    )
                    .await
                    .unwrap();
                requests.push((store, stage, target, token));
            }
            let results =
                futures::future::join_all(requests.iter().map(|(store, stage, target, token)| {
                    store.move_if_absent(
                        "test",
                        "publish",
                        staged.clone(),
                        stage,
                        visible.clone(),
                        target,
                        1,
                        Some(*token),
                    )
                }))
                .await;
            let mut winners = 0;
            for (index, (result, (store, stage, target, _))) in
                results.into_iter().zip(&requests).enumerate()
            {
                if result.unwrap() {
                    winners += 1;
                    assert_eq!(
                        store
                            .read("test", "read", "entry", visible.clone(), target, 1, 1)
                            .await
                            .unwrap(),
                        vec![(1, vec![index as u8])]
                    );
                } else {
                    assert_eq!(
                        store
                            .read("test", "read", "entry", staged.clone(), stage, 1, 1)
                            .await
                            .unwrap(),
                        vec![(1, vec![index as u8])]
                    );
                }
            }
            assert_eq!(winners, if same_target { 1 } else { stores.len() });
        }
    }

    #[test]
    async fn append_many_writes_the_complete_batch() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(
            tempdir
                .path()
                .join("indexed.db")
                .to_string_lossy()
                .into_owned(),
        )
        .await;
        let namespace = oplog_namespace("sqlite-batch");

        storage
            .append_many(
                "test",
                "append_many",
                "entry",
                &namespace,
                "oplog",
                vec![
                    (1, Bytes::from_static(b"first")),
                    (2, Bytes::from_static(b"second")),
                ]
                .into(),
                None,
                None,
            )
            .await
            .unwrap();

        let mut actual = storage
            .read("test", "read", "entry", namespace, "oplog", 1, 2)
            .await
            .unwrap();
        actual.sort_unstable_by_key(|(id, _)| *id);
        assert_eq!(
            actual,
            vec![(1, b"first".to_vec()), (2, b"second".to_vec())]
        );
    }

    #[test]
    async fn append_many_rolls_back_the_batch_on_conflict() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(
            tempdir
                .path()
                .join("indexed.db")
                .to_string_lossy()
                .into_owned(),
        )
        .await;
        let namespace = oplog_namespace("sqlite-atomic-batch");

        storage
            .append(
                "test",
                "append",
                "entry",
                namespace.clone(),
                "oplog",
                2,
                b"existing".to_vec(),
                None,
                None,
            )
            .await
            .unwrap();

        let result = storage
            .append_many(
                "test",
                "append_many",
                "entry",
                &namespace,
                "oplog",
                vec![
                    (1, Bytes::from_static(b"must-roll-back")),
                    (2, Bytes::from_static(b"conflict")),
                ]
                .into(),
                None,
                None,
            )
            .await;

        assert!(matches!(result, Err(IndexedStorageError::Conflict(_))));
        assert_eq!(
            storage
                .read("test", "read", "entry", namespace, "oplog", 1, 2)
                .await
                .unwrap(),
            vec![(2, b"existing".to_vec())]
        );
    }

    #[test]
    async fn set_key_epoch_checks_explicit_presence_and_removes_absent_claims() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage =
            sqlite_storage(tempdir.path().join("presence.db").to_string_lossy().into()).await;
        let namespace = oplog_namespace("unrelated-owner");
        let extra = sealed_state(AgentMode::Ephemeral).manifest_namespace(99);
        let key = "portable-test-key";
        assert!(
            !storage
                .set_key_epoch(
                    "test",
                    "claim",
                    namespace.clone(),
                    key,
                    ShardEpoch(1),
                    &[],
                    true
                )
                .await
                .unwrap()
                .has_data
        );
        assert_eq!(storage.empty_database_epochs().await.unwrap(), Some(vec![]));
        assert!(
            !storage
                .set_key_epoch(
                    "test",
                    "claim",
                    namespace.clone(),
                    key,
                    ShardEpoch(2),
                    &[],
                    false
                )
                .await
                .unwrap()
                .has_data
        );
        assert_eq!(
            storage.empty_database_epochs().await.unwrap(),
            Some(vec![(
                SqliteIndexedStorage::namespace(namespace.clone()),
                key.into(),
                2
            )])
        );
        storage
            .append(
                "test",
                "append",
                "entry",
                extra.clone(),
                key,
                1,
                vec![1],
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            !storage
                .set_key_epoch(
                    "test",
                    "claim",
                    namespace.clone(),
                    key,
                    ShardEpoch(3),
                    &[],
                    true
                )
                .await
                .unwrap()
                .has_data
        );
        assert!(
            storage
                .set_key_epoch(
                    "test",
                    "claim",
                    namespace.clone(),
                    key,
                    ShardEpoch(4),
                    std::slice::from_ref(&extra),
                    true
                )
                .await
                .unwrap()
                .has_data
        );
        storage
            .delete("test", "delete", extra.clone(), key)
            .await
            .unwrap();
        storage
            .pool
            .with_rw("test", "marker")
            .execute(
                sqlx::query(
                    "INSERT INTO index_storage (namespace, key, id, value) VALUES (?, ?, 0, x'');",
                )
                .bind(format!(
                    "{}-present",
                    SqliteIndexedStorage::namespace(extra.clone())
                ))
                .bind(key),
            )
            .await
            .unwrap();
        assert!(
            storage
                .set_key_epoch(
                    "test",
                    "claim",
                    namespace.clone(),
                    key,
                    ShardEpoch(5),
                    std::slice::from_ref(&extra),
                    true
                )
                .await
                .unwrap()
                .has_data
        );
        assert!(matches!(
            storage
                .set_key_epoch(
                    "test",
                    "old_claim",
                    namespace.clone(),
                    key,
                    ShardEpoch(4),
                    &[],
                    true
                )
                .await,
            Err(IndexedStorageError::Fenced {
                actual: Some(ShardEpoch(5)),
                ..
            })
        ));
        storage
            .delete("test", "delete", extra.clone(), key)
            .await
            .unwrap();
        storage
            .append(
                "test",
                "append",
                "entry",
                namespace.clone(),
                key,
                1,
                vec![1],
                Some(ShardEpoch(5)),
                None,
            )
            .await
            .unwrap();
        assert!(
            storage
                .set_key_epoch(
                    "test",
                    "claim",
                    namespace,
                    key,
                    ShardEpoch(6),
                    &[extra],
                    true
                )
                .await
                .unwrap()
                .has_data
        );
    }

    #[test]
    // The column is `i64`-bound (see `to_i64`'s doc). An epoch that does not fit it must be
    // rejected here rather than silently wrapped to a negative value that a later `epoch as u64`
    // read turns into a spuriously huge one.
    async fn set_key_epoch_rejects_an_epoch_that_does_not_fit_i64() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(
            tempdir
                .path()
                .join("indexed.db")
                .to_string_lossy()
                .into_owned(),
        )
        .await;
        let namespace = oplog_namespace("sqlite-epoch-overflow");

        let result = storage
            .set_key_epoch(
                "test",
                "set_key_epoch",
                namespace,
                "oplog",
                ShardEpoch(u64::MAX),
                &[],
                false,
            )
            .await;

        assert!(
            matches!(result, Err(IndexedStorageError::Other(_))),
            "an epoch above i64::MAX must be a rejected write, not a wrapped negative one, got {result:?}"
        );
    }

    #[test]
    // The largest value that does fit is the boundary right below the rejected one, and must
    // still succeed - a regression here would mean the checked conversion rejects valid input.
    async fn set_key_epoch_accepts_the_largest_epoch_that_fits_i64() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(
            tempdir
                .path()
                .join("indexed.db")
                .to_string_lossy()
                .into_owned(),
        )
        .await;
        let namespace = oplog_namespace("sqlite-epoch-boundary");

        storage
            .set_key_epoch(
                "test",
                "set_key_epoch",
                namespace,
                "oplog",
                ShardEpoch(i64::MAX as u64),
                &[],
                false,
            )
            .await
            .unwrap();
    }

    #[test]
    async fn bounded_scan_merges_presence_and_data_without_duplicates_or_budget_overrun() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(tempdir.path().join("scan.db").to_string_lossy().into()).await;
        for (namespace, key, id) in [
            ("durable-worker-oplog", "component:a", 1_i64),
            ("durable-worker-oplog", "component:a", 2),
            ("durable-worker-oplog-present", "component:a", 0),
            ("durable-worker-oplog-present", "component:b", 0),
            ("durable-worker-oplog", "component:c", 1),
            ("durable-worker-oplog-present", "component:c", 0),
            ("durable-worker-oplog-present", "component:d", 0),
            ("durable-worker-oplog-present", "component;outside", 0),
            ("ephemeral-worker-oplog-present", "component:e", 0),
        ] {
            storage.pool.with_rw("test", "seed_scan").execute(sqlx::query("INSERT INTO index_storage (namespace, key, id, value) VALUES (?, ?, ?, x'');")
                .bind(namespace).bind(key).bind(id)).await.unwrap();
        }
        let mut resume = None;
        let mut keys = Vec::new();
        for page_index in 0..3 {
            let (next, page) = storage
                .scan_stable(
                    "test",
                    "scan",
                    IndexedStorageMetaNamespace::Oplog {
                        agent_mode: AgentMode::Durable,
                    },
                    Some("component:"),
                    resume,
                    2,
                )
                .await
                .unwrap();
            assert!(page.len() <= 2);
            assert_eq!(next.is_some(), page_index < 2);
            keys.extend(page);
            resume = next;
        }
        assert_eq!(
            keys,
            vec!["component:a", "component:b", "component:c", "component:d"]
        );
    }

    #[test]
    async fn bounded_scan_uses_binary_covering_index_range() {
        let tempdir = tempfile::tempdir().unwrap();
        let storage = sqlite_storage(
            tempdir
                .path()
                .join("indexed.db")
                .to_string_lossy()
                .into_owned(),
        )
        .await;
        let explain = format!("EXPLAIN QUERY PLAN {SCAN_INCLUSIVE_BOUNDED_QUERY}");
        let query = sqlx::query_as::<_, (i64, i64, i64, String)>(&explain)
            .bind("durable-worker-oplog")
            .bind("component:")
            .bind("component;")
            .bind("durable-worker-oplog-present")
            .bind("component:")
            .bind("component;")
            .bind(sqlx::types::Json(50_u64));
        let plan = storage
            .pool
            .with_ro("test", "scan_query_plan")
            .fetch_all_as(query)
            .await
            .unwrap();
        let details = plan
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(details.contains("COVERING INDEX"), "{details}");
        assert!(details.contains("namespace=?"), "{details}");
        assert!(
            details.contains("key>?") && details.contains("key<?"),
            "{details}"
        );
        assert!(!details.contains("TEMP B-TREE"), "{details}");

        let collation = sqlx::query_as::<_, (String,)>(
            "SELECT coll FROM pragma_index_xinfo('idx_key') WHERE name = 'key';",
        );
        assert_eq!(
            storage
                .pool
                .with_ro("test", "scan_index_collation")
                .fetch_one_as(collation)
                .await
                .unwrap()
                .0,
            "BINARY"
        );
    }

    #[test]
    async fn migration_rejects_non_utf8_database() {
        let tempdir = tempfile::tempdir().unwrap();
        let database = tempdir.path().join("utf16.db");
        let options = SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(true);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        connection
            .execute("PRAGMA encoding = 'UTF-16';")
            .await
            .unwrap();
        connection
            .execute("CREATE TABLE encoding_marker (value TEXT);")
            .await
            .unwrap();
        drop(connection);

        let result = SqliteIndexedStorage::migrate(&DbSqliteConfig {
            database: database.to_string_lossy().into_owned(),
            max_connections: 1,
            foreign_keys: false,
        })
        .await;
        assert!(result.is_err());
    }

    mod agent_transition_contracts {
        use super::*;
        use crate::storage::indexed::IndexedStorageLabelledApi;
        use crate::storage::indexed::memory::InMemoryIndexedStorage;
        use test_r::test;

        async fn backends(
            directory: &std::path::Path,
        ) -> Vec<(&'static str, Box<dyn IndexedStorage + Send + Sync>)> {
            vec![
                (
                    "sqlite",
                    Box::new(
                        sqlite_storage(directory.join("probe.db").to_string_lossy().into()).await,
                    ),
                ),
                ("memory", Box::new(InMemoryIndexedStorage::new())),
            ]
        }

        #[test]
        async fn agent_observation_ignores_epochs_and_covers_all_agent_data() {
            let tempdir = tempfile::tempdir().unwrap();
            for (backend, storage) in backends(tempdir.path()).await {
                let state = sealed_state(AgentMode::Ephemeral);
                let agent = &state.owned_agent_id;
                let key = agent.storage_key();
                let lookup = storage.with("test", "authority_lookup");
                assert!(
                    !lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}"
                );

                let other = OwnedAgentId::new(agent.environment_id, &AgentId::from_parts(7, 2));
                storage
                    .append(
                        "test",
                        "other_agent",
                        "entry",
                        IndexedStorageNamespace::OpLog {
                            owned_agent_id: other.clone(),
                            agent_mode: AgentMode::Durable,
                        },
                        &other.storage_key(),
                        1,
                        vec![1],
                        None,
                        None,
                    )
                    .await
                    .unwrap();
                assert!(
                    lookup.observe_agent(&other).await.unwrap().has_data,
                    "{backend}"
                );
                assert!(
                    !lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}"
                );

                let staged = IndexedStorageNamespace::StagedOpLog {
                    owned_agent_id: agent.clone(),
                    agent_mode: AgentMode::Durable,
                };
                let stage_key = format!("{key}#{}", uuid::Uuid::from_u128(42));
                for (namespace, epoch_key) in [
                    (state.fence_namespace(), key.as_str()),
                    (staged.clone(), stage_key.as_str()),
                ] {
                    storage
                        .set_key_epoch(
                            "test",
                            "bare_epoch",
                            namespace,
                            epoch_key,
                            ShardEpoch(5),
                            &[],
                            false,
                        )
                        .await
                        .unwrap();
                }
                assert!(
                    !lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}: bare epochs"
                );

                let token = storage
                    .reserve_stage("test", "reserve", agent, 42)
                    .await
                    .unwrap()
                    .unwrap();
                storage
                    .append(
                        "test",
                        "staged_row",
                        "entry",
                        staged.clone(),
                        &stage_key,
                        1,
                        vec![1],
                        Some(ShardEpoch(5)),
                        Some(token),
                    )
                    .await
                    .unwrap();
                assert!(
                    lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}: staged row in other mode"
                );
                storage
                    .discard_stage("test", "remove_stage", staged, &stage_key, Some(token))
                    .await
                    .unwrap();
                assert!(
                    !lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}"
                );

                storage
                    .mark_sealed(
                        "test",
                        "seal",
                        &state,
                        ShardEpoch(5),
                        sealed_fingerprint(),
                        None,
                    )
                    .await
                    .unwrap();
                assert!(
                    lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}: seal marker alone"
                );
                storage
                    .delete("test", "remove_marker", state.marker_namespace(), &key)
                    .await
                    .unwrap();
                assert!(
                    !lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}"
                );

                storage
                    .append(
                        "test",
                        "unconfigured_level",
                        "entry",
                        state.manifest_namespace(99),
                        &key,
                        1,
                        vec![1],
                        None,
                        None,
                    )
                    .await
                    .unwrap();
                assert!(
                    lookup.observe_agent(agent).await.unwrap().has_data,
                    "{backend}: unconfigured namespace"
                );
            }
        }

        #[test]
        async fn retained_deletion_checks_exact_target_epoch_on_first_attempt_and_retry() {
            let tempdir = tempfile::tempdir().unwrap();
            for (backend, storage) in backends(tempdir.path()).await {
                let state = sealed_state(AgentMode::Durable);
                let key = state.owned_agent_id.storage_key();
                let target = state.fence_namespace();
                let other = state.manifest_namespace(99);
                storage
                    .set_key_epoch(
                        "test",
                        "claim_target",
                        target.clone(),
                        &key,
                        ShardEpoch(5),
                        &[],
                        false,
                    )
                    .await
                    .unwrap();
                storage
                    .mark_sealed(
                        "test",
                        "mark",
                        &state,
                        ShardEpoch(5),
                        sealed_fingerprint(),
                        None,
                    )
                    .await
                    .unwrap();
                storage
                    .append(
                        "test",
                        "append",
                        "entry",
                        target.clone(),
                        &key,
                        1,
                        vec![1],
                        Some(ShardEpoch(5)),
                        None,
                    )
                    .await
                    .unwrap();
                storage
                    .set_key_epoch(
                        "test",
                        "claim_other",
                        other.clone(),
                        &key,
                        ShardEpoch(6),
                        &[],
                        false,
                    )
                    .await
                    .unwrap();
                for api in ["delete", "retry"] {
                    storage
                        .delete_with_epoch(
                            "test",
                            api,
                            target.clone(),
                            &key,
                            Some(ShardEpoch(5)),
                            true,
                        )
                        .await
                        .unwrap_or_else(|error| {
                            panic!("{backend}: exact target epoch must permit {api}: {error}")
                        });
                    assert!(
                        !storage
                            .exists("test", "exists", target.clone(), &key)
                            .await
                            .unwrap()
                    );
                    assert!(
                        !storage
                            .exists("test", "exists", state.marker_namespace(), &key)
                            .await
                            .unwrap()
                    );
                }
                assert!(
                    matches!(
                        storage
                            .set_key_epoch(
                                "test",
                                "check_other",
                                other,
                                &key,
                                ShardEpoch(5),
                                &[],
                                false
                            )
                            .await,
                        Err(IndexedStorageError::Fenced {
                            actual: Some(ShardEpoch(6)),
                            ..
                        })
                    ),
                    "{backend}: deletion must preserve other epochs"
                );
                storage
                    .append(
                        "test",
                        "append",
                        "entry",
                        target.clone(),
                        &key,
                        1,
                        vec![1],
                        Some(ShardEpoch(5)),
                        None,
                    )
                    .await
                    .unwrap();
                storage
                    .delete_with_epoch(
                        "test",
                        "remove_target",
                        target.clone(),
                        &key,
                        Some(ShardEpoch(5)),
                        false,
                    )
                    .await
                    .unwrap();
                assert!(
                    matches!(
                        storage
                            .delete_with_epoch(
                                "test",
                                "recover_missing",
                                target,
                                &key,
                                Some(ShardEpoch(5)),
                                true
                            )
                            .await,
                        Err(IndexedStorageError::Fenced {
                            actual: Some(ShardEpoch(6)),
                            ..
                        })
                    ),
                    "{backend}: missing-row recovery must respect other epochs"
                );
            }
        }

        #[test]
        async fn staged_epoch_collection_preserves_fence_when_agent_has_primary_data() {
            // Authority: remove_key_epoch_if_empty requires no data anywhere for the agent.
            let tempdir = tempfile::tempdir().unwrap();
            for (backend, storage) in backends(tempdir.path()).await {
                let state = sealed_state(AgentMode::Durable);
                let key = state.owned_agent_id.storage_key();
                let staged_key = format!("{key}#{}", uuid::Uuid::from_u128(123));
                let staged = IndexedStorageNamespace::StagedOpLog {
                    owned_agent_id: state.owned_agent_id.clone(),
                    agent_mode: state.agent_mode,
                };
                storage
                    .set_key_epoch(
                        "test",
                        "claim",
                        staged.clone(),
                        &staged_key,
                        ShardEpoch(5),
                        &[],
                        false,
                    )
                    .await
                    .unwrap();
                storage
                    .append(
                        "test",
                        "seed",
                        "entry",
                        state.primary_namespace(),
                        &key,
                        1,
                        vec![7],
                        None,
                        None,
                    )
                    .await
                    .unwrap();
                let removed = storage
                    .remove_key_epoch_if_empty(
                        "test",
                        "collect",
                        staged,
                        &staged_key,
                        ShardEpoch(5),
                    )
                    .await
                    .unwrap();
                assert!(
                    !removed,
                    "{backend}: agent data must prevent staged epoch collection"
                );
            }
        }

        #[test]
        async fn retained_staged_deletion_removes_agent_seal_marker() {
            // Authority: delete_with_epoch(retain_epoch=true) removes the seal marker atomically.
            let tempdir = tempfile::tempdir().unwrap();
            for (backend, storage) in backends(tempdir.path()).await {
                let state = sealed_state(AgentMode::Durable);
                let key = state.owned_agent_id.storage_key();
                let staged_key = format!("{key}#{}", uuid::Uuid::from_u128(123));
                let staged = IndexedStorageNamespace::StagedOpLog {
                    owned_agent_id: state.owned_agent_id.clone(),
                    agent_mode: state.agent_mode,
                };
                storage
                    .set_key_epoch(
                        "test",
                        "claim",
                        state.fence_namespace(),
                        &key,
                        ShardEpoch(5),
                        &[],
                        false,
                    )
                    .await
                    .unwrap();
                storage
                    .mark_sealed(
                        "test",
                        "mark",
                        &state,
                        ShardEpoch(5),
                        sealed_fingerprint(),
                        None,
                    )
                    .await
                    .unwrap();
                storage
                    .set_key_epoch(
                        "test",
                        "claim",
                        staged.clone(),
                        &staged_key,
                        ShardEpoch(5),
                        &[],
                        false,
                    )
                    .await
                    .unwrap();
                storage
                    .delete_with_epoch(
                        "test",
                        "delete",
                        staged,
                        &staged_key,
                        Some(ShardEpoch(5)),
                        true,
                    )
                    .await
                    .unwrap();
                assert!(
                    !storage
                        .exists("test", "exists", state.marker_namespace(), &key)
                        .await
                        .unwrap(),
                    "{backend}: retained staged deletion must remove the agent seal marker"
                );
            }
        }

        #[test]
        async fn unsealed_primary_is_visible_to_stable_scan() {
            // Authority: scan_stable returns a key present for the whole walk at least once.
            let tempdir = tempfile::tempdir().unwrap();
            for (backend, storage) in backends(tempdir.path()).await.into_iter().rev() {
                let state = sealed_state(AgentMode::Durable);
                let key = state.owned_agent_id.storage_key();
                assert_eq!(
                    storage
                        .unseal_agent(
                            "test",
                            "restore",
                            &state,
                            ShardEpoch(5),
                            ShardEpoch(5),
                            SystemTime::now() + Duration::from_secs(60),
                            sealed_fingerprint(),
                        )
                        .await
                        .unwrap(),
                    UnsealAgentResult::Unsealed
                );
                assert!(
                    storage
                        .exists("test", "exists", state.primary_namespace(), &key)
                        .await
                        .unwrap()
                );
                let mut resume = None;
                let mut keys = Vec::new();
                loop {
                    let (next, page) = storage
                        .scan_stable(
                            "test",
                            "scan",
                            IndexedStorageMetaNamespace::Oplog {
                                agent_mode: state.agent_mode,
                            },
                            None,
                            resume,
                            10,
                        )
                        .await
                        .unwrap();
                    keys.extend(page);
                    if next.is_none() {
                        break;
                    }
                    resume = next;
                }
                assert!(
                    keys.contains(&key),
                    "{backend}: the restored primary is present throughout the scan"
                );
            }
        }
    }
}
