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
mod normalisation;
mod protobuf;

#[cfg(test)]
mod tests;

pub mod schema_evolution;
pub mod structural_format;

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

use crate::model::component_metadata::ComponentMetadata;
use async_trait::async_trait;
use base64::Engine;
use desert_rust::BinaryCodec;
use golem_wasm::analysis::analysed_type::{case, str, tuple, variant};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{print_value_and_type, FromValue, IntoValue, Value, ValueAndType};
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

impl Display for Url {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
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

impl ParsedAgentId {
    pub fn new(agent_type: AgentTypeName, parameters: DataValue, phantom_id: Option<Uuid>) -> Result<Self, String> {
        use crate::model::agent::structural_format::format_structural;

        let formatted = format_structural(&parameters).map_err(|e| e.to_string())?;
        let mut as_string = format!("{}({})", agent_type.0, formatted);
        if let Some(phantom_id) = &phantom_id {
            use std::fmt::Write;
            write!(as_string, "[{phantom_id}]").unwrap();
        }
        Ok(Self {
            agent_type,
            parameters,
            phantom_id,
            as_string,
        })
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
        use crate::model::agent::structural_format::{
            format_structural, normalize_structural, parse_structural,
        };

        let s = s.as_ref();
        let (agent_type_name, param_list, phantom_id_str) = parse_agent_id_parts(s)?;

        let phantom_id = phantom_id_str
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| format!("Invalid UUID in phantom ID: {e}"))?;

        let agent_type = resolver
            .resolve_agent_type_by_name(&AgentTypeName(agent_type_name.to_string()))?;
        let normalized_param_list = normalize_structural(param_list);
        let value = parse_structural(&normalized_param_list, &agent_type.constructor.input_schema)
            .map_err(|e| e.to_string())?;

        let formatted = format_structural(&value).map_err(|e| e.to_string())?;
        let mut as_string = format!("{}({})", agent_type.type_name.0, formatted);
        if let Some(phantom_id) = &phantom_id {
            use std::fmt::Write;
            write!(as_string, "[{phantom_id}]").unwrap();
        }

        let agent_id = ParsedAgentId {
            agent_type: agent_type.type_name.clone(),
            parameters: value,
            phantom_id,
            as_string,
        };
        Ok((agent_id, agent_type))
    }

    pub fn with_phantom_id(&self, phantom_id: Option<Uuid>) -> Self {
        use crate::model::agent::structural_format::format_structural;
        use std::fmt::Write;

        let formatted = format_structural(&self.parameters).unwrap_or_default();
        let mut as_string = format!("{}({})", self.agent_type.0, formatted);
        if let Some(ref id) = phantom_id {
            write!(as_string, "[{id}]").unwrap();
        }
        Self {
            agent_type: self.agent_type.clone(),
            parameters: self.parameters.clone(),
            phantom_id,
            as_string,
        }
    }

    /// Normalizes an agent ID string without requiring component metadata.
    /// Strips unnecessary whitespace by parsing WAVE values and re-emitting them compactly.
    pub fn normalize_text(s: &str) -> Result<String, String> {
        normalisation::normalize_agent_id_text(s)
    }
}

impl Display for ParsedAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_string)
    }
}

#[async_trait]
impl AgentTypeResolver for &ComponentMetadata {
    fn resolve_agent_type_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentType, String> {
        let result = self.find_agent_type_by_name(agent_type)?;
        result.ok_or_else(|| format!("Agent type not found: {agent_type}"))
    }
}
