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

use crate::durable_host::durability::InterruptSources;
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::Future;
use std::pin::Pin;

/// Only native I/O observation belongs here, not replay resolution or durable completion.
pub(crate) struct NativeIoWait {
    interrupt: Pin<Box<dyn Future<Output = InterruptKind> + Send>>,
}

impl NativeIoWait {
    pub(crate) fn new(sources: InterruptSources<'_>) -> Self {
        Self {
            interrupt: sources.subscribe(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_signal(
        interrupt: impl Future<Output = InterruptKind> + Send + 'static,
    ) -> Self {
        Self {
            interrupt: Box::pin(interrupt),
        }
    }

    pub(crate) async fn wait<T>(self, native: impl Future<Output = T>) -> Result<T, InterruptKind> {
        tokio::select! {
            result = native => Ok(result),
            kind = self.interrupt => Err(kind),
        }
    }
}

#[cfg(test)]
mod tests;
