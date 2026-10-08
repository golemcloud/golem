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

//! A rustic backend over the blob storage of the executor.
//!
//! The backend keeps the files of one repository in one blob storage namespace, with the paths of
//! the restic repository format. rustic calls the backend from threads outside the async runtime,
//! and each call waits for the blob storage on the runtime that the backend holds. Each call waits
//! for at most a deadline, and a cancelled operation makes no more calls.

use super::fault::{BlobCallFailed, ConfigExists, FileMissing};
use super::files::SnapshotFiles;
use super::publish::{SnapshotStage, StagedSnapshot};
use bytes::Bytes;
use futures::StreamExt as _;
use golem_service_base::storage::blob::{BlobRangeError, PutIfAbsent};
use kept::KeptPacks;
use rustic_core::{
    BytesList, ErrorKind, FileType, Id, ReadBackend, RusticError, RusticResult, WriteBackend,
};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::runtime::Handle;
use tokio_util::task::task_tracker::TaskTrackerToken;

/// The path of the config file of a repository.
pub(super) const CONFIG_PATH: &str = "config";

/// The largest number of bytes of tree packs that the backend of one operation of the store keeps
/// in memory.
pub(super) const KEPT_PACKS_LIMIT: usize = 32 * 1024 * 1024;

/// The largest number of reads ahead of one call of [`BlobBackend::read_ahead`] that run at the
/// same time.
pub(super) const SNAPSHOT_FILE_READS: usize = 32;

/// The largest number of reads ahead of a store that run at the same time, over all its
/// operations. It bounds the reads of a recovery that starts many agents at once.
pub(super) const MAX_SNAPSHOT_FILE_READS: usize = 128;

/// A call that the backend makes on the blob storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageCall {
    Stat,
    List,
    Read,
    ReadRange,
    Write,
    Delete,
}

impl StorageCall {
    /// Gives the operation label of the call.
    fn label(self) -> &'static str {
        match self {
            Self::Stat => "stat",
            Self::List => "list",
            Self::Read => "read",
            Self::ReadRange => "read_range",
            Self::Write => "write",
            Self::Delete => "delete",
        }
    }
}

/// A rustic backend that keeps the files of one repository in one blob storage namespace.
///
/// A file of the type `Config` is at `config`. A pack is at `data/<two hex digits>/<id>`, where
/// the two digits are the start of the id. Each other file is at `<directory of the type>/<id>`.
/// These are the paths of the restic repository format.
///
/// The backend holds a handle of the async runtime and the blobs of the scope, which the caller
/// gives when it makes the backend. Each call goes through those blobs, with their deadline, token
/// and lease, and waits for them with `Handle::block_on` on that runtime.
/// Thus a thread that is not a thread of the runtime can call the backend, for example a thread of
/// rustic. A call from a task of the runtime panics, because `Handle::block_on` panics in an async
/// context. The executor builds with `panic = "abort"`, so that panic stops the executor. Thus the
/// store runs rustic only through `execute_native`, which runs rustic on a blocking thread.
///
/// A call that gets no answer from the blob storage within the deadline gives an error. The
/// runtime must be a multi-thread runtime, because on a `current_thread` runtime `Handle::block_on`
/// does not drive the timer of the deadline.
///
/// The config file and the index files are written only when their path has no blob. A snapshot
/// file goes into the stage of the backend when it has one, and the backend does not write it.
#[derive(Debug)]
pub(super) struct BlobBackend {
    files: SnapshotFiles,
    runtime: Handle,
    stage: Option<Arc<SnapshotStage>>,
    /// Counts the backend as work of a tracker, until the last owner drops the backend.
    _tracked: Option<TaskTrackerToken>,
    /// The packs of tree blobs that the operation of the backend read.
    kept: KeptPacks,
    /// The files that [`BlobBackend::read_ahead`] read and no read of rustic took yet, by path.
    read_ahead: Mutex<HashMap<Box<Path>, Bytes>>,
    /// The paths of the index files that a listing of the backend gave.
    index_listed: Mutex<HashSet<Box<Path>>>,
    /// The slots of the reads ahead, which the backends of one store share.
    read_slots: Arc<tokio::sync::Semaphore>,
}

