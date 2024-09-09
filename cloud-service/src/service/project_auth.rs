use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::{ProjectAction, ProjectActions, Role};
use cloud_common::model::{ProjectAuthorisedActions, ProjectPolicyId};
use golem_common::model::{AccountId, ProjectId};
use tracing::info;

use crate::auth::AccountAuthorisation;

use crate::repo::project::ProjectRepo;
use crate::repo::RepoError;
use crate::service::project_grant::{ProjectGrantError, ProjectGrantService};
use crate::service::project_policy::{ProjectPolicyError, ProjectPolicyService};

#[derive(Debug, thiserror::Error)]
pub enum ProjectAuthorisationError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl ProjectAuthorisationError {
    fn internal<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::Internal(anyhow::Error::msg(error.to_string()))
    }

    fn unauthorized<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::Unauthorized(error.to_string())
    }
}

impl From<RepoError> for ProjectAuthorisationError {
    fn from(error: RepoError) -> Self {
        ProjectAuthorisationError::Internal(anyhow::Error::msg(error).context("Repository error"))
    }
}

impl From<ProjectGrantError> for ProjectAuthorisationError {
    fn from(error: ProjectGrantError) -> Self {
        match error {
            ProjectGrantError::Internal(error) => ProjectAuthorisationError::Internal(error),
            ProjectGrantError::Unauthorized(error) => {
                ProjectAuthorisationError::Unauthorized(error)
            }
            ProjectGrantError::ProjectNotFound(_) => ProjectAuthorisationError::internal(error),
            ProjectGrantError::ProjectPolicyNotFound(_) => {
                ProjectAuthorisationError::internal(error)
            }
            ProjectGrantError::AccountNotFound(_) => ProjectAuthorisationError::internal(error),
        }
    }
}

impl From<ProjectPolicyError> for ProjectAuthorisationError {
    fn from(error: ProjectPolicyError) -> Self {
        match error {
            ProjectPolicyError::Internal(error) => ProjectAuthorisationError::Internal(error),
        }
    }
}

#[async_trait]
pub trait ProjectAuthorisationService {
    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<ProjectAuthorisedActions, ProjectAuthorisationError>;

    async fn get_all(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<HashMap<ProjectId, ProjectActions>, ProjectAuthorisationError>;
}

pub struct ProjectAuthorisationServiceDefault {
    project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    project_grant_service: Arc<dyn ProjectGrantService + Sync + Send>,
    project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
}

impl ProjectAuthorisationServiceDefault {
    pub fn new(
        project_repo: Arc<dyn ProjectRepo + Sync + Send>,
        project_grant_service: Arc<dyn ProjectGrantService + Sync + Send>,
        project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
    ) -> Self {
        ProjectAuthorisationServiceDefault {
            project_repo,
            project_grant_service,
            project_policy_service,
        }
    }
}

#[async_trait]
impl ProjectAuthorisationService for ProjectAuthorisationServiceDefault {
    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<ProjectAuthorisedActions, ProjectAuthorisationError> {
        info!("Get project authorisations for project: {}", project_id);
        let project = self.project_repo.get(&project_id.0).await?;
        if let Some(project) = project {
            let owner_account_id = AccountId::from(project.owner_account_id.as_str());
            if auth.has_account_or_role(&owner_account_id, &Role::Admin) {
                Ok(ProjectAuthorisedActions {
                    project_id: project_id.clone(),
                    owner_account_id,
                    actions: ProjectActions::all(),
                })
            } else {
                let grants = self
                    .project_grant_service
                    .get_by_project(project_id, auth)
                    .await?;

                let policy_ids = grants
                    .iter()
                    .map(|p| p.data.project_policy_id.clone())
                    .collect::<Vec<ProjectPolicyId>>();

                let actions = if !policy_ids.is_empty() {
                    let policies = self.project_policy_service.get_all(policy_ids).await?;

                    let actions = policies
                        .iter()
                        .flat_map(|p| p.clone().project_actions.actions)
                        .collect::<HashSet<ProjectAction>>();

                    ProjectActions { actions }
                } else {
                    ProjectActions::empty()
                };

                Ok(ProjectAuthorisedActions {
                    project_id: project_id.clone(),
                    owner_account_id,
                    actions,
                })
            }
        } else {
            Err(ProjectAuthorisationError::unauthorized("Unauthorized"))
        }
    }

    async fn get_all(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<HashMap<ProjectId, ProjectActions>, ProjectAuthorisationError> {
        let account_id = &auth.token.account_id;
        let own_projects = self.project_repo.get_own(&account_id.value).await?;

        let grants = self
            .project_grant_service
            .get_by_account(account_id, auth)
            .await?;

        let policy_ids = grants
            .iter()
            .map(|p| p.data.project_policy_id.clone())
            .collect::<Vec<ProjectPolicyId>>();

        let mut project_actions: HashMap<ProjectId, ProjectActions> = HashMap::new();

        if !policy_ids.is_empty() {
            let policies = self.project_policy_service.get_all(policy_ids).await?;

            for grant in grants {
                if let Some(policy) = policies
                    .iter()
                    .find(|p| p.id == grant.data.project_policy_id)
                {
                    project_actions.insert(
                        grant.data.grantor_project_id,
                        policy.clone().project_actions,
                    );
                } else {
                    project_actions.insert(grant.data.grantor_project_id, ProjectActions::empty());
                }
            }
        }

        for project in own_projects {
            let project_id = ProjectId(project.project_id);
            project_actions.insert(project_id, ProjectActions::all());
        }

        Ok(project_actions)
    }
}

#[derive(Default)]
pub struct ProjectAuthorisationServiceNoOp {}

#[async_trait]
impl ProjectAuthorisationService for ProjectAuthorisationServiceNoOp {
    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<ProjectAuthorisedActions, ProjectAuthorisationError> {
        Ok(ProjectAuthorisedActions {
            project_id: project_id.clone(),
            owner_account_id: auth.token.account_id.clone(),
            actions: ProjectActions::empty(),
        })
    }

    async fn get_all(
        &self,
        _auth: &AccountAuthorisation,
    ) -> Result<HashMap<ProjectId, ProjectActions>, ProjectAuthorisationError> {
        Ok(HashMap::new())
    }
}
