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
//! Includes [`legacy_parsed_agent_id_to_schema`], which walks the legacy
//! `LegacyParsedAgentId::parameters` (a [`DataValue`]) and projects each
//! `ComponentModelElementValue` into a [`SchemaValue`] paired with a
//! [`SchemaType`] driven by the element's embedded `AnalysedType`.

use crate::base_model::agent::{
    AgentConstructor, AgentDependency, AgentMethod, AgentType, DataValue, ElementValue,
    LegacyParsedAgentId,
};
use crate::schema::adapters::analysed_type::SchemaGraphBuilder;
use crate::schema::adapters::data_schema::{
    data_schema_to_input_schema, data_schema_to_output_schema, input_schema_to_data_schema,
    output_schema_to_data_schema,
};
use crate::schema::adapters::error::SchemaAdapterError;
use crate::schema::adapters::value::value_to_schema_value;
use crate::schema::agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
    ParsedAgentId,
};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::{BinaryValuePayload, SchemaValue, TextValuePayload};

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
        read_only: method.read_only.clone(),
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
        read_only: method.read_only.clone(),
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
        mode: ty.mode,
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
        mode: ty.mode,
        http_mount: ty.http_mount.clone(),
        snapshotting: ty.snapshotting.clone(),
        config: ty.config.clone(),
    })
}

/// Project a [`LegacyParsedAgentId`] into the schema-layer [`ParsedAgentId`].
///
/// Walks the legacy `parameters: DataValue` and constructs a
/// [`TypedSchemaValue`] whose root is a [`SchemaType::Record`] (for the tuple
/// shape) or a `list<variant<...> with Role::Multimodal>` (for the multimodal
/// shape), with one element per legacy `ElementValue`. Each element's
/// component-model body is paired with its embedded `AnalysedType`, converted
/// in-place via [`analysed_type_to_schema_type_inline`] +
/// [`value_to_schema_value`]. Unstructured text/binary elements become
/// inline [`SchemaValue::Text`] / [`SchemaValue::Binary`]; URL references
/// have no schema-layer counterpart and return
/// [`SchemaAdapterError::LossySchemaType`].
pub fn legacy_parsed_agent_id_to_schema(
    parsed: &LegacyParsedAgentId,
) -> Result<ParsedAgentId, SchemaAdapterError> {
    let typed = legacy_data_value_to_typed_schema_value(&parsed.parameters)?;
    Ok(ParsedAgentId::new(
        parsed.agent_type.clone(),
        typed,
        parsed.phantom_id,
    ))
}

