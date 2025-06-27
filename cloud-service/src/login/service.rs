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

use super::oauth2_provider_client::{OAuth2ProviderClient, OAuth2ProviderClientError};
use super::oauth2_token_repo::{OAuth2TokenRecord, OAuth2TokenRepo};
use super::oauth2_web_flow_state_repo::{LinkedTokenState, OAuth2WebFlowStateRepo};
use crate::model::{AccountData, ExternalLogin, OAuth2Provider, OAuth2Token, UnsafeToken};
use crate::service::account::{AccountError, AccountService};
use crate::service::token::{TokenService, TokenServiceError};
use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::auth::TokenSecret;
use golem_common::model::AccountId;
use golem_common::model::TokenId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use tracing::debug;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error(transparent)]
    InternalAccountError(Box<AccountError>),
    #[error(transparent)]
    InternalOAuth2ProviderClientError(OAuth2ProviderClientError),
    #[error(transparent)]
    InternalRepoError(#[from] RepoError),
    #[error(transparent)]
    InternalTokenServiceError(Box<TokenServiceError>),

    #[error("External error: {0}")]
    External(String),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Unknown token state: {0}")]
    UnknownTokenState(String),
    #[error("Internal serialization error: {context}: {error}")]
    InternalSerializationError {
        error: serde_json::Error,
        context: String,
    },
}

impl LoginError {
    fn internal(error: impl AsRef<str>) -> Self {
        Self::Internal(error.as_ref().to_string())
    }

    fn external(error: impl AsRef<str>) -> Self {
        Self::External(error.as_ref().to_string())
    }
}

impl SafeDisplay for LoginError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalAccountError(inner) => inner.to_safe_string(),
            Self::InternalOAuth2ProviderClientError(inner) => inner.to_safe_string(),
            Self::InternalRepoError(inner) => inner.to_safe_string(),
            Self::InternalTokenServiceError(inner) => inner.to_safe_string(),

            Self::External(_) => self.to_string(),
            Self::Internal(_) => self.to_string(),
            Self::UnknownTokenState(_) => self.to_string(),
            Self::InternalSerializationError { .. } => self.to_string(),
        }
    }
}

impl From<OAuth2ProviderClientError> for LoginError {
    fn from(err: OAuth2ProviderClientError) -> Self {
        match err {
            OAuth2ProviderClientError::External(msg) => LoginError::external(msg),
            _ => LoginError::InternalOAuth2ProviderClientError(err),
        }
    }
}

impl From<AccountError> for LoginError {
    fn from(value: AccountError) -> Self {
        Self::InternalAccountError(Box::new(value))
    }
}

impl From<TokenServiceError> for LoginError {
    fn from(value: TokenServiceError) -> Self {
        Self::InternalTokenServiceError(Box::new(value))
    }
}

#[async_trait]
pub trait LoginService: Send + Sync {
    async fn oauth2(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<UnsafeToken, LoginError>;

    async fn generate_temp_token_state(
        &self,
        redirect: Option<url::Url>,
    ) -> Result<String, LoginError>;

    async fn link_temp_token(
        &self,
        token: &TokenId,
        state: &str,
    ) -> Result<UnsafeTokenWithMetadata, LoginError>;

    async fn unlink_temp_token(&self, token_id: &TokenId) -> Result<(), LoginError>;

    async fn get_temp_token(
        &self,
        state: &str,
    ) -> Result<Option<UnsafeTokenWithMetadata>, LoginError>;
}

pub struct LoginServiceDefault {
    client: Arc<dyn OAuth2ProviderClient>,
    account_service: Arc<dyn AccountService>,
    token_service: Arc<dyn TokenService>,
    oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
    oauth2_web_flow_state_repo: Arc<dyn OAuth2WebFlowStateRepo>,
}

impl LoginServiceDefault {
    pub fn new(
        client: Arc<dyn OAuth2ProviderClient>,
        account_service: Arc<dyn AccountService>,
        token_service: Arc<dyn TokenService>,
        oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
        oauth2_web_flow_state_repo: Arc<dyn OAuth2WebFlowStateRepo>,
    ) -> Self {
        Self {
            client,
            account_service,
            token_service,
            oauth2_token_repo,
            oauth2_web_flow_state_repo,
        }
    }

    async fn make_account(
        &self,
        _provider: &OAuth2Provider,
        external_login: &ExternalLogin,
    ) -> Result<AccountId, LoginError> {
        let email = external_login
            .email
            .clone()
            .ok_or(LoginError::External(format!(
                "No user email from OAuth2 Provider for login {}",
                external_login.external_id
            )))?;

        let name = external_login
            .name
            .clone()
            .unwrap_or(external_login.external_id.clone());

        let fresh_account_id = AccountId::generate();

        let account = self
            .account_service
            .create(&fresh_account_id, &AccountData { name, email })
            .await?;

        Ok(account.id)
    }

