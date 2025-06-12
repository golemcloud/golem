// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use futures_util::{stream, StreamExt, TryStreamExt};
use golem_common::model::auth::TokenSecret;
use golem_common::model::plugin::PluginInstallation;
use golem_service_base::clients::plugin::{PluginError, PluginServiceClient};
use golem_service_base::dto;
use golem_service_base::model::Component;
use std::sync::Arc;

pub struct RemoteCloudApiMapper {
    plugin_service_client: Arc<dyn PluginServiceClient + Sync + Send>,
}

impl RemoteCloudApiMapper {
    pub fn new(plugin_service_client: Arc<dyn PluginServiceClient + Sync + Send>) -> Self {
        Self {
            plugin_service_client,
        }
    }

    pub async fn convert_plugin_installation(
        &self,
        token: &TokenSecret,
        plugin_installation: PluginInstallation,
    ) -> Result<dto::PluginInstallation, PluginError> {
        let definition = self
            .plugin_service_client
            .get_by_id(&plugin_installation.plugin_id, token)
            .await?
            .expect("Plugin referenced by id not found");
        Ok(dto::PluginInstallation::from_model(
            plugin_installation,
            definition,
        ))
    }

    pub async fn convert_component(
        &self,
        token: &TokenSecret,
        component: Component,
    ) -> Result<dto::Component, PluginError> {
        let installed_plugins = stream::iter(component.installed_plugins)
            .then(async |p| self.convert_plugin_installation(token, p).await)
            .try_collect::<Vec<_>>()
            .await?;

        Ok(dto::Component {
            account_id: component.owner.account_id,
            project_id: component.owner.project_id,
            versioned_component_id: component.versioned_component_id,
            component_name: component.component_name,
            component_size: component.component_size,
            metadata: component.metadata,
            created_at: component.created_at,
            component_type: component.component_type,
            files: component.files,
            installed_plugins,
            env: component.env,
        })
    }
}
