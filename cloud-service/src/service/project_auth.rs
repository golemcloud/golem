use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::ProjectPolicyId;
use golem_common::model::ProjectId;
use golem_common::model::TemplateId;
use tracing::info;

use crate::auth::AccountAuthorisation;
use crate::model::{ProjectAction, ProjectActions};
use crate::repo::project::ProjectRepo;
use crate::repo::template::TemplateRepo;
use crate::repo::RepoError;
use crate::service::project_grant::{ProjectGrantError, ProjectGrantService};
use crate::service::project_policy::{ProjectPolicyError, ProjectPolicyService};

#[derive(Debug, Clone)]
pub enum ProjectAuthorisationError {
    Internal(String),
    Unauthorized(String),
}

impl ProjectAuthorisationError {
    pub fn internal<T: Display>(error: T) -> Self {
        ProjectAuthorisationError::Internal(error.to_string())
    }
}

impl From<RepoError> for ProjectAuthorisationError {
    fn from(error: RepoError) -> Self {
        ProjectAuthorisationError::internal(error)
    }
}

impl From<ProjectGrantError> for ProjectAuthorisationError {
    fn from(error: ProjectGrantError) -> Self {
        match error {
            ProjectGrantError::Internal(error) => ProjectAuthorisationError::Internal(error),
            ProjectGrantError::Unauthorized(error) => {
                ProjectAuthorisationError::Unauthorized(error)
            }
            ProjectGrantError::ProjectIdNotFound(_) => {
                ProjectAuthorisationError::Internal("Project not found".to_string())
            }
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
    ) -> Result<ProjectActions, ProjectAuthorisationError>;

    async fn get_by_template(
        &self,
        template_id: &TemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<ProjectActions, ProjectAuthorisationError>;

    async fn get_all(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<HashMap<ProjectId, ProjectActions>, ProjectAuthorisationError>;
}

pub struct ProjectAuthorisationServiceDefault {
    project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    project_grant_service: Arc<dyn ProjectGrantService + Sync + Send>,
    project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
    template_repo: Arc<dyn TemplateRepo + Sync + Send>,
}

impl ProjectAuthorisationServiceDefault {
    pub fn new(
        project_repo: Arc<dyn ProjectRepo + Sync + Send>,
        project_grant_service: Arc<dyn ProjectGrantService + Sync + Send>,
        project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
        template_repo: Arc<dyn TemplateRepo + Sync + Send>,
    ) -> Self {
        ProjectAuthorisationServiceDefault {
            project_repo,
            project_grant_service,
            project_policy_service,
            template_repo,
        }
    }
}

#[async_trait]
impl ProjectAuthorisationService for ProjectAuthorisationServiceDefault {
    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<ProjectActions, ProjectAuthorisationError> {
        info!("Get project authorisations for project: {}", project_id);

        let project = self.project_repo.get(&project_id.0).await?;

        if let Some(project) = project {
            if project.owner_account_id == auth.token.account_id.value {
                Ok(ProjectActions::all())
            } else {
                let grants = self
                    .project_grant_service
                    .get_by_project(project_id, auth)
                    .await?;

                let policy_ids = grants
                    .iter()
                    .map(|p| p.data.project_policy_id.clone())
                    .collect::<Vec<ProjectPolicyId>>();

                if !policy_ids.is_empty() {
                    let policies = self.project_policy_service.get_all(policy_ids).await?;

                    let actions = policies
                        .iter()
                        .flat_map(|p| p.clone().project_actions.actions)
                        .collect::<HashSet<ProjectAction>>();

                    Ok(ProjectActions { actions })
                } else {
                    Ok(ProjectActions::empty())
                }
            }
        } else {
            Ok(ProjectActions::empty())
        }
    }

    async fn get_by_template(
        &self,
        template_id: &TemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<ProjectActions, ProjectAuthorisationError> {
        info!("Get project authorisations for template: {}", template_id);
        let template = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?;

        if let Some(template) = template {
            let project_id = ProjectId(template.project_id);
            self.get_by_project(&project_id, auth).await
        } else {
            Ok(ProjectActions::all())
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
        _project_id: &ProjectId,
        _auth: &AccountAuthorisation,
    ) -> Result<ProjectActions, ProjectAuthorisationError> {
        Ok(ProjectActions::empty())
    }

    async fn get_by_template(
        &self,
        _template_id: &TemplateId,
        _auth: &AccountAuthorisation,
    ) -> Result<ProjectActions, ProjectAuthorisationError> {
        Ok(ProjectActions::empty())
    }

    async fn get_all(
        &self,
        _auth: &AccountAuthorisation,
    ) -> Result<HashMap<ProjectId, ProjectActions>, ProjectAuthorisationError> {
        Ok(HashMap::new())
    }
}
