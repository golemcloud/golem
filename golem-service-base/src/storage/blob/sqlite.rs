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
use crate::repo::RepoError;
use crate::storage::blob::{
    BLOB_STREAM_CHUNK_SIZE, BlobMetadata, BlobRangeStream, BlobStorageBackend,
    BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath, PutIfAbsent,
    agent_path_segment, blob_child_path, validate_range,
};
use anyhow::{Error, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::NaiveDateTime;
use sqlx::Connection;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::ptr::NonNull;

struct ReadBlob<'a> {
    raw: NonNull<libsqlite3_sys::sqlite3_blob>,
    _connection: PhantomData<&'a mut ()>,
}

impl<'a> ReadBlob<'a> {
    fn open(
        handle: &'a mut sqlx::sqlite::LockedSqliteHandle<'_>,
        rowid: i64,
    ) -> Result<Self, Error> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: SQLx excludes concurrent connection access while locked. The returned
        // blob borrows that lock, and all names are static, NUL-terminated C strings.
        let result = unsafe {
            libsqlite3_sys::sqlite3_blob_open(
                handle.as_raw_handle().as_ptr(),
                c"main".as_ptr(),
                c"blob_storage".as_ptr(),
                c"value".as_ptr(),
                rowid,
                0,
                &mut raw,
            )
        };
        anyhow::ensure!(
            result == libsqlite3_sys::SQLITE_OK,
            "SQLite blob open failed ({result})"
        );
        Ok(Self {
            raw: NonNull::new(raw).ok_or_else(|| anyhow!("SQLite returned no blob"))?,
            _connection: PhantomData,
        })
    }

    fn size(&self) -> u64 {
        // SAFETY: the blob and its exclusively locked connection remain alive.
        unsafe { libsqlite3_sys::sqlite3_blob_bytes(self.raw.as_ptr()) as u64 }
    }

    fn read(&mut self, offset: u64, length: usize) -> Result<Bytes, Error> {
        let offset = i32::try_from(offset)?;
        let length_i32 = i32::try_from(length)?;
        let mut bytes = vec![0; length];
        // SAFETY: the live blob is exclusively owned, the output buffer has length bytes,
        // and SQLite checks the offset/length against the blob size.
        let result = unsafe {
            libsqlite3_sys::sqlite3_blob_read(
                self.raw.as_ptr(),
                bytes.as_mut_ptr().cast(),
                length_i32,
                offset,
            )
        };
        anyhow::ensure!(
            result == libsqlite3_sys::SQLITE_OK,
            "SQLite blob read failed ({result})"
        );
        Ok(Bytes::from(bytes))
    }
}

impl Drop for ReadBlob<'_> {
    fn drop(&mut self) {
        // SAFETY: close exactly once, before releasing the borrowed connection lock.
        unsafe {
            libsqlite3_sys::sqlite3_blob_close(self.raw.as_ptr());
        }
    }
}

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
                    PRIMARY KEY (namespace, parent, name, is_directory)   -- A blob and a directory can hold one path
                );
                "#)).await?;
        self.pool
            .with_rw("blob_storage", "init")
            .execute(sqlx::query(
                "CREATE INDEX IF NOT EXISTS blob_storage_directories ON blob_storage (namespace, parent, name) WHERE is_directory = TRUE;",
            ))
            .await?;
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
                fingerprint,
            } => {
                let agent = agent_path_segment(&agent_id);
                let fingerprint = fingerprint.0;
                format!("filesystem_snapshots-{environment_id}-{agent}-{fingerprint}")
            }
        }
    }
}

