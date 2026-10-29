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
use crate::operation_failure::OperationContext;
#[cfg(target_os = "linux")]
use crate::path_removal::RemoveFailure;
use std::fmt::{Display, Formatter};
use std::path::Path;

#[derive(Debug)]
pub(crate) struct StorageVolumeError {
    context: OperationContext,
    cleanup_failed: bool,
    original_error: Option<Box<StorageVolumeError>>,
}

impl StorageVolumeError {
    fn from_context(context: OperationContext) -> Self {
        Self {
            context,
            cleanup_failed: false,
            original_error: None,
        }
    }

    pub(crate) fn verification(operation: &'static str, path: Option<&Path>) -> Self {
        Self::from_context(OperationContext::verification(operation, path))
    }

    pub(crate) fn io(operation: &'static str, path: Option<&Path>, source: std::io::Error) -> Self {
        Self::from_context(OperationContext::io(operation, path, source))
    }

    pub(crate) fn task_failure(
        operation: &'static str,
        path: Option<&Path>,
        source: BlockingTaskError,
    ) -> Self {
        Self::from_context(OperationContext::task_failure(operation, path, source))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn from_removal(
        operation: &'static str,
        path: &Path,
        failure: RemoveFailure,
    ) -> Self {
        Self::from_context(OperationContext::new(operation, Some(path), failure.into()))
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

    pub(crate) fn context(&self) -> &OperationContext {
        &self.context
    }
    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }
    pub(crate) fn into_context(self) -> OperationContext {
        self.context
    }
    pub(crate) fn take_original_error(&mut self) -> Option<Box<Self>> {
        self.original_error.take()
    }

    /// A probe failure is primary. Only failure of its subsequent removal is cleanup failure.
    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(super) fn after_cleanup(
        result: Result<(), Self>,
        cleanup: Result<(), Self>,
    ) -> Result<(), Self> {
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(cleanup)) => Err(cleanup.during_cleanup()),
            (Err(error), Err(mut cleanup)) => {
                cleanup.original_error = Some(Box::new(error));
                Err(cleanup.during_cleanup())
            }
        }
    }
}

impl Display for StorageVolumeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(original) = &self.original_error {
            write!(formatter, "{original}; cleanup also failed: ")?;
        }
        Display::fmt(self.context(), formatter)
    }
}

impl std::error::Error for StorageVolumeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.context().source()
    }
}

#[cfg(test)]
mod tests;
