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

pub mod extraction;
mod normalisation;
mod protobuf;

#[cfg(test)]
mod tests;

pub mod schema_evolution;
pub mod structural_format;
pub mod text_utils;

use crate::model::component_metadata::ComponentMetadata;
use crate::schema::AgentTypeSchema;
use crate::schema::graph::TypedSchemaValue;
use crate::schema::render::cli_text::value_to_cli_text;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use base64::Engine;
use desert_rust::BinaryCodec;
use regex::Regex;
use std::fmt::{Display, Formatter};
use std::sync::LazyLock;
use uuid::Uuid;

pub use crate::base_model::agent::*;
use crate::model::AgentId;
pub use crate::schema::agent::ParsedAgentId;

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

impl Display for Url {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

pub trait AgentTypeSchemaResolver {
    fn resolve_agent_type_schema_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentTypeSchema, String>;
}

impl AgentTypeSchemaResolver for &ComponentMetadata {
    fn resolve_agent_type_schema_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentTypeSchema, String> {
        self.find_agent_type_by_name(agent_type)
            .ok_or_else(|| format!("Agent type not found: {agent_type}"))
    }
}

impl ParsedAgentId {
    pub fn try_new(
        agent_type: AgentTypeName,
        parameters: TypedSchemaValue,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, String> {
        let parsed = Self::new(agent_type, parameters, phantom_id);
        AgentId::validate_length(&parsed.to_string())?;
        Ok(parsed)
    }

    pub fn new_auto_phantom(
        agent_type: AgentTypeName,
        parameters: TypedSchemaValue,
        phantom_id: Option<Uuid>,
        mode: AgentMode,
    ) -> Result<Self, String> {
        let phantom_id = match (mode, phantom_id) {
            (_, Some(id)) => Some(id),
            (AgentMode::Ephemeral, None) => Some(Uuid::new_v4()),
            (AgentMode::Durable, None) => None,
        };
        Self::try_new(agent_type, parameters, phantom_id)
    }

    pub fn parse(
        s: impl AsRef<str>,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<Self, String> {
        Self::parse_and_resolve_type(s, resolver).map(|(agent_id, _)| agent_id)
    }

    pub fn parse_and_resolve_type(
        s: impl AsRef<str>,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<(Self, AgentTypeSchema), String> {
        use crate::model::agent::structural_format::{
            normalize_structural, parse_structural_typed,
        };
        use crate::schema::schema_type::NamedFieldType;

        let s = s.as_ref();
        let (agent_type_name, param_list, phantom_id_str) = parse_agent_id_parts(s)?;
        let agent_type_name = AgentTypeName(agent_type_name.to_string());
        let phantom_id = phantom_id_str
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| format!("Invalid UUID in phantom ID: {e}"))?;
        let agent_type = resolver.resolve_agent_type_schema_by_name(&agent_type_name)?;
        let root = SchemaType::record(
            agent_type
                .constructor
                .input_schema
                .fields()
                .iter()
                .map(|field| NamedFieldType {
                    name: field.name.clone(),
                    body: field.schema.clone(),
                    metadata: field.metadata.clone(),
                })
                .collect(),
        );
        let value =
            parse_structural_typed(&normalize_structural(param_list), &agent_type.schema, &root)
                .map_err(|e| e.to_string())?;
        let parameters = typed_constructor_parameters(&agent_type, value);
        let agent_id = Self::try_new(agent_type_name, parameters, phantom_id)?;
        Ok((agent_id, agent_type))
    }

    pub fn parse_agent_type_name(s: &str) -> Result<AgentTypeName, String> {
        let (agent_type_name, _, _) = parse_agent_id_parts(s)?;
        Ok(AgentTypeName(agent_type_name.to_string()))
    }

    pub fn normalize_text(s: &str) -> Result<String, String> {
        normalisation::normalize_agent_id_text(s)
    }

    pub fn with_phantom_id(&self, phantom_id: Option<Uuid>) -> Result<Self, String> {
        Self::try_new(self.agent_type.clone(), self.parameters.clone(), phantom_id)
    }
}

impl Display for ParsedAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        use crate::model::agent::structural_format::format_structural_typed;
        let rendered = format_structural_typed(&self.parameters).map_err(|_| std::fmt::Error)?;
        write!(f, "{}({rendered})", self.agent_type)?;
        if let Some(phantom_id) = self.phantom_id {
            write!(f, "[{phantom_id}]")?;
        }
        Ok(())
    }
}

pub fn typed_constructor_parameters(
    agent_type: &AgentTypeSchema,
    value: SchemaValue,
) -> TypedSchemaValue {
    use crate::schema::schema_type::NamedFieldType;

    let root = SchemaType::record(
        agent_type
            .constructor
            .input_schema
            .fields()
            .iter()
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    );
    let mut graph = agent_type.schema.clone();
    graph.root = root;
    TypedSchemaValue::new(graph, value)
}
