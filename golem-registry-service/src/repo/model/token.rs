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

use crate::repo::model::datetime::SqlDateTime;
use golem_common::model::account::AccountId;
use golem_common::model::auth::{Token, TokenId, TokenSecret, TokenWithSecret};
use sqlx::FromRow;
use std::fmt::Debug;
use uuid::Uuid;

#[derive(FromRow, Clone, PartialEq)]
pub struct TokenRecord {
    pub token_id: Uuid,
    pub secret: String,
    pub account_id: Uuid,
    pub created_at: SqlDateTime,
    pub expires_at: SqlDateTime,
}

impl Debug for TokenRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRecord")
            .field("token_id", &self.token_id)
            .field("account_id", &self.account_id)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl From<TokenRecord> for TokenWithSecret {
    fn from(value: TokenRecord) -> Self {
        TokenWithSecret {
            id: TokenId(value.token_id),
            secret: TokenSecret::trusted(value.secret),
            account_id: AccountId(value.account_id),
            created_at: value.created_at.into(),
            expires_at: value.expires_at.into(),
        }
    }
}

impl From<TokenRecord> for Token {
    fn from(value: TokenRecord) -> Self {
        Token {
            id: TokenId(value.token_id),
            account_id: AccountId(value.account_id),
            created_at: value.created_at.into(),
            expires_at: value.expires_at.into(),
        }
    }
}
