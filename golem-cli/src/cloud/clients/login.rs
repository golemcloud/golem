// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_cloud_client::api::LoginClient as HttpClient;
use golem_cloud_client::model::{OAuth2Data, Token, TokenSecret, UnsafeToken};
use golem_cloud_client::{Context, Security};
use tracing::info;

use crate::model::GolemError;

#[async_trait]
pub trait LoginClient {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError>;

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError>;

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError>;
}

pub struct LoginClientLive<C: HttpClient + Sync + Send> {
    pub client: C,
    pub context: Context,
}

#[async_trait]
impl<C: HttpClient + Sync + Send> LoginClient for LoginClientLive<C> {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError> {
        info!("Getting token info");
        let mut context = self.context.clone();
        context.security_token = Security::Bearer(manual_token.value.to_string());

        let client = golem_cloud_client::api::LoginClientLive { context };

        Ok(client.v_2_login_token_get().await?)
    }

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError> {
        info!("Start OAuth2 workflow");
        Ok(self.client.login_oauth_2_device_start_post().await?)
    }

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError> {
        info!("Complete OAuth2 workflow");
        Ok(self
            .client
            .login_oauth_2_device_complete_post(&session)
            .await?)
    }
}
