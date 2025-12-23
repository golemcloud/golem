// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use crate::components::new_reqwest_client;
use async_trait::async_trait;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use tokio::sync::OnceCell;
use tracing::info;

pub struct ProvidedRegistryService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    base_http_client: OnceCell<reqwest::Client>,
    admin_account_id: AccountId,
    admin_account_email: AccountEmail,
    admin_account_token: TokenSecret,
    default_plan_id: PlanId,
    low_fuel_plan_id: PlanId,
}

impl ProvidedRegistryService {
    pub async fn new(
        host: String,
        http_port: u16,
        grpc_port: u16,
        admin_account_id: AccountId,
        admin_account_email: AccountEmail,
        admin_account_token: TokenSecret,
        default_plan_id: PlanId,
        low_fuel_plan_id: PlanId,
    ) -> Self {
        info!("Using already running golem-worker-service on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            base_http_client: OnceCell::new(),
            admin_account_id,
            admin_account_email,
            admin_account_token,
            default_plan_id,
            low_fuel_plan_id,
        }
    }
}

#[async_trait]
impl RegistryService for ProvidedRegistryService {
    fn http_host(&self) -> String {
        self.host.clone()
    }
    fn http_port(&self) -> u16 {
        self.http_port
    }

    fn grpc_host(&self) -> String {
        self.host.clone()
    }
    fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    fn admin_account_id(&self) -> AccountId {
        self.admin_account_id
    }
    fn admin_account_email(&self) -> AccountEmail {
        self.admin_account_email.clone()
    }
    fn admin_account_token(&self) -> TokenSecret {
        self.admin_account_token.clone()
    }

    fn default_plan(&self) -> PlanId {
        self.default_plan_id
    }
    fn low_fuel_plan(&self) -> PlanId {
        self.low_fuel_plan_id
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn kill(&self) {}
}
