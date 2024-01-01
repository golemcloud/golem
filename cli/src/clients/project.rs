use async_trait::async_trait;
use golem_client::model::Project;
use golem_client::model::ProjectDataRequest;
use indoc::formatdoc;
use tracing::info;

use crate::model::{AccountId, GolemError, ProjectId, ProjectRef};

#[async_trait]
pub trait ProjectClient {
    async fn create(
        &self,
        owner_account_id: AccountId,
        name: String,
        description: Option<String>,
    ) -> Result<Project, GolemError>;
    async fn find(&self, name: Option<String>) -> Result<Vec<Project>, GolemError>;
    async fn find_default(&self) -> Result<Project, GolemError>;
    async fn delete(&self, project_id: ProjectId) -> Result<(), GolemError>;

    async fn resolve_id(&self, project_ref: ProjectRef) -> Result<Option<ProjectId>, GolemError>;

    async fn resolve_id_or_default(
        &self,
        project_ref: ProjectRef,
    ) -> Result<ProjectId, GolemError> {
        match self.resolve_id(project_ref).await? {
            None => Ok(ProjectId(self.find_default().await?.project_id)),
            Some(project_id) => Ok(project_id),
        }
    }
}

pub struct ProjectClientLive<C: golem_client::api::ProjectClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ProjectClient + Sync + Send> ProjectClient for ProjectClientLive<C> {
    async fn create(
        &self,
        owner_account_id: AccountId,
        name: String,
        description: Option<String>,
    ) -> Result<Project, GolemError> {
        info!("Create new project {name}.");

        let request = ProjectDataRequest {
            name,
            owner_account_id: owner_account_id.id,
            description: description.unwrap_or("".to_string()),
        };
        Ok(self.client.post(&request).await?)
    }

    async fn find(&self, name: Option<String>) -> Result<Vec<Project>, GolemError> {
        info!("Listing projects.");

        Ok(self.client.get(name.as_deref()).await?)
    }

    async fn find_default(&self) -> Result<Project, GolemError> {
        info!("Getting default project.");

        Ok(self.client.default_get().await?)
    }

    async fn delete(&self, project_id: ProjectId) -> Result<(), GolemError> {
        info!("Deleting project {project_id:?}");

        let _ = self.client.project_id_delete(&project_id.0).await?;

        Ok(())
    }

    async fn resolve_id(&self, project_ref: ProjectRef) -> Result<Option<ProjectId>, GolemError> {
        match project_ref {
            ProjectRef::Id(id) => Ok(Some(id)),
            ProjectRef::Name(name) => {
                let projects = self.find(Some(name.clone())).await?;

                if projects.len() > 1 {
                    let projects: Vec<String> =
                        projects.iter().map(|p| p.project_id.to_string()).collect();
                    Err(GolemError(formatdoc!(
                        "
                            Multiple projects found for name {name}:
                            {}
                            Use explicit --project-id or set target project as default.
                        ",
                        projects.join(", ")
                    )))
                } else {
                    match projects.first() {
                        None => Err(GolemError(format!("Can't find project with name {name}"))),
                        Some(project) => Ok(Some(ProjectId(project.project_id))),
                    }
                }
            }
            ProjectRef::Default => Ok(None),
        }
    }
}
