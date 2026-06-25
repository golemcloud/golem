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
use super::registry_change_notifier::{RegistryChangeNotifier, RequiresNotificationSignalExt};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::card::CardRecord;
use crate::repo::model::permission_share::{
    PermissionShareAuthExtRevisionRecord, PermissionShareRepoError, PermissionShareRevisionRecord,
};
use crate::repo::permission_share::PermissionShareRepo;
use golem_common::model::account::{Account, AccountEmail, AccountId};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AccountPermissionShareResourcePattern, AccountPermissionShareVerb, Card, CardAlgebraError,
    CardId, CardManagedBy, CardManagedByPermissionShare, CardParseError, ClassPermissionTarget,
    EffectiveSurface, PermissionPattern, PermissionTarget,
};
use golem_common::model::permission_share::{
    PermissionShare, PermissionShareCreation, PermissionShareData, PermissionShareId,
    PermissionShareName, PermissionShareRevision, PermissionShareUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::str::FromStr;
use std::sync::Arc;

const MAX_CARD_TREE_DELETE_ATTEMPTS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum PermissionShareError {
    #[error("There is already a permission share with this name")]
    PermissionShareAlreadyExists,
    #[error("Permission share {0} not found")]
    PermissionShareNotFound(PermissionShareId),
    #[error("Permission share for name {0} not found")]
    PermissionShareByNameNotFound(PermissionShareName),
    #[error("Target account {0} not found")]
    TargetAccountNotFound(String),
    #[error("Invalid permission grant {grant}: {message}")]
    InvalidGrant { grant: String, message: String },
    #[error("Permission grant recipient must be '*' or target account '{target_account}'")]
    InvalidRecipient { target_account: String },
    #[error("Permission grants are not delegable by the caller: {0}")]
    GrantNotDelegable(String),
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
            Self::GrantNotDelegable(_) => self.to_string(),
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
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl PermissionShareService {
    pub fn new(
        permission_share_repo: Arc<dyn PermissionShareRepo>,
        account_service: Arc<AccountService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            permission_share_repo,
            account_service,
            registry_change_notifier,
        }
    }

    pub async fn create(
        &self,
        owner_account_id: AccountId,
        data: PermissionShareCreation,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let owner_account = self.get_account(owner_account_id, auth).await?;
        authorize_permission_share_permission(
            auth,
            &owner_account.email,
            AccountPermissionShareVerb::Create,
            AccountPermissionShareResourcePattern::Any,
        )?;

        let target_account = self
            .get_account_by_email(&data.target_account_email)
            .await?;

        let id = PermissionShareId::new();
        let card =
            self.permission_share_card(id, &data.data, target_account.email.as_str(), auth)?;
        let revision = PermissionShareRevisionRecord::creation(
            id,
            data.name,
            data.data,
            auth.actor_account_id(),
        );

        for attempt in 0..MAX_CARD_TREE_DELETE_ATTEMPTS {
            match self
                .permission_share_repo
                .create(
                    owner_account_id.0,
                    target_account.id.0,
                    revision.clone(),
                    card.clone(),
                )
                .await
            {
                Ok(record) => return Ok(record.try_into()?),
                Err(PermissionShareRepoError::CardTreeChangedDuringDelete)
                    if attempt + 1 < MAX_CARD_TREE_DELETE_ATTEMPTS =>
                {
                    continue;
                }
                Err(PermissionShareRepoError::ShareViolatesUniqueness) => {
                    return Err(PermissionShareError::PermissionShareAlreadyExists);
                }
                Err(other) => return Err(other.into()),
            }
        }

        Err(PermissionShareError::ConcurrentModification)
    }

    pub async fn update(
        &self,
        permission_share_id: PermissionShareId,
        update: PermissionShareUpdate,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let record = self.get_record_by_id(permission_share_id).await?;
        let owner_account_email = record.owner_account_email();
        let target_account_email = record.target_account_email();
        let mut share: PermissionShare = record.share.try_into()?;

        self.authorize_view(&share, &owner_account_email, &target_account_email, auth)
            .map_err(|err| match err {
                PermissionShareError::Unauthorized(_) => {
                    PermissionShareError::PermissionShareNotFound(permission_share_id)
                }
                other => other,
            })?;

        authorize_permission_share_permission(
            auth,
            &owner_account_email,
            AccountPermissionShareVerb::Update,
            AccountPermissionShareResourcePattern::Name(share.name.clone()),
        )?;

        if share.revision != update.current_revision {
            return Err(PermissionShareError::ConcurrentModification);
        }

        let replacement_card = self.permission_share_card(
            permission_share_id,
            &update.data,
            target_account_email.as_str(),
            auth,
        )?;

        share.revision = share.revision.next()?;
        share.name = update.name;
        share.data = update.data;

        let audit = DeletableRevisionAuditFields::new(auth.actor_account_id().0);

        let revision = PermissionShareRevisionRecord::from_model(share, audit);

        for attempt in 0..MAX_CARD_TREE_DELETE_ATTEMPTS {
            match self
                .permission_share_repo
                .update(revision.clone(), replacement_card.clone())
                .await
            {
                Ok(record) => {
                    let permission_share: PermissionShare = record
                        .signal_new_events_available(&self.registry_change_notifier)
                        .try_into()?;

                    return Ok(permission_share);
                }
                Err(PermissionShareRepoError::CardTreeChangedDuringDelete)
                    if attempt + 1 < MAX_CARD_TREE_DELETE_ATTEMPTS =>
                {
                    continue;
                }
                Err(PermissionShareRepoError::ShareViolatesUniqueness) => {
                    return Err(PermissionShareError::PermissionShareAlreadyExists);
                }
                Err(PermissionShareRepoError::ConcurrentModification) => {
                    return Err(PermissionShareError::ConcurrentModification);
                }
                Err(other) => return Err(other.into()),
            }
        }

        Err(PermissionShareError::ConcurrentModification)
    }

    pub async fn delete(
        &self,
        permission_share_id: PermissionShareId,
        current_revision: PermissionShareRevision,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let record = self.get_record_by_id(permission_share_id).await?;
        let owner_account_email = record.owner_account_email();
        let target_account_email = record.target_account_email();
        let mut share: PermissionShare = record.share.try_into()?;

        self.authorize_view(&share, &owner_account_email, &target_account_email, auth)
            .map_err(|err| match err {
                PermissionShareError::Unauthorized(_) => {
                    PermissionShareError::PermissionShareNotFound(permission_share_id)
                }
                other => other,
            })?;

        authorize_permission_share_permission(
            auth,
            &owner_account_email,
            AccountPermissionShareVerb::Delete,
            AccountPermissionShareResourcePattern::Name(share.name.clone()),
        )?;

        if share.revision != current_revision {
            return Err(PermissionShareError::ConcurrentModification);
        }

        share.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.actor_account_id().0);

        let revision = PermissionShareRevisionRecord::from_model(share, audit);

        for attempt in 0..MAX_CARD_TREE_DELETE_ATTEMPTS {
            match self.permission_share_repo.delete(revision.clone()).await {
                Ok(record) => {
                    let permission_share: PermissionShare = record
                        .signal_new_events_available(&self.registry_change_notifier)
                        .try_into()?;

                    return Ok(permission_share);
                }
                Err(PermissionShareRepoError::CardTreeChangedDuringDelete)
                    if attempt + 1 < MAX_CARD_TREE_DELETE_ATTEMPTS =>
                {
                    continue;
                }
                Err(PermissionShareRepoError::ConcurrentModification) => {
                    return Err(PermissionShareError::ConcurrentModification);
                }
                Err(other) => return Err(other.into()),
            }
        }

        Err(PermissionShareError::ConcurrentModification)
    }

    pub async fn get(
        &self,
        permission_share_id: PermissionShareId,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let record = self.get_record_by_id(permission_share_id).await?;
        let owner_account_email = record.owner_account_email();
        let target_account_email = record.target_account_email();
        let share: PermissionShare = record.share.try_into()?;

        self.authorize_view(&share, &owner_account_email, &target_account_email, auth)
            .map_err(|err| match err {
                PermissionShareError::Unauthorized(_) => {
                    PermissionShareError::PermissionShareNotFound(permission_share_id)
                }
                other => other,
            })?;

        Ok(share)
    }

    async fn get_record_by_id(
        &self,
        permission_share_id: PermissionShareId,
    ) -> Result<PermissionShareAuthExtRevisionRecord, PermissionShareError> {
        self.permission_share_repo
            .get_by_id(permission_share_id.0)
            .await?
            .ok_or(PermissionShareError::PermissionShareNotFound(
                permission_share_id,
            ))
    }

    pub async fn get_by_owner_and_name(
        &self,
        owner_account_id: AccountId,
        name: &str,
        auth: &AuthCtx,
    ) -> Result<PermissionShare, PermissionShareError> {
        let owner_account = self.get_account(owner_account_id, auth).await?;
        authorize_permission_share_permission(
            auth,
            &owner_account.email,
            AccountPermissionShareVerb::View,
            AccountPermissionShareResourcePattern::Name(PermissionShareName(name.to_string())),
        )?;

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
        let owner_account = self.get_account(owner_account_id, auth).await?;

        let shares = self
            .permission_share_repo
            .get_for_owner(owner_account_id.0)
            .await?
            .into_iter()
            .map(|record| record.try_into().map_err(Into::into))
            .collect::<Result<Vec<_>, PermissionShareError>>()?;

        Ok(shares
            .into_iter()
            .filter(|share: &PermissionShare| {
                authorize_permission_share_permission(
                    auth,
                    &owner_account.email,
                    AccountPermissionShareVerb::View,
                    AccountPermissionShareResourcePattern::Name(share.name.clone()),
                )
                .is_ok()
            })
            .collect())
    }

    pub async fn get_for_target(
        &self,
        target_account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<PermissionShare>, PermissionShareError> {
        let target_account = self.get_account(target_account_id, auth).await?;

        let shares = self
            .permission_share_repo
            .get_for_target(target_account_id.0)
            .await?
            .into_iter()
            .map(|record| record.try_into().map_err(Into::into))
            .collect::<Result<Vec<_>, PermissionShareError>>()?;

        Ok(shares
            .into_iter()
            .filter(|share: &PermissionShare| {
                authorize_permission_share_permission(
                    auth,
                    &target_account.email,
                    AccountPermissionShareVerb::View,
                    AccountPermissionShareResourcePattern::Name(share.name.clone()),
                )
                .is_ok()
            })
            .collect())
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

    async fn get_account(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Account, PermissionShareError> {
        self.account_service
            .get(account_id, auth)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                    PermissionShareError::TargetAccountNotFound(account_id.to_string())
                }
                other => other.into(),
            })
    }

    async fn get_account_by_email(
        &self,
        account_email: &AccountEmail,
    ) -> Result<Account, PermissionShareError> {
        self.account_service
            .get_by_email(account_email.as_str(), &AuthCtx::System)
            .await
            .map_err(|err| match err {
                AccountError::AccountByEmailNotFound(_) | AccountError::Unauthorized(_) => {
                    PermissionShareError::TargetAccountNotFound(account_email.as_str().to_string())
                }
                other => other.into(),
            })
    }

    fn permission_share_card(
        &self,
        permission_share_id: PermissionShareId,
        data: &PermissionShareData,
        target_account: &str,
        auth: &AuthCtx,
    ) -> Result<CardRecord, PermissionShareError> {
        let parsed = self.parse_and_validate_data_for_target(data, target_account)?;
        validate_derivation(auth, &parsed)?;

        Ok(CardRecord::creation(
            CardId::new(),
            Vec::new(),
            parsed.lower_positive,
            parsed.lower_negative,
            parsed.upper_positive,
            parsed.upper_negative,
            None,
            true,
            Some(CardManagedBy::PermissionShare(
                CardManagedByPermissionShare {
                    permission_share_id: permission_share_id.0,
                },
            )),
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
        owner_account_email: &AccountEmail,
        target_account_email: &AccountEmail,
        auth: &AuthCtx,
    ) -> Result<(), PermissionShareError> {
        authorize_permission_share_permission(
            auth,
            owner_account_email,
            AccountPermissionShareVerb::View,
            AccountPermissionShareResourcePattern::Name(share.name.clone()),
        )
        .or_else(|_| {
            authorize_permission_share_permission(
                auth,
                target_account_email,
                AccountPermissionShareVerb::View,
                AccountPermissionShareResourcePattern::Name(share.name.clone()),
            )
        })?;

        Ok(())
    }
}

fn authorize_permission_share_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    verb: AccountPermissionShareVerb,
    resource: AccountPermissionShareResourcePattern,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::AccountPermissionShare(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: AccountOwnerPattern::Account {
                account: account_email.clone(),
            },
            resource,
        },
    ))
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
        RecipientPattern::Account { account } if account.as_str() == target_account => Ok(()),
        _ => Err(PermissionShareError::InvalidRecipient {
            target_account: target_account.to_string(),
        }),
    }
}

