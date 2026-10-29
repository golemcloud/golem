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

use crate::operation_failure::OperationContext;
use std::fmt::{Debug, Display, Formatter};

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
