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
use crate::filesystem::path_removal::RemoveFailure;
use std::fmt::{Debug, Display, Formatter};
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

    pub(crate) fn io_error(&self) -> Option<&std::io::Error> {
        self.failure.io_error()
    }
    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.io_error().map(std::io::Error::kind)
    }
    pub(crate) fn task_error(&self) -> Option<&BlockingTaskError> {
        self.failure.task_error()
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

/// Diagnostic-chain bookkeeping. Callers explicitly choose whether a failure is cleanup.
#[derive(Debug)]
pub(crate) struct CleanupFailure<E> {
    context: OperationContext,
    cleanup_failed: bool,
    original_error: Option<Box<E>>,
}

impl<E> CleanupFailure<E> {
    pub(crate) fn new(context: OperationContext) -> Self {
        Self {
            context,
            cleanup_failed: false,
            original_error: None,
        }
    }

    pub(crate) fn context(&self) -> &OperationContext {
        &self.context
    }
    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }

    pub(crate) fn during_cleanup(mut self) -> Self {
        self.cleanup_failed = true;
        self
    }

    pub(crate) fn with_original(mut self, original: E) -> Self {
        self.original_error = Some(Box::new(original));
        self
    }

    pub(crate) fn map_original<F>(self, map: impl FnOnce(E) -> F) -> CleanupFailure<F> {
        CleanupFailure {
            context: self.context,
            cleanup_failed: self.cleanup_failed,
            original_error: self.original_error.map(|original| Box::new(map(*original))),
        }
    }
}

impl<E: Display> Display for CleanupFailure<E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(original) = &self.original_error {
            write!(formatter, "{original}; cleanup also failed: ")?;
        }
        Display::fmt(&self.context, formatter)
    }
}

impl<E: Debug + Display> std::error::Error for CleanupFailure<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.context.source()
    }
}

#[cfg(test)]
mod tests;
