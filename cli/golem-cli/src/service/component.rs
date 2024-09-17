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

use crate::clients::component::ComponentClient;
use crate::model::component::{Component, ComponentView};
use crate::model::text::{ComponentAddView, ComponentGetView, ComponentUpdateView};
use crate::model::{ComponentName, Format, GolemError, GolemResult, PathBufOrStdin};
use async_trait::async_trait;
use golem_client::model::ComponentType;
use golem_common::model::ComponentId;
use golem_common::uri::oss::uri::ComponentUri;
use golem_common::uri::oss::url::ComponentUrl;
use golem_common::uri::oss::urn::ComponentUrn;
use indoc::formatdoc;
use itertools::Itertools;
use std::fmt::Display;

#[async_trait]
pub trait ComponentService {
    type ProjectContext: Send + Sync;

    async fn add(
        &self,
        component_name: ComponentName,
        component_file: PathBufOrStdin,
        component_type: ComponentType,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
    ) -> Result<GolemResult, GolemError>;
    async fn update(
        &self,
        component_uri: ComponentUri,
        component_file: PathBufOrStdin,
        component_type: Option<ComponentType>,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
    ) -> Result<GolemResult, GolemError>;
    async fn list(
        &self,
        component_name: Option<ComponentName>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn get(
        &self,
        component_uri: ComponentUri,
        version: Option<u64>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn resolve_uri(
        &self,
        uri: ComponentUri,
        project: &Option<Self::ProjectContext>,
    ) -> Result<ComponentUrn, GolemError>;
    async fn get_metadata(
        &self,
        component_urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError>;
    async fn get_latest_metadata(
        &self,
        component_urn: &ComponentUrn,
    ) -> Result<Component, GolemError>;
}

pub struct ComponentServiceLive<ProjectContext> {
    pub client: Box<dyn ComponentClient<ProjectContext = ProjectContext> + Send + Sync>,
}

#[async_trait]
impl<ProjectContext: Display + Send + Sync> ComponentService
    for ComponentServiceLive<ProjectContext>
{
    type ProjectContext = ProjectContext;

    async fn add(
        &self,
        component_name: ComponentName,
        component_file: PathBufOrStdin,
        component_type: ComponentType,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
    ) -> Result<GolemResult, GolemError> {
        let result = self
            .client
            .add(
                component_name.clone(),
                component_file.clone(),
                &project,
                component_type.clone(),
            )
            .await;

        let can_fallback = format == Format::Text;
        let result = match result {
            Err(GolemError(message))
                if message.starts_with("Component already exists") && can_fallback =>
            {
                let answer = {
                    if non_interactive {
                        Ok(true)
                    } else {
                        inquire::Confirm::new("Would you like to update the existing component?")
                            .with_default(false)
                            .with_help_message(&message)
                            .prompt()
                    }
                };

                match answer {
                    Ok(true) => {
                        let component_uri = ComponentUri::URL(ComponentUrl {
                            name: component_name.0.clone(),
                        });
                        let urn = self.resolve_uri(component_uri, &project).await?;
                        self.client.update(urn, component_file, Some(component_type)).await.map(|component| GolemResult::Ok(Box::new(ComponentUpdateView(component.into()))))

                    }
                    Ok(false) => Err(GolemError(message)),
                    Err(error) => Err(GolemError(format!("Error while asking for confirmation: {}; Use the --non-interactive (-y) flag to bypass it.", error))),
                }
            }
            Err(other) => Err(other),
            Ok(component) => Ok(GolemResult::Ok(Box::new(ComponentAddView(
                component.into(),
            )))),
        }?;

        Ok(result)
    }

    async fn update(
        &self,
        component_uri: ComponentUri,
        component_file: PathBufOrStdin,
        component_type: Option<ComponentType>,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
    ) -> Result<GolemResult, GolemError> {
        let result = self.resolve_uri(component_uri.clone(), &project).await;

        let can_fallback =
            format == Format::Text && matches!(component_uri, ComponentUri::URL { .. });
        let result = match result {
            Err(GolemError(message))
                if message.starts_with("Can't find component") && can_fallback =>
            {
                let answer = {
                    if non_interactive {
                        Ok(true)
                    } else {
                        inquire::Confirm::new("Would you like to create a new component?")
                            .with_default(false)
                            .with_help_message(&message)
                            .prompt()
                    }
                };

                match answer {
                        Ok(true) => {
                            let component_name = match &component_uri {
                                ComponentUri::URL(ComponentUrl { name }) => ComponentName(name.clone()),
                                _ => unreachable!(),
                            };
                            self.client.add(component_name, component_file, &project, component_type.unwrap_or(ComponentType::Durable)).await.map(|component| {
                                GolemResult::Ok(Box::new(ComponentAddView(component.into())))
                            })

                        }
                        Ok(false) => Err(GolemError(message)),
                        Err(error) => Err(GolemError(format!("Error while asking for confirmation: {}; Use the --non-interactive (-y) flag to bypass it.", error))),
                    }
            }
            Err(other) => Err(other),
            Ok(urn) => self
                .client
                .update(urn, component_file.clone(), component_type)
                .await
                .map(|component| GolemResult::Ok(Box::new(ComponentUpdateView(component.into())))),
        }?;

        Ok(result)
    }

    async fn list(
        &self,
        component_name: Option<ComponentName>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let components = self.client.find(component_name, &project).await?;
        let views: Vec<ComponentView> = components.into_iter().map(|t| t.into()).collect();

        Ok(GolemResult::Ok(Box::new(views)))
    }

    async fn get(
        &self,
        component_uri: ComponentUri,
        version: Option<u64>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let urn = self.resolve_uri(component_uri, &project).await?;
        let component = match version {
            Some(v) => self.get_metadata(&urn, v).await?,
            None => self.get_latest_metadata(&urn).await?,
        };
        let view: ComponentView = component.into();
        Ok(GolemResult::Ok(Box::new(ComponentGetView(view))))
    }

    async fn resolve_uri(
        &self,
        uri: ComponentUri,
        project_context: &Option<Self::ProjectContext>,
    ) -> Result<ComponentUrn, GolemError> {
        match uri {
            ComponentUri::URN(urn) => Ok(urn),
            ComponentUri::URL(ComponentUrl { name }) => {
                let components = self
                    .client
                    .find(Some(ComponentName(name.clone())), project_context)
                    .await?;
                let components: Vec<Component> = components
                    .into_iter()
                    .chunk_by(|c| c.versioned_component_id.component_id)
                    .into_iter()
                    .map(|(_, group)| {
                        group
                            .max_by_key(|c| c.versioned_component_id.version)
                            .unwrap()
                    })
                    .collect();

                if components.len() > 1 {
                    let project_msg = match project_context {
                        None => "".to_string(),
                        Some(project) => format!(" in project {project}"),
                    };
                    let ids: Vec<String> = components
                        .into_iter()
                        .map(|c| c.versioned_component_id.component_id.to_string())
                        .collect();
                    Err(GolemError(formatdoc!(
                        "
                        Multiple components found for name {name}{project_msg}:
                        {}
                        Use explicit --component-id
                    ",
                        ids.join(", ")
                    )))
                } else {
                    match components.first() {
                        None => Err(GolemError(format!("Can't find component {name}"))),
                        Some(component) => Ok(ComponentUrn {
                            id: ComponentId(component.versioned_component_id.component_id),
                        }),
                    }
                }
            }
        }
    }

    async fn get_metadata(
        &self,
        urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError> {
        self.client.get_metadata(urn, version).await
    }

    async fn get_latest_metadata(&self, urn: &ComponentUrn) -> Result<Component, GolemError> {
        self.client.get_latest_metadata(urn).await
    }
}
