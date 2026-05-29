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
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::card::CardRecord;
use crate::repo::model::permission_share::{
    PermissionShareRepoError, PermissionShareRevisionRecord,
};
use crate::repo::permission_share::PermissionShareRepo;
use golem_common::model::account::{Account, AccountId};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{Card, CardId, CardManagedBy, CardParseError, PermissionPattern};
use golem_common::model::permission_share::{
    PermissionShare, PermissionShareCreation, PermissionShareData, PermissionShareId,
    PermissionShareName, PermissionShareRevision, PermissionShareUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AccountAction, AuthCtx, AuthorizationError};
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PermissionShareError {
    #[error("There is already a permission share with this name")]
    PermissionShareAlreadyExists,
    #[error("Permission share {0} not found")]
    PermissionShareNotFound(PermissionShareId),
    #[error("Permission share for name {0} not found")]
    PermissionShareByNameNotFound(PermissionShareName),
    #[error("Target account {0} not found")]
    TargetAccountNotFound(AccountId),
    #[error("Invalid permission grant {grant}: {message}")]
    InvalidGrant { grant: String, message: String },
    #[error("Permission grant recipient must be '*' or target account '{target_account}'")]
    InvalidRecipient { target_account: String },
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for PermissionShareError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::PermissionShareAlreadyExists => self.to_string(),
            Self::PermissionShareNotFound(_) => self.to_string(),
            Self::PermissionShareByNameNotFound(_) => self.to_string(),
            Self::TargetAccountNotFound(_) => self.to_string(),
            Self::InvalidGrant { .. } => self.to_string(),
            Self::InvalidRecipient { .. } => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(PermissionShareError, PermissionShareRepoError, AccountError);

pub struct PermissionShareService {
    permission_share_repo: Arc<dyn PermissionShareRepo>,
    account_service: Arc<AccountService>,
}

impl PermissionShareService {
    pub fn new(
        permission_share_repo: Arc<dyn PermissionShareRepo>,
        account_service: Arc<AccountService>,
    ) -> Self {
        Self {
            permission_share_repo,
            account_service,
        }
    }

    pub async fn create(
        &self,
        owner_account_id: AccountId,
        data: PermissionShareCreation,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        auth.authorize_account_action(owner_account_id, AccountAction::CreatePermissionShare)?;

        let target_account = self.get_account(data.target_account_id).await?;

        let id = PermissionShareId::new();
        let card = self.permission_share_card(id, &data.data, target_account.email.as_str())?;
        let revision = PermissionShareRevisionRecord::creation(
            id,
            data.name,
            data.data,
            auth.actor_account_id(),
        );

        match self
            .permission_share_repo
            .create(owner_account_id.0, data.target_account_id.0, revision, card)
            .await
        {
            Ok(record) => Ok(record.try_into()?),
            Err(PermissionShareRepoError::ShareViolatesUniqueness) => {
                Err(PermissionShareError::PermissionShareAlreadyExists)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn update(
        &self,
        permission_share_id: PermissionShareId,
        update: PermissionShareUpdate,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let mut share = self.get(permission_share_id, auth).await?;
        auth.authorize_account_action(
            share.owner_account_id,
            AccountAction::UpdatePermissionShare,
        )?;

        if share.revision != update.current_revision {
            return Err(PermissionShareError::ConcurrentModification);
        }

        let target_account = self.get_account(share.target_account_id).await?;
        let replacement_card = self.permission_share_card(
            permission_share_id,
            &update.data,
            target_account.email.as_str(),
        )?;

        share.revision = share.revision.next()?;
        share.name = update.name;
        share.data = update.data;

        let audit = DeletableRevisionAuditFields::new(auth.actor_account_id().0);

        match self
            .permission_share_repo
            .update(
                PermissionShareRevisionRecord::from_model(share, audit),
                replacement_card,
            )
            .await
        {
            Ok(record) => Ok(record.try_into()?),
            Err(PermissionShareRepoError::ShareViolatesUniqueness) => {
                Err(PermissionShareError::PermissionShareAlreadyExists)
            }
            Err(PermissionShareRepoError::ConcurrentModification) => {
                Err(PermissionShareError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn delete(
        &self,
        permission_share_id: PermissionShareId,
        current_revision: PermissionShareRevision,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let mut share = self.get(permission_share_id, auth).await?;
        auth.authorize_account_action(
            share.owner_account_id,
            AccountAction::DeletePermissionShare,
        )?;

        if share.revision != current_revision {
            return Err(PermissionShareError::ConcurrentModification);
        }

        share.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.actor_account_id().0);

        match self
            .permission_share_repo
            .delete(PermissionShareRevisionRecord::from_model(share, audit))
            .await
        {
            Ok(record) => Ok(record.try_into()?),
            Err(PermissionShareRepoError::ConcurrentModification) => {
                Err(PermissionShareError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn get(
        &self,
        permission_share_id: PermissionShareId,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let share: PermissionShare = self
            .permission_share_repo
            .get_by_id(permission_share_id.0)
            .await?
            .ok_or(PermissionShareError::PermissionShareNotFound(
                permission_share_id,
            ))?
            .try_into()?;

        self.authorize_view(&share, auth)?;

        Ok(share)
    }

    pub async fn get_by_owner_and_name(
        &self,
        owner_account_id: AccountId,
        name: &str,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        auth.authorize_account_action(owner_account_id, AccountAction::ViewPermissionShare)?;

        self.permission_share_repo
            .get_by_owner_and_name(owner_account_id.0, name)
            .await?
            .ok_or(PermissionShareError::PermissionShareNotFound(
                PermissionShareId::new(),
            ))?
            .try_into()
            .map_err(Into::into)
    }

    pub async fn get_for_owner(
        &self,
        owner_account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<PermissionShare>, PermissionShareError> {
        auth.authorize_account_action(owner_account_id, AccountAction::ViewPermissionShare)?;

        self.permission_share_repo
            .get_for_owner(owner_account_id.0)
            .await?
            .into_iter()
            .map(|record| record.try_into().map_err(Into::into))
            .collect()
    }

    pub async fn get_for_target(
        &self,
        target_account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<PermissionShare>, PermissionShareError> {
        auth.authorize_account_action(target_account_id, AccountAction::ViewPermissionShare)?;

        self.permission_share_repo
            .get_for_target(target_account_id.0)
            .await?
            .into_iter()
            .map(|record| record.try_into().map_err(Into::into))
            .collect()
    }

    pub async fn active_share_cards_for_target(
        &self,
        target_account_id: AccountId,
    ) -> Result<Vec<Card>, PermissionShareError> {
        self.permission_share_repo
            .active_cards_for_target(target_account_id.0)
            .await?
            .into_iter()
            .map(|record| {
                record
                    .try_into()
                    .map_err(PermissionShareRepoError::from)
                    .map_err(Into::into)
            })
            .collect()
    }

    async fn get_account(&self, account_id: AccountId) -> Result<Account, PermissionShareError> {
        self.account_service
            .get(account_id, &AuthCtx::System)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                    PermissionShareError::TargetAccountNotFound(account_id)
                }
                other => other.into(),
            })
    }

    fn permission_share_card(
        &self,
        permission_share_id: PermissionShareId,
        data: &PermissionShareData,
        target_account: &str,
    ) -> Result<CardRecord, PermissionShareError> {
        let parsed = self.parse_and_validate_data_for_target(data, target_account)?;
        let card_id = Uuid::now_v7();

        Ok(CardRecord::creation(
            CardId(card_id),
            Vec::new(),
            parsed.lower_positive,
            parsed.lower_negative,
            parsed.upper_positive,
            parsed.upper_negative,
            None,
            true,
            Some(CardManagedBy::PermissionShare {
                permission_share_id: permission_share_id.0,
            }),
        ))
    }

    fn parse_and_validate_data_for_target(
        &self,
        data: &PermissionShareData,
        target_account: &str,
    ) -> Result<ParsedPermissionShareData, PermissionShareError> {
        Ok(ParsedPermissionShareData {
            lower_positive: parse_and_validate_grants(&data.lower_positive, target_account)?,
            lower_negative: parse_and_validate_grants(&data.lower_negative, target_account)?,
            upper_positive: parse_and_validate_grants(&data.upper_positive, target_account)?,
            upper_negative: parse_and_validate_grants(&data.upper_negative, target_account)?,
        })
    }

    fn authorize_view(
        &self,
        share: &PermissionShare,
        auth: &AuthCtx,
    ) -> Result<(), PermissionShareError> {
        auth.authorize_account_action(share.owner_account_id, AccountAction::ViewPermissionShare)
            .or_else(|_| {
                auth.authorize_account_action(
                    share.target_account_id,
                    AccountAction::ViewPermissionShare,
                )
            })?;

        Ok(())
    }
}

struct ParsedPermissionShareData {
    lower_positive: Vec<PermissionPattern>,
    lower_negative: Vec<PermissionPattern>,
    upper_positive: Vec<PermissionPattern>,
    upper_negative: Vec<PermissionPattern>,
}

fn parse_and_validate_grants(
    grants: &[String],
    target_account: &str,
) -> Result<Vec<PermissionPattern>, PermissionShareError> {
    grants
        .iter()
        .map(|grant| {
            let permission =
                PermissionPattern::from_str(grant).map_err(|err| invalid_grant(grant, err))?;
            validate_recipient(permission.recipient(), target_account)?;
            Ok(permission)
        })
        .collect()
}

fn validate_recipient(
    recipient: &RecipientPattern,
    target_account: &str,
) -> Result<(), PermissionShareError> {
    match recipient {
        RecipientPattern::Any => Ok(()),
        RecipientPattern::Account { account } if account == target_account => Ok(()),
        _ => Err(PermissionShareError::InvalidRecipient {
            target_account: target_account.to_string(),
        }),
    }
}

fn invalid_grant(grant: &str, err: CardParseError) -> PermissionShareError {
    PermissionShareError::InvalidGrant {
        grant: grant.to_string(),
        message: err.to_string(),
    }
}
