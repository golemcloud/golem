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

use crate::services::golem_config::KeyValueStoragePostgresConfig;
use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use golem_common::SafeDisplay;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::include_dir;
use sqlx::{Postgres, QueryBuilder};

static DB_MIGRATIONS: include_dir::Dir =
    include_dir!("$CARGO_MANIFEST_DIR/db/migration/postgres/keyvalue");

#[derive(Debug, Clone)]
pub struct PostgresKeyValueStorage {
    pool: PostgresPool,
}

impl PostgresKeyValueStorage {
    const SET_MANY_CHUNK_SIZE: usize = 1024;
    const MANY_KEYS_CHUNK_SIZE: usize = 1024;

    pub async fn configured(config: &KeyValueStoragePostgresConfig) -> Result<Self, String> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::postgres::migrate(
            &config.postgres,
            migrations.postgres_migrations(),
        )
        .await
        .map_err(|err| format!("Postgres key-value storage migration failed: {err:?}"))?;

        let pool = PostgresPool::configured(&config.postgres)
            .await
            .map_err(|err| {
                format!("Postgres key-value storage pool initialization failed: {err:?}")
            })?;

        Ok(Self { pool })
    }

    pub async fn new(pool: PostgresPool) -> Result<Self, String> {
        Ok(Self { pool })
    }

    fn namespace(namespace: KeyValueStorageNamespace) -> String {
        match namespace {
            KeyValueStorageNamespace::RunningWorkers => "running-workers".to_string(),
            KeyValueStorageNamespace::Worker { .. } => "worker".to_string(),
            KeyValueStorageNamespace::Promise { .. } => "promises".to_string(),
            KeyValueStorageNamespace::Schedule => "schedule".to_string(),
            KeyValueStorageNamespace::UserDefined {
                environment_id,
                bucket,
            } => format!("user-defined:{environment_id}:{bucket}"),
        }
    }

    fn value_hash(value: &[u8]) -> Vec<u8> {
        blake3::hash(value).as_bytes().to_vec()
    }
}

