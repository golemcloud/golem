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

use async_trait::async_trait;
use clap::Subcommand;
use indoc::formatdoc;
use itertools::Itertools;
use uuid::Uuid;

use crate::clients::component::{ComponentClient, ComponentView};
use crate::clients::project::ProjectClient;
use crate::model::{
    ComponentIdOrName, ComponentName, GolemError, GolemResult, PathBufOrStdin, ProjectId,
    ProjectRef, RawComponentId,
};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubcommand {
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
        component_id_or_name: ComponentIdOrName,

        /// The WASM file to be used as as a new version of the Golem component
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
}

#[async_trait]
pub trait ComponentHandler {
    async fn handle(&self, subcommand: ComponentSubcommand) -> Result<GolemResult, GolemError>;

    async fn resolve_id(&self, reference: ComponentIdOrName) -> Result<RawComponentId, GolemError>;
}

pub struct ComponentHandlerLive<
    'p,
    C: ComponentClient + Send + Sync,
    P: ProjectClient + Sync + Send,
> {
    pub client: C,
    pub projects: &'p P,
}

#[async_trait]
impl<'p, C: ComponentClient + Send + Sync, P: ProjectClient + Sync + Send> ComponentHandler
    for ComponentHandlerLive<'p, C, P>
{
    async fn handle(&self, subcommand: ComponentSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            ComponentSubcommand::Add {
                project_ref,
                component_name,
                component_file,
            } => {
                let project_id = self.projects.resolve_id(project_ref).await?;
                let component = self
                    .client
                    .add(project_id, component_name, component_file)
                    .await?;

                Ok(GolemResult::Ok(Box::new(component)))
            }
            ComponentSubcommand::Update {
                component_id_or_name,
                component_file,
            } => {
                let id = self.resolve_id(component_id_or_name).await?;
                let component = self.client.update(id, component_file).await?;

                Ok(GolemResult::Ok(Box::new(component)))
            }
            ComponentSubcommand::List {
                project_ref,
                component_name,
            } => {
                let project_id = self.projects.resolve_id(project_ref).await?;
                let components = self.client.find(project_id, component_name).await?;

                Ok(GolemResult::Ok(Box::new(components)))
            }
        }
    }

    async fn resolve_id(&self, reference: ComponentIdOrName) -> Result<RawComponentId, GolemError> {
        match reference {
            ComponentIdOrName::Id(id) => Ok(id),
            ComponentIdOrName::Name(name, project_ref) => {
                let project_id = self.projects.resolve_id(project_ref).await?;
                let components = self
                    .client
                    .find(project_id.clone(), Some(name.clone()))
                    .await?;
                let components: Vec<ComponentView> = components
                    .into_iter()
                    .group_by(|c| c.component_id.clone())
                    .into_iter()
                    .map(|(_, group)| group.max_by_key(|c| c.component_version).unwrap())
                    .collect();

                if components.len() > 1 {
                    let project_str =
                        project_id.map_or("default".to_string(), |ProjectId(id)| id.to_string());
                    let component_name = name.0;
                    let ids: Vec<String> = components.into_iter().map(|c| c.component_id).collect();
                    Err(GolemError(formatdoc!(
                        "
                        Multiple components found for name {component_name} in project {project_str}:
                        {}
                        Use explicit --component-id
                    ",
                        ids.join(", ")
                    )))
                } else {
                    match components.first() {
                        None => {
                            let project_str = project_id
                                .map_or("default".to_string(), |ProjectId(id)| id.to_string());
                            let component_name_name = name.0;
                            Err(GolemError(format!(
                                "Can't find component ${component_name_name} in {project_str}"
                            )))
                        }
                        Some(component) => {
                            let parsed = Uuid::parse_str(&component.component_id);

                            match parsed {
                                Ok(id) => Ok(RawComponentId(id)),
                                Err(err) => {
                                    Err(GolemError(format!("Failed to parse component id: {err}")))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
