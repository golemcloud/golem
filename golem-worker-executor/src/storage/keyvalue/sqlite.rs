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

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace, retry_on_pool_timeout};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::config::DbSqliteConfig;
use golem_common::metrics::db::record_db_serialized_size;
use golem_common::model::RetryConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{DBValue, LabelledPoolApi, LabelledPoolTransaction, PoolApi};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::include_dir;
use std::collections::HashMap;

const DB_TYPE: &str = "sqlite";

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration/keyvalue");

#[derive(Debug, Clone)]
pub struct SqliteKeyValueStorage {
    pool: SqlitePool,
    retry_config: RetryConfig,
}

impl SqliteKeyValueStorage {
    pub async fn configured(
        config: &DbSqliteConfig,
        retry_config: RetryConfig,
    ) -> Result<Self, String> {
        Self::migrate(config).await?;

        let pool = SqlitePool::configured(config).await.map_err(|err| {
            format!("Sqlite key-value storage pool initialization failed: {err:?}")
        })?;

        Ok(Self { pool, retry_config })
    }

    /// Apply the key-value storage migrations on the given sqlite config without
    /// creating a pool.
    pub async fn migrate(config: &DbSqliteConfig) -> Result<(), String> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::sqlite::migrate(config, migrations.sqlite_migrations())
            .await
            .map_err(|err| format!("Sqlite key-value storage migration failed: {err:?}"))
    }

    pub fn new(pool: SqlitePool, retry_config: RetryConfig) -> Self {
        Self { pool, retry_config }
    }

    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    fn namespace(ns: KeyValueStorageNamespace) -> String {
        match ns {
            KeyValueStorageNamespace::Worker { .. } => "worker".to_string(),
            // agent_id embedded so each agent's split status fields are an isolated key space
            // (per-agent `keys`/`del_many` select only that agent's rows).
            KeyValueStorageNamespace::AgentStatus { agent_id } => {
                format!("agent-status:{}", agent_id.to_redis_key())
            }
            KeyValueStorageNamespace::AgentStatusCheckpoint { agent_id } => {
                format!("agent-status-checkpoint:{}", agent_id.to_redis_key())
            }
            KeyValueStorageNamespace::Promise { .. } => "promise".to_string(),
            KeyValueStorageNamespace::Schedule => "schedule".to_string(),
            KeyValueStorageNamespace::UserDefined {
                environment_id,
                bucket,
            } => {
                format!("user-defined:{environment_id}:{bucket}")
            }
            KeyValueStorageNamespace::RunningWorkers => "running-workers".to_string(),
        }
    }
}

