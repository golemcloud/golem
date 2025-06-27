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

use crate::api::dto;
use crate::error::ComponentError;
use crate::model as domain;
use crate::service::plugin::PluginService;
use futures::{stream, StreamExt, TryStreamExt};
use golem_common::model::plugin::PluginInstallation;
use std::sync::Arc;

pub struct ApiMapper {
    plugin_service: Arc<PluginService>,
}

impl ApiMapper {
    pub fn new(plugin_service: Arc<PluginService>) -> Self {
        Self { plugin_service }
    }
}

impl ApiMapper {
    pub async fn convert_plugin_installation(
        &self,
        plugin_installation: PluginInstallation,
    ) -> Result<dto::PluginInstallation, ComponentError> {
        let definition = self
            .plugin_service
            .get_by_id(&plugin_installation.plugin_id)
            .await?
            .expect("Plugin referenced by id not found");
        Ok(dto::PluginInstallation::from_model(
            plugin_installation,
            definition,
        ))
    }

    pub async fn convert_component(
        &self,
        component: domain::Component,
    ) -> Result<dto::Component, ComponentError> {
        let installed_plugins = stream::iter(component.installed_plugins)
            .then(async |p| self.convert_plugin_installation(p).await)
            .try_collect::<Vec<_>>()
            .await?;

        Ok(dto::Component {
            versioned_component_id: component.versioned_component_id,
            component_name: component.component_name,
            component_size: component.component_size,
            account_id: component.owner.account_id,
            project_id: component.owner.project_id,
            metadata: component.metadata,
            created_at: component.created_at,
            component_type: component.component_type,
            files: component.files,
            installed_plugins,
            env: component.env,
        })
    }
}
