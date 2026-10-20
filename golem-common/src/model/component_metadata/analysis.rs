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

use super::*;
use crate::component_introspection::wit_parser::WitAnalysisContext;
use crate::component_introspection::{AnalysisResult, TopLevelExport};

impl ComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
        tool_middlewares: BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata>,
    ) -> Result<Self, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(Self {
            data: Arc::new(raw.into_metadata(
                agent_types,
                agent_type_provision_configs,
                tools,
                tool_middlewares,
            )),
        })
    }

    pub fn analyse_component_with_config(
        data: &[u8],
        config_schema: ComponentConfigSchema,
        component_provision_config: ComponentProvisionConfig,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
        tool_middlewares: BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata>,
    ) -> Result<Self, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        let mut metadata = raw.into_metadata(
            agent_types,
            agent_type_provision_configs,
            tools,
            tool_middlewares,
        );
        metadata.config_schema = config_schema;
        metadata.component_provision_config = component_provision_config;
        Ok(Self {
            data: Arc::new(metadata),
        })
    }
}

impl From<wasmparser::MemoryType> for LinearMemory {
    fn from(value: wasmparser::MemoryType) -> Self {
        Self {
            initial: value.initial * LinearMemory::PAGE_SIZE,
            maximum: value.maximum.map(|m| m * LinearMemory::PAGE_SIZE),
            shared: value.shared,
        }
    }
}

/// Interface name prefixes used to identify known Golem capabilities.
const AGENT_GUEST_PREFIX: &str = "golem:agent/guest";
const SAVE_SNAPSHOT_PREFIX: &str = "golem:api/save-snapshot";
const LOAD_SNAPSHOT_PREFIX: &str = "golem:api/load-snapshot";
const OPLOG_PROCESSOR_PREFIX: &str = "golem:api/oplog-processor";
const TOOL_GUEST_PREFIX: &str = "golem:tool/guest";
const TOOL_MIDDLEWARE_GUEST_PREFIX: &str = "golem:tool/tool-middleware-guest";

fn record_known_export(
    slot: &mut Option<String>,
    capability_name: &str,
    export_name: &str,
) -> AnalysisResult<()> {
    match slot {
        Some(previous) => Err(AnalysisFailure::failed(format!(
            "Duplicate {capability_name} export: found both {previous} and {export_name}"
        ))),
        None => {
            *slot = Some(export_name.to_string());
            Ok(())
        }
    }
}

/// Extract a `KnownExports` index from the top-level exports.
/// Only instance exports are considered; each supported interface prefix
/// is matched at most once (the exact versioned name is stored).
pub fn extract_known_exports(exports: &[TopLevelExport]) -> AnalysisResult<KnownExports> {
    let mut known = KnownExports::default();

    for export in exports {
        if let TopLevelExport::Instance(instance) = export {
            let name = &instance.name;
            if name == AGENT_GUEST_PREFIX || name.starts_with(&format!("{AGENT_GUEST_PREFIX}@")) {
                record_known_export(&mut known.agent_guest_interface, "agent guest", name)?;
            } else if name == SAVE_SNAPSHOT_PREFIX
                || name.starts_with(&format!("{SAVE_SNAPSHOT_PREFIX}@"))
            {
                record_known_export(&mut known.save_snapshot_interface, "save snapshot", name)?;
            } else if name == LOAD_SNAPSHOT_PREFIX
                || name.starts_with(&format!("{LOAD_SNAPSHOT_PREFIX}@"))
            {
                record_known_export(&mut known.load_snapshot_interface, "load snapshot", name)?;
            } else if name == OPLOG_PROCESSOR_PREFIX
                || name.starts_with(&format!("{OPLOG_PROCESSOR_PREFIX}@"))
            {
                record_known_export(
                    &mut known.oplog_processor_interface,
                    "oplog processor",
                    name,
                )?;
            } else if name == TOOL_GUEST_PREFIX
                || name.starts_with(&format!("{TOOL_GUEST_PREFIX}@"))
            {
                record_known_export(&mut known.tool_guest_interface, "tool guest", name)?;
            } else if name == TOOL_MIDDLEWARE_GUEST_PREFIX
                || name.starts_with(&format!("{TOOL_MIDDLEWARE_GUEST_PREFIX}@"))
            {
                record_known_export(
                    &mut known.tool_middleware_guest_interface,
                    "tool middleware guest",
                    name,
                )?;
            }
        }
    }

    Ok(known)
}

impl RawComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
    ) -> Result<RawComponentMetadata, ComponentProcessingError> {
        let wit_analysis =
            WitAnalysisContext::new(data).map_err(ComponentProcessingError::Analysis)?;

        let exports = wit_analysis
            .get_top_level_exports()
            .map_err(ComponentProcessingError::Analysis)?;
        let root_package = wit_analysis.root_package_name();

        for warning in wit_analysis.warnings() {
            tracing::warn!("Wit analysis warning: {}", warning);
        }

        let known_exports =
            extract_known_exports(&exports).map_err(ComponentProcessingError::Analysis)?;

        let memories = wit_analysis
            .linear_memories()
            .iter()
            .cloned()
            .map(LinearMemory::from)
            .collect();

        let producers = wit_analysis.producers().to_vec();

        Ok(RawComponentMetadata {
            known_exports,
            producers,
            memories,
            root_package_name: root_package
                .as_ref()
                .map(|pkg| format!("{}:{}", pkg.namespace, pkg.name)),
            root_package_version: root_package.and_then(|pkg| pkg.version.map(|v| v.to_string())),
        })
    }

    pub fn into_metadata(
        self,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
        tool_middlewares: BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata>,
    ) -> ComponentMetadataInnerData {
        let producers = self
            .producers
            .into_iter()
            .map(|producers| producers.into())
            .collect::<Vec<_>>();

        let memories = self.memories.into_iter().collect();

        ComponentMetadataInnerData {
            known_exports: self.known_exports,
            producers,
            memories,
            root_package_name: self.root_package_name,
            root_package_version: self.root_package_version,
            config_schema: ComponentConfigSchema::default(),
            component_provision_config: ComponentProvisionConfig::default(),
            agent_method_streams: ComponentMetadata::derive_agent_method_streams(&agent_types),
            agent_types,
            agent_type_provision_configs,
            tools,
            tool_middlewares,
        }
    }
}

impl From<crate::component_introspection::metadata::Producers> for Producers {
    fn from(value: crate::component_introspection::metadata::Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<Producers> for crate::component_introspection::metadata::Producers {
    fn from(value: Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<crate::component_introspection::metadata::ProducersField> for ProducerField {
    fn from(value: crate::component_introspection::metadata::ProducersField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(|value| VersionedName {
                    name: value.name,
                    version: value.version,
                })
                .collect(),
        }
    }
}

impl From<ProducerField> for crate::component_introspection::metadata::ProducersField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(
                    |value| crate::component_introspection::metadata::VersionedName {
                        name: value.name,
                        version: value.version,
                    },
                )
                .collect(),
        }
    }
}
