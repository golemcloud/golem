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

pub use super::parsed_function_name::{
    ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, SemVer,
};
use crate::SafeDisplay;
use crate::base_model::component::InitialAgentFile;
pub use crate::base_model::component_metadata::*;
use crate::base_model::worker::TypedAgentConfigEntry;
use crate::model::agent::{AgentType, AgentTypeName};
use crate::model::component::InstalledPlugin;
use golem_wasm::analysis::wit_parser::WitAnalysisContext;
use golem_wasm::analysis::{AnalysedExport, AnalysisFailure, AnalysisResult};
use golem_wasm::metadata::Producers as WasmAstProducers;
use std::collections::BTreeMap;
use std::fmt::{self, Debug, Display, Formatter};
use std::sync::Arc;
use wasmtime::component::__internal::wasmtime_environ::wasmparser;

impl ComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
        agent_types: Vec<AgentType>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    ) -> Result<Self, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(Self {
            data: Arc::new(raw.into_metadata(agent_types, agent_type_provision_configs)),
        })
    }

    pub fn from_parts(
        known_exports: KnownExports,
        memories: Vec<LinearMemory>,
        root_package_name: Option<String>,
        root_package_version: Option<String>,
        agent_types: Vec<AgentType>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    ) -> Self {
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                known_exports,
                producers: vec![],
                memories,
                root_package_name,
                root_package_version,
                agent_types,
                agent_type_provision_configs,
            }),
        }
    }

    /// Returns a new `ComponentMetadata` with the provision configs replaced.
    /// All other analysed fields (known_exports, memories, agent types, etc.) are preserved.
    /// Use this when provision configs are updated independently of the WASM binary.
    pub fn with_provision_configs(
        &self,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    ) -> Self {
        let data = self.data.as_ref();
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                known_exports: data.known_exports.clone(),
                producers: data.producers.clone(),
                memories: data.memories.clone(),
                root_package_name: data.root_package_name.clone(),
                root_package_version: data.root_package_version.clone(),
                agent_types: data.agent_types.clone(),
                agent_type_provision_configs,
            }),
        }
    }

    pub fn producers(&self) -> &[Producers] {
        &self.data.producers
    }

    pub fn memories(&self) -> &[LinearMemory] {
        &self.data.memories
    }

    pub fn known_exports(&self) -> &KnownExports {
        &self.data.known_exports
    }

    pub fn root_package_name(&self) -> &Option<String> {
        &self.data.root_package_name
    }

    pub fn root_package_version(&self) -> &Option<String> {
        &self.data.root_package_version
    }

    pub fn agent_types(&self) -> &[AgentType] {
        &self.data.agent_types
    }

    pub fn agent_type_provision_configs(
        &self,
    ) -> &BTreeMap<AgentTypeName, AgentTypeProvisionConfig> {
        &self.data.agent_type_provision_configs
    }

    pub fn agent_type_provision_config(
        &self,
        name: &AgentTypeName,
    ) -> Option<&AgentTypeProvisionConfig> {
        self.data.agent_type_provision_configs.get(name)
    }

    pub fn agent_type_env(&self, name: &AgentTypeName) -> Option<&BTreeMap<String, String>> {
        self.agent_type_provision_config(name)
            .map(|config| &config.env)
    }

    pub fn agent_type_config(&self, name: &AgentTypeName) -> Option<&[TypedAgentConfigEntry]> {
        self.agent_type_provision_config(name)
            .map(|config| config.config.as_slice())
    }

    pub fn agent_type_wasi_config(
        &self,
        name: &AgentTypeName,
    ) -> Option<&BTreeMap<String, String>> {
        self.agent_type_provision_config(name)
            .map(|config| &config.wasi_config)
    }

    pub fn agent_type_files(&self, name: &AgentTypeName) -> Option<&[InitialAgentFile]> {
        self.agent_type_provision_config(name)
            .map(|config| config.files.as_slice())
    }

    pub fn agent_type_plugins(&self, name: &AgentTypeName) -> Option<&[InstalledPlugin]> {
        self.agent_type_provision_config(name)
            .map(|config| config.plugins.as_slice())
    }

    pub fn is_agent(&self) -> bool {
        !self.data.agent_types.is_empty()
    }

    pub fn has_load_snapshot(&self) -> bool {
        self.data.known_exports.load_snapshot_interface.is_some()
    }

    pub fn has_save_snapshot(&self) -> bool {
        self.data.known_exports.save_snapshot_interface.is_some()
    }

    pub fn has_agent_guest(&self) -> bool {
        self.data.known_exports.agent_guest_interface.is_some()
    }

    pub fn has_oplog_processor(&self) -> bool {
        self.data.known_exports.oplog_processor_interface.is_some()
    }

    /// Returns the fully-qualified WIT function name for `golem:api/load-snapshot.load`
    pub fn load_snapshot_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .load_snapshot_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{load}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:api/save-snapshot.save`
    pub fn save_snapshot_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .save_snapshot_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{save}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:agent/guest.initialize`
    pub fn agent_initialize_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .agent_guest_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{initialize}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:agent/guest.invoke`
    pub fn agent_invoke_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .agent_guest_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{invoke}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:api/oplog-processor.process`
    pub fn oplog_processor_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .oplog_processor_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{process}}"))
    }

    pub fn find_agent_type_by_name(&self, agent_type: &AgentTypeName) -> Option<AgentType> {
        self.data
            .agent_types
            .iter()
            .find(|t| &t.type_name == agent_type)
            .cloned()
    }
}

