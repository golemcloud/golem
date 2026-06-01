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
//! - `UnstructuredText { restrictions }` → `SchemaType::Text { restrictions }`
//! - `UnstructuredBinary { restrictions }` → `SchemaType::Binary { restrictions }`
//!
//! Reverse is strict: only `Text` / `Binary` with no extra restrictions (no
//! min/max, no regex) round-trip back to the legacy unstructured forms;
//! everything else round-trips via the AnalysedType adapter.

use crate::base_model::agent::{
    BinaryDescriptor, BinaryType, ComponentModelElementSchema, ElementSchema, TextDescriptor,
    TextType,
};
use crate::schema::adapters::analysed_type::{
    analysed_type_to_schema_type_inline, schema_type_to_analysed_type,
};
use crate::schema::adapters::error::{SchemaAdapterError, resolve_ref};
use crate::schema::graph::SchemaGraph;
use crate::schema::schema_type::{BinaryRestrictions, SchemaType, TextRestrictions};

/// Convert an [`ElementSchema`] into an inline [`SchemaType`].
pub fn element_schema_to_schema_type(
    schema: &ElementSchema,
) -> Result<SchemaType, SchemaAdapterError> {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            analysed_type_to_schema_type_inline(element_type)
        }
        ElementSchema::UnstructuredText(TextDescriptor { restrictions }) => {
            let languages = restrictions
                .as_ref()
                .map(|r| r.iter().map(|t| t.language_code.clone()).collect());
            Ok(SchemaType::text(TextRestrictions {
                languages,
                min_length: None,
                max_length: None,
                regex: None,
            }))
        }
        ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions }) => {
            let mime_types = restrictions
                .as_ref()
                .map(|r| r.iter().map(|t| t.mime_type.clone()).collect());
            Ok(SchemaType::binary(BinaryRestrictions {
                mime_types,
                min_bytes: None,
                max_bytes: None,
            }))
        }
    }
}

/// Reverse: project a [`SchemaType`] back into an [`ElementSchema`].
///
/// Round-trip rules:
///
/// - [`SchemaType::Ref`] is resolved against `graph` before classification,
///   so a `Ref` to a `Text`/`Binary`/component-model body round-trips the
///   same way as the inline form.
/// - `Text` only round-trips if `min_length`, `max_length`, and `regex` are
///   all `None`.
/// - `Binary` only round-trips if `min_bytes` and `max_bytes` are both
///   `None`.
/// - All other types attempt a `SchemaType → AnalysedType` projection and
///   wrap the result as `ComponentModel`.
pub fn schema_type_to_element_schema(
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<ElementSchema, SchemaAdapterError> {
    let ty = resolve_ref(graph, ty)?;
    match ty {
        SchemaType::Text { restrictions, .. } => {
            if restrictions.min_length.is_some()
                || restrictions.max_length.is_some()
                || restrictions.regex.is_some()
            {
                return Err(SchemaAdapterError::LossySchemaType(
                    "SchemaType::Text with min/max/regex restrictions has no legacy ElementSchema counterpart"
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
            Ok(ElementSchema::UnstructuredText(TextDescriptor {
                restrictions,
            }))
        }
        SchemaType::Binary { restrictions, .. } => {
            if restrictions.min_bytes.is_some() || restrictions.max_bytes.is_some() {
                return Err(SchemaAdapterError::LossySchemaType(
                    "SchemaType::Binary with size restrictions has no legacy ElementSchema counterpart"
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
            Ok(ElementSchema::UnstructuredBinary(BinaryDescriptor {
                restrictions,
            }))
        }
        other => {
            let element_type = schema_type_to_analysed_type(graph, other)?;
            Ok(ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type,
            }))
        }
    }
}
