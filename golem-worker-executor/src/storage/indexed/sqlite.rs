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
    IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace, IndexedStorageNamespace,
    ScanResume,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use golem_common::SafeDisplay;
use golem_common::config::DbSqliteConfig;
use golem_common::metrics::db::record_db_serialized_size;
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
}

impl SqliteIndexedStorage {
    pub async fn configured(config: &DbSqliteConfig) -> Result<Self, String> {
        Self::migrate(config).await?;

        let pool = SqlitePool::configured(config)
            .await
            .map_err(|err| format!("Sqlite indexed storage pool initialization failed: {err:?}"))?;

        Ok(Self { pool })
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
        Self { pool }
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

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
    ) -> Result<(), IndexedStorageError> {
        record_db_serialized_size(DB_TYPE, svc_name, entity_name, value.len());
        let primary_oplog_insert = matches!(&namespace, IndexedStorageNamespace::OpLog { .. });
        let query = sqlx::query(
            r#"
                    INSERT INTO index_storage (namespace, key, id, value) VALUES (?,?,?,?);
                    "#,
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(sqlx::types::Json(id))
        .bind(value);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| {
                if primary_oplog_insert {
                    Self::classify_repo_error_primary_oplog_insert(err)
                } else {
                    Self::classify_repo_error(err)
                }
            })
    }

    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
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

        self.pool
            .with_tx(svc_name, api_name, |tx| {
                async move {
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
                }
                .boxed()
            })
            .await
            .map_err(|err| {
                if primary_oplog_insert {
                    Self::classify_repo_error_primary_oplog_insert(err)
                } else {
                    Self::classify_repo_error(err)
                }
            })
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
