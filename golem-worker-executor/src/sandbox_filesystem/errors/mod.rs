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

use crate::blocking::BlockingTaskError;
use std::fmt::{Debug, Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilesystemStorageErrorKind {
    General,
    AllocationUnsupported,
    SandboxEscape,
}

pub struct FilesystemStorageError {
    inner: Box<FilesystemStorageErrorInner>,
}

/// The facts of one [`FilesystemStorageError`]. They stay behind one box, so that the error is one
/// pointer wide.
struct FilesystemStorageErrorInner {
    operation: &'static str,
    path: PathBuf,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
    task_failed: bool,
    kind: FilesystemStorageErrorKind,
}

impl FilesystemStorageError {
    pub(crate) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        // cap-primitives exposes its escape refusal as a private string payload. OS permission
        // failures retain their errno and must not be classified as sandbox refusals.
        let kind = if source.kind() == std::io::ErrorKind::PermissionDenied
            && source.raw_os_error().is_none()
            && source.to_string() == "a path led outside of the filesystem"
        {
            FilesystemStorageErrorKind::SandboxEscape
        } else {
            FilesystemStorageErrorKind::General
        };
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: Some(source),
                cleanup_failed: false,
                task_failed: false,
                kind,
            }),
        }
    }

    pub(crate) fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: None,
                cleanup_failed: false,
                task_failed: false,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    pub(crate) fn allocation_unsupported(path: &Path) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation: "observe allocation without quota authority",
                path: path.to_path_buf(),
                source: None,
                cleanup_failed: false,
                task_failed: false,
                kind: FilesystemStorageErrorKind::AllocationUnsupported,
            }),
        }
    }

    pub(crate) fn cleanup_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: Some(source),
                cleanup_failed: true,
                task_failed: false,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    pub(in crate::sandbox_filesystem) fn cleanup_verification(
        operation: &'static str,
        path: &Path,
    ) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: None,
                cleanup_failed: true,
                task_failed: false,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    pub(crate) fn task_failure(
        operation: &'static str,
        path: &Path,
        source: BlockingTaskError,
    ) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path: path.to_path_buf(),
                source: Some(std::io::Error::other(source)),
                cleanup_failed: false,
                task_failed: true,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }

    /// Gives the same error about `path`, for an operation that reached the path under another
    /// name, such as a path below `/proc/self/fd/`.
    #[cfg(test)]
    pub(in crate::sandbox_filesystem) fn about(mut self, path: &Path) -> Self {
        self.inner.path = path.to_path_buf();
        self
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.inner.cleanup_failed
    }

    pub(crate) fn during_cleanup(mut self) -> Self {
        self.inner.cleanup_failed = true;
        self
    }

    pub(crate) fn is_storage_exhaustion(&self) -> bool {
        self.inner.source.as_ref().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            )
        })
    }

    pub(crate) fn is_terminal_failure(&self) -> bool {
        self.inner.task_failed
            || (!self.is_sandbox_escape()
                && self.inner.source.as_ref().is_some_and(|source| {
                    matches!(
                        source.kind(),
                        std::io::ErrorKind::InvalidData
                            | std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::ReadOnlyFilesystem
                    ) || is_terminal_storage_errno(source)
                }))
    }

    pub(crate) fn is_sandbox_escape(&self) -> bool {
        self.inner.kind == FilesystemStorageErrorKind::SandboxEscape
    }

    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.inner.source.as_ref().map(std::io::Error::kind)
    }

    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.inner.source.as_ref()
    }

    pub(crate) fn allocation_is_unsupported(&self) -> bool {
        self.inner.kind == FilesystemStorageErrorKind::AllocationUnsupported
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::sandbox_filesystem) fn is_terminal_storage_errno(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(errno) if matches!(errno, libc::EIO | libc::ESTALE | libc::ENODEV))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(in crate::sandbox_filesystem) fn is_terminal_storage_errno(_error: &std::io::Error) -> bool {
    false
}

impl Debug for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemStorageError")
            .field("operation", &self.inner.operation)
            .field("path", &self.inner.path)
            .field("source", &self.inner.source)
            .field("cleanup_failed", &self.inner.cleanup_failed)
            .field("task_failed", &self.inner.task_failed)
            .field("kind", &self.inner.kind)
            .finish()
    }
}

impl Display for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to {} filesystem {}",
            self.inner.operation,
            self.inner.path.display()
        )?;
        if let Some(source) = &self.inner.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner
            .source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

impl From<crate::storage_volume::StorageVolumeError> for FilesystemStorageError {
    fn from(error: crate::storage_volume::StorageVolumeError) -> Self {
        let (operation, path, source, cleanup_failed, task_failed) = error.into_parts();
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path,
                source,
                cleanup_failed,
                task_failed,
                kind: FilesystemStorageErrorKind::General,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
