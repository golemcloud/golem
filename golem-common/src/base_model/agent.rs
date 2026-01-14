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

use crate::base_model::component::{ComponentId, ComponentRevision};
use crate::model::Empty;
use async_trait::async_trait;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use golem_wasm_derive::{FromValue, IntoValue};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryReferenceValue {
    pub value: BinaryReference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextReferenceValue {
    pub value: TextReference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct JsonComponentModelValue {
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RegisteredAgentTypeImplementer {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue,)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RegisteredAgentType {
    pub agent_type: AgentType,
    pub implemented_by: RegisteredAgentTypeImplementer,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[repr(i32)]
pub enum AgentMode {
    Durable = 0,
    Ephemeral = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryType {
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentModelElementSchema {
    pub element_type: AnalysedType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextDescriptor {
    pub restrictions: Option<Vec<TextType>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextType {
    pub language_code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
#[cfg_attr(feature = "full", wit_transparent)]
pub struct Url {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextSource {
    pub data: String,
    pub text_type: Option<TextType>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum TextReference {
    Url(Url),
    Inline(TextSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryDescriptor {
    pub restrictions: Option<Vec<BinaryType>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinarySource {
    pub data: Vec<u8>,
    pub binary_type: BinaryType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum BinaryReference {
    Url(Url),
    Inline(BinarySource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum ElementSchema {
    ComponentModel(ComponentModelElementSchema),
    UnstructuredText(TextDescriptor),
    UnstructuredBinary(BinaryDescriptor),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchema {
    pub name: String,
    pub schema: ElementSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchemas {
    pub elements: Vec<NamedElementSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum DataSchema {
    Tuple(NamedElementSchemas),
    Multimodal(NamedElementSchemas),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentConstructor {
    pub name: Option<String>,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentMethod {
    pub name: String,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
    pub output_schema: DataSchema,
    pub http_endpoint: Vec<HttpEndpointDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentDependency {
    pub type_name: String,
    pub description: Option<String>,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution(FieldAdded("http_mount", None))))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentType {
    pub type_name: AgentTypeName,
    pub description: String,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
    pub dependencies: Vec<AgentDependency>,
    pub mode: AgentMode,
    pub http_mount: Option<HttpMountDetails>,
}

#[async_trait]
pub trait AgentTypeResolver {
    fn resolve_agent_type_by_wrapper_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentType, String>;
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, IntoValue, FromValue,
)]
#[cfg_attr(
    feature = "full",
    derive(poem_openapi::NewType, desert_rust::BinaryCodec)
)]
#[repr(transparent)]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentTypeName(pub String);

impl Display for AgentTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(poem_openapi::Union, desert_rust::BinaryCodec)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum DataValue {
    Tuple(ElementValues),
    Multimodal(NamedElementValues),
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "full", derive(IntoValue, FromValue))]
pub enum UntypedDataValue {
    Tuple(Vec<UntypedElementValue>),
    Multimodal(Vec<UntypedNamedElementValue>),
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "full", derive(IntoValue, FromValue))]
pub struct UntypedNamedElementValue {
    pub name: String,
    pub value: UntypedElementValue,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "full", derive(IntoValue, FromValue))]
pub enum UntypedElementValue {
    ComponentModel(Value),
    UnstructuredText(TextReferenceValue),
    UnstructuredBinary(BinaryReferenceValue),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct UntypedJsonNamedElementValue {
    pub name: String,
    pub value: UntypedJsonElementValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct UntypedJsonElementValues {
    pub elements: Vec<UntypedJsonElementValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct UntypedJsonNamedElementValues {
    pub elements: Vec<UntypedJsonNamedElementValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum UntypedJsonDataValue {
    Tuple(UntypedJsonElementValues),
    Multimodal(UntypedJsonNamedElementValues),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum UntypedJsonElementValue {
    ComponentModel(JsonComponentModelValue),
    UnstructuredText(TextReferenceValue),
    UnstructuredBinary(BinaryReferenceValue),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ElementValues {
    pub elements: Vec<ElementValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementValues {
    pub elements: Vec<NamedElementValue>,
}

/// Identifies a deployed, instantiated agent.
///
/// AgentId is convertible to and from string, and is used as _worker names_.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentId {
    pub agent_type: AgentTypeName,
    pub parameters: DataValue,
    pub phantom_id: Option<Uuid>,
    pub(crate) wrapper_agent_type: String,
    pub(crate) as_string: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementValue {
    pub name: String,
    pub value: ElementValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union, IntoValue)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum ElementValue {
    ComponentModel(
        #[cfg_attr(feature = "full", wit_field(convert = golem_wasm::WitValue))] ValueAndType,
    ),
    UnstructuredText(TextReference),
    UnstructuredBinary(BinaryReference),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct HttpMountDetails {
    pub path_prefix: Vec<PathSegment>,
    pub header_vars: Vec<HeaderVariable>,
    pub query_vars: Vec<QueryVariable>,
    pub auth_details: Option<AgentHttpAuthDetails>,
    pub phantom_agent: bool,
    pub cors_options: CorsOptions,
    pub webhook_suffix: Vec<PathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct HttpEndpointDetails {
    pub http_method: HttpMethod,
    pub path_suffix: Vec<PathSegment>,
    pub header_vars: Vec<HeaderVariable>,
    pub query_vars: Vec<QueryVariable>,
    pub auth_details: Option<AgentHttpAuthDetails>,
    pub cors_options: CorsOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum HttpMethod {
    #[unit_case]
    Get(Empty),
    #[unit_case]
    Put(Empty),
    #[unit_case]
    Post(Empty),
    #[unit_case]
    Delete(Empty),
    Custom(CustomHttpMethod),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
#[cfg_attr(feature = "full", wit_transparent)]
pub struct CustomHttpMethod {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct CorsOptions {
    pub allowed_patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PathSegment {
    pub concat: Vec<PathSegmentNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PathSegmentNode {
    Literal(LiteralSegment),
    SystemVariable(SystemVariableSegment),
    PathVariable(PathVariable),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
#[cfg_attr(feature = "full", wit_transparent)]
pub struct LiteralSegment {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
#[cfg_attr(feature = "full", wit_transparent)]
pub struct SystemVariableSegment {
    pub value: SystemVariable,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
pub enum SystemVariable {
    AgentType,
    AgentVersion,
}

impl Display for SystemVariable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SystemVariable::AgentType => "AgentType",
            SystemVariable::AgentVersion => "AgentVersion",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PathVariable {
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct HeaderVariable {
    pub header_name: String,
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct QueryVariable {
    pub query_param_name: String,
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentHttpAuthDetails {
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
// Meaning of the various claims: https://openid.net/specs/openid-connect-core-1_0.html#StandardClaims
pub struct AgentHttpAuthContext {
    pub sub: String,
    pub provider: String,
    pub email: String,
    pub name: String,
    pub email_verified: Option<bool>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    // Url of the user's picture or avatar
    pub picture: Option<String>,
    pub preferred_username: Option<String>,
    pub claims: String,
}
