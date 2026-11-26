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
    IndexedStorageMetaNamespace, IndexedStorageNamespace, ReleaseAgentResult, ScanResume,
    UnsealAgentResult,
};
use crate::services::golem_config::IndexedStoragePostgresConfig;
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use golem_common::SafeDisplay;
use golem_common::metrics::db::record_db_serialized_size;
use golem_common::model::agent::AgentMode;
use golem_common::model::{OwnedAgentId, ShardEpoch};
use golem_service_base::db::postgres::{PostgresLabelledTransaction, PostgresPool};
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use golem_service_base::repo::RepoError;
use include_dir::include_dir;
use sqlx::{Postgres, QueryBuilder};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Semaphore;
use uuid::Uuid;

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration/indexed");

const DB_TYPE: &str = "postgres";
// The two-i32 advisory key space is separate from the migrator's single-i64 key space.
const AGENT_LOCK_CLASS: i32 = 0x474f4c49;

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
pub struct PostgresIndexedStorage {
    pool: PostgresPool,
    drop_prefix_delete_batch_size: u64,
    semaphore: Option<Arc<Semaphore>>,
}

impl PostgresIndexedStorage {
    const APPEND_MANY_CHUNK_SIZE: usize = 1024;

    #[doc(hidden)]
    pub const OBSERVE_AGENT_QUERY: &str = "SELECT EXISTS(SELECT 1 FROM index_storage WHERE env = $1 AND agent = $2), (SELECT value FROM index_storage WHERE namespace = 'durable-worker-sealed-present' AND env = $1 AND agent = $2 AND id = 0), (SELECT value FROM index_storage WHERE namespace = 'ephemeral-worker-sealed-present' AND env = $1 AND agent = $2 AND id = 0);";
    #[doc(hidden)]
    pub const AGENT_ROWS_QUERY: &str =
        "SELECT namespace, id, value FROM index_storage WHERE env = $1 AND agent = $2;";
    #[doc(hidden)]
    pub const DELETE_AGENT_ROWS_QUERY: &str =
        "DELETE FROM index_storage WHERE env = $1 AND agent = $2;";

    pub async fn configured(config: &IndexedStoragePostgresConfig) -> Result<Self, String> {
        if config.drop_prefix_delete_batch_size == 0 {
            return Err(
                "Postgres indexed storage drop_prefix_delete_batch_size must be greater than 0"
                    .to_string(),
            );
        }

        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::postgres::migrate(
            &config.postgres,
            migrations.postgres_migrations(),
        )
        .await
        .map_err(|err| format!("Postgres indexed storage migration failed: {err:?}"))?;

        let pool = PostgresPool::configured(&config.postgres)
            .await
            .map_err(|err| {
                format!("Postgres indexed storage pool initialization failed: {err:?}")
            })?;

        let semaphore = {
            let max_ops = config
                .max_concurrent_ops
                .unwrap_or_else(|| config.postgres.max_connections.saturating_sub(2).max(1));
            Some(Arc::new(Semaphore::new(max_ops as usize)))
        };

        Ok(Self {
            pool,
            drop_prefix_delete_batch_size: config.drop_prefix_delete_batch_size,
            semaphore,
        })
    }

    pub async fn new(pool: PostgresPool) -> Result<Self, String> {
        Ok(Self {
            pool,
            drop_prefix_delete_batch_size: 1024,
            semaphore: None,
        })
    }

    pub async fn run_metrics_loop(&self) -> anyhow::Result<()> {
        self.pool.run_metrics_loop("indexed_storage").await
    }

    fn namespace(namespace: IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::SealedPresent { agent_mode, .. } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-sealed-present")
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

    fn key(namespace: &mut String, key: &str) -> Result<(Uuid, Uuid), IndexedStorageError> {
        let (key, stage) = match key.split_once('#') {
            Some((key, stage)) if namespace.ends_with("-worker-staged-oplog") => {
                let parsed = Uuid::parse_str(stage).map_err(|_| {
                    IndexedStorageError::Other("invalid Postgres staged oplog key".into())
                })?;
                if parsed.to_string() != stage {
                    return Err(IndexedStorageError::Other(
                        "noncanonical Postgres stage UUID".into(),
                    ));
                }
                (key, Some(parsed))
            }
            _ => (key, None),
        };
        let owned = golem_common::model::OwnedAgentId::from_storage_key(key).ok_or_else(|| {
            IndexedStorageError::Other(
                "Postgres indexed storage requires a canonical env/agent key".into(),
            )
        })?;
        if let Some(stage) = stage {
            namespace.push('#');
            namespace.push_str(&stage.to_string());
        }
        Ok((owned.environment_id.0, owned.agent_id.as_uuid()))
    }

    /// The first canonical key at or after an arbitrary text bound. Fixed separators and hex
    /// digits make the canonical domain ordered exactly like the pair of UUID columns.
    fn scan_bound(text: &str) -> Option<(Uuid, Uuid)> {
        const HEX: &[u8] = b"0123456789abcdef";
        const MIN: &[u8; 73] =
            b"00000000-0000-0000-0000-000000000000/00000000-0000-0000-0000-000000000000";
        let allowed = |i: usize| match MIN[i] {
            b'-' => &b"-"[..],
            b'/' => &b"/"[..],
            _ => HEX,
        };
        let mut key = *MIN;
        let input = text.as_bytes();
        let mut i = 0;
        while i < input.len().min(key.len()) {
            if let Some(&digit) = allowed(i).iter().find(|&&digit| digit >= input[i]) {
                key[i] = digit;
                if digit > input[i] {
                    break;
                }
                i += 1;
            } else {
                break;
            }
        }
        if (i < input.len().min(key.len()) && key[i] < input[i])
            || input.len() > key.len() && i == key.len()
        {
            loop {
                if i == 0 {
                    return None;
                }
                i -= 1;
                if let Some(&next) = allowed(i).iter().find(|&&digit| digit > key[i]) {
                    key[i] = next;
                    key[i + 1..].copy_from_slice(&MIN[i + 1..]);
                    break;
                }
            }
        }
        let key = std::str::from_utf8(&key).expect("canonical ASCII key");
        Some((key[..36].parse().unwrap(), key[37..].parse().unwrap()))
    }

    fn to_i64(value: u64, field_name: &'static str) -> Result<i64, IndexedStorageError> {
        i64::try_from(value).map_err(|_| {
            IndexedStorageError::Other(format!(
                "Postgres indexed storage cannot represent {field_name}={value} as i64"
            ))
        })
    }

    /// A stored epoch that will not fit a `u64` is corruption, not a fence. `to_i64` refuses to
    /// write one, so a negative column value came from outside this code - and reading it back as
    /// `u64` would wrap it into a near-ceiling epoch that fences every writer out of the key.
    fn negative_epoch_message(value: i64, key: &str) -> String {
        format!("Postgres indexed storage read a negative epoch {value} for key '{key}'")
    }

    /// The writer generation recorded for `key`, read inside `tx`, and holding its row until `tx` ends: a writer recording a new generation for the key waits
    /// for this transaction.
    async fn stored_epoch(
        tx: &mut PostgresLabelledTransaction,
        namespace: &str,
        env: Uuid,
        agent: Uuid,
    ) -> Result<Option<i64>, RepoError> {
        let stored: Option<(i64,)> = tx
            .fetch_optional_as(
                sqlx::query_as(
                    "SELECT epoch FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3 FOR UPDATE;",
                )
                .bind(namespace.to_string())
                .bind(env)
                .bind(agent),
            )
            .await?;
        Ok(stored.map(|(epoch,)| epoch))
    }

    /// Refuses the write `tx` makes unless `expected` is the generation recorded for `key`.
    async fn check_epoch(
        tx: &mut PostgresLabelledTransaction,
        namespace: &str,
        key: &str,
        env: Uuid,
        agent: Uuid,
        expected: ShardEpoch,
    ) -> Result<(), FencedTxError> {
        let stored = Self::stored_epoch(tx, namespace, env, agent).await?;
        FencedTxError::check_record(key, expected, stored, Self::negative_epoch_message)
    }

    fn classify_repo_error(err: RepoError, primary_oplog_insert: bool) -> IndexedStorageError {
        let timed_out = matches!(&err, RepoError::InternalError(error) if error
            .downcast_ref::<sqlx::Error>()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|error| error.code())
            .is_some_and(|code| matches!(code.as_ref(), "55P03" | "57014")));
        if timed_out || primary_oplog_insert && err.is_pool_timeout() {
            IndexedStorageError::Transient(err.to_string())
        } else if primary_oplog_insert && err.is_transient() {
            IndexedStorageError::Indeterminate(err.to_string())
        } else if primary_oplog_insert && err.is_unique_violation() {
            IndexedStorageError::Conflict(format!(
                "possible shard ownership mismatch while writing oplog: {}",
                err.to_safe_string()
            ))
        } else if err.is_transient() {
            IndexedStorageError::Transient(err.to_string())
        } else {
            IndexedStorageError::Other(err.to_safe_string())
        }
    }

