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

mod compact_value_formatter;
mod conversions;
pub mod extraction;
mod normalisation;
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
            "golem:core/types": golem_wasm::golem_core_1_5_x::types,
          },
          wasmtime_crate: ::wasmtime
    });
}

use crate::model::agent::compact_value_formatter::ToCompactString;
use crate::model::agent::wit_naming::ToWitNaming;
use crate::model::component_metadata::ComponentMetadata;
use async_trait::async_trait;
use base64::Engine;
use desert_rust::BinaryCodec;
use golem_wasm::analysis::analysed_type::{case, str, tuple, variant};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{
    parse_value_and_type, print_value_and_type, FromValue, IntoValue, IntoValueAndType, Value,
    ValueAndType,
};
use golem_wasm_derive::{FromValue, IntoValue};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::LazyLock;
use uuid::Uuid;

pub use crate::base_model::agent::*;

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

#[derive(Debug, Clone, BinaryCodec, IntoValue, FromValue)]
pub enum AgentError {
    InvalidInput(String),
    InvalidMethod(String),
    InvalidType(String),
    InvalidAgentId(String),
    CustomError(ValueAndType),
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

impl AgentType {
    pub fn wrapper_type_name(&self) -> String {
        self.type_name.0.to_wit_naming()
    }
}

impl Display for BinaryReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryReference::Url(url) => write!(f, "{url}"),
            BinaryReference::Inline(binary_source) => write!(f, "{binary_source}"),
        }
    }
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

impl NamedElementSchemas {
    pub fn empty() -> Self {
        Self {
            elements: Vec::new(),
        }
    }
}

impl DataSchema {
    pub fn is_unit(&self) -> bool {
        match self {
            DataSchema::Tuple(element_schemas) => element_schemas.elements.is_empty(),
            DataSchema::Multimodal(element_schemas) => element_schemas.elements.is_empty(),
        }
    }
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
                                .find(|element_schema| {
                                    element_schema.name == element_name
                                        || element_schema.name.to_wit_naming() == element_name
                                })
                                .ok_or_else(|| {
                                    format!(
                                        "Unknown multimodal element name: `{}`. Should be one of {}",
                                        element_name,
                                        element_schemas.elements.iter().map(|element_schema| element_schema.name.to_wit_naming()).collect::<Vec<_>>().join(", ")
                                    )
                                })?;
                            let element_value =
                                ElementValue::parse(element_value, &element_schema.schema)?;
                            element_values.push(NamedElementValue {
                                name: element_schema.name.clone(),
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

static AGENT_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([^(]+)\((.*)\)(?:\[([^\]]+)\])?$").expect("Invalid agent ID regex")
});

/// Parses the outer structure of an agent ID string into its components:
/// (agent_type_name, param_list, optional_phantom_uuid_str)
pub(crate) fn parse_agent_id_parts(s: &str) -> Result<(&str, &str, Option<&str>), String> {
    let captures = AGENT_ID_REGEX.captures(s.trim()).ok_or_else(|| {
        format!("Unexpected agent-id format - must be 'agent-type(...)' or 'agent-type(...)[uuid]', got: {s}")
    })?;

    let agent_type_name = captures.get(1).unwrap().as_str().trim();
    let param_list = captures.get(2).unwrap().as_str();
    let phantom_id_str = captures.get(3).map(|m| m.as_str().trim());

    if agent_type_name.is_empty() {
        return Err("Agent type name cannot be empty".to_string());
    }

    Ok((agent_type_name, param_list, phantom_id_str))
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

impl DataValue {
    pub fn extract_schema(&self) -> DataSchema {
        match self {
            DataValue::Tuple(elements) => DataSchema::Tuple(NamedElementSchemas {
                elements: elements
                    .elements
                    .iter()
                    .enumerate()
                    .map(|(i, e)| NamedElementSchema {
                        name: i.to_string(),
                        schema: e.extract_schema(),
                    })
                    .collect(),
            }),
            DataValue::Multimodal(elements) => DataSchema::Multimodal(NamedElementSchemas {
                elements: elements
                    .elements
                    .iter()
                    .map(|e| NamedElementSchema {
                        name: e.name.clone(),
                        schema: e.value.extract_schema(),
                    })
                    .collect(),
            }),
        }
    }
}

impl ElementValue {
    pub fn extract_schema(&self) -> ElementSchema {
        match self {
            ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
                ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: value.typ.clone(),
                })
            }
            ElementValue::UnstructuredText(UnstructuredTextElementValue { descriptor, .. }) => {
                ElementSchema::UnstructuredText(descriptor.clone())
            }
            ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                descriptor, ..
            }) => ElementSchema::UnstructuredBinary(descriptor.clone()),
        }
    }
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

