// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
pub mod text_utils;

use crate::model::component_metadata::ComponentMetadata;
use crate::schema::adapters::value::{
    typed_schema_value_to_value_and_type, value_and_type_to_typed_schema_value,
};
use crate::schema::graph::TypedSchemaValue;
use crate::schema::render::cli_text::value_to_cli_text;
use async_trait::async_trait;
use base64::Engine;
use desert_rust::BinaryCodec;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::analysis::analysed_type::{case, str, tuple, variant};
use golem_wasm::{FromValue, IntoValue, Value, ValueAndType};
use golem_wasm_derive::{FromValue, IntoValue};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::sync::LazyLock;
use uuid::Uuid;

pub use crate::base_model::agent::*;
use crate::model::AgentId;

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

#[derive(Debug, Clone, BinaryCodec)]
#[allow(clippy::large_enum_variant)]
pub enum AgentError {
    InvalidInput(String),
    InvalidMethod(String),
    InvalidType(String),
    InvalidAgentId(String),
    CustomError(TypedSchemaValue),
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
            AgentError::CustomError(typed) => {
                let rendered = value_to_cli_text(typed.graph(), typed.root_type(), typed.value())
                    .unwrap_or_else(|_| "Unprintable error".to_string());
                write!(f, "{rendered}")
            }
        }
    }
}

// The WIT-side carrier for `CustomError` is still a `value-and-type` record,
// so the encoding for the legacy `IntoValue` / `FromValue` traits must
// continue to produce the same on-the-wire shape (matching what
// `#[derive(IntoValue, FromValue)]` would have produced for
// `CustomError(ValueAndType)`). The schema-layer payload is bridged through
// the value adapters at the boundary.
const AGENT_ERROR_CASE_INVALID_INPUT: u32 = 0;
const AGENT_ERROR_CASE_INVALID_METHOD: u32 = 1;
const AGENT_ERROR_CASE_INVALID_TYPE: u32 = 2;
const AGENT_ERROR_CASE_INVALID_AGENT_ID: u32 = 3;
const AGENT_ERROR_CASE_CUSTOM_ERROR: u32 = 4;

impl IntoValue for AgentError {
    fn into_value(self) -> Value {
        let (case_idx, case_value): (u32, Option<Box<Value>>) = match self {
            AgentError::InvalidInput(msg) => (
                AGENT_ERROR_CASE_INVALID_INPUT,
                Some(Box::new(msg.into_value())),
            ),
            AgentError::InvalidMethod(msg) => (
                AGENT_ERROR_CASE_INVALID_METHOD,
                Some(Box::new(msg.into_value())),
            ),
            AgentError::InvalidType(msg) => (
                AGENT_ERROR_CASE_INVALID_TYPE,
                Some(Box::new(msg.into_value())),
            ),
            AgentError::InvalidAgentId(msg) => (
                AGENT_ERROR_CASE_INVALID_AGENT_ID,
                Some(Box::new(msg.into_value())),
            ),
            AgentError::CustomError(typed) => {
                // `TypedSchemaValue` can carry rich scalars, unions, and
                // capability nodes that have no legacy `ValueAndType`
                // counterpart. `IntoValue` is a boundary encoder used by
                // oplog and gRPC serialization, so falling back to an
                // `InvalidType` payload is safer than panicking.
                match typed_schema_value_to_value_and_type(&typed) {
                    Ok(vat) => (
                        AGENT_ERROR_CASE_CUSTOM_ERROR,
                        Some(Box::new(vat.into_value())),
                    ),
                    Err(e) => (
                        AGENT_ERROR_CASE_INVALID_TYPE,
                        Some(Box::new(
                            format!("Invalid custom error payload: {e}").into_value(),
                        )),
                    ),
                }
            }
        };
        Value::Variant {
            case_idx,
            case_value,
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("InvalidInput", str()),
            case("InvalidMethod", str()),
            case("InvalidType", str()),
            case("InvalidAgentId", str()),
            case("CustomError", ValueAndType::get_type()),
        ])
    }
}

