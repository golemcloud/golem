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

//! Agent-facing schema format: input/output schemas, named fields with source
//! annotations, and the typed identifier for an instantiated agent.
//!
//! These types describe the shape of agent constructor and method signatures
//! in terms of the recursive [`SchemaType`] model. They are intentionally
//! ordered and named on the input side, so that each input position can carry
//! both its declared schema and a [`FieldSource`] annotation that tells the
//! runtime whether the value is user-supplied or auto-injected by the host.
//!
//! Outputs are either [`OutputSchema::Unit`] (no return value) or a single
//! [`SchemaType`]. A "multimodal" output is just `Single(list<union<…>>)`
//! with `role = Multimodal` on the inner element type; no separate enum
//! case is needed.
//!
//! [`ParsedAgentId`] is the typed identifier for an instantiated agent. The
//! constructor parameters travel as a self-contained [`TypedSchemaValue`]
//! pair so receivers do not need an external schema registry to interpret
//! the value tree.

use crate::base_model::agent::AgentTypeName;
use crate::schema::graph::TypedSchemaValue;
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::SchemaType;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Input parameter list for an agent constructor or method.
///
/// The single [`InputSchema::Parameters`] case carries an ordered list of
/// [`NamedField`]s. Each field carries its name, its schema, its metadata,
/// and a [`FieldSource`] that tells the runtime where the value comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
pub enum InputSchema {
    Parameters(Vec<NamedField>),
}

impl InputSchema {
    /// Convenience: build a [`Parameters`](InputSchema::Parameters) input
    /// schema from an iterator of [`NamedField`]s.
    pub fn parameters(fields: impl IntoIterator<Item = NamedField>) -> Self {
        Self::Parameters(fields.into_iter().collect())
    }

    /// The parameter list, regardless of which `InputSchema` case this is.
    pub fn fields(&self) -> &[NamedField] {
        match self {
            Self::Parameters(fields) => fields,
        }
    }
}

/// Output schema of an agent method.
///
/// Multimodal outputs are expressed as
/// `Single(list<union<…>> with role = Multimodal)`, not as a separate enum
/// case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
pub enum OutputSchema {
    /// Method returns no value.
    Unit,
    /// Method returns exactly one value, shaped by the inner schema.
    Single(SchemaType),
}

impl OutputSchema {
    /// The schema of the returned value, if any.
    pub fn schema(&self) -> Option<&SchemaType> {
        match self {
            Self::Unit => None,
            Self::Single(ty) => Some(ty),
        }
    }
}

/// A single named field inside [`InputSchema::Parameters`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedField {
    /// Field name in the input parameter list. Unique within the enclosing
    /// [`InputSchema::Parameters`].
    pub name: String,
    /// Where the value for this field comes from at invocation time.
    #[serde(default)]
    pub source: FieldSource,
    /// Schema of the field's value.
    pub schema: SchemaType,
    /// Per-field metadata (docs, aliases, examples, deprecation, role).
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    pub metadata: MetadataEnvelope,
}

impl NamedField {
    /// Convenience: construct a user-supplied field with no metadata.
    pub fn user_supplied(name: impl Into<String>, schema: SchemaType) -> Self {
        Self {
            name: name.into(),
            source: FieldSource::UserSupplied,
            schema,
            metadata: MetadataEnvelope::default(),
        }
    }

    /// Convenience: construct an auto-injected field with no metadata.
    pub fn auto_injected(
        name: impl Into<String>,
        kind: AutoInjectedKind,
        schema: SchemaType,
    ) -> Self {
        Self {
            name: name.into(),
            source: FieldSource::AutoInjected(kind),
            schema,
            metadata: MetadataEnvelope::default(),
        }
    }
}

/// Where the value for a field comes from at invocation time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
pub enum FieldSource {
    /// The caller provides the value when invoking the constructor or method.
    #[default]
    UserSupplied,
    /// The host injects the value automatically; the caller does not provide
    /// it. The kind tells the host which value to inject.
    AutoInjected(AutoInjectedKind),
}

/// Closed enumeration of host-provided auto-injected value kinds.
///
/// Today this is limited to [`Principal`](AutoInjectedKind::Principal). New
/// kinds are added here as the auto-injection surface grows.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoInjectedKind {
    /// The authenticated principal of the calling identity.
    Principal,
}

/// Identifies a deployed, instantiated agent.
///
/// Carries the constructor parameters as a self-contained
/// [`TypedSchemaValue`] pair so receivers can interpret the parameter values
/// without an external schema registry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParsedAgentId {
    /// Agent type identifier (e.g. `"weather-agent"`).
    pub agent_type: AgentTypeName,
    /// Constructor parameters, paired with their self-contained schema graph.
    pub parameters: TypedSchemaValue,
    /// Optional phantom identifier used to disambiguate otherwise-identical
    /// agent ids; `None` when the agent type does not use phantom ids.
    #[serde(default)]
    pub phantom_id: Option<Uuid>,
}

impl ParsedAgentId {
    pub fn new(
        agent_type: AgentTypeName,
        parameters: TypedSchemaValue,
        phantom_id: Option<Uuid>,
    ) -> Self {
        Self {
            agent_type,
            parameters,
            phantom_id,
        }
    }
}

#[cfg(test)]
mod tests;
