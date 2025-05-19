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

use crate::model::base64::Base64;
use crate::model::ComponentType;
use crate::{virtual_exports, SafeDisplay};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::wit_parser::WitAnalysisContext;
use golem_wasm_ast::analysis::AnalysedFunctionParameter;
use golem_wasm_ast::core::Mem;
use golem_wasm_ast::metadata::Producers as WasmAstProducers;
use golem_wasm_ast::{
    analysis::{AnalysedExport, AnalysedFunction, AnalysisContext, AnalysisFailure},
    component::Component,
    IgnoreAllButMetadata,
};
use rib::ParsedFunctionSite;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{self, Debug, Display, Formatter};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentMetadata {
    pub exports: Vec<AnalysedExport>,
    pub producers: Vec<Producers>,
    pub memories: Vec<LinearMemory>,
    #[serde(default)]
    pub binary_wit: Base64,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,

    #[serde(default)]
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
}

impl ComponentMetadata {
    pub fn analyse_component(data: &[u8]) -> Result<ComponentMetadata, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(raw.into())
    }
}

impl Debug for ComponentMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentMetadata")
            .field("exports", &self.exports)
            .field("producers", &self.producers)
            .field("memories", &self.memories)
            .field("binary_wit_len", &self.binary_wit.len())
            .field("root_package_name", &self.root_package_name)
            .field("root_package_version", &self.root_package_version)
            .field("dynamic_linking", &self.dynamic_linking)
            .finish()
    }
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

impl From<RawComponentMetadata> for ComponentMetadata {
    fn from(value: RawComponentMetadata) -> Self {
        let producers = value
            .producers
            .into_iter()
            .map(|producers| producers.into())
            .collect::<Vec<_>>();

        let exports = value.exports.into_iter().collect::<Vec<_>>();

        let memories = value.memories.into_iter().map(LinearMemory::from).collect();

        ComponentMetadata {
            exports,
            producers,
            memories,
            dynamic_linking: HashMap::new(),
            binary_wit: Base64(value.binary_wit),
            root_package_name: value.root_package_name,
            root_package_version: value.root_package_version,
        }
    }
}

// Metadata of Component in terms of golem_wasm_ast types
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
            ComponentProcessingError::Parsing(e) => write!(f, "Parsing error: {}", e),
            ComponentProcessingError::Analysis(source) => {
                let AnalysisFailure { reason } = source;
                write!(f, "Analysis error: {}", reason)
            }
        }
    }
}

fn add_resource_drops(exports: &mut Vec<AnalysedExport>) {
    // Components are not exporting explicit drop functions for exported resources, but
    // worker executor does. So we keep golem-wasm-ast as a universal library and extend
    // its result with the explicit drops here, for each resource, identified by an exported
    // constructor.

    let mut to_add = Vec::new();
    for export in exports.iter_mut() {
        match export {
            AnalysedExport::Function(fun) => {
                if fun.is_constructor() {
                    to_add.push(AnalysedExport::Function(drop_from_constructor(fun)));
                }
            }
            AnalysedExport::Instance(instance) => {
                let mut to_add = Vec::new();
                for fun in &instance.functions {
                    if fun.is_constructor() {
                        to_add.push(drop_from_constructor(fun));
                    }
                }
                instance.functions.extend(to_add.into_iter());
            }
        }
    }

    exports.extend(to_add);
}

fn drop_from_constructor(constructor: &AnalysedFunction) -> AnalysedFunction {
    let drop_name = constructor.name.replace("[constructor]", "[drop]");
    AnalysedFunction {
        name: drop_name,
        parameters: constructor
            .results
            .iter()
            .map(|result| AnalysedFunctionParameter {
                name: "self".to_string(),
                typ: result.typ.clone(),
            })
            .collect(),
        results: vec![],
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
        ComponentMetadata, DynamicLinkedInstance, DynamicLinkedWasmRpc, LinearMemory,
        ProducerField, Producers, VersionedName, WasmRpcTarget,
    };
    use std::collections::HashMap;

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
            })
        }
    }

    impl From<ComponentMetadata> for golem_api_grpc::proto::golem::component::ComponentMetadata {
        fn from(value: ComponentMetadata) -> Self {
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
