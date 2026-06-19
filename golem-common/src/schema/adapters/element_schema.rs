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

//! `ElementSchema` ↔ `SchemaType` conversion.
//!
//! `ElementSchema` is the legacy per-slot schema used inside `DataSchema`. It
//! lowers to:
//!
//! - `ComponentModel(t)` → `analysed_type_to_schema_type_inline(t)`
//! - `UnstructuredText { restrictions }` → the canonical role-marked variant
//!   `variant { inline: text<…>, url: url } with Role::UnstructuredText`
//! - `UnstructuredBinary { restrictions }` → the canonical role-marked variant
//!   `variant { inline: binary<…>, url: url } with Role::UnstructuredBinary`
//!
//! The canonical unstructured form (see [`crate::schema::adapters::unstructured`])
//! is what the guest SDKs publish and what both bridge generators detect, so
//! legacy-originated agents lower to exactly the same shape as native ones.
//!
//! Reverse is strict: only the role-marked unstructured variants (whose inline
//! body carries no extra `min`/`max`/`regex` restrictions) round-trip back to
//! the legacy unstructured forms; everything else round-trips via the
//! AnalysedType adapter.

use crate::base_model::agent::{
    BinaryDescriptor, BinaryType, ComponentModelElementSchema, ElementSchema, TextDescriptor,
    TextType,
};
use crate::schema::adapters::analysed_type::{
    SchemaGraphBuilder, analysed_type_to_schema_type_inline, schema_type_to_analysed_type,
};
use crate::schema::adapters::error::{SchemaAdapterError, resolve_ref};
use crate::schema::adapters::unstructured::{
    unstructured_binary_restrictions, unstructured_binary_schema_type,
    unstructured_text_restrictions, unstructured_text_schema_type,
};
use crate::schema::graph::SchemaGraph;
use crate::schema::schema_type::{BinaryRestrictions, SchemaType, TextRestrictions};

/// Convert an [`ElementSchema`] into an inline [`SchemaType`].
///
/// `ComponentModel` element types are inlined anonymously (no named-type
/// registry). Use [`element_schema_to_schema_type_in`] when the legacy named
/// composites must be preserved as [`SchemaType::Ref`]s into a shared graph.
pub fn element_schema_to_schema_type(
    schema: &ElementSchema,
) -> Result<SchemaType, SchemaAdapterError> {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            analysed_type_to_schema_type_inline(element_type)
        }
        ElementSchema::UnstructuredText(d) => Ok(text_descriptor_to_schema_type(d)),
        ElementSchema::UnstructuredBinary(d) => Ok(binary_descriptor_to_schema_type(d)),
    }
}

/// Graph-threaded variant of [`element_schema_to_schema_type`]: lowers a
/// `ComponentModel` element type through the shared `builder` so legacy named
/// composites are hoisted into the agent's [`SchemaGraph`] and referenced via
/// [`SchemaType::Ref`] instead of being inlined anonymously. This preserves
/// the type identity the per-language bridge renderers need to print native
/// type names.
pub(crate) fn element_schema_to_schema_type_in(
    builder: &mut SchemaGraphBuilder,
    schema: &ElementSchema,
) -> Result<SchemaType, SchemaAdapterError> {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            builder.lower(element_type)
        }
        ElementSchema::UnstructuredText(d) => Ok(text_descriptor_to_schema_type(d)),
        ElementSchema::UnstructuredBinary(d) => Ok(binary_descriptor_to_schema_type(d)),
    }
}

fn text_descriptor_to_schema_type(TextDescriptor { restrictions }: &TextDescriptor) -> SchemaType {
    let languages = restrictions
        .as_ref()
        .map(|r| r.iter().map(|t| t.language_code.clone()).collect());
    unstructured_text_schema_type(TextRestrictions {
        languages,
        min_length: None,
        max_length: None,
        regex: None,
    })
}

fn binary_descriptor_to_schema_type(
    BinaryDescriptor { restrictions }: &BinaryDescriptor,
) -> SchemaType {
    let mime_types = restrictions
        .as_ref()
        .map(|r| r.iter().map(|t| t.mime_type.clone()).collect());
    unstructured_binary_schema_type(BinaryRestrictions {
        mime_types,
        min_bytes: None,
        max_bytes: None,
    })
}

/// Reverse: project a [`SchemaType`] back into an [`ElementSchema`].
///
/// Round-trip rules:
///
/// - [`SchemaType::Ref`] is resolved against `graph` before classification,
///   so a `Ref` to an unstructured variant / component-model body round-trips
///   the same way as the inline form.
/// - The canonical role-marked unstructured-text variant round-trips to
///   `UnstructuredText` only if its inline `text` body has no `min_length` /
///   `max_length` / `regex` restriction.
/// - The canonical role-marked unstructured-binary variant round-trips to
///   `UnstructuredBinary` only if its inline `binary` body has no `min_bytes` /
///   `max_bytes` restriction.
/// - All other types attempt a `SchemaType → AnalysedType` projection and
///   wrap the result as `ComponentModel`. (Bare `Text` / `Binary` rich scalars
///   have no legacy `ElementSchema` counterpart and fail as lossy.)
pub fn schema_type_to_element_schema(
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<ElementSchema, SchemaAdapterError> {
    if let Some(restrictions) = unstructured_text_restrictions(graph, ty)? {
        if restrictions.min_length.is_some()
            || restrictions.max_length.is_some()
            || restrictions.regex.is_some()
        {
            return Err(SchemaAdapterError::LossySchemaType(
                "Role::UnstructuredText with min/max/regex restrictions has no legacy ElementSchema counterpart"
                    .into(),
            ));
        }
        let restrictions = restrictions.languages.as_ref().map(|langs| {
            langs
                .iter()
                .map(|code| TextType {
                    language_code: code.clone(),
                })
                .collect()
        });
        return Ok(ElementSchema::UnstructuredText(TextDescriptor {
            restrictions,
        }));
    }
    if let Some(restrictions) = unstructured_binary_restrictions(graph, ty)? {
        if restrictions.min_bytes.is_some() || restrictions.max_bytes.is_some() {
            return Err(SchemaAdapterError::LossySchemaType(
                "Role::UnstructuredBinary with size restrictions has no legacy ElementSchema counterpart"
                    .into(),
            ));
        }
        let restrictions = restrictions.mime_types.as_ref().map(|mimes| {
            mimes
                .iter()
                .map(|m| BinaryType {
                    mime_type: m.clone(),
                })
                .collect()
        });
        return Ok(ElementSchema::UnstructuredBinary(BinaryDescriptor {
            restrictions,
        }));
    }

    let element_type = schema_type_to_analysed_type(graph, resolve_ref(graph, ty)?)?;
    Ok(ElementSchema::ComponentModel(ComponentModelElementSchema {
        element_type,
    }))
}
