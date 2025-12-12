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

use crate::repo::account::AccountRepo;
use crate::repo::model::account::{AccountBySecretRecord, AccountRepoError};
use chrono::Utc;
use golem_common::model::account::Account;
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
use std::collections::BTreeSet;
use std::sync::Arc;
use tracing::warn;

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

error_forwarding!(AuthError, AccountRepoError);

pub struct AuthService {
    account_repo: Arc<dyn AccountRepo>,
}

impl AuthService {
    pub fn new(account_repo: Arc<dyn AccountRepo>) -> Self {
        Self { account_repo }
    }

    pub async fn authenticate_user(&self, token: TokenSecret) -> Result<UserAuthCtx, AuthError> {
        let record: AccountBySecretRecord = self
            .account_repo
            .get_by_secret(token.secret())
            .await?
            .ok_or(AuthError::CouldNotAuthenticate)?;

        // IMPORTANT: make sure the token is still valid
        if *record.token_expires_at.as_utc() <= Utc::now() {
            warn!("Tried to resolve an expired token {}", record.token_id);
            return Err(AuthError::CouldNotAuthenticate);
        };

        let account: Account = record.value.try_into()?;

        let account_roles: BTreeSet<AccountRole> = BTreeSet::from_iter(account.roles.clone());

        Ok(UserAuthCtx {
            account_id: account.id,
            account_roles,
            account_plan_id: account.plan_id,
        })
    }

    pub async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthError> {
        let user = self.authenticate_user(token).await?;
        Ok(AuthCtx::User(user))
    }
}
