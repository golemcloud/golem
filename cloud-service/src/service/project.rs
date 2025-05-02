use crate::auth::AccountAuthorisation;
use crate::model::{
    AccountAction, Project, ProjectData, ProjectPluginInstallationTarget, ProjectType,
};
use crate::repo::project::{ProjectRecord, ProjectRepo};
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};
use async_trait::async_trait;
use cloud_common::clients::plugin::{PluginError, PluginServiceClient};
use cloud_common::model::ProjectAction;
use cloud_common::model::TokenSecret;
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

use super::auth::{AuthService, AuthServiceError, ViewableProjects};

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("Limit Exceeded: {0}")]
    LimitExceeded(String),
    #[error(transparent)]
    InternalPlanLimitError(PlanLimitError),
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
    #[error("Cannot delete default project")]
    CannotDeleteDefaultProject,
    #[error(transparent)]
    InternalProjectAuthorisationError(#[from] AuthServiceError),
}

impl ProjectError {
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
            ProjectError::InternalPlanLimitError(inner) => inner.to_safe_string(),
            ProjectError::InternalProjectAuthorisationError(inner) => inner.to_safe_string(),
            ProjectError::FailedToCreateDefaultProject(_) => self.to_string(),
            ProjectError::InternalRepoError(inner) => inner.to_safe_string(),
            ProjectError::InternalConversionError { .. } => self.to_string(),
            ProjectError::PluginNotFound { .. } => self.to_string(),
            ProjectError::InternalPluginError(inner) => inner.to_safe_string(),
            ProjectError::CannotDeleteDefaultProject => self.to_string(),
        }
    }
}

impl From<PlanLimitError> for ProjectError {
    fn from(error: PlanLimitError) -> Self {
        match error {
            PlanLimitError::LimitExceeded(error) => ProjectError::limit_exceeded(error),
            PlanLimitError::AuthError(inner) => inner.into(),
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

    async fn get_default(&self, auth: &AccountAuthorisation) -> Result<Project, ProjectError>;

    async fn get_all(&self, auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError>;

    async fn get_all_by_name(
        &self,
        name: &str,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Project>, ProjectError>;

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
    auth_service: Arc<dyn AuthService>,
    project_repo: Arc<dyn ProjectRepo + Send + Sync>,
    plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,
    plugin_service: Arc<dyn PluginServiceClient + Send + Sync>,
}

impl ProjectServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService>,
        project_repo: Arc<dyn ProjectRepo + Send + Sync>,
        plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,
        plugin_service: Arc<dyn PluginServiceClient + Send + Sync>,
    ) -> Self {
        ProjectServiceDefault {
            auth_service,
            project_repo,
            plan_limit_service,
            plugin_service,
        }
    }

    async fn batch_update_plugin_installations_for_project_noauth(
        &self,
        project_id: &ProjectId,
        actions: &[PluginInstallationAction],
        auth: &AccountAuthorisation,
        token: &TokenSecret,
    ) -> Result<Vec<Option<PluginInstallation>>, ProjectError> {
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

#[async_trait]
impl ProjectService for ProjectServiceDefault {
    async fn create(
        &self,
        project: &Project,
        auth: &AccountAuthorisation,
    ) -> Result<(), ProjectError> {
        self.auth_service
            .authorize_account_action(
                auth,
                &project.project_data.owner_account_id,
                &AccountAction::CreateProject,
            )
            .await?;

        info!("Create project {}", project.project_id);

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
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::DeleteProject)
            .await?;

        info!("Delete project {}", project_id);

        let project = self.project_repo.get(&project_id.0).await?;

        if let Some(project) = project {
            if project.is_default {
                Err(ProjectError::CannotDeleteDefaultProject)?
            };

            // FIXME delete components, workers ...

            // let component_count = self
            //     .component_repo
            //     .get_count_by_projects(vec![project_id.0])
            //     .await?;

            self.project_repo.delete(&project_id.0).await?;
        }

        Ok(())
    }

    async fn get_default(&self, auth: &AccountAuthorisation) -> Result<Project, ProjectError> {
        let account_id = &auth.token.account_id;

        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::ViewDefaultProject)
            .await?;

        info!("Getting default project for account {}", account_id);
        let result = self
            .project_repo
            .get_default(account_id.value.as_str())
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
                .get_default(account_id.value.as_str())
                .await?;
            Ok(result
                .ok_or(ProjectError::FailedToCreateDefaultProject(
                    account_id.clone(),
                ))?
                .into())
        }
    }

    async fn get_all(&self, auth: &AccountAuthorisation) -> Result<Vec<Project>, ProjectError> {
        let visible_projects = self.auth_service.viewable_projects(auth).await?;

        match visible_projects {
            ViewableProjects::All => {
                info!("Getting all projects");
                let result = self.project_repo.get_all().await?;
                Ok(result.iter().map(|p| p.clone().into()).collect())
            }
            ViewableProjects::OwnedAndAdditional {
                additional_project_ids,
            } => {
                let account_id = &auth.token.account_id;
                info!("Getting projects for account {}", account_id);
                let additional_project_ids = additional_project_ids
                    .into_iter()
                    .map(|pid| pid.0)
                    .collect::<Vec<_>>();
                let result = self
                    .project_repo
                    .get_owned(account_id.value.as_str(), &additional_project_ids)
                    .await?;
                Ok(result.iter().map(|p| p.clone().into()).collect())
            }
        }
    }

    async fn get_all_by_name(
        &self,
        name: &str,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Project>, ProjectError> {
        // Auth is done in get_all

        let result = self.get_all(auth).await?;
        Ok(result
            .into_iter()
            .filter(|p| p.project_data.name == name)
            .collect())
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<Project>, ProjectError> {
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::ViewProject)
            .await?;
        info!("Getting project {}", project_id);
        let result = self.project_repo.get(&project_id.0).await?;
        Ok(result.map(|p| p.into()))
    }

    async fn get_plugin_installations_for_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<PluginInstallation>, ProjectError> {
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::ViewPluginInstallations)
            .await?;

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
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::CreatePluginInstallation)
            .await?;

        let result = self
            .batch_update_plugin_installations_for_project_noauth(
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
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::UpdatePluginInstallation)
            .await?;

        let _ = self
            .batch_update_plugin_installations_for_project_noauth(
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
        self.auth_service
            .authorize_project_action(auth, project_id, &ProjectAction::DeletePluginInstallation)
            .await?;

        let _ = self
            .batch_update_plugin_installations_for_project_noauth(
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
        self.auth_service
            .authorize_project_action(
                auth,
                project_id,
                &ProjectAction::BatchUpdatePluginInstallations,
            )
            .await?;

        let result = self
            .batch_update_plugin_installations_for_project_noauth(project_id, actions, auth, token)
            .await?;

        Ok(result)
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
