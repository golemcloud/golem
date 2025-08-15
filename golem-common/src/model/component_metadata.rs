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

use crate::model::agent::{AgentConstructor, AgentMethod, AgentType};
use crate::model::base64::Base64;
use crate::model::ComponentType;
use crate::{virtual_exports, SafeDisplay};
use bincode::de::BorrowDecoder;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use golem_wasm_ast::analysis::wit_parser::WitAnalysisContext;
use golem_wasm_ast::analysis::{
    AnalysedFunctionParameter, AnalysedInstance, AnalysedResourceId, AnalysedResourceMode,
    AnalysedType, TypeHandle,
};
use golem_wasm_ast::core::Mem;
use golem_wasm_ast::metadata::Producers as WasmAstProducers;
use golem_wasm_ast::{
    analysis::{AnalysedExport, AnalysedFunction, AnalysisContext, AnalysisFailure},
    component::Component,
    IgnoreAllButMetadata,
};
use heck::ToKebabCase;
use rib::{ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, SemVer};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt::{self, Debug, Display, Formatter};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct ComponentMetadata {
    data: Arc<ComponentMetadataInnerData>,
    cache: Arc<Mutex<ComponentMetadataInnerCache>>,
}

impl ComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        agent_types: Vec<AgentType>,
    ) -> Result<Self, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(Self {
            data: Arc::new(raw.into_metadata(dynamic_linking, agent_types)),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        })
    }

    pub fn from_parts(
        exports: Vec<AnalysedExport>,
        memories: Vec<LinearMemory>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
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
                dynamic_linking,
                agent_types,
            }),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        }
    }

    pub fn exports(&self) -> &[AnalysedExport] {
        &self.data.exports
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
    pub fn dynamic_linking(&self) -> &HashMap<String, DynamicLinkedInstance> {
        &self.data.dynamic_linking
    }
    pub fn agent_types(&self) -> &[AgentType] {
        &self.data.agent_types
    }

    pub async fn load_snapshot(&self) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().await.load_snapshot(&self.data)
    }

    pub async fn save_snapshot(&self) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().await.save_snapshot(&self.data)
    }

    pub async fn find_function(&self, name: &str) -> Result<Option<InvokableFunction>, String> {
        self.cache.lock().await.find_function(&self.data, name)
    }

    pub async fn find_parsed_function(
        &self,
        parsed: &ParsedFunctionName,
    ) -> Result<Option<InvokableFunction>, String> {
        self.cache
            .lock()
            .await
            .find_parsed_function(&self.data, parsed)
    }

    pub async fn find_agent_type(&self, agent_type: &str) -> Result<Option<AgentType>, String> {
        self.cache
            .lock()
            .await
            .find_agent_type(&self.data, agent_type)
    }
}

impl Debug for ComponentMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentMetadata")
            .field("exports", &self.data.exports)
            .field("producers", &self.data.producers)
            .field("memories", &self.data.memories)
            .field("binary_wit_len", &self.data.binary_wit.len())
            .field("root_package_name", &self.data.root_package_name)
            .field("root_package_version", &self.data.root_package_version)
            .field("dynamic_linking", &self.data.dynamic_linking)
            .field("agent_types", &self.data.agent_types)
            .finish()
    }
}

impl PartialEq for ComponentMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl Eq for ComponentMetadata {}

impl Serialize for ComponentMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.data.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ComponentMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = ComponentMetadataInnerData::deserialize(deserializer)?;
        Ok(Self {
            data: Arc::new(data),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        })
    }
}

impl Encode for ComponentMetadata {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.data.encode(encoder)
    }
}

impl<Context> Decode<Context> for ComponentMetadata {
    fn decode<D: bincode::de::Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let data = ComponentMetadataInnerData::decode(decoder)?;
        Ok(Self {
            data: Arc::new(data),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        })
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for ComponentMetadata {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let data = ComponentMetadataInnerData::borrow_decode(decoder)?;
        Ok(Self {
            data: Arc::new(data),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        })
    }
}