    fn classify_repo_error_general(err: RepoError) -> IndexedStorageError {
        Self::classify_repo_error(err, false)
    }

    /// The oplog-insert classifier as a plain `fn`, so it can be handed to
    /// [`FencedTxError::into_indexed_storage_error`], which takes a function pointer.
    fn classify_repo_error_oplog_insert(err: RepoError) -> IndexedStorageError {
        Self::classify_repo_error(err, true)
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
            Self::classify_repo_error(err, false)
        }
    }

    async fn acquire_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        match &self.semaphore {
            Some(sem) => Some(sem.clone().acquire_owned().await.expect("semaphore closed")),
            None => None,
        }
    }

    /// FNV-1a over the two UUIDs in their canonical byte order. Collisions only serialize agents.
    fn agent_lock_key(env: Uuid, agent: Uuid) -> i32 {
        let hash = env
            .as_bytes()
            .iter()
            .chain(agent.as_bytes())
            .fold(0x811c9dc5u32, |hash, byte| {
                (hash ^ u32::from(*byte)).wrapping_mul(0x01000193)
            });
        i32::from_be_bytes(hash.to_be_bytes())
    }

    async fn prepare_agent_transaction(
        tx: &mut PostgresLabelledTransaction,
    ) -> Result<(), RepoError> {
        tx.execute(sqlx::query(
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED;",
        ))
        .await?;
        tx.execute(sqlx::query("SET LOCAL lock_timeout = '1s';"))
            .await?;
        Ok(())
    }

    /// Acquire separately from the state queries so their READ COMMITTED snapshots follow waits.
    async fn lock_agent_key(
        tx: &mut PostgresLabelledTransaction,
        key: i32,
    ) -> Result<(), RepoError> {
        tx.execute(
            sqlx::query("SELECT pg_advisory_xact_lock($1, $2);")
                .bind(AGENT_LOCK_CLASS)
                .bind(key),
        )
        .await?;
        Ok(())
    }

    async fn lock_agent(
        tx: &mut PostgresLabelledTransaction,
        env: Uuid,
        agent: Uuid,
    ) -> Result<(), RepoError> {
        Self::prepare_agent_transaction(tx).await?;
        Self::lock_agent_key(tx, Self::agent_lock_key(env, agent)).await
    }

    async fn locked_agent_epochs(
        tx: &mut PostgresLabelledTransaction,
        env: Uuid,
        agent: Uuid,
    ) -> Result<Vec<(String, i64)>, RepoError> {
        tx.fetch_all_as(
            sqlx::query_as(
                "SELECT namespace, epoch FROM indexed_key_epoch WHERE env = $1 AND agent = $2 AND (namespace LIKE 'durable-worker-%' OR namespace LIKE 'ephemeral-worker-%') ORDER BY namespace FOR UPDATE;",
            ).bind(env).bind(agent),
        ).await
    }

    fn check_agent_epochs(
        rows: &[(String, i64)],
        key: &str,
        epoch: ShardEpoch,
    ) -> Result<(), FencedTxError> {
        for (_, stored) in rows {
            let actual = u64::try_from(*stored)
                .map_err(|_| FencedTxError::Corrupt(Self::negative_epoch_message(*stored, key)))?;
            if actual > epoch.0 {
                return Err(FencedTxError::Fenced {
                    key: key.to_string(),
                    expected: epoch,
                    actual: Some(ShardEpoch(actual)),
                });
            }
        }
        Ok(())
    }

    fn manifest_rows(
        state: &AgentIndexedState,
    ) -> Result<Vec<(String, i64, Vec<u8>)>, IndexedStorageError> {
        let mut rows = Vec::new();
        for (level, pairs) in &state.manifests {
            let namespace = Self::namespace(state.manifest_namespace(*level));
            for (id, value) in pairs.iter() {
                rows.push((namespace.clone(), Self::to_i64(*id, "id")?, value.to_vec()));
            }
        }
        rows.sort();
        Ok(rows)
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl IndexedStorage for PostgresIndexedStorage {
    fn supports_release(&self) -> bool {
        true
    }

    async fn reserve_stage(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
        _stage_id: Uuid,
        epoch: Option<ShardEpoch>,
    ) -> Result<Option<i64>, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let env = owned_agent_id.environment_id.0;
        let agent = owned_agent_id.agent_id.as_uuid();
        let key = owned_agent_id.storage_key();
        let primary = Self::namespace(IndexedStorageNamespace::OpLog {
            owned_agent_id: owned_agent_id.clone(),
            agent_mode: AgentMode::Durable,
        });
        let token = super::fresh_stage_token();
        let stored_epoch = epoch
            .map(|epoch| Self::to_i64(epoch.0, "epoch"))
            .transpose()?;
        self.pool.with_tx_err::<Option<i64>, FencedTxError, _>(svc_name, api_name, |tx| {
            async move {
                Self::lock_agent(tx, env, agent).await?;
                if let Some(stored_epoch) = stored_epoch {
                    let result = tx.execute(sqlx::query(
                        "INSERT INTO indexed_key_epoch (namespace, env, agent, epoch) VALUES ($1, $2, $3, $4) ON CONFLICT (namespace, env, agent) DO UPDATE SET epoch = EXCLUDED.epoch WHERE indexed_key_epoch.epoch <= EXCLUDED.epoch;"
                    ).bind(&primary).bind(env).bind(agent).bind(stored_epoch)).await?;
                    if result.rows_affected() == 0 {
                        let stored = Self::stored_epoch(tx, &primary, env, agent).await?;
                        FencedTxError::check_record(&key, epoch.unwrap(), stored, Self::negative_epoch_message)?;
                    }
                }
                // Resident fenced writes hold their epoch row, not the advisory lock.
                Self::locked_agent_epochs(tx, env, agent).await?;
                let (present,): (bool,) = tx.fetch_one_as(sqlx::query_as(
                    "SELECT EXISTS(SELECT 1 FROM index_storage WHERE env = $1 AND agent = $2);"
                ).bind(env).bind(agent)).await?;
                if present { return Ok(None); }
                tx.execute(sqlx::query("INSERT INTO indexed_key_epoch (namespace, env, agent, epoch) VALUES ($1, $2, $3, $4) ON CONFLICT (namespace, env, agent) DO UPDATE SET epoch = EXCLUDED.epoch;")
                    .bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent).bind(token)).await?;
                Ok(Some(token))
            }.boxed()
        }).await.map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error_general))
    }

    async fn remove_stage_reservation(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
        token: Option<i64>,
    ) -> Result<bool, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let env = owned_agent_id.environment_id.0;
        let agent = owned_agent_id.agent_id.as_uuid();
        self.pool.with_tx(svc_name, api_name, |tx| Box::pin(async move {
            Self::lock_agent(tx, env, agent).await?;
            let result = tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3 AND ($4::bigint IS NULL OR epoch = $4);")
                .bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent).bind(token)).await?;
            Ok(result.rows_affected() > 0)
        })).await.map_err(Self::classify_repo_error_general)
    }

    async fn observe_agent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<AgentDataObservation, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        self.pool
            .with_ro(svc_name, api_name)
            .fetch_one_as(
                sqlx::query_as::<_, (bool, Option<Vec<u8>>, Option<Vec<u8>>)>(
                    Self::OBSERVE_AGENT_QUERY,
                )
                .bind(owned_agent_id.environment_id.0)
                .bind(owned_agent_id.agent_id.as_uuid()),
            )
            .await
            .map_err(Self::classify_repo_error_general)
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
        let _permit = self.acquire_permit().await;
        Self::to_i64(epoch.0, "epoch")?;
        let key = state.owned_agent_id.storage_key();
        let mut fence = Self::namespace(state.fence_namespace());
        let (env, agent) = Self::key(&mut fence, &key)?;
        let marker = Self::namespace(state.marker_namespace());
        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                async move {
                    Self::lock_agent(tx, env, agent).await?;
                    Self::check_epoch(tx, &fence, &key, env, agent, epoch).await?;
                    let existing: Option<(Vec<u8>,)> = tx.fetch_optional_as(sqlx::query_as(
                        "SELECT value FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 AND id = 0;",
                    ).bind(&marker).bind(env).bind(agent)).await?;
                    let replacement = match existing {
                        Some((value,)) => super::SealMarker::decode(&value).and_then(|marker| marker.replacement(fingerprint, superseded_marker)).map_err(FencedTxError::Corrupt)?,
                        None => Some(super::SealMarker::fresh(fingerprint)),
                    };
                    if let Some(replacement) = replacement {
                        tx.execute(sqlx::query(
                            "INSERT INTO index_storage (namespace, env, agent, id, value) VALUES ($1, $2, $3, 0, $4) ON CONFLICT (namespace, env, agent, id) DO UPDATE SET value = EXCLUDED.value;",
                        ).bind(&marker).bind(env).bind(agent).bind(replacement.encode().map_err(FencedTxError::Corrupt)?)).await?;
                    }
                    Ok(())
                }.boxed()
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error_general))
    }

    async fn release_agent(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        state: &AgentIndexedState,
        epoch: ShardEpoch,
    ) -> Result<ReleaseAgentResult, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        Self::to_i64(epoch.0, "epoch")?;
        let key = state.owned_agent_id.storage_key();
        let mut fence = Self::namespace(state.fence_namespace());
        let (env, agent) = Self::key(&mut fence, &key)?;
        let expected = Self::manifest_rows(state)?;
        self.pool.with_tx_err::<ReleaseAgentResult, FencedTxError, _>(svc_name, api_name, |tx| {
            async move {
                Self::lock_agent(tx, env, agent).await?;
                let epochs = Self::locked_agent_epochs(tx, env, agent).await?;
                let rows: Vec<(String, i64, Vec<u8>)> = tx.fetch_all_as(
                    sqlx::query_as("SELECT namespace, id, value FROM index_storage WHERE env = $1 AND agent = $2 ORDER BY namespace, id;")
                        .bind(env).bind(agent)).await?;
                if rows.is_empty() && epochs.is_empty() {
                    tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent)).await?;
                    return Ok(ReleaseAgentResult::Released);
                }
                FencedTxError::check_record(&key, epoch,
                    epochs.iter().find(|(namespace, _)| namespace == &fence).map(|(_, epoch)| *epoch),
                    Self::negative_epoch_message)?;
                Self::check_agent_epochs(&epochs, &key, epoch)?;
                let mut manifests = Vec::new();
                for (namespace, id, value) in rows {
                    if namespace == "durable-worker-sealed-present" || namespace == "ephemeral-worker-sealed-present" {
                        super::SealMarker::decode(&value).map_err(FencedTxError::Corrupt)?;
                    }
                    // Presence rows and staged entries do not contain primary/compressed history.
                    if namespace == "durable-worker-oplog" || namespace == "ephemeral-worker-oplog"
                        || ((namespace.starts_with("durable-worker-c") || namespace.starts_with("ephemeral-worker-c"))
                            && namespace.ends_with("-oplog"))
                    {
                        return Ok(ReleaseAgentResult::NotReleased);
                    }
                    if (namespace.starts_with("durable-worker-b") || namespace.starts_with("ephemeral-worker-b"))
                        && namespace.ends_with("-oplog")
                    {
                        manifests.push((namespace, id, value));
                    }
                }
                manifests.sort();
                if manifests != expected {
                    return Ok(ReleaseAgentResult::NotReleased);
                }
                tx.execute(sqlx::query(Self::DELETE_AGENT_ROWS_QUERY)
                    .bind(env).bind(agent)).await?;
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE env = $1 AND agent = $2 AND (namespace LIKE 'durable-worker-%' OR namespace LIKE 'ephemeral-worker-%');")
                    .bind(env).bind(agent)).await?;
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent)).await?;
                Ok(ReleaseAgentResult::Released)
            }.boxed()
        }).await.map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error_general))
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
        let _permit = self.acquire_permit().await;
        let epoch_i64 = Self::to_i64(epoch.0, "epoch")?;
        Self::to_i64(record_epoch.0, "record_epoch")?;
        let write_epoch = Self::to_i64(epoch.max(record_epoch).0, "epoch")?;
        let key = state.owned_agent_id.storage_key();
        if epoch < record_epoch {
            return Err(IndexedStorageError::Fenced {
                key,
                expected: epoch,
                actual: Some(record_epoch),
            });
        }
        let mut primary = Self::namespace(state.primary_namespace());
        let (env, agent) = Self::key(&mut primary, &key)?;
        let marker = Self::namespace(state.marker_namespace());
        let manifests = Self::manifest_rows(state)?;
        let configured: Vec<String> = state
            .epoch_namespaces()
            .into_iter()
            .map(Self::namespace)
            .collect();
        let deadline: time::OffsetDateTime = valid_until.into();
        let result = self.pool.with_tx_err::<UnsealAgentResult, UnsealTxError, _>(svc_name, api_name, |tx| {
            async move {
                tx.execute(sqlx::query("SET LOCAL statement_timeout = '1s';")).await?;
                Self::lock_agent(tx, env, agent).await?;
                let epochs = Self::locked_agent_epochs(tx, env, agent).await?;
                Self::check_agent_epochs(&epochs, &key, epoch)?;
                let mut rows: Vec<(String, i64, Vec<u8>)> = tx.fetch_all_as(
                    sqlx::query_as(Self::AGENT_ROWS_QUERY)
                        .bind(env).bind(agent)).await?;
                if !rows.is_empty() {
                    let mut expected_rows = manifests.clone();
                    expected_rows.push((format!("{primary}-present"), 0, Vec::new()));
                    expected_rows.push((marker.clone(), 0, Vec::new()));
                    for (namespace, id, value) in &rows {
                        if namespace == "durable-worker-sealed-present" || namespace == "ephemeral-worker-sealed-present" {
                            let decoded = super::SealMarker::decode(value).map_err(FencedTxError::Corrupt)?;
                            if namespace == &marker && *id == 0 && decoded.fingerprint == fingerprint {
                                expected_rows.last_mut().unwrap().2 = value.clone();
                            }
                        }
                    }
                    rows.sort();
                    expected_rows.sort();
                    if rows == expected_rows && epochs.iter().all(|(_, stored)| *stored == epoch_i64)
                        && configured.iter().all(|namespace| epochs.iter().any(|(stored, _)| stored == namespace))
                    {
                        if Self::stored_epoch(tx, super::STAGE_RESERVATION_NAMESPACE, env, agent).await?.is_some() {
                            tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent)).await?;
                        }
                        return Ok(UnsealAgentResult::Unsealed);
                    }
                    return Ok(UnsealAgentResult::AlreadyPresent);
                }
                let existing: Vec<String> = epochs.into_iter().map(|(namespace, _)| namespace).collect();
                let missing: Vec<String> = configured.into_iter().filter(|namespace| !existing.contains(namespace)).collect();
                if !existing.is_empty() {
                    let updated = tx.execute(sqlx::query(
                        "UPDATE indexed_key_epoch SET epoch = $4 WHERE namespace = ANY($1::text[]) AND env = $2 AND agent = $3 AND clock_timestamp() < $5;",
                    ).bind(&existing).bind(env).bind(agent).bind(write_epoch).bind(deadline)).await?;
                    if updated.rows_affected() != existing.len() as u64 {
                        return Err(UnsealTxError::Expired);
                    }
                }
                if !missing.is_empty() {
                    // A namespace absent from the locked snapshot must not overwrite a row
                    // inserted after that snapshot. A unique violation aborts the transaction.
                    let inserted = tx.execute(sqlx::query(
                        "INSERT INTO indexed_key_epoch (namespace, env, agent, epoch) SELECT namespace, $2, $3, $4 FROM unnest($1::text[]) AS namespace WHERE clock_timestamp() < $5;",
                    ).bind(&missing).bind(env).bind(agent).bind(write_epoch).bind(deadline)).await?;
                    if inserted.rows_affected() != missing.len() as u64 {
                        return Err(UnsealTxError::Expired);
                    }
                }
                for (namespace, id, value) in manifests {
                    tx.execute(sqlx::query("INSERT INTO index_storage (namespace, env, agent, id, value) VALUES ($1, $2, $3, $4, $5);")
                        .bind(namespace).bind(env).bind(agent).bind(id).bind(value)).await?;
                }
                for (namespace, value) in [(format!("{primary}-present"), Vec::new()), (marker, super::SealMarker::fresh(fingerprint).encode().map_err(FencedTxError::Corrupt)?)] {
                    tx.execute(sqlx::query("INSERT INTO index_storage (namespace, env, agent, id, value) VALUES ($1, $2, $3, 0, $4);")
                        .bind(namespace).bind(env).bind(agent).bind(value)).await?;
                }
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent)).await?;
                let (in_time,): (bool,) = tx.fetch_one_as(sqlx::query_as("SELECT clock_timestamp() < $1;").bind(deadline)).await?;
                if !in_time {
                    return Err(UnsealTxError::Expired);
                }
                Ok(UnsealAgentResult::Unsealed)
            }.boxed()
        }).await;
        match result {
            Ok(result) => Ok(result),
            Err(UnsealTxError::Expired) => Ok(UnsealAgentResult::Expired),
            Err(UnsealTxError::Storage(error)) => {
                Err(error.into_indexed_storage_error(Self::classify_repo_error_general))
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
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let epoch = Self::to_i64(epoch.0, "epoch")?;
        self.pool.with_tx(svc_name, api_name, |tx| {
            async move {
                Self::lock_agent(tx, env, agent).await?;
                Self::locked_agent_epochs(tx, env, agent).await?;
                let deleted = tx.execute(sqlx::query(
                    "DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3 AND epoch = $4 AND NOT EXISTS (SELECT 1 FROM index_storage WHERE env = $2 AND agent = $3);",
                ).bind(namespace).bind(env).bind(agent).bind(epoch)).await?;
                Ok(deleted.rows_affected() != 0)
            }.boxed()
        }).await.map_err(Self::classify_repo_error_general)
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
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let query = sqlx::query_as::<_, (bool,)>(
            "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace IN ($1, $2) AND env = $3 AND agent = $4);",
        )
        .bind(format!("{namespace}-present"))
        .bind(namespace)
        .bind(env)
        .bind(agent);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_one_as(query)
            .await
            .map(|row| row.0)
            .map_err(Self::classify_repo_error_general)
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
        let _permit = self.acquire_permit().await;
        let count_i64 = Self::to_i64(count, "count")?;
        let mut bounds = super::stable_scan_key_bounds(prefix, resume, "Postgres")?;
        let namespace = Self::meta_namespace(namespace);
        if !bounds.inclusive {
            bounds.lower.push('\0');
        }
        let Some(lower) = Self::scan_bound(&bounds.lower) else {
            return Ok((None, Vec::new()));
        };
        let upper = bounds.upper.as_deref().and_then(Self::scan_bound);
        if upper.is_some_and(|upper| lower >= upper) {
            return Ok((None, Vec::new()));
        }
        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT DISTINCT env, agent FROM index_storage WHERE namespace IN (",
        );
        query.push_bind(format!("{namespace}-present"));
        query.push(", ").push_bind(namespace).push(")");
        if let Some(upper) = upper.filter(|upper| upper.0 == lower.0) {
            query
                .push(" AND env = ")
                .push_bind(lower.0)
                .push(" AND agent >= ")
                .push_bind(lower.1)
                .push(" AND agent < ")
                .push_bind(upper.1);
        } else {
            query
                .push(" AND (env, agent) >= (")
                .push_bind(lower.0)
                .push(", ")
                .push_bind(lower.1)
                .push(")");
            if let Some(upper) = upper {
                query
                    .push(" AND (env, agent) < (")
                    .push_bind(upper.0)
                    .push(", ")
                    .push_bind(upper.1)
                    .push(")");
            }
        }
        query
            .push(" ORDER BY env, agent LIMIT ")
            .push_bind(count_i64);

        let keys = self
            .pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<(Uuid, Uuid), _>(query.build_query_as())
            .await
            .map(|keys| {
                keys.into_iter()
                    .map(|(env, agent)| format!("{env}/{agent}"))
                    .collect::<Vec<String>>()
            })
            .map_err(Self::classify_repo_error_general)?;

        Ok((super::last_key_resume(&keys, count), keys))
    }

    /// Delegates to [`Self::append_many`] so a single entry and a batch share the id validation,
    /// the permit and the epoch check. An entry that asserts an epoch is checked in the same
    /// transaction as its insert, like a batch; an unfenced insert takes the agent advisory lock.
    /// The permit is acquired there, not here.
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
        let staged = matches!(namespace, IndexedStorageNamespace::StagedOpLog { .. });
        let initial = matches!(namespace, IndexedStorageNamespace::OpLog { .. })
            && pairs.iter().any(|(id, _)| *id == 1);
        if pairs.is_empty() && !staged {
            return Ok(());
        }
        let _permit = self.acquire_permit().await;
        let classify: fn(RepoError) -> IndexedStorageError = match namespace {
            IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. } => {
                Self::classify_repo_error_oplog_insert
            }
            IndexedStorageNamespace::BlobOplogManifest { .. } => {
                Self::classify_repo_error_manifest_insert
            }
            IndexedStorageNamespace::CompressedOpLog { .. }
            | IndexedStorageNamespace::SealedPresent { .. } => Self::classify_repo_error_general,
        };
        let mut namespace = Self::namespace((*namespace).clone());
        let (env, agent) = Self::key(&mut namespace, key)?;
        let key = key.to_string();
        for (id, value) in pairs.iter() {
            record_db_serialized_size(DB_TYPE, svc_name, entity_name, value.len());
            Self::to_i64(*id, "id")?;
        }

        if expected_epoch.is_none() && !staged && !initial {
            return self
                .pool
                .with_tx(svc_name, api_name, |tx| {
                    async move {
                        Self::lock_agent(tx, env, agent).await?;
                        for chunk in pairs.chunks(Self::APPEND_MANY_CHUNK_SIZE) {
                            let mut builder = QueryBuilder::<Postgres>::new(
                                "INSERT INTO index_storage (namespace, env, agent, id, value) ",
                            );
                            builder.push_values(chunk.iter(), |mut row, (id, value)| {
                                row.push_bind(namespace.as_str())
                                    .push_bind(env)
                                    .push_bind(agent)
                                    .push_bind(i64::try_from(*id).expect("validated oplog index"))
                                    .push_bind(value.as_ref());
                            });
                            tx.execute(builder.build()).await?;
                        }
                        Ok(())
                    }
                    .boxed()
                })
                .await
                .map_err(classify);
        }

        if let Some(expected) = expected_epoch
            && pairs.len() <= Self::APPEND_MANY_CHUNK_SIZE
            && !staged
            && !initial
        {
            // The implicit transaction holds the epoch row through the insert and commit.
            // Materialize the locked row before filtering so refusals retain the stored epoch.
            let mut query_builder = QueryBuilder::<Postgres>::new(
                "WITH locked_epoch AS MATERIALIZED (SELECT epoch FROM indexed_key_epoch WHERE namespace = ",
            );
            query_builder
                .push_bind(namespace.as_str())
                .push(" AND env = ")
                .push_bind(env)
                .push(" AND agent = ")
                .push_bind(agent)
                .push(" FOR UPDATE), inserted AS (INSERT INTO index_storage (namespace, env, agent, id, value) SELECT ")
                .push_bind(namespace.as_str())
                .push(", ")
                .push_bind(env)
                .push(", ")
                .push_bind(agent)
                .push(", data.id, data.value FROM locked_epoch CROSS JOIN (");
            query_builder.push_values(pairs.iter(), |mut builder, (id, value)| {
                builder
                    .push_bind(i64::try_from(*id).expect("validated oplog index"))
                    .push_bind(value.as_ref());
            });
            query_builder
                .push(") AS data(id, value) WHERE locked_epoch.epoch = ")
                .push_bind(i64::try_from(expected.0).ok())
                .push(" RETURNING id) SELECT (SELECT epoch FROM locked_epoch);");

            // fetch_one drains through ReadyForQuery, including the implicit commit, rather
            // than acknowledging success when the first result row arrives.
            let (stored,): (Option<i64>,) = self
                .pool
                .with_rw(svc_name, api_name)
                .fetch_one_as(query_builder.build_query_as())
                .await
                .map_err(classify)?;
            return FencedTxError::check_record(
                &key,
                expected,
                stored,
                Self::negative_epoch_message,
            )
            .map_err(|err| err.into_indexed_storage_error(classify));
        }

        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                async move {
                    // Inside the insert transaction, and holding the row, so a writer that has
                    // lost the shard cannot slip a batch in between the check and the insert.
                    // `FOR UPDATE` is what serialises two executors racing over the same oplog.
                    if staged || initial {
                        Self::lock_agent(tx, env, agent).await?;
                    }
                    if staged {
                        let stored = Self::stored_epoch(tx, super::STAGE_RESERVATION_NAMESPACE, env, agent).await?;
                        FencedTxError::check_stage_token(&key, stage_token, stored)?;
                    }
                    if let Some(expected) = expected_epoch {
                        Self::check_epoch(tx, &namespace, &key, env, agent, expected).await?;
                    }

                    for chunk in pairs.chunks(Self::APPEND_MANY_CHUNK_SIZE) {
                        let mut query_builder = QueryBuilder::<Postgres>::new(
                            "INSERT INTO index_storage (namespace, env, agent, id, value) ",
                        );

                        query_builder.push_values(chunk.iter(), |mut builder, (id, value)| {
                            builder
                                .push_bind(namespace.as_str())
                                .push_bind(env)
                                .push_bind(agent)
                                .push_bind(i64::try_from(*id).expect("validated oplog index"))
                                .push_bind(value.as_ref());
                        });

                        let query = query_builder.build();
                        tx.execute(query).await?;
                    }

                    if initial {
                        tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent)).await?;
                    }
                    Ok(())
                }
                .boxed()
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(classify))
    }

    /// Postgres's half of [`IndexedStorage::set_key_epoch`], which states the rule this
    /// enforces.
    ///
    /// The `WHERE` on the conflict path is where it lives: `epoch <= EXCLUDED.epoch`, a higher
    /// generation or the one already recorded. With no record there is no conflict and any epoch is inserted. Postgres
    /// reports one row affected for an insert and for an accepted update, and zero when the `WHERE`
    /// excludes it - which is what the read-back below turns into a fence.
    /// The claim, presence snapshot and optional removal share one advisory-locked transaction.
    async fn set_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        new_epoch: ShardEpoch,
        presence_namespaces: &[IndexedStorageNamespace],
        remove_if_absent: bool,
    ) -> Result<bool, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let epoch = Self::to_i64(new_epoch.0, "epoch")?;
        let mut presence = vec![namespace.clone(), format!("{namespace}-present")];
        for candidate in presence_namespaces {
            let mut candidate = Self::namespace(candidate.clone());
            if candidate.ends_with("-worker-staged-oplog")
                && let Some((_, stage)) = namespace.split_once('#')
            {
                candidate.push('#');
                candidate.push_str(stage);
            }
            presence.push(format!("{candidate}-present"));
            presence.push(candidate);
        }
        let key = key.to_string();
        self.pool.with_tx_err::<bool, FencedTxError, _>(svc_name, api_name, |tx| {
            async move {
                Self::lock_agent(tx, env, agent).await?;
                let result = tx.execute(sqlx::query(
                    "INSERT INTO indexed_key_epoch (namespace, env, agent, epoch) VALUES ($1, $2, $3, $4) ON CONFLICT (namespace, env, agent) DO UPDATE SET epoch = EXCLUDED.epoch WHERE indexed_key_epoch.epoch <= EXCLUDED.epoch;",
                ).bind(&namespace).bind(env).bind(agent).bind(epoch)).await?;
                if result.rows_affected() == 0 {
                    let stored = Self::stored_epoch(tx, &namespace, env, agent).await?;
                    FencedTxError::check_record(&key, new_epoch, stored, Self::negative_epoch_message)?;
                }
                // Stabilize fenced writes in every explicitly observed namespace before taking
                // the presence snapshot; unfenced writes and new claims hold the advisory lock.
                tx.fetch_all_as::<(String,), _>(sqlx::query_as(
                    "SELECT namespace FROM indexed_key_epoch WHERE env = $1 AND agent = $2 AND namespace = ANY($3) ORDER BY namespace FOR UPDATE;",
                ).bind(env).bind(agent).bind(&presence)).await?;
                let (present,): (bool,) = tx.fetch_one_as(sqlx::query_as(
                    "SELECT EXISTS(SELECT 1 FROM index_storage WHERE env = $1 AND agent = $2 AND namespace = ANY($3));",
                ).bind(env).bind(agent).bind(&presence)).await?;
                if !present && remove_if_absent {
                    tx.execute(sqlx::query(
                        "DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3 AND epoch = $4;",
                    ).bind(namespace).bind(env).bind(agent).bind(epoch)).await?;
                }
                Ok(present)
            }.boxed()
        }).await.map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error_general))
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
        let _permit = self.acquire_permit().await;
        if retain_epoch && expected_epoch.is_none() {
            return Err(IndexedStorageError::Other(
                "retaining an epoch requires an asserted epoch".to_string(),
            ));
        }
        let retained_epoch = if retain_epoch {
            Some(Self::to_i64(
                expected_epoch.expect("asserted epoch").0,
                "epoch",
            )?)
        } else {
            None
        };
        let marker = match &namespace {
            IndexedStorageNamespace::SealedPresent { agent_mode, .. }
            | IndexedStorageNamespace::OpLog { agent_mode, .. }
            | IndexedStorageNamespace::StagedOpLog { agent_mode, .. }
            | IndexedStorageNamespace::CompressedOpLog { agent_mode, .. }
            | IndexedStorageNamespace::BlobOplogManifest { agent_mode, .. } => {
                format!(
                    "{}-worker-sealed-present",
                    super::agent_mode_prefix(*agent_mode)
                )
            }
        };
        let staged = matches!(&namespace, IndexedStorageNamespace::StagedOpLog { .. });
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let key = key.to_string();
        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                async move {
                    if expected_epoch.is_none() || staged {
                        Self::lock_agent(tx, env, agent).await?;
                    } else if retain_epoch {
                        Self::prepare_agent_transaction(tx).await?;
                    }
                    // Holding the row, as an append does: a writer taking the key over waits for
                    // this transaction, and then finds nothing left to take over.
                    if let Some(expected) = expected_epoch {
                        let mut stored = Self::stored_epoch(tx, &namespace, env, agent).await?;
                        if stored.is_none() && retain_epoch {
                            // No row lock is held here. Acquire the advisory lock before retrying
                            // the read, matching claims and unseal without reversing lock order.
                            Self::lock_agent_key(tx, Self::agent_lock_key(env, agent)).await?;
                            stored = Self::stored_epoch(tx, &namespace, env, agent).await?;
                            let epochs = Self::locked_agent_epochs(tx, env, agent).await?;
                            Self::check_agent_epochs(&epochs, &key, expected)?;
                        }
                        // Neither a record nor entries: the key is already gone, most often taken
                        // by an earlier attempt of this same deletion whose later steps failed, and
                        // a writer that lost the key has nothing here to destroy. Refusing would
                        // leave that deletion unable to finish.
                        if stored.is_none() {
                            let (has_entries,): (bool,) = tx
                                .fetch_one_as(
                                    sqlx::query_as(
                                        "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace IN ($1, $2) AND env = $3 AND agent = $4);",
                                    )
                                    .bind(format!("{namespace}-present"))
                                    .bind(namespace.clone())
                                    .bind(env)
                                    .bind(agent),
                                )
                                .await?;
                            if !has_entries && !retain_epoch {
                                return Ok(());
                            }
                            if !has_entries && retain_epoch {
                                tx.execute(sqlx::query(
                                    "INSERT INTO indexed_key_epoch (namespace, env, agent, epoch) VALUES ($1, $2, $3, $4);",
                                ).bind(&namespace).bind(env).bind(agent).bind(retained_epoch.expect("retained epoch"))).await?;
                            } else {
                                FencedTxError::check_record(
                                    &key, expected, stored, Self::negative_epoch_message,
                                )?;
                            }
                        } else {
                            FencedTxError::check_record(
                                &key, expected, stored, Self::negative_epoch_message,
                            )?;
                        }
                    }
                    tx.execute(
                        sqlx::query(
                            "DELETE FROM index_storage WHERE namespace IN ($1, $2) AND env = $3 AND agent = $4;",
                        )
                        .bind(format!("{namespace}-present"))
                        .bind(namespace.clone())
                        .bind(env)
                        .bind(agent),
                    )
                    .await?;
                    if retain_epoch {
                        tx.execute(sqlx::query(
                            "DELETE FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3;",
                        ).bind(marker).bind(env).bind(agent)).await?;
                    } else {
                        tx.execute(
                            sqlx::query(
                                "DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;",
                            )
                            .bind(namespace)
                            .bind(env)
                            .bind(agent),
                        )
                        .await?;
                    }
                    Ok(())
                }
                .boxed()
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error_general))
    }

    async fn delete_empty_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<bool, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let key = key.to_string();
        self.pool
            .with_tx_err::<bool, FencedTxError, _>(svc_name, api_name, |tx| {
                async move {
                    // Holding the row, as an append does: a writer adding entries waits for this
                    // transaction, so the emptiness seen below is the emptiness deleted.
                    if let Some(expected) = expected_epoch {
                        Self::check_epoch(tx, &namespace, &key, env, agent, expected).await?;
                    } else {
                        Self::lock_agent(tx, env, agent).await?;
                    }
                    let (has_entries,): (bool,) = tx
                        .fetch_one_as(
                            sqlx::query_as(
                                "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3);",
                            )
                            .bind(namespace.clone())
                            .bind(env)
                            .bind(agent),
                        )
                        .await?;
                    if has_entries {
                        return Ok(false);
                    }
                    tx.execute(
                        sqlx::query("DELETE FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3;")
                            .bind(format!("{namespace}-present"))
                            .bind(env)
                            .bind(agent),
                    )
                    .await?;
                    Ok(true)
                }
                .boxed()
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error_general))
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
        let expected = Self::to_i64(expected_last_id, "expected_last_id")?;
        let staged = matches!(
            &source_namespace,
            IndexedStorageNamespace::StagedOpLog { .. }
        );
        let mut source_namespace = Self::namespace(source_namespace);
        let mut target_namespace = Self::namespace(target_namespace);
        let (source_env, source_agent) = Self::key(&mut source_namespace, source_key)?;
        let (target_env, target_agent) = Self::key(&mut target_namespace, target_key)?;
        let _permit = self.acquire_permit().await;
        let source_key = source_key.to_string();
        let result = self.pool.with_tx_err::<bool, FencedTxError, _>(svc_name, api_name, |tx| async move {
            Self::prepare_agent_transaction(tx).await?;
            let mut locks = vec![Self::agent_lock_key(source_env, source_agent), Self::agent_lock_key(target_env, target_agent)];
            locks.sort_unstable();
            locks.dedup();
            for key in locks {
                Self::lock_agent_key(tx, key).await?;
            }
            if staged {
                let stored = Self::stored_epoch(tx, super::STAGE_RESERVATION_NAMESPACE, source_env, source_agent).await?;
                FencedTxError::check_stage_token(&source_key, stage_token, stored)?;
            }
            let target_exists: (bool,) = tx.fetch_one_as(
                sqlx::query_as("SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace IN ($1, $2) AND env = $3 AND agent = $4);")
                    .bind(&target_namespace).bind(format!("{target_namespace}-present")).bind(target_env).bind(target_agent)).await?;
            if target_exists.0 { return Ok(false); }
            let source: (i64, Option<i64>, Option<i64>) = tx.fetch_one_as(
                sqlx::query_as("SELECT COUNT(*), MIN(id), MAX(id) FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3;")
                    .bind(&source_namespace).bind(source_env).bind(source_agent)).await?;
            if source != (expected, Some(1), Some(expected)) {
                return Err(FencedTxError::Corrupt("source index is missing, empty, gapped, or has an unexpected tip".into()));
            }
            tx.execute(sqlx::query(
                "INSERT INTO index_storage (namespace, env, agent, id, value) VALUES ($1, $2, $3, 0, ''::bytea);")
                .bind(format!("{target_namespace}-present")).bind(target_env).bind(target_agent)).await?;
            tx.execute(sqlx::query(
                "UPDATE index_storage SET namespace = $1, env = $2, agent = $3 WHERE namespace = $4 AND env = $5 AND agent = $6;")
                .bind(&target_namespace).bind(target_env).bind(target_agent).bind(&source_namespace).bind(source_env).bind(source_agent)).await?;
            tx.execute(sqlx::query("DELETE FROM index_storage WHERE namespace IN ($1, $2) AND env = $3 AND agent = $4;")
                .bind(&source_namespace).bind(format!("{source_namespace}-present")).bind(source_env).bind(source_agent)).await?;
            if staged {
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;").bind(super::STAGE_RESERVATION_NAMESPACE).bind(source_env).bind(source_agent)).await?;
            }
            Ok(true)
        }.boxed()).await;
        match result {
            Err(FencedTxError::Repo(err)) if err.is_unique_violation() => Ok(false),
            result => result.map_err(|err| {
                err.into_indexed_storage_error(Self::classify_repo_error_oplog_insert)
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
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let query = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3;",
        )
        .bind(namespace)
        .bind(env)
        .bind(agent);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_one_as(query)
            .await
            .map(|row| row.0 as u64)
            .map_err(Self::classify_repo_error_general)
    }

    async fn discard_stage(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        self.pool.with_tx(svc_name, api_name, |tx| async move {
            Self::lock_agent(tx, env, agent).await?;
            let stored = Self::stored_epoch(tx, super::STAGE_RESERVATION_NAMESPACE, env, agent).await?;
            if stage_token.is_some() && stored == stage_token {
                tx.execute(sqlx::query("DELETE FROM index_storage WHERE namespace IN ($1, $2) AND env = $3 AND agent = $4;")
                    .bind(format!("{namespace}-present")).bind(namespace).bind(env).bind(agent)).await?;
                tx.execute(sqlx::query("DELETE FROM indexed_key_epoch WHERE namespace = $1 AND env = $2 AND agent = $3;")
                    .bind(super::STAGE_RESERVATION_NAMESPACE).bind(env).bind(agent)).await?;
            }
            Ok(())
        }.boxed()).await.map_err(Self::classify_repo_error_general)
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        self.pool
            .with_tx(svc_name, api_name, |tx| {
                async move {
                    Self::lock_agent(tx, env, agent).await?;
                    tx.execute(sqlx::query(
                        "DELETE FROM index_storage WHERE namespace IN ($1, $2) AND env = $3 AND agent = $4;",
                    ).bind(format!("{namespace}-present")).bind(namespace).bind(env).bind(agent)).await?;
                    Ok(())
                }.boxed()
            })
            .await
            .map_err(Self::classify_repo_error_general)
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
        let _permit = self.acquire_permit().await;
        let start_id = Self::to_i64(start_id, "start_id")?;
        let end_id = Self::to_i64(end_id, "end_id")?;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 AND id BETWEEN $4 AND $5 ORDER BY id ASC;",
        )
        .bind(namespace)
        .bind(env)
        .bind(agent)
        .bind(start_id)
        .bind(end_id);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<DBIdValue, _>(query)
            .await
            .map(|vec| vec.into_iter().map(|row| row.into_pair()).collect())
            .map_err(Self::classify_repo_error_general)
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 ORDER BY id ASC LIMIT 1;",
        )
        .bind(namespace)
        .bind(env)
        .bind(agent);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<DBIdValue, _>(query)
            .await
            .map(|op| op.map(|row| row.into_pair()))
            .map_err(Self::classify_repo_error_general)
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 ORDER BY id DESC LIMIT 1;",
        )
        .bind(namespace)
        .bind(env)
        .bind(agent);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<DBIdValue, _>(query)
            .await
            .map(|op| op.map(|row| row.into_pair()))
            .map_err(Self::classify_repo_error_general)
    }

    async fn last_id(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        let _permit = self.acquire_permit().await;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let query = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 ORDER BY id DESC LIMIT 1;",
        )
        .bind(namespace)
        .bind(env)
        .bind(agent);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<(i64,), _>(query)
            .await
            .map(|op| op.map(|row| row.0 as u64))
            .map_err(Self::classify_repo_error_general)
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
        let _permit = self.acquire_permit().await;
        let id = Self::to_i64(id, "id")?;
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 AND id >= $4 ORDER BY id ASC LIMIT 1;",
        )
        .bind(namespace)
        .bind(env)
        .bind(agent)
        .bind(id);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<DBIdValue, _>(query)
            .await
            .map(|op| op.map(|row| row.into_pair()))
            .map_err(Self::classify_repo_error_general)
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
        let _permit = self.acquire_permit().await;
        let delete_if_empty = matches!(
            &namespace,
            IndexedStorageNamespace::CompressedOpLog { .. }
                | IndexedStorageNamespace::BlobOplogManifest { .. }
        );
        let staged = matches!(&namespace, IndexedStorageNamespace::StagedOpLog { .. });
        let mut namespace = Self::namespace(namespace);
        let (env, agent) = Self::key(&mut namespace, key)?;
        let key = key.to_string();
        let last_dropped_id = Self::to_i64(last_dropped_id, "last_dropped_id")?;
        let batch_size_i64 = Self::to_i64(
            self.drop_prefix_delete_batch_size,
            "drop_prefix_delete_batch_size",
        )?;
        let delete_batch_size = self.drop_prefix_delete_batch_size;
        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                async move {
                    // Holding the row, as an append does: the whole trim happens inside one
                    // ownership generation, so a stale executor's archive transfer cannot
                    // destroy entries the new owner accepted.
                    if staged && expected_epoch.is_some() {
                        Self::lock_agent(tx, env, agent).await?;
                    }
                    if let Some(expected) = expected_epoch {
                        Self::check_epoch(tx, &namespace, &key, env, agent, expected).await?;
                    } else {
                        Self::lock_agent(tx, env, agent).await?;
                    }
                    tx.execute(sqlx::query(
                        "INSERT INTO index_storage (namespace, env, agent, id, value) SELECT $1, $2, $3, 0, ''::bytea WHERE EXISTS (SELECT 1 FROM index_storage WHERE namespace = $4 AND env = $2 AND agent = $3) ON CONFLICT DO NOTHING;")
                        .bind(format!("{namespace}-present")).bind(env).bind(agent).bind(&namespace)).await?;
                    let mut deleted_rows = delete_batch_size;

                    while deleted_rows >= delete_batch_size {
                        let query = sqlx::query(
                            "WITH rows AS (SELECT ctid FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 AND id <= $4 ORDER BY id LIMIT $5) DELETE FROM index_storage t USING rows WHERE t.ctid = rows.ctid;",
                        )
                        .bind(&namespace)
                        .bind(env)
                        .bind(agent)
                        .bind(last_dropped_id)
                        .bind(batch_size_i64);

                        deleted_rows = tx.execute(query).await?.rows_affected();
                    }

                    if delete_if_empty {
                        tx.execute(
                            sqlx::query(
                                "DELETE FROM index_storage WHERE namespace = $1 AND env = $2 AND agent = $3 AND NOT EXISTS (SELECT 1 FROM index_storage WHERE namespace = $4 AND env = $2 AND agent = $3);",
                            )
                            .bind(format!("{namespace}-present"))
                            .bind(env)
                            .bind(agent)
                            .bind(&namespace),
                        )
                        .await?;
                    }

                    Ok(())
                }
                .boxed()
            })
            .await
            .map_err(|err| err.into_indexed_storage_error(Self::classify_repo_error_general))
    }
}

