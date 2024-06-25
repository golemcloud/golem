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
use crate::model::{
    ComponentId, ComponentIdOrName, ComponentName, GolemError, GolemResult, PathBufOrStdin,
};
use async_trait::async_trait;
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
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn update(
        &self,
        component_id_or_name: ComponentIdOrName,
        component_file: PathBufOrStdin,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn list(
        &self,
        component_name: Option<ComponentName>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn get(
        &self,
        component_id_or_name: ComponentIdOrName,
        version: Option<u64>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn resolve_id(
        &self,
        reference: ComponentIdOrName,
        project: Option<Self::ProjectContext>,
    ) -> Result<ComponentId, GolemError>;
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
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component = self
            .client
            .add(component_name, component_file, &project)
            .await?;
        let view: ComponentView = component.into();

        Ok(GolemResult::Ok(Box::new(ComponentAddView(view))))
    }

    async fn update(
        &self,
        component_id_or_name: ComponentIdOrName,
        component_file: PathBufOrStdin,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let id = self.resolve_id(component_id_or_name, project).await?;
        let component = self.client.update(id, component_file).await?;
        let view: ComponentView = component.into();

        Ok(GolemResult::Ok(Box::new(ComponentUpdateView(view))))
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
        component_id_or_name: ComponentIdOrName,
        version: Option<u64>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self.resolve_id(component_id_or_name, project).await?;
        let component = match version {
            Some(v) => self.get_metadata(&component_id, v).await?,
            None => self.get_latest_metadata(&component_id).await?,
        };
        let view: ComponentView = component.into();
        Ok(GolemResult::Ok(Box::new(ComponentGetView(view))))
    }

    async fn resolve_id(
        &self,
        reference: ComponentIdOrName,
        project_context: Option<Self::ProjectContext>,
    ) -> Result<ComponentId, GolemError> {
        match reference {
            ComponentIdOrName::Id(id) => Ok(id),
            ComponentIdOrName::Name(name) => {
                let components = self
                    .client
                    .find(Some(name.clone()), &project_context)
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
                    let component_name = name.0;
                    let ids: Vec<String> = components
                        .into_iter()
                        .map(|c| c.versioned_component_id.component_id.to_string())
                        .collect();
                    Err(GolemError(formatdoc!(
                        "
                        Multiple components found for name {component_name}{project_msg}:
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
