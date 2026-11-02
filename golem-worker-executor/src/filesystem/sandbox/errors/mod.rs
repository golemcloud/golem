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

use crate::filesystem::blocking::BlockingTaskError;
use crate::filesystem::errors::CleanupFailure;
use crate::filesystem::errors::OperationContext;
use crate::filesystem::host_directories::HostDirectoryError;
use crate::filesystem::volume::StorageVolumeError;
use std::fmt::{Debug, Display, Formatter};
use std::path::Path;

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
    failure: CleanupFailure<FilesystemStorageError>,
    kind: FilesystemStorageErrorKind,
}

impl FilesystemStorageError {
    fn from_failure(failure: CleanupFailure<Self>) -> Self {
        Self {
            inner: Box::new(FilesystemStorageErrorInner {
                failure,
                kind: FilesystemStorageErrorKind::General,
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
        let mut error = Self::from(OperationContext::io(operation, path, source));
        error.inner.kind = kind;
        error
    }

    pub(crate) fn allocation_unsupported(path: Option<&Path>) -> Self {
        let mut error = Self::from(OperationContext::verification(
            "observe allocation without quota authority",
            path,
        ));
        error.inner.kind = FilesystemStorageErrorKind::AllocationUnsupported;
        error
    }

    pub(crate) fn cleanup_io(
        operation: &'static str,
        path: Option<&Path>,
        source: std::io::Error,
    ) -> Self {
        // A cleanup error keeps its storage classification rather than becoming an escape refusal.
        Self::from(OperationContext::io(operation, path, source)).during_cleanup()
    }

    pub(in crate::filesystem::sandbox) fn cleanup_verification(
        operation: &'static str,
        path: Option<&Path>,
    ) -> Self {
        Self::from(OperationContext::verification(operation, path)).during_cleanup()
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.inner.failure.cleanup_failed()
    }

    pub(crate) fn during_cleanup(mut self) -> Self {
        self.inner.failure = self.inner.failure.during_cleanup();
        self
    }

    /// Keeps the cleanup failure's classification and both diagnostics.
    /// A failed rollback is cleanup failure even if its lower-level error was unflagged.
    pub(crate) fn with_cleanup_failure(self, mut cleanup: Self) -> Self {
        cleanup.inner.failure = cleanup.inner.failure.with_original(self).during_cleanup();
        cleanup
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
        self.inner.failure.context().io_kind()
    }
    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.inner.failure.context().io_error()
    }

    pub(crate) fn task_error(&self) -> Option<&BlockingTaskError> {
        self.inner.failure.context().task_error()
    }
    pub(crate) fn allocation_is_unsupported(&self) -> bool {
        self.inner.kind == FilesystemStorageErrorKind::AllocationUnsupported
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::filesystem::sandbox) fn is_terminal_storage_errno(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(errno) if matches!(errno, libc::EIO | libc::ESTALE | libc::ENODEV))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(in crate::filesystem::sandbox) fn is_terminal_storage_errno(_error: &std::io::Error) -> bool {
    false
}

impl Debug for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("FilesystemStorageError");
        debug
            .field("failure", &self.inner.failure)
            .field("kind", &self.inner.kind);
        debug.finish()
    }
}

impl Display for FilesystemStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner.failure, formatter)
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.failure.source()
    }
}

impl From<OperationContext> for FilesystemStorageError {
    fn from(context: OperationContext) -> Self {
        Self::from_failure(CleanupFailure::new(context))
    }
}

impl From<HostDirectoryError> for FilesystemStorageError {
    fn from(error: HostDirectoryError) -> Self {
        Self::from(error.into_context())
    }
}

impl From<StorageVolumeError> for FilesystemStorageError {
    fn from(error: StorageVolumeError) -> Self {
        let failure: CleanupFailure<StorageVolumeError> = error.into();
        Self::from_failure(failure.map_original(Self::from))
    }
}

#[cfg(test)]
mod tests;
