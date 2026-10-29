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
use crate::path_removal::RemoveFailure;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

/// The context and cause of a volume or host-directory operation.
#[derive(Debug)]
pub(crate) struct FilesystemOperationError {
    pub(crate) operation: &'static str,
    pub(crate) path: Option<PathBuf>,
    failure: FilesystemFailure,
    cleanup_failed: bool,
}

#[derive(Debug)]
enum FilesystemFailure {
    Io(std::io::Error),
    Task(std::io::Error),
    Verification,
}

impl FilesystemOperationError {
    pub(crate) fn verification(operation: &'static str, path: Option<&Path>) -> Self {
        Self {
            operation,
            path: path.map(Path::to_path_buf),
            failure: FilesystemFailure::Verification,
            cleanup_failed: false,
        }
    }

    pub(crate) fn io(operation: &'static str, path: Option<&Path>, source: std::io::Error) -> Self {
        Self {
            failure: FilesystemFailure::Io(source),
            ..Self::verification(operation, path)
        }
    }

    pub(crate) fn cleanup_io(
        operation: &'static str,
        path: Option<&Path>,
        source: std::io::Error,
    ) -> Self {
        Self::io(operation, path, source).during_cleanup()
    }

    pub(crate) fn task_failure(
        operation: &'static str,
        path: Option<&Path>,
        source: BlockingTaskError,
    ) -> Self {
        Self {
            failure: FilesystemFailure::Task(std::io::Error::other(source)),
            ..Self::verification(operation, path)
        }
    }

    pub(crate) fn removal(operation: &'static str, path: &Path, failure: RemoveFailure) -> Self {
        match failure {
            RemoveFailure::Io(source) => Self::io(operation, Some(path), source),
            RemoveFailure::StillPresent => Self::verification(operation, Some(path)),
        }
        .during_cleanup()
    }

    pub(crate) fn during_cleanup(mut self) -> Self {
        self.cleanup_failed = true;
        self
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }

    pub(crate) fn task_failed(&self) -> bool {
        matches!(self.failure, FilesystemFailure::Task(_))
    }

    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        match &self.failure {
            FilesystemFailure::Io(source) | FilesystemFailure::Task(source) => Some(source),
            FilesystemFailure::Verification => None,
        }
    }

    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.io_error().map(std::io::Error::kind)
    }
}

impl Display for FilesystemOperationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to {} filesystem", self.operation)?;
        if let Some(path) = &self.path {
            write!(f, " {}", path.display())?;
        }
        if let Some(source) = self.io_error() {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemOperationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.io_error().map(|source| source as _)
    }
}

#[cfg(test)]
mod tests;
