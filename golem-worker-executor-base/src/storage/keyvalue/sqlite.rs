// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use golem_service_base::storage::sqlite::{DBValue, SqlitePool};
use std::collections::HashMap;

#[derive(Debug)]
pub struct SqliteKeyValueStorage {
    pool: SqlitePool,
}

impl SqliteKeyValueStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, String> {
        let result = Self { pool };
        result.init().await?;
        Ok(result)
    }

    async fn init(&self) -> Result<(), String> {
        self.pool
            .execute(sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS kv_storage (
                    key TEXT NOT NULL,         -- The key to store
                    value BLOB NOT NULL,       -- The value to store
                    namespace TEXT NOT NULL,   -- The namespace of  the key value
                    PRIMARY KEY(key, namespace)     -- Avoid duplicate key values in a namespace
                );
                "#,
            ))
            .await?;

        self.pool.execute(sqlx::query(
            r#"
                  CREATE TABLE IF NOT EXISTS set_storage (
                    key TEXT NOT NULL,             -- The set's key
                    value BLOB NOT NULL,           -- The value (element)
                    namespace TEXT NOT NULL,       -- The namespace of  the key value
                    PRIMARY KEY (key, value, namespace)   -- Composite primary key ensure uniqueness of values per (set, namespace)
                  );
                "#,
        ))
            .await?;
        self.pool.execute(sqlx::query(
            r#"
                    CREATE INDEX IF NOT EXISTS idx_set_storage_key_namespace ON set_storage (key, namespace);
                "#))
            .await?;

        self.pool.execute(sqlx::query(
            r#"
                  CREATE TABLE IF NOT EXISTS sorted_set_storage (
                    key TEXT NOT NULL,             -- The sorted set's key
                    value BLOB NOT NULL,           -- The value (element)
                    namespace TEXT NOT NULL,       -- The namespace of  the key value
                    score REAL NOT NULL,           -- The score associated with the value
                    PRIMARY KEY(key, value, namespace)  -- Composite primary key ensure uniqueness of values per (set, namespace)
                  );
                "#,
        ))
            .await?;
        self.pool.execute(sqlx::query(
            r#"
                        CREATE INDEX IF NOT EXISTS idx_sorted_set_storage_key_namespace ON sorted_set_storage (key, namespace);
                    "#))
            .await?;
        self.pool.execute(sqlx::query(
            r#"
                        CREATE INDEX IF NOT EXISTS idx_sorted_set_storage_score  ON sorted_set_storage (score);
                    "#))
            .await?;

        Ok(())
    }

    fn namespace(ns: KeyValueStorageNamespace) -> String {
        match ns {
            KeyValueStorageNamespace::Worker => "worker".to_string(),
            KeyValueStorageNamespace::Promise => "promise".to_string(),
            KeyValueStorageNamespace::Schedule => "schedule".to_string(),
            KeyValueStorageNamespace::UserDefined { account_id, bucket } => {
                format!("user-defined:{}:{}", account_id, bucket)
            }
        }
    }
}

#[async_trait]
impl KeyValueStorage for SqliteKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let query = sqlx::query(
            "INSERT OR REPLACE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
        )
        .bind(key)
        .bind(value)
        .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        let api = self.pool.with(svc_name, api_name);
        let mut tx = api.begin().await?;

        for (field_key, field_value) in pairs {
            tx.execute(
                sqlx::query(
                    "INSERT OR REPLACE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
                )
                .bind(field_key)
                .bind(field_value)
                .bind(Self::namespace(namespace.clone())),
            )
            .await?;
        }
        api.commit(tx).await
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
        let api = self.pool.with(svc_name, api_name);
        let existing: Option<(i32,)> = api
            .fetch_optional_as(
                sqlx::query_as::<_, (i32,)>(
                    "SELECT 1 FROM kv_storage WHERE key = ? AND namespace = ?",
                )
                .bind(key)
                .bind(Self::namespace(namespace.clone())),
            )
            .await?;

        let query = sqlx::query(
            "INSERT OR IGNORE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
        )
        .bind(key)
        .bind(value)
        .bind(Self::namespace(namespace));

        api.execute(query).await.map(|_| existing.is_none())
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        let query = sqlx::query_as("SELECT value FROM kv_storage WHERE key = ? AND namespace = ?;")
            .bind(key)
            .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .fetch_optional_as::<DBValue, _>(query)
            .await
            .map(|r| r.map(|op| op.into_bytes()))
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
            "SELECT key, value FROM kv_storage WHERE key IN ({}) AND namespace = ?;",
            placeholders
        );
        let mut query = sqlx::query_as(&statement);

        for key in &keys {
            query = query.bind(key);
        }
        query = query.bind(Self::namespace(namespace));

        let results: Vec<DBKeyValue> = self.pool.with(svc_name, api_name).fetch_all(query).await?;

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

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let query = sqlx::query("DELETE FROM kv_storage WHERE key = ? AND namespace = ?;")
            .bind(key)
            .bind(Self::namespace(namespace));
        self.pool
            .with(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        let api = self.pool.with(svc_name, api_name);
        let mut tx = api.begin().await?;
        for key in keys {
            tx.execute(
                sqlx::query("DELETE FROM kv_storage WHERE key = ? AND namespace = ?;")
                    .bind(key)
                    .bind(Self::namespace(namespace.clone())),
            )
            .await?;
        }
        api.commit(tx).await
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        let query = sqlx::query("SELECT 1 FROM kv_storage WHERE key = ? AND namespace = ?")
            .bind(key)
            .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .fetch_optional(query)
            .await
            .map(|row| row.is_some())
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        let query = sqlx::query_as("SELECT key FROM kv_storage WHERE namespace = ?;")
            .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .fetch_all::<(String,), _>(query)
            .await
            .map(|vec| vec.into_iter().map(|k| k.0).collect::<Vec<String>>())
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
            "INSERT OR REPLACE INTO set_storage (namespace, key, value) VALUES (?, ?, ?);",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(value);

        self.pool
            .with(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
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
        let query =
            sqlx::query("DELETE FROM set_storage WHERE key = ? AND value = ? AND namespace = ?;")
                .bind(key)
                .bind(value)
                .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        let query =
            sqlx::query_as("SELECT value FROM set_storage WHERE key = ? AND namespace = ?;")
                .bind(key)
                .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .fetch_all::<DBValue, _>(query)
            .await
            .map(|vec| {
                vec.into_iter()
                    .map(|k| k.into_bytes())
                    .collect::<Vec<Bytes>>()
            })
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
        let query = sqlx::query(
                    r#"
                    INSERT INTO sorted_set_storage (key, value, namespace, score) VALUES (?, ?, ?, ?)
                    ON CONFLICT(key, value, namespace) DO UPDATE SET score = excluded.score;
                    "#,
                )
                .bind(key)
                .bind(value)
                .bind(Self::namespace(namespace))
                .bind(score);

        self.pool
            .with(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
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
        let query = sqlx::query(
            "DELETE FROM sorted_set_storage WHERE key = ? AND value = ? AND namespace = ?;",
        )
        .bind(key)
        .bind(value)
        .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let query =
                    sqlx::query_as("SELECT score, value FROM sorted_set_storage WHERE key = ? AND namespace = ? ORDER BY score ASC;")
                        .bind(key)
                        .bind(Self::namespace(namespace));

        self.pool
            .with(svc_name, api_name)
            .fetch_all::<DBScoreValue, _>(query)
            .await
            .map(|vec| {
                vec.into_iter()
                    .map(|k| k.into_pair())
                    .collect::<Vec<(f64, Bytes)>>()
            })
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
        let query =
            sqlx::query_as("SELECT value, score FROM sorted_set_storage WHERE key = ? AND namespace = ? AND score BETWEEN ? AND ? ORDER BY score ASC;")
                .bind(key)
                .bind(Self::namespace(namespace))
                .bind(min)
                .bind(max);

        self.pool
            .with(svc_name, api_name)
            .fetch_all::<DBScoreValue, _>(query)
            .await
            .map(|vec| {
                vec.into_iter()
                    .map(|k| k.into_pair())
                    .collect::<Vec<(f64, Bytes)>>()
            })
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