    async fn make_token(
        &self,
        provider: &OAuth2Provider,
        external_login: &ExternalLogin,
        account_id: &AccountId,
    ) -> Result<UnsafeToken, LoginError> {
        let expiration = Utc::now()
            // Ten years.
            .checked_add_months(chrono::Months::new(10 * 12))
            .ok_or(LoginError::internal("Failed to calculate token expiry"))?;

        let unsafe_token = self.token_service.create(account_id, &expiration).await?;

        {
            let token = unsafe_token.data.clone();

            let oauth2_token = crate::model::OAuth2Token {
                provider: provider.clone(),
                external_id: external_login.external_id.clone(),
                account_id: account_id.clone(),
                token_id: Some(token.id),
            };

            let record: OAuth2TokenRecord = oauth2_token.into();

            self.oauth2_token_repo.upsert(&record).await?;
        }

        Ok(unsafe_token)
    }
}

#[async_trait]
impl LoginService for LoginServiceDefault {
    async fn oauth2(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<UnsafeToken, LoginError> {
        self.oauth2_web_flow_state_repo
            .delete_expired_states()
            .await?;
        let external_login = self.client.external_user_id(provider, access_token).await?;

        let existing_data = self
            .oauth2_token_repo
            .get(&provider.to_string(), &external_login.external_id)
            .await?
            .map(TryInto::<OAuth2Token>::try_into)
            .transpose()
            .map_err(LoginError::Internal)?;

        let account_id = match existing_data.clone() {
            Some(token) => token.account_id,
            None => self.make_account(provider, &external_login).await?,
        };

        let unsafe_token = match existing_data.and_then(|token| token.token_id) {
            Some(token_id) => self.token_service.get_unsafe(&token_id).await?,
            None => {
                self.make_token(provider, &external_login, &account_id)
                    .await?
            }
        };
        Ok(unsafe_token)
    }

    async fn generate_temp_token_state(
        &self,
        redirect: Option<url::Url>,
    ) -> Result<String, LoginError> {
        let metadata = TempTokenMetadata { redirect };
        let metadata_bytes =
            serde_json::to_vec(&metadata).map_err(|e| LoginError::InternalSerializationError {
                error: e,
                context: "Failed to serialize temp token metadata".to_string(),
            })?;

        let token_state = self
            .oauth2_web_flow_state_repo
            .generate_temp_token_state(&metadata_bytes)
            .await?;

        Ok(token_state)
    }

    async fn link_temp_token(
        &self,
        token_id: &TokenId,
        state: &str,
    ) -> Result<UnsafeTokenWithMetadata, LoginError> {
        debug!("Get link temp token {}", token_id);
        self.oauth2_web_flow_state_repo
            .delete_expired_states()
            .await?;

        match self
            .oauth2_web_flow_state_repo
            .link_temp_token(&token_id.0, state)
            .await
        {
            Ok(Some(linked_token)) => {
                let token = UnsafeTokenWithMetadata::try_from(linked_token).map_err(|e| {
                    LoginError::InternalSerializationError {
                        error: e,
                        context: "Failed to deserialize temp token".to_string(),
                    }
                })?;

                Ok(token)
            }
            Ok(None) => Err(LoginError::UnknownTokenState(state.to_string())),
            Err(error) => {
                error!("Failed to link temporary token. {}", error);
                Err(error.into())
            }
        }
    }

    async fn unlink_temp_token(&self, token_id: &TokenId) -> Result<(), LoginError> {
        debug!("Unlink temp token id {}", token_id);

        let tokens = self.oauth2_token_repo.get_by_token_id(&token_id.0).await?;

        // it is not expected that there will be more than one token records with same token_id and different account_id
        for token in tokens {
            self.oauth2_token_repo
                .clean_token_id(&token.provider, &token.external_id)
                .await?;
        }

        Ok(())
    }

    async fn get_temp_token(
        &self,
        state: &str,
    ) -> Result<Option<UnsafeTokenWithMetadata>, LoginError> {
        debug!("Get temp token by state");
        self.oauth2_web_flow_state_repo
            .delete_expired_states()
            .await?;

        let token_state = self
            .oauth2_web_flow_state_repo
            .get_temp_token(state)
            .await?;

        match token_state {
            LinkedTokenState::Linked(linked_token) => {
                let token = UnsafeTokenWithMetadata::try_from(linked_token).map_err(|e| {
                    LoginError::InternalSerializationError {
                        error: e,
                        context: "Failed to deserialize temp token".to_string(),
                    }
                })?;
                Ok(Some(token))
            }
            LinkedTokenState::Pending => Ok(None),
            LinkedTokenState::NotFound => Err(LoginError::UnknownTokenState(state.to_string())),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TempTokenMetadata {
    pub redirect: Option<url::Url>,
}

#[derive(Debug, Clone)]
pub struct UnsafeTokenWithMetadata {
    pub token: UnsafeToken,
    pub metadata: TempTokenMetadata,
}

impl TryFrom<super::oauth2_web_flow_state_repo::LinkedToken> for UnsafeTokenWithMetadata {
    type Error = serde_json::Error;

    fn try_from(
        linked_token: super::oauth2_web_flow_state_repo::LinkedToken,
    ) -> Result<Self, Self::Error> {
        let secret: TokenSecret = TokenSecret::new(linked_token.token.secret);
        let metadata: TempTokenMetadata = serde_json::from_slice(&linked_token.metadata)?;
        let token = UnsafeToken {
            data: linked_token.token.into(),
            secret,
        };
        Ok(UnsafeTokenWithMetadata { token, metadata })
    }
}
