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

use std::sync::Arc;

use crate::api::dto;
use crate::model as domain;
use crate::service::plugin::{PluginError, PluginService};
use futures::{stream, StreamExt, TryStreamExt};
use golem_common::model::component::ComponentOwner;
use golem_common::model::plugin::{PluginInstallation, PluginScope};

#[async_trait::async_trait]
pub trait ApiMapper<Owner: ComponentOwner>: Send + Sync {
    async fn convert_plugin_installation(
        &self,
        owner: &Owner::PluginOwner,
        plugin_installation: PluginInstallation,
    ) -> Result<dto::PluginInstallation, PluginError>;
    async fn convert_component(
        &self,
        component: domain::Component<Owner>,
    ) -> Result<dto::Component, PluginError>;
}

pub struct DefaultApiMapper<Owner: ComponentOwner, Scope: PluginScope> {
    plugin_service: Arc<dyn PluginService<Owner::PluginOwner, Scope>>,
}

impl<Owner: ComponentOwner, Scope: PluginScope> DefaultApiMapper<Owner, Scope> {
    pub fn new(plugin_service: Arc<dyn PluginService<Owner::PluginOwner, Scope>>) -> Self {
        Self { plugin_service }
    }
}

#[async_trait::async_trait]
impl<Owner: ComponentOwner, Scope: PluginScope> ApiMapper<Owner>
    for DefaultApiMapper<Owner, Scope>
{
    async fn convert_plugin_installation(
        &self,
        owner: &Owner::PluginOwner,
        plugin_installation: PluginInstallation,
    ) -> Result<dto::PluginInstallation, PluginError> {
        let definition = self
            .plugin_service
            .get_by_id(owner, &plugin_installation.plugin_id)
            .await?
            .expect("Plugin referenced by id not found");
        Ok(dto::PluginInstallation::from_model(
            plugin_installation,
            definition,
        ))
    }

    async fn convert_component(
        &self,
        component: domain::Component<Owner>,
    ) -> Result<dto::Component, PluginError> {
        let installed_plugins = stream::iter(component.installed_plugins)
            .then(async |p| {
                self.convert_plugin_installation(&component.owner.clone().into(), p)
                    .await
            })
            .try_collect::<Vec<_>>()
            .await?;

        Ok(dto::Component {
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
