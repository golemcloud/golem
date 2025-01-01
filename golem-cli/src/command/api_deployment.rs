// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::{ApiDefinitionId, ApiDefinitionIdWithVersion, GolemError, GolemResult};
use crate::service::api_deployment::ApiDeploymentService;
use crate::service::project::ProjectResolver;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ApiDeploymentSubcommand<ProjectRef: clap::Args> {
    /// Create or update deployment
    #[command()]
    Deploy {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Api definition id with version
        #[arg(short = 'd', long = "definition")]
        definitions: Vec<ApiDefinitionIdWithVersion>,

        #[arg(short = 'H', long)]
        host: String,

        #[arg(short, long)]
        subdomain: Option<String>,
    },

    /// Get api deployment
    #[command()]
    Get {
        /// Deployment site
        #[arg(value_name = "subdomain.host")]
        site: String,
    },

    /// List api deployment for api definition
    #[command()]
    List {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,
    },

    /// Delete api deployment
    #[command()]
    Delete {
        /// Deployment site
        #[arg(value_name = "subdomain.host")]
        site: String,
    },
}

impl<ProjectRef: clap::Args + Send + Sync + 'static> ApiDeploymentSubcommand<ProjectRef> {
    pub async fn handle<ProjectContext>(
        self,
        service: &(dyn ApiDeploymentService<ProjectContext = ProjectContext> + Send + Sync),
        projects: &(dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ApiDeploymentSubcommand::Deploy {
                project_ref,
                definitions,
                host,
                subdomain,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service
                    .deploy(definitions, host, subdomain, &project_id)
                    .await
            }
            ApiDeploymentSubcommand::Get { site } => service.get(site).await,
            ApiDeploymentSubcommand::List { project_ref, id } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.list(id, &project_id).await
            }
            ApiDeploymentSubcommand::Delete { site } => service.delete(site).await,
        }
    }
}
