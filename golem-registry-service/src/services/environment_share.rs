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

use crate::repo::environment_share::EnvironmentShareRepo;
use crate::repo::model::environment_share::{
    EnvironmentShareRepoError, EnvironmentShareRevisionRecord,
};
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_share::{
    EnvironmentShare, EnvironmentShareId, NewEnvironmentShare, UpdateEnvironmentShare,
};
use golem_common::{SafeDisplay, error_forwarding};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::error;

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

error_forwarding!(EnvironmentShareError, EnvironmentShareRepoError);

pub struct EnvironmentShareService {
    environment_share_repo: Arc<dyn EnvironmentShareRepo>,
}

impl EnvironmentShareService {
    pub fn new(environment_share_repo: Arc<dyn EnvironmentShareRepo>) -> Self {
        Self {
            environment_share_repo,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: NewEnvironmentShare,
        actor: AccountId,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let id = EnvironmentShareId::new_v4();

        let record = EnvironmentShareRevisionRecord::creation(id, data.roles, actor);

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
        update: UpdateEnvironmentShare,
        actor: AccountId,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let mut environment_share: EnvironmentShare = self
            .environment_share_repo
            .get_by_id(&environment_share_id.0)
            .await?
            .ok_or(EnvironmentShareError::EnvironmentShareNotFound(
                environment_share_id.clone(),
            ))?
            .try_into()?;

        let current_revision = environment_share.revision;

        environment_share.revision = current_revision.next()?;
        environment_share.roles = update.new_roles;

        let result = self
            .environment_share_repo
            .update(
                current_revision.into(),
                EnvironmentShareRevisionRecord::from_model(environment_share, actor),
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
        actor: AccountId,
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let mut environment_share: EnvironmentShare = self
            .environment_share_repo
            .get_by_id(&environment_share_id.0)
            .await?
            .ok_or(EnvironmentShareError::EnvironmentShareNotFound(
                environment_share_id.clone(),
            ))?
            .try_into()?;

        let current_revision = environment_share.revision;

        environment_share.revision = current_revision.next()?;

        let result = self
            .environment_share_repo
            .delete(
                current_revision.into(),
                EnvironmentShareRevisionRecord::from_model(environment_share, actor),
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
    ) -> Result<EnvironmentShare, EnvironmentShareError> {
        let result = self
            .environment_share_repo
            .get_by_id(&environment_share_id.0)
            .await?
            .ok_or(EnvironmentShareError::EnvironmentShareNotFound(
                environment_share_id.clone(),
            ))?
            .try_into()?;

        Ok(result)
    }

    pub async fn get_shares_in_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<EnvironmentShare>, EnvironmentShareError> {
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
    ) -> Result<Option<EnvironmentShare>, EnvironmentShareError> {
        let result = self
            .environment_share_repo
            .get_for_environment_and_grantee(&environment_id.0, &grantee_account_id.0)
            .await?
            .map(|r| r.try_into())
            .transpose()?;

        Ok(result)
    }
}
