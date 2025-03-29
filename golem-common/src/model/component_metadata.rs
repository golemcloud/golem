// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bincode::{Decode, Encode};
use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

use crate::model::ComponentType;
use crate::{virtual_exports, SafeDisplay};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentMetadata {
    pub exports: Vec<AnalysedExport>,
    pub producers: Vec<Producers>,
    pub memories: Vec<LinearMemory>,

    #[serde(default)]
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
}

impl ComponentMetadata {
    pub fn analyse_component(data: &[u8]) -> Result<ComponentMetadata, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(raw.into())
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
    pub remote: RpcRemote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum RpcRemote {
    OpenApi(OpenApiRemote),
    Grpc(GrpcRemote),
    GolemWorker(GolemWorkerRemote),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct GolemWorkerRemote {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct GrpcRemote {
    pub metadata: Option<GrpcMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct GrpcMetadata {
    pub file_descriptor_set: Vec<u8>,
    pub package_name: String,
    pub url: String,
    pub api_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct OpenApiRemote {
    pub metadata: Option<OpenApiMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct EndpointConfig {
    pub method: String,
    pub endpoint: String,
    pub no_path_params: i32,
    pub no_query_params: i32,
    pub query_param_names: Option<Vec<String>>,
    pub has_body: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct OpenApiMetadata {
    pub endpoints: HashMap<String, EndpointConfig>,
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
        }
    }
}

// Metadata of Component in terms of golem_wasm_ast types
pub struct RawComponentMetadata {
    pub exports: Vec<AnalysedExport>,
    pub producers: Vec<WasmAstProducers>,
    pub memories: Vec<Mem>,
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

        let state = AnalysisContext::new(component);

        let mut exports = state
            .get_top_level_exports()
            .map_err(ComponentProcessingError::Analysis)?;

        add_resource_drops(&mut exports);
        add_virtual_exports(&mut exports);

        let exports = exports.into_iter().collect::<Vec<_>>();

        let memories: Vec<Mem> = state
            .get_all_memories()
            .map_err(ComponentProcessingError::Analysis)?
            .into_iter()
            .collect();

        Ok(RawComponentMetadata {
            exports,
            producers,
            memories,
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

    use crate::model::component_metadata::{
        ComponentMetadata, DynamicLinkedInstance, DynamicLinkedWasmRpc, LinearMemory,
        OpenApiMetadata, ProducerField, Producers, VersionedName, WasmRpcTarget,
    };
    use std::collections::HashMap;

    use super::{
        EndpointConfig, GolemWorkerRemote, GrpcMetadata, GrpcRemote, OpenApiRemote, RpcRemote,
    };

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
                remote: Some(value.remote.into()),
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
            match value.remote {
                Some(remote) => Ok(Self {
                    targets: value
                        .targets
                        .into_iter()
                        .map(|(key, target)| target.try_into().map(|target| (key, target)))
                        .collect::<Result<HashMap<_, _>, _>>()?,
                    remote: remote.try_into()?,
                }),
                None => Err("Missing rpc remote".to_string()),
            }
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

    impl From<golem_api_grpc::proto::golem::component::OpenApiMetadata> for OpenApiMetadata {
        fn from(value: golem_api_grpc::proto::golem::component::OpenApiMetadata) -> Self {
            let endpoints = value
                .endpoints
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        EndpointConfig {
                            method: v.method,
                            endpoint: v.endpoint,
                            no_path_params: v.no_path_params,
                            no_query_params: v.no_query_params,
                            query_param_names: Some(v.query_param_names),
                            has_body: v.has_body,
                        },
                    )
                })
                .collect();
            Self { endpoints }
        }
    }

    impl From<OpenApiMetadata> for golem_api_grpc::proto::golem::component::OpenApiMetadata {
        fn from(value: OpenApiMetadata) -> Self {
            let endpoints = value
                .endpoints
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        golem_api_grpc::proto::golem::component::EndpointConfig {
                            method: v.method,
                            endpoint: v.endpoint,
                            no_path_params: v.no_path_params,
                            no_query_params: v.no_query_params,
                            query_param_names: v.query_param_names.unwrap_or_default(),
                            has_body: v.has_body,
                        },
                    )
                })
                .collect();
            Self { endpoints }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::RpcRemote> for RpcRemote {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::component::RpcRemote,
        ) -> Result<Self, Self::Error> {
            match value.remote {
                Some(golem_api_grpc::proto::golem::component::rpc_remote::Remote::Grpc(
                    grpc_remote,
                )) => Ok(Self::Grpc(grpc_remote.into())),
                Some(golem_api_grpc::proto::golem::component::rpc_remote::Remote::OpenApi(
                    open_api_remote,
                )) => Ok(Self::OpenApi(open_api_remote.into())),
                Some(golem_api_grpc::proto::golem::component::rpc_remote::Remote::GolemWorker(
                    golem_worker_remote,
                )) => Ok(Self::GolemWorker(golem_worker_remote.into())),
                None => Err("Missing rpc remote".to_string()),
            }
        }
    }

    impl From<RpcRemote> for golem_api_grpc::proto::golem::component::RpcRemote {
        fn from(value: RpcRemote) -> Self {
            match value {
                RpcRemote::Grpc(grpc_remote) => Self {
                    remote: Some(
                        golem_api_grpc::proto::golem::component::rpc_remote::Remote::Grpc(
                            grpc_remote.into(),
                        ),
                    ),
                },
                RpcRemote::OpenApi(open_api_remote) => Self {
                    remote: Some(
                        golem_api_grpc::proto::golem::component::rpc_remote::Remote::OpenApi(
                            open_api_remote.into(),
                        ),
                    ),
                },
                RpcRemote::GolemWorker(golem_worker_remote) => Self {
                    remote: Some(
                        golem_api_grpc::proto::golem::component::rpc_remote::Remote::GolemWorker(
                            golem_worker_remote.into(),
                        ),
                    ),
                },
            }
        }
    }

    impl From<GrpcRemote> for golem_api_grpc::proto::golem::component::GrpcRemote {
        fn from(value: GrpcRemote) -> Self {
            let Some(metadata) = value.metadata else {
                return Self { metadata: None };
            };
            Self {
                metadata: Some(metadata.into()),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::GrpcRemote> for GrpcRemote {
        fn from(value: golem_api_grpc::proto::golem::component::GrpcRemote) -> Self {
            let Some(metadata) = value.metadata else {
                return Self { metadata: None };
            };
            Self {
                metadata: Some(metadata.into()),
            }
        }
    }

    impl From<OpenApiRemote> for golem_api_grpc::proto::golem::component::OpenApiRemote {
        fn from(value: OpenApiRemote) -> Self {
            let Some(metadata) = value.metadata else {
                return Self { metadata: None };
            };
            Self {
                metadata: Some(metadata.into()),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::OpenApiRemote> for OpenApiRemote {
        fn from(value: golem_api_grpc::proto::golem::component::OpenApiRemote) -> Self {
            let Some(metadata) = value.metadata else {
                return Self { metadata: None };
            };
            Self {
                metadata: Some(metadata.into()),
            }
        }
    }

    impl From<GolemWorkerRemote> for golem_api_grpc::proto::golem::component::GolemWorkerRemote {
        fn from(_: GolemWorkerRemote) -> Self {
            Self {}
        }
    }

    impl From<golem_api_grpc::proto::golem::component::GolemWorkerRemote> for GolemWorkerRemote {
        fn from(_: golem_api_grpc::proto::golem::component::GolemWorkerRemote) -> Self {
            Self {}
        }
    }

    impl From<golem_api_grpc::proto::golem::component::GrpcMetadata> for GrpcMetadata {
        fn from(value: golem_api_grpc::proto::golem::component::GrpcMetadata) -> Self {
            Self {
                file_descriptor_set: value.file_descriptor_set,
                package_name: value.package_name,
                url: value.url,
                api_token: value.api_token,
            }
        }
    }

    impl From<GrpcMetadata> for golem_api_grpc::proto::golem::component::GrpcMetadata {
        fn from(value: GrpcMetadata) -> Self {
            Self {
                file_descriptor_set: value.file_descriptor_set,
                package_name: value.package_name,
                url: value.url,
                api_token: value.api_token,
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
