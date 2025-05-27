use crate::auth::AccountAuthorisation;
use crate::model::{Token, UnsafeToken};
use crate::repo::account::AccountRepo;
use crate::repo::oauth2_web_flow_state::{LinkedTokenState, OAuth2WebFlowStateRepo};
use crate::repo::token::TokenRepo;
use crate::service::oauth2_token::{OAuth2TokenError, OAuth2TokenService};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cloud_common::model::Role;
use cloud_common::model::TokenId;
use cloud_common::model::TokenSecret;
use golem_common::model::AccountId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, error};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TokenServiceError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
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
    #[error(transparent)]
    InternalTokenError(OAuth2TokenError),
    #[error("Internal serialization error: {context}: {error}")]
    InternalSerializationError {
        error: serde_json::Error,
        context: String,
    },
    #[error("Can't create known secret for account {account_id} - already exists for account {existing_account_id}")]
    InternalSecretAlreadyExists {
        account_id: AccountId,
        existing_account_id: AccountId,
    },
}

impl TokenServiceError {
    fn unauthorized(error: impl AsRef<str>) -> Self {
        Self::Unauthorized(error.as_ref().to_string())
    }
}

impl SafeDisplay for TokenServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            TokenServiceError::Unauthorized(_) => self.to_string(),
            TokenServiceError::AccountNotFound(_) => self.to_string(),
            TokenServiceError::UnknownToken(_) => self.to_string(),
            TokenServiceError::UnknownTokenState(_) => self.to_string(),
            TokenServiceError::ArgValidation(_) => self.to_string(),
            TokenServiceError::InternalRepoError(inner) => inner.to_safe_string(),
            TokenServiceError::InternalTokenError(inner) => inner.to_safe_string(),
            TokenServiceError::InternalSerializationError { .. } => self.to_string(),
            TokenServiceError::InternalSecretAlreadyExists { .. } => self.to_string(),
        }
    }
}

impl From<OAuth2TokenError> for TokenServiceError {
    fn from(error: OAuth2TokenError) -> Self {
        match error {
            OAuth2TokenError::AccountNotFound(id) => TokenServiceError::AccountNotFound(id),
            OAuth2TokenError::Unauthorized(message) => TokenServiceError::Unauthorized(message),
            _ => TokenServiceError::InternalTokenError(error),
        }
    }
}