#[cfg(feature = "poem")]
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

#[cfg(feature = "poem")]
impl poem_openapi::types::IsObjectType for ComponentMetadata {}

#[cfg(feature = "poem")]
impl poem_openapi::types::ParseFromJSON for ComponentMetadata {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_json(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        })
    }
}

#[cfg(feature = "poem")]
impl poem_openapi::types::ToJSON for ComponentMetadata {
    fn to_json(&self) -> Option<serde_json::Value> {
        self.data.to_json()
    }
}

#[cfg(feature = "poem")]
impl poem_openapi::types::ParseFromXML for ComponentMetadata {
    fn parse_from_xml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_xml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        })
    }
}

#[cfg(feature = "poem")]
impl poem_openapi::types::ToXML for ComponentMetadata {
    fn to_xml(&self) -> Option<serde_json::Value> {
        self.data.to_xml()
    }
}

#[cfg(feature = "poem")]
impl poem_openapi::types::ParseFromYAML for ComponentMetadata {
    fn parse_from_yaml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_yaml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
            cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
        })
    }
}

#[cfg(feature = "poem")]
impl poem_openapi::types::ToYAML for ComponentMetadata {
    fn to_yaml(&self) -> Option<serde_json::Value> {
        self.data.to_yaml()
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(
    feature = "poem",
    oai(rename = "ComponentMetadata", rename_all = "camelCase")
)]
#[serde(rename = "ComponentMetadata", rename_all = "camelCase")]
pub struct ComponentMetadataInnerData {
    pub exports: Vec<AnalysedExport>,
    pub producers: Vec<Producers>,
    pub memories: Vec<LinearMemory>,
    #[serde(default)]
    pub binary_wit: Base64,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,

    #[serde(default)]
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,

    #[serde(default)]
    pub agent_types: Vec<AgentType>,
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