#[async_trait]
impl KeyValueStorage for SqliteKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        record_db_serialized_size(DB_TYPE, svc_name, entity_name, value.len());
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "set", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query(
                    "INSERT OR REPLACE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
                )
                .bind(key)
                .bind(value)
                .bind(namespace);
                self.pool
                    .with_rw(svc_name, api_name)
                    .execute(query)
                    .await
                    .map(|_| ())
            }
        })
        .await
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        for (_, field_value) in pairs {
            record_db_serialized_size(DB_TYPE, svc_name, entity_name, field_value.len());
        }
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "set_many", || {
            let namespace = &namespace;
            async move {
                let api = self.pool.with_rw(svc_name, api_name);
                let mut tx = api.begin().await?;

                for (field_key, field_value) in pairs {
                    tx.execute(
                        sqlx::query(
                            "INSERT OR REPLACE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
                        )
                        .bind(field_key)
                        .bind(field_value)
                        .bind(namespace.clone()),
                    )
                    .await?;
                }
                tx.commit().await
            }
        })
        .await
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String> {
        record_db_serialized_size(DB_TYPE, svc_name, entity_name, value.len());
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "set_if_not_exists", || {
            let namespace = namespace.clone();
            async move {
                let mut api = self.pool.with_rw(svc_name, api_name);
                let existing: Option<(i32,)> = api
                    .fetch_optional_as(
                        sqlx::query_as::<_, (i32,)>(
                            "SELECT 1 FROM kv_storage WHERE key = ? AND namespace = ?",
                        )
                        .bind(key)
                        .bind(namespace.clone()),
                    )
                    .await?;

                let query = sqlx::query(
                    "INSERT OR IGNORE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
                )
                .bind(key)
                .bind(value)
                .bind(namespace);

                api.execute(query).await.map(|_| existing.is_none())
            }
        })
        .await
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "get", || {
            let namespace = namespace.clone();
            async move {
                let query =
                    sqlx::query_as("SELECT value FROM kv_storage WHERE key = ? AND namespace = ?;")
                        .bind(key)
                        .bind(namespace);
                self.pool
                    .with_ro(svc_name, api_name)
                    .fetch_optional_as::<DBValue, _>(query)
                    .await
                    .map(|r| r.map(|op| op.into_bytes()))
            }
        })
        .await
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");

        let statement = format!(
            "SELECT key, value FROM kv_storage WHERE key IN ({placeholders}) AND namespace = ?;"
        );
        let namespace = Self::namespace(namespace);

        let results: Vec<DBKeyValue> =
            retry_on_pool_timeout(&self.retry_config, "get_many", || {
                let namespace = namespace.clone();
                let statement = statement.as_str();
                let keys = &keys;
                async move {
                    let mut query = sqlx::query_as(statement);
                    for key in keys {
                        query = query.bind(key);
                    }
                    query = query.bind(namespace);
                    self.pool
                        .with_ro(svc_name, api_name)
                        .fetch_all_as(query)
                        .await
                }
            })
            .await?;

        let mut result_map = results
            .into_iter()
            .map(|kv| kv.into_pair())
            .collect::<HashMap<String, Bytes>>();

        let values = keys
            .into_iter()
            .map(|key| result_map.remove(&key))
            .collect::<Vec<Option<Bytes>>>();

        Ok(values)
    }

    async fn get_all(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, String> {
        let namespace = Self::namespace(namespace);
        let results: Vec<DBKeyValue> = retry_on_pool_timeout(&self.retry_config, "get_all", || {
            let namespace = namespace.clone();
            async move {
                let query =
                    sqlx::query_as("SELECT key, value FROM kv_storage WHERE namespace = ?;")
                        .bind(namespace);
                self.pool
                    .with_ro(svc_name, api_name)
                    .fetch_all_as(query)
                    .await
            }
        })
        .await?;

        Ok(results.into_iter().map(|kv| kv.into_pair()).collect())
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "del", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query("DELETE FROM kv_storage WHERE key = ? AND namespace = ?;")
                    .bind(key)
                    .bind(namespace);
                self.pool
                    .with_rw(svc_name, api_name)
                    .execute(query)
                    .await
                    .map(|_| ())
            }
        })
        .await
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "del_many", || {
            let namespace = &namespace;
            let keys = &keys;
            async move {
                let api = self.pool.with_rw(svc_name, api_name);
                let mut tx = api.begin().await?;
                for key in keys {
                    tx.execute(
                        sqlx::query("DELETE FROM kv_storage WHERE key = ? AND namespace = ?;")
                            .bind(key)
                            .bind(namespace.clone()),
                    )
                    .await?;
                }
                tx.commit().await
            }
        })
        .await
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "exists", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query("SELECT 1 FROM kv_storage WHERE key = ? AND namespace = ?")
                    .bind(key)
                    .bind(namespace);
                self.pool
                    .with_ro(svc_name, api_name)
                    .fetch_optional(query)
                    .await
                    .map(|row| row.is_some())
            }
        })
        .await
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "keys", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query_as("SELECT key FROM kv_storage WHERE namespace = ?;")
                    .bind(namespace);
                self.pool
                    .with_ro(svc_name, api_name)
                    .fetch_all_as::<(String,), _>(query)
                    .await
                    .map(|vec| vec.into_iter().map(|k| k.0).collect::<Vec<String>>())
            }
        })
        .await
    }

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "add_to_set", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query(
                    "INSERT OR REPLACE INTO set_storage (namespace, key, value) VALUES (?, ?, ?);",
                )
                .bind(namespace)
                .bind(key)
                .bind(value);
                self.pool
                    .with_rw(svc_name, api_name)
                    .execute(query)
                    .await
                    .map(|_| ())
            }
        })
        .await
    }

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "remove_from_set", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query(
                    "DELETE FROM set_storage WHERE key = ? AND value = ? AND namespace = ?;",
                )
                .bind(key)
                .bind(value)
                .bind(namespace);
                self.pool
                    .with_rw(svc_name, api_name)
                    .execute(query)
                    .await
                    .map(|_| ())
            }
        })
        .await
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "members_of_set", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query_as(
                    "SELECT value FROM set_storage WHERE key = ? AND namespace = ?;",
                )
                .bind(key)
                .bind(namespace);
                self.pool
                    .with_ro(svc_name, api_name)
                    .fetch_all_as::<DBValue, _>(query)
                    .await
                    .map(|vec| {
                        vec.into_iter()
                            .map(|k| k.into_bytes())
                            .collect::<Vec<Bytes>>()
                    })
            }
        })
        .await
    }

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "add_to_sorted_set", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query(
                    r#"
                    INSERT INTO sorted_set_storage (key, value, namespace, score) VALUES (?, ?, ?, ?)
                    ON CONFLICT(key, value, namespace) DO UPDATE SET score = excluded.score;
                    "#,
                )
                .bind(key)
                .bind(value)
                .bind(namespace)
                .bind(score);
                self.pool
                    .with_rw(svc_name, api_name)
                    .execute(query)
                    .await
                    .map(|_| ())
            }
        })
        .await
    }

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "remove_from_sorted_set", || {
            let namespace = namespace.clone();
            async move {
                let query = sqlx::query(
                    "DELETE FROM sorted_set_storage WHERE key = ? AND value = ? AND namespace = ?;",
                )
                .bind(key)
                .bind(value)
                .bind(namespace);
                self.pool
                    .with_rw(svc_name, api_name)
                    .execute(query)
                    .await
                    .map(|_| ())
            }
        })
        .await
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "get_sorted_set", || {
            let namespace = namespace.clone();
            async move {
                let query =
                    sqlx::query_as("SELECT score, value FROM sorted_set_storage WHERE key = ? AND namespace = ? ORDER BY score ASC;")
                        .bind(key)
                        .bind(namespace);
                self.pool
                    .with_ro(svc_name, api_name)
                    .fetch_all_as::<DBScoreValue, _>(query)
                    .await
                    .map(|vec| {
                        vec.into_iter()
                            .map(|k| k.into_pair())
                            .collect::<Vec<(f64, Bytes)>>()
                    })
            }
        })
        .await
    }

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let namespace = Self::namespace(namespace);
        retry_on_pool_timeout(&self.retry_config, "query_sorted_set", || {
            let namespace = namespace.clone();
            async move {
                let query =
                    sqlx::query_as("SELECT value, score FROM sorted_set_storage WHERE key = ? AND namespace = ? AND score BETWEEN ? AND ? ORDER BY score ASC;")
                        .bind(key)
                        .bind(namespace)
                        .bind(min)
                        .bind(max);
                self.pool
                    .with_ro(svc_name, api_name)
                    .fetch_all_as::<DBScoreValue, _>(query)
                    .await
                    .map(|vec| {
                        vec.into_iter()
                            .map(|k| k.into_pair())
                            .collect::<Vec<(f64, Bytes)>>()
                    })
            }
        })
        .await
    }
}

#[derive(sqlx::FromRow, Debug)]
struct DBKeyValue {
    pub key: String,
    value: Vec<u8>,
}

impl DBKeyValue {
    fn into_pair(self) -> (String, Bytes) {
        (self.key, Bytes::from(self.value))
    }
}

#[derive(sqlx::FromRow, Debug)]
struct DBScoreValue {
    score: f64,
    value: Vec<u8>,
}

impl DBScoreValue {
    fn into_pair(self) -> (f64, Bytes) {
        (self.score, Bytes::from(self.value))
    }
}
