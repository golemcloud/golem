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

use crate::db::DBValue;
use crate::db::sqlite::SqlitePool;
use crate::replayable_stream::ErasedReplayableStream;
use crate::repo::RepoError;
use crate::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, blob_file_name_to_string,
    blob_parent_to_string, blob_path_to_string, validate_relative_blob_path,
};
use anyhow::{Error, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::NaiveDateTime;
use futures::TryStreamExt;
use futures::stream::BoxStream;
use std::path::{Path, PathBuf};
use std::pin::Pin;

#[derive(Debug)]
pub struct SqliteBlobStorage {
    pool: SqlitePool,
}

impl SqliteBlobStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, RepoError> {
        let result = Self { pool };
        result.init().await?;
        Ok(result)
    }

    async fn init(&self) -> Result<(), RepoError> {
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
                "#)).await?;
        Ok(())
    }

    fn namespace(namespace: BlobStorageNamespace) -> String {
        match namespace {
            BlobStorageNamespace::CompilationCache { environment_id } => {
                format!("compilation_cache-{environment_id}")
            }
            BlobStorageNamespace::CustomStorage { environment_id } => {
                format!("custom_data-{environment_id}")
            }
            BlobStorageNamespace::OplogPayload {
                environment_id,
                agent_id,
            } => format!("oplog_payload-{environment_id}-{}", agent_id.agent_id),
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                component_id,
                level,
            } => format!("compressed_oplog-{environment_id}-{component_id}-{level}"),
            BlobStorageNamespace::InitialAgentFiles { environment_id } => {
                format!("initial_agent_files-{environment_id}")
            }
            BlobStorageNamespace::Components { environment_id } => {
                format!("components-{environment_id}")
            }
        }
    }

    fn escape_like(value: &str) -> String {
        let mut result = String::with_capacity(value.len());
        for ch in value.chars() {
            match ch {
                '%' | '_' | '!' => {
                    result.push('!');
                    result.push(ch);
                }
                _ => result.push(ch),
            }
        }
        result
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
    ) -> Result<Option<Vec<u8>>, Error> {
        validate_relative_blob_path(path)?;
        let query = sqlx::query_as("SELECT value FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = FALSE;")
            .bind(Self::namespace(namespace))
            .bind(blob_parent_to_string(path)?)
            .bind(blob_file_name_to_string(path)?);

        let result = self
            .pool
            .with_ro(target_label, op_label)
            .fetch_optional_as::<DBValue, _>(query)
            .await
            .map(|r| r.map(|op| op.into_bytes().to_vec()))?;

        Ok(result)
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        validate_relative_blob_path(path)?;
        let result = self
            .get_raw(target_label, op_label, namespace, path)
            .await?;
        Ok(result.map(|bytes| {
            let stream = tokio_stream::once(Ok(Bytes::from(bytes)));
            let boxed: Pin<Box<dyn futures::Stream<Item = Result<Bytes, Error>> + Send>> =
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
    ) -> Result<Option<BlobMetadata>, Error> {
        validate_relative_blob_path(path)?;
        let query = sqlx::query_as(
            "SELECT last_modified_at, size FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ?;",
        )
            .bind(Self::namespace(namespace))
            .bind(blob_parent_to_string(path)?)
            .bind(blob_file_name_to_string(path)?);

        let result = self
            .pool
            .with_ro(target_label, op_label)
            .fetch_optional_as::<DBMetadata, _>(query)
            .await?
            .map(|r| r.into_blob_metadata().map_err(|e| anyhow!(e)))
            .transpose()?;

        Ok(result)
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error> {
        validate_relative_blob_path(path)?;
        let size = data.len() as i64;
        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, ?, ?, FALSE)
                        ON CONFLICT(namespace, parent, name) DO UPDATE SET value = excluded.value, size = excluded.size, last_modified_at = CURRENT_TIMESTAMP;
                    "#,
                )
                    .bind(Self::namespace(namespace))
                    .bind(blob_parent_to_string(path)?)
                    .bind(blob_file_name_to_string(path)?)
                    .bind(data)
                    .bind(size);

        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await?;

        Ok(())
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        validate_relative_blob_path(path)?;
        let data = stream
            .make_stream_erased()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let data = Bytes::from(data.concat());
        self.put_raw(target_label, op_label, namespace, path, &data)
            .await?;
        Ok(())
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        validate_relative_blob_path(path)?;
        let query = sqlx::query(
            "DELETE FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ?;",
        )
        .bind(Self::namespace(namespace))
        .bind(blob_parent_to_string(path)?)
        .bind(blob_file_name_to_string(path)?);
        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await?;

        Ok(())
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        validate_relative_blob_path(path)?;
        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, NULL, 0, TRUE)
                        ON CONFLICT(namespace, parent, name) DO UPDATE SET is_directory = TRUE, value = NULL, size = 0;
                    "#
                )
                .bind(Self::namespace(namespace))
                .bind(blob_parent_to_string(path)?)
                .bind(blob_file_name_to_string(path)?);

        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await?;

        Ok(())
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        validate_relative_blob_path(path)?;
        let query =
            sqlx::query_as("SELECT name FROM blob_storage WHERE namespace = ? AND parent = ?;")
                .bind(Self::namespace(namespace))
                .bind(blob_path_to_string(path)?);

        let result = self
            .pool
            .with_ro(target_label, op_label)
            .fetch_all_as::<(String,), _>(query)
            .await
            .map(|r| r.into_iter().map(|row| path.join(row.0)).collect())?;

        Ok(result)
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        validate_relative_blob_path(path)?;

        if path.as_os_str().is_empty() {
            return Ok(false);
        }

        let parent = blob_parent_to_string(path)?;
        let name = blob_file_name_to_string(path)?;

        let exists_query = sqlx::query_as::<_, (i64,)>(
            "SELECT 1 FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = TRUE LIMIT 1;",
        )
        .bind(Self::namespace(namespace.clone()))
        .bind(parent.clone())
        .bind(name.clone());

        let exists = self
            .pool
            .with_ro(target_label, op_label)
            .fetch_optional_as(exists_query)
            .await?
            .is_some();

        if !exists {
            return Ok(false);
        }

        let dir_path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        let descendants_prefix = format!("{}/", dir_path);
        let descendants_like = format!("{}%", Self::escape_like(&descendants_prefix));

        let query = sqlx::query(
            r#"DELETE FROM blob_storage WHERE namespace = ? AND
                     ((parent = ? AND name = ? AND is_directory = TRUE) OR (parent = ?) OR (parent LIKE ? ESCAPE '!'));
            "#,
        )
        .bind(Self::namespace(namespace))
        .bind(parent)
        .bind(name)
        .bind(dir_path)
        .bind(descendants_like);

        let result = self
            .pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await
            .map(|result| result.rows_affected() > 0)?;

        Ok(result)
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        validate_relative_blob_path(path)?;
        let query = sqlx::query_as(
            "SELECT is_directory FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? LIMIT 1;",
        )
        .bind(Self::namespace(namespace))
        .bind(blob_parent_to_string(path)?)
        .bind(blob_file_name_to_string(path)?);

        let result = self
            .pool
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
            })?;

        Ok(result)
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
