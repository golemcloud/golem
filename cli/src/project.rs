use async_trait::async_trait;
use clap::Subcommand;

use crate::clients::project::ProjectClient;
use crate::clients::CloudAuthentication;
use crate::model::{AccountId, GolemError, GolemResult};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProjectSubcommand {
    #[command()]
    Add {
        #[arg(short, long)]
        project_name: String,

        #[arg(short = 't', long)]
        project_description: Option<String>,
    },

    #[command()]
    List {
        #[arg(short, long)]
        project_name: Option<String>,
    },

    #[command()]
    GetDefault {},
}

#[async_trait]
pub trait ProjectHandler {
    async fn handle(
        &self,
        token: &CloudAuthentication,
        subcommand: ProjectSubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct ProjectHandlerLive<'c, C: ProjectClient + Send + Sync> {
    pub client: &'c C,
}

#[async_trait]
impl<'c, C: ProjectClient + Send + Sync> ProjectHandler for ProjectHandlerLive<'c, C> {
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        subcommand: ProjectSubcommand,
    ) -> Result<GolemResult, GolemError> {
        match subcommand {
            ProjectSubcommand::Add {
                project_name,
                project_description,
            } => {
                let project = self
                    .client
                    .create(
                        AccountId {
                            id: auth.0.data.account_id.clone(),
                        },
                        project_name,
                        project_description,
                    )
                    .await?;

                Ok(GolemResult::Ok(Box::new(project)))
            }
            ProjectSubcommand::List { project_name } => {
                let projects = self.client.find(project_name).await?;

                Ok(GolemResult::Ok(Box::new(projects)))
            }
            ProjectSubcommand::GetDefault {} => {
                let project = self.client.find_default().await?;

                Ok(GolemResult::Ok(Box::new(project)))
            }
        }
    }
}
