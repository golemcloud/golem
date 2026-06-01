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

//! Agent-level adapters: `AgentType`, `AgentConstructor`, `AgentMethod`,
//! `AgentDependency` ↔ their schema-layer mirrors.
//!
//! These adapters are thin wrappers around [`data_schema_to_input_schema`] /
//! [`data_schema_to_output_schema`] / their reverses, plus straight-through
//! propagation of non-schema fields (`mode`, `http_mount`, `snapshotting`,
//! `config`, `http_endpoint`). The non-schema fields are carried verbatim;
//! only the slots that previously held a `DataSchema` change shape.
//!
//! Graph carriers:
//!
//! - [`AgentTypeSchema`] and [`AgentDependencySchema`] each own a
//!   [`SchemaGraph`] that holds shared named definitions for their
//!   constructor and methods (see §4.22 of the design doc). The constructor
//!   / method [`InputSchema`] and [`OutputSchema`] bodies may use
//!   [`SchemaType::Ref`] to reference entries in that graph.
//! - Forward adapters (legacy → schema) produce an
//!   [`SchemaGraph::empty`] graph with all bodies fully inlined — the
//!   legacy representation has no named-type registry to hoist.
//! - Reverse adapters (schema → legacy) are graph-aware: they pass the
//!   agent's / dependency's [`SchemaGraph`] into
//!   [`input_schema_to_data_schema`] /
//!   [`output_schema_to_data_schema`] so refs resolve against the right
//!   pool. Bodies declared at the parent constructor/method level resolve
//!   against [`AgentTypeSchema::schema`]; bodies inside a dependency
//!   resolve against the dependency's own
//!   [`AgentDependencySchema::schema`].
//!
//! Out of scope for these adapters: `LegacyParsedAgentId` ↔ schema-layer
//! `ParsedAgentId`. That conversion would have to map a legacy `DataValue`
//! to a `TypedSchemaValue` and back, but there is no `DataValue` ↔
//! `SchemaValue` adapter yet, so the agent-id adapter is intentionally
//! deferred to a follow-up step.

use crate::base_model::agent::{AgentConstructor, AgentDependency, AgentMethod, AgentType};
use crate::schema::adapters::data_schema::{
    data_schema_to_input_schema, data_schema_to_output_schema, input_schema_to_data_schema,
    output_schema_to_data_schema,
};
use crate::schema::adapters::error::SchemaAdapterError;
use crate::schema::agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
};
use crate::schema::graph::SchemaGraph;

/// Forward: legacy [`AgentConstructor`] → [`AgentConstructorSchema`].
pub fn agent_constructor_to_schema(
    ctor: &AgentConstructor,
) -> Result<AgentConstructorSchema, SchemaAdapterError> {
    Ok(AgentConstructorSchema {
        name: ctor.name.clone(),
        description: ctor.description.clone(),
        prompt_hint: ctor.prompt_hint.clone(),
        input_schema: data_schema_to_input_schema(&ctor.input_schema)?,
    })
}

/// Reverse: [`AgentConstructorSchema`] → legacy [`AgentConstructor`].
///
/// Refs in the input schema body are resolved against `graph` — the
/// enclosing agent's or dependency's [`SchemaGraph`].
pub fn schema_agent_constructor_to_legacy(
    graph: &SchemaGraph,
    ctor: &AgentConstructorSchema,
) -> Result<AgentConstructor, SchemaAdapterError> {
    Ok(AgentConstructor {
        name: ctor.name.clone(),
        description: ctor.description.clone(),
        prompt_hint: ctor.prompt_hint.clone(),
        input_schema: input_schema_to_data_schema(graph, &ctor.input_schema)?,
    })
}

/// Forward: legacy [`AgentMethod`] → [`AgentMethodSchema`].
pub fn agent_method_to_schema(
    method: &AgentMethod,
) -> Result<AgentMethodSchema, SchemaAdapterError> {
    Ok(AgentMethodSchema {
        name: method.name.clone(),
        description: method.description.clone(),
        prompt_hint: method.prompt_hint.clone(),
        input_schema: data_schema_to_input_schema(&method.input_schema)?,
        output_schema: data_schema_to_output_schema(&method.output_schema)?,
        http_endpoint: method.http_endpoint.clone(),
    })
}

