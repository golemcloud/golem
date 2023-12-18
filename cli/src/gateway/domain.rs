use async_trait::async_trait;
use clap::Subcommand;

use crate::clients::gateway::domain::DomainClient;
use crate::clients::project::ProjectClient;
use crate::model::{GolemError, GolemResult, ProjectRef};

#[derive(Subcommand, Debug)]
#[command()]
pub enum DomainSubcommand {
    #[command()]
    Get {
        #[command(flatten)]
        project_ref: ProjectRef,
    },
    #[command()]
    Add {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long, value_hint = clap::ValueHint::Other)]
        domain_name: String,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(value_name = "domain-name", value_hint = clap::ValueHint::Other)]
        domain_name: String,
    },
}

#[async_trait]
pub trait DomainHandler {
    async fn handle(&self, command: DomainSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct DomainHandlerLive<'p, C: DomainClient + Sync + Send, P: ProjectClient + Sync + Send> {
    pub client: C,
    pub projects: &'p P,
}

#[async_trait]
impl<'p, C: DomainClient + Sync + Send, P: ProjectClient + Sync + Send> DomainHandler
    for DomainHandlerLive<'p, C, P>
{
    async fn handle(&self, command: DomainSubcommand) -> Result<GolemResult, GolemError> {
        match command {
            DomainSubcommand::Get { project_ref } => {
                let project_id = self.projects.resolve_id_or_default(project_ref).await?;

                let res = self.client.get(project_id).await?;

                Ok(GolemResult::Ok(Box::new(res)))
            }
            DomainSubcommand::Add {
                project_ref,
                domain_name,
            } => {
                let project_id = self.projects.resolve_id_or_default(project_ref).await?;

                let res = self.client.update(project_id, domain_name).await?;

                Ok(GolemResult::Ok(Box::new(res)))
            }
            DomainSubcommand::Delete {
                project_ref,
                domain_name,
            } => {
                let project_id = self.projects.resolve_id_or_default(project_ref).await?;
                let res = self.client.delete(project_id, &domain_name).await?;
                Ok(GolemResult::Ok(Box::new(res)))
            }
        }
    }
}
