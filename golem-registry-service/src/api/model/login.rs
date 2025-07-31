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

use chrono::Utc;
use golem_common_next::model::{AccountId, Empty, TokenId};
use poem_openapi::ApiResponse;
use poem_openapi::payload::Json;
use poem_openapi_derive::Object;

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct Token {
    pub id: TokenId,
    pub account_id: AccountId,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Object)]
pub struct TokenWithSecret {
    pub data: Token,
    pub secret: String,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct OAuth2Data {
    pub url: String,
    pub user_code: String,
    pub expires: chrono::DateTime<Utc>,
    pub encoded_session: String,
}

#[derive(Debug, Clone, Object)]
pub struct WebFlowAuthorizeUrlResponse {
    pub url: String,
    pub state: String,
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowPollResponse {
    /// OAuth flow has completed
    #[oai(status = 200)]
    Completed(Json<TokenWithSecret>),
    /// OAuth flow is pending
    #[oai(status = 202)]
    Pending(Json<Empty>),
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowCallbackResponse {
    /// Redirect to the given URL specified in the web flow start
    #[oai(status = 302)]
    Redirect(Json<Empty>, #[oai(header = "Location")] String),
    /// OAuth flow has completed
    #[oai(status = 200)]
    Success(Json<Empty>),
}
