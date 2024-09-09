use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::Role;
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use tracing::info;

use crate::auth::AccountAuthorisation;
use crate::model::{Project, ProjectData, ProjectType};
use crate::repo::project::{ProjectRecord, ProjectRepo};
use crate::repo::RepoError;
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};
use crate::service::project_auth::{ProjectAuthorisationError, ProjectAuthorisationService};

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("Limit Exceeded: {0}")]
    LimitExceeded(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl ProjectError {
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

    fn limit_exceeded<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::LimitExceeded(error.to_string())
    }
}

impl From<RepoError> for ProjectError {
    fn from(error: RepoError) -> Self {
        ProjectError::Internal(anyhow::Error::msg(error).context("Repository error"))
    }
}

impl From<ProjectAuthorisationError> for ProjectError {
    fn from(error: ProjectAuthorisationError) -> Self {
        match error {
            ProjectAuthorisationError::Internal(error) => ProjectError::Internal(error),
            ProjectAuthorisationError::Unauthorized(error) => ProjectError::unauthorized(error),
        }
    }
}

impl From<PlanLimitError> for ProjectError {
    fn from(error: PlanLimitError) -> Self {
        match error {
            PlanLimitError::Unauthorized(error) => ProjectError::Unauthorized(error),
            PlanLimitError::Internal(error) => ProjectError::Internal(error),
            PlanLimitError::AccountNotFound(_) => ProjectError::internal(error),
            PlanLimitError::ProjectNotFound(_) => ProjectError::internal(error),
            PlanLimitError::LimitExceeded(error) => ProjectError::limit_exceeded(error),
        }
    }
}

