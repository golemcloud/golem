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

use crate::base_model::account::AccountId;
use crate::base_model::component::{ComponentId, ComponentRevision};
use crate::base_model::WorkerId;
use crate::model::Empty;
use async_trait::async_trait;
use golem_wasm::agentic::unstructured_binary::{AllowedMimeTypes, UnstructuredBinary};
use golem_wasm::agentic::unstructured_text::{AllowedLanguages, UnstructuredText};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{Value, ValueAndType};
use golem_wasm_derive::{FromValue, IntoValue};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
/// RegisteredAgentType with deployment specific information
/// Deployment related information can only be safely used if it is information of the _currently deployed_ component revision.
pub struct DeployedRegisteredAgentType {
    pub agent_type: AgentType,
    pub implemented_by: RegisteredAgentTypeImplementer,
    pub webhook_prefix_authority_and_path: Option<String>,
}

impl From<DeployedRegisteredAgentType> for RegisteredAgentType {
    fn from(value: DeployedRegisteredAgentType) -> Self {
        Self {
            agent_type: value.agent_type,
            implemented_by: value.implemented_by,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[repr(i32)]
pub enum AgentMode {
    Durable = 0,
    Ephemeral = 1,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentInvocationMode {
    Await,
    Schedule,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, IntoValue, FromValue,
)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, IntoValue, FromValue,
)]
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

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, IntoValue, FromValue,
)]
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

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, IntoValue, FromValue,
)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl AgentDependency {
    pub fn normalized(mut self) -> Self {
        self.methods.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            type_name: self.type_name,
            description: self.description,
            constructor: self.constructor,
            methods: self.methods,
        }
    }
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
    pub snapshotting: Snapshotting,
}

impl AgentType {
    pub fn normalized(mut self) -> Self {
        self.methods.sort_by(|a, b| a.name.cmp(&b.name));
        self.dependencies
            .sort_by(|a, b| a.type_name.cmp(&b.type_name));

        Self {
            type_name: self.type_name,
            description: self.description,
            constructor: self.constructor,
            methods: self.methods,
            dependencies: self
                .dependencies
                .into_iter()
                .map(AgentDependency::normalized)
                .collect(),
            mode: self.mode,
            http_mount: self.http_mount,
            snapshotting: self.snapshotting,
        }
    }