impl IntoValue for TextReferenceValue {
    fn into_value(self) -> Value {
        self.value.into_value()
    }

    fn get_type() -> AnalysedType {
        TextReference::get_type()
    }
}

impl FromValue for TextReferenceValue {
    fn from_value(value: Value) -> Result<Self, String> {
        TextReference::from_value(value).map(|value| Self { value })
    }
}

impl IntoValue for BinaryReferenceValue {
    fn into_value(self) -> Value {
        self.value.into_value()
    }

    fn get_type() -> AnalysedType {
        BinaryReference::get_type()
    }
}

impl FromValue for BinaryReferenceValue {
    fn from_value(value: Value) -> Result<Self, String> {
        BinaryReference::from_value(value).map(|value| Self { value })
    }
}

impl ElementValue {
    pub fn parse(s: &str, schema: &ElementSchema) -> Result<Self, String> {
        match schema {
            ElementSchema::ComponentModel(typ) => {
                let mut value_and_type = parse_value_and_type(&typ.element_type.to_wit_naming(), s)
                    .map_err(|e| format!("Failed to parse parameter value {s}: {e}"))?;
                value_and_type.typ = typ.element_type.clone(); // Store the original type, not the wit-naming one
                Ok(ElementValue::ComponentModel(ComponentModelElementValue {
                    value: value_and_type,
                }))
            }
            ElementSchema::UnstructuredText(descriptor) => {
                if s.starts_with('"') && s.ends_with('"') {
                    let string_value = parse_value_and_type(&str(), s)?;
                    let data = match string_value.value {
                        Value::String(data) => data,
                        _ => unreachable!(),
                    };
                    Ok(ElementValue::UnstructuredText(
                        UnstructuredTextElementValue {
                            value: TextReference::Inline(TextSource {
                                data,
                                text_type: None,
                            }),
                            descriptor: descriptor.clone(),
                        },
                    ))
                } else if s.starts_with('[') {
                    if let Some((prefix, rest)) = s.split_once(']') {
                        if rest.starts_with('"') && rest.ends_with('"') {
                            let language_code = &prefix[1..];
                            let string_value = parse_value_and_type(&str(), rest)?;
                            let data = match string_value.value {
                                Value::String(data) => data,
                                _ => unreachable!(),
                            };
                            Ok(ElementValue::UnstructuredText(
                                UnstructuredTextElementValue {
                                    value: TextReference::Inline(TextSource {
                                        data,
                                        text_type: Some(TextType {
                                            language_code: language_code.to_string(),
                                        }),
                                    }),
                                    descriptor: descriptor.clone(),
                                },
                            ))
                        } else {
                            Err(format!("Invalid unstructured text parameter syntax: {s}"))
                        }
                    } else {
                        Err(format!("Invalid unstructured text parameter syntax: {s}"))
                    }
                } else {
                    let url = ::url::Url::parse(s)
                        .map_err(|e| format!("Failed to parse parameter value {s} as URL: {e}"))?;
                    Ok(ElementValue::UnstructuredText(
                        UnstructuredTextElementValue {
                            value: TextReference::Url(Url {
                                value: url.to_string(),
                            }),
                            descriptor: descriptor.clone(),
                        },
                    ))
                }
            }
            ElementSchema::UnstructuredBinary(descriptor) => {
                if s.starts_with('[') {
                    if let Some((prefix, rest)) = s.split_once(']') {
                        if rest.starts_with('"') && rest.ends_with('"') {
                            let mime_type = &prefix[1..];
                            let base64_data = &rest[1..rest.len() - 1];
                            let data = base64::engine::general_purpose::STANDARD
                                .decode(base64_data.as_bytes())
                                .map_err(|e| format!("Failed to decode base64 data: {e}"))?;
                            Ok(ElementValue::UnstructuredBinary(
                                UnstructuredBinaryElementValue {
                                    value: BinaryReference::Inline(BinarySource {
                                        data,
                                        binary_type: BinaryType {
                                            mime_type: mime_type.to_string(),
                                        },
                                    }),
                                    descriptor: descriptor.clone(),
                                },
                            ))
                        } else {
                            Err(format!("Invalid unstructured text parameter syntax: {s}"))
                        }
                    } else {
                        Err(format!("Invalid unstructured text parameter syntax: {s}"))
                    }
                } else {
                    let url = ::url::Url::parse(s)
                        .map_err(|e| format!("Failed to parse parameter value {s} as URL: {e}"))?;
                    Ok(ElementValue::UnstructuredBinary(
                        UnstructuredBinaryElementValue {
                            value: BinaryReference::Url(Url {
                                value: url.to_string(),
                            }),
                            descriptor: descriptor.clone(),
                        },
                    ))
                }
            }
        }
    }
}

