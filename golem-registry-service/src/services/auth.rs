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
use crate::repo::model::card::{CardRecord, CardRepoError};
use crate::services::account::{AccountError, AccountService};
use crate::services::permission_share::{PermissionShareError, PermissionShareService};
use chrono::Utc;
use golem_common::model::account::{Account, AccountId, TokenRootCardEpoch};
use golem_common::model::auth::{TokenId, TokenSecret};
use golem_common::model::card::{
    Card, CardAlgebraError, CardId, CardManagedBy, EffectiveSurface, PermissionPattern,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{
    AdminImpersonationAuthCtx, AuthCtx, GlobalAction, UserAuthCtx,
};
use golem_service_base::repo::RepoError;
use std::collections::BTreeSet;
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

const MAX_REDERIVE_ATTEMPTS: usize = 5;

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
                let token_root_card_id = self
                    .ensure_token_root_card(
                        target_account.id,
                        target_account.email.as_str().to_string(),
                        target_account.account_root_card_id,
                        target_account.token_root_card_id,
                        target_account.token_root_card_epoch,
                    )
                    .await?;

                Ok(AuthCtx::User(UserAuthCtx {
                    account_id: target_account.id,
                    account_roles,
                    account_plan_id: target_account.plan_id,
                    token_root_card_id: Some(token_root_card_id),
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
                    let admin_auth_ctx = AuthCtx::User(UserAuthCtx {
                        account_id: admin_account.id,
                        account_roles,
                        account_plan_id: admin_account.plan_id,
                        token_root_card_id: None,
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
                let token_root_card_id = self
                    .ensure_token_root_card(
                        target_account.id,
                        target_account.email.as_str().to_string(),
                        target_account.account_root_card_id,
                        target_account.token_root_card_id,
                        target_account.token_root_card_epoch,
                    )
                    .await?;

                Ok(AuthCtx::AdminImpersonation(AdminImpersonationAuthCtx {
                    admin_account_id,
                    target_account_id: target_account.id,
                    target_account_roles,
                    target_account_plan_id: target_account.plan_id,
                    token_root_card_id: Some(token_root_card_id),
                }))
            }
        }
    }

    async fn ensure_token_root_card(
        &self,
        account_id: AccountId,
        mut account_holder: String,
        mut account_root_card_id: CardId,
        snapshot_card_id: Option<CardId>,
        snapshot_epoch: TokenRootCardEpoch,
    ) -> Result<CardId, AuthError> {
        if let Some(card_id) = snapshot_card_id {
            return Ok(card_id);
        }

        let mut expected_epoch = snapshot_epoch;

        for _ in 0..MAX_REDERIVE_ATTEMPTS {
            let account_root_card: Card = self
                .card_repo
                .get(account_root_card_id)
                .await?
                .ok_or_else(|| {
                    tracing::warn!(
                        "Account root card {} for account {} does not exist",
                        account_root_card_id,
                        account_id
                    );
                    AuthError::CouldNotAuthenticate
                })?
                .try_into()?;

            let share_cards = self
                .permission_share_service
                .active_share_cards_for_target(account_id)
                .await?;

            let mut parent_ids = Vec::with_capacity(1 + share_cards.len());
            parent_ids.push(account_root_card_id);
            parent_ids.extend(share_cards.iter().map(|card| card.card_id));

            let mut parent_cards = Vec::with_capacity(1 + share_cards.len());
            parent_cards.push(account_root_card);
            parent_cards.extend(share_cards);

            let mut lower_positive = Vec::new();
            let mut lower_negative = Vec::new();
            let mut upper_positive = Vec::new();
            let mut upper_negative = Vec::new();
            for card in &parent_cards {
                add_card_grants(
                    &mut lower_positive,
                    &mut lower_negative,
                    &mut upper_positive,
                    &mut upper_negative,
                    card,
                );
            }

            EffectiveSurface::from_cards(&parent_cards, &account_holder)
                .and_then(|surface| surface.validates_derivation(&lower_positive, &upper_positive))
                .map_err(|err| invalid_token_root_derivation(account_id, err))?;

            if let Some(card_id) = self
                .insert_token_root_card(
                    account_id,
                    expected_epoch,
                    parent_ids,
                    lower_positive,
                    lower_negative,
                    upper_positive,
                    upper_negative,
                )
                .await?
            {
                return Ok(card_id);
            }

            // Creating the card failed, refetch account to ensure we are working with up-to-date data on next loop.
            // This will happen if a new permission share is concurrently created or invalidated

            let account = self
                .account_service
                .get(account_id, &AuthCtx::System)
                .await
                .map_err(|_| AuthError::CouldNotAuthenticate)?;

            if let Some(card_id) = account.token_root_card_id {
                return Ok(card_id);
            }

            account_holder = account.email.as_str().to_string();
            account_root_card_id = account.account_root_card_id;
            expected_epoch = account.token_root_card_epoch;
        }

        Err(AuthError::InternalError(anyhow::anyhow!(
            "Failed to rederive token root card for account {} after {} attempts",
            account_id,
            MAX_REDERIVE_ATTEMPTS
        )))
    }

    async fn insert_token_root_card(
        &self,
        account_id: AccountId,
        expected_epoch: TokenRootCardEpoch,
        parent_ids: Vec<CardId>,
        lower_positive: Vec<PermissionPattern>,
        lower_negative: Vec<PermissionPattern>,
        upper_positive: Vec<PermissionPattern>,
        upper_negative: Vec<PermissionPattern>,
    ) -> Result<Option<CardId>, AuthError> {
        let card_id = CardId(Uuid::now_v7());
        let card = CardRecord::creation(
            card_id,
            parent_ids,
            lower_positive,
            lower_negative,
            upper_positive,
            upper_negative,
            None,
            true,
            Some(CardManagedBy::TokenRoot { account_id }),
        );

        match self
            .card_repo
            .insert_token_root_card(account_id.0, expected_epoch.into(), card)
            .await
        {
            Ok(record) => Ok(Some(CardId(record.card_id))),
            Err(CardRepoError::ConcurrentModification | CardRepoError::ParentNotFound(_)) => {
                Ok(None)
            }
            Err(err) => Err(err.into()),
        }
    }
}

fn add_card_grants(
    lower_positive: &mut Vec<PermissionPattern>,
    lower_negative: &mut Vec<PermissionPattern>,
    upper_positive: &mut Vec<PermissionPattern>,
    upper_negative: &mut Vec<PermissionPattern>,
    card: &Card,
) {
    lower_positive.extend(card.lower_positive.clone());
    lower_negative.extend(card.lower_negative.clone());
    upper_positive.extend(card.upper_positive.clone());
    upper_negative.extend(card.upper_negative.clone());
}

fn invalid_token_root_derivation(account_id: AccountId, err: CardAlgebraError) -> AuthError {
    AuthError::InternalError(anyhow::anyhow!(
        "Failed to validate token root card derivation for account {}: {:?}",
        account_id,
        err
    ))
}
