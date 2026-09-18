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
    IndexedStorageNamespace, ScanCursor, ScanResume, WriterId,
};
use async_trait::async_trait;
use bytes::Bytes;
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

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration/indexed");

#[derive(Debug, Clone)]
pub struct SqliteIndexedStorage {
    pool: SqlitePool,
    /// Recorded beside the epoch on every oplog this process claims, so an equal epoch from
    /// another process is refused rather than shared. One per process; see [`WriterId`].
    writer_id: WriterId,
}

impl SqliteIndexedStorage {
    /// Whether this backend enforces the shard-epoch fence on writes.
    ///
    /// A constant rather than a literal in the trait impl because
    /// [`super::multi_sqlite::MultiSqliteIndexedStorage`] is a fan-out of these and must always
    /// answer the same way: it has no namespace to delegate the question through, so this is what
    /// keeps the two from drifting apart.
    pub(crate) const SUPPORTS_EPOCH_FENCING: bool = true;

    pub async fn configured(config: &DbSqliteConfig) -> Result<Self, String> {
        Self::migrate(config).await?;

        let pool = SqlitePool::configured(config)
            .await
            .map_err(|err| format!("Sqlite indexed storage pool initialization failed: {err:?}"))?;

        Ok(Self {
            pool,
            writer_id: WriterId::process(),
        })
    }

    /// Writes as `writer_id` rather than as this process's own. The fan-out backend uses it to
    /// give every storage it opens one identity, and a test uses it to play two executors racing
    /// over one oplog inside a single process.
    pub fn for_writer(mut self, writer_id: WriterId) -> Self {
        self.writer_id = writer_id;
        self
    }

    /// Apply the indexed storage migrations on the given sqlite config without
    /// creating a pool.
    pub async fn migrate(config: &DbSqliteConfig) -> Result<(), String> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::sqlite::migrate(config, migrations.sqlite_migrations())
            .await
            .map_err(|err| format!("Sqlite indexed storage migration failed: {err:?}"))
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
    /// than go through `Json`, which encodes as TEXT - see [`Self::upsert_oplog_metadata`]) has to
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
        format!("SQLite indexed storage read a negative shard epoch {value} for key '{key}'")
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

    fn to_like_prefix(prefix: &str) -> String {
        let mut result = String::with_capacity(prefix.len() + 1);
        for ch in prefix.chars() {
            match ch {
                '%' | '_' | '\\' => {
                    result.push('\\');
                    result.push(ch);
                }
                _ => result.push(ch),
            }
        }
        result.push('%');
        result
    }
}

