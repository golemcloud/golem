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

use crate::model::agent::wit_naming::ToWitNaming;
use crate::model::agent::{AgentType, AgentTypeName};
use crate::model::base64::Base64;
use crate::SafeDisplay;
use golem_wasm::analysis::wit_parser::WitAnalysisContext;
use golem_wasm::analysis::{AnalysedExport, AnalysedFunction, AnalysisFailure};
use golem_wasm::analysis::{
    AnalysedFunctionParameter, AnalysedInstance, AnalysedResourceId, AnalysedResourceMode,
    AnalysedType, TypeHandle,
};
use golem_wasm::metadata::Producers as WasmAstProducers;
use std::collections::HashMap;
use std::fmt::{self, Debug, Display, Formatter};
use std::sync::Arc;
use wasmtime::component::__internal::wasmtime_environ::wasmparser;

pub use super::parsed_function_name::{
    ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, SemVer,
};
pub use crate::base_model::component_metadata::*;

/// Describes an exported function that can be invoked on a worker
#[derive(Debug, Clone)]
pub struct InvokableFunction {
    pub name: ParsedFunctionName,
    pub analysed_export: AnalysedFunction,
    pub agent_method_or_constructor: Option<AgentMethodOrConstructor>,
}

impl ComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
        agent_types: Vec<AgentType>,
    ) -> Result<Self, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(Self {
            data: Arc::new(raw.into_metadata(agent_types)),
            cache: Arc::default(),
        })
    }

    pub fn from_parts(
        exports: Vec<AnalysedExport>,
        memories: Vec<LinearMemory>,
        root_package_name: Option<String>,
        root_package_version: Option<String>,
        agent_types: Vec<AgentType>,
    ) -> Self {
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                exports,
                producers: vec![],
                memories,
                binary_wit: Base64(vec![]),
                root_package_name,
                root_package_version,
                agent_types,
            }),
            cache: Arc::default(),
        }
    }

    pub fn producers(&self) -> &[Producers] {
        &self.data.producers
    }

    pub fn memories(&self) -> &[LinearMemory] {
        &self.data.memories
    }

    pub fn binary_wit(&self) -> &Base64 {
        &self.data.binary_wit
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

    pub fn is_agent(&self) -> bool {
        !self.data.agent_types.is_empty()
    }

    pub fn load_snapshot(&self) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().unwrap().load_snapshot(&self.data)
    }

    pub fn save_snapshot(&self) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().unwrap().save_snapshot(&self.data)
    }

    pub fn agent_initialize(&self) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().unwrap().agent_initialize(&self.data)
    }

    pub fn agent_invoke(&self) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().unwrap().agent_invoke(&self.data)
    }

    pub fn oplog_processor(&self) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().unwrap().oplog_processor(&self.data)
    }

    pub fn find_function(&self, name: &str) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().unwrap().find_function(&self.data, name)
    }

    pub fn find_parsed_function(
        &self,
        parsed: &ParsedFunctionName,
    ) -> Result<Option<InvokableFunction>, String> {
        self.cache
            .lock()
            .unwrap()
            .find_parsed_function(&self.data, parsed)
    }

    pub fn find_agent_type_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentType>, String> {
        self.cache
            .lock()
            .unwrap()
            .find_agent_type_by_name(&self.data, agent_type)
    }

    pub fn find_agent_type_by_wrapper_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentType>, String> {
        self.cache
            .lock()
            .unwrap()
            .find_agent_type_by_wrapper_name(&self.data, agent_type)
    }

    pub fn find_wrapper_function_by_agent_constructor(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<Option<InvokableFunction>, String> {
        self.cache
            .lock()
            .unwrap()
            .find_wrapper_function_by_agent_constructor(&self.data, agent_type)
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
            cache: Arc::default(),
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
            cache: Arc::default(),
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
            cache: Arc::default(),
        })
    }
}

impl poem_openapi::types::ToYAML for ComponentMetadata {
    fn to_yaml(&self) -> Option<serde_json::Value> {
        self.data.to_yaml()
    }
}

impl ComponentMetadataInnerData {
    pub fn load_snapshot(&self) -> Result<Option<InvokableFunction>, String> {
        self.find_parsed_function_ignoring_version(&ParsedFunctionName::new(
            ParsedFunctionSite::PackagedInterface {
                namespace: "golem".to_string(),
                package: "api".to_string(),
                interface: "load-snapshot".to_string(),
                version: None,
            },
            ParsedFunctionReference::Function {
                function: "load".to_string(),
            },
        ))
    }

