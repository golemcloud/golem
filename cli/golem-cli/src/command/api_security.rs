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

use crate::model::{GolemError, GolemResult, IdentityProviderType};
use crate::service::api_security::ApiSecuritySchemeService;
use crate::service::project::ProjectResolver;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ApiSecuritySchemeSubcommand<ProjectRef: clap::Args> {
    /// Create ApiSecurity Scheme
    #[command()]
    Create {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Api definition id with version
        #[arg(short = 'i', long = "scheme.id")]
        id: String,

        #[arg(short = 'p', long = "provider.type")]
        provider_type: IdentityProviderType,

        #[arg(long = "client.id")]
        client_id: String,

        #[arg(long = "client.secret")]
        client_secret: String,

        #[arg(short = 's', long = "scopes")]
        scopes: Vec<String>,

        #[arg(short = 'r', long = "redirect.url")]
        redirect_url: String,
    },

    /// Get api security
    #[command()]
    Get {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Security Scheme Id
        #[arg(value_name = "scheme.id")]
        id: String,
    },
}

impl<ProjectRef: clap::Args + Send + Sync + 'static> ApiSecuritySchemeSubcommand<ProjectRef> {
    pub async fn handle<ProjectContext>(
        self,
        service: &(dyn ApiSecuritySchemeService<ProjectContext = ProjectContext> + Send + Sync),
        projects: &(dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ApiSecuritySchemeSubcommand::Create {
                project_ref,
                id,
                provider_type,
                client_id,
                client_secret,
                scopes,
                redirect_url,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service
                    .create(
                        id,
                        provider_type.into(),
                        client_id,
                        client_secret,
                        scopes,
                        redirect_url,
                        &project_id,
                    )
                    .await
            }
            ApiSecuritySchemeSubcommand::Get { id, project_ref } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;

                service.get(id, &project_id).await
            }
        }
    }
}
