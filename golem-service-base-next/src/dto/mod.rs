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

use crate::model::ComponentName;
use golem_common_next::model::component::VersionedComponentId;
use golem_common_next::model::component_metadata::ComponentMetadata;
use golem_common_next::model::{
    plugin as common_plugin_model, AccountId, ComponentType, InitialComponentFile,
    PluginInstallationId, ProjectId,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallation {
    pub id: PluginInstallationId,
    pub plugin_name: String,
    pub plugin_version: String,
    /// Whether the referenced plugin is still registered. If false, the installation will still work but the plugin will not show up when listing plugins.
    pub plugin_registered: bool,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

impl PluginInstallation {
    pub fn from_model(
        model: common_plugin_model::PluginInstallation,
        plugin_definition: common_plugin_model::PluginDefinition,
    ) -> Self {
        Self {
            id: model.id,
            plugin_name: plugin_definition.name,
            plugin_version: plugin_definition.version,
            plugin_registered: !plugin_definition.deleted,
            priority: model.priority,
            parameters: model.parameters,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub account_id: AccountId,
    pub project_id: ProjectId,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
    pub env: HashMap<String, String>,
}
