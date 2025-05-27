use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::model::ProjectGrant;
use crate::repo::account::AccountRepo;
use crate::repo::project_grant::{ProjectGrantRecord, ProjectGrantRepo};
use crate::repo::project_policy::ProjectPolicyRepo;
use async_trait::async_trait;
use cloud_common::model::{ProjectAction, ProjectGrantId, ProjectPolicyId};
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use tracing::info;

use super::auth::{AuthService, AuthServiceError};

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
pub trait ProjectGrantService {
    async fn create(
        &self,
        project_grant: &ProjectGrant,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError>;

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError>;

    async fn get(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<ProjectGrant>, ProjectGrantError>;

    async fn delete(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError>;
}

pub struct ProjectGrantServiceDefault {
    project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
    project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
    auth_service: Arc<dyn AuthService>,
}

impl ProjectGrantServiceDefault {
    pub fn new(
        project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
        project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
        auth_service: Arc<dyn AuthService>,
    ) -> Self {
        ProjectGrantServiceDefault {
            project_grant_repo,
            project_policy_repo,
            account_repo,
            auth_service,
        }
    }
}

#[async_trait]
impl ProjectGrantService for ProjectGrantServiceDefault {
    async fn create(
        &self,
        project_grant: &ProjectGrant,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
        self.auth_service
            .authorize_project_action(
                auth,
                &project_grant.data.grantor_project_id,
                &ProjectAction::CreateProjectGrants,
            )
            .await?;

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
        auth: &AccountAuthorisation,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError> {
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::ViewProjectGrants)
            .await?;

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
        auth: &AccountAuthorisation,
    ) -> Result<Option<ProjectGrant>, ProjectGrantError> {
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::ViewProjectGrants)
            .await?;

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
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::DeleteProjectGrants)
            .await?;

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