    pub fn save_snapshot(&self) -> Result<Option<InvokableFunction>, String> {
        self.find_parsed_function_ignoring_version(&ParsedFunctionName::new(
            ParsedFunctionSite::PackagedInterface {
                namespace: "golem".to_string(),
                package: "api".to_string(),
                interface: "save-snapshot".to_string(),
                version: None,
            },
            ParsedFunctionReference::Function {
                function: "save".to_string(),
            },
        ))
    }

    pub fn agent_initialize(&self) -> Result<Option<InvokableFunction>, String> {
        self.find_parsed_function_ignoring_version(&ParsedFunctionName::new(
            ParsedFunctionSite::PackagedInterface {
                namespace: "golem".to_string(),
                package: "agent".to_string(),
                interface: "guest".to_string(),
                version: None,
            },
            ParsedFunctionReference::Function {
                function: "initialize".to_string(),
            },
        ))
    }

    pub fn agent_invoke(&self) -> Result<Option<InvokableFunction>, String> {
        self.find_parsed_function_ignoring_version(&ParsedFunctionName::new(
            ParsedFunctionSite::PackagedInterface {
                namespace: "golem".to_string(),
                package: "agent".to_string(),
                interface: "guest".to_string(),
                version: None,
            },
            ParsedFunctionReference::Function {
                function: "invoke".to_string(),
            },
        ))
    }

    pub fn oplog_processor(&self) -> Result<Option<InvokableFunction>, String> {
        self.find_parsed_function_ignoring_version(&ParsedFunctionName::new(
            ParsedFunctionSite::PackagedInterface {
                namespace: "golem".to_string(),
                package: "api".to_string(),
                interface: "oplog-processor".to_string(),
                version: None,
            },
            ParsedFunctionReference::Function {
                function: "process".to_string(),
            },
        ))
    }

    pub fn find_function(&self, name: &str) -> Result<Option<InvokableFunction>, String> {
        let parsed = ParsedFunctionName::parse(name)?;
        self.find_parsed_function(&parsed)
    }

    pub fn find_parsed_function(
        &self,
        parsed: &ParsedFunctionName,
    ) -> Result<Option<InvokableFunction>, String> {
        Ok(self
            .find_analysed_function(parsed)
            .map(|analysed_export| {
                self.find_agent_method_or_constructor(parsed)
                    .map(|agent_method_or_constructor| {
                        (analysed_export, agent_method_or_constructor)
                    })
            })
            .transpose()?
            .map(
                |(analysed_export, agent_method_or_constructor)| InvokableFunction {
                    name: parsed.clone(),
                    analysed_export,
                    agent_method_or_constructor,
                },
            ))
    }

    pub fn find_agent_type_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentType>, String> {
        Ok(self
            .agent_types
            .iter()
            .find(|t| &t.type_name == agent_type)
            .cloned())
    }

    pub fn find_agent_type_by_wrapper_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentType>, String> {
        Ok(self
            .agent_types
            .iter()
            .find(|t| &t.type_name.to_wit_naming() == agent_type)
            .cloned())
    }

