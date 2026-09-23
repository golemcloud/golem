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
    FencedTxError, IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace,
    IndexedStorageNamespace, ScanResume, WriterId,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use golem_common::SafeDisplay;
use golem_common::config::DbSqliteConfig;
use golem_common::metrics::db::record_db_serialized_size;
use golem_common::model::ShardEpoch;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use golem_service_base::repo::RepoError;
use include_dir::include_dir;
use std::sync::Arc;
use std::time::Duration;

const DB_TYPE: &str = "sqlite";
const SCAN_INCLUSIVE_BOUNDED_QUERY: &str = "SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key >= ? AND key < ? ORDER BY key LIMIT ?;";
const SCAN_EXCLUSIVE_BOUNDED_QUERY: &str = "SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key > ? AND key < ? ORDER BY key LIMIT ?;";
const SCAN_INCLUSIVE_UNBOUNDED_QUERY: &str =
    "SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key >= ? ORDER BY key LIMIT ?;";
const SCAN_EXCLUSIVE_UNBOUNDED_QUERY: &str =
    "SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key > ? ORDER BY key LIMIT ?;";

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration/indexed");

#[derive(Debug, Clone)]
pub struct SqliteIndexedStorage {
    pool: SqlitePool,
    /// Recorded beside the epoch on every key this process claims, so an equal epoch from
    /// another process is refused rather than shared. One per process; see [`WriterId`].
    writer_id: WriterId,
}

impl SqliteIndexedStorage {
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
            writer_id: WriterId::process(),
        })
    }

    /// Writes as `writer_id` rather than as this process's own. The fan-out backend uses it to
    /// give every storage it opens one identity, and a test uses it to play two processes racing
    /// over one key inside a single process.
    pub fn for_writer(mut self, writer_id: WriterId) -> Self {
        self.writer_id = writer_id;
        self
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
            writer_id: WriterId::process(),
        }
    }

    fn namespace(namespace: IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog {
                agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-oplog")
            }
            IndexedStorageNamespace::StagedOpLog {
                agent_id: _,
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-staged-oplog")
            }
            IndexedStorageNamespace::CompressedOpLog {
                agent_id: _,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}-worker-c{level}-oplog")
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
        }
    }

    /// sqlx has no `Encode<Sqlite>` for `u64`, so a value that must stay integer-bound (rather
    /// than go through `Json`, which encodes as TEXT - see [`Self::set_key_epoch`]) has to
    /// cross to `i64` first. Checked, like Postgres's own `to_i64`: an unchecked `as i64` on a
    /// value above `i64::MAX` wraps to negative, and reading that back `as u64` produces a
    /// spuriously huge epoch instead of failing loudly.
    fn to_i64(value: u64, field_name: &'static str) -> Result<i64, IndexedStorageError> {
        i64::try_from(value).map_err(|_| {
            IndexedStorageError::Other(format!(
                "SQLite indexed storage cannot represent {field_name}={value} as i64"
            ))
        })
    }

    /// A stored epoch that will not fit a `u64` is corruption, not a fence: `to_i64` refuses to
    /// write one, so a negative column value came from outside this code, and reading it back as
    /// `u64` would wrap it into a spuriously huge epoch.
    fn negative_epoch_message(value: i64, key: &str) -> String {
        format!("SQLite indexed storage read a negative epoch {value} for key '{key}'")
    }

    fn classify_repo_error(err: RepoError) -> IndexedStorageError {
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
}

