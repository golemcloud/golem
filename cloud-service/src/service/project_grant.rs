use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::Role;
use cloud_common::model::{ProjectAction, ProjectGrantId, ProjectPolicyId};
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use tracing::info;
use uuid::Uuid;

use crate::auth::AccountAuthorisation;
use crate::model::{ProjectGrant, ProjectPolicy};
use crate::repo::account::AccountRepo;
use crate::repo::project::ProjectRepo;
use crate::repo::project_grant::{ProjectGrantRecord, ProjectGrantRepo};
use crate::repo::project_policy::ProjectPolicyRepo;
use crate::repo::RepoError;

#[derive(Debug, thiserror::Error)]
pub enum ProjectGrantError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Project Not Found: {0}")]
    ProjectNotFound(ProjectId),
    #[error("Project Policy Not Found: {0}")]
    ProjectPolicyNotFound(ProjectPolicyId),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl ProjectGrantError {
    pub fn internal<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::Internal(anyhow::Error::msg(error.to_string()))
    }

    pub fn unauthorized<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::Unauthorized(error.to_string())
    }
}

impl From<RepoError> for ProjectGrantError {
    fn from(error: RepoError) -> Self {
        ProjectGrantError::internal(error)
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

    async fn get_by_account(
        &self,
        account_id: &AccountId,
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
    project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
    project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
}

impl ProjectGrantServiceDefault {
    pub fn new(
        project_repo: Arc<dyn ProjectRepo + Sync + Send>,
        project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
        project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
    ) -> Self {
        ProjectGrantServiceDefault {
            project_repo,
            project_grant_repo,
            project_policy_repo,
            account_repo,
        }
    }

    async fn get_permitted_grants(
        &self,
        grants: Vec<ProjectGrant>,
        project_action: &ProjectAction,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError> {
        if auth.has_role(&Role::Admin) {
            Ok(grants)
        } else {
            let policy_ids: Vec<Uuid> = grants
                .iter()
                .filter(|g| g.data.grantee_account_id == auth.token.account_id)
                .map(|g| g.data.project_policy_id.0)
                .collect();

            if !policy_ids.is_empty() {
                let records = self.project_policy_repo.get_all(policy_ids).await?;

                let policies = records
                    .iter()
                    .map(|p| {
                        let policy: ProjectPolicy = p.clone().into();
                        (policy.id, policy.project_actions.actions)
                    })
                    .collect::<HashMap<ProjectPolicyId, HashSet<ProjectAction>>>();

                let mut grants_with_policy: Vec<ProjectGrant> = Vec::new();
                for grant in grants {
                    if let Some(actions) = policies.get(&grant.data.project_policy_id) {
                        if actions.contains(project_action) {
                            grants_with_policy.push(grant);
                        }
                    }
                }
                Ok(grants_with_policy)
            } else {
                Ok(vec![])
            }
        }
    }

    async fn is_authorised_by_policy(
        &self,
        project_id: &ProjectId,
        project_action: &ProjectAction,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
        let result = self
            .project_grant_repo
            .get_by_project(&project_id.0)
            .await?;
        let project_grants = self
            .get_permitted_grants(
                result.iter().map(|p| p.clone().into()).collect(),
                project_action,
                auth,
            )
            .await?;
        if project_grants.is_empty() {
            Err(ProjectGrantError::unauthorized("Unauthorized"))
        } else {
            Ok(())
        }
    }

    fn is_authorised_by_account(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
        if auth.has_account_or_role(account_id, &Role::Admin) {
            Ok(())
        } else {
            Err(ProjectGrantError::unauthorized("Unauthorized"))
        }
    }
}

// FIXME check auth
#[async_trait]
impl ProjectGrantService for ProjectGrantServiceDefault {
    async fn create(
        &self,
        project_grant: &ProjectGrant,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
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

        let project_id = project_grant.data.grantor_project_id.clone();

        let project = self.project_repo.get(&project_id.0).await?;

        if let Some(project) = project {
            if !auth.has_account_or_role(
                &AccountId::from(project.owner_account_id.as_str()),
                &Role::Admin,
            ) {
                self.is_authorised_by_policy(
                    &project_id,
                    &ProjectAction::CreateProjectGrants,
                    auth,
                )
                .await?;
            }
            let project_grant: ProjectGrantRecord = project_grant.clone().into();
            self.project_grant_repo
                .create(&project_grant)
                .await
                .map_err(ProjectGrantError::internal)
        } else {
            Err(ProjectGrantError::ProjectNotFound(project_id))
        }
    }

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError> {
        info!("Getting project grants for project {}", project_id);

        let project = self.project_repo.get(&project_id.0).await?;

        if let Some(project) = project {
            let result = self
                .project_grant_repo
                .get_by_project(&project_id.0)
                .await?;

            let project_grants = result.iter().map(|p| p.clone().into()).collect();

            if auth.has_account_or_role(
                &AccountId::from(project.owner_account_id.as_str()),
                &Role::Admin,
            ) {
                Ok(project_grants)
            } else {
                let project_grants = self
                    .get_permitted_grants(project_grants, &ProjectAction::ViewProjectGrants, auth)
                    .await?;

                Ok(project_grants)
            }
        } else {
            Err(ProjectGrantError::ProjectNotFound(project_id.clone()))
        }
    }

    async fn get_by_account(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError> {
        info!("Getting project grants for account {}", account_id);
        self.is_authorised_by_account(account_id, auth)?;
        let result = self
            .project_grant_repo
            .get_by_account(account_id.value.as_str())
            .await?;
        Ok(result.iter().map(|p| p.clone().into()).collect())
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<ProjectGrant>, ProjectGrantError> {
        info!("Getting project {} grant {}", project_id, project_grant_id);
        let project = self.project_repo.get(&project_id.0).await?;

        if let Some(project) = project {
            let project_grant = self.project_grant_repo.get(&project_grant_id.0).await?;
            if let Some(project_grant) = project_grant {
                let project_grant: ProjectGrant = project_grant.into();
                if project_grant.data.grantor_project_id == *project_id {
                    let maybe_project_grant = if auth.has_account_or_role(
                        &AccountId::from(project.owner_account_id.as_str()),
                        &Role::Admin,
                    ) {
                        Some(project_grant)
                    } else {
                        let project_grants = self
                            .get_permitted_grants(
                                vec![project_grant.clone()],
                                &ProjectAction::ViewProjectGrants,
                                auth,
                            )
                            .await?;

                        project_grants.first().cloned()
                    };

                    return Ok(maybe_project_grant);
                }
            }
            Ok(None)
        } else {
            Err(ProjectGrantError::ProjectNotFound(project_id.clone()))
        }
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        project_grant_id: &ProjectGrantId,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
        info!("Deleting project {} grant {}", project_id, project_grant_id);
        let project = self.project_repo.get(&project_id.0).await?;
        if let Some(project) = project {
            if !auth.has_account_or_role(
                &AccountId::from(project.owner_account_id.as_str()),
                &Role::Admin,
            ) {
                self.is_authorised_by_policy(project_id, &ProjectAction::DeleteProjectGrants, auth)
                    .await?;
            }
        }
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

#[derive(Default)]
pub struct ProjectGrantServiceNoOp {}

#[async_trait]
impl ProjectGrantService for ProjectGrantServiceNoOp {
    async fn create(
        &self,
        _project_grant: &ProjectGrant,
        _auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
        Ok(())
    }

    async fn get_by_project(
        &self,
        _project_id: &ProjectId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError> {
        Ok(vec![])
    }

    async fn get_by_account(
        &self,
        _account_id: &AccountId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<ProjectGrant>, ProjectGrantError> {
        Ok(vec![])
    }

    async fn get(
        &self,
        _project_id: &ProjectId,
        _project_grant_id: &ProjectGrantId,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<ProjectGrant>, ProjectGrantError> {
        Ok(None)
    }

    async fn delete(
        &self,
        _project_id: &ProjectId,
        _project_grant_id: &ProjectGrantId,
        _auth: &AccountAuthorisation,
    ) -> Result<(), ProjectGrantError> {
        Ok(())
    }
}
