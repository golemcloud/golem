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
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::SchemaValue;
use crate::schema::validation::value::validate_value;
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// Name of the synthetic single user-supplied field that carries the parts of
/// a multimodal agent input (`list<variant<… Role::Multimodal>>`).
pub const MULTIMODAL_PARTS_FIELD_NAME: &str = "parts";

/// Name of the synthetic field used to wrap a single-value (non-multimodal)
/// agent output that has no declared field name of its own.
pub const FALLBACK_OUTPUT_FIELD_NAME: &str = "value";

/// Lift raw client JSON (a bare schema-native [`SchemaValue`]) plus an agent
/// constructor/method [`InputSchema`] and its owning [`SchemaGraph`] into a
/// validated [`TypedSchemaValue`].
///
/// The caller only provides the [`FieldSource::UserSupplied`] fields;
/// auto-injected fields (e.g. the principal) are filled by the host out of
/// band and are excluded from the synthesized record schema, so the incoming
/// value must contain exactly the user-supplied fields.
///
/// The synthesized input record is the single root of the returned value, so
/// its `defs` are projected to exactly the named-type definitions reachable
/// from that root (see [`SchemaGraph`] and [`reachable_defs`]). This keeps the
/// result self-contained — every `SchemaType::Ref` in the field bodies resolves
/// during validation and for any later receiver — while dropping the rest of
/// the agent's multi-root definition registry, which the value can never
/// reference.
pub fn json_input_schema_value_to_typed_schema_value(
    json: JsonValue,
    graph: &SchemaGraph,
    input_schema: &InputSchema,
) -> Result<TypedSchemaValue, String> {
    let value: SchemaValue =
        serde_json::from_value(json).map_err(|e| format!("invalid schema value: {e}"))?;
    // Only user-supplied fields are part of the caller's input; auto-injected
    // fields (e.g. the principal) are filled by the host out of band and are
    // not present in the incoming value, so they are excluded from the record
    // schema the value is validated against.
    let fields = input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .map(|field| NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        })
        .collect();
    let root = SchemaType::record(fields);
    let result_graph = SchemaGraph {
        defs: reachable_defs(graph, &root),
        root,
    };
    validate_value(&result_graph, &result_graph.root, &value).map_err(|errors| {
        errors
            .into_iter()
            .map(|err| err.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    Ok(TypedSchemaValue::new(result_graph, value))
}

pub use crate::schema::graph::reachable_defs;

/// Build a self-contained [`TypedSchemaValue`] from an already-validated
/// [`SchemaValue`] and an explicit `root`, projecting `graph`'s definitions to
/// exactly those reachable from `root`.
///
/// Use this when `value` has already been decoded/validated against `graph` and
/// `root` and only a self-contained carrier needs to be produced: it projects
/// the reachable definition subset (see [`reachable_defs`]) instead of cloning
/// the agent's whole multi-root `defs` registry. This is the projection half of
/// [`json_input_schema_value_to_typed_schema_value`], without the JSON decode
/// and validation steps.
pub fn typed_schema_value_with_projected_defs(
    graph: &SchemaGraph,
    root: SchemaType,
    value: SchemaValue,
) -> TypedSchemaValue {
    let defs = reachable_defs(graph, &root);
    TypedSchemaValue::new(SchemaGraph { defs, root }, value)
}

/// Input parameter list for an agent constructor or method.
///
/// The single [`InputSchema::Parameters`] case carries an ordered list of
/// [`NamedField`]s. Each field carries its name, its schema, its metadata,
/// and a [`FieldSource`] that tells the runtime where the value comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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

/// Combine ordered, independently-typed argument values into a single record
/// [`TypedSchemaValue`] representing an agent method's (or constructor's)
/// parameter list.
///
/// Each argument carries its own [`SchemaGraph`]; their named definitions are
/// merged (deduplicated by `TypeId`) and each argument's root type becomes a
/// positional field (`p0`, `p1`, …) of the resulting record. The value is a
/// [`SchemaValue::Record`](crate::schema::schema_value::SchemaValue::Record)
/// of the argument values in declaration order.
///
/// This is the schema-native replacement for the test DSL's legacy
/// `data_value!` builder: callers pass values whose types implement
/// [`IntoSchema`](crate::schema::IntoSchema) and obtain a self-contained typed
/// carrier ready to hand to the invocation DSL.
pub fn build_input_record(
    args: Vec<TypedSchemaValue>,
) -> Result<TypedSchemaValue, crate::schema::MergeError> {
    use crate::schema::conversion::merge_agent_graphs;
    use crate::schema::schema_type::NamedFieldType;
    use crate::schema::schema_value::SchemaValue;

    let merged = merge_agent_graphs(args.iter().map(|arg| arg.graph().clone()))?;

    let mut fields = Vec::with_capacity(args.len());
    let mut values = Vec::with_capacity(args.len());
    for (idx, arg) in args.into_iter().enumerate() {
        let (graph, value) = arg.into_parts();
        fields.push(NamedFieldType {
            name: format!("p{idx}"),
            body: graph.root,
            metadata: MetadataEnvelope::default(),
        });
        values.push(value);
    }

    let graph = SchemaGraph {
        defs: merged.defs,
        root: SchemaType::record(fields),
    };
    Ok(TypedSchemaValue::new(
        graph,
        SchemaValue::Record { fields: values },
    ))
}

/// Constructor signature for an agent type, schema-layer form.
///
/// Mirrors the legacy `AgentConstructor`, with `input_schema` replaced by the
/// new [`InputSchema`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
/// Carries the `value_type` as a schema-native [`SchemaType`]. This keeps
/// [`AgentTypeSchema`] free of the legacy value/type carriers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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
#[cfg_attr(feature = "full", derive(golem_schema_derive::PoemSchema))]
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

    pub fn normalized_vec(mut agent_types: Vec<Self>) -> Vec<Self> {
        agent_types.sort_by(|a, b| a.type_name.cmp(&b.type_name));
        agent_types.into_iter().map(Self::normalized).collect()
    }

    /// Validates the semantic constraints of the agent type. Mirrors the legacy
    /// `AgentType::validate`: ephemeral agents must not declare read-only
    /// methods (there is no shared state to read from).
    pub fn validate(&self) -> Result<(), String> {
        if self.mode == AgentMode::Ephemeral {
            for method in &self.methods {
                if method.read_only.is_some() {
                    return Err(format!(
                        "Agent type '{}' is ephemeral but method '{}' is marked as read-only. \
                         Read-only methods have no benefit on ephemeral agents (no shared state to read from). \
                         Remove the read-only marker or make the agent durable.",
                        self.type_name, method.name
                    ));
                }
            }
        }
        Ok(())
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
            "golem:core/types@2.0.0": golem_schema::schema::wit::wire,
            "wasi:io/streams.input-stream": wasmtime_wasi::DynInputStream,
            "wasi:io/streams.output-stream": wasmtime_wasi::DynOutputStream,
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