#[async_trait]
pub trait TokenService {
    async fn get(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<Token, TokenServiceError>;

    async fn get_unsafe(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<UnsafeToken, TokenServiceError>;

    async fn get_by_secret(&self, secret: &TokenSecret)
        -> Result<Option<Token>, TokenServiceError>;

    async fn find(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Token>, TokenServiceError>;

    async fn create(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        auth: &AccountAuthorisation,
    ) -> Result<UnsafeToken, TokenServiceError>;

    async fn create_known_secret(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        secret: &TokenSecret,
        auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError>;

    async fn delete(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError>;

    async fn generate_temp_token_state(
        &self,
        redirect: Option<url::Url>,
    ) -> Result<String, TokenServiceError>;

    async fn valid_temp_token_state(&self, state: &str) -> Result<(), TokenServiceError>;

    async fn link_temp_token(
        &self,
        token_id: &TokenId,
        state: &str,
    ) -> Result<UnsafeTokenWithMetadata, TokenServiceError>;

    /// Returns None if the token exists, but has not yet been linked
    /// Returns Error if state is not found
    async fn get_temp_token(
        &self,
        state: &str,
    ) -> Result<Option<UnsafeTokenWithMetadata>, TokenServiceError>;
}

pub struct TokenServiceDefault {
    token_repo: Arc<dyn TokenRepo + Send + Sync>,
    oauth2_web_flow_state_repo: Arc<dyn OAuth2WebFlowStateRepo + Send + Sync>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
    oauth2_token_service: Arc<dyn OAuth2TokenService + Send + Sync>,
}

impl TokenServiceDefault {
    pub fn new(
        token_repo: Arc<dyn TokenRepo + Send + Sync>,
        oauth2_web_flow_state_repo: Arc<dyn OAuth2WebFlowStateRepo + Send + Sync>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
        oauth2_token_service: Arc<dyn OAuth2TokenService + Send + Sync>,
    ) -> Self {
        Self {
            token_repo,
            oauth2_web_flow_state_repo,
            account_repo,
            oauth2_token_service,
        }
    }

    fn check_authorization(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError> {
        if auth.has_account_or_role(account_id, &Role::Admin) {
            Ok(())
        } else {
            Err(TokenServiceError::unauthorized(
                "Access to another account.",
            ))
        }
    }

    fn check_admin(&self, auth: &AccountAuthorisation) -> Result<(), TokenServiceError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            Err(TokenServiceError::unauthorized("Admin access only."))
        }
    }

    async fn check_token_authorization_if_exists(
        &self,
        token_id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError> {
        match self.token_repo.get(&token_id.0).await {
            Ok(Some(record)) => {
                let token: Token = record.into();
                self.check_authorization(&token.account_id, auth)?;
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn create_known_secret_unsafe(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        secret: &TokenSecret,
        auth: &AccountAuthorisation,
    ) -> Result<UnsafeToken, TokenServiceError> {
        self.check_authorization(account_id, auth)?;
        let token_id = TokenId(Uuid::new_v4());
        self.check_token_authorization_if_exists(&token_id, auth)
            .await?;
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

    fn delete_expired_states(&self) {
        let oauth2_web_flow_state_repo = self.oauth2_web_flow_state_repo.clone();
        tokio::spawn(async move {
            let result = oauth2_web_flow_state_repo.delete_expired_states().await;
            if let Err(error) = result {
                error!("Failed to delete expired states. {}", error);
            }
        });
    }
}

#[async_trait]
impl TokenService for TokenServiceDefault {
    async fn get(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<Token, TokenServiceError> {
        match self.token_repo.get(&id.0).await {
            Ok(Some(record)) => {
                let token: Token = record.into();
                self.check_authorization(&token.account_id, auth)?;
                Ok(token)
            }
            Ok(None) => Err(TokenServiceError::UnknownToken(id.clone())),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn get_unsafe(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<UnsafeToken, TokenServiceError> {
        self.check_admin(auth)?;
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

    async fn find(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Token>, TokenServiceError> {
        self.check_authorization(account_id, auth)?;
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
        auth: &AccountAuthorisation,
    ) -> Result<UnsafeToken, TokenServiceError> {
        self.check_authorization(account_id, auth)?;
        debug!("{} is authorised", account_id.value);
        let account = self.account_repo.get(account_id.value.as_str()).await?;
        if account.is_none() {
            return Err(TokenServiceError::AccountNotFound(account_id.clone()));
        }
        let secret = TokenSecret::new(Uuid::new_v4());
        self.create_known_secret_unsafe(account_id, expires_at, &secret, auth)
            .await
    }

    async fn create_known_secret(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        secret: &TokenSecret,
        auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError> {
        self.check_authorization(account_id, auth)?;
        debug!("{} is authorised", account_id.value);
        match self.get_by_secret(secret).await? {
            Some(token) => Err(TokenServiceError::InternalSecretAlreadyExists {
                account_id: account_id.clone(),
                existing_account_id: token.account_id.clone(),
            }),
            None => {
                self.create_known_secret_unsafe(account_id, expires_at, secret, auth)
                    .await?;
                Ok(())
            }
        }
    }

    async fn delete(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError> {
        self.check_token_authorization_if_exists(id, auth).await?;
        self.oauth2_token_service.unlink_token_id(id, auth).await?;
        match self.token_repo.delete(&id.0).await {
            Ok(_) => Ok(()),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn generate_temp_token_state(
        &self,
        redirect: Option<url::Url>,
    ) -> Result<String, TokenServiceError> {
        self.delete_expired_states();

        let metadata = TempTokenMetadata { redirect };
        let metadata_bytes = serde_json::to_vec(&metadata).map_err(|e| {
            TokenServiceError::InternalSerializationError {
                error: e,
                context: "Failed to serialize temp token metadata".to_string(),
            }
        })?;

        Ok(self
            .oauth2_web_flow_state_repo
            .generate_temp_token_state(&metadata_bytes)
            .await?)
    }

    async fn valid_temp_token_state(&self, state: &str) -> Result<(), TokenServiceError> {
        self.delete_expired_states();

        match self
            .oauth2_web_flow_state_repo
            .valid_temp_token(state)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(TokenServiceError::UnknownTokenState(state.to_string())),
            Err(error) => {
                error!("Failed to validate temporary token. {}", error);
                Err(error.into())
            }
        }
    }

    async fn link_temp_token(
        &self,
        token_id: &TokenId,
        state: &str,
    ) -> Result<UnsafeTokenWithMetadata, TokenServiceError> {
        self.delete_expired_states();

        match self
            .oauth2_web_flow_state_repo
            .link_temp_token(&token_id.0, state)
            .await
        {
            Ok(Some(linked_token)) => {
                let token = UnsafeTokenWithMetadata::try_from(linked_token).map_err(|e| {
                    TokenServiceError::InternalSerializationError {
                        error: e,
                        context: "Failed to deserialize temp token".to_string(),
                    }
                })?;

                Ok(token)
            }
            Ok(None) => Err(TokenServiceError::UnknownTokenState(state.to_string())),
            Err(error) => {
                error!("Failed to link temporary token. {}", error);
                Err(error.into())
            }
        }
    }

    async fn get_temp_token(
        &self,
        state: &str,
    ) -> Result<Option<UnsafeTokenWithMetadata>, TokenServiceError> {
        self.delete_expired_states();

        let token_state = self
            .oauth2_web_flow_state_repo
            .get_temp_token(state)
            .await?;
        match token_state {
            LinkedTokenState::Linked(linked_token) => {
                let token = UnsafeTokenWithMetadata::try_from(linked_token).map_err(|e| {
                    TokenServiceError::InternalSerializationError {
                        error: e,
                        context: "Failed to deserialize temp token".to_string(),
                    }
                })?;
                Ok(Some(token))
            }
            LinkedTokenState::Pending => Ok(None),
            LinkedTokenState::NotFound => {
                Err(TokenServiceError::UnknownTokenState(state.to_string()))
            }
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

impl TryFrom<crate::repo::oauth2_web_flow_state::LinkedToken> for UnsafeTokenWithMetadata {
    type Error = serde_json::Error;

    fn try_from(
        linked_token: crate::repo::oauth2_web_flow_state::LinkedToken,
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
