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

mod conversions;
pub mod extraction;
pub mod moonbit;
pub mod wit;

#[cfg(test)]
pub mod test;

// Golem Agent types
// NOTE: this is going to be moved to golem-common

mod bindings {
    wasmtime::component::bindgen!({
          path: "wit",
          world: "golem-cli",
          async: true,
          with: {
            "golem:rpc/types": golem_wasm_rpc::golem_rpc_0_2_x::types,
          },
          wasmtime_crate: ::wasmtime
    });
}

use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::Value;
use golem_wasm_rpc_derive::IntoValue;
use serde_derive::{Deserialize, Serialize};

// NOTE: The primary reason for duplicating the model with handwritten Rust types is to avoid the need
// to work with WitValue and WitType directly in the application code. Instead, we are converting them
// to Value and AnalysedType which are much more ergonomic to work with.

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct AgentConstructor {
    pub name: Option<String>,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct AgentDependency {
    pub type_name: String,
    pub description: Option<String>,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
}

#[derive(Debug, Clone, Encode, Decode, IntoValue)]
pub enum AgentError {
    InvalidInput(String),
    InvalidMethod(String),
    InvalidType(String),
    InvalidAgentId(String),
    CustomError(DataValue),
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct AgentMethod {
    pub name: String,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
    pub output_schema: DataSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct AgentType {
    pub type_name: String,
    pub description: String,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
    pub dependencies: Vec<AgentDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct BinaryDescriptor {
    pub restrictions: Option<Vec<BinaryType>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub enum BinaryReference {
    Url(String),
    Inline(BinarySource),
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct BinarySource {
    pub data: Vec<u8>,
    pub binary_type: BinaryType,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct BinaryType {
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct NamedElementSchema {
    pub name: String,
    pub schema: ElementSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub enum DataSchema {
    Tuple(Vec<NamedElementSchema>),
    Multimodal(Vec<NamedElementSchema>),
}

#[derive(Debug, Clone, Encode, Decode, IntoValue)]
pub enum DataValue {
    Tuple(Vec<ElementValue>),
    Multimodal(Vec<NamedElementValue>),
}

#[derive(Debug, Clone, Encode, Decode, IntoValue)]
pub struct NamedElementValue {
    pub name: String,
    pub value: ElementValue,
}

#[derive(Debug, Clone, Encode, Decode, IntoValue)]
pub enum ElementValue {
    ComponentModel(Value),
    UnstructuredText(TextReference),
    UnstructuredBinary(BinaryReference),
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub enum ElementSchema {
    ComponentModel(AnalysedType),
    UnstructuredText(TextDescriptor),
    UnstructuredBinary(BinaryDescriptor),
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct TextDescriptor {
    pub restrictions: Option<Vec<TextType>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub enum TextReference {
    Url(String),
    Inline(TextSource),
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct TextSource {
    pub data: String,
    pub text_type: Option<TextType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub struct TextType {
    pub language_code: String,
}
