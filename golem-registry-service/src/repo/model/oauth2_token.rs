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

use crate::model::login::OAuth2Token;
use anyhow::Context;
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenId;
use golem_common::model::login::OAuth2Provider;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct OAuth2TokenRecord {
    pub provider: String,
    pub external_id: String,
    pub token_id: Option<Uuid>,
    pub account_id: Uuid,
}

impl TryFrom<OAuth2TokenRecord> for OAuth2Token {
    type Error = RepoError;
    fn try_from(value: OAuth2TokenRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            provider: OAuth2Provider::from_str(&value.provider)
                .context("Failed converting oauth2 provider")?,
            external_id: value.external_id,
            account_id: AccountId(value.account_id),
            token_id: value.token_id.map(TokenId),
        })
    }
}

impl From<OAuth2Token> for OAuth2TokenRecord {
    fn from(value: OAuth2Token) -> Self {
        Self {
            provider: value.provider.to_string(),
            external_id: value.external_id,
            token_id: value.token_id.map(|tid| tid.0),
            account_id: value.account_id.0,
        }
    }
}
