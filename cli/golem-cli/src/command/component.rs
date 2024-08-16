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

use crate::command::ComponentRefSplit;
use crate::model::{
    ComponentName, Format, GolemError, GolemResult, PathBufOrStdin, WorkerUpdateMode,
};
use crate::service::component::ComponentService;
use crate::service::deploy::DeployService;
use crate::service::project::ProjectResolver;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubCommand<ProjectRef: clap::Args, ComponentRef: clap::Args> {
    /// Creates a new component with a given name by uploading the component WASM
    #[command(alias = "create")]
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

        /// Do not ask for performing an update in case the component already exists
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },

    /// Updates an existing component by uploading a new version of its WASM
    #[command()]
    Update {
        /// The component to update
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The WASM file to be used as a new version of the Golem component
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBufOrStdin, // TODO: validate exists

        /// Try to automatically update all existing workers to the new version
        #[arg(long, default_value_t = false)]
        try_update_workers: bool,

        /// Update mode - auto or manual
        #[arg(long, default_value = "auto", requires = "try_update_workers")]
        update_mode: WorkerUpdateMode,

        /// Do not ask for creating a new component in case it does not exists
        #[arg(short = 'y', long)]
        non_interactive: bool,
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
        /// The Golem component
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The version of the component
        #[arg(short = 't', long)]
        version: Option<u64>,
    },
    /// Try to automatically update all existing workers to the latest version
    #[command()]
    TryUpdateWorkers {
        /// The component to redeploy
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Update mode - auto or manual
        #[arg(long, default_value = "auto")]
        update_mode: WorkerUpdateMode,
    },
    /// Redeploy all workers of a component using the latest version
    #[command()]
    Redeploy {
        /// The component to redeploy
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Do not ask for confirmation
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },
}

impl<
        ProjectRef: clap::Args + Send + Sync + 'static,
        ComponentRef: ComponentRefSplit<ProjectRef> + clap::Args,
    > ComponentSubCommand<ProjectRef, ComponentRef>
{
    pub async fn handle<ProjectContext: Clone + Send + Sync>(
        self,
        format: Format,
        service: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
        deploy_service: Arc<dyn DeployService<ProjectContext = ProjectContext> + Send + Sync>,
        projects: &(dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ComponentSubCommand::Add {
                project_ref,
                component_name,
                component_file,
                non_interactive,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service
                    .add(
                        component_name,
                        component_file,
                        Some(project_id),
                        non_interactive,
                        format,
                    )
                    .await
            }
            ComponentSubCommand::Update {
                component_name_or_uri,
                component_file,
                try_update_workers,
                update_mode,
                non_interactive,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                let mut result = service
                    .update(
                        component_name_or_uri.clone(),
                        component_file,
                        project_id.clone(),
                        non_interactive,
                        format,
                    )
                    .await?;

                if try_update_workers {
                    let deploy_result = deploy_service
                        .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                        .await?;
                    result = result.merge(deploy_result);
                }
                Ok(result)
            }
            ComponentSubCommand::List {
                project_ref,
                component_name,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.list(component_name, Some(project_id)).await
            }
            ComponentSubCommand::Get {
                component_name_or_uri,
                version,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .get(component_name_or_uri, version, project_id)
                    .await
            }
            ComponentSubCommand::TryUpdateWorkers {
                component_name_or_uri,
                update_mode,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                deploy_service
                    .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                    .await
            }
            ComponentSubCommand::Redeploy {
                component_name_or_uri,
                non_interactive,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                deploy_service
                    .redeploy(component_name_or_uri, project_id, non_interactive, format)
                    .await
            }
        }
    }
}
