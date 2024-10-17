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

use std::path::{Path, PathBuf};

use crate::storage::sqlite::DBValue;
use crate::storage::{
    blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult},
    sqlite::SqlitePool,
};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::NaiveDateTime;

#[derive(Debug)]
pub struct SqliteBlobStorage {
    pool: SqlitePool,
}

impl SqliteBlobStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, String> {
        let result = Self { pool };
        result.init().await?;
        Ok(result)
    }

    async fn init(&self) -> Result<(), String> {
        self.pool.execute(sqlx::query(r#"
                CREATE TABLE IF NOT EXISTS blob_storage (
                    namespace TEXT NOT NULL,       -- 'Bucket' or namespace
                    path TEXT NOT NULL,            -- Full path or index within the namespace
                    value BLOB,                    -- The actual blob data
                    last_modified_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP, -- Metadata: Last modified timestamp
                    size INTEGER NOT NULL,         -- Metadata: Size of the blob
                    is_directory BOOLEAN DEFAULT FALSE NOT NULL, -- Flag indicating if the row represents a directory
                    PRIMARY KEY (namespace, path)  -- Composite primary key
                );
                "#)).await?;
        Ok(())
    }

    fn namespace(namespace: BlobStorageNamespace) -> String {
        match namespace {
            BlobStorageNamespace::CompilationCache => "compilation_cache".to_string(),
            BlobStorageNamespace::CustomStorage(account_id) => {
                format!("custom_data-{}", account_id.value)
            }
            BlobStorageNamespace::OplogPayload {
                account_id,
                worker_id,
            } => format!(
                "oplog_payload-{}-{}",
                account_id.value, worker_id.worker_name
            ),
            BlobStorageNamespace::CompressedOplog {
                account_id,
                component_id,
                level,
            } => format!(
                "compressed_oplog-{}-{}-{}",
                account_id.value, component_id, level
            ),
        }
    }

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().to_string()
    }
}

#[async_trait]
impl BlobStorage for SqliteBlobStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Bytes>, String> {
        let query = sqlx::query_as("SELECT value FROM blob_storage WHERE namespace = ? AND path = ? AND is_directory = FALSE;")
                .bind(Self::namespace(namespace))
                .bind(Self::path_string(path));

        self.pool
            .with(target_label, op_label)
            .fetch_optional_as::<DBValue, _>(query)
            .await
            .map(|r| r.map(|op| op.into_bytes()))
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String> {
        let query = sqlx::query_as(
            "SELECT last_modified_at, size FROM blob_storage WHERE namespace = ? AND path = ?;",
        )
        .bind(Self::namespace(namespace))
        .bind(Self::path_string(path));

        self.pool
            .with(target_label, op_label)
            .fetch_optional_as::<DBMetadata, _>(query)
            .await
            .map(|r| r.map(|op| op.into_blob_metadata()))?
            .transpose()
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String> {
        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, path, value, size, is_directory)
                        VALUES (?, ?, ?, ?, FALSE)
                        ON CONFLICT(namespace, path) DO UPDATE SET value = excluded.value, size = excluded.size, last_modified_at = CURRENT_TIMESTAMP;
                    "#,
                )
                    .bind(Self::namespace(namespace))
                    .bind(Self::path_string(path))
                    .bind(data)
                    .bind(data.len() as i64);

        self.pool
            .with(target_label, op_label)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let query = sqlx::query("DELETE FROM blob_storage WHERE namespace = ?  AND path = ?;")
            .bind(Self::namespace(namespace))
            .bind(Self::path_string(path));
        self.pool
            .with(target_label, op_label)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, path, value, size, is_directory)
                        VALUES (?, ?, NULL, 0, TRUE)
                        ON CONFLICT(namespace, path) DO UPDATE SET is_directory = TRUE, value = NULL, size = 0;
                    "#
                )
                .bind(Self::namespace(namespace))
                .bind(Self::path_string(path));
        self.pool
            .with(target_label, op_label)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        let path = Self::path_string(path);
        let path_like = if path.ends_with("/") || path.is_empty() {
            format!("{path}%")
        } else {
            format!("{path}/%")
        };
        let query = sqlx::query_as(
            "SELECT path FROM blob_storage WHERE namespace = ? AND path LIKE ? AND path <> ?;",
        )
        .bind(Self::namespace(namespace))
        .bind(&path_like)
        .bind(path);

        self.pool
            .with(target_label, op_label)
            .fetch_all::<(String,), _>(query)
            .await
            .map(|r| r.into_iter().map(|row| PathBuf::from(row.0)).collect())
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let path_like = format!("{}%", Self::path_string(path));
        let query = sqlx::query("DELETE FROM blob_storage WHERE namespace = ?  AND path LIKE ?;")
            .bind(Self::namespace(namespace))
            .bind(path_like);
        self.pool
            .with(target_label, op_label)
            .execute(query)
            .await
            .map(|_| ())
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String> {
        let query = sqlx::query_as(
            "SELECT is_directory FROM blob_storage WHERE namespace = ? AND path = ? LIMIT 1;",
        )
        .bind(Self::namespace(namespace))
        .bind(Self::path_string(path));

        self.pool
            .with(target_label, op_label)
            .fetch_optional_as(query)
            .await
            .map(|row| {
                if let Some((is_directory,)) = row {
                    if is_directory {
                        ExistsResult::Directory
                    } else {
                        ExistsResult::File
                    }
                } else {
                    ExistsResult::DoesNotExist
                }
            })
    }
}

#[derive(sqlx::FromRow)]
struct DBMetadata {
    last_modified_at: NaiveDateTime,
    size: i64,
}
impl DBMetadata {
    pub const ISO_8601_FORMAT: &'static str = "%Y-%m-%dT%H:%M:%S";
    fn into_blob_metadata(self) -> Result<BlobMetadata, String> {
        let str = self
            .last_modified_at
            .format(Self::ISO_8601_FORMAT)
            .to_string();
        str.parse().map(|last_modified_at| BlobMetadata {
            last_modified_at,
            size: self.size as u64,
        })
    }
}