#[async_trait]
impl KeyValueStorage for PostgresKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let query =
            sqlx::query("INSERT INTO kv_storage (namespace, key, value) VALUES ($1, $2, $3) ON CONFLICT (namespace, key) DO UPDATE SET value = EXCLUDED.value;")
                .bind(Self::namespace(namespace))
                .bind(key)
                .bind(value);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        if pairs.is_empty() {
            return Ok(());
        }

        let namespace = Self::namespace(namespace);
        let pairs: Vec<(String, Vec<u8>)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_vec()))
            .collect();

        self.pool
            .with_tx(svc_name, api_name, |tx| {
                async move {
                    for chunk in pairs.chunks(Self::SET_MANY_CHUNK_SIZE) {
                        let mut query_builder = QueryBuilder::<Postgres>::new(
                            "INSERT INTO kv_storage (namespace, key, value) ",
                        );
                        query_builder.push_values(chunk.iter(), |mut builder, (key, value)| {
                            builder
                                .push_bind(namespace.clone())
                                .push_bind(key)
                                .push_bind(value);
                        });
                        query_builder.push(
                            " ON CONFLICT (namespace, key) DO UPDATE SET value = EXCLUDED.value;",
                        );

                        tx.execute(query_builder.build()).await?;
                    }
                    Ok(())
                }
                .boxed()
            })
            .await
            .map_err(|err| err.to_safe_string())
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String> {
        let query = sqlx::query(
            "INSERT INTO kv_storage (namespace, key, value) VALUES ($1, $2, $3) ON CONFLICT (namespace, key) DO NOTHING;",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(value);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|err| err.to_safe_string())
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        let query = sqlx::query_as::<_, DBValue>(
            "SELECT value FROM kv_storage WHERE namespace = $1 AND key = $2;",
        )
        .bind(Self::namespace(namespace))
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_optional_as::<DBValue, _>(query)
            .await
            .map(|v| v.map(DBValue::into_bytes))
            .map_err(|err| err.to_safe_string())
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let namespace = Self::namespace(namespace);
        let mut result = Vec::with_capacity(keys.len());

        for chunk in keys.chunks(Self::MANY_KEYS_CHUNK_SIZE) {
            let query = sqlx::query_as::<_, DBOrderedValue>(
                "SELECT kv.value
                 FROM unnest($2::text[]) WITH ORDINALITY AS requested(key, ord)
                 LEFT JOIN kv_storage kv ON kv.namespace = $1 AND kv.key = requested.key
                 ORDER BY requested.ord;",
            )
            .bind(namespace.clone())
            .bind(chunk);

            let rows = self
                .pool
                .with_ro(svc_name, api_name)
                .fetch_all_as::<DBOrderedValue, _>(query)
                .await
                .map_err(|err| err.to_safe_string())?;

            result.extend(rows.into_iter().map(DBOrderedValue::into_bytes));
        }

        Ok(result)
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let query = sqlx::query("DELETE FROM kv_storage WHERE namespace = $1 AND key = $2;")
            .bind(Self::namespace(namespace))
            .bind(key);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        if keys.is_empty() {
            return Ok(());
        }

        let namespace = Self::namespace(namespace);
        self.pool
            .with_tx(svc_name, api_name, |tx| {
                async move {
                    for chunk in keys.chunks(Self::MANY_KEYS_CHUNK_SIZE) {
                        let query = sqlx::query(
                            "DELETE FROM kv_storage WHERE namespace = $1 AND key = ANY($2);",
                        )
                        .bind(namespace.clone())
                        .bind(chunk);

                        tx.execute(query).await?;
                    }
                    Ok(())
                }
                .boxed()
            })
            .await
            .map_err(|err| err.to_safe_string())
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        let query = sqlx::query_as::<_, (bool,)>(
            "SELECT EXISTS(SELECT 1 FROM kv_storage WHERE namespace = $1 AND key = $2);",
        )
        .bind(Self::namespace(namespace))
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_one_as::<(bool,), _>(query)
            .await
            .map(|row| row.0)
            .map_err(|err| err.to_safe_string())
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        let query = sqlx::query_as::<_, (String,)>(
            "SELECT key FROM kv_storage WHERE namespace = $1 ORDER BY key ASC;",
        )
        .bind(Self::namespace(namespace));

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<(String,), _>(query)
            .await
            .map(|rows| rows.into_iter().map(|row| row.0).collect())
            .map_err(|err| err.to_safe_string())
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
        let query = sqlx::query(
            "INSERT INTO set_storage (namespace, key, value) VALUES ($1, $2, $3) ON CONFLICT (namespace, key, value) DO NOTHING;",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(value);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
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
        let query = sqlx::query(
            "DELETE FROM set_storage WHERE namespace = $1 AND key = $2 AND value = $3;",
        )
                .bind(Self::namespace(namespace))
                .bind(key)
                .bind(value);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        let query = sqlx::query_as::<_, DBValue>(
            "SELECT value FROM set_storage WHERE namespace = $1 AND key = $2;",
        )
        .bind(Self::namespace(namespace))
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<DBValue, _>(query)
            .await
            .map(|rows| rows.into_iter().map(DBValue::into_bytes).collect())
            .map_err(|err| err.to_safe_string())
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
        let value_hash = Self::value_hash(value);
        let query = sqlx::query(
            "INSERT INTO sorted_set_storage (namespace, key, value_hash, value, score) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (namespace, key, value_hash) DO UPDATE SET value = EXCLUDED.value, score = EXCLUDED.score;",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(value_hash)
        .bind(value)
        .bind(score);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
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
        let value_hash = Self::value_hash(value);
        let query = sqlx::query(
            "DELETE FROM sorted_set_storage WHERE namespace = $1 AND key = $2 AND value_hash = $3 AND value = $4;",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(value_hash)
        .bind(value);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let query = sqlx::query_as::<_, DBScoreValue>(
            "SELECT score, value FROM sorted_set_storage WHERE namespace = $1 AND key = $2 ORDER BY score ASC;",
        )
        .bind(Self::namespace(namespace))
        .bind(key);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<DBScoreValue, _>(query)
            .await
            .map(|rows| rows.into_iter().map(DBScoreValue::into_pair).collect())
            .map_err(|err| err.to_safe_string())
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
        let query = sqlx::query_as::<_, DBScoreValue>(
            "SELECT score, value FROM sorted_set_storage WHERE namespace = $1 AND key = $2 AND score BETWEEN $3 AND $4 ORDER BY score ASC;",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(min)
        .bind(max);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<DBScoreValue, _>(query)
            .await
            .map(|rows| rows.into_iter().map(DBScoreValue::into_pair).collect())
            .map_err(|err| err.to_safe_string())
    }
}

#[derive(sqlx::FromRow, Debug)]
struct DBValue {
    value: Vec<u8>,
}

impl DBValue {
    fn into_bytes(self) -> Bytes {
        Bytes::from(self.value)
    }
}

#[derive(sqlx::FromRow, Debug)]
struct DBOrderedValue {
    value: Option<Vec<u8>>,
}

impl DBOrderedValue {
    fn into_bytes(self) -> Option<Bytes> {
        self.value.map(Bytes::from)
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
