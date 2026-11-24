// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use crate::services::decision::{Decisions, account_target};
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use chrono::{DateTime, Utc};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::TokenId;
use golem_common::model::auth::{TokenSecret, TokenWithSecret};
use golem_common::model::authority::{CatalogOperation, Operation, Resource};
use golem_common::model::card::owner::{AccountOwnerPattern, EmptyOwnerPattern};
use golem_common::model::card::{
    AccountTokenResourcePattern, AccountTokenVerb, ClassPermissionTarget, PermissionTarget,
    SystemResourcePattern, SystemVerb,
};
use golem_common::{SafeDisplay, error_forwarding};
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
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    decisions: Decisions,
}

impl TokenService {
    pub fn new(
        token_repo: Arc<dyn TokenRepo>,
        account_service: Arc<AccountService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
        decisions: Decisions,
    ) -> Self {
        Self {
            token_repo,
            account_service,
            registry_change_notifier,
            decisions,
        }
    }

    pub async fn get(
        &self,
        token_id: TokenId,
        auth: &AuthCtx,
    ) -> Result<TokenWithSecret, TokenError> {
        let record = self
            .token_repo
            .get_by_id(token_id.0)
            .await?
            .ok_or(TokenError::TokenNotFound(token_id))?;
        let account_email = record.account_email();
        let account_id = AccountId(record.token.account_id);
        let token: TokenWithSecret = record.token.into();

        self.authorize_token(
            auth,
            account_id,
            &account_email,
            CatalogOperation::ViewApiToken,
            AccountTokenVerb::View,
            Some(token_id),
        )
        .await
        .map_err(|_| TokenError::TokenNotFound(token_id))?;

        Ok(token)
    }

    /// Decides `operation` on the API tokens of `account_id`, from today's check of `verb` on the
    /// token (or on every token of the account).
    async fn authorize_token(
        &self,
        auth: &AuthCtx,
        account_id: AccountId,
        account_email: &AccountEmail,
        operation: CatalogOperation,
        verb: AccountTokenVerb,
        token: Option<TokenId>,
    ) -> Result<(), AuthorizationError> {
        let (resource, token_resource) = match token {
            Some(token_id) => (
                Resource::ApiToken(token_id),
                AccountTokenResourcePattern::Token(token_id),
            ),
            None => (Resource::None, AccountTokenResourcePattern::Any),
        };
        self.decisions
            .decide(
                auth,
                Operation::single(operation, resource, account_target(account_id)),
                account_token_permission_target(account_email, verb, token_resource),
            )
            .await
            .map(|_| ())
    }

    pub async fn get_by_secret(
        &self,
        secret: &TokenSecret,
        auth: &AuthCtx,
    ) -> Result<TokenWithSecret, TokenError> {
        let record = self
            .token_repo
            .get_by_secret(secret.secret())
            .await?
            .ok_or(TokenError::TokenBySecretNotFound)?;
        let account_email = record.account_email();
        let account_id = AccountId(record.token.account_id);
        let token: TokenWithSecret = record.token.into();

        self.authorize_token(
            auth,
            account_id,
            &account_email,
            CatalogOperation::ViewApiToken,
            AccountTokenVerb::View,
            Some(token.id),
        )
        .await
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
        let account =
            self.account_service
                .get(account_id, auth)
                .await
                .map_err(|err| match err {
                    AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                        TokenError::ParentAccountNotFound(account_id)
                    }
                    other => other.into(),
                })?;

        let tokens: Vec<TokenWithSecret> = self
            .token_repo
            .get_by_account(account_id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        let mut visible = Vec::with_capacity(tokens.len());
        for token in tokens {
            if self
                .authorize_token(
                    auth,
                    account.id,
                    &account.email,
                    CatalogOperation::ViewApiToken,
                    AccountTokenVerb::View,
                    Some(token.id),
                )
                .await
                .is_ok()
            {
                visible.push(token);
            }
        }
        Ok(visible)
    }

