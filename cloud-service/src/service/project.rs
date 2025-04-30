use crate::auth::AccountAuthorisation;
use crate::model::{Project, ProjectData, ProjectPluginInstallationTarget, ProjectType};
use crate::repo::project::{ProjectRecord, ProjectRepo};
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};
use crate::service::project_auth::{ProjectAuthorisationError, ProjectAuthorisationService};
use async_trait::async_trait;
use cloud_common::clients::plugin::{PluginError, PluginServiceClient};
use cloud_common::model::{Role, TokenSecret};
use cloud_common::repo::CloudPluginOwnerRow;
use golem_common::model::plugin::{
    PluginInstallation, PluginInstallationAction, PluginInstallationCreation,
    PluginInstallationUpdate, PluginInstallationUpdateWithId, PluginUninstallation,
};
use golem_common::model::{AccountId, PluginInstallationId};
use golem_common::model::{PluginId, ProjectId};
use golem_common::SafeDisplay;
use golem_service_base::repo::plugin_installation::PluginInstallationRecord;
use golem_service_base::repo::RepoError;
use std::fmt::Display;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("Limit Exceeded: {0}")]
    LimitExceeded(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error(transparent)]
    InternalPlanLimitError(PlanLimitError),
    #[error(transparent)]
    InternalProjectAuthorisationError(ProjectAuthorisationError),
    #[error("Failed to create default project for account {0}")]
    FailedToCreateDefaultProject(AccountId),
    #[error("Internal repository error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error("Internal error: failed to convert {what}: {error}")]
    InternalConversionError { what: String, error: String },
    #[error("Plugin not found: {plugin_name}@{plugin_version}")]
    PluginNotFound {
        plugin_name: String,
        plugin_version: String,
    },
    #[error("Internal plugin error: {0}")]
    InternalPluginError(#[from] PluginError),
}

impl ProjectError {
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

    pub fn conversion_error(what: impl AsRef<str>, error: String) -> Self {
        Self::InternalConversionError {
            what: what.as_ref().to_string(),
            error,
        }
    }
}

impl SafeDisplay for ProjectError {
    fn to_safe_string(&self) -> String {
        match self {
            ProjectError::LimitExceeded(_) => self.to_string(),
            ProjectError::Unauthorized(_) => self.to_string(),
            ProjectError::InternalPlanLimitError(inner) => inner.to_safe_string(),
            ProjectError::InternalProjectAuthorisationError(inner) => inner.to_safe_string(),
            ProjectError::FailedToCreateDefaultProject(_) => self.to_string(),
            ProjectError::InternalRepoError(inner) => inner.to_safe_string(),
            ProjectError::InternalConversionError { .. } => self.to_string(),
            ProjectError::PluginNotFound { .. } => self.to_string(),
            ProjectError::InternalPluginError(inner) => inner.to_safe_string(),
        }
    }
}

impl From<ProjectAuthorisationError> for ProjectError {
    fn from(error: ProjectAuthorisationError) -> Self {
        match error {
            ProjectAuthorisationError::Unauthorized(error) => ProjectError::unauthorized(error),
            _ => ProjectError::InternalProjectAuthorisationError(error),
        }
    }
}

