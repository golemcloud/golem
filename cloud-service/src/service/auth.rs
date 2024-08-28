use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::TokenSecret;

use crate::auth::AccountAuthorisation;
use crate::service::account_grant::{AccountGrantService, AccountGrantServiceError};
use crate::service::token::{TokenService, TokenServiceError};
use cloud_common::model::Role;

#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    #[error("Invalid Token: {0}")]
    InvalidToken(String),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl AuthServiceError {
    pub fn internal<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::Internal(anyhow::Error::msg(error.to_string()))
    }

    pub fn invalid_token<M>(error: M) -> Self
    where
        M: Display,
    {
        AuthServiceError::InvalidToken(error.to_string())
    }
}

impl From<TokenServiceError> for AuthServiceError {
    fn from(error: TokenServiceError) -> Self {
        match error {
            TokenServiceError::ArgValidation(_) => AuthServiceError::internal(error.to_string()),
            TokenServiceError::UnknownToken(id) => {
                AuthServiceError::invalid_token(format!("Invalid token id: {}", id))
            }
            TokenServiceError::AccountNotFound(_) => AuthServiceError::internal(error.to_string()),
            TokenServiceError::Internal(message) => AuthServiceError::internal(message),
            TokenServiceError::Unauthorized(message) => {
                AuthServiceError::internal(format!("Failed access with Admin account: {}", message))
            }
        }
    }
}

impl From<AccountGrantServiceError> for AuthServiceError {
    fn from(error: AccountGrantServiceError) -> Self {
        match error {
            AccountGrantServiceError::ArgValidation(_) => {
                AuthServiceError::internal(error.to_string())
            }
            AccountGrantServiceError::AccountNotFound(_) => {
                AuthServiceError::internal(error.to_string())
            }
            AccountGrantServiceError::Internal(message) => AuthServiceError::internal(message),
            AccountGrantServiceError::Unauthorized(message) => {
                AuthServiceError::internal(format!("Failed access with Admin account: {}", message))
            }
        }
    }
}

#[async_trait]
pub trait AuthService {
    async fn authorization(
        &self,
        secret: &TokenSecret,
    ) -> Result<AccountAuthorisation, AuthServiceError>;
}

pub struct AuthServiceDefault {
    token_service: Arc<dyn TokenService + Send + Sync>,
    account_grant_service: Arc<dyn AccountGrantService + Send + Sync>,
}

impl AuthServiceDefault {
    pub fn new(
        token_service: Arc<dyn TokenService + Send + Sync>,
        account_grant_service: Arc<dyn AccountGrantService + Send + Sync>,
    ) -> Self {
        AuthServiceDefault {
            token_service,
            account_grant_service,
        }
    }
}

#[async_trait]
impl AuthService for AuthServiceDefault {
    async fn authorization(
        &self,
        secret: &TokenSecret,
    ) -> Result<AccountAuthorisation, AuthServiceError> {
        let token = self
            .token_service
            .get_by_secret(secret)
            .await?
            .ok_or(AuthServiceError::invalid_token("Unknown token secret."))?;
        let mut account_roles = self
            .account_grant_service
            .get(&token.account_id, &AccountAuthorisation::admin())
            .await?;
        account_roles.append(&mut Role::all_project_roles()); // TODO; Capture them in account grants table and use monoidal addition of capabilities similar to project grants and capabilities
        let now = chrono::Utc::now();
        if token.expires_at > now {
            Ok(AccountAuthorisation::new(
                token,
                account_roles.into_iter().collect(),
            ))
        } else {
            Err(AuthServiceError::invalid_token("Expired auth token."))
        }
    }
}

#[derive(Default)]
pub struct AuthServiceNoOp {}

#[async_trait]
impl AuthService for AuthServiceNoOp {
    async fn authorization(
        &self,
        _secret: &TokenSecret,
    ) -> Result<AccountAuthorisation, AuthServiceError> {
        Ok(AccountAuthorisation::admin())
    }
}
