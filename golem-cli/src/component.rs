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
use golem_client::model::Component;
use indoc::formatdoc;
use itertools::Itertools;

use crate::clients::component::ComponentClient;
use crate::model::component::ComponentView;
use crate::model::text::{ComponentAddView, ComponentUpdateView};
use crate::model::{
    ComponentId, ComponentIdOrName, ComponentName, GolemError, GolemResult, PathBufOrStdin,
};

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubCommand {
    /// Creates a new component with a given name by uploading the component WASM
    #[command()]
    Add {
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
        /// Optionally look for only components matching a given name
        #[arg(short, long)]
        component_name: Option<ComponentName>,
    },
}

#[async_trait]
pub trait ComponentHandler {
    async fn handle(&self, subcommand: ComponentSubCommand) -> Result<GolemResult, GolemError>;

    async fn resolve_id(&self, reference: ComponentIdOrName) -> Result<ComponentId, GolemError>;

    async fn get_metadata(
        &self,
        component_id: &ComponentId,
        version: u64,
    ) -> Result<Component, GolemError>;

    async fn get_latest_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<Component, GolemError>;
}

pub struct ComponentHandlerLive<C: ComponentClient + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: ComponentClient + Send + Sync> ComponentHandler for ComponentHandlerLive<C> {
    async fn handle(&self, subcommand: ComponentSubCommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            ComponentSubCommand::Add {
                component_name,
                component_file,
            } => {
                let component = self.client.add(component_name, component_file).await?;
                let view: ComponentView = component.into();

                Ok(GolemResult::Ok(Box::new(ComponentAddView(view))))
            }
            ComponentSubCommand::Update {
                component_id_or_name,
                component_file,
            } => {
                let id = self.resolve_id(component_id_or_name).await?;
                let component = self.client.update(id, component_file).await?;
                let view: ComponentView = component.into();

                Ok(GolemResult::Ok(Box::new(ComponentUpdateView(view))))
            }
            ComponentSubCommand::List { component_name } => {
                let components = self.client.find(component_name).await?;
                let views: Vec<ComponentView> = components.into_iter().map(|t| t.into()).collect();

                Ok(GolemResult::Ok(Box::new(views)))
            }
        }
    }

    async fn resolve_id(&self, reference: ComponentIdOrName) -> Result<ComponentId, GolemError> {
        match reference {
            ComponentIdOrName::Id(id) => Ok(id),
            ComponentIdOrName::Name(name) => {
                let components = self.client.find(Some(name.clone())).await?;
                let components: Vec<Component> = components
                    .into_iter()
                    .group_by(|c| c.versioned_component_id.component_id)
                    .into_iter()
                    .map(|(_, group)| {
                        group
                            .max_by_key(|c| c.versioned_component_id.version)
                            .unwrap()
                    })
                    .collect();

                if components.len() > 1 {
                    let component_name = name.0;
                    let ids: Vec<String> = components
                        .into_iter()
                        .map(|c| c.versioned_component_id.component_id.to_string())
                        .collect();
                    Err(GolemError(formatdoc!(
                        "
                        Multiple components found for name {component_name}:
                        {}
                        Use explicit --component-id
                    ",
                        ids.join(", ")
                    )))
                } else {
                    match components.first() {
                        None => {
                            let component_name = name.0;
                            Err(GolemError(format!("Can't find component {component_name}")))
                        }
                        Some(component) => {
                            Ok(ComponentId(component.versioned_component_id.component_id))
                        }
                    }
                }
            }
        }
    }

    async fn get_metadata(
        &self,
        component_id: &ComponentId,
        version: u64,
    ) -> Result<Component, GolemError> {
        self.client.get_metadata(component_id, version).await
    }

    async fn get_latest_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<Component, GolemError> {
        self.client.get_latest_metadata(component_id).await
    }
}
