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
use crate::host_directories::{HostDirectoryError, HostDirectoryFailure};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct StorageVolumeError {
    operation: &'static str,
    path: PathBuf,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
    task_failed: bool,
}

impl StorageVolumeError {
    pub(crate) fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.into(),
            source: None,
            cleanup_failed: false,
            task_failed: false,
        }
    }

    pub(crate) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            source: Some(source),
            ..Self::verification(operation, path)
        }
    }

    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(crate) fn cleanup_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            cleanup_failed: true,
            ..Self::io(operation, path, source)
        }
    }

    pub(crate) fn task_failure(
        operation: &'static str,
        path: &Path,
        source: BlockingTaskError,
    ) -> Self {
        Self {
            task_failed: true,
            ..Self::io(operation, path, std::io::Error::other(source))
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn about(mut self, path: &Path) -> Self {
        self.path = path.into();
        self
    }

    #[cfg(test)]
    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.source.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.source.as_ref().map(std::io::Error::kind)
    }

    #[cfg(test)]
    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }

    pub(crate) fn into_parts(self) -> (&'static str, PathBuf, Option<std::io::Error>, bool, bool) {
        (
            self.operation,
            self.path,
            self.source,
            self.cleanup_failed,
            self.task_failed,
        )
    }
}

impl From<HostDirectoryError> for StorageVolumeError {
    fn from(error: HostDirectoryError) -> Self {
        let (operation, path, failure, cleanup_failed) = error.into_parts();
        let mut error = match failure {
            HostDirectoryFailure::Io(source) => Self::io(operation, &path, source),
            HostDirectoryFailure::Task(source) => Self::task_failure(operation, &path, source),
            HostDirectoryFailure::Verification => Self::verification(operation, &path),
        };
        error.cleanup_failed = cleanup_failed;
        error
    }
}

impl Display for StorageVolumeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to {} filesystem {}",
            self.operation,
            self.path.display()
        )?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for StorageVolumeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|source| source as _)
    }
}

#[cfg(test)]
mod tests;