    pub async fn create(
        &self,
        account_id: AccountId,
        expires_at: DateTime<Utc>,
        auth: &AuthCtx,
    ) -> Result<TokenWithSecret, TokenError> {
        let account =
            self.account_service
                .get(account_id, auth)
                .await
                .map_err(|err| match err {
                    AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                        TokenError::ParentAccountNotFound(account_id)
                    }
                    other => other.into(),
                })?;

        self.authorize_token(
            auth,
            account.id,
            &account.email,
            CatalogOperation::CreateApiTokens,
            AccountTokenVerb::Create,
            None,
        )
        .await?;

        let secret = TokenSecret::new();
        self.create_known_secret(account_id, secret, expires_at, None)
            .await
    }

    async fn create_known_secret(
        &self,
        account_id: AccountId,
        secret: TokenSecret,
        expires_at: DateTime<Utc>,
        impersonated_by: Option<AccountId>,
    ) -> Result<TokenWithSecret, TokenError> {
        let created_at = Utc::now();
        let token_id = TokenId::new();

        let record = TokenRecord {
            token_id: token_id.0,
            secret: secret.into_secret(),
            account_id: account_id.0,
            created_at: created_at.into(),
            expires_at: expires_at.into(),
            impersonated_by: impersonated_by.map(|id| id.0),
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
                self.create_known_secret(
                    *account_id,
                    secret.clone(),
                    DateTime::<Utc>::MAX_UTC,
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Create an impersonation token. Only admin users may call this.
    /// The token's `account_id` is the target account; `impersonated_by` records the admin.
    /// When this token is used for authentication the resulting `AuthCtx` is
    /// `AdminImpersonation`, which uses the target account for access checks but the
    /// admin account for audit writes.
    pub async fn create_impersonation_token(
        &self,
        target_account_id: AccountId,
        expires_at: DateTime<Utc>,
        auth: &AuthCtx,
    ) -> Result<TokenWithSecret, TokenError> {
        authorize_system_permission(auth, SystemVerb::ImpersonateUser)?;

        // Verify the target account exists.
        self.account_service
            .get(target_account_id, &AuthCtx::System)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                    TokenError::ParentAccountNotFound(target_account_id)
                }
                other => other.into(),
            })?;

        let secret = TokenSecret::new();
        self.create_known_secret(
            target_account_id,
            secret,
            expires_at,
            Some(auth.actor_account_id()),
        )
        .await
    }

    pub async fn delete(&self, token_id: TokenId, auth: &AuthCtx) -> Result<(), TokenError> {
        let record = self
            .token_repo
            .get_by_id(token_id.0)
            .await?
            .ok_or(TokenError::TokenNotFound(token_id))?;
        let account_email = record.account_email();
        let account_id = AccountId(record.token.account_id);

        self.authorize_token(
            auth,
            account_id,
            &account_email,
            CatalogOperation::ViewApiToken,
            AccountTokenVerb::View,
            Some(token_id),
        )
        .await
        .map_err(|_| TokenError::TokenNotFound(token_id))?;

        self.authorize_token(
            auth,
            account_id,
            &account_email,
            CatalogOperation::RevokeApiToken,
            AccountTokenVerb::Delete,
            Some(token_id),
        )
        .await?;

        if let Some(account_id) = self.token_repo.delete(token_id.0).await? {
            let _ = account_id.signal_new_events_available(&self.registry_change_notifier);
        }

        Ok(())
    }
}

fn authorize_system_permission(auth: &AuthCtx, verb: SystemVerb) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::System(ClassPermissionTarget {
        verb: Some(verb),
        owner: EmptyOwnerPattern,
        resource: SystemResourcePattern,
    }))
}

fn account_token_permission_target(
    account_email: &AccountEmail,
    verb: AccountTokenVerb,
    resource: AccountTokenResourcePattern,
) -> PermissionTarget {
    PermissionTarget::AccountToken(ClassPermissionTarget {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: account_email.clone(),
        },
        resource,
    })
}
