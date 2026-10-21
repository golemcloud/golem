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

use super::{
    AgentConfigSource, AgentMetadataDto, AgentTypeName, OwnerKind, TypedAgentConfigEntry,
    UntypedAgentConfigEntry,
};
use crate::base_model::render_config_path;
use crate::model::component_metadata::ComponentMetadata;
use std::collections::BTreeMap;

impl TypedAgentConfigEntry {
    fn render_path(path: &[String]) -> String {
        render_config_path(path)
    }

    pub fn to_flat_pair(&self) -> Option<(String, String)> {
        golem_schema::schema::render::to_json_value(
            self.value.graph(),
            self.value.root_type(),
            self.value.value(),
        )
        .ok()
        .map(|json| {
            let rendered_value = match json {
                serde_json::Value::String(value) => value,
                other => other.to_string(),
            };

            (Self::render_path(&self.path), rendered_value)
        })
    }

    pub fn to_flat_map(entries: &[TypedAgentConfigEntry]) -> BTreeMap<String, String> {
        entries
            .iter()
            .filter_map(TypedAgentConfigEntry::to_flat_pair)
            .collect()
    }
}

impl AgentMetadataDto {
    pub fn redact_host_managed_values_for_external(&mut self) {
        for entry in &mut self.config {
            entry.value = crate::schema::redact_host_managed_typed_value(entry.value.clone());
        }
    }
}

impl UntypedAgentConfigEntry {
    pub fn enrich_with_type(
        self,
        component_metadata: &ComponentMetadata,
        owner_kind: OwnerKind,
        agent_type_name: Option<&AgentTypeName>,
    ) -> Result<TypedAgentConfigEntry, String> {
        let (declarations, schema) = if owner_kind == OwnerKind::ComponentAgent {
            let agent_type_name = agent_type_name
                .ok_or_else(|| "Real agent config requires an agent type".to_string())?;
            let agent_type = component_metadata
                .find_agent_type_by_name_ref(agent_type_name)
                .ok_or_else(|| {
                    format!("did not find expected agent type {agent_type_name} in the metadata")
                })?;
            (&agent_type.config, &agent_type.schema)
        } else {
            let config_schema = component_metadata.config_schema();
            (&config_schema.declarations, &config_schema.schema)
        };

        let declaration = declarations
            .iter()
            .find(|c| c.source == AgentConfigSource::Local && c.path == self.path)
            .ok_or_else(|| {
                format!(
                    "did not find config key {} in the metadata",
                    self.path.join(".")
                )
            })?;

        // Reattach the owner's native config schema graph (so named/ref
        // composites are preserved) to the stored schema value. Refs in
        // `value_type` resolve against the owner's `SchemaGraph`; the resulting
        // carrier is single-root, so project the defs to exactly those reachable
        // from `value_type` instead of cloning the whole registry.
        let value = crate::schema::agent::typed_schema_value_with_projected_defs(
            schema,
            declaration.value_type.clone(),
            self.value,
        );

        Ok(TypedAgentConfigEntry {
            path: self.path,
            value,
        })
    }
}

#[cfg(test)]
mod tests;
