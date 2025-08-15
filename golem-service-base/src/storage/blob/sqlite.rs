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

use std::path::{Path, PathBuf};
use std::pin::Pin;

use crate::db::sqlite::SqlitePool;
use crate::db::DBValue;
use crate::replayable_stream::ErasedReplayableStream;
use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::NaiveDateTime;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use golem_common::SafeDisplay;

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
        self.pool.with_rw("blob_storage", "init").execute(sqlx::query(r#"
                CREATE TABLE IF NOT EXISTS blob_storage (
                    namespace TEXT NOT NULL,                              -- 'Bucket' or namespace
                    parent TEXT NOT NULL,                                 -- Parent path
                    name TEXT NOT NULL,                                   -- Name of the entry
                    value BLOB,                                           -- The actual blob data
                    last_modified_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP, -- Metadata: Last modified timestamp
                    size INTEGER NOT NULL,                                -- Metadata: Size of the blob
                    is_directory BOOLEAN DEFAULT FALSE NOT NULL,          -- Flag indicating if the row represents a directory
                    PRIMARY KEY (namespace, parent, name)  -- Composite primary key
                );
                "#)).await.map_err(|err| err.to_safe_string())?;
        Ok(())
    }

    fn namespace(namespace: BlobStorageNamespace) -> String {
        match namespace {
            BlobStorageNamespace::CompilationCache { project_id } => {
                format!("compilation_cache-{project_id}")
            }
            BlobStorageNamespace::CustomStorage { project_id } => {
                format!("custom_data-{project_id}")
            }
            BlobStorageNamespace::OplogPayload {
                project_id,
                worker_id,
            } => format!("oplog_payload-{}-{}", project_id, worker_id.worker_name),
            BlobStorageNamespace::CompressedOplog {
                project_id,
                component_id,
                level,
            } => format!("compressed_oplog-{project_id}-{component_id}-{level}"),
            BlobStorageNamespace::InitialComponentFiles { project_id } => {
                format!("initial_component_files-{project_id}")
            }
            BlobStorageNamespace::Components { project_id } => format!("components-{project_id}"),
            BlobStorageNamespace::PluginWasmFiles { account_id } => {
                format!("plugin_wasm_files-{account_id}")
            }
        }
    }

    fn parent_string(path: &Path) -> String {
        path.parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or("".to_string())
    }

    fn name_string(path: &Path) -> String {
        tracing::info!("Path: {:?}", path);
        path.file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string()
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
        let query = sqlx::query_as("SELECT value FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = FALSE;")
            .bind(Self::namespace(namespace))
            .bind(Self::parent_string(path))
            .bind(Self::name_string(path));

        self.pool
            .with_ro(target_label, op_label)
            .fetch_optional_as::<DBValue, _>(query)
            .await
            .map(|r| r.map(|op| op.into_bytes()))
            .map_err(|err| err.to_safe_string())
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, String>>>, String> {
        let result = self
            .get_raw(target_label, op_label, namespace, path)
            .await?;
        Ok(result.map(|bytes| {
            let stream = tokio_stream::once(Ok(bytes));
            let boxed: Pin<Box<dyn futures::Stream<Item = Result<Bytes, String>> + Send>> =
                Box::pin(stream);
            boxed
        }))
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String> {
        let query = sqlx::query_as(
            "SELECT last_modified_at, size FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ?;",
        )
            .bind(Self::namespace(namespace))
            .bind(Self::parent_string(path))
            .bind(Self::name_string(path));

        self.pool
            .with_ro(target_label, op_label)
            .fetch_optional_as::<DBMetadata, _>(query)
            .await
            .map(|r| r.map(|op| op.into_blob_metadata()))
            .map_err(|err| err.to_safe_string())?
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
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, ?, ?, FALSE)
                        ON CONFLICT(namespace, parent, name) DO UPDATE SET value = excluded.value, size = excluded.size, last_modified_at = CURRENT_TIMESTAMP;
                    "#,
                )
                    .bind(Self::namespace(namespace))
                    .bind(Self::parent_string(path))
                    .bind(Self::name_string(path))
                    .bind(data)
                    .bind(data.len() as i64);

        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Bytes, String>, Error = String>,
    ) -> Result<(), String> {
        let data = stream
            .make_stream_erased()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let data = Bytes::from(data.concat());
        self.put_raw(target_label, op_label, namespace, path, &data)
            .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let query = sqlx::query(
            "DELETE FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ?;",
        )
        .bind(Self::namespace(namespace))
        .bind(Self::parent_string(path))
        .bind(Self::name_string(path));
        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
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
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, NULL, 0, TRUE)
                        ON CONFLICT(namespace, parent, name) DO UPDATE SET is_directory = TRUE, value = NULL, size = 0;
                    "#
                )
                .bind(Self::namespace(namespace))
                .bind(Self::parent_string(path))
                .bind(Self::name_string(path));
        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        let query =
            sqlx::query_as("SELECT name FROM blob_storage WHERE namespace = ? AND parent = ?;")
                .bind(Self::namespace(namespace))
                .bind(path.to_string_lossy().to_string());

        self.pool
            .with_ro(target_label, op_label)
            .fetch_all::<(String,), _>(query)
            .await
            .map(|r| r.into_iter().map(|row| path.join(row.0)).collect())
            .map_err(|err| err.to_safe_string())
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, String> {
        let parent = Self::parent_string(path);
        let name = Self::name_string(path);
        let parent_prefix = format!("{parent}%");

        let query = sqlx::query(
            r#"DELETE FROM blob_storage WHERE namespace = ? AND
                     ((parent = ? AND name = ?) OR (parent LIKE ?));
            "#,
        )
        .bind(Self::namespace(namespace))
        .bind(parent)
        .bind(name)
        .bind(parent_prefix);
        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await
            .map(|result| result.rows_affected() > 0)
            .map_err(|err| err.to_safe_string())
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String> {
        let query = sqlx::query_as(
            "SELECT is_directory FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? LIMIT 1;",
        )
        .bind(Self::namespace(namespace))
        .bind(Self::parent_string(path))
        .bind(Self::name_string(path));

        self.pool
            .with_ro(target_label, op_label)
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
            .map_err(|err| err.to_safe_string())
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
