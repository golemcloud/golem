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

use super::environment::{EnvironmentError, EnvironmentService};
use crate::repo::environment_share::EnvironmentShareRepo;
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::environment_share::{
    EnvironmentShareRepoError, EnvironmentShareRevisionRecord,
};
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_share::{
    EnvironmentShare, EnvironmentShareCreation, EnvironmentShareId, EnvironmentShareUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentShareError {
    #[error("There is already a share for the account")]
    ShareForAccountAlreadyExists,
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Environment share {0} not found")]
    EnvironmentShareNotFound(EnvironmentShareId),
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentShareError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ShareForAccountAlreadyExists => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::EnvironmentShareNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    EnvironmentShareError,
    EnvironmentShareRepoError,
    EnvironmentError
);

pub struct EnvironmentShareService {
    environment_share_repo: Arc<dyn EnvironmentShareRepo>,
    environment_service: Arc<EnvironmentService>,
}

impl EnvironmentShareService {
    pub fn new(
        environment_share_repo: Arc<dyn EnvironmentShareRepo>,
        environment_service: Arc<EnvironmentService>,
    ) -> Self {
        Self {
            environment_share_repo,
            environment_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: EnvironmentShareCreation,
        auth: &AuthCtx,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        self.environment_service
            .get_and_authorize(&environment_id, EnvironmentAction::CreateShare, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    EnvironmentShareError::ParentEnvironmentNotFound(environment_id.clone())
                }
                EnvironmentError::Unauthorized(inner) => EnvironmentShareError::Unauthorized(inner),
                other => other.into(),
            })?;

        let id = EnvironmentShareId::new_v4();
        let record = EnvironmentShareRevisionRecord::creation(id, data.roles, auth.account_id().clone());

        let result = self
            .environment_share_repo
            .create(environment_id.0, record, data.grantee_account_id.0)
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(EnvironmentShareRepoError::ShareViolatesUniqueness) => {
                Err(EnvironmentShareError::ShareForAccountAlreadyExists)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn update(
        &self,
        environment_share_id: &EnvironmentShareId,
        update: EnvironmentShareUpdate,
        auth: &AuthCtx,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let mut environment_share: EnvironmentShare = self.get(environment_share_id, auth).await?;

        self.environment_service
            .get_and_authorize(
                &environment_share.environment_id,
                EnvironmentAction::UpdateShare,
                false,
                auth,
            )
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    EnvironmentShareError::EnvironmentShareNotFound(environment_share_id.clone())
                }
                EnvironmentError::Unauthorized(inner) => EnvironmentShareError::Unauthorized(inner),
                other => other.into(),
            })?;

        let current_revision = environment_share.revision;

        environment_share.revision = current_revision.next()?;
        environment_share.roles = update.new_roles;

        let audit = DeletableRevisionAuditFields::new(auth.account_id().0);

        let result = self
            .environment_share_repo
            .update(
                current_revision.into(),
                EnvironmentShareRevisionRecord::from_model(environment_share, audit),
            )
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(EnvironmentShareRepoError::ConcurrentModification) => {
                Err(EnvironmentShareError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn delete(
        &self,
        environment_share_id: &EnvironmentShareId,
        auth: &AuthCtx,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let mut environment_share: EnvironmentShare = self.get(environment_share_id, auth).await?;

        self.environment_service
            .get_and_authorize(
                &environment_share.environment_id,
                EnvironmentAction::DeleteShare,
                false,
                auth,
            )
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    EnvironmentShareError::EnvironmentShareNotFound(environment_share_id.clone())
                }
                EnvironmentError::Unauthorized(inner) => EnvironmentShareError::Unauthorized(inner),
                other => other.into(),
            })?;

        let current_revision = environment_share.revision;

        environment_share.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.account_id().0);

        let result = self
            .environment_share_repo
            .delete(
                current_revision.into(),
                EnvironmentShareRevisionRecord::from_model(environment_share, audit),
            )
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(EnvironmentShareRepoError::ConcurrentModification) => {
                Err(EnvironmentShareError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn get(
        &self,
        environment_share_id: &EnvironmentShareId,
        auth: &AuthCtx,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let environment_share: EnvironmentShare = self
            .environment_share_repo
            .get_by_id(&environment_share_id.0)
            .await?
            .ok_or(EnvironmentShareError::EnvironmentShareNotFound(
                environment_share_id.clone(),
            ))?
            .try_into()?;

        self.environment_service
            .get_and_authorize(
                &environment_share.environment_id,
                EnvironmentAction::ViewShares,
                false,
                auth,
            )
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) | EnvironmentError::Unauthorized(_) => {
                    EnvironmentShareError::EnvironmentShareNotFound(environment_share_id.clone())
                }
                other => other.into(),
            })?;

        Ok(environment_share)
    }

    pub async fn get_shares_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<EnvironmentShare>, EnvironmentShareError> {
        self.environment_service
            .get_and_authorize(&environment_id, EnvironmentAction::ViewShares, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    EnvironmentShareError::ParentEnvironmentNotFound(environment_id.clone())
                }
                EnvironmentError::Unauthorized(inner) => EnvironmentShareError::Unauthorized(inner),
                other => other.into(),
            })?;

        let result = self
            .environment_share_repo
            .get_for_environment(&environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    pub async fn get_share_for_environment_and_grantee(
        &self,
        environment_id: &EnvironmentId,
        grantee_account_id: &AccountId,
        auth: &AuthCtx,
    ) -> Result<Option<EnvironmentShare>, EnvironmentShareError> {
        self.environment_service
            .get_and_authorize(environment_id, EnvironmentAction::ViewShares, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    EnvironmentShareError::ParentEnvironmentNotFound(environment_id.clone())
                }
                EnvironmentError::Unauthorized(inner) => EnvironmentShareError::Unauthorized(inner),
                other => other.into(),
            })?;

        let result = self
            .environment_share_repo
            .get_for_environment_and_grantee(&environment_id.0, &grantee_account_id.0)
            .await?
            .map(|r| r.try_into())
            .transpose()?;

        Ok(result)
    }
}
