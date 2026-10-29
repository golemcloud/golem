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
use crate::operation_failure::{OperationContext, OperationFailure};
use std::fmt::{Display, Formatter};
use std::path::Path;

#[derive(Debug)]
pub(crate) struct HostDirectoryError {
    context: OperationContext,
}

impl HostDirectoryError {
    pub(super) fn new(operation: &'static str, path: &Path, failure: OperationFailure) -> Self {
        Self {
            context: OperationContext::new(operation, Some(path), failure),
        }
    }

    pub(super) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            context: OperationContext::io(operation, Some(path), source),
        }
    }

    pub(super) fn task_failure(
        operation: &'static str,
        path: &Path,
        source: BlockingTaskError,
    ) -> Self {
        Self {
            context: OperationContext::task_failure(operation, Some(path), source),
        }
    }

    pub(crate) fn context(&self) -> &OperationContext {
        &self.context
    }
    pub(crate) fn into_context(self) -> OperationContext {
        self.context
    }
}

impl Display for HostDirectoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.context(), formatter)
    }
}

impl std::error::Error for HostDirectoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.context().source()
    }
}