    pub fn find_agent_type(&self, agent_type: &str) -> Result<Option<AgentType>, String> {
        Ok(self
            .agent_types
            .iter()
            .find(|t| t.type_name == agent_type)
            .cloned())
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
                            .find(|agent| agent.type_name.to_kebab_case() == resource_name);

                        let method = agent.and_then(|agent| {
                            agent
                                .methods
                                .iter()
                                .find(|method| {
                                    method.name.to_kebab_case() == parsed.function().function_name()
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
                                .find(|agent| agent.type_name.to_kebab_case() == resource_name);

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
struct ComponentMetadataInnerCache {
    load_snapshot: Option<Result<Option<InvokableFunction>, String>>,
    save_snapshot: Option<Result<Option<InvokableFunction>, String>>,
    functions_unparsed: HashMap<String, Result<Option<InvokableFunction>, String>>,
    functions_parsed: HashMap<ParsedFunctionName, Result<Option<InvokableFunction>, String>>,
    agent_types: HashMap<String, Result<Option<AgentType>, String>>,
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

    pub fn find_agent_type(
        &mut self,
        data: &ComponentMetadataInnerData,
        agent_type: &str,
    ) -> Result<Option<AgentType>, String> {
        if let Some(cached) = self.agent_types.get(agent_type) {
            cached.clone()
        } else {
            let result = data.find_agent_type(agent_type);
            self.agent_types
                .insert(agent_type.to_string(), result.clone());
            result
        }
    }
}

#[derive(Debug, Clone)]
pub enum AgentMethodOrConstructor {
    Method(AgentMethod),
    Constructor(AgentConstructor),
}

/// Describes an exported function that can be invoked on a worker
#[derive(Debug, Clone)]
pub struct InvokableFunction {
    pub name: ParsedFunctionName,
    pub analysed_export: AnalysedFunction,
    pub agent_method_or_constructor: Option<AgentMethodOrConstructor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum DynamicLinkedInstance {
    WasmRpc(DynamicLinkedWasmRpc),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DynamicLinkedWasmRpc {
    /// Maps resource names within the dynamic linked interface to target information
    pub targets: HashMap<String, WasmRpcTarget>,
}

impl DynamicLinkedWasmRpc {
    pub fn target(&self, stub_resource: &str) -> Result<WasmRpcTarget, String> {
        self.targets.get(stub_resource).cloned().ok_or_else(|| {
            format!("Resource '{stub_resource}' not found in dynamic linked interface")
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WasmRpcTarget {
    pub interface_name: String,
    pub component_name: String,
    pub component_type: ComponentType,
}

impl WasmRpcTarget {
    pub fn site(&self) -> Result<ParsedFunctionSite, String> {
        ParsedFunctionSite::parse(&self.interface_name)
    }
}

impl Display for WasmRpcTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{}({})",
            self.interface_name, self.component_name, self.component_type
        )
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ProducerField {
    pub name: String,
    pub values: Vec<VersionedName>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct VersionedName {
    pub name: String,
    pub version: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct Producers {
    pub fields: Vec<ProducerField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct LinearMemory {
    /// Initial size of the linear memory in bytes
    pub initial: u64,
    /// Optional maximal size of the linear memory in bytes
    pub maximum: Option<u64>,
}

impl LinearMemory {
    const PAGE_SIZE: u64 = 65536;
}

impl From<Mem> for LinearMemory {
    fn from(value: Mem) -> Self {
        Self {
            initial: value.mem_type.limits.min * LinearMemory::PAGE_SIZE,
            maximum: value
                .mem_type
                .limits
                .max
                .map(|m| m * LinearMemory::PAGE_SIZE),
        }
    }
}

/// Metadata of Component in terms of golem_wasm_ast types
#[derive(Default)]
pub struct RawComponentMetadata {
    pub exports: Vec<AnalysedExport>,
    pub producers: Vec<WasmAstProducers>,
    pub memories: Vec<Mem>,
    pub binary_wit: Vec<u8>,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

impl RawComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
    ) -> Result<RawComponentMetadata, ComponentProcessingError> {
        let component = Component::<IgnoreAllButMetadata>::from_bytes(data)
            .map_err(ComponentProcessingError::Parsing)?;

        let producers = component
            .get_all_producers()
            .into_iter()
            .collect::<Vec<_>>();

        let analysis = AnalysisContext::new(component);

        let wit_analysis =
            WitAnalysisContext::new(data).map_err(ComponentProcessingError::Analysis)?;

        let mut exports = wit_analysis
            .get_top_level_exports()
            .map_err(ComponentProcessingError::Analysis)?;
        let binary_wit = wit_analysis
            .serialized_interface_only()
            .map_err(ComponentProcessingError::Analysis)?;
        let root_package = wit_analysis.root_package_name();

        #[cfg(feature = "observability")]
        for warning in wit_analysis.warnings() {
            tracing::warn!("Wit analysis warning: {}", warning);
        }

        add_resource_drops(&mut exports);
        add_virtual_exports(&mut exports);

        let exports = exports.into_iter().collect::<Vec<_>>();

        let memories: Vec<Mem> = analysis
            .get_all_memories()
            .map_err(ComponentProcessingError::Analysis)?
            .into_iter()
            .collect();

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

    pub fn into_metadata(
        self,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        agent_types: Vec<AgentType>,
    ) -> ComponentMetadataInnerData {
        let producers = self
            .producers
            .into_iter()
            .map(|producers| producers.into())
            .collect::<Vec<_>>();

        let exports = self.exports.into_iter().collect::<Vec<_>>();

        let memories = self.memories.into_iter().map(LinearMemory::from).collect();

        ComponentMetadataInnerData {
            exports,
            producers,
            memories,
            dynamic_linking,
            binary_wit: Base64(self.binary_wit),
            root_package_name: self.root_package_name,
            root_package_version: self.root_package_version,
            agent_types,
        }
    }
}

impl From<golem_wasm_ast::metadata::Producers> for Producers {
    fn from(value: golem_wasm_ast::metadata::Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<Producers> for golem_wasm_ast::metadata::Producers {
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

impl From<golem_wasm_ast::metadata::ProducersField> for ProducerField {
    fn from(value: golem_wasm_ast::metadata::ProducersField) -> Self {
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

impl From<ProducerField> for golem_wasm_ast::metadata::ProducersField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(|value| golem_wasm_ast::metadata::VersionedName {
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

fn add_virtual_exports(exports: &mut Vec<AnalysedExport>) {
    // Some interfaces like the golem/http:incoming-handler do not exist on the component,
    // but are dynamically created by the worker executor based on other existing interfaces.

    if virtual_exports::http_incoming_handler::implements_required_interfaces(exports) {
        exports.extend(vec![
            virtual_exports::http_incoming_handler::ANALYZED_EXPORT.clone(),
        ]);
    };
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::base64::Base64;
    use crate::model::component_metadata::{
        ComponentMetadata, ComponentMetadataInnerCache, ComponentMetadataInnerData,
        DynamicLinkedInstance, DynamicLinkedWasmRpc, LinearMemory, ProducerField, Producers,
        VersionedName, WasmRpcTarget,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

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
                cache: Arc::new(Mutex::new(ComponentMetadataInnerCache::default())),
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
                dynamic_linking: value
                    .dynamic_linking
                    .into_iter()
                    .map(|(k, v)| v.try_into().map(|v| (k, v)))
                    .collect::<Result<_, _>>()?,
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
                dynamic_linking: HashMap::from_iter(
                    value
                        .dynamic_linking
                        .into_iter()
                        .map(|(k, v)| (k, v.into())),
                ),
                binary_wit: value.binary_wit.0,
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
                agent_types: value.agent_types.into_iter().map(|at| at.into()).collect(),
            }
        }
    }

    impl From<DynamicLinkedInstance>
        for golem_api_grpc::proto::golem::component::DynamicLinkedInstance
    {
        fn from(value: DynamicLinkedInstance) -> Self {
            match value {
                DynamicLinkedInstance::WasmRpc(dynamic_linked_wasm_rpc) => Self {
                    dynamic_linked_instance: Some(
                        golem_api_grpc::proto::golem::component::dynamic_linked_instance::DynamicLinkedInstance::WasmRpc(
                            dynamic_linked_wasm_rpc.into())),
                },
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::DynamicLinkedInstance>
        for DynamicLinkedInstance
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::DynamicLinkedInstance,
        ) -> Result<Self, Self::Error> {
            match value.dynamic_linked_instance {
                Some(golem_api_grpc::proto::golem::component::dynamic_linked_instance::DynamicLinkedInstance::WasmRpc(dynamic_linked_wasm_rpc)) => Ok(Self::WasmRpc(dynamic_linked_wasm_rpc.try_into()?)),
                None => Err("Missing dynamic_linked_instance".to_string()),
            }
        }
    }

    impl From<DynamicLinkedWasmRpc> for golem_api_grpc::proto::golem::component::DynamicLinkedWasmRpc {
        fn from(value: DynamicLinkedWasmRpc) -> Self {
            Self {
                targets: value
                    .targets
                    .into_iter()
                    .map(|(key, target)| (key, target.into()))
                    .collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::DynamicLinkedWasmRpc>
        for DynamicLinkedWasmRpc
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::DynamicLinkedWasmRpc,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                targets: value
                    .targets
                    .into_iter()
                    .map(|(key, target)| target.try_into().map(|target| (key, target)))
                    .collect::<Result<HashMap<_, _>, _>>()?,
            })
        }
    }

    impl From<WasmRpcTarget> for golem_api_grpc::proto::golem::component::WasmRpcTarget {
        fn from(value: WasmRpcTarget) -> Self {
            Self {
                interface_name: value.interface_name,
                component_name: value.component_name,
                component_type: golem_api_grpc::proto::golem::component::ComponentType::from(
                    value.component_type,
                ) as i32,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::WasmRpcTarget> for WasmRpcTarget {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::WasmRpcTarget,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                interface_name: value.interface_name,
                component_name: value.component_name,
                component_type: value.component_type.try_into()?,
            })
        }
    }
}
