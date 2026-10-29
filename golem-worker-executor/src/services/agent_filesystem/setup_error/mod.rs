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

use crate::filesystem_pressure::FilesystemPressureConfigError;
use crate::sandbox_filesystem::FilesystemStorageError;
use std::fmt::{Display, Formatter};

/// A failure to bind executor filesystems before worker admission starts.
#[derive(Debug)]
pub struct FilesystemSetupError {
    failure: SetupFailure,
}

#[derive(Debug)]
enum SetupFailure {
    Storage(FilesystemStorageError),
    PressureConfiguration(FilesystemPressureConfigError),
}

impl From<FilesystemStorageError> for FilesystemSetupError {
    fn from(error: FilesystemStorageError) -> Self {
        Self {
            failure: SetupFailure::Storage(error),
        }
    }
}

impl From<FilesystemPressureConfigError> for FilesystemSetupError {
    fn from(error: FilesystemPressureConfigError) -> Self {
        Self {
            failure: SetupFailure::PressureConfiguration(error),
        }
    }
}

impl Display for FilesystemSetupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.failure {
            SetupFailure::Storage(error) => Display::fmt(error, formatter),
            SetupFailure::PressureConfiguration(error) => Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for FilesystemSetupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.failure {
            SetupFailure::Storage(error) => Some(error),
            SetupFailure::PressureConfiguration(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests;
