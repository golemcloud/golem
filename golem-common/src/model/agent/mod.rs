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

#[cfg(feature = "protobuf")]
mod protobuf;

mod bindings {
    wasmtime::component::bindgen!({
          path: "wit",
          world: "golem-common",
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
use serde::{Deserialize, Serialize};

// NOTE: The primary reason for duplicating the model with handwritten Rust types is to avoid the need
// to work with WitValue and WitType directly in the application code. Instead, we are converting them
// to Value and AnalysedType which are much more ergonomic to work with.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentConstructor {
    pub name: Option<String>,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentMethod {
    pub name: String,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
    pub output_schema: DataSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentType {
    pub type_name: String,
    pub description: String,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
    pub dependencies: Vec<AgentDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryDescriptor {
    pub restrictions: Option<Vec<BinaryType>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub enum BinaryReference {
    Url(String),
    Inline(BinarySource),
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinarySource {
    pub data: Vec<u8>,
    pub binary_type: BinaryType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryType {
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchema {
    pub name: String,
    pub schema: ElementSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchemas {
    pub elements: Vec<NamedElementSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum DataSchema {
    Tuple(NamedElementSchemas),
    Multimodal(NamedElementSchemas),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum ElementSchema {
    ComponentModel(AnalysedType),
    UnstructuredText(TextDescriptor),
    UnstructuredBinary(BinaryDescriptor),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextDescriptor {
    pub restrictions: Option<Vec<TextType>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
pub enum TextReference {
    Url(String),
    Inline(TextSource),
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextSource {
    pub data: String,
    pub text_type: Option<TextType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextType {
    pub language_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentTypes {
    pub types: Vec<AgentType>,
}
