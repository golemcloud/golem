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
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use uuid::Uuid;

pub struct AdminOnlyStubRegistryService {
    admin_account_id: AccountId,
    admin_account_email: String,
    admin_token: TokenSecret,
}

impl AdminOnlyStubRegistryService {
    pub fn new(
        admin_account_id: AccountId,
        admin_account_email: String,
        admin_token: Uuid,
    ) -> Self {
        Self {
            admin_account_id,
            admin_account_email,
            admin_token: TokenSecret(admin_token),
        }
    }
}

#[async_trait]
impl RegistryService for AdminOnlyStubRegistryService {
    fn http_host(&self) -> String {
        panic!("No registry service running")
    }

    fn http_port(&self) -> u16 {
        panic!("No registry service running")
    }

    fn grpc_host(&self) -> String {
        panic!("No registry service running")
    }

    fn gprc_port(&self) -> u16 {
        panic!("No registry service running")
    }

    fn admin_account_id(&self) -> AccountId {
        self.admin_account_id.clone()
    }

    fn admin_account_email(&self) -> String {
        self.admin_account_email.clone()
    }

    fn admin_account_token(&self) -> TokenSecret {
        self.admin_token.clone()
    }

    async fn kill(&mut self) {}

    async fn base_http_client(&self) -> reqwest::Client {
        panic!("No registry service running")
    }
}
