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

use golem_common::model::component::{ComponentName, VersionedComponentId};
use golem_common::model::component::{ComponentType, InitialComponentFile};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{PluginId, PluginInstallationId};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PluginInstallation {
    pub id: PluginInstallationId,
    pub plugin_id: PluginId,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

impl From<PluginInstallation> for golem_api_grpc::proto::golem::component::PluginInstallation {
    fn from(plugin_installation: PluginInstallation) -> Self {
        golem_api_grpc::proto::golem::component::PluginInstallation {
            id: Some(plugin_installation.id.into()),
            plugin_id: Some(plugin_installation.plugin_id.into()),
            priority: plugin_installation.priority,
            parameters: plugin_installation.parameters,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PluginInstallation> for PluginInstallation {
    type Error = String;

    fn try_from(
        proto: golem_api_grpc::proto::golem::component::PluginInstallation,
    ) -> Result<Self, Self::Error> {
        Ok(PluginInstallation {
            id: proto.id.ok_or("Missing id")?.try_into()?,
            plugin_id: proto.plugin_id.ok_or("Missing plugin id")?.try_into()?,
            priority: proto.priority,
            parameters: proto.parameters,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Component {
    pub environment_id: EnvironmentId,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
    pub env: HashMap<String, String>,
}

// TODO:
// impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
//     type Error = String;

//     fn try_from(
//         value: golem_api_grpc::proto::golem::component::Component,
//     ) -> Result<Self, Self::Error> {
//         let account_id = value.account_id.ok_or("missing account_id")?.into();

//         let project_id = value
//             .project_id
//             .ok_or("missing project_id")?
//             .try_into()
//             .map_err(|_| "Failed to convert project_id".to_string())?;

//         let created_at = value
//             .created_at
//             .ok_or("missing created_at")?
//             .apply(SystemTime::try_from)
//             .map_err(|_| "Failed to convert timestamp".to_string())?
//             .into();

//         let component_type = value
//             .component_type
//             .ok_or("missing component_type")?
//             .try_into()
//             .map_err(|_| "Failed to convert component_type".to_string())?;

//         let files = value
//             .files
//             .into_iter()
//             .map(|f| f.try_into())
//             .collect::<Result<Vec<_>, _>>()?;

//         let installed_plugins = value
//             .installed_plugins
//             .into_iter()
//             .map(|p| p.try_into())
//             .collect::<Result<Vec<_>, _>>()?;

//         Ok(Self {
//             owner: ComponentOwner {
//                 account_id,
//                 project_id,
//             },
//             versioned_component_id: value
//                 .versioned_component_id
//                 .ok_or("Missing versioned_component_id")?
//                 .try_into()?,
//             component_name: ComponentName(value.component_name.clone()),
//             component_size: value.component_size,
//             metadata: value
//                 .metadata
//                 .clone()
//                 .ok_or("Missing metadata")?
//                 .try_into()?,
//             created_at,
//             component_type,
//             files,
//             installed_plugins,
//             env: value.env,
//         })
//     }
// }
