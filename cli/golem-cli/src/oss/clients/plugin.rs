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

use crate::clients::plugin::PluginClient;
use crate::model::GolemError;
use crate::oss::model::OssContext;
use async_trait::async_trait;
use golem_common::model::plugin::DefaultPluginScope;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PluginClientLive<C: golem_client::api::PluginClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::PluginClient + Sync + Send> PluginClient for PluginClientLive<C> {
    type ProjectContext = OssContext;
    type PluginDefinition =
        golem_client::model::PluginDefinitionDefaultPluginOwnerDefaultPluginScope;
    type PluginDefinitionWithoutOwner =
        golem_client::model::PluginDefinitionWithoutOwnerDefaultPluginScope;
    type PluginScope = DefaultPluginScope;

    async fn list_plugins(
        &self,
        scope: Option<DefaultPluginScope>,
    ) -> Result<Vec<Self::PluginDefinition>, GolemError> {
        info!("Getting registered plugins");

        Ok(self.client.list_plugins(scope.as_ref()).await?)
    }

    async fn get_plugin(
        &self,
        plugin_name: &str,
        plugin_version: &str,
    ) -> Result<Self::PluginDefinition, GolemError> {
        info!("Getting plugin {} version {}", plugin_name, plugin_version);

        Ok(self.client.get_plugin(plugin_name, plugin_version).await?)
    }

    async fn register_plugin(
        &self,
        definition: Self::PluginDefinitionWithoutOwner,
    ) -> Result<Self::PluginDefinition, GolemError> {
        info!("Registering plugin {}", definition.name);

        let _ = self.client.create_plugin(&definition).await?;
        let definition = self
            .client
            .get_plugin(&definition.name, &definition.version)
            .await?;
        Ok(definition)
    }

    async fn unregister_plugin(
        &self,
        plugin_name: &str,
        plugin_version: &str,
    ) -> Result<(), GolemError> {
        info!(
            "Unregistering plugin {} version {}",
            plugin_name, plugin_version
        );

        let _ = self
            .client
            .delete_plugin(plugin_name, plugin_version)
            .await?;
        Ok(())
    }
}