#[async_trait]
impl IndexedStorage for SqliteIndexedStorage {
    fn supports_epoch_fencing(&self) -> bool {
        Self::SUPPORTS_EPOCH_FENCING
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
        let query = sqlx::query_as::<_, (bool,)>(
            "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace = ? AND key = ?);",
        )
        .bind(Self::namespace(namespace))
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as(query)
            .await
            .map(|row| row.unwrap_or((false,)).0)
            .map_err(Self::classify_repo_error)
    }

    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), IndexedStorageError> {
        let query = match prefix {
            Some(prefix) => {
                let key = Self::to_like_prefix(prefix);
                sqlx::query_as(
                    "SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key LIKE ? ESCAPE '\\' ORDER BY key LIMIT ? OFFSET ?;",
                )
                .bind(Self::meta_namespace(namespace))
                .bind(key)
                .bind(sqlx::types::Json(count))
                .bind(sqlx::types::Json(cursor))
            }
            None => sqlx::query_as(
                "SELECT DISTINCT key FROM index_storage WHERE namespace = ? ORDER BY key LIMIT ? OFFSET ?;",
            )
            .bind(Self::meta_namespace(namespace))
            .bind(sqlx::types::Json(count))
            .bind(sqlx::types::Json(cursor)),
        };

        let keys = self
            .pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<(String,), _>(query)
            .await
            .map(|keys| keys.into_iter().map(|k| k.0).collect::<Vec<String>>())
            .map_err(Self::classify_repo_error)?;

        let new_cursor = if keys.len() < count as usize {
            0
        } else {
            cursor + count
        };

        Ok((new_cursor, keys))
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
        // Stored keys are never empty, so `key > ''` starts the first page at the first key.
        let after = resume
            .map(|resume| resume.into_marker("SQLite"))
            .transpose()?
            .unwrap_or_default();
        let query = match prefix {
            Some(prefix) => {
                let like = Self::to_like_prefix(prefix);
                sqlx::query_as(
                    "SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key > ? AND key LIKE ? ESCAPE '\\' ORDER BY key LIMIT ?;",
                )
                .bind(Self::meta_namespace(namespace))
                .bind(after)
                .bind(like)
                .bind(sqlx::types::Json(count))
            }
            None => sqlx::query_as(
                "SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key > ? ORDER BY key LIMIT ?;",
            )
            .bind(Self::meta_namespace(namespace))
            .bind(after)
            .bind(sqlx::types::Json(count)),
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
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.append_many(
            svc_name,
            api_name,
            entity_name,
            &namespace,
            key,
            vec![(id, Bytes::from(value))].into(),
            shard_epoch,
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
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        if pairs.is_empty() {
            return Ok(());
        }

        let primary_oplog_insert = matches!(namespace, IndexedStorageNamespace::OpLog { .. });
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
                    if let Some(expected) = shard_epoch {
                        let stored: Option<(i64, String)> = tx
                            .fetch_optional_as(
                                sqlx::query_as(
                                    "SELECT epoch, owner FROM oplog_metadata WHERE namespace = ? AND key = ?;",
                                )
                                .bind(namespace.clone())
                                .bind(key.clone()),
                            )
                            .await?;
                        let mut actual = None;
                        let mut owner_matches = false;
                        if let Some((epoch, owner)) = stored {
                            let epoch = u64::try_from(epoch).map_err(|_| {
                                FencedTxError::Corrupt(Self::negative_epoch_message(epoch, &key))
                            })?;
                            actual = Some(ShardEpoch(epoch));
                            owner_matches = owner == writer_id;
                        }
                        // The epoch says which generation may write; the writer says which of two
                        // processes holding that generation recorded it, which only a shard
                        // manager that lost its state can produce.
                        if actual != Some(expected) || !owner_matches {
                            return Err(FencedTxError::Fenced {
                                key: key.clone(),
                                expected,
                                actual,
                                owner_conflict: actual == Some(expected) && !owner_matches,
                            });
                        }
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

    /// SQLite's half of [`IndexedStorage::upsert_oplog_metadata`], which states the rule this
    /// enforces. The unqualified `epoch`/`owner` in the `WHERE` are the existing row's, and
    /// `excluded` is the row being written.
    async fn upsert_oplog_metadata(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        shard_epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError> {
        let namespace = Self::namespace(namespace);
        // `i64`, not `u64`: sqlx has no `Encode<Sqlite>` for `u64`, which is why ids elsewhere in
        // this file go through `Json`. That encodes as TEXT, and comparison affinity is applied
        // per operand, so this column stays integer-bound everywhere. Checked (see `to_i64`)
        // rather than `as i64`, which would silently wrap an out-of-range epoch to negative.
        let epoch = Self::to_i64(shard_epoch.0, "shard_epoch")?;

        let writer_id = self.writer_id.to_string();

        let mut api = self.pool.with_rw(svc_name, api_name);
        let result = api
            .execute(
                sqlx::query(
                    r#"INSERT INTO oplog_metadata (namespace, key, epoch, owner) VALUES (?, ?, ?, ?)
                       ON CONFLICT(namespace, key) DO UPDATE SET epoch = excluded.epoch, owner = excluded.owner
                       WHERE epoch < excluded.epoch
                          OR (epoch = excluded.epoch AND owner = excluded.owner);"#,
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
                        "SELECT epoch, owner FROM oplog_metadata WHERE namespace = ? AND key = ?;",
                    )
                    .bind(namespace)
                    .bind(key),
                )
                .await
                .map_err(Self::classify_repo_error)?;
            let mut actual = None;
            let mut owner_matches = false;
            if let Some((epoch, owner)) = stored {
                let epoch = u64::try_from(epoch).map_err(|_| {
                    IndexedStorageError::Other(Self::negative_epoch_message(epoch, key))
                })?;
                actual = Some(ShardEpoch(epoch));
                owner_matches = owner == writer_id;
            }
            return Err(IndexedStorageError::Fenced {
                key: key.to_string(),
                expected: shard_epoch,
                actual,
                owner_conflict: actual == Some(shard_epoch) && !owner_matches,
            });
        }

        Ok(())
    }

    async fn delete_oplog_metadata(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let query = sqlx::query("DELETE FROM oplog_metadata WHERE namespace = ? AND key = ?;")
            .bind(Self::namespace(namespace))
            .bind(key);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(Self::classify_repo_error)
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
        let query = sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ?;")
            .bind(Self::namespace(namespace))
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
        let query =
            sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ? AND id <= ?;")
                .bind(Self::namespace(namespace))
                .bind(key)
                .bind(sqlx::types::Json(last_dropped_id));

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
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
    // read turns into a spuriously huge one - the class of bug that let a corrupted epoch panic
    // downstream in the shard manager (`ShardEpoch::next`'s `checked_add(1).expect(..)`).
    async fn upsert_oplog_metadata_rejects_an_epoch_that_does_not_fit_i64() {
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
            .upsert_oplog_metadata(
                "test",
                "upsert_oplog_metadata",
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
    async fn upsert_oplog_metadata_accepts_the_largest_epoch_that_fits_i64() {
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
            .upsert_oplog_metadata(
                "test",
                "upsert_oplog_metadata",
                namespace,
                "oplog",
                ShardEpoch(i64::MAX as u64),
            )
            .await
            .unwrap();
    }
}
