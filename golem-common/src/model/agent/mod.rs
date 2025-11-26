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

pub mod compact_value_formatter;
pub mod extraction;
mod protobuf;

#[cfg(test)]
mod tests;

pub mod wit_naming;

pub mod bindings {
    wasmtime::component::bindgen!({
          path: "wit",
          world: "golem-common",
          async: true,
          trappable_imports: true,
          with: {
            "golem:rpc/types": golem_wasm::golem_rpc_0_2_x::types,
          },
          wasmtime_crate: ::wasmtime
    });
}

use crate::model::agent::compact_value_formatter::ToCompactString;
use crate::model::agent::wit_naming::ToWitNaming;
use crate::model::component_metadata::ComponentMetadata;
use crate::model::ComponentId;
use async_trait::async_trait;
use base64::Engine;
use desert_rust::BinaryCodec;
use golem_wasm::analysis::analysed_type::{case, str, tuple, variant};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{
    parse_value_and_type, print_value_and_type, IntoValue, IntoValueAndType, Value, ValueAndType,
};
use golem_wasm_derive::{FromValue, IntoValue};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::LazyLock;
use uuid::Uuid;

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    Hash,
    BinaryCodec,
    Serialize,
    Deserialize,
    IntoValue,
    FromValue,
    poem_openapi::Enum,
)]
#[repr(i32)]
pub enum AgentMode {
    Durable = 0,
    Ephemeral = 1,
}

impl TryFrom<i32> for AgentMode {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AgentMode::Durable),
            1 => Ok(AgentMode::Ephemeral),
            _ => Err(format!("Unknown AgentMode: {value}")),
        }
    }
}

impl Display for AgentMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AgentMode::Durable => "Durable",
            AgentMode::Ephemeral => "Ephemeral",
        };
        write!(f, "{s}")
    }
}