impl BlobBackend {
    /// Makes a backend over the blobs. Each call goes through `files` and waits on `runtime`. The
    /// backend keeps tree packs up to `kept_limit` bytes, and has
    /// [`MAX_SNAPSHOT_FILE_READS`] slots of reads ahead of its own.
    pub(super) fn new(files: SnapshotFiles, runtime: Handle, kept_limit: usize) -> Self {
        Self {
            files,
            runtime,
            stage: None,
            _tracked: None,
            kept: KeptPacks::new(kept_limit),
            read_ahead: Mutex::default(),
            index_listed: Mutex::default(),
            read_slots: Arc::new(tokio::sync::Semaphore::new(MAX_SNAPSHOT_FILE_READS)),
        }
    }

    /// Gives the backend with the slots of reads ahead `slots`, which it shares with the other
    /// backends that got them.
    pub(super) fn reading_under(self, slots: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            read_slots: slots,
            ..self
        }
    }

    /// Gives the paths of the index files that a listing of the backend gave since the last call,
    /// and forgets them.
    pub(super) fn take_index_listed(&self) -> HashSet<Box<Path>> {
        std::mem::take(
            &mut *self
                .index_listed
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Reads the files of the type `tpe` with `ids` ahead, at most [`SNAPSHOT_FILE_READS`] at a
    /// time and at most one for each of its slots of reads ahead, and keeps them until a read of
    /// rustic takes each one. A read that finds no file keeps nothing, so the read of rustic gives
    /// its own answer. After the first read that fails, no new read starts; the call returns when
    /// every read in flight has ended, with the error of the first read that failed.
    pub(super) fn read_ahead(
        &self,
        tpe: FileType,
        ids: impl IntoIterator<Item = Id>,
    ) -> RusticResult<()> {
        let paths = ids
            .into_iter()
            .filter_map(|id| file_path(tpe, &id).ok())
            .collect::<Vec<_>>();
        let failed = AtomicBool::new(false);
        let failed = &failed;
        let read = self.runtime.block_on(
            futures::stream::iter(paths)
                .map(|path| async move {
                    if failed.load(Ordering::SeqCst) {
                        return Ok(None);
                    }
                    let Ok(_slot) = self.read_slots.acquire().await else {
                        return Ok(None);
                    };
                    if failed.load(Ordering::SeqCst) {
                        return Ok(None);
                    }
                    match self.files.get(StorageCall::Read.label(), &path).await {
                        Ok(content) => Ok(content.map(|content| (path, Bytes::from(content)))),
                        Err(error) => {
                            failed.store(true, Ordering::SeqCst);
                            Err(storage_error(StorageCall::Read, &path, error))
                        }
                    }
                })
                .buffer_unordered(SNAPSHOT_FILE_READS)
                .collect::<Vec<_>>(),
        );
        let read = read.into_iter().collect::<RusticResult<Vec<_>>>()?;
        self.read_ahead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(read.into_iter().flatten());
        Ok(())
    }

    /// Gives the backend with a stage for the snapshot file of a save.
    pub(super) fn staging_in(self, stage: Arc<SnapshotStage>) -> Self {
        Self {
            stage: Some(stage),
            ..self
        }
    }

    /// Gives the backend with a token of a task tracker. The tracker counts the backend until the
    /// last owner drops it, for example a thread of rustic.
    pub(super) fn tracked_by(self, token: TaskTrackerToken) -> Self {
        Self {
            _tracked: Some(token),
            ..self
        }
    }

    /// Waits for one call on the blobs, which each call of the backend goes through. A failed call
    /// gives the error of the call at the path.
    fn request<T>(
        &self,
        call: StorageCall,
        path: &Path,
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> RusticResult<T> {
        self.runtime
            .block_on(future)
            .map_err(|error| storage_error(call, path, error))
    }

    /// Writes the content at the path only when the path has no blob, and gives whether it wrote.
    fn write_if_absent(&self, path: &Path, content: &[u8]) -> RusticResult<PutIfAbsent> {
        self.request(
            StorageCall::Write,
            path,
            self.files
                .put_if_absent(StorageCall::Write.label(), path, content),
        )
    }
}

impl ReadBackend for BlobBackend {
    fn location(&self) -> String {
        self.files.location()
    }

    fn list_with_size(&self, tpe: FileType) -> RusticResult<Vec<(Id, u32)>> {
        match tpe {
            FileType::Config => {
                let path = Path::new(CONFIG_PATH);
                self.request(
                    StorageCall::Stat,
                    path,
                    self.files.stat(StorageCall::Stat.label(), path),
                )?
                .map(|metadata| file_size(path, metadata.size).map(|size| (Id::default(), size)))
                .into_iter()
                .collect()
            }
            _ => {
                let directory = Path::new(tpe.dirname());
                let listed = self.request(
                    StorageCall::List,
                    directory,
                    self.files.list_below(StorageCall::List.label(), directory),
                )?;
                if tpe == FileType::Index {
                    self.index_listed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend(listed.iter().map(|blob| blob.path.clone()));
                }
                listed
                    .iter()
                    .filter_map(|blob| {
                        blob.path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .and_then(|name| Id::parse_some(name, tpe))
                            .map(|id| (id, blob))
                    })
                    .map(|(id, blob)| file_size(&blob.path, blob.size).map(|size| (id, size)))
                    .collect()
            }
        }
    }

    fn read_full(&self, tpe: FileType, id: &Id) -> RusticResult<Bytes> {
        let path = file_path(tpe, id)?;
        let read_ahead = self
            .read_ahead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&path);
        if let Some(content) = read_ahead {
            return Ok(content);
        }
        self.request(
            StorageCall::Read,
            &path,
            self.files.get(StorageCall::Read.label(), &path),
        )?
        .map(Bytes::from)
        .ok_or_else(|| missing_file(&path))
    }

    fn read_partial(
        &self,
        tpe: FileType,
        id: &Id,
        cacheable: bool,
        offset: u32,
        length: u32,
    ) -> RusticResult<Bytes> {
        let path = file_path(tpe, id)?;
        let Some(last) = length.checked_sub(1) else {
            return Ok(Bytes::new());
        };
        // rustic marks the reads of tree blobs as cacheable, and reads each tree blob on its own.
        // A pack of tree blobs is read whole and kept, until the kept packs fill their limit or a
        // pack that was read whole does not fit. After that, a pack that is not kept is read by its
        // range, as each other blob.
        if cacheable
            && tpe == FileType::Pack
            && let Some(pack) = self.kept.get_or_read(id, || self.read_full(tpe, id))
        {
            return range_of(&pack?, &path, offset, last);
        }
        let start = u64::from(offset);
        self.request(
            StorageCall::ReadRange,
            &path,
            self.files.get_slice(
                StorageCall::ReadRange.label(),
                &path,
                start,
                start + u64::from(last),
            ),
        )?
        .map(Bytes::from)
        .ok_or_else(|| missing_file(&path))
    }

    fn warmup_path(&self, tpe: FileType, id: &Id) -> String {
        file_path(tpe, id)
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }
}

impl WriteBackend for BlobBackend {
    fn write_bytes(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        content: BytesList,
    ) -> RusticResult<()> {
        let path = file_path(tpe, id)?;
        let parts = content.into_vec();
        let content = match parts.as_slice() {
            [part] => part.clone(),
            parts => Bytes::from(join(parts)),
        };
        match (tpe, &self.stage) {
            (FileType::Snapshot, Some(stage)) => stage
                .keep(StagedSnapshot {
                    path: Arc::from(path),
                    content,
                })
                .map_err(|staged| second_snapshot(&staged.path)),
            (FileType::Config, _) => match self.write_if_absent(&path, &content)? {
                PutIfAbsent::Written => Ok(()),
                PutIfAbsent::AlreadyExists => Err(config_exists(&path)),
            },
            // The name of an index file is the hash of its content, so a blob at the path holds
            // the same content.
            (FileType::Index, _) => self.write_if_absent(&path, &content).map(|_| ()),
            _ => self.request(
                StorageCall::Write,
                &path,
                self.files.put(StorageCall::Write.label(), &path, &content),
            ),
        }
    }

    fn remove(&self, tpe: FileType, id: &Id, _cacheable: bool) -> RusticResult<()> {
        let path = file_path(tpe, id)?;
        self.request(
            StorageCall::Delete,
            &path,
            self.files.delete(StorageCall::Delete.label(), &path),
        )
    }
}

/// Gives the path of a file of the repository, relative to the root of the namespace.
fn file_path(tpe: FileType, id: &Id) -> RusticResult<Box<Path>> {
    let hex = id.to_hex();
    let name = hex.as_str();
    let path = match tpe {
        FileType::Config => CONFIG_PATH.to_string(),
        FileType::Pack => {
            let prefix = name.get(..2).ok_or_else(|| {
                RusticError::new(
                    ErrorKind::Internal,
                    "The id `{id}` has fewer than two hex digits.",
                )
                .attach_context("id", name)
            })?;
            format!("{}/{prefix}/{name}", tpe.dirname())
        }
        FileType::Index | FileType::Key | FileType::Snapshot => {
            format!("{}/{name}", tpe.dirname())
        }
    };
    Ok(PathBuf::from(path).into_boxed_path())
}

/// Gives the size of the file at the path as a rustic file size, which has 32 bits.
fn file_size(path: &Path, size: u64) -> RusticResult<u32> {
    u32::try_from(size).map_err(|_| {
        RusticError::new(
            ErrorKind::Backend,
            "The file `{path}` has `{size}` bytes, which is more than a rustic file can have.",
        )
        .attach_context("path", path.display().to_string())
        .attach_context("size", size.to_string())
    })
}

/// Gives the bytes of the parts one after the other, in one allocation of the final size.
fn join(parts: &[Bytes]) -> Box<[u8]> {
    parts
        .iter()
        .fold(
            Vec::with_capacity(parts.iter().map(Bytes::len).sum()),
            |mut joined, part| {
                joined.extend_from_slice(part);
                joined
            },
        )
        .into_boxed_slice()
}

/// Gives the bytes from `offset` to `last` of the pack as a slice of the pack. A range outside the
/// pack gives the error of a ranged read outside a blob.
fn range_of(pack: &Bytes, path: &Path, offset: u32, last: u32) -> RusticResult<Bytes> {
    let start = u64::from(offset);
    let end = start + u64::from(last);
    usize::try_from(start)
        .ok()
        .zip(usize::try_from(end).ok())
        .filter(|(_, end)| *end < pack.len())
        .map(|(start, end)| pack.slice(start..=end))
        .ok_or_else(|| {
            storage_error(
                StorageCall::ReadRange,
                path,
                anyhow::Error::new(BlobRangeError { start, end }),
            )
        })
}

/// The error of a file that the blob storage does not hold.
fn missing_file(path: &Path) -> Box<RusticError> {
    RusticError::with_source(
        ErrorKind::Backend,
        "The blob storage holds no file at `{path}`.",
        FileMissing { path: path.into() },
    )
    .attach_context("path", path.display().to_string())
}

/// The error of a config file that another writer made first.
fn config_exists(path: &Path) -> Box<RusticError> {
    RusticError::with_source(
        ErrorKind::Backend,
        "The blob storage already holds the config file `{path}`.",
        ConfigExists,
    )
    .attach_context("path", path.display().to_string())
}

/// The error of a second snapshot file in one stage.
fn second_snapshot(path: &Path) -> Box<RusticError> {
    RusticError::new(
        ErrorKind::Internal,
        "The stage already holds a snapshot file, so it cannot keep `{path}`.",
    )
    .attach_context("path", path.display().to_string())
}

/// The error of a blob storage call that failed.
fn storage_error(call: StorageCall, path: &Path, error: anyhow::Error) -> Box<RusticError> {
    RusticError::with_source(
        ErrorKind::Backend,
        "The blob storage call `{call}` failed at `{path}`.",
        BlobCallFailed::new(error),
    )
    .attach_context("call", call.label())
    .attach_context("path", path.display().to_string())
}

mod kept;

#[cfg(test)]
mod tests;
