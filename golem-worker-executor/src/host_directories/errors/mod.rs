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
pub(crate) struct HostDirectoryError {
    operation: &'static str,
    path: PathBuf,
    failure: HostDirectoryFailure,
    cleanup_failed: bool,
}

#[derive(Debug)]
pub(crate) enum HostDirectoryFailure {
    Io(std::io::Error),
    Task(BlockingTaskError),
    Verification,
}

impl HostDirectoryError {
    pub(super) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.into(),
            failure: HostDirectoryFailure::Io(source),
            cleanup_failed: false,
        }
    }

    pub(super) fn task(operation: &'static str, path: &Path, source: BlockingTaskError) -> Self {
        Self {
            operation,
            path: path.into(),
            failure: HostDirectoryFailure::Task(source),
            cleanup_failed: false,
        }
    }

    pub(super) fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.into(),
            failure: HostDirectoryFailure::Verification,
            cleanup_failed: false,
        }
    }

    pub(super) fn during_cleanup(mut self) -> Self {
        self.cleanup_failed = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match &self.failure {
            HostDirectoryFailure::Io(error) => Some(error.kind()),
            HostDirectoryFailure::Task(_) => Some(std::io::ErrorKind::Other),
            HostDirectoryFailure::Verification => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }

    pub(crate) fn into_parts(self) -> (&'static str, PathBuf, HostDirectoryFailure, bool) {
        (self.operation, self.path, self.failure, self.cleanup_failed)
    }
}

impl Display for HostDirectoryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to {} {}", self.operation, self.path.display())?;
        match &self.failure {
            HostDirectoryFailure::Io(error) => write!(f, ": {error}"),
            HostDirectoryFailure::Task(error) => write!(f, ": {error}"),
            HostDirectoryFailure::Verification => Ok(()),
        }
    }
}

impl std::error::Error for HostDirectoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.failure {
            HostDirectoryFailure::Io(error) => Some(error),
            HostDirectoryFailure::Task(error) => Some(error),
            HostDirectoryFailure::Verification => None,
        }
    }
}