    pub fn normalized_vec(mut agent_types: Vec<Self>) -> Vec<Self> {
        agent_types.sort_by(|a, b| a.type_name.cmp(&b.type_name));
        agent_types.into_iter().map(Self::normalized).collect()
    }
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

impl FromStr for AgentTypeName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl AgentTypeName {
    pub fn as_str(&self) -> &str {
        &self.0
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

impl DataValue {
    pub fn try_from_untyped_json(
        value: UntypedJsonDataValue,
        schema: DataSchema,
    ) -> Result<Self, String> {
        match (value, schema) {
            (UntypedJsonDataValue::Tuple(tuple), DataSchema::Tuple(schema)) => {
                if tuple.elements.len() != schema.elements.len() {
                    return Err("Tuple length mismatch".to_string());
                }
                Ok(DataValue::Tuple(ElementValues {
                    elements: tuple
                        .elements
                        .into_iter()
                        .zip(schema.elements)
                        .map(|(value, schema)| {
                            ElementValue::try_from_untyped_json(value, schema.schema)
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
            (UntypedJsonDataValue::Multimodal(multimodal), DataSchema::Multimodal(schema)) => {
                Ok(DataValue::Multimodal(NamedElementValues {
                    elements: multimodal
                        .elements
                        .into_iter()
                        .zip(schema.elements)
                        .map(|(value, schema)| {
                            ElementValue::try_from_untyped_json(value.value, schema.schema).map(
                                |v| NamedElementValue {
                                    name: value.name,
                                    value: v,
                                },
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
            _ => Err("Data value does not match schema".to_string()),
        }
    }

    /// Returns the DataValue as a single ComponentModel Value if possible
    ///
    /// Note that this conversion does not support unstructured binary/text and multimodal return values
    pub fn into_return_value(self) -> Option<Value> {
        match self {
            DataValue::Tuple(mut elements) if elements.elements.len() == 1 => {
                match elements.elements.remove(0) {
                    ElementValue::ComponentModel(ComponentModelElementValue { value }) => Some(value.value),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "full", derive(IntoValue, FromValue, desert_rust::BinaryCodec))]
pub enum UntypedDataValue {
    Tuple(Vec<UntypedElementValue>),
    Multimodal(Vec<UntypedNamedElementValue>),
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "full", derive(IntoValue, FromValue, desert_rust::BinaryCodec))]
pub struct UntypedNamedElementValue {
    pub name: String,
    pub value: UntypedElementValue,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "full", derive(IntoValue, FromValue, desert_rust::BinaryCodec))]
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

impl From<NamedElementValue> for UntypedJsonNamedElementValue {
    fn from(value: NamedElementValue) -> Self {
        Self {
            name: value.name,
            value: value.value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct UntypedJsonElementValues {
    pub elements: Vec<UntypedJsonElementValue>,
}

impl From<ElementValues> for UntypedJsonElementValues {
    fn from(value: ElementValues) -> Self {
        Self {
            elements: value
                .elements
                .into_iter()
                .map(UntypedJsonElementValue::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct UntypedJsonNamedElementValues {
    pub elements: Vec<UntypedJsonNamedElementValue>,
}

impl From<NamedElementValues> for UntypedJsonNamedElementValues {
    fn from(value: NamedElementValues) -> Self {
        Self {
            elements: value
                .elements
                .into_iter()
                .map(UntypedJsonNamedElementValue::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum UntypedJsonDataValue {
    Tuple(UntypedJsonElementValues),
    Multimodal(UntypedJsonNamedElementValues),
}

impl From<DataValue> for UntypedJsonDataValue {
    fn from(value: DataValue) -> Self {
        match value {
            DataValue::Tuple(elements) => UntypedJsonDataValue::Tuple(elements.into()),
            DataValue::Multimodal(elements) => UntypedJsonDataValue::Multimodal(elements.into()),
        }
    }
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

impl From<ElementValue> for UntypedJsonElementValue {
    fn from(value: ElementValue) -> Self {
        match value {
            ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
                UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                    value: value
                        .to_json_value()
                        .expect("Invalid ValueAndType in ElementValue"), // TODO: convert to TryFrom and propagate this
                })
            }
            ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
                UntypedJsonElementValue::UnstructuredText(TextReferenceValue { value })
            }
            ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
                UntypedJsonElementValue::UnstructuredBinary(BinaryReferenceValue { value })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
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
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentModelElementValue {
    #[cfg_attr(feature = "full", wit_field(convert = golem_wasm::WitValue))]
    pub value: ValueAndType,
}

#[cfg(feature = "full")]
impl golem_wasm::FromValue for ComponentModelElementValue {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 1 => {
                let wit_value =
                    <golem_wasm::WitValue as golem_wasm::FromValue>::from_value(fields.remove(0))?;
                let value: Value = wit_value.into();
                // NOTE: The type information is lost during WitValue serialization.
                // The actual type should be reconstructed from the accompanying DataSchema
                // when available (e.g., via TypedDataValue).
                Ok(ComponentModelElementValue {
                    value: ValueAndType::new(
                        value,
                        AnalysedType::Str(golem_wasm::analysis::TypeStr),
                    ),
                })
            }
            _ => Err(format!(
                "Expected Record with 1 field for ComponentModelElementValue, got {:?}",
                value
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct UnstructuredTextElementValue {
    pub value: TextReference,
    #[cfg_attr(feature = "full", wit_field(skip))]
    #[serde(default)]
    pub descriptor: TextDescriptor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct UnstructuredBinaryElementValue {
    pub value: BinaryReference,
    #[cfg_attr(feature = "full", wit_field(skip))]
    #[serde(default)]
    pub descriptor: BinaryDescriptor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union, IntoValue, FromValue)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum ElementValue {
    ComponentModel(ComponentModelElementValue),
    UnstructuredText(UnstructuredTextElementValue),
    UnstructuredBinary(UnstructuredBinaryElementValue),
}

impl ElementValue {
    pub fn try_from_untyped_json(
        value: UntypedJsonElementValue,
        schema: ElementSchema,
    ) -> Result<Self, String> {
        match (value, schema) {
            (
                UntypedJsonElementValue::ComponentModel(json_value),
                ElementSchema::ComponentModel(component_model_schema),
            ) => {
                let typ: AnalysedType = component_model_schema.element_type;
                let value_and_type = ValueAndType::parse_with_type(&json_value.value, &typ)
                    .map_err(|errors: Vec<String>| {
                        format!(
                            "Failed to parse JSON as ComponentModel value: {}",
                            errors.join(", ")
                        )
                    })?;
                Ok(ElementValue::ComponentModel(ComponentModelElementValue { value: value_and_type }))
            }
            (
                UntypedJsonElementValue::UnstructuredText(text),
                ElementSchema::UnstructuredText(descriptor),
            ) => Ok(ElementValue::UnstructuredText(UnstructuredTextElementValue { value: text.value, descriptor })),
            (
                UntypedJsonElementValue::UnstructuredBinary(binary),
                ElementSchema::UnstructuredBinary(descriptor),
            ) => Ok(ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value: binary.value, descriptor })),
            _ => Err("Element value does not match schema".to_string()),
        }
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
pub struct HttpMountDetails {
    pub path_prefix: Vec<PathSegment>,
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

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, IntoValue, FromValue,
)]
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
    Head(Empty),
    #[unit_case]
    Post(Empty),
    #[unit_case]
    Put(Empty),
    #[unit_case]
    Delete(Empty),
    #[unit_case]
    Connect(Empty),
    #[unit_case]
    Options(Empty),
    #[unit_case]
    Trace(Empty),
    #[unit_case]
    Patch(Empty),
    Custom(CustomHttpMethod),
}

#[cfg(feature = "full")]
impl TryFrom<HttpMethod> for http::Method {
    type Error = anyhow::Error;

    fn try_from(value: HttpMethod) -> Result<Self, Self::Error> {
        match value {
            HttpMethod::Get(_) => Ok(http::Method::GET),
            HttpMethod::Head(_) => Ok(http::Method::HEAD),
            HttpMethod::Post(_) => Ok(http::Method::POST),
            HttpMethod::Put(_) => Ok(http::Method::PUT),
            HttpMethod::Delete(_) => Ok(http::Method::DELETE),
            HttpMethod::Connect(_) => Ok(http::Method::CONNECT),
            HttpMethod::Options(_) => Ok(http::Method::OPTIONS),
            HttpMethod::Trace(_) => Ok(http::Method::TRACE),
            HttpMethod::Patch(_) => Ok(http::Method::PATCH),
            HttpMethod::Custom(custom) => {
                let converted = http::Method::from_bytes(custom.value.as_bytes())?;
                Ok(converted)
            }
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, IntoValue, FromValue,
)]
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
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PathSegment {
    Literal(LiteralSegment),
    SystemVariable(SystemVariableSegment),
    PathVariable(PathVariable),
    RemainingPathVariable(PathVariable),
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
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum Snapshotting {
    #[unit_case]
    Disabled(Empty),
    Enabled(SnapshottingConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(
    feature = "full",
    oai(discriminator_name = "configType", one_of = true)
)]
#[serde(tag = "configType")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum SnapshottingConfig {
    #[unit_case]
    Default(Empty),
    Periodic(SnapshottingPeriodic),
    EveryNInvocation(SnapshottingEveryNInvocation),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SnapshottingPeriodic {
    pub duration_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SnapshottingEveryNInvocation {
    pub count: u16,
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
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum Principal {
    Oidc(OidcPrincipal),
    Agent(AgentPrincipal),
    GolemUser(GolemUserPrincipal),
    #[unit_case]
    Anonymous(Empty),
}

impl Principal {
    pub fn anonymous() -> Self {
        Self::Anonymous(Empty {})
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
// Meaning of the various claims: https://openid.net/specs/openid-connect-core-1_0.html#StandardClaims
pub struct OidcPrincipal {
    pub sub: String,
    pub issuer: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub email_verified: Option<bool>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub picture: Option<String>,
    pub preferred_username: Option<String>,
    pub claims: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentPrincipal {
    pub agent_id: WorkerId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct GolemUserPrincipal {
    pub account_id: AccountId,
}

pub trait UnstructuredTextExtensions {
    fn into_text_reference(self) -> TextReference;
    fn from_text_reference(text_ref: TextReference) -> Result<Self, String>
    where
        Self: Sized;
}

pub trait UnstructuredBinaryExtensions {
    fn into_binary_reference(self) -> BinaryReference;
    fn from_binary_reference(binary_ref: BinaryReference) -> Result<Self, String>
    where
        Self: Sized;
}

impl<LC: AllowedLanguages> UnstructuredTextExtensions for UnstructuredText<LC> {
    fn into_text_reference(self) -> TextReference {
        match self {
            UnstructuredText::Url(url) => TextReference::Url(Url { value: url }),
            UnstructuredText::Text {
                text,
                language_code,
            } => TextReference::Inline(TextSource {
                data: text,
                text_type: language_code.map(|lc| TextType {
                    language_code: lc.to_language_code().to_string(),
                }),
            }),
        }
    }

    fn from_text_reference(text_ref: TextReference) -> Result<Self, String> {
        match text_ref {
            TextReference::Url(url) => Ok(UnstructuredText::Url(url.value)),
            TextReference::Inline(source) => {
                let language_code =
                    if let Some(tt) = &source.text_type {
                        Some(LC::from_language_code(&tt.language_code).ok_or_else(|| {
                            format!("Invalid language code: {}", tt.language_code)
                        })?)
                    } else {
                        None
                    };
                Ok(UnstructuredText::Text {
                    text: source.data,
                    language_code,
                })
            }
        }
    }
}

impl<MT: AllowedMimeTypes> UnstructuredBinaryExtensions for UnstructuredBinary<MT> {
    fn into_binary_reference(self) -> BinaryReference {
        match self {
            UnstructuredBinary::Url(url) => BinaryReference::Url(Url { value: url }),
            UnstructuredBinary::Inline { data, mime_type } => {
                BinaryReference::Inline(BinarySource {
                    data,
                    binary_type: BinaryType {
                        mime_type: mime_type.to_string(),
                    },
                })
            }
        }
    }

    fn from_binary_reference(binary_ref: BinaryReference) -> Result<Self, String> {
        match binary_ref {
            BinaryReference::Url(url) => Ok(UnstructuredBinary::Url(url.value)),
            BinaryReference::Inline(source) => MT::from_string(&source.binary_type.mime_type)
                .ok_or_else(|| format!("Invalid mime type: {}", source.binary_type.mime_type))
                .map(|mime_type| UnstructuredBinary::Inline {
                    data: source.data,
                    mime_type,
                }),
        }
    }
}
