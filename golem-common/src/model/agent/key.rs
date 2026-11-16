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

use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};

use super::structural_format::{
    StructuralFormatError, format_structural_typed, normalize_structural, parse_structural_typed,
};
use super::{AgentTypeName, AgentTypeSchemaResolver, parse_agent_id_parts};
use crate::schema::AgentTypeSchema;
use crate::schema::graph::TypedSchemaValue;

/// An agent type plus its constructor parameter values: the identity of a keyed agent.
///
/// Its canonical text is `Type(params)`, where `params` is the canonical agent key text
/// of [`super::structural_format`]. Two keys are equal when their type and canonical
/// parameter text are equal, so the order in which a map's entries were built does not
/// matter.
#[derive(Debug, Clone)]
pub struct AgentKey {
    agent_type: AgentTypeName,
    parameters: TypedSchemaValue,
    canonical_params: String,
}

impl AgentKey {
    /// Fails when the parameters have no canonical text, for example a map with
    /// duplicate keys or a value whose payload does not match its case.
    pub fn new(
        agent_type: AgentTypeName,
        parameters: TypedSchemaValue,
    ) -> Result<Self, StructuralFormatError> {
        let canonical_params = format_structural_typed(&parameters)?;
        Ok(Self {
            agent_type,
            parameters,
            canonical_params,
        })
    }

    /// Parses `Type(params)` text. Whitespace outside literals is ignored, and the
    /// parameters may be in any non-canonical form the parser accepts, such as an
    /// unsorted map.
    pub fn parse(
        s: &str,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<(Self, AgentTypeSchema), String> {
        let (agent_type_name, param_list, phantom_id) = parse_agent_id_parts(s)?;
        if phantom_id.is_some() {
            return Err(format!("An agent key has no phantom ID suffix, got: {s}"));
        }
        Self::parse_parts(agent_type_name, param_list, resolver)
    }

    pub(super) fn parse_parts(
        agent_type_name: &str,
        param_list: &str,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<(Self, AgentTypeSchema), String> {
        let agent_type_name = AgentTypeName(agent_type_name.to_string());
        let agent_type = resolver.resolve_agent_type_schema_by_name(&agent_type_name)?;
        let root = super::constructor_parameter_record(&agent_type);
        let value =
            parse_structural_typed(&normalize_structural(param_list), &agent_type.schema, &root)
                .map_err(|e| e.to_string())?;
        let parameters = super::typed_constructor_parameters(&agent_type, value);
        let key = Self::new(agent_type_name, parameters).map_err(|e| e.to_string())?;
        Ok((key, agent_type))
    }

    pub fn agent_type(&self) -> &AgentTypeName {
        &self.agent_type
    }

    pub fn parameters(&self) -> &TypedSchemaValue {
        &self.parameters
    }

    /// The canonical parameter text without the agent type name: the hash input of a
    /// keyed agent ID.
    pub fn canonical_params(&self) -> &str {
        &self.canonical_params
    }

    pub fn into_parts(self) -> (AgentTypeName, TypedSchemaValue) {
        (self.agent_type, self.parameters)
    }
}

impl PartialEq for AgentKey {
    fn eq(&self, other: &Self) -> bool {
        self.agent_type == other.agent_type && self.canonical_params == other.canonical_params
    }
}

impl Eq for AgentKey {}

impl Hash for AgentKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.agent_type.hash(state);
        self.canonical_params.hash(state);
    }
}

impl Display for AgentKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.agent_type, self.canonical_params)
    }
}
