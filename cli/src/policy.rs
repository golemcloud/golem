use async_trait::async_trait;
use clap::Subcommand;

use crate::clients::policy::ProjectPolicyClient;
use crate::clients::CloudAuthentication;
use crate::model::{GolemError, GolemResult, ProjectAction, ProjectPolicyId};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProjectPolicySubcommand {
    #[command()]
    Add {
        #[arg(long)]
        project_policy_name: String,

        #[arg(value_name = "Actions")]
        project_actions: Vec<ProjectAction>,
    },

    #[command()]
    Get {
        #[arg(value_name = "ID")]
        project_policy_id: ProjectPolicyId,
    },
}

#[async_trait]
pub trait ProjectPolicyHandler {
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        subcommand: ProjectPolicySubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct ProjectPolicyHandlerLive<C: ProjectPolicyClient + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: ProjectPolicyClient + Send + Sync> ProjectPolicyHandler for ProjectPolicyHandlerLive<C> {
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        subcommand: ProjectPolicySubcommand,
    ) -> Result<GolemResult, GolemError> {
        match subcommand {
            ProjectPolicySubcommand::Add {
                project_actions,
                project_policy_name,
            } => {
                let policy = self
                    .client
                    .create(project_policy_name, project_actions, auth)
                    .await?;

                Ok(GolemResult::Ok(Box::new(policy)))
            }
            ProjectPolicySubcommand::Get { project_policy_id } => {
                let policy = self.client.get(project_policy_id, auth).await?;

                Ok(GolemResult::Ok(Box::new(policy)))
            }
        }
    }
}
