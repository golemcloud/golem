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
#[cfg(target_os = "linux")]
use crate::path_removal::RemoveFailure;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct StorageVolumeError {
    operation: &'static str,
    path: Option<PathBuf>,
    failure: OperationFailure,
    cleanup_failed: bool,
}

impl StorageVolumeError {
    pub(crate) fn verification(operation: &'static str, path: Option<&Path>) -> Self {
        Self {
            operation,
            path: path.map(Path::to_path_buf),
            failure: OperationFailure::Verification,
            cleanup_failed: false,
        }
    }

    pub(crate) fn io(operation: &'static str, path: Option<&Path>, source: std::io::Error) -> Self {
        Self {
            failure: OperationFailure::Io(source),
            ..Self::verification(operation, path)
        }
    }

    pub(crate) fn task_failure(
        operation: &'static str,
        path: Option<&Path>,
        source: BlockingTaskError,
    ) -> Self {
        Self {
            failure: OperationFailure::Task(source),
            ..Self::verification(operation, path)
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn from_removal(
        operation: &'static str,
        path: &Path,
        failure: RemoveFailure,
    ) -> Self {
        Self {
            failure: failure.into(),
            ..Self::verification(operation, Some(path))
        }
    }

    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(crate) fn cleanup_io(
        operation: &'static str,
        path: Option<&Path>,
        source: std::io::Error,
    ) -> Self {
        Self::io(operation, path, source).during_cleanup()
    }

    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(super) fn during_cleanup(mut self) -> Self {
        self.cleanup_failed = true;
        self
    }

    pub(crate) fn operation(&self) -> &'static str {
        self.operation
    }
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }
    pub(crate) fn into_failure(self) -> OperationFailure {
        self.failure
    }

    #[cfg(test)]
    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.failure.io_error()
    }
    #[cfg(test)]
    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.io_error().map(std::io::Error::kind)
    }
}

impl Display for StorageVolumeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "failed to {}", self.operation)?;
        if let Some(path) = &self.path {
            write!(formatter, " at {}", path.display())?;
        }
        if let Some(cause) = self.failure.cause() {
            write!(formatter, ": {cause}")?;
        }
        Ok(())
    }
}

impl std::error::Error for StorageVolumeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failure.cause()
    }
}

#[cfg(test)]
mod tests;
