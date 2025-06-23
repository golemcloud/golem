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

use super::auth::AuthServiceError;
use crate::model::ProjectGrant;
use crate::repo::account::AccountRepo;
use crate::repo::project_grant::{ProjectGrantRecord, ProjectGrantRepo};
use crate::repo::project_policy::ProjectPolicyRepo;
use async_trait::async_trait;
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use golem_common::model::{ProjectGrantId, ProjectPolicyId};
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum ProjectGrantError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Project Not Found: {0}")]
    ProjectNotFound(ProjectId),
    #[error("Project Policy Not Found: {0}")]
    ProjectPolicyNotFound(ProjectPolicyId),
    #[error("Internal repository error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
}

impl SafeDisplay for ProjectGrantError {
    fn to_safe_string(&self) -> String {
        match self {
            ProjectGrantError::AccountNotFound(_) => self.to_string(),
            ProjectGrantError::ProjectNotFound(_) => self.to_string(),
            ProjectGrantError::ProjectPolicyNotFound(_) => self.to_string(),
            ProjectGrantError::InternalRepoError(inner) => inner.to_safe_string(),
            ProjectGrantError::AuthError(inner) => inner.to_safe_string(),
        }
    }
}

#[async_trait]
pub trait ProjectGrantService: Send + Sync {
    async fn create(&self, project_grant: &ProjectGrant) -> Result<(), ProjectGrantError>;

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError>;

    async fn get(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
    ) -> Result<Option<ProjectGrant>, ProjectGrantError>;

    async fn delete(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
    ) -> Result<(), ProjectGrantError>;
}

pub struct ProjectGrantServiceDefault {
    project_grant_repo: Arc<dyn ProjectGrantRepo>,
    project_policy_repo: Arc<dyn ProjectPolicyRepo>,
    account_repo: Arc<dyn AccountRepo>,
}

impl ProjectGrantServiceDefault {
    pub fn new(
        project_grant_repo: Arc<dyn ProjectGrantRepo>,
        project_policy_repo: Arc<dyn ProjectPolicyRepo>,
        account_repo: Arc<dyn AccountRepo>,
    ) -> Self {
        ProjectGrantServiceDefault {
            project_grant_repo,
            project_policy_repo,
            account_repo,
        }
    }
}

#[async_trait]
impl ProjectGrantService for ProjectGrantServiceDefault {
    async fn create(&self, project_grant: &ProjectGrant) -> Result<(), ProjectGrantError> {
        info!(
            "Create project {} grant {}",
            &project_grant.data.grantor_project_id, project_grant.id
        );

        let account_id = project_grant.data.grantee_account_id.clone();

        let account = self.account_repo.get(account_id.value.as_str()).await?;

        if account.is_none() {
            return Err(ProjectGrantError::AccountNotFound(account_id));
        }

        let project_policy_id = project_grant.data.project_policy_id.clone();

        let project_policy = self.project_policy_repo.get(&project_policy_id.0).await?;

        if project_policy.is_none() {
            return Err(ProjectGrantError::ProjectPolicyNotFound(project_policy_id));
        }

        let project_grant: ProjectGrantRecord = project_grant.clone().into();
        self.project_grant_repo.create(&project_grant).await?;
        Ok(())
    }

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError> {
        info!("Getting project grants for project {}", project_id);

        let result = self
            .project_grant_repo
            .get_by_project(&project_id.0)
            .await?;

        let project_grants = result.iter().map(|p| p.clone().into()).collect();

        Ok(project_grants)
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
    ) -> Result<Option<ProjectGrant>, ProjectGrantError> {
        info!("Getting project {} grant {}", project_id, project_grant_id);

        let project_grant = self.project_grant_repo.get(&project_grant_id.0).await?;

        match project_grant {
            Some(project_grant) if project_grant.grantor_project_id == project_id.0 => {
                Ok(Some(project_grant.into()))
            }
            _ => Ok(None),
        }
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
    ) -> Result<(), ProjectGrantError> {
        info!("Deleting project {} grant {}", project_id, project_grant_id);

        let project_grant = self.project_grant_repo.get(&project_grant_id.0).await?;
        if let Some(project_grant) = project_grant {
            let project_grant: ProjectGrant = project_grant.into();
            if project_grant.data.grantor_project_id == *project_id {
                self.project_grant_repo.delete(&project_grant_id.0).await?;
            }
        }
        Ok(())
    }
}
