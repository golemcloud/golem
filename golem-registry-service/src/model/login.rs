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

use std::fmt::Display;
use std::str::FromStr;
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenId;
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct ExternalLogin {
    pub external_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub verified_emails: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum OAuth2Provider {
    Github,
}

impl Display for OAuth2Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuth2Provider::Github => write!(f, "github"),
        }
    }
}

impl FromStr for OAuth2Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(OAuth2Provider::Github),
            _ => Err(format!("Invalid OAuth2Provider: {s}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OAuth2Token {
    pub provider: OAuth2Provider,
    pub external_id: String,
    pub account_id: AccountId,
    pub token_id: Option<TokenId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Session {
    pub device_code: String,
    pub interval: std::time::Duration,
    pub expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct EncodedOAuth2Session {
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct OAuth2AccessToken {
    pub provider: OAuth2Provider,
    pub access_token: String,
}

#[derive(Debug, Clone)]
pub struct OAuth2Data {
    pub url: String,
    pub user_code: String,
    pub expires: chrono::DateTime<Utc>,
    pub encoded_session: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuth2WebflowStateMetadata {
    pub redirect: Option<url::Url>,
}