impl FromValue for AgentError {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match case_idx {
                AGENT_ERROR_CASE_INVALID_INPUT => {
                    let payload =
                        case_value.ok_or_else(|| "Missing payload for InvalidInput".to_string())?;
                    Ok(AgentError::InvalidInput(String::from_value(*payload)?))
                }
                AGENT_ERROR_CASE_INVALID_METHOD => {
                    let payload = case_value
                        .ok_or_else(|| "Missing payload for InvalidMethod".to_string())?;
                    Ok(AgentError::InvalidMethod(String::from_value(*payload)?))
                }
                AGENT_ERROR_CASE_INVALID_TYPE => {
                    let payload =
                        case_value.ok_or_else(|| "Missing payload for InvalidType".to_string())?;
                    Ok(AgentError::InvalidType(String::from_value(*payload)?))
                }
                AGENT_ERROR_CASE_INVALID_AGENT_ID => {
                    let payload = case_value
                        .ok_or_else(|| "Missing payload for InvalidAgentId".to_string())?;
                    Ok(AgentError::InvalidAgentId(String::from_value(*payload)?))
                }
                AGENT_ERROR_CASE_CUSTOM_ERROR => {
                    let payload =
                        case_value.ok_or_else(|| "Missing payload for CustomError".to_string())?;
                    let vat = ValueAndType::from_value(*payload)?;
                    let typed = value_and_type_to_typed_schema_value(&vat).map_err(|e| {
                        format!("Failed to promote agent error custom payload to schema layer: {e}")
                    })?;
                    Ok(AgentError::CustomError(typed))
                }
                other => Err(format!("Unknown AgentError variant index: {other}")),
            },
            other => Err(format!(
                "Expected Variant value for AgentError, got {other:?}"
            )),
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

// Strict constructors enforce the AgentId length limit at creation time and should be used
// by normal production code paths. Lenient variants skip that check so oversized ids can
// still be represented temporarily for tests and diagnostics, but they are not intended for
// general runtime use because AgentId creation still validates the final identifier length.
impl LegacyParsedAgentId {
    pub fn new(
        agent_type: AgentTypeName,
        parameters: DataValue,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, String> {
        Self::new_internal(agent_type, parameters, phantom_id, false)
    }

    pub fn new_lenient(
        agent_type: AgentTypeName,
        parameters: DataValue,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, String> {
        Self::new_internal(agent_type, parameters, phantom_id, true)
    }

    pub fn new_auto_phantom(
        agent_type: AgentTypeName,
        parameters: DataValue,
        phantom_id: Option<Uuid>,
        mode: AgentMode,
    ) -> Result<Self, String> {
        Self::new_auto_phantom_internal(agent_type, parameters, phantom_id, mode, false)
    }

    pub fn new_auto_phantom_lenient(
        agent_type: AgentTypeName,
        parameters: DataValue,
        phantom_id: Option<Uuid>,
        mode: AgentMode,
    ) -> Result<Self, String> {
        Self::new_auto_phantom_internal(agent_type, parameters, phantom_id, mode, true)
    }

    pub fn parse(s: impl AsRef<str>, resolver: impl AgentTypeResolver) -> Result<Self, String> {
        Self::parse_and_resolve_type_internal(s, resolver, false).map(|(agent_id, _)| agent_id)
    }

    pub fn parse_lenient(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
    ) -> Result<Self, String> {
        Self::parse_and_resolve_type_internal(s, resolver, true).map(|(agent_id, _)| agent_id)
    }

    pub fn parse_agent_type_name(s: &str) -> Result<AgentTypeName, String> {
        let (agent_type_name, _, _) = parse_agent_id_parts(s)?;
        Ok(AgentTypeName(agent_type_name.to_string()))
    }

    pub fn parse_and_resolve_type(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
    ) -> Result<(Self, AgentType), String> {
        Self::parse_and_resolve_type_internal(s, resolver, false)
    }

    pub fn parse_and_resolve_type_lenient(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
    ) -> Result<(Self, AgentType), String> {
        Self::parse_and_resolve_type_internal(s, resolver, true)
    }

    pub fn with_phantom_id(&self, phantom_id: Option<Uuid>) -> Result<Self, String> {
        self.with_phantom_id_internal(phantom_id, false)
    }

    pub fn with_phantom_id_lenient(&self, phantom_id: Option<Uuid>) -> Result<Self, String> {
        self.with_phantom_id_internal(phantom_id, true)
    }

    fn new_internal(
        agent_type: AgentTypeName,
        parameters: DataValue,
        phantom_id: Option<Uuid>,
        lenient: bool,
    ) -> Result<Self, String> {
        use crate::model::agent::structural_format::format_structural;

        let formatted = format_structural(&parameters).map_err(|e| e.to_string())?;
        let mut as_string = format!("{}({})", agent_type.0, formatted);
        if let Some(phantom_id) = &phantom_id {
            use std::fmt::Write;
            write!(as_string, "[{phantom_id}]").unwrap();
        }

        if !lenient {
            Self::validate_length(&as_string)?;
        }

        Ok(Self {
            agent_type,
            parameters,
            phantom_id,
            as_string,
        })
    }

    fn new_auto_phantom_internal(
        agent_type: AgentTypeName,
        parameters: DataValue,
        phantom_id: Option<Uuid>,
        mode: AgentMode,
        lenient: bool,
    ) -> Result<Self, String> {
        let phantom_id = match (mode, phantom_id) {
            (_, Some(id)) => Some(id),
            (AgentMode::Ephemeral, None) => Some(Uuid::new_v4()),
            (AgentMode::Durable, None) => None,
        };

        Self::new_internal(agent_type, parameters, phantom_id, lenient)
    }

    fn parse_and_resolve_type_internal(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
        lenient: bool,
    ) -> Result<(Self, AgentType), String> {
        use crate::model::agent::structural_format::{normalize_structural, parse_structural};

        let s = s.as_ref();
        let (agent_type_name, param_list, phantom_id_str) = parse_agent_id_parts(s)?;

        let phantom_id = phantom_id_str
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| format!("Invalid UUID in phantom ID: {e}"))?;

        let agent_type =
            resolver.resolve_agent_type_by_name(&AgentTypeName(agent_type_name.to_string()))?;
        let normalized_param_list = normalize_structural(param_list);
        let value = parse_structural(&normalized_param_list, &agent_type.constructor.input_schema)
            .map_err(|e| e.to_string())?;

        let agent_id =
            Self::new_internal(agent_type.type_name.clone(), value, phantom_id, lenient)?;
        Ok((agent_id, agent_type))
    }

    fn with_phantom_id_internal(
        &self,
        phantom_id: Option<Uuid>,
        lenient: bool,
    ) -> Result<Self, String> {
        Self::new_internal(
            self.agent_type.clone(),
            self.parameters.clone(),
            phantom_id,
            lenient,
        )
    }

    fn validate_length(as_string: &str) -> Result<(), String> {
        AgentId::validate_length(as_string)
    }

    /// Normalizes an agent ID string without requiring component metadata.
    /// Strips unnecessary whitespace by parsing WAVE values and re-emitting them compactly.
    pub fn normalize_text(s: &str) -> Result<String, String> {
        normalisation::normalize_agent_id_text(s)
    }
}

impl Display for LegacyParsedAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_string)
    }
}

#[async_trait]
impl AgentTypeResolver for &ComponentMetadata {
    fn resolve_agent_type_by_name(&self, agent_type: &AgentTypeName) -> Result<AgentType, String> {
        self.find_legacy_agent_type_by_name(agent_type)?
            .ok_or_else(|| format!("Agent type not found: {agent_type}"))
    }
}
