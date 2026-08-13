// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::base_model::json::NormalizedJsonValue;
use crate::model::diff;

pub use crate::base_model::component::*;
pub use crate::base_model::path::{AgentFilePath, ArchiveFilePath, CanonicalFilePath};
pub use crate::base_model::worker::AgentConfigEntryDto;

impl ComponentDto {
    pub fn to_diffable(&self) -> Result<diff::Component, diff::DiffError> {
        let agent_type_provision_configs =
            self.metadata
                .agent_type_provision_configs()
                .iter()
                .map(|(name, config)| {
                    let state =
                        diff::AgentTypeProvisionConfig {
                            env: config.env.clone(),
                            config: config
                                .config
                                .iter()
                                .map(|e| {
                                    Ok((
                                        e.path.join("."),
                                        NormalizedJsonValue::new(
                                            crate::schema::render::to_json_value(
                                                e.value.graph(),
                                                e.value.root_type(),
                                                e.value.value(),
                                            )
                                            .map_err(|reason| {
                                                diff::DiffError::TypedConfigJsonConversion {
                                                    operation:
                                                        "component dto to_diffable config entry conversion",
                                                    path: e.path.join("."),
                                                    reason: reason.to_string(),
                                                }
                                            })?,
                                        ),
                                    ))
                                })
                                .collect::<Result<_, _>>()?,
                            files_by_path: config
                                .files
                                .iter()
                                .map(|file| {
                                    (
                                        file.path.to_abs_string(),
                                        diff::AgentFile {
                                            hash: file.content_hash.0,
                                            permissions: file.permissions,
                                        }
                                        .into(),
                                    )
                                })
                                .collect(),
                            plugins_by_grant_id: config
                                .plugins
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
                            initial_permissions: self
                                .metadata
                                .agent_type_initial_permission_card(name)
                                .map(|card| diff::AgentTypeInitialPermission {
                                    lower_positive: card.lower_positive.clone(),
                                    lower_negative: card.lower_negative.clone(),
                                    upper_positive: card.upper_positive.clone(),
                                    upper_negative: card.upper_negative.clone(),
                                })
                                .unwrap_or_else(|| diff::AgentTypeInitialPermission {
                                    lower_positive: Vec::new(),
                                    lower_negative: Vec::new(),
                                    upper_positive: Vec::new(),
                                    upper_negative: Vec::new(),
                                }),
                        };
                    Ok((name.0.clone(), state.into()))
                })
                .collect::<Result<_, _>>()?;

        let tool_deployment_configs = self
            .metadata
            .tools()
            .iter()
            .map(|(name, metadata)| {
                (
                    name.as_str().to_string(),
                    diff::ToolDeploymentConfig {
                        definition: metadata.definition.clone(),
                        config: metadata.provision.config.clone(),
                        env: metadata.provision.env.clone(),
                        files_by_path: metadata
                            .provision
                            .files
                            .iter()
                            .map(|file| {
                                (
                                    file.path.to_abs_string(),
                                    diff::AgentFile {
                                        hash: file.content_hash.0,
                                        permissions: file.permissions,
                                    }
                                    .into(),
                                )
                            })
                            .collect(),
                        plugins_by_grant_id: metadata
                            .provision
                            .plugins
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
                        environment_binding: metadata.environment_binding.clone(),
                        agent_bindings: metadata
                            .agent_bindings
                            .iter()
                            .map(|(agent_name, binding)| (agent_name.0.clone(), binding.clone()))
                            .collect(),
                    }
                    .into(),
                )
            })
            .collect();

        Ok(diff::Component {
            wasm_hash: self.wasm_hash,
            agent_type_provision_configs,
            tool_deployment_configs,
        })
    }
}

impl InitialAgentFile {
    pub fn is_read_only(&self) -> bool {
        self.permissions == AgentFilePermissions::ReadOnly
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentDto, ComponentId, ComponentName, ComponentRevision};
    use crate::model::account::AccountId;
    use crate::model::application::ApplicationId;
    use crate::model::component_metadata::{ComponentMetadata, KnownExports};
    use crate::model::diff;
    use crate::model::diff::Hashable;
    use crate::model::environment::EnvironmentId;
    use crate::model::json::NormalizedJsonValue;
    use crate::model::tool::{ToolDeploymentMetadata, ToolName, ToolProvisionConfig};
    use crate::schema::SchemaGraph;
    use crate::schema::tool::{CommandTree, Tool};
    use std::collections::BTreeMap;
    use test_r::test;

    #[test]
    fn component_dto_tool_state_uses_the_same_canonical_diff_hash_shape() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let definition = Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree { nodes: Vec::new() },
            schema: SchemaGraph::empty(),
        };
        let config = NormalizedJsonValue::new(serde_json::json!({"root": "/workspace"}));
        let metadata = ComponentMetadata::from_parts_with_tools(
            KnownExports::default(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            BTreeMap::new(),
            BTreeMap::from([(
                tool_name.clone(),
                ToolDeploymentMetadata {
                    definition: definition.clone(),
                    provision: ToolProvisionConfig {
                        config: config.clone(),
                        env: BTreeMap::from([("RUST_LOG".to_string(), "info".to_string())]),
                        plugins: Vec::new(),
                        files: Vec::new(),
                    },
                    environment_binding: None,
                    agent_bindings: BTreeMap::new(),
                },
            )]),
        );
        let wasm_hash = diff::Hash::empty();
        let dto = ComponentDto {
            id: ComponentId::new(),
            revision: ComponentRevision::new(1).unwrap(),
            environment_id: EnvironmentId::new(),
            component_name: ComponentName("app:main".to_string()),
            hash: diff::Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: AccountId::new(),
            component_size: 0,
            metadata,
            created_at: chrono::Utc::now(),
            wasm_hash,
        };
        let expected = diff::Component {
            wasm_hash,
            agent_type_provision_configs: BTreeMap::new(),
            tool_deployment_configs: BTreeMap::from([(
                tool_name.as_str().to_string(),
                diff::ToolDeploymentConfig {
                    definition,
                    config,
                    env: BTreeMap::from([("RUST_LOG".to_string(), "info".to_string())]),
                    files_by_path: BTreeMap::new(),
                    plugins_by_grant_id: BTreeMap::new(),
                    environment_binding: None,
                    agent_bindings: BTreeMap::new(),
                }
                .into(),
            )]),
        };

        assert_eq!(
            dto.to_diffable().unwrap().hash().unwrap(),
            expected.hash().unwrap()
        );
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
