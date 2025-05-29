// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::{IndexedStorage, IndexedStorageNamespace, ScanCursor};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::SafeDisplay;
use golem_service_base::db::sqlite::SqlitePool;
use std::time::Duration;

#[derive(Debug)]
pub struct SqliteIndexedStorage {
    pool: SqlitePool,
}

impl SqliteIndexedStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, String> {
        let result = Self { pool };
        result.init().await?;
        Ok(result)
    }

    async fn init(&self) -> Result<(), String> {
        let pool = self.pool.with_rw("indexed_storage", "init");

        pool.execute(
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
        ))
            .await.map_err(|err| err.to_safe_string())?;

        pool.execute(sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_key ON index_storage (namespace, key);",
        ))
        .await
        .map_err(|err| err.to_safe_string())?;
        Ok(())
    }

    fn namespace(namespace: IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog => "worker-oplog".to_string(),
            IndexedStorageNamespace::CompressedOpLog { level } => {
                format!("worker-c{level}-oplog")
            }
        }
    }
}

#[async_trait]
impl IndexedStorage for SqliteIndexedStorage {
    async fn number_of_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
    ) -> Result<u8, String> {
        Ok(1)
    }

    async fn wait_for_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<u8, String> {
        Ok(1)
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
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
            .map_err(|err| err.to_safe_string())
    }

    async fn scan(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        let key = pattern.replace("*", "%").replace("?", "_");
        let query =
            sqlx::query_as("SELECT DISTINCT key FROM index_storage WHERE namespace = ? AND key LIKE ? ORDER BY key LIMIT ? OFFSET ?;")
                .bind(Self::namespace(namespace))
                .bind(&key)
                .bind(sqlx::types::Json(count))
                .bind(sqlx::types::Json(cursor));

        let keys = self
            .pool
            .with_ro(svc_name, api_name)
            .fetch_all::<(String,), _>(query)
            .await
            .map(|keys| keys.into_iter().map(|k| k.0).collect::<Vec<String>>())
            .map_err(|err| err.to_safe_string())?;

        let new_cursor = if keys.len() < count as usize {
            0
        } else {
            cursor + count
        };

        Ok((new_cursor, keys))
    }

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: &[u8],
    ) -> Result<(), String> {
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
            .map_err(|err| err.to_safe_string())
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String> {
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
            .map_err(|err| err.to_safe_string())
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let query = sqlx::query("DELETE FROM index_storage WHERE namespace = ? AND key = ?;")
            .bind(Self::namespace(namespace))
            .bind(key);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
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
    ) -> Result<Vec<(u64, Bytes)>, String> {
        let query = sqlx::query_as(
            "SELECT id, value FROM index_storage WHERE namespace = ? AND key = ? AND id BETWEEN ? AND ?;",
        )
            .bind(Self::namespace(namespace))
            .bind(key)
            .bind(sqlx::types::Json(start_id))
            .bind(sqlx::types::Json(end_id));

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all::<DBIdValue, _>(query)
            .await
            .map(|vec| vec.into_iter().map(|row| row.into_pair()).collect())
            .map_err(|err| err.to_safe_string())
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
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
            .map_err(|err| err.to_safe_string())
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Bytes)>, String> {
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
            .map_err(|err| err.to_safe_string())
    }

    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Bytes)>, String> {
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
            .map_err(|err| err.to_safe_string())
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), String> {
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
            .map_err(|err| err.to_safe_string())
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
