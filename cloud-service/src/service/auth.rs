use async_trait::async_trait;
use cloud_common::model::TokenSecret;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::service::account_grant::{AccountGrantService, AccountGrantServiceError};
use crate::service::token::{TokenService, TokenServiceError};
use cloud_common::model::Role;
use golem_common::SafeDisplay;

#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    #[error("Invalid Token: {0}")]
    InvalidToken(String),
    #[error(transparent)]
    InternalAccountGrantError(#[from] AccountGrantServiceError),
    #[error(transparent)]
    InternalTokenServiceError(TokenServiceError),
}

impl AuthServiceError {
    fn invalid_token(error: impl AsRef<str>) -> Self {
        AuthServiceError::InvalidToken(error.as_ref().to_string())
    }
}

impl SafeDisplay for AuthServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            AuthServiceError::InvalidToken(_) => self.to_string(),
            AuthServiceError::InternalAccountGrantError(inner) => inner.to_safe_string(),
            AuthServiceError::InternalTokenServiceError(inner) => inner.to_safe_string(),
        }
    }
}

impl From<TokenServiceError> for AuthServiceError {
    fn from(error: TokenServiceError) -> Self {
        match error {
            TokenServiceError::UnknownToken(id) => {
                AuthServiceError::invalid_token(format!("Invalid token id: {}", id))
            }
            _ => AuthServiceError::InternalTokenServiceError(error),
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