#[async_trait]
pub trait ProjectService {
    async fn create(
        &self,
        project: &Project,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectError>;

    async fn delete(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectError>;

    async fn get_own_default(&self, auth: &AccountAuthorisation) -> Result<Project, ProjectError>;

    async fn get_own(&self, auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError>;

    async fn get_own_by_name(
        &self,
        name: &str,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Project>, ProjectError>;

    async fn get_own_count(&self, auth: &AccountAuthorisation) -> Result<u64, ProjectError>;

    async fn get_all(&self, auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError>;

    async fn get(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<Project>, ProjectError>;
}

pub struct ProjectServiceDefault {
    project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
    plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
}

impl ProjectServiceDefault {
    pub fn new(
        project_repo: Arc<dyn ProjectRepo + Sync + Send>,
        project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
        plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
    ) -> Self {
        ProjectServiceDefault {
            project_repo,
            project_auth_service,
            plan_limit_service,
        }
    }
}

#[async_trait]
impl ProjectService for ProjectServiceDefault {
    async fn create(
        &self,
        project: &Project,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectError> {
        info!("Create project {}", project.project_id);
        is_authorised_by_account(
            &project.project_data.owner_account_id,
            &Role::CreateProject,
            auth,
        )?;

        let check_limit_result = self
            .plan_limit_service
            .check_project_limit(&project.project_data.owner_account_id)
            .await?;

        if check_limit_result.in_limit() {
            let project: ProjectRecord = project.clone().into();
            self.project_repo.create(&project).await?;
            Ok(())
        } else {
            Err(ProjectError::limit_exceeded(format!(
                "Project limit exceeded (limit: {})",
                check_limit_result.limit
            )))
        }
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectError> {
        info!("Delete project {}", project_id);
        let project = self.project_repo.get(&project_id.0).await?;

        if let Some(project) = project {
            // FIXME delete components, workers ...

            // let component_count = self
            //     .component_repo
            //     .get_count_by_projects(vec![project_id.0])
            //     .await?;

            if auth.has_account_or_role(
                &AccountId::from(project.owner_account_id.as_str()),
                &Role::Admin,
            ) && !project.is_default
            // && component_count == 0
            {
                self.project_repo.delete(&project_id.0).await?;
            } else {
                return Err(ProjectError::unauthorized("Unauthorized".to_string()));
            }
        }

        Ok(())
    }

    async fn get_own_default(&self, auth: &AccountAuthorisation) -> Result<Project, ProjectError> {
        let account_id = &auth.token.account_id;
        info!("Getting default project for account {}", account_id);
        is_authorised(&Role::ViewProject, auth)?;
        let result = self
            .project_repo
            .get_own_default(account_id.value.as_str())
            .await?;

        if let Some(result) = result {
            Ok(result.into())
        } else {
            info!("Creating default project for account {}", account_id);
            let project = create_default_project(&auth.token.account_id);
            let create_res = self.project_repo.create(&project.clone().into()).await;
            if let Err(err) = create_res {
                info!("Project creation failed: {err:?}");
            }
            let result = self
                .project_repo
                .get_own_default(account_id.value.as_str())
                .await?;
            Ok(result
                .ok_or(ProjectError::internal("Failed to create default project"))?
                .into())
        }
    }

    async fn get_own(&self, auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError> {
        let account_id = &auth.token.account_id;
        info!("Getting projects for account {}", account_id);
        is_authorised(&Role::ViewProject, auth)?;
        let result = self.project_repo.get_own(account_id.value.as_str()).await?;
        Ok(result.iter().map(|p| p.clone().into()).collect())
    }

    async fn get_own_by_name(
        &self,
        name: &str,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Project>, ProjectError> {
        let account_id = &auth.token.account_id;
        info!(
            "Getting projects for account {} with name {}",
            account_id, name
        );
        is_authorised(&Role::ViewProject, auth)?;
        let result = self.project_repo.get_own(account_id.value.as_str()).await?;
        Ok(result
            .iter()
            .filter(|p| p.name == name)
            .map(|p| p.clone().into())
            .collect())
    }

    async fn get_own_count(&self, auth: &AccountAuthorisation) -> Result<u64, ProjectError> {
        let account_id = &auth.token.account_id;
        info!("Getting projects count for account {}", account_id);
        is_authorised(&Role::ViewProject, auth)?;
        let result = self
            .project_repo
            .get_own_count(account_id.value.as_str())
            .await?;
        Ok(result)
    }

    async fn get_all(&self, auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError> {
        info!("Getting projects");
        is_authorised(&Role::ViewProject, auth)?;
        let result = self.project_repo.get_all().await?;
        Ok(result.iter().map(|p| p.clone().into()).collect())
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<Project>, ProjectError> {
        info!("Getting project {}", project_id);
        let actions = self
            .project_auth_service
            .get_by_project(project_id, auth)
            .await?;
        if actions.actions.actions.is_empty() {
            Err(ProjectError::unauthorized("Unauthorized"))
        } else {
            let result = self.project_repo.get(&project_id.0).await?;
            Ok(result.map(|p| p.into()))
        }
    }
}

pub fn is_authorised(role: &Role, auth: &AccountAuthorisation) -> Result<(), ProjectError> {
    if auth.has_role(role) || auth.has_role(&Role::Admin) {
        Ok(())
    } else {
        Err(ProjectError::unauthorized("Unauthorized"))
    }
}

pub fn is_authorised_by_account(
    account_id: &AccountId,
    role: &Role,
    auth: &AccountAuthorisation,
) -> Result<(), ProjectError> {
    if auth.has_account_and_role(account_id, role) || auth.has_role(&Role::Admin) {
        Ok(())
    } else {
        Err(ProjectError::unauthorized("Unauthorized"))
    }
}

#[derive(Default)]
pub struct ProjectServiceNoOp {}

#[async_trait]
impl ProjectService for ProjectServiceNoOp {
    async fn create(
        &self,
        _project: &Project,
        _auth: &AccountAuthorisation,
    ) -> Result<(), ProjectError> {
        Ok(())
    }

    async fn delete(
        &self,
        _project_id: &ProjectId,
        _auth: &AccountAuthorisation,
    ) -> Result<(), ProjectError> {
        Ok(())
    }

    async fn get_own_default(&self, auth: &AccountAuthorisation) -> Result<Project, ProjectError> {
        Ok(create_default_project(&auth.token.account_id))
    }

    async fn get_own(&self, _auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError> {
        Ok(vec![])
    }

    async fn get_own_by_name(
        &self,
        _name: &str,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<Project>, ProjectError> {
        Ok(vec![])
    }

    async fn get_own_count(&self, _auth: &AccountAuthorisation) -> Result<u64, ProjectError> {
        Ok(0)
    }

    async fn get_all(&self, _auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError> {
        Ok(vec![])
    }

    async fn get(
        &self,
        _project_id: &ProjectId,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<Project>, ProjectError> {
        Ok(None)
    }
}

pub fn create_default_project(account_id: &AccountId) -> Project {
    Project {
        project_id: ProjectId::new_v4(),
        project_data: ProjectData {
            name: "default-project".to_string(),
            owner_account_id: account_id.clone(),
            description: format!("Default project of the account {}", account_id.value),
            default_environment_id: "default".to_string(),
            project_type: ProjectType::Default,
        },
    }
}
