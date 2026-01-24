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

use crate::model::component_metadata::dynamic_linking_to_diffable;
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
                dynamic_linking_wasm_rpc: dynamic_linking_to_diffable(
                    self.metadata.dynamic_linking(),
                ),
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
    use super::{ComponentDto, InstalledPlugin};
    use super::{ComponentName, ComponentRevision, PluginPriority};
    use applying::Apply;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

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

    impl TryFrom<golem_api_grpc::proto::golem::component::Component> for ComponentDto {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::component::Component,
        ) -> Result<Self, Self::Error> {
            let id = value
                .component_id
                .ok_or("Missing component id")?
                .try_into()
                .map_err(|e| format!("Invalid component id: {}", e))?;

            let revision = ComponentRevision(value.revision);

            let environment_id = value
                .environment_id
                .ok_or("Missing environment id")?
                .try_into()
                .map_err(|e| format!("Invalid environment id: {}", e))?;

            let application_id = value
                .application_id
                .ok_or("Missing application id")?
                .try_into()
                .map_err(|e| format!("Invalid application id: {}", e))?;

            let account_id = value
                .account_id
                .ok_or("Missing account id")?
                .try_into()
                .map_err(|e| format!("Invalid account id: {}", e))?;

            let component_name = ComponentName(value.component_name);
            let component_size = value.component_size;
            let metadata = value
                .metadata
                .ok_or("Missing metadata")?
                .try_into()
                .map_err(|e| format!("Invalid metadata: {}", e))?;

            let created_at = value
                .created_at
                .ok_or("missing created_at")?
                .apply(SystemTime::try_from)
                .map_err(|_| "Failed to convert timestamp".to_string())?
                .into();

            let original_files = value
                .original_files
                .into_iter()
                .map(|f| f.try_into())
                .collect::<Result<Vec<_>, _>>()?;

            let files = value
                .files
                .into_iter()
                .map(|f| f.try_into())
                .collect::<Result<Vec<_>, _>>()?;

            let installed_plugins = value
                .installed_plugins
                .into_iter()
                .map(|p| p.try_into())
                .collect::<Result<Vec<_>, _>>()?;

            let original_env = value.original_env.into_iter().collect::<BTreeMap<_, _>>();

            let env = value.env.into_iter().collect::<BTreeMap<_, _>>();

            let hash = value.hash.ok_or("Missing hash field")?.try_into()?;

            let wasm_hash = value
                .wasm_hash
                .ok_or("Missing wasm hash field")?
                .try_into()?;

            Ok(Self {
                id,
                revision,
                environment_id,
                application_id,
                account_id,
                component_name,
                component_size,
                metadata,
                created_at,
                original_files,
                files,
                installed_plugins,
                original_env,
                env,
                wasm_hash,
                hash,
            })
        }
    }

    impl From<ComponentDto> for golem_api_grpc::proto::golem::component::Component {
        fn from(value: ComponentDto) -> Self {
            Self {
                component_id: Some(value.id.into()),
                revision: value.revision.0,
                component_name: value.component_name.0,
                component_size: value.component_size,
                metadata: Some(value.metadata.into()),
                account_id: Some(value.account_id.into()),
                application_id: Some(value.application_id.into()),
                environment_id: Some(value.environment_id.into()),
                created_at: Some(prost_types::Timestamp::from(SystemTime::from(
                    value.created_at,
                ))),
                original_files: value
                    .original_files
                    .into_iter()
                    .map(|file| file.into())
                    .collect(),
                files: value.files.into_iter().map(|file| file.into()).collect(),
                installed_plugins: value
                    .installed_plugins
                    .into_iter()
                    .map(|plugin| plugin.into())
                    .collect(),
                original_env: value.original_env.into_iter().collect(),
                env: value.env.into_iter().collect(),
                wasm_hash: Some(value.wasm_hash.into()),
                hash: Some(value.hash.into()),
            }
        }
    }
}
