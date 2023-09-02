use async_trait::async_trait;
use crate::clients::CloudAuthentication;
use crate::clients::project::ProjectClient;
use crate::model::{GolemError, GolemResult};
use crate::ProjectSubcommand;

#[async_trait]
pub trait ProjectHandler {
    async fn handle(&self, token: &CloudAuthentication, subcommand: ProjectSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct ProjectHandlerLive<'c, C: ProjectClient + Send + Sync> {
    pub client: &'c C
}

#[async_trait]
impl<'c, C: ProjectClient + Send + Sync> ProjectHandler for ProjectHandlerLive<'c, C> {
    async fn handle(&self, auth: &CloudAuthentication, subcommand: ProjectSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            ProjectSubcommand::Add { project_name, project_description } => {
                let project = self.client.create(project_name, project_description, auth).await?;

                Ok(GolemResult::Ok(Box::new(project)))
            }
            ProjectSubcommand::List { project_name } => {
                let projects = self.client.find(project_name, auth).await?;

                Ok(GolemResult::Ok(Box::new(projects)))
            }
            ProjectSubcommand::GetDefault { } => {
                let project = self.client.find_default(auth).await?;

                Ok(GolemResult::Ok(Box::new(project)))
            }
        }
    }
}