impl From<PlanLimitError> for ProjectError {
    fn from(error: PlanLimitError) -> Self {
        match error {
            PlanLimitError::Unauthorized(error) => ProjectError::Unauthorized(error),
            PlanLimitError::LimitExceeded(error) => ProjectError::limit_exceeded(error),
            _ => ProjectError::InternalPlanLimitError(error),
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

    /// Gets the list of installed plugins for a given project
    async fn get_plugin_installations_for_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<PluginInstallation>, ProjectError>;

    async fn create_plugin_installation_for_project(
        &self,
        project_id: &ProjectId,
        installation: PluginInstallationCreation,
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<PluginInstallation, ProjectError>;

    async fn update_plugin_installation_for_project(
        &self,
        project_id: &ProjectId,
        installation_id: &PluginInstallationId,
        update: PluginInstallationUpdate,
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<(), ProjectError>;

    async fn delete_plugin_installation_for_project(
        &self,
        installation_id: &PluginInstallationId,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<(), ProjectError>;

    async fn batch_update_plugin_installations_for_project(
        &self,
        project_id: &ProjectId,
        actions: &[PluginInstallationAction],
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<Vec<Option<PluginInstallation>>, ProjectError>;
}

pub struct ProjectServiceDefault {
    project_repo: Arc<dyn ProjectRepo + Send + Sync>,
    project_auth_service: Arc<dyn ProjectAuthorisationService + Send + Sync>,
    plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,
    plugin_service: Arc<dyn PluginServiceClient + Send + Sync>,
}

impl ProjectServiceDefault {
    pub fn new(
        project_repo: Arc<dyn ProjectRepo + Send + Sync>,
        project_auth_service: Arc<dyn ProjectAuthorisationService + Send + Sync>,
        plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,
        plugin_service: Arc<dyn PluginServiceClient + Send + Sync>,
    ) -> Self {
        ProjectServiceDefault {
            project_repo,
            project_auth_service,
            plan_limit_service,
            plugin_service,
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
                .ok_or(ProjectError::FailedToCreateDefaultProject(
                    account_id.clone(),
                ))?
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

    async fn get_plugin_installations_for_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<PluginInstallation>, ProjectError> {
        is_authorised(&Role::ViewProject, auth)?;
        let owner_record = auth.as_plugin_owner().into();
        let records = self
            .project_repo
            .get_installed_plugins(&owner_record, &project_id.0)
            .await?;
        records
            .into_iter()
            .map(PluginInstallation::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ProjectError::conversion_error("plugin installation", e))
    }

    async fn create_plugin_installation_for_project(
        &self,
        project_id: &ProjectId,
        installation: PluginInstallationCreation,
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<PluginInstallation, ProjectError> {
        let result = self
            .batch_update_plugin_installations_for_project(
                project_id,
                &[PluginInstallationAction::Install(installation)],
                auth,
                token,
            )
            .await?;
        Ok(result.into_iter().next().unwrap().unwrap())
    }

    async fn update_plugin_installation_for_project(
        &self,
        project_id: &ProjectId,
        installation_id: &PluginInstallationId,
        update: PluginInstallationUpdate,
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<(), ProjectError> {
        let _ = self
            .batch_update_plugin_installations_for_project(
                project_id,
                &[PluginInstallationAction::Update(
                    PluginInstallationUpdateWithId {
                        installation_id: installation_id.clone(),
                        priority: update.priority,
                        parameters: update.parameters,
                    },
                )],
                auth,
                token,
            )
            .await?;
        Ok(())
    }

    async fn delete_plugin_installation_for_project(
        &self,
        installation_id: &PluginInstallationId,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<(), ProjectError> {
        let _ = self
            .batch_update_plugin_installations_for_project(
                project_id,
                &[PluginInstallationAction::Uninstall(PluginUninstallation {
                    installation_id: installation_id.clone(),
                })],
                auth,
                token,
            )
            .await?;
        Ok(())
    }

    async fn batch_update_plugin_installations_for_project(
        &self,
        project_id: &ProjectId,
        actions: &[PluginInstallationAction],
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<Vec<Option<PluginInstallation>>, ProjectError> {
        is_authorised(&Role::UpdateProject, auth)?;
        let owner_record: CloudPluginOwnerRow = auth.as_plugin_owner().into();

        let mut result = Vec::new();
        for action in actions {
            match action {
                PluginInstallationAction::Install(installation) => {
                    let plugin_definition = self
                        .plugin_service
                        .get(&installation.name, &installation.version, token)
                        .await?
                        .ok_or(ProjectError::PluginNotFound {
                            plugin_name: installation.name.clone(),
                            plugin_version: installation.version.clone(),
                        })?;

                    let record = PluginInstallationRecord {
                        installation_id: PluginId::new_v4().0,
                        plugin_id: plugin_definition.id.0,
                        priority: installation.priority,
                        parameters: serde_json::to_vec(&installation.parameters).map_err(|e| {
                            ProjectError::conversion_error(
                                "plugin installation parameters",
                                e.to_string(),
                            )
                        })?,
                        target: ProjectPluginInstallationTarget {
                            project_id: project_id.clone(),
                        }
                        .into(),
                        owner: owner_record.clone(),
                    };

                    self.project_repo.install_plugin(&record).await?;

                    let installation = PluginInstallation::try_from(record)
                        .map_err(|e| ProjectError::conversion_error("plugin record", e))?;
                    result.push(Some(installation));
                }
                PluginInstallationAction::Update(update) => {
                    self.project_repo
                        .update_plugin_installation(
                            &owner_record,
                            &project_id.0,
                            &update.installation_id.0,
                            update.priority,
                            serde_json::to_vec(&update.parameters).map_err(|e| {
                                ProjectError::conversion_error(
                                    "plugin installation parameters",
                                    e.to_string(),
                                )
                            })?,
                        )
                        .await?;
                    result.push(None);
                }
                PluginInstallationAction::Uninstall(uninstallation) => {
                    self.project_repo
                        .uninstall_plugin(
                            &owner_record,
                            &project_id.0,
                            &uninstallation.installation_id.0,
                        )
                        .await?;
                    result.push(None);
                }
            }
        }

        Ok(result)
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
