use clap::Subcommand;

use crate::cloud::service::project::ProjectService;
use golem_cli::model::{GolemError, GolemResult};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProjectSubcommand {
    /// Add a new project
    #[command()]
    Add {
        /// The new project's name
        #[arg(short, long)]
        project_name: String,

        /// The new project's description
        #[arg(short = 't', long)]
        project_description: Option<String>,
    },

    /// Lists existing projects
    #[command()]
    List {
        /// Optionally filter projects by name
        #[arg(short, long)]
        project_name: Option<String>,
    },

    /// Gets the default project which is used when no explicit project is specified
    #[command()]
    GetDefault {},
}

impl ProjectSubcommand {
    pub async fn handle(
        self,
        service: &(dyn ProjectService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ProjectSubcommand::Add {
                project_name,
                project_description,
            } => service.add(project_name, project_description).await,
            ProjectSubcommand::List { project_name } => service.list(project_name).await,
            ProjectSubcommand::GetDefault {} => service.get_default().await,
        }
    }
}
