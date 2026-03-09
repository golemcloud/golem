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

use super::{IndexedStorage, IndexedStorageMetaNamespace, IndexedStorageNamespace, ScanCursor};
use crate::services::golem_config::IndexedStoragePostgresConfig;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::{LabelledPoolTransaction, Pool};
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::include_dir;
use std::time::Duration;

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

#[derive(Debug, Clone)]
pub struct PostgresIndexedStorage {
    pool: PostgresPool,
    drop_prefix_delete_batch_size: u64,
}

impl PostgresIndexedStorage {
    pub async fn configured(config: &IndexedStoragePostgresConfig) -> Result<Self, String> {
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

        Ok(Self {
            pool,
            drop_prefix_delete_batch_size: config.drop_prefix_delete_batch_size,
        })
    }

    pub async fn new(pool: PostgresPool) -> Result<Self, String> {
        Ok(Self {
            pool,
            drop_prefix_delete_batch_size: 1024,
        })
    }

    fn namespace(namespace: IndexedStorageNamespace) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog { worker_id: _ } => "worker-oplog".to_string(),
            IndexedStorageNamespace::CompressedOpLog {
                worker_id: _,
                level,
            } => {
                format!("worker-c{level}-oplog")
            }
        }
    }

    fn meta_namespace(namespace: IndexedStorageMetaNamespace) -> String {
        match namespace {
            IndexedStorageMetaNamespace::Oplog => "worker-oplog".to_string(),
            IndexedStorageMetaNamespace::CompressedOplog { level } => {
                format!("worker-c{level}-oplog")
            }
        }
    }
}

#[async_trait]
impl IndexedStorage for PostgresIndexedStorage {
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
            "SELECT EXISTS(SELECT 1 FROM index_storage WHERE namespace = $1 AND key = $2);",
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
        namespace: IndexedStorageMetaNamespace,
        pattern: &str,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), String> {
        let key = pattern.replace('*', "%").replace('?', "_");
        let query = sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT key FROM index_storage WHERE namespace = $1 AND key LIKE $2 ORDER BY key LIMIT $3 OFFSET $4;",
        )
        .bind(Self::meta_namespace(namespace))
        .bind(&key)
        .bind(count as i64)
        .bind(cursor as i64);

        let keys = self
            .pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<(String,), _>(query)
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
        value: Vec<u8>,
    ) -> Result<(), String> {
        let query = sqlx::query(
            "INSERT INTO index_storage (namespace, key, id, value) VALUES ($1, $2, $3, $4);",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(id as i64)
        .bind(value);

        self.pool
            .with_rw(svc_name, api_name)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        pairs: Vec<(u64, Vec<u8>)>,
    ) -> Result<(), String> {
        if pairs.is_empty() {
            return Ok(());
        }

        let mut tx = self
            .pool
            .with_rw(svc_name, api_name)
            .begin()
            .await
            .map_err(|err| err.to_safe_string())?;

        for (id, value) in pairs {
            let query = sqlx::query(
                "INSERT INTO index_storage (namespace, key, id, value) VALUES ($1, $2, $3, $4);",
            )
            .bind(Self::namespace(namespace.clone()))
            .bind(key)
            .bind(id as i64)
            .bind(value);

            tx.execute(query)
                .await
                .map_err(|err| err.to_safe_string())?;
        }

        tx.commit().await.map_err(|err| err.to_safe_string())
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, String> {
        let query = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM index_storage WHERE namespace = $1 AND key = $2;",
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
        let query = sqlx::query("DELETE FROM index_storage WHERE namespace = $1 AND key = $2;")
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
    ) -> Result<Vec<(u64, Vec<u8>)>, String> {
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND key = $2 AND id BETWEEN $3 AND $4 ORDER BY id ASC;",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(start_id as i64)
        .bind(end_id as i64);

        self.pool
            .with_ro(svc_name, api_name)
            .fetch_all_as::<DBIdValue, _>(query)
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
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND key = $2 ORDER BY id ASC LIMIT 1;",
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
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND key = $2 ORDER BY id DESC LIMIT 1;",
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
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        let query = sqlx::query_as::<_, DBIdValue>(
            "SELECT id, value FROM index_storage WHERE namespace = $1 AND key = $2 AND id >= $3 ORDER BY id ASC LIMIT 1;",
        )
        .bind(Self::namespace(namespace))
        .bind(key)
        .bind(id as i64);

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
        let namespace = Self::namespace(namespace);
        let mut deleted_rows = self.drop_prefix_delete_batch_size;

        while deleted_rows >= self.drop_prefix_delete_batch_size {
            let query = sqlx::query(
                "DELETE FROM index_storage WHERE ctid IN (SELECT ctid FROM index_storage WHERE namespace = $1 AND key = $2 AND id <= $3 LIMIT $4);",
            )
            .bind(&namespace)
            .bind(key)
            .bind(last_dropped_id as i64)
            .bind(self.drop_prefix_delete_batch_size as i64);

            deleted_rows = self
                .pool
                .with_rw(svc_name, api_name)
                .execute(query)
                .await
                .map_err(|err| err.to_safe_string())?
                .rows_affected();
        }

        Ok(())
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
