use async_trait::async_trait;
use golem_client::model::{Project, ProjectDataRequest};
use indoc::formatdoc;
use tracing::info;

use crate::clients::CloudAuthentication;
use crate::model::{GolemError, ProjectId, ProjectRef};

#[async_trait]
pub trait ProjectClient {
    async fn create(
        &self,
        name: String,
        description: Option<String>,
        auth: &CloudAuthentication,
    ) -> Result<Project, GolemError>;
    async fn find(
        &self,
        name: Option<String>,
        auth: &CloudAuthentication,
    ) -> Result<Vec<Project>, GolemError>;
    async fn find_default(&self, auth: &CloudAuthentication) -> Result<Project, GolemError>;
    async fn delete(
        &self,
        project_id: ProjectId,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError>;

    async fn resolve_id(
        &self,
        project_ref: ProjectRef,
        auth: &CloudAuthentication,
    ) -> Result<Option<ProjectId>, GolemError>;

    async fn resolve_id_or_default(
        &self,
        project_ref: ProjectRef,
        auth: &CloudAuthentication,
    ) -> Result<ProjectId, GolemError> {
        match self.resolve_id(project_ref, auth).await? {
            None => Ok(ProjectId(self.find_default(auth).await?.project_id)),
            Some(project_id) => Ok(project_id),
        }
    }
}

pub struct ProjectClientLive<C: golem_client::project::Project + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::project::Project + Send + Sync> ProjectClient for ProjectClientLive<C> {
    async fn create(
        &self,
        name: String,
        description: Option<String>,
        auth: &CloudAuthentication,
    ) -> Result<Project, GolemError> {
        info!("Create new project {name}.");

        let request = ProjectDataRequest {
            name,
            owner_account_id: auth.account_id().id,
            description: description.unwrap_or("".to_string()),
        };
        Ok(self.client.post_project(request, &auth.header()).await?)
    }

    async fn find(
        &self,
        name: Option<String>,
        auth: &CloudAuthentication,
    ) -> Result<Vec<Project>, GolemError> {
        info!("Listing projects.");

        Ok(self
            .client
            .get_projects(name.as_deref(), &auth.header())
            .await?)
    }

    async fn find_default(&self, auth: &CloudAuthentication) -> Result<Project, GolemError> {
        info!("Getting default project.");

        Ok(self.client.get_default_project(&auth.header()).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError> {
        info!("Deleting project {project_id:?}");

        Ok(self
            .client
            .delete_project(&project_id.0.to_string(), &auth.header())
            .await?)
    }

    async fn resolve_id(
        &self,
        project_ref: ProjectRef,
        auth: &CloudAuthentication,
    ) -> Result<Option<ProjectId>, GolemError> {
        match project_ref {
            ProjectRef::Id(id) => Ok(Some(id)),
            ProjectRef::Name(name) => {
                let projects = self.find(Some(name.clone()), auth).await?;

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
