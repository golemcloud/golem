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

use std::collections::HashMap;
use std::io::Read;

use async_trait::async_trait;
use golem_client::model::ComponentFilePathWithPermissionsList;

use crate::clients::component::ComponentClient;
use crate::model::component::Component;
use crate::model::{ComponentName, GolemError, PathBufOrStdin};
use crate::oss::model::OssContext;
use golem_client::model::{PluginInstallation, PluginInstallationCreation};
use golem_common::uri::oss::urn::ComponentUrn;
use std::path::Path;
use tokio::fs::File;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ComponentClientLive<C: golem_client::api::ComponentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ComponentClient + Sync + Send> ComponentClient
    for ComponentClientLive<C>
{
    type ProjectContext = OssContext;

    async fn get_metadata(
        &self,
        component_urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError> {
        info!("Getting component version");

        Ok(self
            .client
            .get_component_metadata(&component_urn.id.0, &version.to_string())
            .await?
            .into())
    }

    async fn get_latest_metadata(
        &self,
        component_urn: &ComponentUrn,
    ) -> Result<Component, GolemError> {
        info!("Getting latest component version");

        Ok(self
            .client
            .get_latest_component_metadata(&component_urn.id.0)
            .await?
            .into())
    }

    async fn find(
        &self,
        name: Option<ComponentName>,
        _project: &Option<Self::ProjectContext>,
    ) -> Result<Vec<Component>, GolemError> {
        info!("Getting components");

        let name = name.map(|n| n.0);

        let components = self.client.get_components(name.as_deref()).await?;
        Ok(components.into_iter().map(|c| c.into()).collect())
    }

    async fn add(
        &self,
        name: ComponentName,
        path: PathBufOrStdin,
        _project: &Option<Self::ProjectContext>,
        component_type: golem_client::model::ComponentType,
        files_archive: Option<&Path>,
        files_permissions: Option<&ComponentFilePathWithPermissionsList>,
    ) -> Result<Component, GolemError> {
        info!("Adding component {name:?} from {path:?}");

        let files_archive_file = match files_archive {
            Some(fa) => {
                let file = File::open(fa)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component files archive: {e}")))?;
                Some(file)
            }
            None => None,
        };

        let component = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client
                    .create_component(
                        &name.0,
                        Some(&component_type),
                        file,
                        files_permissions,
                        files_archive_file,
                    )
                    .await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client
                    .create_component(
                        &name.0,
                        Some(&component_type),
                        bytes,
                        files_permissions,
                        files_archive_file,
                    )
                    .await?
            }
        };

        Ok(component.into())
    }

    async fn update(
        &self,
        urn: ComponentUrn,
        path: PathBufOrStdin,
        component_type: Option<golem_client::model::ComponentType>,
        files_archive: Option<&Path>,
        files_permissions: Option<&ComponentFilePathWithPermissionsList>,
    ) -> Result<Component, GolemError> {
        info!("Updating component {urn} from {path:?}");

        let files_archive_file = match files_archive {
            Some(fa) => {
                let file = File::open(fa)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component files archive: {e}")))?;
                Some(file)
            }
            None => None,
        };

        let component = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client
                    .update_component(
                        &urn.id.0,
                        component_type.as_ref(),
                        file,
                        files_permissions,
                        files_archive_file,
                    )
                    .await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes)
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client
                    .update_component(
                        &urn.id.0,
                        component_type.as_ref(),
                        bytes,
                        files_permissions,
                        files_archive_file,
                    )
                    .await?
            }
        };

        Ok(component.into())
    }

    async fn install_plugin(
        &self,
        urn: &ComponentUrn,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> Result<PluginInstallation, GolemError> {
        info!("Installing plugin {plugin_name} version {plugin_version} to {urn}");

        Ok(self
            .client
            .install_plugin(
                &urn.id.0,
                &PluginInstallationCreation {
                    name: plugin_name.to_string(),
                    version: plugin_version.to_string(),
                    priority,
                    parameters,
                },
            )
            .await?)
    }

    async fn get_installations(
        &self,
        urn: &ComponentUrn,
        version: u64,
    ) -> Result<Vec<PluginInstallation>, GolemError> {
        info!("Getting plugin installations for {urn} version {version}");

        Ok(self
            .client
            .get_installed_plugins(&urn.id.0, &version.to_string())
            .await?)
    }

    async fn uninstall_plugin(
        &self,
        urn: &ComponentUrn,
        installation_id: &Uuid,
    ) -> Result<(), GolemError> {
        info!("Uninstalling plugin {installation_id} from {urn}");

        self.client
            .uninstall_plugin(&urn.id.0, installation_id)
            .await?;

        Ok(())
    }
}