fn validate_derivation(
    auth: &AuthCtx,
    parsed: &ParsedPermissionShareData,
) -> Result<(), PermissionShareError> {
    match auth {
        AuthCtx::System => Ok(()),
        AuthCtx::User(user) => {
            validate_effective_surface_derivation(&user.effective_surface, parsed)
        }
        AuthCtx::AdminImpersonation(ctx) => {
            validate_effective_surface_derivation(&ctx.effective_surface, parsed)
        }
        AuthCtx::Agent(_) => Err(PermissionShareError::GrantNotDelegable(
            "agent contexts cannot delegate permission grants".to_string(),
        )),
    }
}

fn validate_effective_surface_derivation(
    effective_surface: &EffectiveSurface,
    parsed: &ParsedPermissionShareData,
) -> Result<(), PermissionShareError> {
    effective_surface
        .validates_derivation(&parsed.lower_positive, &parsed.upper_positive)
        .map_err(derivation_error)
}

fn derivation_error(error: CardAlgebraError) -> PermissionShareError {
    PermissionShareError::GrantNotDelegable(format!("{error:?}"))
}

fn invalid_grant(grant: &str, err: CardParseError) -> PermissionShareError {
    PermissionShareError::InvalidGrant {
        grant: grant.to_string(),
        message: err.to_string(),
    }
}