/// Walk a legacy `DataValue` into a [`TypedSchemaValue`] using the type
/// info embedded in each `ComponentModelElementValue::value` (a
/// `ValueAndType`). Used by the agent-id adapter and by CLI display sites
/// that hold a bare `DataValue` (the schema is implicitly recoverable from
/// the values themselves).
///
/// Element types are lowered through a single shared [`SchemaGraphBuilder`]
/// so legacy named composites that appear in multiple positional elements
/// (or repeat across multimodal entries) are hoisted to one
/// [`SchemaTypeDef`] keyed by their `(owner, name)`. This preserves the
/// identity that per-language renderers need to print native-looking
/// type names instead of structural anonymous bodies.
pub fn legacy_data_value_to_typed_schema_value(
    value: &DataValue,
) -> Result<TypedSchemaValue, SchemaAdapterError> {
    use crate::base_model::agent::NamedElementValues;
    use crate::schema::metadata::{MetadataEnvelope, Role};
    use crate::schema::schema_type::VariantCaseType;
    use crate::schema::schema_value::VariantValuePayload;
    let mut builder = SchemaGraphBuilder::new();
    match value {
        DataValue::Tuple(elements) => {
            let mut fields = Vec::with_capacity(elements.elements.len());
            let mut values = Vec::with_capacity(elements.elements.len());
            for (i, element) in elements.elements.iter().enumerate() {
                let (name, schema_ty, schema_value) =
                    legacy_element_to_named(&mut builder, element, i)?;
                fields.push(NamedFieldType {
                    name,
                    body: schema_ty,
                    metadata: MetadataEnvelope::default(),
                });
                values.push(schema_value);
            }
            Ok(TypedSchemaValue::new(
                builder.into_graph_with_root(SchemaType::record(fields)),
                SchemaValue::Record { fields: values },
            ))
        }
        DataValue::Multimodal(NamedElementValues { elements }) => {
            let mut cases = Vec::with_capacity(elements.len());
            let mut variant_values = Vec::with_capacity(elements.len());
            for (i, named) in elements.iter().enumerate() {
                let (_synth_name, schema_ty, schema_value) =
                    legacy_element_to_named(&mut builder, &named.value, i)?;
                let case_index = cases.len() as u32;
                cases.push(VariantCaseType {
                    name: named.name.clone(),
                    payload: Some(schema_ty),
                    metadata: MetadataEnvelope::default(),
                });
                variant_values.push(SchemaValue::Variant(VariantValuePayload {
                    case: case_index,
                    payload: Some(Box::new(schema_value)),
                }));
            }
            let mut variant_ty = SchemaType::variant(cases);
            variant_ty.metadata_mut().role = Some(Role::Multimodal);
            Ok(TypedSchemaValue::new(
                builder.into_graph_with_root(SchemaType::list(variant_ty)),
                SchemaValue::List {
                    elements: variant_values,
                },
            ))
        }
    }
}

/// Returns the synthetic positional name `p{idx}`, the converted
/// `SchemaType`, and the converted `SchemaValue` for one legacy
/// [`ElementValue`].
///
/// Named legacy composites are registered into the shared
/// [`SchemaGraphBuilder`] and returned as [`SchemaType::Ref`] so identity
/// is preserved across all elements of the surrounding `DataValue`.
fn legacy_element_to_named(
    builder: &mut SchemaGraphBuilder,
    element: &ElementValue,
    idx: usize,
) -> Result<(String, SchemaType, SchemaValue), SchemaAdapterError> {
    use crate::base_model::agent::{
        BinaryReference, BinarySource, ComponentModelElementValue, TextReference, TextSource,
        TextType, UnstructuredBinaryElementValue, UnstructuredTextElementValue,
    };
    use crate::schema::schema_type::{BinaryRestrictions, TextRestrictions};
    let name = format!("p{idx}");
    match element {
        ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
            let schema_ty = builder.lower(&value.typ)?;
            let schema_value = value_to_schema_value(&value.value, &value.typ)?;
            Ok((name, schema_ty, schema_value))
        }
        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => match value {
            TextReference::Inline(TextSource { data, text_type }) => {
                let payload = TextValuePayload {
                    text: data.clone(),
                    language: text_type
                        .as_ref()
                        .map(|TextType { language_code }| language_code.clone()),
                };
                let schema_ty = SchemaType::text(TextRestrictions::default());
                Ok((name, schema_ty, SchemaValue::Text(payload)))
            }
            TextReference::Url(_) => Err(SchemaAdapterError::LossySchemaType(
                "URL text references cannot be projected into SchemaValue::Text".into(),
            )),
        },
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            match value {
                BinaryReference::Inline(BinarySource { data, binary_type }) => {
                    let payload = BinaryValuePayload {
                        bytes: data.clone(),
                        mime_type: Some(binary_type.mime_type.clone()),
                    };
                    let schema_ty = SchemaType::binary(BinaryRestrictions::default());
                    Ok((name, schema_ty, SchemaValue::Binary(payload)))
                }
                BinaryReference::Url(_) => Err(SchemaAdapterError::LossySchemaType(
                    "URL binary references cannot be projected into SchemaValue::Binary".into(),
                )),
            }
        }
    }
}
