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

/// A low-level cause, without an operation, path, domain or cleanup policy.
#[derive(Debug)]
pub(crate) enum OperationFailure {
    Io(std::io::Error),
    Task(BlockingTaskError),
    Verification,
}

impl OperationFailure {
    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        match self {
            Self::Io(error) => Some(error),
            Self::Task(_) | Self::Verification => None,
        }
    }

    pub(crate) fn task_error(&self) -> Option<&BlockingTaskError> {
        match self {
            Self::Task(error) => Some(error),
            Self::Io(_) | Self::Verification => None,
        }
    }

    pub(crate) fn cause(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Task(error) => Some(error),
            Self::Verification => None,
        }
    }
}

/// Diagnostic facts only. The owning domain decides classification and cleanup policy.
#[derive(Debug)]
pub(crate) struct OperationContext {
    operation: &'static str,
    path: Option<PathBuf>,
    failure: OperationFailure,
}

impl OperationContext {
    /// `operation` is a verb phrase; the context supplies "at" before a path.
    pub(crate) fn new(
        operation: &'static str,
        path: Option<&Path>,
        failure: OperationFailure,
    ) -> Self {
        Self {
            operation,
            path: path.map(Path::to_path_buf),
            failure,
        }
    }

    pub(crate) fn verification(operation: &'static str, path: Option<&Path>) -> Self {
        Self::new(operation, path, OperationFailure::Verification)
    }

    pub(crate) fn io(operation: &'static str, path: Option<&Path>, source: std::io::Error) -> Self {
        Self::new(operation, path, OperationFailure::Io(source))
    }

    pub(crate) fn task_failure(
        operation: &'static str,
        path: Option<&Path>,
        source: BlockingTaskError,
    ) -> Self {
        Self::new(operation, path, OperationFailure::Task(source))
    }

    pub(crate) fn operation(&self) -> &'static str {
        self.operation
    }
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub(crate) fn failure(&self) -> &OperationFailure {
        &self.failure
    }
    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.failure.io_error()
    }
    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.io_error().map(std::io::Error::kind)
    }
    pub(crate) fn task_error(&self) -> Option<&BlockingTaskError> {
        self.failure.task_error()
    }

    #[cfg(test)]
    pub(crate) fn about(mut self, path: &Path) -> Self {
        self.path = Some(path.to_path_buf());
        self
    }
}

impl Display for OperationContext {
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

impl std::error::Error for OperationContext {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failure.cause()
    }
}

impl From<RemoveFailure> for OperationFailure {
    fn from(failure: RemoveFailure) -> Self {
        match failure {
            RemoveFailure::Io(error) => Self::Io(error),
            RemoveFailure::StillPresent => Self::Verification,
        }
    }
}

#[cfg(test)]
mod tests;
