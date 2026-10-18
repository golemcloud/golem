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

use super::RegistryService;
use crate::components::cloud_http::{SpreadClients, cloud_client_builder};
use async_trait::async_trait;
use golem_client::api::RegistryServiceClientLive;
use golem_client::{Context, Security};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use tokio::sync::OnceCell;
use tracing::info;
use url::Url;

/// Registry-service client for cloud mode.
///
/// In the deployed Golem environment both registry-service and worker-service
/// are reachable behind a single Gateway API hostname
/// (e.g. `https://release.dev-api.golem.cloud`). This struct holds that shared
/// `api_url`; routing to the correct backend service is done by the Gateway
/// based on URL path.
pub struct CloudRegistryService {
    api_url: Url,
    admin_token: TokenSecret,
    builtin_plugin_owner_account_id: AccountId,
    default_plan_id: PlanId,
    base_http_client: OnceCell<reqwest_middleware::ClientWithMiddleware>,
}

impl CloudRegistryService {
    pub fn new(
        api_url: Url,
        admin_token: TokenSecret,
        builtin_plugin_owner_account_id: AccountId,
        default_plan_id: PlanId,
    ) -> Self {
        info!("Using cloud API gateway at {api_url}");
        Self {
            api_url,
            admin_token,
            builtin_plugin_owner_account_id,
            default_plan_id,
            base_http_client: OnceCell::new(),
        }
    }
}

/// Constructs the HTTP client for cloud-mode benchmark connections.
///
/// The client sends the requests over `CLOUD_CONNECTIONS` connections, spread evenly over
/// the zones of the gateway; see `SpreadClients`. The settings of each connection are in
/// `cloud_client_builder`.
pub fn new_cloud_reqwest_client() -> reqwest_middleware::ClientWithMiddleware {
    let fallback = cloud_client_builder()
        .build()
        .expect("Failed to build cloud HTTP client");
    reqwest_middleware::ClientBuilder::new(fallback)
        .with(reqwest_tracing::TracingMiddleware::default())
        .with(SpreadClients::new())
        .build()
}

#[async_trait]
impl RegistryService for CloudRegistryService {
    fn http_host(&self) -> String {
        self.api_url.host_str().unwrap_or("localhost").to_string()
    }

    fn http_port(&self) -> u16 {
        self.api_url.port_or_known_default().unwrap_or(443)
    }

    fn grpc_host(&self) -> String {
        panic!("grpc_host() is not available through the Gateway in cloud mode");
    }

    fn grpc_port(&self) -> u16 {
        panic!("grpc_port() is not available through the Gateway in cloud mode");
    }

    fn admin_account_id(&self) -> AccountId {
        AccountId(uuid::Uuid::nil())
    }

    fn admin_account_email(&self) -> AccountEmail {
        AccountEmail::new(String::new())
    }

    fn admin_account_token(&self) -> TokenSecret {
        self.admin_token.clone()
    }

    fn builtin_plugin_owner_account_id(&self) -> AccountId {
        self.builtin_plugin_owner_account_id
    }

    fn default_plan(&self) -> PlanId {
        self.default_plan_id
    }

    fn low_fuel_plan(&self) -> PlanId {
        panic!(
            "low_fuel_plan is not supported in cloud mode; \
             the benchmark calling this method requires a local or provided cluster"
        );
    }

    fn low_disk_space_plan(&self) -> PlanId {
        panic!(
            "low_disk_space_plan is not supported in cloud mode; \
             the benchmark calling this method requires a local or provided cluster"
        );
    }

    fn low_http_calls_plan(&self) -> PlanId {
        panic!(
            "low_http_calls_plan is not supported in cloud mode; \
             the benchmark calling this method requires a local or provided cluster"
        );
    }

    fn low_rpc_calls_plan(&self) -> PlanId {
        panic!(
            "low_rpc_calls_plan is not supported in cloud mode; \
             the benchmark calling this method requires a local or provided cluster"
        );
    }

    async fn kill(&self) {}

    async fn base_http_client(&self) -> reqwest_middleware::ClientWithMiddleware {
        self.base_http_client
            .get_or_init(|| async { new_cloud_reqwest_client() })
            .await
            .clone()
    }

    async fn client(&self, token: &TokenSecret) -> RegistryServiceClientLive {
        RegistryServiceClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: self.api_url.clone(),
                security_token: Security::Bearer(token.secret().to_string()),
            },
        }
    }
}
