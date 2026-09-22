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
//! for at most a deadline.

use bytes::Bytes;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use rustic_core::{
    BytesList, ErrorKind, FileType, Id, ReadBackend, RusticError, RusticResult, WriteBackend,
};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;

/// The target label of each blob storage call of the backend.
const TARGET_LABEL: &str = "filesystem_snapshot";

/// The path of the config file of a repository.
const CONFIG_PATH: &str = "config";

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
/// The backend holds a handle of the async runtime and a deadline, which the caller gives when it
/// makes the backend. Each call waits for the blob storage with `Handle::block_on` on that runtime.
/// Thus a thread that is not a thread of the runtime can call the backend, for example a thread of
/// rustic. A call from a task of the runtime panics, because `Handle::block_on` panics in an async
/// context. The executor builds with `panic = "abort"`, so that panic stops the executor. Thus the
/// store runs rustic only through `execute_native`, which runs rustic on a blocking thread.
///
/// A call that gets no answer from the blob storage within the deadline gives an error. The
/// runtime must be a multi-thread runtime, because on a `current_thread` runtime `Handle::block_on`
/// does not drive the timer of the deadline.
#[derive(Debug)]
pub(super) struct BlobBackend {
    storage: Arc<dyn BlobStorage>,
    namespace: BlobStorageNamespace,
    runtime: Handle,
    deadline: Duration,
}

impl BlobBackend {
    /// Makes a backend over the namespace of the storage. Each call waits on `runtime`, for at most
    /// `deadline`.
    pub(super) fn new(
        storage: Arc<dyn BlobStorage>,
        namespace: BlobStorageNamespace,
        runtime: Handle,
        deadline: Duration,
    ) -> Self {
        Self {
            storage,
            namespace,
            runtime,
            deadline,
        }
    }

    /// Waits for one call on the blob storage, and gives its result as a rustic result.
    ///
    /// Each call of the backend on the blob storage goes through this function. A call that gives
    /// no answer within the deadline gives an error, the same as a call that failed.
    fn request<T>(
        &self,
        call: StorageCall,
        path: &Path,
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> RusticResult<T> {
        self.runtime
            .block_on(answer_within(self.deadline, future))
            .map_err(|error| storage_error(call, path, error))
    }
}

/// Gives the output of the future, or an error when the future gives no output within the deadline.
///
/// The timer starts at the first poll of the returned future, in the runtime of that poll. Thus a
/// thread without a runtime context can wait for the result with `Handle::block_on`. A future that
/// is ready at its first poll always gives its output. At the deadline, the function drops the
/// future and gives an error whose root cause is tokio's `Elapsed`.
async fn answer_within<T>(
    deadline: Duration,
    future: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(deadline, future)
        .await
        .unwrap_or_else(|elapsed| {
            Err(anyhow::Error::new(elapsed).context(format!(
                "the blob storage gave no answer within {deadline:?}"
            )))
        })
}

impl ReadBackend for BlobBackend {
    fn location(&self) -> String {
        format!("golem-blob-storage:{:?}", self.namespace)
    }

    fn list_with_size(&self, tpe: FileType) -> RusticResult<Vec<(Id, u32)>> {
        match tpe {
            FileType::Config => {
                let path = Path::new(CONFIG_PATH);
                self.request(
                    StorageCall::Stat,
                    path,
                    self.storage.get_metadata(
                        TARGET_LABEL,
                        StorageCall::Stat.label(),
                        self.namespace.clone(),
                        path,
                    ),
                )?
                .map(|metadata| file_size(path, metadata.size).map(|size| (Id::default(), size)))
                .into_iter()
                .collect()
            }
            _ => {
                let directory = Path::new(tpe.dirname());
                self.request(
                    StorageCall::List,
                    directory,
                    self.storage.list_blobs_below(
                        TARGET_LABEL,
                        StorageCall::List.label(),
                        self.namespace.clone(),
                        directory,
                    ),
                )?
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
        self.request(
            StorageCall::Read,
            &path,
            self.storage.get_raw(
                TARGET_LABEL,
                StorageCall::Read.label(),
                self.namespace.clone(),
                &path,
            ),
        )?
        .map(Bytes::from)
        .ok_or_else(|| missing_file(&path))
    }

    fn read_partial(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        offset: u32,
        length: u32,
    ) -> RusticResult<Bytes> {
        let path = file_path(tpe, id)?;
        let Some(last) = length.checked_sub(1) else {
            return Ok(Bytes::new());
        };
        let start = u64::from(offset);
        self.request(
            StorageCall::ReadRange,
            &path,
            self.storage.get_raw_slice(
                TARGET_LABEL,
                StorageCall::ReadRange.label(),
                self.namespace.clone(),
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
        let joined;
        let data: &[u8] = match parts.as_slice() {
            [part] => part,
            parts => {
                joined = join(parts);
                &joined
            }
        };
        self.request(
            StorageCall::Write,
            &path,
            self.storage.put_raw(
                TARGET_LABEL,
                StorageCall::Write.label(),
                self.namespace.clone(),
                &path,
                data,
            ),
        )
    }

    fn remove(&self, tpe: FileType, id: &Id, _cacheable: bool) -> RusticResult<()> {
        let path = file_path(tpe, id)?;
        self.request(
            StorageCall::Delete,
            &path,
            self.storage.delete(
                TARGET_LABEL,
                StorageCall::Delete.label(),
                self.namespace.clone(),
                &path,
            ),
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

/// The error of a file that the blob storage does not hold.
fn missing_file(path: &Path) -> Box<RusticError> {
    RusticError::new(
        ErrorKind::Backend,
        "The blob storage holds no file at `{path}`.",
    )
    .attach_context("path", path.display().to_string())
}

/// The error of a blob storage call that failed.
fn storage_error(call: StorageCall, path: &Path, error: anyhow::Error) -> Box<RusticError> {
    RusticError::with_source(
        ErrorKind::Backend,
        "The blob storage call `{call}` failed at `{path}`.",
        error,
    )
    .attach_context("call", call.label())
    .attach_context("path", path.display().to_string())
}

#[cfg(test)]
mod tests;
