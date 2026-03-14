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

use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// A graph-wide shutdown signal for background tasks spawned by services.
///
/// Services that spawn background loops should use the `CancellationToken`
/// obtained via `token()` to stop promptly when the executor shuts down,
/// rather than relying solely on `Weak::upgrade()` which can race with
/// other services being torn down.
///
/// The token is cancelled explicitly via `cancel()` (typically from
/// `RunDetails::drop()`). As a safety net, if all `Shutdown` handles are
/// dropped without an explicit cancel, the `Drop` impl on the inner
/// `Arc` will cancel the token.
#[derive(Clone)]
pub struct Shutdown {
    inner: Arc<Inner>,
}

struct Inner {
    token: CancellationToken,
}

impl Shutdown {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                token: CancellationToken::new(),
            }),
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.inner.token.clone()
    }

    pub fn cancel(&self) {
        self.inner.token.cancel();
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.token.cancel();
    }
}
