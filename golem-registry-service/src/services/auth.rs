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

use super::account::{AccountError, AccountService};
use super::token::{TokenError, TokenService};
use crate::model::auth::AuthCtx;
use chrono::Utc;
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::{SafeDisplay, error_forwarding};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Could not authenticate user using token")]
    CouldNotAuthenticate,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AuthError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::CouldNotAuthenticate => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AuthError, AccountError, TokenError);

/// Note that only _direct_ access to entities is checked here. Listing of accounts / applications / environments has their
/// security enforced on the
pub struct AuthService {
    account_service: Arc<AccountService>,
    token_service: Arc<TokenService>,
}

impl AuthService {
    pub fn new(account_service: Arc<AccountService>, token_service: Arc<TokenService>) -> Self {
        Self {
            account_service,
            token_service,
        }
    }

    pub async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthError> {
        let token = self
            .token_service
            .get_by_secret(&token)
            .await
            .map_err(|err| match err {
                TokenError::TokenBySecretFound => AuthError::CouldNotAuthenticate,
                err => err.into(),
            })?;

        if token.expires_at <= Utc::now() {
            Err(AuthError::CouldNotAuthenticate)?
        };

        let account =
            self.account_service
                .get(&token.account_id)
                .await
                .map_err(|err| match err {
                    // This covers the account being deleted
                    AccountError::AccountNotFound(_) => AuthError::CouldNotAuthenticate,
                    other => other.into(),
                })?;

        let account_roles: HashSet<AccountRole> = HashSet::from_iter(account.roles.clone());

        Ok(AuthCtx {
            account_id: account.id,
            account_roles,
        })
    }
}