impl FromStr for AgentMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Durable" => Ok(AgentMode::Durable),
            "Ephemeral" => Ok(AgentMode::Ephemeral),
            _ => Err(format!("Unknown AgentMode: {s}")),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentConstructor {
    pub name: Option<String>,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentDependency {
    pub type_name: String,
    pub description: Option<String>,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
}

#[derive(Debug, Clone, BinaryCodec, IntoValue)]
pub enum AgentError {
    InvalidInput(String),
    InvalidMethod(String),
    InvalidType(String),
    InvalidAgentId(String),
    CustomError(#[wit_field(convert = golem_wasm::WitValue)] ValueAndType),
}

impl Display for AgentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::InvalidInput(msg) => {
                write!(f, "Invalid input: {msg}")
            }
            AgentError::InvalidMethod(msg) => {
                write!(f, "Invalid method: {msg}")
            }
            AgentError::InvalidType(msg) => {
                write!(f, "Invalid type: {msg}")
            }
            AgentError::InvalidAgentId(msg) => {
                write!(f, "Invalid agent id: {msg}")
            }
            AgentError::CustomError(value_and_type) => {
                write!(
                    f,
                    "{}",
                    print_value_and_type(value_and_type).unwrap_or("Unprintable error".to_string())
                )
            }
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentMethod {
    pub name: String,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
    pub output_schema: DataSchema,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentType {
    pub type_name: String,
    pub description: String,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
    pub dependencies: Vec<AgentDependency>,
    pub mode: AgentMode,
}

impl AgentType {
    pub fn wrapper_type_name(&self) -> String {
        self.type_name.to_wit_naming()
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct BinaryDescriptor {
    pub restrictions: Option<Vec<BinaryType>>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
pub enum BinaryReference {
    Url(Url),
    Inline(BinarySource),
}

impl Display for BinaryReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryReference::Url(url) => write!(f, "{url}"),
            BinaryReference::Inline(binary_source) => write!(f, "{binary_source}"),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct BinarySource {
    pub data: Vec<u8>,
    pub binary_type: BinaryType,
}

impl Display for BinarySource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]\"{}\"",
            self.binary_type.mime_type,
            base64::engine::general_purpose::STANDARD.encode(&self.data)
        )
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct BinaryType {
    pub mime_type: String,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchema {
    pub name: String,
    pub schema: ElementSchema,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchemas {
    pub elements: Vec<NamedElementSchema>,
}

impl NamedElementSchemas {
    pub fn empty() -> Self {
        Self {
            elements: Vec::new(),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
pub enum DataSchema {
    Tuple(NamedElementSchemas),
    Multimodal(NamedElementSchemas),
}

impl DataSchema {
    pub fn is_unit(&self) -> bool {
        match self {
            DataSchema::Tuple(element_schemas) => element_schemas.elements.is_empty(),
            DataSchema::Multimodal(element_schemas) => element_schemas.elements.is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, poem_openapi::Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
pub enum DataValue {
    Tuple(ElementValues),
    Multimodal(NamedElementValues),
}

impl DataValue {
    pub fn parse(s: &str, schema: &DataSchema) -> Result<Self, String> {
        match schema {
            DataSchema::Tuple(element_schemas) => {
                let element_strings = split_top_level_commas(s);
                if element_strings.len() != element_schemas.elements.len() {
                    Err(format!(
                        "Unexpected number of parameters: got {}, expected {}",
                        element_strings.len(),
                        element_schemas.elements.len()
                    ))
                } else {
                    let mut element_values = Vec::with_capacity(element_strings.len());
                    for (s, schema) in element_strings.iter().zip(element_schemas.elements.iter()) {
                        element_values.push(ElementValue::parse(s, &schema.schema)?);
                    }
                    Ok(DataValue::Tuple(ElementValues {
                        elements: element_values,
                    }))
                }
            }
            DataSchema::Multimodal(element_schemas) => {
                let element_strings = split_top_level_commas(s);
                let mut element_values = Vec::with_capacity(element_strings.len());
                for s in element_strings {
                    if let Some((element_name, element_value)) = s.split_once('(') {
                        if let Some(element_value) = element_value.strip_suffix(')') {
                            let element_schema = element_schemas
                                .elements
                                .iter()
                                .find(|element_schema| element_schema.name == element_name)
                                .ok_or_else(|| {
                                    format!(
                                        "Unknown multimodal element name: `{}`. Should be one of {}",
                                        element_name,
                                        element_schemas.elements.iter().map(|element_schema| element_schema.name.clone()).collect::<Vec<_>>().join(", ")
                                    )
                                })?;
                            let element_value =
                                ElementValue::parse(element_value, &element_schema.schema)?;
                            element_values.push(NamedElementValue {
                                name: element_name.to_string(),
                                value: element_value,
                            })
                        } else {
                            return Err(format!(
                                "Multimodal value does not end with `)`: {s}; expected to be `name(value)`"
                            ));
                        }
                    } else {
                        return Err(format!(
                            "Invalid multimodal value: {s}; expected to be `name(value)`"
                        ));
                    }
                }
                Ok(DataValue::Multimodal(NamedElementValues {
                    elements: element_values,
                }))
            }
        }
    }
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut result = Vec::new();

    let chars = s.char_indices();
    let mut start = 0;
    let mut nesting = 0;
    let mut in_string = false;
    let mut skip_next = false;
    for (idx, ch) in chars {
        if !skip_next {
            match ch {
                ',' if !in_string => {
                    if nesting == 0 {
                        result.push(&s[start..idx]);
                        start = idx + 1;
                    }
                }
                '\\' if in_string => {
                    skip_next = true;
                }
                '"' => {
                    in_string = !in_string;
                }
                '(' | '[' | '{' if !in_string => {
                    nesting += 1;
                }
                ')' | ']' | '}' if !in_string => {
                    nesting -= 1;
                }
                _ => {}
            }
        } else {
            skip_next = false;
        }
    }
    if start < s.len() {
        result.push(&s[start..]);
    }

    result
}

impl Display for DataValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DataValue::Tuple(values) => write!(f, "{values}"),
            DataValue::Multimodal(values) => write!(f, "{values}"),
        }
    }
}

impl IntoValue for DataValue {
    fn into_value(self) -> Value {
        match self {
            DataValue::Tuple(elements) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(elements.elements.into_value())),
            },
            DataValue::Multimodal(elements) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(elements.elements.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("tuple", Vec::<ElementValue>::get_type()),
            case("multimodal", Vec::<NamedElementValue>::get_type()),
        ])
    }
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, IntoValue, poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct ElementValues {
    pub elements: Vec<ElementValue>,
}

impl Display for ElementValues {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.elements
                .iter()
                .map(|element_value| element_value.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, IntoValue, poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NamedElementValues {
    pub elements: Vec<NamedElementValue>,
}

impl Display for NamedElementValues {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.elements
                .iter()
                .map(|element_value| element_value.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, poem_openapi::Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NamedElementValue {
    pub name: String,
    pub value: ElementValue,
}

impl Display for NamedElementValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
    }
}

impl IntoValue for NamedElementValue {
    fn into_value(self) -> Value {
        Value::Tuple(vec![self.name.into_value(), self.value.into_value()])
    }

    fn get_type() -> AnalysedType {
        tuple(vec![str(), ElementValue::get_type()])
    }
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, IntoValue, poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
pub enum ElementValue {
    ComponentModel(#[wit_field(convert = golem_wasm::WitValue)] ValueAndType),
    UnstructuredText(TextReference),
    UnstructuredBinary(BinaryReference),
}

impl ElementValue {
    pub fn parse(s: &str, schema: &ElementSchema) -> Result<Self, String> {
        match schema {
            ElementSchema::ComponentModel(typ) => {
                let value_and_type = parse_value_and_type(&typ.element_type, s)
                    .map_err(|e| format!("Failed to parse parameter value {s}: {e}"))?;
                Ok(ElementValue::ComponentModel(value_and_type))
            }
            ElementSchema::UnstructuredText(_) => {
                if s.starts_with('"') && s.ends_with('"') {
                    let string_value = parse_value_and_type(&str(), s)?;
                    let data = match string_value.value {
                        Value::String(data) => data,
                        _ => unreachable!(),
                    };
                    Ok(ElementValue::UnstructuredText(TextReference::Inline(
                        TextSource {
                            data,
                            text_type: None,
                        },
                    )))
                } else if s.starts_with('[') {
                    if let Some((prefix, rest)) = s.split_once(']') {
                        if rest.starts_with('"') && rest.ends_with('"') {
                            let language_code = &prefix[1..];
                            let string_value = parse_value_and_type(&str(), rest)?;
                            let data = match string_value.value {
                                Value::String(data) => data,
                                _ => unreachable!(),
                            };
                            Ok(ElementValue::UnstructuredText(TextReference::Inline(
                                TextSource {
                                    data,
                                    text_type: Some(TextType {
                                        language_code: language_code.to_string(),
                                    }),
                                },
                            )))
                        } else {
                            Err(format!("Invalid unstructured text parameter syntax: {s}"))
                        }
                    } else {
                        Err(format!("Invalid unstructured text parameter syntax: {s}"))
                    }
                } else {
                    let url = ::url::Url::parse(s)
                        .map_err(|e| format!("Failed to parse parameter value {s} as URL: {e}"))?;
                    Ok(ElementValue::UnstructuredText(TextReference::Url(Url {
                        value: url.to_string(),
                    })))
                }
            }
            ElementSchema::UnstructuredBinary(_) => {
                if s.starts_with('[') {
                    if let Some((prefix, rest)) = s.split_once(']') {
                        if rest.starts_with('"') && rest.ends_with('"') {
                            let mime_type = &prefix[1..];
                            let base64_data = &rest[1..rest.len() - 1];
                            let data = base64::engine::general_purpose::STANDARD
                                .decode(base64_data.as_bytes())
                                .map_err(|e| format!("Failed to decode base64 data: {e}"))?;
                            Ok(ElementValue::UnstructuredBinary(BinaryReference::Inline(
                                BinarySource {
                                    data,
                                    binary_type: BinaryType {
                                        mime_type: mime_type.to_string(),
                                    },
                                },
                            )))
                        } else {
                            Err(format!("Invalid unstructured text parameter syntax: {s}"))
                        }
                    } else {
                        Err(format!("Invalid unstructured text parameter syntax: {s}"))
                    }
                } else {
                    let url = ::url::Url::parse(s)
                        .map_err(|e| format!("Failed to parse parameter value {s} as URL: {e}"))?;
                    Ok(ElementValue::UnstructuredBinary(BinaryReference::Url(
                        Url {
                            value: url.to_string(),
                        },
                    )))
                }
            }
        }
    }
}

impl Display for ElementValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ElementValue::ComponentModel(value) => {
                write!(f, "{}", print_value_and_type(value).unwrap_or_default())
                // NOTE: this is expected to be always working, because we only use values in ElementValues that are printable
            }
            ElementValue::UnstructuredText(text_reference) => write!(f, "{text_reference}"),
            ElementValue::UnstructuredBinary(binary_reference) => write!(f, "{binary_reference}"),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
pub enum ElementSchema {
    ComponentModel(ComponentModelElementSchema),
    UnstructuredText(TextDescriptor),
    UnstructuredBinary(BinaryDescriptor),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct ComponentModelElementSchema {
    pub element_type: AnalysedType,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct TextDescriptor {
    pub restrictions: Option<Vec<TextType>>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Union,
)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[desert(evolution())]
pub enum TextReference {
    Url(Url),
    Inline(TextSource),
}

impl Display for TextReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TextReference::Url(url) => write!(f, "{url}"),
            TextReference::Inline(text_source) => {
                write!(f, "{text_source}")
            }
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Url {
    pub value: String,
}

impl Display for Url {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct TextSource {
    pub data: String,
    pub text_type: Option<TextType>,
}

impl Display for TextSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let encoded_data = print_value_and_type(&self.data.clone().into_value_and_type())
            .unwrap_or_else(|_| self.data.clone());
        match &self.text_type {
            None => write!(f, "{}", encoded_data),
            Some(text_type) => write!(f, "[{}]{}", text_type.language_code, encoded_data),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct TextType {
    pub language_code: String,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentTypes {
    pub types: Vec<AgentType>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    Serialize,
    Deserialize,
    IntoValue,
    FromValue,
    poem_openapi::Object,
)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct RegisteredAgentType {
    pub agent_type: AgentType,
    pub implemented_by: ComponentId,
}

/// Identifies a deployed, instantiated agent.
///
/// AgentId is convertible to and from string, and is used as _worker names_.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentId {
    pub agent_type: String,
    pub parameters: DataValue,
    pub phantom_id: Option<Uuid>,
    wrapper_agent_type: String,
}

impl AgentId {
    pub fn new(agent_type: String, parameters: DataValue, phantom_id: Option<Uuid>) -> Self {
        let wrapper_agent_type = agent_type.to_wit_naming();
        Self {
            agent_type,
            parameters,
            phantom_id,
            wrapper_agent_type,
        }
    }

    pub fn parse(s: impl AsRef<str>, resolver: impl AgentTypeResolver) -> Result<Self, String> {
        Self::parse_and_resolve_type(s, resolver).map(|(agent_id, _)| agent_id)
    }

    pub fn parse_and_resolve_type(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
    ) -> Result<(Self, AgentType), String> {
        static AGENT_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^([^(]+)\((.*)\)(?:\[([^\]]+)\])?$").expect("Invalid agent ID regex")
        });

        let s = s.as_ref();

        let captures = AGENT_ID_REGEX.captures(s).ok_or_else(|| {
            format!("Unexpected agent-id format - must be 'agent-type(...)' or 'agent-type(...)[uuid]', got: {s}")
        })?;

        let agent_type_name = captures.get(1).unwrap().as_str();
        let param_list = captures.get(2).unwrap().as_str();
        let phantom_id = captures
            .get(3)
            .map(|m| Uuid::parse_str(m.as_str()))
            .transpose()
            .map_err(|e| format!("Invalid UUID in phantom ID: {e}"))?;

        let agent_type = resolver.resolve_agent_type_by_wrapper_name(agent_type_name)?;
        let value = DataValue::parse(param_list, &agent_type.constructor.input_schema)?;

        Ok((
            AgentId {
                agent_type: agent_type.type_name.clone(),
                wrapper_agent_type: agent_type.type_name.to_wit_naming(),
                parameters: value,
                phantom_id,
            },
            agent_type,
        ))
    }

    pub fn wrapper_agent_type(&self) -> &str {
        self.wrapper_agent_type.as_str()
    }
}

impl Display for AgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({})",
            self.wrapper_agent_type,
            self.parameters.to_compact_string()
        )?;
        if let Some(phantom_id) = &self.phantom_id {
            write!(f, "[{phantom_id}]")?;
        }
        Ok(())
    }
}

#[async_trait]
pub trait AgentTypeResolver {
    fn resolve_agent_type_by_wrapper_name(&self, agent_type: &str) -> Result<AgentType, String>;
}

#[async_trait]
impl AgentTypeResolver for &ComponentMetadata {
    fn resolve_agent_type_by_wrapper_name(&self, agent_type: &str) -> Result<AgentType, String> {
        let result = self
            .find_agent_type_by_wrapper_name(agent_type)?
            .to_wit_naming();
        result.ok_or_else(|| format!("Agent type not found: {agent_type}"))
    }
}
