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

use crate::model::{
    ApiDefinitionFileFormat, ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult,
    PathBufOrStdin,
};
use crate::service::api_definition::ApiDefinitionService;
use crate::service::project::ProjectResolver;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ApiDefinitionSubcommand<ProjectRef: clap::Args> {
    /// Lists all api definitions
    #[command()]
    List {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Api definition id to get all versions. Optional.
        #[arg(short, long)]
        id: Option<ApiDefinitionId>,
    },

    /// Creates an api definition
    ///
    /// Golem API definition file format expected
    #[command(alias = "create")]
    Add {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// The Golem API definition file
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists

        /// Api Definition format
        #[arg(short, long)]
        def_format: Option<ApiDefinitionFileFormat>,
    },

    /// Updates an api definition
    ///
    /// Golem API definition file format expected
    #[command()]
    Update {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// The Golem API definition file
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists

        /// Api Definition format
        #[arg(short, long)]
        def_format: Option<ApiDefinitionFileFormat>,
    },

    /// Import OpenAPI file as api definition
    #[command()]
    Import {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// The OpenAPI json or yaml file to be used as the api definition
        ///
        /// Json format expected unless file name ends up in `.yaml`
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists

        /// Api Definition format
        #[arg(short, long)]
        def_format: Option<ApiDefinitionFileFormat>,
    },

    /// Retrieves metadata about an existing api definition
    #[command()]
    Get {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,

        /// Version of the api definition
        #[arg(short = 'V', long)]
        version: ApiDefinitionVersion,
    },

    /// Deletes an existing api definition
    #[command()]
    Delete {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,

        /// Version of the api definition
        #[arg(short = 'V', long)]
        version: ApiDefinitionVersion,
    },
}

impl<ProjectRef: clap::Args + Send + Sync + 'static> ApiDefinitionSubcommand<ProjectRef> {
    pub async fn handle<ProjectContext>(
        self,
        service: &(dyn ApiDefinitionService<ProjectContext = ProjectContext> + Send + Sync),
        projects: &(dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        let with_default: fn(Option<ApiDefinitionFileFormat>) -> ApiDefinitionFileFormat =
            |format| format.unwrap_or(ApiDefinitionFileFormat::Json);

        match self {
            ApiDefinitionSubcommand::Get {
                project_ref,
                id,
                version,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.get(id, version, &project_id).await
            }
            ApiDefinitionSubcommand::Add {
                project_ref,
                definition,
                def_format: format,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service
                    .add(definition, &project_id, &with_default(format))
                    .await
            }
            ApiDefinitionSubcommand::Update {
                project_ref,
                definition,
                def_format: format,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service
                    .update(definition, &project_id, &with_default(format))
                    .await
            }
            ApiDefinitionSubcommand::Import {
                project_ref,
                definition,
                def_format: format,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service
                    .import(definition, &project_id, &with_default(format))
                    .await
            }
            ApiDefinitionSubcommand::List { project_ref, id } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.list(id, &project_id).await
            }
            ApiDefinitionSubcommand::Delete {
                project_ref,
                id,
                version,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.delete(id, version, &project_id).await
            }
        }
    }
}