impl poem_openapi::types::Type for ComponentMetadata {
    const IS_REQUIRED: bool =
        <ComponentMetadataInnerData as poem_openapi::types::Type>::IS_REQUIRED;
    type RawValueType = <ComponentMetadataInnerData as poem_openapi::types::Type>::RawValueType;
    type RawElementValueType =
        <ComponentMetadataInnerData as poem_openapi::types::Type>::RawElementValueType;

    fn name() -> std::borrow::Cow<'static, str> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::name()
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::schema_ref()
    }

    fn register(registry: &mut poem_openapi::registry::Registry) {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::register(registry);
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::as_raw_value(&self.data)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::raw_element_iter(&self.data)
    }
}

impl poem_openapi::types::IsObjectType for ComponentMetadata {}

impl poem_openapi::types::ParseFromJSON for ComponentMetadata {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_json(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToJSON for ComponentMetadata {
    fn to_json(&self) -> Option<serde_json::Value> {
        self.data.to_json()
    }
}

impl poem_openapi::types::ParseFromXML for ComponentMetadata {
    fn parse_from_xml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_xml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToXML for ComponentMetadata {
    fn to_xml(&self) -> Option<serde_json::Value> {
        self.data.to_xml()
    }
}

impl poem_openapi::types::ParseFromYAML for ComponentMetadata {
    fn parse_from_yaml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_yaml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToYAML for ComponentMetadata {
    fn to_yaml(&self) -> Option<serde_json::Value> {
        self.data.to_yaml()
    }
}

impl From<wasmparser::MemoryType> for LinearMemory {
    fn from(value: wasmparser::MemoryType) -> Self {
        Self {
            initial: value.initial * LinearMemory::PAGE_SIZE,
            maximum: value.maximum.map(|m| m * LinearMemory::PAGE_SIZE),
        }
    }
}

/// Metadata of Component in terms of golem_wasm_ast types
#[derive(Default)]
pub struct RawComponentMetadata {
    pub known_exports: KnownExports,
    pub producers: Vec<WasmAstProducers>,
    pub memories: Vec<LinearMemory>,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

/// Interface name prefixes used to identify known Golem capabilities.
const AGENT_GUEST_PREFIX: &str = "golem:agent/guest";
const SAVE_SNAPSHOT_PREFIX: &str = "golem:api/save-snapshot";
const LOAD_SNAPSHOT_PREFIX: &str = "golem:api/load-snapshot";
const OPLOG_PROCESSOR_PREFIX: &str = "golem:api/oplog-processor";

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

/// Extract a `KnownExports` index from the full analysed export tree.
/// Only instance exports are considered; each supported interface prefix
/// is matched at most once (the exact versioned name is stored).
pub fn extract_known_exports(exports: &[AnalysedExport]) -> AnalysisResult<KnownExports> {
    let mut known = KnownExports::default();

    for export in exports {
        if let AnalysedExport::Instance(instance) = export {
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
        agent_types: Vec<AgentType>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
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
            agent_types,
            agent_type_provision_configs,
        }
    }
}

impl From<golem_wasm::metadata::Producers> for Producers {
    fn from(value: golem_wasm::metadata::Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<Producers> for golem_wasm::metadata::Producers {
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

impl From<golem_wasm::metadata::ProducersField> for ProducerField {
    fn from(value: golem_wasm::metadata::ProducersField) -> Self {
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

impl From<ProducerField> for golem_wasm::metadata::ProducersField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(|value| golem_wasm::metadata::VersionedName {
                    name: value.name,
                    version: value.version,
                })
                .collect(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ComponentProcessingError {
    Parsing(String),
    Analysis(AnalysisFailure),
}

impl SafeDisplay for ComponentProcessingError {
    fn to_safe_string(&self) -> String {
        match self {
            ComponentProcessingError::Parsing(_) => self.to_string(),
            ComponentProcessingError::Analysis(_) => self.to_string(),
        }
    }
}

impl Display for ComponentProcessingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ComponentProcessingError::Parsing(e) => write!(f, "Parsing error: {e}"),
            ComponentProcessingError::Analysis(source) => {
                let AnalysisFailure { reason } = source;
                write!(f, "Analysis error: {reason}")
            }
        }
    }
}

mod protobuf {
    use crate::base_model::component_metadata::{AgentTypeProvisionConfig, KnownExports};
    use crate::model::agent::AgentTypeName;
    use crate::model::component_metadata::{
        ComponentMetadata, ComponentMetadataInnerData, LinearMemory, ProducerField, Producers,
        VersionedName,
    };
    use std::sync::Arc;

    impl From<golem_api_grpc::proto::golem::component::VersionedName> for VersionedName {
        fn from(value: golem_api_grpc::proto::golem::component::VersionedName) -> Self {
            Self {
                name: value.name,
                version: value.version,
            }
        }
    }

    impl From<VersionedName> for golem_api_grpc::proto::golem::component::VersionedName {
        fn from(value: VersionedName) -> Self {
            Self {
                name: value.name,
                version: value.version,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::ProducerField> for ProducerField {
        fn from(value: golem_api_grpc::proto::golem::component::ProducerField) -> Self {
            Self {
                name: value.name,
                values: value.values.into_iter().map(|value| value.into()).collect(),
            }
        }
    }

    impl From<ProducerField> for golem_api_grpc::proto::golem::component::ProducerField {
        fn from(value: ProducerField) -> Self {
            Self {
                name: value.name,
                values: value.values.into_iter().map(|value| value.into()).collect(),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::Producers> for Producers {
        fn from(value: golem_api_grpc::proto::golem::component::Producers) -> Self {
            Self {
                fields: value.fields.into_iter().map(|field| field.into()).collect(),
            }
        }
    }

    impl From<Producers> for golem_api_grpc::proto::golem::component::Producers {
        fn from(value: Producers) -> Self {
            Self {
                fields: value.fields.into_iter().map(|field| field.into()).collect(),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::LinearMemory> for LinearMemory {
        fn from(value: golem_api_grpc::proto::golem::component::LinearMemory) -> Self {
            Self {
                initial: value.initial,
                maximum: value.maximum,
            }
        }
    }

    impl From<LinearMemory> for golem_api_grpc::proto::golem::component::LinearMemory {
        fn from(value: LinearMemory) -> Self {
            Self {
                initial: value.initial,
                maximum: value.maximum,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ComponentMetadata> for ComponentMetadata {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ComponentMetadata,
        ) -> Result<Self, Self::Error> {
            let inner_data = ComponentMetadataInnerData::try_from(value)?;
            Ok(Self {
                data: Arc::new(inner_data),
            })
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ComponentMetadata>
        for ComponentMetadataInnerData
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ComponentMetadata,
        ) -> Result<Self, Self::Error> {
            let known_exports = value
                .known_exports
                .map(KnownExports::from)
                .unwrap_or_default();
            Ok(Self {
                known_exports,
                producers: value
                    .producers
                    .into_iter()
                    .map(|producer| producer.into())
                    .collect(),
                memories: value
                    .memories
                    .into_iter()
                    .map(|memory| memory.into())
                    .collect(),
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
                agent_types: value
                    .agent_types
                    .into_iter()
                    .map(|at| at.try_into())
                    .collect::<Result<_, _>>()?,
                agent_type_provision_configs: value
                    .agent_type_provision_configs
                    .into_iter()
                    .map(|(k, v)| {
                        AgentTypeProvisionConfig::try_from(v)
                            .map(|config| (AgentTypeName(k), config))
                    })
                    .collect::<Result<_, _>>()?,
            })
        }
    }

    impl From<golem_api_grpc::proto::golem::component::KnownExports> for KnownExports {
        fn from(value: golem_api_grpc::proto::golem::component::KnownExports) -> Self {
            Self {
                agent_guest_interface: value.agent_guest_interface,
                save_snapshot_interface: value.save_snapshot_interface,
                load_snapshot_interface: value.load_snapshot_interface,
                oplog_processor_interface: value.oplog_processor_interface,
            }
        }
    }

    impl From<KnownExports> for golem_api_grpc::proto::golem::component::KnownExports {
        fn from(value: KnownExports) -> Self {
            Self {
                agent_guest_interface: value.agent_guest_interface,
                save_snapshot_interface: value.save_snapshot_interface,
                load_snapshot_interface: value.load_snapshot_interface,
                oplog_processor_interface: value.oplog_processor_interface,
            }
        }
    }

    impl From<ComponentMetadata> for golem_api_grpc::proto::golem::component::ComponentMetadata {
        fn from(value: ComponentMetadata) -> Self {
            value.data.as_ref().clone().into()
        }
    }

    impl From<ComponentMetadataInnerData>
        for golem_api_grpc::proto::golem::component::ComponentMetadata
    {
        fn from(value: ComponentMetadataInnerData) -> Self {
            Self {
                known_exports: Some(value.known_exports.into()),
                producers: value
                    .producers
                    .into_iter()
                    .map(|producer| producer.into())
                    .collect(),
                memories: value
                    .memories
                    .into_iter()
                    .map(|memory| memory.into())
                    .collect(),
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
                agent_types: value.agent_types.into_iter().map(|at| at.into()).collect(),
                agent_type_provision_configs: value
                    .agent_type_provision_configs
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k.0,
                            golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig::from(
                                v,
                            ),
                        )
                    })
                    .collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig>
        for AgentTypeProvisionConfig
    {
        type Error = String;

        fn try_from(
            proto: golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig,
        ) -> Result<Self, Self::Error> {
            use crate::base_model::component::{InitialAgentFile, InstalledPlugin};
            use crate::base_model::worker::TypedAgentConfigEntry;

            let config = proto
                .config_values
                .into_iter()
                .map(TypedAgentConfigEntry::try_from)
                .collect::<Result<Vec<_>, _>>()?;

            let plugins = proto
                .plugins
                .into_iter()
                .map(InstalledPlugin::try_from)
                .collect::<Result<Vec<_>, _>>()?;

            let files = proto
                .files
                .into_iter()
                .map(InitialAgentFile::try_from)
                .collect::<Result<Vec<_>, _>>()?;

            Ok(AgentTypeProvisionConfig {
                env: proto.env.into_iter().collect(),
                wasi_config: proto.wasi_config.into_iter().collect(),
                config,
                plugins,
                files,
            })
        }
    }

    impl From<AgentTypeProvisionConfig>
        for golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig
    {
        fn from(config: AgentTypeProvisionConfig) -> Self {
            use crate::base_model::component::{InitialAgentFile, InstalledPlugin};

            Self {
                env: config.env.into_iter().collect(),
                wasi_config: config.wasi_config.into_iter().collect(),
                config_values: config
                    .config
                    .into_iter()
                    .map(golem_api_grpc::proto::golem::worker::TypedAgentConfigEntry::from)
                    .collect(),
                plugins: config
                    .plugins
                    .into_iter()
                    .map(|p: InstalledPlugin| {
                        golem_api_grpc::proto::golem::component::PluginInstallation::from(p)
                    })
                    .collect(),
                files: config
                    .files
                    .into_iter()
                    .map(|f: InitialAgentFile| {
                        golem_api_grpc::proto::golem::component::InitialAgentFile::from(f)
                    })
                    .collect(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm::analysis::{AnalysedExport, AnalysedInstance};
    use test_r::test;

    fn instance_export(name: &str) -> AnalysedExport {
        AnalysedExport::Instance(AnalysedInstance {
            name: name.to_string(),
            functions: vec![],
        })
    }

    #[test]
    fn extract_known_exports_collects_supported_interfaces() {
        let exports = vec![
            instance_export("example:ignored/foo@1.0.0"),
            instance_export("golem:agent/guest@1.5.0"),
            instance_export("golem:api/save-snapshot@1.5.0"),
            instance_export("golem:api/load-snapshot@1.5.0"),
            instance_export("golem:api/oplog-processor@1.5.0"),
        ];

        let known = extract_known_exports(&exports).unwrap();

        assert_eq!(
            known,
            KnownExports {
                agent_guest_interface: Some("golem:agent/guest@1.5.0".to_string()),
                save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".to_string()),
                load_snapshot_interface: Some("golem:api/load-snapshot@1.5.0".to_string()),
                oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
            }
        );
    }

    #[test]
    fn extract_known_exports_rejects_duplicate_capabilities() {
        let exports = vec![
            instance_export("golem:api/save-snapshot@1.5.0"),
            instance_export("golem:api/save-snapshot@1.6.0"),
        ];

        let error = extract_known_exports(&exports).unwrap_err();

        assert!(error.reason.contains("Duplicate save snapshot export"));
        assert!(error.reason.contains("golem:api/save-snapshot@1.5.0"));
        assert!(error.reason.contains("golem:api/save-snapshot@1.6.0"));
    }

    #[test]
    fn component_metadata_helpers_use_exact_known_export_names() {
        let metadata = ComponentMetadata::from_parts(
            KnownExports {
                agent_guest_interface: Some("golem:agent/guest@1.5.0".to_string()),
                save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".to_string()),
                load_snapshot_interface: Some("golem:api/load-snapshot@1.5.0".to_string()),
                oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
            },
            vec![],
            None,
            None,
            vec![],
            BTreeMap::new(),
        );

        assert_eq!(
            metadata.agent_initialize_function_name(),
            Some("golem:agent/guest@1.5.0.{initialize}".to_string())
        );
        assert_eq!(
            metadata.agent_invoke_function_name(),
            Some("golem:agent/guest@1.5.0.{invoke}".to_string())
        );
        assert_eq!(
            metadata.save_snapshot_function_name(),
            Some("golem:api/save-snapshot@1.5.0.{save}".to_string())
        );
        assert_eq!(
            metadata.load_snapshot_function_name(),
            Some("golem:api/load-snapshot@1.5.0.{load}".to_string())
        );
        assert_eq!(
            metadata.oplog_processor_function_name(),
            Some("golem:api/oplog-processor@1.5.0.{process}".to_string())
        );
    }
}
