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

use crate::repo::model::token::TokenRecord;
use crate::repo::token::TokenRepo;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenId;
use golem_common::model::auth::{TokenSecret, TokenWithSecret};
use golem_common::{SafeDisplay, error_forwarders};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Token secret already exists")]
    TokenSecretAlreadyExists,
    #[error("Token for id not found")]
    TokenNotFound(TokenId),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for TokenError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::TokenSecretAlreadyExists => self.to_safe_string(),
            Self::TokenNotFound(_) => self.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarders!(TokenError, RepoError);

pub struct TokenService {
    token_repo: Arc<dyn TokenRepo>,
}

impl TokenService {
    pub fn new(token_repo: Arc<dyn TokenRepo>) -> Self {
        Self { token_repo }
    }

    pub async fn get(&self, token_id: &TokenId) -> anyhow::Result<TokenWithSecret> {
        let record = self
            .token_repo
            .get_by_id(&token_id.0)
            .await?
            .ok_or(TokenError::TokenNotFound(token_id.clone()))?;

        Ok(record.into())
    }

    pub async fn get_by_secret(&self, _token_id: &TokenId) -> Result<TokenWithSecret, TokenError> {
        // TODO: missing in repo
        todo!()
    }

    pub async fn create(
        &self,
        account_id: AccountId,
        expires_at: DateTime<Utc>,
    ) -> Result<TokenWithSecret, TokenError> {
        let secret = TokenSecret::new_v4();
        self.create_known_secret(account_id, secret, expires_at)
            .await
    }

    pub async fn create_known_secret(
        &self,
        account_id: AccountId,
        secret: TokenSecret,
        expires_at: DateTime<Utc>,
    ) -> Result<TokenWithSecret, TokenError> {
        let created_at = Utc::now();
        let token_id = TokenId::new_v4();

        let record = TokenRecord {
            token_id: token_id.0,
            secret: secret.0,
            account_id: account_id.0,
            created_at: created_at.into(),
            expires_at: expires_at.into(),
        };

        let record = self
            .token_repo
            .create(record)
            .await?
            .ok_or(TokenError::TokenSecretAlreadyExists)?;

        Ok(record.into())
    }
}
