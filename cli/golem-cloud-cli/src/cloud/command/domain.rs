use crate::cloud::model::ProjectRef;
use crate::cloud::service::domain::DomainService;
use clap::Subcommand;

use golem_cli::model::{GolemError, GolemResult};

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

impl DomainSubcommand {
    pub async fn handle(
        self,
        service: &(dyn DomainService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            DomainSubcommand::Get { project_ref } => service.get(project_ref).await,
            DomainSubcommand::Add {
                project_ref,
                domain_name,
            } => service.add(project_ref, domain_name).await,
            DomainSubcommand::Delete {
                project_ref,
                domain_name,
            } => service.delete(project_ref, domain_name).await,
        }
    }
}
