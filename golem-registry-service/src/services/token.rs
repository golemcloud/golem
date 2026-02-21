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

use super::account::{AccountError, AccountService};
use crate::repo::model::token::TokenRecord;
use crate::repo::token::TokenRepo;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenId;
use golem_common::model::auth::{TokenSecret, TokenWithSecret};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::AccountAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Token secret already exists")]
    TokenSecretAlreadyExists,
    #[error("Token for id not found")]
    TokenNotFound(TokenId),
    #[error("Token for secret not found")]
    TokenBySecretNotFound,
    #[error("Parent account not found")]
    ParentAccountNotFound(AccountId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for TokenError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::TokenSecretAlreadyExists => self.to_string(),
            Self::TokenNotFound(_) => self.to_string(),
            Self::TokenBySecretNotFound => self.to_string(),
            Self::ParentAccountNotFound(_) => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(TokenError, RepoError, AccountError);

pub struct TokenService {
    token_repo: Arc<dyn TokenRepo>,
    account_service: Arc<AccountService>,
}

impl TokenService {
    pub fn new(token_repo: Arc<dyn TokenRepo>, account_service: Arc<AccountService>) -> Self {
        Self {
            token_repo,
            account_service,
        }
    }

    pub async fn get(
        &self,
        token_id: TokenId,
        auth: &AuthCtx,
    ) -> Result<TokenWithSecret, TokenError> {
        let token: TokenWithSecret = self
            .token_repo
            .get_by_id(token_id.0)
            .await?
            .ok_or(TokenError::TokenNotFound(token_id))?
            .into();

        auth.authorize_account_action(token.account_id, AccountAction::ViewToken)
            .map_err(|_| TokenError::TokenNotFound(token_id))?;

        Ok(token)
    }

    pub async fn get_by_secret(
        &self,
        secret: &TokenSecret,
        auth: &AuthCtx,
    ) -> Result<TokenWithSecret, TokenError> {
        let token: TokenWithSecret = self
            .token_repo
            .get_by_secret(secret.secret())
            .await?
            .ok_or(TokenError::TokenBySecretNotFound)?
            .into();

        auth.authorize_account_action(token.account_id, AccountAction::ViewToken)
            .map_err(|_| TokenError::TokenBySecretNotFound)?;

        Ok(token)
    }

    pub async fn get_optional_by_secret(
        &self,
        secret: &TokenSecret,
        auth: &AuthCtx,
    ) -> Result<Option<TokenWithSecret>, TokenError> {
        self.get_by_secret(secret, auth)
            .await
            .map(Some)
            .or_else(|err| match err {
                TokenError::TokenBySecretNotFound => Ok(None),
                other => Err(other),
            })
    }

    pub async fn list_in_account(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<TokenWithSecret>, TokenError> {
        self.account_service
            .get(account_id, auth)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                    TokenError::ParentAccountNotFound(account_id)
                }
                other => other.into(),
            })?;

        auth.authorize_account_action(account_id, AccountAction::ViewToken)?;

        let tokens: Vec<TokenWithSecret> = self
            .token_repo
            .get_by_account(account_id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(tokens)
    }

    pub async fn create(
        &self,
        account_id: AccountId,
        expires_at: DateTime<Utc>,
        auth: &AuthCtx,
    ) -> Result<TokenWithSecret, TokenError> {
        self.account_service
            .get(account_id, auth)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                    TokenError::ParentAccountNotFound(account_id)
                }
                other => other.into(),
            })?;

        auth.authorize_account_action(account_id, AccountAction::CreateToken)?;

        let secret = TokenSecret::new();
        self.create_known_secret(account_id, secret, expires_at)
            .await
    }

    async fn create_known_secret(
        &self,
        account_id: AccountId,
        secret: TokenSecret,
        expires_at: DateTime<Utc>,
    ) -> Result<TokenWithSecret, TokenError> {
        let created_at = Utc::now();
        let token_id = TokenId::new();

        let record = TokenRecord {
            token_id: token_id.0,
            secret: secret.into_secret(),
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

    pub async fn create_initial_tokens(
        &self,
        entries: impl IntoIterator<Item = &(AccountId, TokenSecret)>,
    ) -> Result<(), TokenError> {
        for (account_id, secret) in entries {
            let existing = self
                .get_optional_by_secret(secret, &AuthCtx::System)
                .await?;
            if existing.is_none() {
                self.create_known_secret(*account_id, secret.clone(), DateTime::<Utc>::MAX_UTC)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn delete(&self, token_id: TokenId, auth: &AuthCtx) -> Result<(), TokenError> {
        let token: TokenWithSecret = self
            .token_repo
            .get_by_id(token_id.0)
            .await?
            .ok_or(TokenError::TokenNotFound(token_id))?
            .into();

        auth.authorize_account_action(token.account_id, AccountAction::DeleteToken)?;

        self.token_repo.delete(token_id.0).await?;

        Ok(())
    }
}
