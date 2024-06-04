// Copyright 2024 Golem Cloud
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

use crate::cloud::model::{CloudComponentIdOrName, ProjectId, ProjectRef};
use crate::cloud::service::project::ProjectService;
use crate::model::{ComponentName, GolemError, GolemResult, PathBufOrStdin};
use crate::service::component::ComponentService;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubCommand {
    /// Creates a new component with a given name by uploading the component WASM
    #[command()]
    Add {
        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Name of the newly created component
        #[arg(short, long)]
        component_name: ComponentName,

        /// The WASM file to be used as a Golem component
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBufOrStdin, // TODO: validate exists
    },

    /// Updates an existing component by uploading a new version of its WASM
    #[command()]
    Update {
        /// The component name or identifier to update
        #[command(flatten)]
        component_id_or_name: CloudComponentIdOrName,

        /// The WASM file to be used as a new version of the Golem component
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBufOrStdin, // TODO: validate exists
    },

    /// Lists the existing components
    #[command()]
    List {
        /// The project to list components from
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Optionally look for only components matching a given name
        #[arg(short, long)]
        component_name: Option<ComponentName>,
    },
    /// Get component
    #[command()]
    Get {
        /// The Golem component id or name
        #[command(flatten)]
        component_id_or_name: CloudComponentIdOrName,

        /// The version of the component
        #[arg(short = 't', long)]
        version: Option<u64>,
    },
}

impl ComponentSubCommand {
    pub async fn handle(
        self,
        service: &(dyn ComponentService<ProjectContext = ProjectId> + Send + Sync),
        projects: &(dyn ProjectService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ComponentSubCommand::Add {
                project_ref,
                component_name,
                component_file,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service
                    .add(component_name, component_file, Some(project_id))
                    .await
            }
            ComponentSubCommand::Update {
                component_id_or_name,
                component_file,
            } => {
                let (component_id_or_name, project_ref) = component_id_or_name.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .update(component_id_or_name, component_file, project_id)
                    .await
            }
            ComponentSubCommand::List {
                project_ref,
                component_name,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.list(component_name, Some(project_id)).await
            }
            ComponentSubCommand::Get {
                component_id_or_name,
                version,
            } => {
                let (component_id_or_name, project_ref) = component_id_or_name.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service.get(component_id_or_name, version, project_id).await
            }
        }
    }
}
