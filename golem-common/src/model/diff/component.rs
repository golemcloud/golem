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

use crate::model::diff::DiffError;
use crate::model::diff::agent::AgentTypeProvisionConfig;
use crate::model::diff::hash::{Hash, HashOf, Hashable, hash_from_serialized_value};
use crate::model::diff::plugin::PluginInstallation;
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use crate::model::json::NormalizedJsonValue;
use crate::model::tool::ToolBindingInput;
use crate::schema::tool::Tool;
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Top-level diffable component state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub wasm_hash: Hash,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub agent_type_provision_configs: BTreeMap<String, HashOf<AgentTypeProvisionConfig>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub tool_deployment_configs: BTreeMap<String, HashOf<ToolDeploymentConfig>>,
}

/// Top-level component diff result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentDiff {
    pub wasm_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_type_provision_config_changes: BTreeMapDiff<String, HashOf<AgentTypeProvisionConfig>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_deployment_config_changes: BTreeMapDiff<String, HashOf<ToolDeploymentConfig>>,
}

impl Diffable for Component {
    type DiffResult = ComponentDiff;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        let wasm_changed = new.wasm_hash != current.wasm_hash;
        let agent_type_provision_config_changes = new
            .agent_type_provision_configs
            .diff_with_current(&current.agent_type_provision_configs)?
            .unwrap_or_default();
        let tool_deployment_config_changes = new
            .tool_deployment_configs
            .diff_with_current(&current.tool_deployment_configs)?
            .unwrap_or_default();

        Ok(
            if wasm_changed
                || !agent_type_provision_config_changes.is_empty()
                || !tool_deployment_config_changes.is_empty()
            {
                Some(ComponentDiff {
                    wasm_changed,
                    agent_type_provision_config_changes,
                    tool_deployment_config_changes,
                })
            } else {
                None
            },
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDeploymentConfig {
    pub definition: Tool,
    pub config: NormalizedJsonValue,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub files_by_path: BTreeMap<String, HashOf<crate::model::diff::AgentFile>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins_by_grant_id: BTreeMap<Uuid, PluginInstallation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_binding: Option<ToolBindingInput>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_bindings: BTreeMap<String, ToolBindingInput>,
}

impl Hashable for ToolDeploymentConfig {
    fn hash(&self) -> Result<Hash, DiffError> {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDeploymentConfigDiff {
    pub definition_changed: bool,
    pub config_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env_changes: BTreeMapDiff<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub file_changes: BTreeMapDiff<String, HashOf<crate::model::diff::AgentFile>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugin_changes: BTreeMapDiff<Uuid, PluginInstallation>,
    pub environment_binding_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_binding_changes: BTreeMapDiff<String, ToolBindingInput>,
}

impl Diffable for ToolDeploymentConfig {
    type DiffResult = ToolDeploymentConfigDiff;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        let definition_changed = new.definition != current.definition;
        let config_changed = new.config != current.config;
        let env_changes = new.env.diff_with_current(&current.env)?.unwrap_or_default();
        let file_changes = new
            .files_by_path
            .diff_with_current(&current.files_by_path)?
            .unwrap_or_default();
        let plugin_changes = new
            .plugins_by_grant_id
            .diff_with_current(&current.plugins_by_grant_id)?
            .unwrap_or_default();
        let environment_binding_changed = new.environment_binding != current.environment_binding;
        let agent_binding_changes = new
            .agent_bindings
            .diff_with_current(&current.agent_bindings)?
            .unwrap_or_default();

        Ok(
            if definition_changed
                || config_changed
                || !env_changes.is_empty()
                || !file_changes.is_empty()
                || !plugin_changes.is_empty()
                || environment_binding_changed
                || !agent_binding_changes.is_empty()
            {
                Some(ToolDeploymentConfigDiff {
                    definition_changed,
                    config_changed,
                    env_changes,
                    file_changes,
                    plugin_changes,
                    environment_binding_changed,
                    agent_binding_changes,
                })
            } else {
                None
            },
        )
    }
}

impl Diffable for ToolBindingInput {
    type DiffResult = ToolBindingInput;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        Ok((new != current).then(|| new.clone()))
    }
}

impl Hashable for Component {
    fn hash(&self) -> Result<Hash, DiffError> {
        hash_from_serialized_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{Component, ToolDeploymentConfig};
    use crate::model::diff::hash::{Hash, Hashable};
    use crate::model::diff::{BTreeMapDiffValue, DiffForHashOf, Diffable};
    use crate::model::json::NormalizedJsonValue;
    use crate::model::tool::ToolBindingInput;
    use crate::schema::SchemaGraph;
    use crate::schema::tool::{CommandTree, Tool};
    use std::collections::BTreeMap;
    use test_r::test;

    fn tool_config(environment_binding: Option<ToolBindingInput>) -> ToolDeploymentConfig {
        ToolDeploymentConfig {
            definition: Tool {
                version: "1.0.0".to_string(),
                commands: CommandTree { nodes: Vec::new() },
                schema: SchemaGraph::empty(),
            },
            config: NormalizedJsonValue::new(serde_json::json!({})),
            env: BTreeMap::new(),
            files_by_path: BTreeMap::new(),
            plugins_by_grant_id: BTreeMap::new(),
            environment_binding,
            agent_bindings: BTreeMap::new(),
        }
    }

    fn component(tool_config: ToolDeploymentConfig) -> Component {
        Component {
            wasm_hash: Hash::empty(),
            agent_type_provision_configs: BTreeMap::new(),
            tool_deployment_configs: BTreeMap::from([("grep".to_string(), tool_config.into())]),
        }
    }

    #[test]
    fn binding_only_tool_change_changes_component_hash_and_produces_value_diff() {
        let current = component(tool_config(None));
        let new = component(tool_config(Some(ToolBindingInput::default())));

        assert_ne!(current.hash().unwrap(), new.hash().unwrap());
        let diff = new.diff_with_current(&current).unwrap().unwrap();
        assert!(!diff.wasm_changed);
        assert!(matches!(
            diff.tool_deployment_config_changes.get("grep"),
            Some(BTreeMapDiffValue::Update(DiffForHashOf::ValueDiff { diff }))
                if diff.environment_binding_changed && !diff.definition_changed
        ));
    }

    #[test]
    fn unchanged_tool_state_preserves_component_no_op() {
        let current = component(tool_config(Some(ToolBindingInput::default())));
        let new = component(tool_config(Some(ToolBindingInput::default())));

        assert!(new.diff_with_current(&current).unwrap().is_none());
    }
}
