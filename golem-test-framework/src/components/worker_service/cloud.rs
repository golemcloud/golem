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

use crate::components::registry_service::cloud::new_cloud_reqwest_client;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use golem_client::api::{AgentClientLive, WorkerClientLive};
use golem_client::{Context, Security};
use golem_common::model::auth::TokenSecret;
use tokio::sync::OnceCell;
use tracing::info;
use url::Url;

/// Worker-service client for cloud mode.
///
/// In the deployed Golem environment both registry-service and worker-service
/// are reachable behind a single Gateway API hostname
/// (e.g. `https://release.dev-api.golem.cloud`). This struct holds that shared
/// `api_url`; routing to worker-service is done by the Gateway based on URL
/// path (`/v1/components/*/workers/**`, `/v1/agents/**`).
pub struct CloudWorkerService {
    api_url: Url,
    base_http_client: OnceCell<reqwest_middleware::ClientWithMiddleware>,
}

impl CloudWorkerService {
    pub fn new(api_url: Url) -> Self {
        info!("Using cloud worker-service via API gateway at {api_url}");
        Self {
            api_url,
            base_http_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl WorkerService for CloudWorkerService {
    fn http_host(&self) -> String {
        self.api_url.host_str().unwrap_or("localhost").to_string()
    }

    fn http_port(&self) -> u16 {
        self.api_url.port_or_known_default().unwrap_or(443)
    }

    fn grpc_host(&self) -> String {
        panic!("grpc_host() is not available through the Gateway in cloud mode");
    }

    fn gprc_port(&self) -> u16 {
        panic!("gprc_port() is not available through the Gateway in cloud mode");
    }

    fn custom_request_host(&self) -> String {
        // Code-first HTTP API deployments are reached via the apps base domain
        // (*.apps.dev.golem.cloud), not through this host.
        panic!("custom_request_host() is not available in cloud mode");
    }

    fn custom_request_port(&self) -> u16 {
        // Code-first HTTP API deployments are reached via the apps base domain
        // (*.apps.dev.golem.cloud), not through this port.
        panic!("custom_request_port() is not available in cloud mode");
    }

    fn mcp_port(&self) -> u16 {
        panic!("mcp_port() is not available in cloud mode");
    }

    async fn kill(&self) {}

    async fn base_http_client(&self) -> reqwest_middleware::ClientWithMiddleware {
        self.base_http_client
            .get_or_init(|| async { new_cloud_reqwest_client() })
            .await
            .clone()
    }

    /// Overrides the trait default to use the configured API gateway URL
    /// (including scheme/TLS), rather than rebuilding `http://{host}:{port}`.
    async fn worker_http_client(&self, token: &TokenSecret) -> WorkerClientLive {
        WorkerClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: self.api_url.clone(),
                security_token: Security::Bearer(token.secret().to_string()),
            },
        }
    }

    /// Overrides the trait default to use the configured API gateway URL
    /// (including scheme/TLS), rather than rebuilding `http://{host}:{port}`.
    async fn agent_http_client(&self, token: &TokenSecret) -> AgentClientLive {
        AgentClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: self.api_url.clone(),
                security_token: Security::Bearer(token.secret().to_string()),
            },
        }
    }
}
