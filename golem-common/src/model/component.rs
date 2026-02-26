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

use crate::model::diff;
use uuid::Uuid;

pub use crate::base_model::component::*;

impl ComponentDto {
    pub fn to_diffable(&self) -> diff::Component {
        diff::Component {
            metadata: diff::ComponentMetadata {
                version: Some("".to_string()), // TODO: atomic
                env: self
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                config_vars: self
                    .config_vars
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            }
            .into(),
            wasm_hash: self.wasm_hash,
            files_by_path: self
                .files
                .iter()
                .map(|file| {
                    (
                        file.path.to_abs_string(),
                        diff::ComponentFile {
                            hash: file.content_hash.0,
                            permissions: file.permissions,
                        }
                        .into(),
                    )
                })
                .collect(),
            plugins_by_grant_id: self
                .installed_plugins
                .iter()
                .map(|plugin| {
                    (
                        plugin.environment_plugin_grant_id.0,
                        diff::PluginInstallation {
                            priority: plugin.priority.0,
                            name: plugin.plugin_name.clone(),
                            version: plugin.plugin_version.clone(),
                            grant_id: plugin.environment_plugin_grant_id.0,
                            parameters: plugin.parameters.clone(),
                        },
                    )
                })
                .collect(),
        }
    }
}

impl InitialComponentFile {
    pub fn is_read_only(&self) -> bool {
        self.permissions == ComponentFilePermissions::ReadOnly
    }
}

impl From<golem_wasm::ComponentId> for ComponentId {
    fn from(host: golem_wasm::ComponentId) -> Self {
        let high_bits = host.uuid.high_bits;
        let low_bits = host.uuid.low_bits;

        Self(Uuid::from_u64_pair(high_bits, low_bits))
    }
}

impl From<ComponentId> for golem_wasm::ComponentId {
    fn from(component_id: ComponentId) -> Self {
        let (high_bits, low_bits) = component_id.0.as_u64_pair();

        golem_wasm::ComponentId {
            uuid: golem_wasm::Uuid {
                high_bits,
                low_bits,
            },
        }
    }
}

mod protobuf {
    use super::InstalledPlugin;
    use super::{ComponentRevision, PluginPriority};

    impl From<InstalledPlugin> for golem_api_grpc::proto::golem::component::PluginInstallation {
        fn from(value: InstalledPlugin) -> Self {
            Self {
                environment_plugin_grant_id: Some(value.environment_plugin_grant_id.into()),
                priority: value.priority.0,
                parameters: value.parameters.into_iter().collect(),

                plugin_registration_id: Some(value.plugin_registration_id.into()),
                plugin_name: value.plugin_name,
                plugin_version: value.plugin_version,

                oplog_processor_component_id: value.oplog_processor_component_id.map(|v| v.into()),
                oplog_processor_component_revision: value
                    .oplog_processor_component_revision
                    .map(|v| v.0),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginInstallation> for InstalledPlugin {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::component::PluginInstallation,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                environment_plugin_grant_id: value
                    .environment_plugin_grant_id
                    .ok_or("Missing environment_plugin_grant_id")?
                    .try_into()?,
                priority: PluginPriority(value.priority),
                parameters: value.parameters.into_iter().collect(),

                plugin_registration_id: value
                    .plugin_registration_id
                    .ok_or("Missing plugin_registration_id")?
                    .try_into()?,
                plugin_name: value.plugin_name,
                plugin_version: value.plugin_version,

                oplog_processor_component_id: value
                    .oplog_processor_component_id
                    .map(|v| v.try_into())
                    .transpose()?,
                oplog_processor_component_revision: value
                    .oplog_processor_component_revision
                    .map(ComponentRevision),
            })
        }
    }
}
