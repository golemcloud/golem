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

use crate::repo::account::AccountRepo;
use crate::repo::model::account::{AccountBySecretRecord, AccountRepoError};
use chrono::Utc;
use golem_common::model::account::{Account, AccountId};
use golem_common::model::auth::TokenSecret;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{
    AdminImpersonationAuthCtx, AuthCtx, GlobalAction, UserAuthCtx,
};
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

    pub async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthError> {
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

        let target_account: Account = record.value.try_into()?;

        match record.impersonated_by {
            // Normal login flow
            None => {
                let account_roles = BTreeSet::from_iter(target_account.roles.clone());
                Ok(AuthCtx::User(UserAuthCtx {
                    account_id: target_account.id,
                    account_roles,
                    account_plan_id: target_account.plan_id,
                }))
            }
            // Impersonation flow
            Some(admin_uuid) => {
                // ensure the admin account is still alive and still has impersonation rights
                let admin_account: Account = self
                    .account_repo
                    .get_by_id(admin_uuid)
                    .await?
                    .ok_or(AuthError::CouldNotAuthenticate)?
                    .try_into()?;

                {
                    let account_roles = BTreeSet::from_iter(admin_account.roles.clone());
                    let admin_auth_ctx = AuthCtx::User(UserAuthCtx {
                        account_id: admin_account.id,
                        account_roles,
                        account_plan_id: admin_account.plan_id,
                    });

                    if admin_auth_ctx
                        .authorize_global_action(GlobalAction::ImpersonateUser)
                        .is_err()
                    {
                        warn!(
                            "Admin that minted the token ({admin_uuid}), is no longer allowed to impersonate. Failing auth"
                        );
                        return Err(AuthError::CouldNotAuthenticate);
                    };
                }

                let target_account_roles = BTreeSet::from_iter(target_account.roles.clone());
                Ok(AuthCtx::AdminImpersonation(AdminImpersonationAuthCtx {
                    admin_account_id: AccountId(admin_uuid),
                    target_account_id: target_account.id,
                    target_account_roles,
                    target_account_plan_id: target_account.plan_id,
                }))
            }
        }
    }
}