    pub fn find_wrapper_function_by_agent_constructor(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<Option<InvokableFunction>, String> {
        let agent_type = self
            .find_agent_type_by_name(agent_type)?
            .ok_or_else(|| format!("Agent type {} not found", agent_type))?;

        let root_package_name = self
            .root_package_name
            .as_ref()
            .ok_or("Agents must have a root package name")?;
        let (namespace, package) = root_package_name
            .split_once(':')
            .ok_or("Root package name must be in the form namespace:package")?;
        let version = self
            .root_package_version
            .as_ref()
            .map(|v| SemVer::parse(v))
            .transpose()?;

        let interface = agent_type.wrapper_type_name();
        let function = ParsedFunctionName::new(
            ParsedFunctionSite::PackagedInterface {
                namespace: namespace.to_string(),
                package: package.to_string(),
                interface,
                version,
            },
            ParsedFunctionReference::Function {
                function: "initialize".to_string(),
            },
        );

        self.find_parsed_function(&function)
    }

    /// Finds a function ignoring the function site's version. Returns None if it was not found.
    fn find_parsed_function_ignoring_version(
        &self,
        name: &ParsedFunctionName,
    ) -> Result<Option<InvokableFunction>, String> {
        Ok(self
            .exports
            .iter()
            .find_map(|export| {
                if let AnalysedExport::Instance(instance) = export {
                    if let Ok(site) = ParsedFunctionSite::parse(&instance.name) {
                        if &site.unversioned() == name.site() {
                            Self::find_function_in_instance(name, instance)
                                .map(|func| (func, name.with_site(site)))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .map(|(analysed_export, name)| {
                self.find_agent_method_or_constructor(&name)
                    .map(|agent_method_or_constructor| {
                        (analysed_export, name, agent_method_or_constructor)
                    })
            })
            .transpose()?
            .map(
                |(analysed_export, name, agent_method_or_constructor)| InvokableFunction {
                    name,
                    analysed_export,
                    agent_method_or_constructor,
                },
            ))
    }

    fn find_analysed_function(&self, parsed: &ParsedFunctionName) -> Option<AnalysedFunction> {
        match &parsed.site().interface_name() {
            None => self.exports.iter().find_map(|f| match f {
                AnalysedExport::Function(f) if f.name == parsed.function().function_name() => {
                    Some(f.clone())
                }
                _ => None,
            }),
            Some(interface_name) => self
                .exports
                .iter()
                .find_map(|instance| match instance {
                    AnalysedExport::Instance(inst) if inst.name == *interface_name => Some(inst),
                    _ => None,
                })
                .and_then(|instance| Self::find_function_in_instance(parsed, instance)),
        }
    }

    fn find_function_in_instance(
        parsed: &ParsedFunctionName,
        instance: &AnalysedInstance,
    ) -> Option<AnalysedFunction> {
        match instance
            .functions
            .iter()
            .find(|f| f.name == parsed.function().function_name())
            .cloned()
        {
            Some(function) => Some(function),
            None => match parsed.method_as_static() {
                Some(parsed_static) => instance
                    .functions
                    .iter()
                    .find(|f| f.name == parsed_static.function().function_name())
                    .cloned(),
                None => None,
            },
        }
    }

    fn find_agent_method_or_constructor(
        &self,
        parsed: &ParsedFunctionName,
    ) -> Result<Option<AgentMethodOrConstructor>, String> {
        if let Some(root_package_name) = &self.root_package_name {
            if let Some((root_namespace, root_package)) = root_package_name.split_once(':') {
                let static_agent_interface = ParsedFunctionSite::PackagedInterface {
                    namespace: root_namespace.to_string(),
                    package: root_package.to_string(),
                    interface: "agent".to_string(),
                    version: self
                        .root_package_version
                        .as_ref()
                        .map(|v| SemVer::parse(v))
                        .transpose()?,
                };

                if parsed.site() == &static_agent_interface {
                    if let Some(resource_name) = parsed.is_method() {
                        let agent = self
                            .agent_types
                            .iter()
                            .find(|agent| agent.wrapper_type_name() == resource_name);

                        let method = agent.and_then(|agent| {
                            agent
                                .methods
                                .iter()
                                .find(|method| {
                                    method.name.to_wit_naming() == parsed.function().function_name()
                                })
                                .cloned()
                        });

                        Ok(method.map(AgentMethodOrConstructor::Method))
                    } else if let Some(resource_name) = parsed.is_static_method() {
                        if parsed.function().function_name() == "create" {
                            // this can be an agent constructor
                            let agent = self
                                .agent_types
                                .iter()
                                .find(|agent| agent.wrapper_type_name() == resource_name);

                            Ok(agent.map(|agent| {
                                AgentMethodOrConstructor::Constructor(agent.constructor.clone())
                            }))
                        } else {
                            Ok(None) // Not the agent constructor
                        }
                    } else {
                        Ok(None) // Not a method or static method
                    }
                } else {
                    Ok(None) // Not belonging to the static agent wrapper interface
                }
            } else {
                Ok(None) // Not a valid root package name
            }
        } else {
            Ok(None) // No root package name
        }
    }
}

#[derive(Default)]
pub(crate) struct ComponentMetadataInnerCache {
    load_snapshot: Option<Result<Option<InvokableFunction>, String>>,
    save_snapshot: Option<Result<Option<InvokableFunction>, String>>,
    agent_initialize: Option<Result<Option<InvokableFunction>, String>>,
    agent_invoke: Option<Result<Option<InvokableFunction>, String>>,
    oplog_processor: Option<Result<Option<InvokableFunction>, String>>,
    functions_unparsed: HashMap<String, Result<Option<InvokableFunction>, String>>,
    functions_parsed: HashMap<ParsedFunctionName, Result<Option<InvokableFunction>, String>>,
    agent_types_by_type_name: HashMap<AgentTypeName, Result<Option<AgentType>, String>>,
    agent_types_by_wrapper_type_name: HashMap<AgentTypeName, Result<Option<AgentType>, String>>,
    functions_by_agent_constructor:
        HashMap<AgentTypeName, Result<Option<InvokableFunction>, String>>,
}

impl ComponentMetadataInnerCache {
    pub fn load_snapshot(
        &mut self,
        data: &ComponentMetadataInnerData,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(snapshot) = &self.load_snapshot {
            snapshot.clone()
        } else {
            let result = data.load_snapshot();
            self.load_snapshot = Some(result.clone());
            result
        }
    }

    pub fn save_snapshot(
        &mut self,
        data: &ComponentMetadataInnerData,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(snapshot) = &self.save_snapshot {
            snapshot.clone()
        } else {
            let result = data.save_snapshot();
            self.save_snapshot = Some(result.clone());
            result
        }
    }

    pub fn agent_initialize(
        &mut self,
        data: &ComponentMetadataInnerData,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(cached) = &self.agent_initialize {
            cached.clone()
        } else {
            let result = data.agent_initialize();
            self.agent_initialize = Some(result.clone());
            result
        }
    }

    pub fn agent_invoke(
        &mut self,
        data: &ComponentMetadataInnerData,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(cached) = &self.agent_invoke {
            cached.clone()
        } else {
            let result = data.agent_invoke();
            self.agent_invoke = Some(result.clone());
            result
        }
    }

    pub fn oplog_processor(
        &mut self,
        data: &ComponentMetadataInnerData,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(cached) = &self.oplog_processor {
            cached.clone()
        } else {
            let result = data.oplog_processor();
            self.oplog_processor = Some(result.clone());
            result
        }
    }

    pub fn find_function(
        &mut self,
        data: &ComponentMetadataInnerData,
        name: &str,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(cached) = self.functions_unparsed.get(name) {
            cached.clone()
        } else {
            let parsed = ParsedFunctionName::parse(name)?;
            let result = data.find_parsed_function(&parsed);
            self.functions_unparsed
                .insert(name.to_string(), result.clone());
            result
        }
    }

    pub fn find_parsed_function(
        &mut self,
        data: &ComponentMetadataInnerData,
        parsed: &ParsedFunctionName,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(cached) = self.functions_parsed.get(parsed) {
            cached.clone()
        } else {
            let result = data.find_parsed_function(parsed);
            self.functions_parsed.insert(parsed.clone(), result.clone());
            result
        }
    }

    pub fn find_agent_type_by_name(
        &mut self,
        data: &ComponentMetadataInnerData,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentType>, String> {
        if let Some(cached) = self.agent_types_by_type_name.get(agent_type) {
            cached.clone()
        } else {
            let result = data.find_agent_type_by_name(agent_type);
            self.agent_types_by_type_name
                .insert(agent_type.clone(), result.clone());
            result
        }
    }

    pub fn find_agent_type_by_wrapper_name(
        &mut self,
        data: &ComponentMetadataInnerData,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentType>, String> {
        if let Some(cached) = self.agent_types_by_wrapper_type_name.get(agent_type) {
            cached.clone()
        } else {
            let result = data.find_agent_type_by_wrapper_name(agent_type);
            self.agent_types_by_wrapper_type_name
                .insert(agent_type.clone(), result.clone());
            result
        }
    }

    pub fn find_wrapper_function_by_agent_constructor(
        &mut self,
        data: &ComponentMetadataInnerData,
        agent_type: &AgentTypeName,
    ) -> Result<Option<InvokableFunction>, String> {
        if let Some(cached) = self.functions_by_agent_constructor.get(agent_type) {
            cached.clone()
        } else {
            let result = data.find_wrapper_function_by_agent_constructor(agent_type);
            self.functions_by_agent_constructor
                .insert(agent_type.clone(), result.clone());
            result
        }
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
    pub exports: Vec<AnalysedExport>,
    pub producers: Vec<WasmAstProducers>,
    pub memories: Vec<LinearMemory>,
    pub binary_wit: Vec<u8>,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

impl RawComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
    ) -> Result<RawComponentMetadata, ComponentProcessingError> {
        let wit_analysis =
            WitAnalysisContext::new(data).map_err(ComponentProcessingError::Analysis)?;

        let mut exports = wit_analysis
            .get_top_level_exports()
            .map_err(ComponentProcessingError::Analysis)?;
        let binary_wit = wit_analysis
            .serialized_interface_only()
            .map_err(ComponentProcessingError::Analysis)?;
        let root_package = wit_analysis.root_package_name();

        for warning in wit_analysis.warnings() {
            tracing::warn!("Wit analysis warning: {}", warning);
        }

        add_resource_drops(&mut exports);

        let memories = wit_analysis
            .linear_memories()
            .iter()
            .cloned()
            .map(LinearMemory::from)
            .collect();

        let producers = wit_analysis.producers().to_vec();

        Ok(RawComponentMetadata {
            exports,
            producers,
            memories,
            binary_wit,
            root_package_name: root_package
                .as_ref()
                .map(|pkg| format!("{}:{}", pkg.namespace, pkg.name)),
            root_package_version: root_package.and_then(|pkg| pkg.version.map(|v| v.to_string())),
        })
    }

    pub fn into_metadata(self, agent_types: Vec<AgentType>) -> ComponentMetadataInnerData {
        let producers = self
            .producers
            .into_iter()
            .map(|producers| producers.into())
            .collect::<Vec<_>>();

        let exports = self.exports.into_iter().collect::<Vec<_>>();

        let memories = self.memories.into_iter().collect();

        ComponentMetadataInnerData {
            exports,
            producers,
            memories,
            binary_wit: Base64(self.binary_wit),
            root_package_name: self.root_package_name,
            root_package_version: self.root_package_version,
            agent_types,
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

fn collect_resource_types(
    resource_types: &mut HashMap<AnalysedResourceId, AnalysedFunction>,
    fun: &AnalysedFunction,
) {
    if fun.is_constructor() {
        if let AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Owned,
            resource_id,
            ..
        }) = &fun.result.as_ref().unwrap().typ
        {
            resource_types.insert(*resource_id, fun.clone());
        }
    } else if fun.is_method() {
        if let AnalysedType::Handle(TypeHandle {
            mode: AnalysedResourceMode::Borrowed,
            resource_id,
            ..
        }) = &fun.parameters[0].typ
        {
            resource_types.insert(*resource_id, fun.clone());
        }
    }
}

fn add_resource_drops(exports: &mut Vec<AnalysedExport>) {
    // Components are not exporting explicit drop functions for exported resources, but
    // worker executor does. So we keep golem-wasm-ast as a universal library and extend
    // its result with the explicit drops here, for each resource, identified by an exported
    // constructor.

    let mut top_level_resource_types = HashMap::new();
    for export in exports.iter_mut() {
        match export {
            AnalysedExport::Function(fun) => {
                collect_resource_types(&mut top_level_resource_types, fun);
            }
            AnalysedExport::Instance(instance) => {
                let mut instance_resource_types = HashMap::new();
                for fun in &instance.functions {
                    collect_resource_types(&mut instance_resource_types, fun);
                }

                for fun in instance_resource_types.values() {
                    instance
                        .functions
                        .push(drop_from_constructor_or_method(fun));
                }
            }
        }
    }

    for fun in top_level_resource_types.values() {
        exports.push(AnalysedExport::Function(drop_from_constructor_or_method(
            fun,
        )));
    }
}

fn drop_from_constructor_or_method(fun: &AnalysedFunction) -> AnalysedFunction {
    if fun.is_constructor() {
        let drop_name = fun.name.replace("[constructor]", "[drop]");
        AnalysedFunction {
            name: drop_name,
            parameters: fun
                .result
                .iter()
                .map(|result| AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: result.typ.clone(),
                })
                .collect(),
            result: None,
        }
    } else {
        let name = fun.name.replace("[method]", "[drop]");
        let (drop_name, _) = name.split_once('.').unwrap();
        AnalysedFunction {
            name: drop_name.to_string(),
            parameters: fun
                .parameters
                .first()
                .map(|param| AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: {
                        let AnalysedType::Handle(TypeHandle {
                            mode: _,
                            resource_id,
                            name,
                            owner,
                        }) = &param.typ
                        else {
                            panic!("Expected handle type for resource drop")
                        };
                        AnalysedType::Handle(TypeHandle {
                            mode: AnalysedResourceMode::Owned,
                            resource_id: *resource_id,
                            name: name.clone(),
                            owner: owner.clone(),
                        })
                    },
                })
                .into_iter()
                .collect(),
            result: None,
        }
    }
}

mod protobuf {
    use crate::model::base64::Base64;
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
                cache: Arc::default(),
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
            Ok(Self {
                exports: value
                    .exports
                    .into_iter()
                    .map(|export| export.try_into())
                    .collect::<Result<_, _>>()?,
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
                binary_wit: Base64(value.binary_wit),
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
                agent_types: value
                    .agent_types
                    .into_iter()
                    .map(|at| at.try_into())
                    .collect::<Result<_, _>>()?,
            })
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
                exports: value
                    .exports
                    .into_iter()
                    .map(|export| export.into())
                    .collect(),
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
                binary_wit: value.binary_wit.0,
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
                agent_types: value.agent_types.into_iter().map(|at| at.into()).collect(),
            }
        }
    }
}
