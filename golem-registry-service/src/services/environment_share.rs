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

use crate::repo::environment::{EnvironmentRepo, EnvironmentRevisionRecord};
use anyhow::anyhow;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::environment::{Environment, EnvironmentId, NewEnvironmentData};
use golem_common::{SafeDisplay, error_forwarders};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::error;
use crate::repo::model::environment_share::{EnvironmentShareRecord, EnvironmentShareRepoError, EnvironmentShareRevisionRecord, EnvironmentShareRoleRecord};
use crate::repo::environment_share::EnvironmentShareRepo;
use golem_common::model::environment_share::{EnvironmentShare, EnvironmentShareId, EnvironmentShareRevision, NewEnvironmentShare, UpdateEnvironmentShare};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use golem_common::model::auth::EnvironmentRole;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentShareError {
    #[error("There is already a share for the account")]
    ShareForAccountAlreadyExists,
    #[error("Environment share for id not found: {0}")]
    EnvironmentShareNotFound(EnvironmentShareId),
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentShareError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ShareForAccountAlreadyExists => self.to_string(),
            Self::EnvironmentShareNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarders!(EnvironmentShareError, EnvironmentShareRepoError);

pub struct EnvironmentShareService {
    environment_share_repo: Arc<dyn EnvironmentShareRepo>,
}

impl EnvironmentShareService {
    pub fn new(environment_share_repo: Arc<dyn EnvironmentShareRepo>) -> Self {
        Self { environment_share_repo }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: NewEnvironmentShare,
        actor: AccountId,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let id = EnvironmentShareId::new_v4();
        let revision = EnvironmentShareRevision::INITIAL;
        let result = self
            .environment_share_repo
            .create(
                environment_id.0,
                EnvironmentShareRevisionRecord {
                    environment_share_id: id.0.into(),
                    revision_id: EnvironmentShareRevision::INITIAL.into(),
                    audit: DeletableRevisionAuditFields::new(actor.0),
                    roles: data.roles.into_iter().map(|r| EnvironmentShareRoleRecord::from_model(id.clone(), revision.clone(), r)).collect()
                },
                data.grantee_account_id.0
            )
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(EnvironmentShareRepoError::ShareViolatesUniqueness) => Err(EnvironmentShareError::ShareForAccountAlreadyExists),
            Err(other) => Err(other.into())
        }
    }

    pub async fn update(
        &self,
        environment_share_id: &EnvironmentShareId,
        update: UpdateEnvironmentShare,
        actor: AccountId,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let mut environment_share: EnvironmentShare = self.environment_share_repo
            .get_by_id(&environment_share_id.0).await?
            .ok_or(EnvironmentShareError::EnvironmentShareNotFound(environment_share_id.clone()))?
            .try_into()?;

        let current_revision = environment_share.revision;

        environment_share.revision = current_revision.clone().next()?;
        environment_share.roles = update.new_roles;

        let result = self
            .environment_share_repo
            .update(
                current_revision.into(),
                EnvironmentShareRevisionRecord::from_model(environment_share, actor)
            )
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(EnvironmentShareRepoError::RevisionAlreadyExists { .. } | EnvironmentShareRepoError::RevisionForUpdateNotFound { .. }) => Err(EnvironmentShareError::ConcurrentModification),
            Err(other) => Err(other.into())
        }
    }

    pub async fn delete(
        &self,
        environment_share_id: &EnvironmentShareId,
        actor: AccountId,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let mut environment_share: EnvironmentShare = self.environment_share_repo
            .get_by_id(&environment_share_id.0).await?
            .ok_or(EnvironmentShareError::EnvironmentShareNotFound(environment_share_id.clone()))?
            .try_into()?;

        let current_revision = environment_share.revision;

        environment_share.revision = current_revision.clone().next()?;

        let result = self
            .environment_share_repo
            .delete(
                current_revision.into(),
                EnvironmentShareRevisionRecord::from_model(environment_share, actor)
            )
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(EnvironmentShareRepoError::RevisionAlreadyExists { .. } | EnvironmentShareRepoError::RevisionForUpdateNotFound { .. }) => Err(EnvironmentShareError::ConcurrentModification),
            Err(other) => Err(other.into())
        }
    }


    pub async fn get(
        &self,
        environment_share_id: &EnvironmentShareId,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let result = self.environment_share_repo
            .get_by_id(&environment_share_id.0).await?
            .ok_or(EnvironmentShareError::EnvironmentShareNotFound(environment_share_id.clone()))?
            .try_into()?;

        Ok(result)
    }

    pub async fn get_shares_in_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<EnvironmentShare>, EnvironmentShareError> {
        let result = self.environment_share_repo
            .get_for_environment(&environment_id.0).await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        Ok(result)
    }
}
