use clap::Subcommand;

use crate::cloud::model::ProjectPolicyId;
use crate::cloud::service::policy::ProjectPolicyService;
use golem_cli::model::{GolemError, GolemResult};
use golem_cloud_client::model::ProjectAction;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProjectPolicySubcommand {
    /// Creates a new project sharing policy
    #[command(alias = "create")]
    Add {
        /// Name of the policy
        #[arg(long)]
        project_policy_name: String,

        /// List of actions allowed by the policy
        #[arg(value_name = "Actions")]
        project_actions: Vec<ProjectAction>,
    },

    /// Gets the existing project sharing policies
    #[command()]
    Get {
        #[arg(value_name = "ID")]
        project_policy_id: ProjectPolicyId,
    },
}

impl ProjectPolicySubcommand {
    pub async fn handle(
        self,
        service: &(dyn ProjectPolicyService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ProjectPolicySubcommand::Add {
                project_actions,
                project_policy_name,
            } => service.add(project_policy_name, project_actions).await,
            ProjectPolicySubcommand::Get { project_policy_id } => {
                service.get(project_policy_id).await
            }
        }
    }
}