#[async_trait]
impl BlobStorageBackend for SqliteBlobStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
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

    async fn get_range_stream_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        let namespace = Self::namespace(namespace);
        let parent = path.parent_text()?;
        let name = path.file_name_text()?;
        // Acquire before spawning: the read pool bounds blocking tasks and pinned readers.
        let mut connection = self.pool.acquire_blob_reader().await?;
        let runtime = tokio::runtime::Handle::current();
        let (ready, opened) = tokio::sync::oneshot::channel();
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let reader = tokio::task::spawn_blocking(move || {
            let mut ready = Some(ready);
            let result = (|| -> Result<(), Error> {
                let mut transaction = runtime.block_on(connection.begin())?;
                let rowid: Option<(i64,)> = runtime.block_on(sqlx::query_as(
                    "SELECT rowid FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = FALSE"
                ).bind(namespace).bind(parent).bind(name).fetch_optional(&mut *transaction))?;
                let Some((rowid,)) = rowid else {
                    let _ = ready.take().unwrap().send(Ok(None));
                    return Ok(());
                };
                // Keep one read snapshot from rowid lookup through the last chunk.
                let mut handle = runtime.block_on(transaction.lock_handle())?;
                let mut blob = ReadBlob::open(&mut handle, rowid)?;
                let total_size = blob.size();
                validate_range(offset, length, total_size)?;
                if ready.take().unwrap().send(Ok(Some(total_size))).is_err() {
                    return Ok(());
                }
                let mut position = offset;
                let mut remaining = length;
                while remaining > 0 && !sender.is_closed() {
                    let count = remaining.min(BLOB_STREAM_CHUNK_SIZE as u64) as usize;
                    let bytes = blob.read(position, count)?;
                    if sender.blocking_send(Ok(bytes)).is_err() {
                        break;
                    }
                    position += count as u64;
                    remaining -= count as u64;
                }
                Ok(())
            })();
            if let Err(error) = result {
                if let Some(ready) = ready {
                    let _ = ready.send(Err(error));
                } else {
                    let _ = sender.blocking_send(Err(error));
                }
            }
        });
        let Some(total_size) = opened.await?? else {
            return Ok(None);
        };
        let stream =
            futures::stream::try_unfold((receiver, reader), |(mut receiver, reader)| async {
                match receiver.recv().await {
                    Some(bytes) => Ok(Some((bytes?, (receiver, reader)))),
                    None => {
                        reader.await?;
                        Ok::<_, Error>(None)
                    }
                }
            });
        Ok(Some(BlobRangeStream {
            total_size,
            stream: Box::pin(stream),
        }))
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error> {
        // A blob row sorts before the row of a directory at the same path, so a blob wins.
        let query = sqlx::query_as(
            "SELECT last_modified_at, size FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? ORDER BY is_directory LIMIT 1;",
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

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error> {
        let size = data.len() as i64;
        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, ?, ?, FALSE)
                        ON CONFLICT(namespace, parent, name, is_directory) DO UPDATE SET value = excluded.value, size = excluded.size, last_modified_at = CURRENT_TIMESTAMP;
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

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        // One statement reads the row of the source and writes the row of the target, so the
        // bytes stay in the database. A source with no row inserts nothing.
        let query = sqlx::query(
            r#"
                INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                SELECT ?, ?, ?, value, size, FALSE FROM blob_storage
                WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = FALSE
                ON CONFLICT(namespace, parent, name, is_directory) DO UPDATE SET value = excluded.value, size = excluded.size, last_modified_at = CURRENT_TIMESTAMP;
            "#,
        )
        .bind(Self::namespace(to_namespace))
        .bind(to.parent_text()?)
        .bind(to.file_name_text()?)
        .bind(Self::namespace(from_namespace))
        .bind(from.parent_text()?)
        .bind(from.file_name_text()?);

        let copied = self
            .pool
            .with_rw(target_label, op_label)
            .execute(query)
            .await?
            .rows_affected();
        Ok(copied > 0)
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        let size = data.len() as i64;
        // The primary key holds the path and the kind of the row, so the insert and the check of
        // the key are one statement. A blob row that is there makes the insert change no row, and
        // the row of a directory at the path is another key.
        let query = sqlx::query(
            r#"
                INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                VALUES (?, ?, ?, ?, ?, FALSE)
                ON CONFLICT(namespace, parent, name, is_directory) DO NOTHING;
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

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            "DELETE FROM blob_storage WHERE namespace = ? AND parent = ? AND name = ? AND is_directory = FALSE;",
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

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        let query = sqlx::query(
                    r#"
                        INSERT INTO blob_storage (namespace, parent, name, value, size, is_directory)
                        VALUES (?, ?, ?, NULL, 0, TRUE)
                        ON CONFLICT(namespace, parent, name, is_directory) DO UPDATE SET last_modified_at = CURRENT_TIMESTAMP;
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

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error> {
        let directory = path.text()?;
        let (descendants_start, descendants_end) = descendant_bounds(&directory);

        let query = if directory.is_empty() {
            sqlx::query_as::<_, (String, String)>(LIST_ROOT_DIR).bind(Self::namespace(namespace))
        } else {
            sqlx::query_as::<_, (String, String)>(LIST_DIR)
                .bind(Self::namespace(namespace))
                .bind(directory)
                .bind(descendants_start)
                .bind(descendants_end)
        };

        let result = self
            .pool
            .with_ro(target_label, op_label)
            .fetch_all_as::<(String, String), _>(query)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|(parent, name)| blob_child_path(&parent, &name).into())
                    .collect()
            })?;

        Ok(result)
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error> {
        let directory = path.text()?;

        // The OR keeps SQLite from a search on `parent`, so it searches the primary key for
        // `namespace` and then reads the rows of that namespace.
        let query = if directory.is_empty() {
            sqlx::query_as::<_, (String, String, i64)>(
                "SELECT parent, name, size FROM blob_storage WHERE namespace = ? AND is_directory = FALSE;",
            )
            .bind(Self::namespace(namespace))
        } else {
            let (descendants_start, descendants_end) = descendant_bounds(&directory);
            sqlx::query_as::<_, (String, String, i64)>(
                "SELECT parent, name, size FROM blob_storage WHERE namespace = ? AND is_directory = FALSE AND (parent = ? OR (parent >= ? AND parent < ?));",
            )
            .bind(Self::namespace(namespace))
            .bind(directory)
            .bind(descendants_start)
            .bind(descendants_end)
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

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        let parent = path.parent_text()?;
        let name = path.file_name_text()?;

        // A directory that only holds blobs has no row of its own, because put_raw writes no
        // row for the parent. One statement removes the row of the directory and every row
        // below it, so the number of removed rows tells whether the directory existed. The OR
        // keeps SQLite from a search on `parent`, so it searches the primary key for `namespace`
        // and then reads the rows of that namespace.
        let dir_path = path.text()?;
        let (descendants_start, descendants_end) = descendant_bounds(&dir_path);

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

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error> {
        let namespace = Self::namespace(namespace);
        let parent = path.parent_text()?;
        let name = path.file_name_text()?;

        // A directory that only holds blobs has no row of its own, because put_raw writes no
        // row for the parent. The second condition is the key range that delete_dir removes,
        // so the same rows that make a directory deletable make it exist. One statement gives
        // both answers.
        let dir_path = path.text()?;
        let (descendants_start, descendants_end) = descendant_bounds(&dir_path);

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

/// The rows whose parent is the directory `?2`, and the rows of directories at any depth below it,
/// whose parents are from `?3` to before `?4` (`descendant_bounds`). The first part searches the
/// primary key and the second the index of the rows of directories, so the listing reads no row of
/// a blob that is deeper below the directory. The union gives a path that a blob and a directory
/// hold one time.
const LIST_DIR: &str = r#"SELECT parent, name FROM blob_storage WHERE namespace = ?1 AND parent = ?2
UNION
SELECT parent, name FROM blob_storage WHERE namespace = ?1 AND is_directory = TRUE AND parent >= ?3 AND parent < ?4;"#;

/// [`LIST_DIR`] at the root, where every row of a directory in the namespace is below the
/// directory.
const LIST_ROOT_DIR: &str = r#"SELECT parent, name FROM blob_storage WHERE namespace = ?1 AND parent = ''
UNION
SELECT parent, name FROM blob_storage WHERE namespace = ?1 AND is_directory = TRUE;"#;

/// Gives the bounds of the `parent` of each row below the directory `dir`, at any depth. The
/// first bound is in the range and the second is not.
///
/// Text comparisons use the BINARY collation, so the match is case-sensitive, unlike LIKE, which
/// ignores ASCII case. A parent below `dir` is at least `dir/` and less than `dir0`, because `0`
/// is the character after `/`.
fn descendant_bounds(dir: &str) -> (String, String) {
    (format!("{dir}/"), format!("{dir}0"))
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

#[cfg(test)]
mod tests {
    use super::{LIST_DIR, LIST_ROOT_DIR, SqliteBlobStorage};
    use crate::db::PoolApi;
    use crate::db::sqlite::SqlitePool;
    use golem_common::config::DbSqliteConfig;
    use test_r::test;

    /// Gives the plan of `sql` with the arguments of a listing, one detail per step.
    async fn plan(pool: &SqlitePool, sql: &str, arguments: &[&'static str]) -> Vec<String> {
        let explain = format!("EXPLAIN QUERY PLAN {sql}");
        let query = arguments.iter().fold(
            sqlx::query_as::<_, (i64, i64, i64, String)>(&explain),
            |query, argument| query.bind(*argument),
        );
        pool.with_ro("test", "plan")
            .fetch_all_as::<(i64, i64, i64, String), _>(query)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect()
    }

    /// Each read of the table in a listing searches the key range of the parent, or the index of
    /// the rows of directories, so a listing does not read every row of the namespace.
    #[test]
    async fn a_listing_reads_only_the_rows_below_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let pool = SqlitePool::configured(&DbSqliteConfig {
            database: root.path().join("blobs.db").to_string_lossy().into_owned(),
            max_connections: 1,
            foreign_keys: false,
        })
        .await
        .unwrap();
        SqliteBlobStorage::new(pool.clone()).await.unwrap();

        for (sql, arguments) in [
            (LIST_DIR, &["namespace", "dir", "dir/", "dir0"][..]),
            (LIST_ROOT_DIR, &["namespace"][..]),
        ] {
            let reads = plan(&pool, sql, arguments)
                .await
                .into_iter()
                .filter(|detail| detail.contains("blob_storage"))
                .collect::<Vec<_>>();
            assert!(
                !reads.is_empty()
                    && reads.iter().all(|detail| {
                        detail.starts_with("SEARCH")
                            && (detail.contains("parent")
                                || detail.contains("blob_storage_directories"))
                    }),
                "{sql}: {reads:?}"
            );
        }
    }
}
