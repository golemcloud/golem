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

pub mod spawned;
pub mod stub;

use async_trait::async_trait;
use golem_client::api::RegistryServiceClientLive;
use golem_client::{Context, Security};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use url::Url;

#[async_trait]
pub trait RegistryService: Send + Sync {
    fn http_host(&self) -> String;
    fn http_port(&self) -> u16;

    fn grpc_host(&self) -> String;
    fn gprc_port(&self) -> u16;

    fn admin_account_id(&self) -> AccountId;
    fn admin_account_email(&self) -> String;
    fn admin_account_token(&self) -> TokenSecret;

    async fn kill(&mut self);

    async fn base_http_client(&self) -> reqwest::Client;

    async fn client(&self, token: &TokenSecret) -> RegistryServiceClientLive {
        let url = format!("http://{}:{}", self.http_host(), self.http_port());
        RegistryServiceClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.0.to_string()),
            },
        }
    }
}
