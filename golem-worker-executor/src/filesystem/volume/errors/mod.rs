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

use crate::filesystem::errors::CleanupFailure;
use crate::filesystem::errors::OperationContext;
use std::fmt::{Display, Formatter};
#[cfg(any(test, target_os = "linux", target_os = "macos"))]
use std::path::Path;

#[derive(Debug)]
pub(crate) struct StorageVolumeError {
    failure: CleanupFailure<Self>,
}

impl From<OperationContext> for StorageVolumeError {
    fn from(context: OperationContext) -> Self {
        Self {
            failure: CleanupFailure::new(context),
        }
    }
}

impl StorageVolumeError {
    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(crate) fn cleanup_io(
        operation: &'static str,
        path: Option<&Path>,
        source: std::io::Error,
    ) -> Self {
        Self::from(OperationContext::io(operation, path, source)).during_cleanup()
    }

    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(super) fn during_cleanup(mut self) -> Self {
        self.failure = self.failure.during_cleanup();
        self
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
                cleanup.failure = cleanup.failure.with_original(error);
                Err(cleanup.during_cleanup())
            }
        }
    }
}

impl AsRef<CleanupFailure<StorageVolumeError>> for StorageVolumeError {
    fn as_ref(&self) -> &CleanupFailure<Self> {
        &self.failure
    }
}

impl From<StorageVolumeError> for CleanupFailure<StorageVolumeError> {
    fn from(error: StorageVolumeError) -> Self {
        error.failure
    }
}

impl Display for StorageVolumeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.as_ref(), formatter)
    }
}

impl std::error::Error for StorageVolumeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.as_ref().source()
    }
}

#[cfg(test)]
mod tests;
