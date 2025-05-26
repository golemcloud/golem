use std::str::FromStr;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::config::{AccountConfig, AccountsConfig};
use crate::model::{AccountData, ExternalLogin, OAuth2Provider, UnsafeToken};
use crate::service::account::{AccountError, AccountService};
use crate::service::account_grant::AccountGrantService;
use crate::service::oauth2_provider_client::{OAuth2ProviderClient, OAuth2ProviderClientError};
use crate::service::oauth2_token::{OAuth2TokenError, OAuth2TokenService};
use crate::service::token::{TokenService, TokenServiceError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cloud_common::model::{TokenId, TokenSecret};
use golem_common::model::AccountId;
use golem_common::SafeDisplay;
use tracing::info;

use super::token::UnsafeTokenWithMetadata;

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("External error: {0}")]
    External(String),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error(transparent)]
    InternalAccountError(#[from] AccountError),
    #[error(transparent)]
    InternalOAuth2ProviderClientError(OAuth2ProviderClientError),
    #[error(transparent)]
    InternalOAuth2TokenError(#[from] OAuth2TokenError),
    #[error(transparent)]
    InternalTokenServiceError(TokenServiceError),
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
            LoginError::External(_) => self.to_string(),
            LoginError::Internal(_) => self.to_string(),
            LoginError::InternalAccountError(inner) => inner.to_safe_string(),
            LoginError::InternalOAuth2ProviderClientError(inner) => inner.to_safe_string(),
            LoginError::InternalOAuth2TokenError(inner) => inner.to_safe_string(),
            LoginError::InternalTokenServiceError(inner) => inner.to_safe_string(),
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

impl From<TokenServiceError> for LoginError {
    fn from(err: TokenServiceError) -> Self {
        match err {
            TokenServiceError::UnknownTokenState(_) => LoginError::external(err.to_string()),
            _ => LoginError::InternalTokenServiceError(err),
        }
    }
}

#[async_trait]
pub trait LoginService {
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

    async fn get_temp_token(
        &self,
        state: &str,
    ) -> Result<Option<UnsafeTokenWithMetadata>, LoginError>;

    async fn create_initial_users(&self) -> Result<(), LoginError>;
}

pub struct LoginServiceDefault {
    client: Arc<dyn OAuth2ProviderClient + Send + Sync>,
    account_service: Arc<dyn AccountService + Send + Sync>,
    grant_service: Arc<dyn AccountGrantService + Send + Sync>,
    token_service: Arc<dyn TokenService + Send + Sync>,
    oauth2_token_service: Arc<dyn OAuth2TokenService + Send + Sync>,
    accounts_config: AccountsConfig,
}

impl LoginServiceDefault {
    pub fn new(
        client: Arc<dyn OAuth2ProviderClient + Send + Sync>,
        account_service: Arc<dyn AccountService + Send + Sync>,
        grant_service: Arc<dyn AccountGrantService + Send + Sync>,
        token_service: Arc<dyn TokenService + Send + Sync>,
        oauth2_token_service: Arc<dyn OAuth2TokenService + Send + Sync>,
        accounts_config: AccountsConfig,
    ) -> LoginServiceDefault {
        LoginServiceDefault {
            client,
            account_service,
            grant_service,
            token_service,
            oauth2_token_service,
            accounts_config,
        }
    }

    async fn make_account(
        &self,
        _provider: &OAuth2Provider,
        external_login: &ExternalLogin,
        authorisation: &AccountAuthorisation,
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
            .create(
                &fresh_account_id,
                &AccountData { name, email },
                authorisation,
            )
            .await?;
        Ok(account.id)
    }

    async fn make_token(
        &self,
        provider: &OAuth2Provider,
        external_login: &ExternalLogin,
        account_id: &AccountId,
        authorisation: &AccountAuthorisation,
    ) -> Result<UnsafeToken, LoginError> {
        let expiration = Utc::now()
            // Ten years.
            .checked_add_months(chrono::Months::new(10 * 12))
            .ok_or(LoginError::internal("Failed to calculate token expiry"))?;

        let unsafe_token = self
            .token_service
            .create(account_id, &expiration, authorisation)
            .await?;
        let token = unsafe_token.data.clone();
        self.oauth2_token_service
            .upsert(&crate::model::OAuth2Token {
                provider: provider.clone(),
                external_id: external_login.external_id.clone(),
                account_id: account_id.clone(),
                token_id: Some(token.id),
            })
            .await?;
        Ok(unsafe_token)
    }

    async fn create_account(&self, account_config: &AccountConfig) -> Result<(), LoginError> {
        info!(
            "Creating initial account({}, {}).",
            account_config.id, account_config.name
        );
        // This unwrap is infallible.
        let account_id = AccountId::from_str(&account_config.id).unwrap();
        self.account_service
            .create(
                &account_id,
                &AccountData {
                    name: account_config.name.clone(),
                    email: account_config.email.clone(),
                },
                &AccountAuthorisation::admin(),
            )
            .await
            .ok();
        self.grant_service
            .add(
                &account_id,
                &account_config.role,
                &AccountAuthorisation::admin(),
            )
            .await
            .ok();
        self.token_service
            .create_known_secret(
                &account_id,
                &DateTime::<Utc>::MAX_UTC,
                &TokenSecret::new(account_config.token),
                &AccountAuthorisation::admin(),
            )
            .await
            .ok();
        Ok(())
    }
}

#[async_trait]
impl LoginService for LoginServiceDefault {
    async fn oauth2(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<UnsafeToken, LoginError> {
        let external_login = self.client.external_user_id(provider, access_token).await?;
        let existing_data = self
            .oauth2_token_service
            .get(provider, &external_login.external_id)
            .await?;
        let account_id = match existing_data.clone() {
            Some(token) => token.account_id,
            None => {
                self.make_account(provider, &external_login, &AccountAuthorisation::admin())
                    .await?
            }
        };
        let unsafe_token = match existing_data.and_then(|token| token.token_id) {
            Some(token_id) => {
                self.token_service
                    .get_unsafe(&token_id, &AccountAuthorisation::admin())
                    .await?
            }
            None => {
                self.make_token(
                    provider,
                    &external_login,
                    &account_id,
                    &AccountAuthorisation::admin(),
                )
                .await?
            }
        };
        Ok(unsafe_token)
    }

    async fn generate_temp_token_state(
        &self,
        redirect: Option<url::Url>,
    ) -> Result<String, LoginError> {
        let state = self
            .token_service
            .generate_temp_token_state(redirect)
            .await?;
        Ok(state)
    }

    async fn link_temp_token(
        &self,
        token: &TokenId,
        state: &str,
    ) -> Result<UnsafeTokenWithMetadata, LoginError> {
        let token = self.token_service.link_temp_token(token, state).await?;
        Ok(token)
    }

    async fn get_temp_token(
        &self,
        state: &str,
    ) -> Result<Option<UnsafeTokenWithMetadata>, LoginError> {
        let token = self.token_service.get_temp_token(state).await?;
        Ok(token)
    }

    async fn create_initial_users(&self) -> Result<(), LoginError> {
        for account_config in self.accounts_config.accounts.values() {
            self.create_account(account_config).await?
        }
        Ok(())
    }
}
