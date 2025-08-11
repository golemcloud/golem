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

use crate::model::account::AccountId;
use super::{TokenId};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct Token {
    pub id: TokenId,
    pub account_id: AccountId,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct TokenWithSecret {
    pub data: Token,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct OAuth2Data {
    pub url: String,
    pub user_code: String,
    pub expires: chrono::DateTime<Utc>,
    pub encoded_session: String,
}
