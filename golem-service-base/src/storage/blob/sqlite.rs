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

use crate::db::sqlite::SqlitePool;
use crate::db::{DBValue, PoolApi};
use crate::replayable_stream::ErasedReplayableStream;
use crate::repo::RepoError;
use crate::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
    agent_path_segment, blob_child_path, normalized_blob_path,
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
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                let agent = agent_path_segment(&agent_id);
                format!("oplog_payload-{mode}-{environment_id}-{agent}")
            }
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                component_id,
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("compressed_oplog-{mode}-{environment_id}-{component_id}-{level}")
            }
            BlobStorageNamespace::InitialAgentFiles { environment_id } => {
                format!("initial_agent_files-{environment_id}")
            }
            BlobStorageNamespace::Components { environment_id } => {
                format!("components-{environment_id}")
            }
            BlobStorageNamespace::FilesystemSnapshots {
                environment_id,
                agent_id,
            } => {
                let agent = agent_path_segment(&agent_id);
                format!("filesystem_snapshots-{environment_id}-{agent}")
            }
        }
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
        let path = normalized_blob_path(path)?;
        let query = sqlx::query_as("SELECT value FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = FALSE;")
            .bind(Self::namespace(namespace))
            .bind(path.parent_text()?)
            .bind(path.file_name_text()?);

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
        let path = normalized_blob_path(path)?;
        let result = self
            .get_raw(target_label, op_label, namespace, &path)
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
        let path = normalized_blob_path(path)?;
        let query = sqlx::query_as(
            "SELECT last_modified_at, size FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ?;",
        )
            .bind(Self::namespace(namespace))
            .bind(path.parent_text()?)
            .bind(path.file_name_text()?);

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
        let path = normalized_blob_path(path)?;
        let size = data.len() as i64;
        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, ?, ?, FALSE)
                        ON CONFLICT(namespace, parent, name) DO UPDATE SET value = excluded.value, size = excluded.size, last_modified_at = CURRENT_TIMESTAMP;
                    "#,
                )
                    .bind(Self::namespace(namespace))
                    .bind(path.parent_text()?)
                    .bind(path.file_name_text()?)
                    .bind(data)
                    .bind(size);

        self.pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await?;

        Ok(())
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        let path = normalized_blob_path(path)?;
        path.reject_root()?;
        let size = data.len() as i64;
        // The primary key holds the path, so the insert and the check of the key are one
        // statement. A row that is there makes the insert change no row.
        let query = sqlx::query(
            r#"
                INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                VALUES (?, ?, ?, ?, ?, FALSE)
                ON CONFLICT(namespace, parent, name) DO NOTHING;
            "#,
        )
        .bind(Self::namespace(namespace))
        .bind(path.parent_text()?)
        .bind(path.file_name_text()?)
        .bind(data)
        .bind(size);

        let inserted = self
            .pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await?
            .rows_affected();

        Ok(if inserted == 0 {
            PutIfAbsent::AlreadyExists
        } else {
            PutIfAbsent::Written
        })
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let path = normalized_blob_path(path)?;
        let data = stream
            .make_stream_erased()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let data = Bytes::from(data.concat());
        self.put_raw(target_label, op_label, namespace, &path, &data)
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
        let path = normalized_blob_path(path)?;
        let query = sqlx::query(
            "DELETE FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ?;",
        )
        .bind(Self::namespace(namespace))
        .bind(path.parent_text()?)
        .bind(path.file_name_text()?);
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
        let path = normalized_blob_path(path)?;

        if path.is_root() {
            return Ok(());
        }

        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, NULL, 0, TRUE)
                        ON CONFLICT(namespace, parent, name) DO UPDATE SET is_directory = TRUE, value = NULL, size = 0;
                    "#
                )
                .bind(Self::namespace(namespace))
                .bind(path.parent_text()?)
                .bind(path.file_name_text()?);

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
        let path = normalized_blob_path(path)?;
        let query =
            sqlx::query_as("SELECT name FROM blob_storage WHERE namespace = ? AND parent = ?;")
                .bind(Self::namespace(namespace))
                .bind(path.text()?);

        let result = self
            .pool
            .with_ro(target_label, op_label)
            .fetch_all_as::<(String,), _>(query)
            .await
            .map(|r| r.into_iter().map(|row| path.join(row.0)).collect())?;

        Ok(result)
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        let path = normalized_blob_path(path)?;
        let directory = path.text()?;

        // Text comparisons use the BINARY collation, so the match is case-sensitive. A parent
        // below `directory` is at least `directory/` and less than `directory0`, because `0` is
        // the character after `/`. The OR keeps SQLite from a search on `parent`, so it searches
        // the primary key for `namespace` and then reads the rows of that namespace.
        let query = if directory.is_empty() {
            sqlx::query_as::<_, (String, String, i64)>(
                "SELECT parent, name, size FROM blob_storage WHERE namespace = ? AND is_directory = FALSE;",
            )
            .bind(Self::namespace(namespace))
        } else {
            sqlx::query_as::<_, (String, String, i64)>(
                "SELECT parent, name, size FROM blob_storage WHERE namespace = ? AND is_directory = FALSE AND (parent = ? OR (parent >= ? AND parent < ?));",
            )
            .bind(Self::namespace(namespace))
            .bind(directory.clone())
            .bind(format!("{directory}/"))
            .bind(format!("{directory}0"))
        };

        self.pool
            .with_ro(target_label, op_label)
            .fetch_all_as::<(String, String, i64), _>(query)
            .await?
            .into_iter()
            .map(|(parent, name, size)| {
                Ok(ListedBlob {
                    path: blob_child_path(&parent, &name),
                    size: u64::try_from(size)?,
                })
            })
            .collect()
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        let path = normalized_blob_path(path)?;

        if path.is_root() {
            return Ok(false);
        }

        let parent = path.parent_text()?;
        let name = path.file_name_text()?;

        // A directory that only holds blobs has no row of its own, because put_raw writes no
        // row for the parent. One statement removes the row of the directory and every row
        // below it, so the number of removed rows tells whether the directory existed.
        let dir_path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        // Text comparisons use the BINARY collation, so the match is case-sensitive, unlike LIKE,
        // which ignores ASCII case. A parent below `dir_path` is at least `dir_path/` and less
        // than `dir_path0`, because `0` is the character after `/`. The OR keeps SQLite from a
        // search on `parent`, so it searches the primary key for `namespace` and then reads the
        // rows of that namespace.
        let descendants_start = format!("{dir_path}/");
        let descendants_end = format!("{dir_path}0");

        let query = sqlx::query(
            r#"DELETE FROM blob_storage WHERE namespace = ? AND
                     ((parent = ? AND name = ? AND is_directory = TRUE) OR (parent = ?) OR (parent >= ? AND parent < ?));
            "#,
        )
        .bind(Self::namespace(namespace))
        .bind(parent)
        .bind(name)
        .bind(dir_path)
        .bind(descendants_start)
        .bind(descendants_end);

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
        let path = normalized_blob_path(path)?;

        // The root of a namespace is a directory, also when the namespace holds no row.
        if path.is_root() {
            return Ok(ExistsResult::Directory);
        }

        let namespace = Self::namespace(namespace);
        let parent = path.parent_text()?;
        let name = path.file_name_text()?;

        // A directory that only holds blobs has no row of its own, because put_raw writes no
        // row for the parent. The second condition is the key range that delete_dir removes,
        // so the same rows that make a directory deletable make it exist. One statement gives
        // both answers.
        let dir_path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        let descendants_start = format!("{dir_path}/");
        let descendants_end = format!("{dir_path}0");

        let query = sqlx::query_as(
            r#"SELECT
                     EXISTS(SELECT 1 FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = FALSE),
                     EXISTS(SELECT 1 FROM blob_storage WHERE namespace = ? AND
                            ((parent = ? AND name = ? AND is_directory = TRUE) OR (parent = ?) OR (parent >= ? AND parent < ?)));
            "#,
        )
        .bind(namespace.clone())
        .bind(parent.clone())
        .bind(name.clone())
        .bind(namespace)
        .bind(parent)
        .bind(name)
        .bind(dir_path)
        .bind(descendants_start)
        .bind(descendants_end);

        let (is_file, is_directory) = self
            .pool
            .with_ro(target_label, op_label)
            .fetch_one_as::<(bool, bool), _>(query)
            .await?;

        // A blob at the path is a file, also when the path has blobs below it.
        if is_file {
            Ok(ExistsResult::File)
        } else if is_directory {
            Ok(ExistsResult::Directory)
        } else {
            Ok(ExistsResult::DoesNotExist)
        }
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
