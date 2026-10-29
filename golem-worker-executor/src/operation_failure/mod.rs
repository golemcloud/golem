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

    pub(crate) fn cause(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Task(error) => Some(error),
            Self::Verification => None,
        }
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