#[async_trait]
impl IndexedStorage for SqliteIndexedStorage {
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
                .bind(namespace)
                .bind(bounds.lower)
                .bind(upper)
                .bind(count_param),
            (false, Some(upper)) => sqlx::query_as(SCAN_EXCLUSIVE_BOUNDED_QUERY)
                .bind(namespace)
                .bind(bounds.lower)
                .bind(upper)
                .bind(count_param),
            (true, None) => sqlx::query_as(SCAN_INCLUSIVE_UNBOUNDED_QUERY)
                .bind(namespace)
                .bind(bounds.lower)
                .bind(count_param),
            (false, None) => sqlx::query_as(SCAN_EXCLUSIVE_UNBOUNDED_QUERY)
                .bind(namespace)
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
    ) -> Result<(), IndexedStorageError> {
        self.append_many(
            svc_name,
            api_name,
            entity_name,
            &namespace,
            key,
            vec![(id, Bytes::from(value))].into(),
            expected_epoch,
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
    ) -> Result<(), IndexedStorageError> {
        if pairs.is_empty() {
            return Ok(());
        }

        let primary_oplog_insert = matches!(
            namespace,
            IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. }
        );
        let namespace = Self::namespace((*namespace).clone());
        let key = key.to_string();
        for (_, value) in pairs.iter() {
            record_db_serialized_size(DB_TYPE, svc_name, entity_name, value.len());
        }

        let writer_id = self.writer_id.to_string();
        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                Box::pin(async move {
                    // SQLite has no `SELECT ... FOR UPDATE`, and it does not need one here: the
                    // write pool is capped at a single connection (golem-service-base
                    // db/sqlite.rs:46-50), so this transaction holds the only writer and the
                    // check cannot be interleaved. Raising that cap means switching this to
                    // `BEGIN IMMEDIATE`.
                    //
                    // That holds within one process. Two processes on one SQLite file would rely
                    // on SQLite's own lock upgrade, which surfaces a loser as `SQLITE_BUSY` - a
                    // storage error, not a fence - so a SQLite file shared between executors is
                    // not supported; give each its own file, or use PostgreSQL.
                    if let Some(expected) = expected_epoch {
                        let stored: Option<(i64, String)> = tx
                            .fetch_optional_as(
                                sqlx::query_as(
                                    "SELECT epoch, writer FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                                )
                                .bind(namespace.clone())
                                .bind(key.clone()),
                            )
                            .await?;
                        FencedTxError::check_record(
                            &key,
                            expected,
                            stored,
                            &writer_id,
                            Self::negative_epoch_message,
                        )?;
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

                    Ok(())
                })
            })
            .await
            .map_err(|err| {
                err.into_indexed_storage_error(if primary_oplog_insert {
                    Self::classify_repo_error_primary_oplog_insert
                } else {
                    Self::classify_repo_error
                })
            })
    }

    /// SQLite's half of [`IndexedStorage::set_key_epoch`], which states the rule this
    /// enforces. The unqualified `epoch`/`writer` in the `WHERE` are the existing row's, and
    /// `excluded` is the row being written.
    async fn set_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        new_epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        // `i64`, not `u64`: sqlx has no `Encode<Sqlite>` for `u64`, which is why ids elsewhere in
        // this file go through `Json`. That encodes as TEXT, and comparison affinity is applied
        // per operand, so this column stays integer-bound everywhere. Checked (see `to_i64`)
        // rather than `as i64`, which would silently wrap an out-of-range epoch to negative.
        let epoch = Self::to_i64(new_epoch.0, "epoch")?;

        let writer_id = self.writer_id.to_string();

        let mut api = self.pool.with_rw(svc_name, api_name);
        let result = api
            .execute(
                sqlx::query(
                    r#"INSERT INTO indexed_key_epoch (namespace, key, epoch, writer) VALUES (?, ?, ?, ?)
                       ON CONFLICT(namespace, key) DO UPDATE SET epoch = excluded.epoch, writer = excluded.writer
                       WHERE epoch < excluded.epoch
                          OR (epoch = excluded.epoch AND writer = excluded.writer);"#,
                )
                .bind(namespace.clone())
                .bind(key)
                .bind(epoch)
                .bind(writer_id.clone()),
            )
            .await
            .map_err(Self::classify_repo_error)?;

        if result.rows_affected() == 0 {
            let stored: Option<(i64, String)> = api
                .fetch_optional_as(
                    sqlx::query_as(
                        "SELECT epoch, writer FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                    )
                    .bind(namespace)
                    .bind(key),
                )
                .await
                .map_err(Self::classify_repo_error)?;
            let mut actual = None;
            let mut writer_matches = false;
            if let Some((epoch, writer)) = stored {
                let epoch = u64::try_from(epoch).map_err(|_| {
                    IndexedStorageError::Other(Self::negative_epoch_message(epoch, key))
                })?;
                actual = Some(ShardEpoch(epoch));
                writer_matches = writer == writer_id;
            }
            return Err(IndexedStorageError::Fenced {
                key: key.to_string(),
                expected: new_epoch,
                actual,
                writer_conflict: actual == Some(new_epoch) && !writer_matches,
            });
        }

        Ok(())
    }

    async fn delete_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        let key = key.to_string();
        let writer_id = self.writer_id.to_string();
        self.pool
            .with_tx_err::<(), FencedTxError, _>(svc_name, api_name, |tx| {
                Box::pin(async move {
                    // The single-connection write pool makes the check and the deletes one
                    // step, as it does for an append (see `append_many`).
                    if let Some(expected) = expected_epoch {
                        let stored: Option<(i64, String)> = tx
                            .fetch_optional_as(
                                sqlx::query_as(
                                    "SELECT epoch, writer FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                                )
                                .bind(namespace.clone())
                                .bind(key.clone()),
                            )
                            .await?;
                        FencedTxError::check_record(
                            &key,
                            expected,
                            stored,
                            &writer_id,
                            Self::negative_epoch_message,
                        )?;
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
                    tx.execute(
                        sqlx::query(
                            "DELETE FROM indexed_key_epoch WHERE namespace = ? AND key = ?;",
                        )
                        .bind(namespace)
                        .bind(key),
                    )
                    .await?;
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
    ) -> Result<bool, IndexedStorageError> {
        if expected_last_id == 0 {
            return Err(IndexedStorageError::Other(
                "source index expected tip must be greater than zero".to_string(),
            ));
        }
        let expected = i64::try_from(expected_last_id).map_err(|_| {
            IndexedStorageError::Other("source index tip exceeds storage range".into())
        })?;
        let source_namespace = Self::namespace(source_namespace);
        let target_namespace = Self::namespace(target_namespace);
        let source_key = source_key.to_string();
        let target_key = target_key.to_string();
        let result = self.pool
            .with_tx(svc_name, api_name, |tx| {
                async move {
                    // Acquire the write lock before reading the source. A read-first
                    // transaction cannot upgrade its snapshot after a concurrent write.
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
                        return Err(RepoError::InternalError(anyhow::anyhow!("source index is missing, empty, gapped, or has an unexpected tip")));
                    }
                    tx.execute(
                        sqlx::query("UPDATE index_storage SET namespace = ?, key = ? WHERE namespace = ? AND key = ?;")
                            .bind(&target_namespace)
                            .bind(&target_key)
                            .bind(&source_namespace)
                            .bind(&source_key),
                    ).await?;
                    tx.execute(sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ?;")
                        .bind(format!("{source_namespace}-present")).bind(&source_key)).await?;
                    Ok(true)
                }.boxed()
            })
            .await;
        match result {
            Err(err) if err.is_unique_violation() => Ok(false),
            result => result.map_err(Self::classify_repo_error_primary_oplog_insert),
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

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let namespace = Self::namespace(namespace);
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
    ) -> Result<(), IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        let key = key.to_string();
        self.pool
            .with_tx(svc_name, api_name, |tx| async move {
                tx.execute(sqlx::query(
                    "INSERT OR IGNORE INTO index_storage (namespace, key, id, value) SELECT ?, ?, 0, x'' WHERE EXISTS (SELECT 1 FROM index_storage WHERE namespace = ? AND key = ?);")
                    .bind(format!("{namespace}-present")).bind(&key).bind(&namespace).bind(&key)).await?;
                tx.execute(sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ? AND id <= ?;")
                    .bind(&namespace).bind(&key).bind(sqlx::types::Json(last_dropped_id))).await?;
                Ok(())
            }.boxed())
            .await
            .map_err(Self::classify_repo_error)
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
    use golem_common::model::AgentId;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{Connection, Executor};
    use test_r::test;

    fn oplog_namespace(agent_id: &str) -> IndexedStorageNamespace {
        IndexedStorageNamespace::OpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: agent_id.to_string(),
            },
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
        let agent = AgentId {
            component_id: ComponentId::new(),
            agent_id: "publication-contention".into(),
        };
        let mode = AgentMode::Durable;
        let staged = IndexedStorageNamespace::StagedOpLog {
            agent_id: agent.clone(),
            agent_mode: mode,
        };
        let visible = IndexedStorageNamespace::OpLog {
            agent_id: agent.clone(),
            agent_mode: mode,
        };
        for same_target in [true, false] {
            let mut requests = Vec::new();
            for (index, store) in stores.iter().enumerate() {
                let stage = format!("stage-{same_target}-{index}");
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
                    )
                    .await
                    .unwrap();
                requests.push((store, stage, target));
            }
            let results =
                futures::future::join_all(requests.iter().map(|(store, stage, target)| {
                    store.move_if_absent(
                        "test",
                        "publish",
                        staged.clone(),
                        stage,
                        visible.clone(),
                        target,
                        1,
                    )
                }))
                .await;
            let mut winners = 0;
            for (index, (result, (store, stage, target))) in
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
            )
            .await
            .unwrap();
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
}
