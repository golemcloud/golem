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
use crate::repo::card::CardRepo;
use crate::repo::model::account::AccountRepoError;
use crate::repo::model::card::CardRepoError;
use crate::services::account::{AccountError, AccountService};
use crate::services::permission_share::{PermissionShareError, PermissionShareService};
use chrono::Utc;
use golem_common::model::account::{Account, AccountId};
use golem_common::model::auth::{TokenId, TokenSecret};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{Card, EffectiveSurface};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{
    AdminImpersonationAuthCtx, AuthCtx, GlobalAction, UserAuthCtx,
};
use golem_service_base::repo::RepoError;
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

error_forwarding!(
    AuthError,
    AccountError,
    AccountRepoError,
    CardRepoError,
    RepoError,
    PermissionShareError
);

pub struct AuthService {
    account_repo: Arc<dyn AccountRepo>,
    account_service: Arc<AccountService>,
    card_repo: Arc<dyn CardRepo>,
    permission_share_service: Arc<PermissionShareService>,
}

impl AuthService {
    pub fn new(
        account_repo: Arc<dyn AccountRepo>,
        account_service: Arc<AccountService>,
        card_repo: Arc<dyn CardRepo>,
        permission_share_service: Arc<PermissionShareService>,
    ) -> Self {
        Self {
            account_repo,
            account_service,
            card_repo,
            permission_share_service,
        }
    }

    pub async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthError> {
        let record = self
            .account_repo
            .get_by_secret(token.secret())
            .await?
            .ok_or(AuthError::CouldNotAuthenticate)?;

        // IMPORTANT: make sure the token is still valid
        if *record.token_expires_at.as_utc() <= Utc::now() {
            warn!(
                "Tried to resolve an expired token {}",
                TokenId(record.token_id)
            );
            return Err(AuthError::CouldNotAuthenticate);
        };

        let impersonated_by = record.impersonated_by.map(AccountId);
        let target_account: Account = record.value.try_into()?;

        match impersonated_by {
            // Normal login flow
            None => {
                let account_roles = BTreeSet::from_iter(target_account.roles.clone());
                let effective_surface = self.materialize_effective_surface(&target_account).await?;

                Ok(AuthCtx::User(UserAuthCtx {
                    account_id: target_account.id,
                    account_roles,
                    account_plan_id: target_account.plan_id,
                    effective_surface,
                }))
            }
            // Impersonation flow
            Some(admin_account_id) => {
                // Ensure the admin account is still alive and still has impersonation rights
                let admin_account: Account = self
                    .account_service
                    .get(admin_account_id, &AuthCtx::System)
                    .await
                    .map_err(|_| AuthError::CouldNotAuthenticate)?;

                {
                    let account_roles = BTreeSet::from_iter(admin_account.roles.clone());
                    let effective_surface =
                        self.materialize_effective_surface(&admin_account).await?;
                    let admin_auth_ctx = AuthCtx::User(UserAuthCtx {
                        account_id: admin_account.id,
                        account_roles,
                        account_plan_id: admin_account.plan_id,
                        effective_surface,
                    });

                    if admin_auth_ctx
                        .authorize_global_action(GlobalAction::ImpersonateUser)
                        .is_err()
                    {
                        warn!(
                            "Admin that minted the token ({}), is no longer allowed to impersonate. Failing auth",
                            admin_account_id
                        );
                        return Err(AuthError::CouldNotAuthenticate);
                    };
                }

                let target_account_roles = BTreeSet::from_iter(target_account.roles.clone());
                let effective_surface = self.materialize_effective_surface(&target_account).await?;

                Ok(AuthCtx::AdminImpersonation(AdminImpersonationAuthCtx {
                    admin_account_id,
                    target_account_id: target_account.id,
                    target_account_roles,
                    target_account_plan_id: target_account.plan_id,
                    effective_surface,
                }))
            }
        }
    }

    async fn materialize_effective_surface(
        &self,
        account: &Account,
    ) -> Result<EffectiveSurface, AuthError> {
        let account_root_card: Card = self
            .card_repo
            .get(account.account_root_card_id)
            .await?
            .ok_or_else(|| {
                tracing::warn!(
                    "Account root card {} for account {} does not exist",
                    account.account_root_card_id,
                    account.id
                );
                AuthError::CouldNotAuthenticate
            })?
            .try_into()?;

        let share_cards = self
            .permission_share_service
            .active_share_cards_for_target(account.id)
            .await?;

        let mut cards = Vec::with_capacity(1 + share_cards.len());
        cards.push(account_root_card);
        cards.extend(share_cards);

        let account_recipient = RecipientPattern::Account {
            account: account.email.as_str().to_string(),
        };

        EffectiveSurface::from_cards(&cards, &account_recipient).map_err(|err| {
            AuthError::InternalError(anyhow::anyhow!(
                "Failed to materialize effective surface for account {}: {:?}",
                account.id,
                err
            ))
        })
    }
}
