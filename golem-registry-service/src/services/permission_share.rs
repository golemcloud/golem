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
use crate::repo::model::permission_share::{
    PermissionShareRepoError, PermissionShareRevisionRecord,
};
use crate::repo::permission_share::PermissionShareRepo;
use golem_common::model::account::AccountId;
use golem_common::model::permission_share::{
    PermissionShare, PermissionShareCreation, PermissionShareId, PermissionShareRevision,
    PermissionShareUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AccountAction, AuthCtx, AuthorizationError};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum PermissionShareError {
    #[error("There is already a permission share with this name")]
    PermissionShareAlreadyExists,
    #[error("Permission share {0} not found")]
    PermissionShareNotFound(PermissionShareId),
    #[error("Target account {0} not found")]
    TargetAccountNotFound(AccountId),
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
            Self::TargetAccountNotFound(_) => self.to_string(),
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

        self.ensure_account_exists(data.target_account_id).await?;

        let id = PermissionShareId::new();
        let revision = PermissionShareRevisionRecord::creation(
            id,
            data.name,
            data.data,
            auth.actor_account_id(),
        );

        match self
            .permission_share_repo
            .create(owner_account_id.0, data.target_account_id.0, revision)
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

        share.revision = share.revision.next()?;
        share.name = update.name;
        share.data = update.data;

        let audit = DeletableRevisionAuditFields::new(auth.actor_account_id().0);

        match self
            .permission_share_repo
            .update(PermissionShareRevisionRecord::from_model(share, audit))
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

    async fn ensure_account_exists(
        &self,
        account_id: AccountId,
    ) -> Result<(), PermissionShareError> {
        self.account_service
            .get(account_id, &AuthCtx::System)
            .await
            .map(|_| ())
            .map_err(|err| match err {
                AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                    PermissionShareError::TargetAccountNotFound(account_id)
                }
                other => other.into(),
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