/// Reverse: [`AgentMethodSchema`] → legacy [`AgentMethod`].
///
/// Refs in input and output bodies are resolved against `graph` — the
/// enclosing agent's or dependency's [`SchemaGraph`].
pub fn schema_agent_method_to_legacy(
    graph: &SchemaGraph,
    method: &AgentMethodSchema,
) -> Result<AgentMethod, SchemaAdapterError> {
    Ok(AgentMethod {
        name: method.name.clone(),
        description: method.description.clone(),
        prompt_hint: method.prompt_hint.clone(),
        input_schema: input_schema_to_data_schema(graph, &method.input_schema)?,
        output_schema: output_schema_to_data_schema(graph, &method.output_schema)?,
        http_endpoint: method.http_endpoint.clone(),
    })
}

/// Forward: legacy [`AgentDependency`] → [`AgentDependencySchema`].
///
/// Produces an empty `SchemaGraph` with all bodies fully inlined. The
/// legacy representation has no named-type registry to hoist.
pub fn agent_dependency_to_schema(
    dep: &AgentDependency,
) -> Result<AgentDependencySchema, SchemaAdapterError> {
    Ok(AgentDependencySchema {
        type_name: dep.type_name.clone(),
        description: dep.description.clone(),
        schema: SchemaGraph::empty(),
        constructor: agent_constructor_to_schema(&dep.constructor)?,
        methods: dep
            .methods
            .iter()
            .map(agent_method_to_schema)
            .collect::<Result<_, _>>()?,
    })
}

/// Reverse: [`AgentDependencySchema`] → legacy [`AgentDependency`].
///
/// Refs in constructor / method bodies resolve against the dependency's
/// own [`AgentDependencySchema::schema`].
pub fn schema_agent_dependency_to_legacy(
    dep: &AgentDependencySchema,
) -> Result<AgentDependency, SchemaAdapterError> {
    Ok(AgentDependency {
        type_name: dep.type_name.clone(),
        description: dep.description.clone(),
        constructor: schema_agent_constructor_to_legacy(&dep.schema, &dep.constructor)?,
        methods: dep
            .methods
            .iter()
            .map(|m| schema_agent_method_to_legacy(&dep.schema, m))
            .collect::<Result<_, _>>()?,
    })
}

/// Forward: legacy [`AgentType`] → [`AgentTypeSchema`].
///
/// Produces an empty `SchemaGraph` with all bodies fully inlined. The
/// legacy representation has no named-type registry to hoist.
pub fn agent_type_to_schema(ty: &AgentType) -> Result<AgentTypeSchema, SchemaAdapterError> {
    Ok(AgentTypeSchema {
        type_name: ty.type_name.clone(),
        description: ty.description.clone(),
        source_language: ty.source_language.clone(),
        schema: SchemaGraph::empty(),
        constructor: agent_constructor_to_schema(&ty.constructor)?,
        methods: ty
            .methods
            .iter()
            .map(agent_method_to_schema)
            .collect::<Result<_, _>>()?,
        dependencies: ty
            .dependencies
            .iter()
            .map(agent_dependency_to_schema)
            .collect::<Result<_, _>>()?,
        mode: ty.mode.clone(),
        http_mount: ty.http_mount.clone(),
        snapshotting: ty.snapshotting.clone(),
        config: ty.config.clone(),
    })
}

/// Reverse: [`AgentTypeSchema`] → legacy [`AgentType`].
///
/// Refs in the parent constructor / method bodies resolve against
/// [`AgentTypeSchema::schema`]. Dependencies resolve refs against their
/// own [`AgentDependencySchema::schema`].
pub fn schema_agent_type_to_legacy(ty: &AgentTypeSchema) -> Result<AgentType, SchemaAdapterError> {
    Ok(AgentType {
        type_name: ty.type_name.clone(),
        description: ty.description.clone(),
        source_language: ty.source_language.clone(),
        constructor: schema_agent_constructor_to_legacy(&ty.schema, &ty.constructor)?,
        methods: ty
            .methods
            .iter()
            .map(|m| schema_agent_method_to_legacy(&ty.schema, m))
            .collect::<Result<_, _>>()?,
        dependencies: ty
            .dependencies
            .iter()
            .map(schema_agent_dependency_to_legacy)
            .collect::<Result<_, _>>()?,
        mode: ty.mode.clone(),
        http_mount: ty.http_mount.clone(),
        snapshotting: ty.snapshotting.clone(),
        config: ty.config.clone(),
    })
}

