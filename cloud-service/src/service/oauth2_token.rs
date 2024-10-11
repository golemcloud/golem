use std::fmt::Debug;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::model::{OAuth2Provider, OAuth2Token};
use crate::repo::account::AccountRepo;
use crate::repo::oauth2_token::{OAuth2TokenRecord, OAuth2TokenRepo};
use crate::repo::token::TokenRepo;
use async_trait::async_trait;
use cloud_common::model::Role;
use cloud_common::model::TokenId;
use golem_common::model::AccountId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum OAuth2TokenError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Token Not Found: {0}")]
    TokenNotFound(TokenId),
    #[error("Internal error: Failed to convert OAuth2 Token record: {0}")]
    InternalConversionError(String),
    #[error("Internal repository error: {0}")]
    InternalRepoError(#[from] RepoError),
}

impl OAuth2TokenError {
    fn unauthorized(error: impl AsRef<str>) -> Self {
        Self::Unauthorized(error.as_ref().to_string())
    }
}

impl SafeDisplay for OAuth2TokenError {
    fn to_safe_string(&self) -> String {
        match self {
            OAuth2TokenError::Unauthorized(_) => self.to_string(),
            OAuth2TokenError::AccountNotFound(_) => self.to_string(),
            OAuth2TokenError::TokenNotFound(_) => self.to_string(),
            OAuth2TokenError::InternalConversionError(_) => self.to_string(),
            OAuth2TokenError::InternalRepoError(inner) => inner.to_safe_string(),
        }
    }
}

#[async_trait]
pub trait OAuth2TokenService {
    async fn upsert(&self, token: &OAuth2Token) -> Result<(), OAuth2TokenError>;

    async fn get(
        &self,
        provider: &OAuth2Provider,
        external_id: &str,
    ) -> Result<Option<OAuth2Token>, OAuth2TokenError>;

    async fn unlink_token_id(
        &self,
        token_id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<(), OAuth2TokenError>;
}

pub struct OAuth2TokenServiceDefault {
    oauth2_token_repo: Arc<dyn OAuth2TokenRepo + Sync + Send>,
    token_repo: Arc<dyn TokenRepo + Send + Sync>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
}

impl OAuth2TokenServiceDefault {
    pub fn new(
        oauth2_token_repo: Arc<dyn OAuth2TokenRepo + Sync + Send>,
        token_repo: Arc<dyn TokenRepo + Send + Sync>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
    ) -> Self {
        OAuth2TokenServiceDefault {
            oauth2_token_repo,
            token_repo,
            account_repo,
        }
    }
}

#[async_trait]
impl OAuth2TokenService for OAuth2TokenServiceDefault {
    async fn upsert(&self, token: &OAuth2Token) -> Result<(), OAuth2TokenError> {
        info!(
            "Upsert token id for provider {}, external id {}, account id {}",
            token.provider, token.external_id, token.account_id
        );

        let account_id = token.account_id.clone();
        let account = self.account_repo.get(account_id.value.as_str()).await?;
        if account.is_none() {
            return Err(OAuth2TokenError::AccountNotFound(account_id.clone()));
        }

        if let Some(token_id) = token.token_id.clone() {
            let token = self.token_repo.get(&token_id.0).await?;
            if token.is_none() {
                return Err(OAuth2TokenError::TokenNotFound(token_id.clone()));
            }
        }

        let record: OAuth2TokenRecord = token.clone().into();

        self.oauth2_token_repo.upsert(&record).await?;

        Ok(())
    }

    async fn get(
        &self,
        provider: &OAuth2Provider,
        external_id: &str,
    ) -> Result<Option<OAuth2Token>, OAuth2TokenError> {
        info!(
            "Getting by provider {} and external id {}",
            provider, external_id
        );
        let result = self
            .oauth2_token_repo
            .get(&provider.to_string(), external_id)
            .await?;

        let result = result
            .map(TryInto::<OAuth2Token>::try_into)
            .transpose()
            .map_err(OAuth2TokenError::InternalConversionError)?;

        Ok(result)
    }

    async fn unlink_token_id(
        &self,
        token_id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<(), OAuth2TokenError> {
        info!("Unlink token id {}", token_id);
        let tokens = self.oauth2_token_repo.get_by_token_id(&token_id.0).await?;

        // it is not expected that there will be more than one token records with same token_id and different account_id
        for token in tokens {
            let account_id = AccountId::from(token.account_id.as_str());
            if !auth.has_account_or_role(&account_id, &Role::Admin) {
                return Err(OAuth2TokenError::unauthorized("Unauthorized"));
            }
            self.oauth2_token_repo
                .clean_token_id(&token.provider, &token.external_id)
                .await?;
        }

        Ok(())
    }
}
