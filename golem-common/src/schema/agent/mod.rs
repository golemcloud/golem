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

use crate::base_model::agent::{
    AgentConfigSource, AgentMode, AgentTypeName, HttpEndpointDetails, HttpMountDetails,
    ReadOnlyConfig, RegisteredAgentTypeImplementer, Snapshotting,
};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::SchemaType;
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Input parameter list for an agent constructor or method.
///
/// The single [`InputSchema::Parameters`] case carries an ordered list of
/// [`NamedField`]s. Each field carries its name, its schema, its metadata,
/// and a [`FieldSource`] that tells the runtime where the value comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
/// `Single(list<variant<…>> with role = Multimodal)`, not as a separate enum
/// case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
pub enum OutputSchema {
    /// Method returns no value.
    Unit,
    /// Method returns exactly one value, shaped by the inner schema.
    Single(Box<SchemaType>),
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
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

/// Constructor signature for an agent type, schema-layer form.
///
/// Mirrors the legacy `AgentConstructor`, with `input_schema` replaced by the
/// new [`InputSchema`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentConstructorSchema {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hint: Option<String>,
    pub input_schema: InputSchema,
}

/// Method signature on an agent type, schema-layer form.
///
/// Mirrors the legacy `AgentMethod`, with `input_schema` / `output_schema`
/// replaced by [`InputSchema`] / [`OutputSchema`]. Non-schema fields
/// (`http_endpoint`, `read_only`) are carried verbatim from the legacy
/// representation during the transition; they are not part of the schema
/// layer's core concern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentMethodSchema {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hint: Option<String>,
    pub input_schema: InputSchema,
    pub output_schema: OutputSchema,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http_endpoint: Vec<HttpEndpointDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<ReadOnlyConfig>,
}

/// Dependent agent type, schema-layer form.
///
/// Owns its own [`SchemaGraph`] — a dependent agent is independently
/// published, so it brings a self-contained graph rather than sharing a
/// namespace with the parent agent. Constructor and method input/output
/// bodies may use [`SchemaType::Ref`] resolving against `schema.defs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentDependencySchema {
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Named-type registry shared by this dependency's constructor and
    /// methods. Refs in those bodies resolve against this graph, not the
    /// parent [`AgentTypeSchema::schema`]. Use [`SchemaGraph::empty`] when
    /// there are no shared definitions to declare.
    ///
    /// The graph's `root` field is a placeholder (empty record) — only
    /// `defs` is consumed by agent-layer helpers.
    #[serde(default = "SchemaGraph::empty")]
    pub schema: SchemaGraph,
    pub constructor: AgentConstructorSchema,
    pub methods: Vec<AgentMethodSchema>,
}

/// Full agent type declaration, schema-layer form.
///
/// Owns a per-agent [`SchemaGraph`] that is shared by this agent's
/// constructor and methods. Shared types across methods become
/// first-class named definitions referenced by [`SchemaType::Ref`]
/// rather than duplicated inline subtrees.
///
/// Dependencies (`AgentDependencySchema`) each carry their own
/// [`SchemaGraph`] — there is no cross-agent registry: two unrelated
/// agents that coincidentally define a same-named type do not share
/// definitions, and a dependency cannot reference defs from its parent
/// agent's graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentTypeSchema {
    pub type_name: AgentTypeName,
    pub description: String,
    #[serde(default)]
    pub source_language: String,
    /// Named-type registry shared by this agent's constructor and methods.
    /// Each [`AgentDependencySchema`] in [`dependencies`](Self::dependencies)
    /// carries its own independent registry; this field does not cover them.
    /// Use [`SchemaGraph::empty`] when there are no shared definitions to
    /// declare.
    ///
    /// The graph's `root` field is a placeholder (empty record) — only
    /// `defs` is consumed by agent-layer helpers. The real roots are the
    /// constructor and method input/output bodies.
    #[serde(default = "SchemaGraph::empty")]
    pub schema: SchemaGraph,
    pub constructor: AgentConstructorSchema,
    pub methods: Vec<AgentMethodSchema>,
    pub dependencies: Vec<AgentDependencySchema>,
    pub mode: AgentMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_mount: Option<HttpMountDetails>,
    pub snapshotting: Snapshotting,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<AgentConfigDeclarationSchema>,
}

/// Schema-layer form of an agent config declaration.
///
/// Mirrors the legacy [`AgentConfigDeclaration`](crate::base_model::agent::AgentConfigDeclaration),
/// with the `value_type` carried as a schema-native [`SchemaType`] instead of
/// the legacy `AnalysedType`. This keeps [`AgentTypeSchema`] free of the legacy
/// value/type carriers (the shared [`AgentConfigDeclaration`](crate::base_model::agent::AgentConfigDeclaration)
/// stays in place for the legacy `AgentType` path — see N1/A3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentConfigDeclarationSchema {
    pub source: AgentConfigSource,
    pub path: Vec<String>,
    pub value_type: SchemaType,
}

/// Schema-model form of a registered agent type. Mirrors the legacy
/// `RegisteredAgentType`, with `agent_type` replaced by [`AgentTypeSchema`].
/// Used by the MCP export path (`CompiledMcp`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct RegisteredAgentTypeSchema {
    pub agent_type: AgentTypeSchema,
    pub implemented_by: RegisteredAgentTypeImplementer,
}

impl AgentDependencySchema {
    pub fn normalized(mut self) -> Self {
        self.methods.sort_by(|a, b| a.name.cmp(&b.name));
        self
    }
}

impl AgentTypeSchema {
    pub fn normalized(mut self) -> Self {
        self.methods.sort_by(|a, b| a.name.cmp(&b.name));
        self.dependencies
            .sort_by(|a, b| a.type_name.cmp(&b.type_name));
        self.dependencies = self
            .dependencies
            .into_iter()
            .map(AgentDependencySchema::normalized)
            .collect();
        self.config.sort_by(|a, b| a.path.cmp(&b.path));
        self
    }
}

/// `wasmtime`-generated bindings for the shared `golem:agent/common@2.0.0`
/// interface (the schema-based agent model) and the `golem:api/retry@1.5.0`
/// retry-policy types. The worker-executor's `preview2` bindgen remaps
/// `golem:agent/common@2.0.0` onto these types so the host and `golem-common`
/// share one definition. This is the single agent bindgen in the workspace.
#[cfg(feature = "full")]
pub mod bindings {
    wasmtime::component::bindgen!({
          path: "wit",
          world: "golem-common-schema",
          imports: {
            default: async | trappable,
          },
          exports: { default: async },
          require_store_data_send: true,
          anyhow: true,
          with: {
            "golem:core/types@2.0.0": golem_wasm::golem_core_2_0_x::types,
          },
          wasmtime_crate: ::wasmtime
    });
}

/// Conversions between the recursive in-memory agent schema types in this
/// module and the flat, index-based `golem:agent/common@2.0.0` wire bindings
/// in [`bindings`].
#[cfg(feature = "full")]
pub mod wit;

#[cfg(test)]
mod tests;
