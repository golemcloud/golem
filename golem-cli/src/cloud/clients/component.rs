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

use std::io::Read;

use crate::clients::component::ComponentClient;
use crate::cloud::model::ProjectId;
use async_trait::async_trait;
use golem_cloud_client::model::ComponentQuery;
use tokio::fs::File;
use tracing::info;

use crate::model::component::Component;
use crate::model::{ComponentId, ComponentName, GolemError, PathBufOrStdin};

#[derive(Debug, Clone)]
pub struct ComponentClientLive<C: golem_cloud_client::api::ComponentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ComponentClient + Sync + Send> ComponentClient
    for ComponentClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn get_metadata(
        &self,
        component_id: &ComponentId,
        version: u64,
    ) -> Result<Component, GolemError> {
        info!("Getting component version");
        let component = self
            .client
            .get_component_metadata(&component_id.0, &version.to_string())
            .await?;
        Ok(component.into())
    }

    async fn get_latest_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<Component, GolemError> {
        info!("Getting latest component version");

        let component = self
            .client
            .get_latest_component_metadata(&component_id.0)
            .await?;
        Ok(component.into())
    }

    async fn find(
        &self,
        name: Option<ComponentName>,
        project: &Option<Self::ProjectContext>,
    ) -> Result<Vec<Component>, GolemError> {
        info!("Getting components");

        let project_id = project.map(|p| p.0);
        let name = name.map(|n| n.0);

        let components = self
            .client
            .get_components(project_id.as_ref(), name.as_deref())
            .await?;
        Ok(components.into_iter().map(|c| c.into()).collect())
    }

    async fn add(
        &self,
        name: ComponentName,
        file: PathBufOrStdin,
        project: &Option<Self::ProjectContext>,
    ) -> Result<Component, GolemError> {
        info!("Adding component {name:?} from {file:?}");

        let query = ComponentQuery {
            project_id: project.map(|ProjectId(id)| id),
            component_name: name.0,
        };

        let component = match file {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client.create_component(&query, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.create_component(&query, bytes).await?
            }
        };

        Ok(component.into())
    }

    async fn update(&self, id: ComponentId, file: PathBufOrStdin) -> Result<Component, GolemError> {
        info!("Updating component {id:?} from {file:?}");

        let component = match file {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client.update_component(&id.0, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.update_component(&id.0, bytes).await?
            }
        };

        Ok(component.into())
    }
}
