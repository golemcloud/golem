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

pub mod client;

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A per-executor connection pool that limits the number of concurrent
/// WebSocket connections, preventing socket exhaustion under load.
///
/// Modeled after `wasmtime_wasi_http::HttpConnectionPool` — callers acquire
/// a permit before establishing a connection. The permit is held for the
/// lifetime of the connection and released when the connection is dropped.
#[derive(Clone)]
pub struct WebSocketConnectionPool {
    semaphore: Arc<Semaphore>,
}

impl WebSocketConnectionPool {
    pub fn new(max_connections: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_connections)),
        }
    }

    /// Acquires a permit, blocking if the pool is at capacity.
    /// The returned permit must be held for the lifetime of the connection.
    pub async fn acquire(&self) -> anyhow::Result<OwnedSemaphorePermit> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("WebSocket connection pool closed unexpectedly"))
    }
}
