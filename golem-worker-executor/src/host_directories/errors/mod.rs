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
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct HostDirectoryError {
    operation: &'static str,
    path: PathBuf,
    failure: OperationFailure,
}

impl HostDirectoryError {
    pub(super) fn new(operation: &'static str, path: &Path, failure: OperationFailure) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            failure,
        }
    }

    pub(super) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::new(operation, path, OperationFailure::Io(source))
    }

    pub(super) fn task_failure(
        operation: &'static str,
        path: &Path,
        source: BlockingTaskError,
    ) -> Self {
        Self::new(operation, path, OperationFailure::Task(source))
    }

    pub(crate) fn operation(&self) -> &'static str {
        self.operation
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn into_failure(self) -> OperationFailure {
        self.failure
    }

    #[cfg(test)]
    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.failure.io_error().map(std::io::Error::kind)
    }
}

impl Display for HostDirectoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to {} at {}",
            self.operation,
            self.path.display()
        )?;
        if let Some(cause) = self.failure.cause() {
            write!(formatter, ": {cause}")?;
        }
        Ok(())
    }
}

impl std::error::Error for HostDirectoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failure.cause()
    }
}
