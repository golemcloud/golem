// Copyright 2024 Golem Cloud
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

use crate::storage::indexed::ScanCursor;
use bytes::Bytes;
use golem_common::config::DbSqliteConfig;
use golem_common::metrics::db::{record_db_failure, record_db_success};
use sqlx::query::QueryAs;
use sqlx::sqlite::{SqliteArguments, SqlitePoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::FromRow;
use sqlx::SqlitePool as SqlitePoolx;
use sqlx::{Error, Sqlite};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct SqlitePool {
    pool: SqlitePoolx,
}

impl SqlitePool {
    pub async fn new(pool: SqlitePoolx) -> Result<Self, anyhow::Error> {
        SqlitePool::init(&pool).await?;
        Ok(SqlitePool { pool })
    }

    pub async fn configured(config: &DbSqliteConfig) -> Result<Self, anyhow::Error> {
        let conn_options = SqliteConnectOptions::new()
            .filename(Path::new(config.database.as_str()))
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .connect_with(conn_options)
            .await?;

        SqlitePool::new(pool).await
    }

    pub async fn init(pool: &SqlitePoolx) -> Result<(), Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS kv_storage (
                key TEXT NOT NULL,         -- The key to store
                value BLOB NOT NULL,       -- The value to store
                namespace TEXT NOT NULL,   -- The namespace of  the key value 
                PRIMARY KEY(key, namespace)     -- Avoid duplicate key values in a namespace
            );
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
              CREATE TABLE IF NOT EXISTS set_storage (
                key TEXT NOT NULL,             -- The set's key
                value BLOB NOT NULL,           -- The value (element)
                namespace TEXT NOT NULL,       -- The namespace of  the key value 
                PRIMARY KEY (key, value, namespace)   -- Composite primary key ensure uniqueness of values per (set, namespace)
              );
            "#,
        )
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
                CREATE INDEX IF NOT EXISTS idx_set_storage_key_namespace ON set_storage (key, namespace);
            "#)
            .execute(pool)
            .await?;

        sqlx::query(
            r#"
              CREATE TABLE IF NOT EXISTS sorted_set_storage (
                key TEXT NOT NULL,             -- The sorted set's key
                value BLOB NOT NULL,           -- The value (element)
                namespace TEXT NOT NULL,       -- The namespace of  the key value 
                score REAL NOT NULL,           -- The score associated with the value
                PRIMARY KEY(key, value, namespace)  -- Composite primary key ensure uniqueness of values per (set, namespace)
              );
            "#,
        )
        .execute(pool)
        .await?;
        sqlx::query(
                r#"
                    CREATE INDEX IF NOT EXISTS idx_sorted_set_storage_key_namespace ON sorted_set_storage (key, namespace);
                "#)
                .execute(pool)
                .await?;
        sqlx::query(
                r#"
                    CREATE INDEX IF NOT EXISTS idx_sorted_set_storage_score  ON sorted_set_storage (score);                
                "#)
                .execute(pool)
                .await?;

        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS index_storage (
                    namespace TEXT NOT NULL,          -- Namespace to logically group entries
                    key TEXT NOT NULL,                -- Unique identifier for the index
                    id INTEGER NOT NULL,              -- Unique numeric identifier for each entry
                    value BLOB NOT NULL,              -- Arbitrary binary payload for each entry
                    PRIMARY KEY (namespace, key, id)  -- Unique constraint on (namespace, key, id)
                );
                 "#,
        )
        .execute(pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_key ON index_storage (namespace, key);")
            .execute(pool)
            .await?;

        Ok(())
    }

    pub fn with(&self, svc_name: &'static str, api_name: &'static str) -> SqliteLabelledApi {
        SqliteLabelledApi {
            svc_name,
            api_name,
            pool: self.pool.clone(),
        }
    }
}

pub struct SqliteLabelledApi {
    svc_name: &'static str,
    api_name: &'static str,
    pool: SqlitePoolx,
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

#[derive(sqlx::FromRow, Debug)]
struct DBKey {
    key: String,
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
struct DBIdValue {
    pub id: i64,
    value: Vec<u8>,
}

impl DBIdValue {
    fn into_pair(self) -> (u64, Bytes) {
        (self.id as u64, Bytes::from(self.value))
    }
}

impl SqliteLabelledApi {
    async fn fetch_optional<'a, T>(
        &self,
        query: QueryAs<'a, Sqlite, T, SqliteArguments<'a>>,
    ) -> Result<Option<T>, Error>
    where
        T: Send + Unpin + for<'r> FromRow<'r, SqliteRow>,
    {
        query.fetch_optional(&self.pool).await
    }

    async fn fetch_all<'a, T>(
        &self,
        query: QueryAs<'a, Sqlite, T, SqliteArguments<'a>>,
    ) -> Result<Vec<T>, Error>
    where
        T: Send + Unpin + for<'r> FromRow<'r, SqliteRow>,
    {
        query.fetch_all(&self.pool).await
    }

    fn record<R>(
        &self,
        start: Instant,
        cmd_name: &'static str,
        result: Result<R, Error>,
    ) -> Result<R, Error> {
        let end = Instant::now();
        match result {
            Ok(result) => {
                record_db_success(
                    "sqlite",
                    self.svc_name,
                    self.api_name,
                    cmd_name,
                    end.duration_since(start),
                );
                Ok(result)
            }
            Err(err) => {
                record_db_failure("sqlite", self.svc_name, self.api_name, cmd_name);
                Err(err)
            }
        }
    }
    pub async fn set(&self, key: &str, value: &[u8], namespace: &str) -> Result<(), Error> {
        let query = sqlx::query(
            "INSERT OR REPLACE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
        )
        .bind(key)
        .bind(value)
        .bind(namespace);

        let start = Instant::now();
        self.record(start, "set", query.execute(&self.pool).await)
            .map(|_| ())
    }

    pub async fn set_many(&self, namespace: &str, pairs: &[(&str, &[u8])]) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        let start = Instant::now();

        for (field_key, field_value) in pairs {
            sqlx::query(
                "INSERT OR REPLACE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
            )
            .bind(field_key)
            .bind(field_value)
            .bind(namespace)
            .execute(&mut *tx)
            .await?;
        }
        let result = tx.commit().await;
        self.record(start, "set_many", result)
    }

    pub async fn set_if_not_exists(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<bool, Error> {
        let existing: Option<(i32,)> =
            sqlx::query_as::<_, (i32,)>("SELECT 1 FROM kv_storage WHERE key = ? AND namespace = ?")
                .bind(key)
                .bind(namespace)
                .fetch_optional(&self.pool)
                .await?;

        let query = sqlx::query(
            "INSERT OR IGNORE INTO kv_storage (key, value, namespace) VALUES (?, ?, ?);",
        )
        .bind(key)
        .bind(value)
        .bind(namespace);

        let start = Instant::now();
        self.record(start, "set_if_not_exists", query.execute(&self.pool).await)
            .map(|_| existing.is_none())
    }

    pub async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, Error> {
        let query = sqlx::query_as("SELECT value FROM kv_storage WHERE key = ? AND namespace = ?;")
            .bind(key)
            .bind(namespace);
        let start = Instant::now();
        self.record(start, "get", self.fetch_optional::<DBValue>(query).await)
            .map(|r| r.map(|op| op.into_bytes()))
    }

    pub async fn get_many(
        &self,
        namespace: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, Error> {
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");

        let statement = format!(
            "SELECT key, value FROM kv_storage WHERE key IN ({}) AND namespace = ?;",
            placeholders
        );
        let mut query = sqlx::query_as(&statement);

        for key in &keys {
            query = query.bind(key);
        }
        query = query.bind(namespace);
        let start = Instant::now();
        let results = self.record(start, "get_many", self.fetch_all::<DBKeyValue>(query).await)?;

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

    pub async fn del(&self, namespace: &str, key: &str) -> Result<(), Error> {
        let query = sqlx::query("DELETE FROM kv_storage WHERE key = ? AND namespace = ?;")
            .bind(key)
            .bind(namespace);
        let start = Instant::now();
        self.record(start, "del", query.execute(&self.pool).await)
            .map(|_| ())
    }

    pub async fn del_many(&self, namespace: &str, keys: Vec<String>) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        let start = Instant::now();

        for key in keys {
            sqlx::query("DELETE FROM kv_storage WHERE key = ? AND namespace = ?;")
                .bind(key)
                .bind(namespace)
                .execute(&mut *tx)
                .await?;
        }
        let result = tx.commit().await;
        self.record(start, "del_many", result)
    }

    pub async fn exists(&self, namespace: &str, key: &str) -> Result<bool, Error> {
        let query = sqlx::query("SELECT 1 FROM kv_storage WHERE key = ? AND namespace = ?")
            .bind(key)
            .bind(namespace);

        let start = Instant::now();
        self.record(start, "exists", query.fetch_optional(&self.pool).await)
            .map(|row| row.is_some())
    }

    pub async fn keys(&self, namespace: &str) -> Result<Vec<String>, Error> {
        let query =
            sqlx::query_as("SELECT key FROM kv_storage WHERE namespace = ?;").bind(namespace);

        let start = Instant::now();
        self.record(start, "keys", self.fetch_all::<DBKey>(query).await)
            .map(|vec| vec.into_iter().map(|k| k.key).collect::<Vec<String>>())
    }

    pub async fn add_to_set(&self, namespace: &str, key: &str, value: &[u8]) -> Result<(), Error> {
        let query = sqlx::query(
            "INSERT OR REPLACE INTO set_storage (namespace, key, value) VALUES (?, ?, ?);",
        )
        .bind(namespace)
        .bind(key)
        .bind(value);

        let start = Instant::now();
        self.record(start, "add_to_set", query.execute(&self.pool).await)
            .map(|_| ())
    }

    pub async fn remove_from_set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        let query =
            sqlx::query("DELETE FROM set_storage WHERE key = ? AND value = ? AND namespace = ?;")
                .bind(key)
                .bind(value)
                .bind(namespace);
        let start = Instant::now();
        self.record(start, "remove_from_set", query.execute(&self.pool).await)
            .map(|_| ())
    }

    pub async fn members_of_set(&self, namespace: &str, key: &str) -> Result<Vec<Bytes>, Error> {
        let query =
            sqlx::query_as("SELECT value FROM set_storage WHERE key = ? AND namespace = ?;")
                .bind(key)
                .bind(namespace);

        let start = Instant::now();
        self.record(
            start,
            "members_of_set",
            self.fetch_all::<DBValue>(query).await,
        )
        .map(|vec| {
            vec.into_iter()
                .map(|k| k.into_bytes())
                .collect::<Vec<Bytes>>()
        })
    }

    pub async fn add_to_sorted_set(
        &self,
        namespace: &str,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), Error> {
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

        let start = Instant::now();
        self.record(start, "add_to_sorted_set", query.execute(&self.pool).await)
            .map(|_| ())
    }

    pub async fn remove_from_sorted_set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        let query = sqlx::query(
            "DELETE FROM sorted_set_storage WHERE key = ? AND value = ? AND namespace = ?;",
        )
        .bind(key)
        .bind(value)
        .bind(namespace);
        let start = Instant::now();
        self.record(
            start,
            "remove_from_sorted_set",
            query.execute(&self.pool).await,
        )
        .map(|_| ())
    }

    pub async fn get_sorted_set(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, Error> {
        let query =
        sqlx::query_as("SELECT score, value FROM sorted_set_storage WHERE key = ? AND namespace = ? ORDER BY score ASC;")
            .bind(key)
            .bind(namespace);

        let start = Instant::now();
        self.record(
            start,
            "get_sorted_set",
            self.fetch_all::<DBScoreValue>(query).await,
        )
        .map(|vec| {
            vec.into_iter()
                .map(|k| k.into_pair())
                .collect::<Vec<(f64, Bytes)>>()
        })
    }

    pub async fn query_sorted_set(
        &self,
        namespace: &str,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, Error> {
        let query =
        sqlx::query_as("SELECT value, score FROM sorted_set_storage WHERE key = ? AND namespace = ? AND score BETWEEN ? AND ? ORDER BY score ASC;")
            .bind(key)
            .bind(namespace)
            .bind(min)
            .bind(max);

        let start = Instant::now();
        self.record(
            start,
            "query_sorted_set",
            self.fetch_all::<DBScoreValue>(query).await,
        )
        .map(|vec| {
            vec.into_iter()
                .map(|k| k.into_pair())
                .collect::<Vec<(f64, Bytes)>>()
        })
    }

    pub async fn exists_index(&self, namespace: &str, key: &str) -> Result<bool, Error> {
        let query = sqlx::query_as::<_, (bool,)>(
            "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace = ? AND key = ?);",
        )
        .bind(namespace)
        .bind(key);

        let start = Instant::now();
        self.record(
            start,
            "exists_index",
            query.fetch_optional(&self.pool).await,
        )
        .map(|row| row.unwrap_or((false,)).0)
    }

    pub async fn scan(
        &self,
        namespace: &str,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), Error> {
        let key = pattern.replace("*", "%").replace("?", "_");
        let query =
        sqlx::query_as("SELECT key FROM index_storage WHERE namespace = ? AND key LIKE ? ORDER BY key LIMIT ? OFFSET ?;")
        .bind(namespace)
            .bind(&key)
            .bind(sqlx::types::Json(count))
            .bind(sqlx::types::Json(cursor));

        let start = Instant::now();
        let keys = self
            .record(start, "scan", self.fetch_all::<DBKey>(query).await)
            .map(|keys| keys.into_iter().map(|k| k.key).collect::<Vec<String>>())?;

        let new_cursor = if keys.len() < count as usize {
            0
        } else {
            cursor + count
        };

        Ok((new_cursor, keys))
    }

    pub async fn append(
        &self,
        namespace: &str,
        key: &str,
        id: u64,
        value: &[u8],
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO index_storage (namespace, key, id, value) VALUES (?,?,?,?);
            "#,
        )
        .bind(namespace)
        .bind(key)
        .bind(sqlx::types::Json(id))
        .bind(value);

        let start = Instant::now();
        self.record(start, "append", query.execute(&self.pool).await)
            .map(|_| ())
    }

    pub async fn length(&self, namespace: &str, key: &str) -> Result<u64, Error> {
        let query = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM index_storage WHERE namespace = ? AND key = ?;",
        )
        .bind(namespace)
        .bind(key);

        let start = Instant::now();
        self.record(start, "length", query.fetch_optional(&self.pool).await)
            .map(|row| row.map(|r| r.0 as u64).unwrap_or(0))
    }

    pub async fn delete(&self, namespace: &str, key: &str) -> Result<(), Error> {
        let query = sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ?;")
            .bind(namespace)
            .bind(key);

        let start = Instant::now();
        self.record(start, "delete", query.execute(&self.pool).await)
            .map(|_| ())
    }

    pub async fn read(
        &self,
        namespace: &str,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Bytes)>, Error> {
        let query = sqlx::query_as(
            "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? AND id BETWEEN ? AND ?;",
        )
        .bind(namespace)
        .bind(key)
        .bind(sqlx::types::Json(start_id))
        .bind(sqlx::types::Json(end_id));

        let start = Instant::now();
        self.record(start, "read", self.fetch_all::<DBIdValue>(query).await)
            .map(|vec| vec.into_iter().map(|row| row.into_pair()).collect())
    }

    pub async fn first(&self, namespace: &str, key: &str) -> Result<Option<(u64, Bytes)>, Error> {
        let query = sqlx::query_as(
            "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? ORDER BY id ASC LIMIT 1;",
        )
        .bind(namespace)
        .bind(key);

        let start = Instant::now();
        self.record(
            start,
            "first",
            self.fetch_optional::<DBIdValue>(query).await,
        )
        .map(|op| op.map(|row| row.into_pair()))
    }

    pub async fn last(&self, namespace: &str, key: &str) -> Result<Option<(u64, Bytes)>, Error> {
        let query = sqlx::query_as(
            "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? ORDER BY id DESC LIMIT 1;",
        )
        .bind(namespace)
        .bind(key);

        let start = Instant::now();
        self.record(start, "last", self.fetch_optional::<DBIdValue>(query).await)
            .map(|op| op.map(|row| row.into_pair()))
    }

    pub async fn closest(
        &self,
        namespace: &str,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Bytes)>, Error> {
        let query = sqlx::query_as(
            "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? AND id >= ? ORDER BY id ASC LIMIT 1;",
        )
        .bind(namespace)
        .bind(key)
        .bind(sqlx::types::Json(id));

        let start = Instant::now();
        self.record(
            start,
            "closest",
            self.fetch_optional::<DBIdValue>(query).await,
        )
        .map(|op| op.map(|row| row.into_pair()))
    }

    pub async fn drop_prefix(
        &self,
        namespace: &str,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), Error> {
        let query =
            sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ? AND id <= ?;")
                .bind(namespace)
                .bind(key)
                .bind(sqlx::types::Json(last_dropped_id));

        let start = Instant::now();
        self.record(start, "drop_prefix", query.execute(&self.pool).await)
            .map(|_| ())
    }
}
