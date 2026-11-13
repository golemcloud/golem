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

use crate::components::new_reqwest_client_with_tracing;
use crate::components::tcp_relay::TcpRelay;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// A client-side view of a worker service whose HTTP, gRPC and custom request listeners are
/// reached through [`TcpRelay`]s, so a test can drop every client connection to it.
pub struct RelayedWorkerService {
    inner: Arc<dyn WorkerService>,
    http: TcpRelay,
    grpc: TcpRelay,
    custom_request: TcpRelay,
    base_http_client: OnceCell<reqwest_middleware::ClientWithMiddleware>,
}

impl RelayedWorkerService {
    pub async fn start(inner: Arc<dyn WorkerService>) -> std::io::Result<Self> {
        Ok(Self {
            http: TcpRelay::start(format!("{}:{}", inner.http_host(), inner.http_port())).await?,
            grpc: TcpRelay::start(format!("{}:{}", inner.grpc_host(), inner.gprc_port())).await?,
            custom_request: TcpRelay::start(format!(
                "{}:{}",
                inner.custom_request_host(),
                inner.custom_request_port()
            ))
            .await?,
            inner,
            base_http_client: OnceCell::new(),
        })
    }

    pub fn relays(&self) -> [&TcpRelay; 3] {
        [&self.http, &self.grpc, &self.custom_request]
    }

    /// Resets every client connection to the worker service and returns how many it reset.
    pub async fn drop_connections(&self) -> u64 {
        let mut reset = 0;
        for relay in self.relays() {
            reset += relay.drop_connections().await;
        }
        reset
    }

    /// Total number of connections that [`Self::drop_connections`] has reset.
    pub fn reset_count(&self) -> u64 {
        self.relays().iter().map(|relay| relay.reset_count()).sum()
    }

    /// Total number of client connections accepted on any listener.
    pub fn accepted_count(&self) -> u64 {
        self.relays()
            .iter()
            .map(|relay| relay.accepted_count())
            .sum()
    }
}

#[async_trait]
impl WorkerService for RelayedWorkerService {
    fn http_host(&self) -> String {
        "127.0.0.1".to_string()
    }
    fn http_port(&self) -> u16 {
        self.http.port()
    }

    fn grpc_host(&self) -> String {
        "127.0.0.1".to_string()
    }
    fn gprc_port(&self) -> u16 {
        self.grpc.port()
    }

    fn custom_request_host(&self) -> String {
        "127.0.0.1".to_string()
    }
    fn custom_request_port(&self) -> u16 {
        self.custom_request.port()
    }

    fn mcp_port(&self) -> u16 {
        self.inner.mcp_port()
    }

    async fn base_http_client(&self) -> reqwest_middleware::ClientWithMiddleware {
        self.base_http_client
            .get_or_init(|| async { new_reqwest_client_with_tracing() })
            .await
            .clone()
    }

    async fn kill(&self) {
        self.inner.kill().await
    }
}
