use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cloud_common::model::TokenId;
use cloud_common::model::TokenSecret;
use golem_common::model::AccountId;
use tracing::{debug, error};
use uuid::Uuid;

use crate::auth::AccountAuthorisation;
use crate::model::{Role, Token, UnsafeToken};
use crate::repo::token::TokenRepo;
use crate::repo::RepoError;
use crate::service::oauth2_token::{OAuth2TokenError, OAuth2TokenService};

#[derive(Debug, Clone)]
pub enum TokenServiceError {
    ArgValidation(Vec<String>),
    UnknownTokenId(TokenId),
    Unexpected(String),
    Unauthorized(String),
}

impl From<RepoError> for TokenServiceError {
    fn from(error: RepoError) -> Self {
        match error {
            RepoError::Internal(_) => TokenServiceError::Unexpected("DB call failed.".to_string()),
        }
    }
}

impl From<OAuth2TokenError> for TokenServiceError {
    fn from(error: OAuth2TokenError) -> Self {
        match error {
            OAuth2TokenError::Internal(message) => TokenServiceError::Unexpected(message),
            OAuth2TokenError::Unauthorized(message) => TokenServiceError::Unauthorized(message),
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
}

pub struct TokenServiceDefault {
    token_repo: Arc<dyn TokenRepo + Send + Sync>,
    oauth2_token_service: Arc<dyn OAuth2TokenService + Send + Sync>,
}

impl TokenServiceDefault {
    pub fn new(
        token_repo: Arc<dyn TokenRepo + Send + Sync>,
        oauth2_token_service: Arc<dyn OAuth2TokenService + Send + Sync>,
    ) -> Self {
        Self {
            token_repo,
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
            Err(TokenServiceError::Unauthorized(
                "Access to another account.".to_string(),
            ))
        }
    }

    fn check_admin(&self, auth: &AccountAuthorisation) -> Result<(), TokenServiceError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            Err(TokenServiceError::Unauthorized(
                "Admin access only.".to_string(),
            ))
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
            Ok(None) => Err(TokenServiceError::UnknownTokenId(id.clone())),
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
            Ok(None) => Err(TokenServiceError::UnknownTokenId(id.clone())),
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
            Some(token) => Err(TokenServiceError::Unexpected(format!(
                "Can't create known secret for account {} - already exists for account {}",
                account_id.value, token.account_id.value
            ))),
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
}

#[derive(Default)]
pub struct TokenServiceNoOp {}

#[async_trait]
impl TokenService for TokenServiceNoOp {
    async fn get(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<Token, TokenServiceError> {
        Ok(Token {
            id: id.clone(),
            account_id: auth.token.account_id.clone(),
            expires_at: Utc::now(),
            created_at: Utc::now(),
        })
    }

    async fn get_unsafe(
        &self,
        id: &TokenId,
        auth: &AccountAuthorisation,
    ) -> Result<UnsafeToken, TokenServiceError> {
        Ok(UnsafeToken {
            data: Token {
                id: id.clone(),
                account_id: auth.token.account_id.clone(),
                expires_at: Utc::now(),
                created_at: Utc::now(),
            },
            secret: TokenSecret::new(Uuid::new_v4()),
        })
    }

    async fn get_by_secret(
        &self,
        _secret: &TokenSecret,
    ) -> Result<Option<Token>, TokenServiceError> {
        Ok(None)
    }

    async fn find(
        &self,
        _account_id: &AccountId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<Token>, TokenServiceError> {
        Ok(vec![])
    }

    async fn create(
        &self,
        account_id: &AccountId,
        expires_at: &DateTime<Utc>,
        _auth: &AccountAuthorisation,
    ) -> Result<UnsafeToken, TokenServiceError> {
        Ok(UnsafeToken {
            data: Token {
                id: TokenId(Uuid::new_v4()),
                account_id: account_id.clone(),
                expires_at: *expires_at,
                created_at: Utc::now(),
            },
            secret: TokenSecret::new(Uuid::new_v4()),
        })
    }

    async fn create_known_secret(
        &self,
        _account_id: &AccountId,
        _expires_at: &DateTime<Utc>,
        _secret: &TokenSecret,
        _auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError> {
        Ok(())
    }

    async fn delete(
        &self,
        _id: &TokenId,
        _auth: &AccountAuthorisation,
    ) -> Result<(), TokenServiceError> {
        Ok(())
    }
}
