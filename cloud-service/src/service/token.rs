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

use crate::model::{Token, UnsafeToken};
use crate::repo::account::AccountRepo;
use crate::repo::token::TokenRepo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::auth::TokenSecret;
use golem_common::model::AccountId;
use golem_common::model::TokenId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, error};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TokenServiceError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Unknown token: {0}")]
    UnknownToken(TokenId),
    #[error("Unknown token state: {0}")]
    UnknownTokenState(String),
    #[error("Arg Validation error: {}", .0.join(", "))]
    ArgValidation(Vec<String>),
    #[error("Internal repository error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error("Can't create known secret for account {account_id} - already exists for account {existing_account_id}")]
    InternalSecretAlreadyExists {
        account_id: AccountId,
        existing_account_id: AccountId,
    },
}

impl SafeDisplay for TokenServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AccountNotFound(_) => self.to_string(),
            Self::UnknownToken(_) => self.to_string(),
            Self::UnknownTokenState(_) => self.to_string(),
            Self::ArgValidation(_) => self.to_string(),
            Self::InternalRepoError(inner) => inner.to_safe_string(),
            Self::InternalSecretAlreadyExists { .. } => self.to_string(),
        }
    }
}

#[async_trait]
pub trait TokenService: Send + Sync {
    async fn get(&self, id: &TokenId) -> Result<Token, TokenServiceError>;

    async fn get_unsafe(&self, id: &TokenId) -> Result<UnsafeToken, TokenServiceError>;

    async fn get_by_secret(&self, secret: &TokenSecret)
        -> Result<Option<Token>, TokenServiceError>;

    async fn find(&self, account_id: &AccountId) -> Result<Vec<Token>, TokenServiceError>;

    async fn create(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
    ) -> Result<UnsafeToken, TokenServiceError>;

    async fn create_known_secret(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        secret: &TokenSecret,
    ) -> Result<(), TokenServiceError>;

    async fn delete(&self, id: &TokenId) -> Result<(), TokenServiceError>;
}

pub struct TokenServiceDefault {
    token_repo: Arc<dyn TokenRepo>,
    account_repo: Arc<dyn AccountRepo>,
}

impl TokenServiceDefault {
    pub fn new(token_repo: Arc<dyn TokenRepo>, account_repo: Arc<dyn AccountRepo>) -> Self {
        Self {
            token_repo,
            account_repo,
        }
    }

    async fn create_known_secret_unsafe(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        secret: &TokenSecret,
    ) -> Result<UnsafeToken, TokenServiceError> {
        let token_id = TokenId(Uuid::new_v4());

        let created_at = Utc::now();
        let token = Token {
            id: token_id,
            account_id: account_id.clone(),
            expires_at: *expires_at,
            created_at,
        };
        let unsafe_token = UnsafeToken::new(token, secret.clone());
        let record = unsafe_token.clone().into();

        match self.token_repo.create(&record).await {
            Ok(_) => Ok(unsafe_token),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }
}

#[async_trait]
impl TokenService for TokenServiceDefault {
    async fn get(&self, id: &TokenId) -> Result<Token, TokenServiceError> {
        match self.token_repo.get(&id.0).await {
            Ok(Some(record)) => {
                let token: Token = record.into();
                Ok(token)
            }
            Ok(None) => Err(TokenServiceError::UnknownToken(id.clone())),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn get_unsafe(&self, id: &TokenId) -> Result<UnsafeToken, TokenServiceError> {
        match self.token_repo.get(&id.0).await {
            Ok(Some(record)) => {
                let secret: TokenSecret = TokenSecret::new(record.secret);
                let data: Token = record.into();
                Ok(UnsafeToken { data, secret })
            }
            Ok(None) => Err(TokenServiceError::UnknownToken(id.clone())),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn get_by_secret(
        &self,
        secret: &TokenSecret,
    ) -> Result<Option<Token>, TokenServiceError> {
        match self.token_repo.get_by_secret(&secret.value).await {
            Ok(Some(record)) => {
                let token: Token = record.into();
                Ok(Some(token))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn find(&self, account_id: &AccountId) -> Result<Vec<Token>, TokenServiceError> {
        match self
            .token_repo
            .get_by_account(account_id.value.as_str())
            .await
        {
            Ok(tokens) => Ok(tokens.iter().map(|t| t.clone().into()).collect()),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn create(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
    ) -> Result<UnsafeToken, TokenServiceError> {
        let account = self.account_repo.get(account_id.value.as_str()).await?;
        if account.is_none() {
            return Err(TokenServiceError::AccountNotFound(account_id.clone()));
        }
        let secret = TokenSecret::new(Uuid::new_v4());
        self.create_known_secret_unsafe(account_id, expires_at, &secret)
            .await
    }

    async fn create_known_secret(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        secret: &TokenSecret,
    ) -> Result<(), TokenServiceError> {
        debug!("{} is authorised", account_id.value);
        match self.get_by_secret(secret).await? {
            Some(token) => Err(TokenServiceError::InternalSecretAlreadyExists {
                account_id: account_id.clone(),
                existing_account_id: token.account_id.clone(),
            }),
            None => {
                self.create_known_secret_unsafe(account_id, expires_at, secret)
                    .await?;
                Ok(())
            }
        }
    }

    async fn delete(&self, id: &TokenId) -> Result<(), TokenServiceError> {
        match self.token_repo.delete(&id.0).await {
            Ok(_) => Ok(()),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }
}