#[derive(sqlx::FromRow, Debug)]
struct DBIdValue {
    id: i64,
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
    use test_r::test;

    #[test]
    fn primary_oplog_insert_classifies_pool_timeout_as_safe_to_retry() {
        let error = RepoError::from(sqlx::Error::PoolTimedOut);

        assert!(matches!(
            PostgresIndexedStorage::classify_repo_error(error, true),
            IndexedStorageError::Transient(_)
        ));
    }

    #[test]
    fn primary_oplog_insert_classifies_io_as_indeterminate() {
        let error = RepoError::from(sqlx::Error::Io(std::io::Error::other("connection lost")));

        assert!(matches!(
            PostgresIndexedStorage::classify_repo_error(error, true),
            IndexedStorageError::Indeterminate(_)
        ));
    }

    #[test]
    fn primary_oplog_insert_classifies_unique_violation_as_conflict() {
        let error = RepoError::UniqueViolation("duplicate index".to_string());

        assert!(matches!(
            PostgresIndexedStorage::classify_repo_error(error, true),
            IndexedStorageError::Conflict(_)
        ));
    }

    #[test]
    fn read_classifies_io_as_retriable() {
        let error = RepoError::from(sqlx::Error::Io(std::io::Error::other("connection lost")));

        assert!(matches!(
            PostgresIndexedStorage::classify_repo_error(error, false),
            IndexedStorageError::Transient(_)
        ));
    }
}
