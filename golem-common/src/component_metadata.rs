// Copyright 2024 Golem Cloud
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
use std::fmt::{self, Display, Formatter};

use crate::exports::Export;
use golem_wasm_ast::analysis::AnalysedFunctionParameter;
use golem_wasm_ast::core::Mem;
use golem_wasm_ast::metadata::Producers as WasmAstProducers;
use golem_wasm_ast::{
    analysis::{AnalysedExport, AnalysedFunction, AnalysisContext, AnalysisFailure},
    component::Component,
    IgnoreAllButMetadata,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, Encode, Decode)]
pub struct ComponentMetadata {
    pub exports: Vec<Export>,
    pub producers: Vec<Producers>,
    pub memories: Vec<LinearMemory>,
}

impl ComponentMetadata {
    pub fn analyse_component(data: &[u8]) -> Result<ComponentMetadata, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(raw.into())
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
    Object,
    Encode,
    Decode,
)]

pub struct ProducerField {
    pub name: String,
    pub values: Vec<VersionedName>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
    Object,
    Encode,
    Decode,
)]
pub struct VersionedName {
    pub name: String,
    pub version: String,
}

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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
    Object,
    Encode,
    Decode,
)]
pub struct Producers {
    pub fields: Vec<ProducerField>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, Encode, Decode)]
pub struct LinearMemory {
    /// Initial size of the linear memory in bytes
    pub initial: u64,
    /// Optional maximal size of the linear memory in bytes
    pub maximum: Option<u64>,
}

impl LinearMemory {
    const PAGE_SIZE: u64 = 65536;
}

impl From<golem_wasm_ast::core::Mem> for LinearMemory {
    fn from(value: golem_wasm_ast::core::Mem) -> Self {
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

impl From<RawComponentMetadata> for ComponentMetadata {
    fn from(value: RawComponentMetadata) -> Self {
        let producers = value
            .producers
            .into_iter()
            .map(|producers| producers.into())
            .collect::<Vec<_>>();

        let exports = value
            .exports
            .into_iter()
            .map(|export| export.into())
            .collect::<Vec<_>>();

        let memories = value.memories.into_iter().map(LinearMemory::from).collect();

        ComponentMetadata {
            exports,
            producers,
            memories,
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

#[derive(Debug, thiserror::Error)]
pub enum ComponentProcessingError {
    Parsing(String),
    Analysis(AnalysisFailure),
}

impl Display for ComponentProcessingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ComponentProcessingError::Parsing(e) => write!(f, "Parsing error: {}", e),
            ComponentProcessingError::Analysis(source) => {
                let AnalysisFailure::Failed(error) = source;
                write!(f, "Analysis error: {}", error)
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
                for fun in &instance.funcs {
                    if fun.is_constructor() {
                        to_add.push(drop_from_constructor(fun));
                    }
                }
                instance.funcs.extend(to_add.into_iter());
            }
        }
    }

    exports.extend(to_add);
}

fn drop_from_constructor(constructor: &AnalysedFunction) -> AnalysedFunction {
    let drop_name = constructor.name.replace("[constructor]", "[drop]");
    AnalysedFunction {
        name: drop_name,
        params: constructor
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
