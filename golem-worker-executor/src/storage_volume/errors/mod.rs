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
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct StorageVolumeError {
    operation: &'static str,
    path: Option<PathBuf>,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
    task_failed: bool,
}

impl StorageVolumeError {
    pub(crate) fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            path: Some(path.into()),
            ..Self::verification_without_path(operation)
        }
    }

    pub(crate) fn verification_without_path(operation: &'static str) -> Self {
        Self {
            operation,
            path: None,
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

    pub(crate) fn io_without_path(operation: &'static str, source: std::io::Error) -> Self {
        Self {
            source: Some(source),
            ..Self::verification_without_path(operation)
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

    pub(crate) fn task_failure_without_path(
        operation: &'static str,
        source: BlockingTaskError,
    ) -> Self {
        Self {
            task_failed: true,
            ..Self::io_without_path(operation, std::io::Error::other(source))
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn during_cleanup(mut self) -> Self {
        self.cleanup_failed = true;
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

    pub(crate) fn into_parts(
        self,
    ) -> (
        &'static str,
        Option<PathBuf>,
        Option<std::io::Error>,
        bool,
        bool,
    ) {
        (
            self.operation,
            self.path,
            self.source,
            self.cleanup_failed,
            self.task_failed,
        )
    }
}

impl Display for StorageVolumeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to {} filesystem", self.operation)?;
        if let Some(path) = &self.path {
            write!(f, " {}", path.display())?;
        }
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
