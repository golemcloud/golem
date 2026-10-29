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
use crate::operation_failure::OperationFailure;
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

struct FilesystemStorageErrorInner {
    operation: &'static str,
    path: Option<PathBuf>,
    failure: OperationFailure,
    cleanup_failed: bool,
    kind: FilesystemStorageErrorKind,
    original_error: Option<Box<FilesystemStorageError>>,
}

impl FilesystemStorageError {
    pub(crate) fn from_failure(
        operation: &'static str,
        path: Option<PathBuf>,
        failure: OperationFailure,
    ) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                operation,
                path,
                failure,
                cleanup_failed: false,
                kind: FilesystemStorageErrorKind::General,
                original_error: None,
            }),
        }
    }

    pub(crate) fn io(operation: &'static str, path: Option<&Path>, source: std::io::Error) -> Self {
        let mut kind = FilesystemStorageErrorKind::General;
        // cap-primitives has no public typed escape error. Real OS refusals retain their errno.
        if source.kind() == std::io::ErrorKind::PermissionDenied
            && source.raw_os_error().is_none()
            && source.to_string() == "a path led outside of the filesystem"
        {
            kind = FilesystemStorageErrorKind::SandboxEscape;
        }
        let mut error = Self::from_failure(
            operation,
            path.map(Path::to_path_buf),
            OperationFailure::Io(source),
        );
        error.inner.kind = kind;
        error
    }

    pub(crate) fn verification(operation: &'static str, path: Option<&Path>) -> Self {
        Self::from_failure(
            operation,
            path.map(Path::to_path_buf),
            OperationFailure::Verification,
        )
    }

    pub(crate) fn allocation_unsupported(path: Option<&Path>) -> Self {
        let mut error = Self::verification("observe allocation without quota authority", path);
        error.inner.kind = FilesystemStorageErrorKind::AllocationUnsupported;
        error
    }

    pub(crate) fn cleanup_io(
        operation: &'static str,
        path: Option<&Path>,
        source: std::io::Error,
    ) -> Self {
        // A cleanup error keeps its storage classification rather than becoming an escape refusal.
        Self::from_failure(
            operation,
            path.map(Path::to_path_buf),
            OperationFailure::Io(source),
        )
        .during_cleanup()
    }

    pub(in crate::sandbox_filesystem) fn cleanup_verification(
        operation: &'static str,
        path: Option<&Path>,
    ) -> Self {
        Self::verification(operation, path).during_cleanup()
    }

    pub(crate) fn task_failure(
        operation: &'static str,
        path: Option<&Path>,
        source: BlockingTaskError,
    ) -> Self {
        Self::from_failure(
            operation,
            path.map(Path::to_path_buf),
            OperationFailure::Task(source),
        )
    }

    #[cfg(test)]
    pub(in crate::sandbox_filesystem) fn about(mut self, path: &Path) -> Self {
        self.inner.path = Some(path.to_path_buf());
        self
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.inner.cleanup_failed
    }

    pub(crate) fn during_cleanup(mut self) -> Self {
        self.inner.cleanup_failed = true;
        self
    }

    /// Keeps the cleanup failure's classification and both diagnostics.
    pub(crate) fn with_cleanup_failure(self, mut cleanup: Self) -> Self {
        cleanup.inner.original_error = Some(Box::new(self));
        cleanup.during_cleanup()
    }

    pub(crate) fn is_storage_exhaustion(&self) -> bool {
        self.io_error().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            )
        })
    }

    pub(crate) fn is_terminal_failure(&self) -> bool {
        self.task_error().is_some()
            || (!self.is_sandbox_escape()
                && self.io_error().is_some_and(|source| {
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
        self.io_error().map(std::io::Error::kind)
    }
    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.inner.failure.io_error()
    }

    pub(crate) fn task_error(&self) -> Option<&BlockingTaskError> {
        match &self.inner.failure {
            OperationFailure::Task(error) => Some(error),
            OperationFailure::Io(_) | OperationFailure::Verification => None,
        }
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
        let mut debug = formatter.debug_struct("FilesystemStorageError");
        debug
            .field("operation", &self.inner.operation)
            .field("path", &self.inner.path)
            .field("failure", &self.inner.failure)
            .field("cleanup_failed", &self.cleanup_failed())
            .field("kind", &self.inner.kind);
        if let Some(original) = &self.inner.original_error {
            debug.field("original_error", original);
        }
        debug.finish()
    }
}

impl Display for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(original) = &self.inner.original_error {
            write!(formatter, "{original}; cleanup also failed: ")?;
        }
        write!(formatter, "failed to {}", self.inner.operation)?;
        if let Some(path) = &self.inner.path {
            write!(formatter, " at {}", path.display())?;
        }
        if let Some(cause) = self.inner.failure.cause() {
            write!(formatter, ": {cause}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.failure.cause()
    }
}

#[cfg(test)]
mod tests;
