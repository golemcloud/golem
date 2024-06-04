use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::Role;
use cloud_common::model::TokenId;
use golem_common::model::AccountId;
use tracing::info;

use crate::auth::AccountAuthorisation;
use crate::model::{OAuth2Provider, OAuth2Token};
use crate::repo::oauth2_token::{OAuth2TokenRecord, OAuth2TokenRepo};
use crate::repo::RepoError;

#[derive(Debug, Clone)]
pub enum OAuth2TokenError {
    Internal(String),
    Unauthorized(String),
}

impl OAuth2TokenError {
    pub fn internal<T: Display>(error: T) -> Self {
        OAuth2TokenError::Internal(error.to_string())
    }
}

impl From<RepoError> for OAuth2TokenError {
    fn from(error: RepoError) -> Self {
        OAuth2TokenError::internal(error)
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
}

impl OAuth2TokenServiceDefault {
    pub fn new(oauth2_token_repo: Arc<dyn OAuth2TokenRepo + Sync + Send>) -> Self {
        OAuth2TokenServiceDefault { oauth2_token_repo }
    }
}

#[async_trait]
impl OAuth2TokenService for OAuth2TokenServiceDefault {
    async fn upsert(&self, token: &OAuth2Token) -> Result<(), OAuth2TokenError> {
        info!(
            "Upsert token id for provider {}, external id {}, account id {}",
            token.provider, token.external_id, token.account_id
        );
        let record: OAuth2TokenRecord = token.clone().into();

        self.oauth2_token_repo
            .upsert(&record)
            .await
            .map_err(OAuth2TokenError::internal)
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
            .map_err(OAuth2TokenError::internal)?;

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
                return Err(OAuth2TokenError::Unauthorized("Unauthorized".to_string()));
            }
            self.oauth2_token_repo
                .clean_token_id(&token.provider, &token.external_id)
                .await?;
        }

        Ok(())
    }
}

#[derive(Default)]
pub struct OAuth2TokenServiceNoOp {}

#[async_trait]
impl OAuth2TokenService for OAuth2TokenServiceNoOp {
    async fn upsert(&self, _token: &OAuth2Token) -> Result<(), OAuth2TokenError> {
        Ok(())
    }
    async fn get(
        &self,
        _provider: &OAuth2Provider,
        _external_id: &str,
    ) -> Result<Option<OAuth2Token>, OAuth2TokenError> {
        Ok(None)
    }

    async fn unlink_token_id(
        &self,
        _token_id: &TokenId,
        _auth: &AccountAuthorisation,
    ) -> Result<(), OAuth2TokenError> {
        Ok(())
    }
}