impl Display for ElementValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
                write!(f, "{}", print_value_and_type(value).unwrap_or_default())
                // NOTE: this is expected to be always working, because we only use values in ElementValues that are printable
            }
            ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
                write!(f, "{value}")
            }
            ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
                write!(f, "{value}")
            }
        }
    }
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

impl Display for Url {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
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
pub struct AgentTypes {
    pub types: Vec<AgentType>,
}

impl AgentId {
    pub fn new(agent_type: AgentTypeName, parameters: DataValue, phantom_id: Option<Uuid>) -> Self {
        let wrapper_agent_type = agent_type.to_wit_naming().0;
        let mut result = Self {
            agent_type,
            parameters,
            phantom_id,
            wrapper_agent_type,
            as_string: "".to_string(),
        };
        result.as_string = result.to_string();
        result
    }

    pub fn parse(s: impl AsRef<str>, resolver: impl AgentTypeResolver) -> Result<Self, String> {
        Self::parse_and_resolve_type(s, resolver).map(|(agent_id, _)| agent_id)
    }

    pub fn parse_agent_type_name(s: &str) -> Result<AgentTypeName, String> {
        let (agent_type_name, _, _) = parse_agent_id_parts(s)?;
        Ok(AgentTypeName(agent_type_name.to_string()))
    }

    pub fn parse_and_resolve_type(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
    ) -> Result<(Self, AgentType), String> {
        let s = s.as_ref();
        let (agent_type_name, param_list, phantom_id_str) = parse_agent_id_parts(s)?;

        let phantom_id = phantom_id_str
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| format!("Invalid UUID in phantom ID: {e}"))?;

        let agent_type = resolver
            .resolve_agent_type_by_wrapper_name(&AgentTypeName(agent_type_name.to_string()))?;
        let value = DataValue::parse(param_list, &agent_type.constructor.input_schema)?;

        let mut agent_id = AgentId {
            agent_type: agent_type.type_name.clone(),
            wrapper_agent_type: agent_type.type_name.to_wit_naming().0,
            parameters: value,
            phantom_id,
            as_string: "".to_string(),
        };
        agent_id.as_string = agent_id.to_string();
        Ok((agent_id, agent_type))
    }

    pub fn wrapper_agent_type(&self) -> &str {
        self.wrapper_agent_type.as_str()
    }

    pub fn with_phantom_id(&self, phantom_id: Option<Uuid>) -> Self {
        Self {
            phantom_id,
            ..self.clone()
        }
    }

    /// Normalizes an agent ID string without requiring component metadata.
    /// Strips unnecessary whitespace by parsing WAVE values and re-emitting them compactly.
    pub fn normalize_text(s: &str) -> Result<String, String> {
        normalisation::normalize_agent_id_text(s)
    }
}

impl Display for AgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.as_string.is_empty() {
            write!(
                f,
                "{}({})",
                self.wrapper_agent_type,
                self.parameters.to_wit_naming().to_compact_string()
            )?;
            if let Some(phantom_id) = &self.phantom_id {
                write!(f, "[{phantom_id}]")?;
            }
            Ok(())
        } else {
            write!(f, "{}", self.as_string)
        }
    }
}

#[async_trait]
impl AgentTypeResolver for &ComponentMetadata {
    fn resolve_agent_type_by_wrapper_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentType, String> {
        let result = self.find_agent_type_by_wrapper_name(agent_type)?;
        result.ok_or_else(|| format!("Agent type not found: {agent_type}"))
    }
}
