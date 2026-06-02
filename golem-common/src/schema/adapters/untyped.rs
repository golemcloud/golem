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

//! `UntypedDataValue` ↔ `TypedSchemaValue` conversion.
//!
//! `UntypedDataValue` is the legacy untyped agent-method payload (a tuple
//! or multimodal list of inline component-model values plus inline
//! text / binary blobs). The new typed payload is [`TypedSchemaValue`],
//! whose value tree is structurally driven by a [`SchemaType`] inside a
//! self-contained [`SchemaGraph`].
//!
//! ## Forward (legacy → schema)
//!
//! Forward conversion bridges a legacy `(UntypedDataValue, DataSchema)`
//! pair into a `TypedSchemaValue`. Two entry points reflect the two legacy
//! directions:
//!
//! - [`untyped_data_value_to_typed_schema_input`] treats the legacy
//!   `DataSchema` as method input. Multimodal is rejected as in
//!   [`super::data_schema::data_schema_to_input_schema`]. Inputs are
//!   wrapped in a synthetic [`SchemaType::Record`] (one field per
//!   parameter) so that the resulting `TypedSchemaValue` carries a single
//!   well-formed root.
//! - [`untyped_data_value_to_typed_schema_output`] treats the legacy
//!   `DataSchema` as method output, mirroring
//!   [`super::data_schema::data_schema_to_output_schema`]:
//!   - empty tuple → empty [`SchemaType::Tuple`] (the typed pair cannot
//!     model `OutputSchema::Unit` directly, so the canonical empty form
//!     is used);
//!   - single tuple element → the element's schema and value inline;
//!   - multi-element tuple → [`SchemaType::Record`];
//!   - multimodal → `list<union<…>>` with the inner union flagged
//!     [`Role::Multimodal`].
//!
//! For every element:
//! - `UntypedElementValue::ComponentModel(value)` is paired with the
//!   element's component-model `AnalysedType` and walked via
//!   [`super::value::value_to_schema_value`].
//! - `UntypedElementValue::UnstructuredText` lowers to
//!   [`SchemaValue::Text`]; only inline text sources round-trip
//!   (URL references have no schema-layer counterpart and return
//!   [`SchemaAdapterError::LossySchemaType`]).
//! - `UntypedElementValue::UnstructuredBinary` lowers to
//!   [`SchemaValue::Binary`]; same inline-only rule.
//!
//! ## Reverse (schema → legacy)
//!
//! [`typed_schema_value_to_untyped_data_value`] projects a
//! [`TypedSchemaValue`] back into a legacy [`UntypedDataValue`] when the
//! root shape matches one of the canonical input/output layouts:
//!
//! - `Tuple { elements: [] }` → `UntypedDataValue::Tuple(vec![])`.
//! - `Record { fields }` → `UntypedDataValue::Tuple(...)` (one element
//!   per field).
//! - `List { element: Union with Role::Multimodal }` →
//!   `UntypedDataValue::Multimodal(...)`.
//! - any other root → `UntypedDataValue::Tuple(vec![single])`.
//!
//! For every position:
//! - `SchemaValue::Text` projects to
//!   `UntypedElementValue::UnstructuredText` with the inline
//!   [`TextSource`].
//! - `SchemaValue::Binary` projects to
//!   `UntypedElementValue::UnstructuredBinary` with the inline
//!   [`BinarySource`].
//! - everything else is lowered to a legacy `Value` via
//!   [`super::value::schema_value_to_value`] and wrapped in
//!   `UntypedElementValue::ComponentModel`.

use crate::base_model::agent::{
    BinaryReference, BinaryReferenceValue, BinarySource, BinaryType, ComponentModelElementSchema,
    DataSchema, ElementSchema, NamedElementSchemas, TextReference, TextReferenceValue, TextSource,
    TextType, UntypedDataValue, UntypedElementValue, UntypedNamedElementValue,
};
use crate::schema::adapters::data_schema::{
    data_schema_to_input_schema, data_schema_to_output_schema,
};
use crate::schema::adapters::error::{SchemaAdapterError, resolve_ref};
use crate::schema::adapters::value::{schema_value_to_value, value_to_schema_value};
use crate::schema::agent::{InputSchema, NamedField, OutputSchema};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::metadata::Role;
use crate::schema::schema_type::{NamedFieldType, SchemaType, UnionBranch};
use crate::schema::schema_value::{
    BinaryValuePayload, SchemaValue, TextValuePayload, UnionValuePayload,
};

// ===========================================================================
// Forward: UntypedDataValue → TypedSchemaValue
// ===========================================================================

/// Convert a legacy `(UntypedDataValue, DataSchema)` pair representing
/// method **inputs** into a [`TypedSchemaValue`].
///
/// The resulting root [`SchemaType`] is a [`SchemaType::Record`] whose
/// fields mirror the input parameters; the value tree is the matching
/// [`SchemaValue::Record`].
///
/// Fails if:
/// - `schema` is [`DataSchema::Multimodal`] (multimodal is an output-only
///   concern);
/// - `value` is [`UntypedDataValue::Multimodal`] or a tuple of mismatched
///   arity;
/// - any element carries a URL text/binary reference (no schema-layer
///   counterpart, see [`SchemaAdapterError::LossySchemaType`]);
/// - any element's component-model value does not match its declared
///   [`ElementSchema`].
pub fn untyped_data_value_to_typed_schema_input(
    value: UntypedDataValue,
    schema: &DataSchema,
) -> Result<TypedSchemaValue, SchemaAdapterError> {
    let input_schema = data_schema_to_input_schema(schema)?;
    let DataSchema::Tuple(NamedElementSchemas {
        elements: schema_elements,
    }) = schema
    else {
        // data_schema_to_input_schema already rejected the multimodal case.
        unreachable!("input data schema must be a tuple")
    };
    let untyped_elements = match value {
        UntypedDataValue::Tuple(elements) => elements,
        UntypedDataValue::Multimodal(_) => {
            return Err(SchemaAdapterError::ValueShapeMismatch(
                "multimodal UntypedDataValue cannot satisfy an input parameter list".into(),
            ));
        }
    };
    if untyped_elements.len() != schema_elements.len() {
        return Err(SchemaAdapterError::ValueShapeMismatch(format!(
            "input tuple arity mismatch: value has {} elements, schema declares {}",
            untyped_elements.len(),
            schema_elements.len()
        )));
    }

    let InputSchema::Parameters(fields) = input_schema;
    let mut record_fields = Vec::with_capacity(fields.len());
    let mut record_values = Vec::with_capacity(fields.len());
    for ((field, untyped), schema_element) in fields
        .into_iter()
        .zip(untyped_elements)
        .zip(schema_elements.iter())
    {
        let NamedField {
            name,
            schema: field_schema,
            metadata,
            source: _,
        } = field;
        let value = untyped_element_to_schema_value(untyped, &schema_element.schema)?;
        record_fields.push(NamedFieldType {
            name,
            body: field_schema,
            metadata,
        });
        record_values.push(value);
    }
    Ok(TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(record_fields)),
        SchemaValue::Record {
            fields: record_values,
        },
    ))
}

/// Convert a legacy `(UntypedDataValue, DataSchema)` pair representing
/// a method **output** into a [`TypedSchemaValue`].
///
/// The resulting root [`SchemaType`] mirrors
/// [`super::data_schema::data_schema_to_output_schema`]:
/// - empty tuple → empty [`SchemaType::Tuple`];
/// - single tuple element → the element's schema and value inline;
/// - multi-element tuple → [`SchemaType::Record`];
/// - multimodal → `list<union<…>>` with the inner union flagged
///   [`Role::Multimodal`].
///
/// Failure modes mirror [`untyped_data_value_to_typed_schema_input`].
pub fn untyped_data_value_to_typed_schema_output(
    value: UntypedDataValue,
    schema: &DataSchema,
) -> Result<TypedSchemaValue, SchemaAdapterError> {
    let output_schema = data_schema_to_output_schema(schema)?;
    match (schema, value) {
        (
            DataSchema::Tuple(NamedElementSchemas {
                elements: schema_elements,
            }),
            UntypedDataValue::Tuple(untyped_elements),
        ) => {
            if untyped_elements.len() != schema_elements.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "output tuple arity mismatch: value has {} elements, schema declares {}",
                    untyped_elements.len(),
                    schema_elements.len()
                )));
            }
            match schema_elements.as_slice() {
                [] => {
                    // OutputSchema::Unit has no SchemaType representation;
                    // pick the canonical empty form so the typed pair is
                    // still well-formed.
                    Ok(TypedSchemaValue::new(
                        SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
                        SchemaValue::Tuple {
                            elements: Vec::new(),
                        },
                    ))
                }
                [_single_schema] => {
                    let untyped = untyped_elements
                        .into_iter()
                        .next()
                        .expect("single-element tuple");
                    let OutputSchema::Single(root_type) = output_schema else {
                        unreachable!("single-element output must be OutputSchema::Single")
                    };
                    let value =
                        untyped_element_to_schema_value(untyped, &schema_elements[0].schema)?;
                    Ok(TypedSchemaValue::new(
                        SchemaGraph::anonymous(*root_type),
                        value,
                    ))
                }
                _ => {
                    let OutputSchema::Single(root_type) = output_schema else {
                        unreachable!("multi-element output must be OutputSchema::Single")
                    };
                    let values = untyped_elements
                        .into_iter()
                        .zip(schema_elements.iter())
                        .map(|(untyped, schema_element)| {
                            untyped_element_to_schema_value(untyped, &schema_element.schema)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(TypedSchemaValue::new(
                        SchemaGraph::anonymous(*root_type),
                        SchemaValue::Record { fields: values },
                    ))
                }
            }
        }
        (
            DataSchema::Multimodal(NamedElementSchemas {
                elements: schema_elements,
            }),
            UntypedDataValue::Multimodal(untyped_elements),
        ) => {
            let OutputSchema::Single(root_type) = output_schema else {
                unreachable!("multimodal output must be OutputSchema::Single")
            };
            let SchemaType::List {
                element: list_element,
                ..
            } = root_type.as_ref()
            else {
                unreachable!("multimodal output root must be a list")
            };
            let SchemaType::Union {
                spec: union_spec, ..
            } = list_element.as_ref()
            else {
                unreachable!("multimodal output list element must be a union")
            };
            let branches: &[UnionBranch] = &union_spec.branches;
            let values = untyped_elements
                .into_iter()
                .map(
                    |UntypedNamedElementValue {
                         name,
                         value: untyped,
                     }| {
                        let (branch_idx, branch) = branches
                            .iter()
                            .enumerate()
                            .find(|(_, b)| b.tag == name)
                            .ok_or_else(|| {
                                SchemaAdapterError::ValueShapeMismatch(format!(
                                    "multimodal element `{name}` has no matching union branch"
                                ))
                            })?;
                        let element_schema = &schema_elements[branch_idx].schema;
                        let inner = untyped_element_to_schema_value(untyped, element_schema)?;
                        Ok(SchemaValue::Union(UnionValuePayload {
                            tag: branch.tag.clone(),
                            body: Box::new(inner),
                        }))
                    },
                )
                .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
            Ok(TypedSchemaValue::new(
                SchemaGraph::anonymous(*root_type),
                SchemaValue::List { elements: values },
            ))
        }
        (DataSchema::Tuple(_), UntypedDataValue::Multimodal(_))
        | (DataSchema::Multimodal(_), UntypedDataValue::Tuple(_)) => {
            Err(SchemaAdapterError::ValueShapeMismatch(
                "output UntypedDataValue shape (tuple/multimodal) does not match schema".into(),
            ))
        }
    }
}

/// Convert a single legacy [`UntypedElementValue`] into a [`SchemaValue`]
/// driven by the matching legacy [`ElementSchema`].
fn untyped_element_to_schema_value(
    value: UntypedElementValue,
    schema: &ElementSchema,
) -> Result<SchemaValue, SchemaAdapterError> {
    match (value, schema) {
        (
            UntypedElementValue::ComponentModel(value),
            ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }),
        ) => value_to_schema_value(&value, element_type),
        (
            UntypedElementValue::UnstructuredText(TextReferenceValue { value: text }),
            ElementSchema::UnstructuredText(_),
        ) => match text {
            TextReference::Inline(TextSource { data, text_type }) => {
                Ok(SchemaValue::Text(TextValuePayload {
                    text: data,
                    language: text_type.map(|TextType { language_code }| language_code),
                }))
            }
            TextReference::Url(_) => Err(SchemaAdapterError::LossySchemaType(
                "URL text references cannot be projected into SchemaValue::Text".into(),
            )),
        },
        (
            UntypedElementValue::UnstructuredBinary(BinaryReferenceValue { value: binary }),
            ElementSchema::UnstructuredBinary(_),
        ) => match binary {
            BinaryReference::Inline(BinarySource { data, binary_type }) => {
                Ok(SchemaValue::Binary(BinaryValuePayload {
                    bytes: data,
                    mime_type: Some(binary_type.mime_type),
                }))
            }
            BinaryReference::Url(_) => Err(SchemaAdapterError::LossySchemaType(
                "URL binary references cannot be projected into SchemaValue::Binary".into(),
            )),
        },
        (other_value, other_schema) => Err(SchemaAdapterError::ValueShapeMismatch(format!(
            "UntypedElementValue variant does not match ElementSchema variant: \
             value = {other_value:?}, schema = {other_schema:?}"
        ))),
    }
}

// ===========================================================================
// Reverse: TypedSchemaValue → UntypedDataValue
// ===========================================================================

/// Project a [`TypedSchemaValue`] back into a legacy [`UntypedDataValue`].
///
/// The decision between [`UntypedDataValue::Tuple`] and
/// [`UntypedDataValue::Multimodal`] is taken from the root [`SchemaType`]:
///
/// - `Tuple { elements: [] }` → `Tuple(vec![])` (canonical empty output).
/// - `Record { fields }` → `Tuple(...)` with one element per field, in
///   declaration order.
/// - `List { element: Union { metadata.role = Multimodal } }` →
///   `Multimodal(...)` with one [`UntypedNamedElementValue`] per list
///   element.
/// - any other root → `Tuple(vec![single])` carrying the whole value.
///
/// Per-element projection mirrors the forward direction: `SchemaValue::Text`
/// → `UnstructuredText`, `SchemaValue::Binary` → `UnstructuredBinary`,
/// everything else lowered via
/// [`super::value::schema_value_to_value`] and wrapped in
/// `ComponentModel`.
pub fn typed_schema_value_to_untyped_data_value(
    typed: &TypedSchemaValue,
) -> Result<UntypedDataValue, SchemaAdapterError> {
    let graph = typed.graph();
    let root_ty = resolve_ref(graph, typed.root_type())?;
    match (root_ty, typed.value()) {
        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: values })
            if elements.is_empty() && values.is_empty() =>
        {
            Ok(UntypedDataValue::Tuple(Vec::new()))
        }
        (
            SchemaType::Record { fields, .. },
            SchemaValue::Record {
                fields: field_values,
            },
        ) => {
            if fields.len() != field_values.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "record arity mismatch: schema has {} fields, value has {}",
                    fields.len(),
                    field_values.len()
                )));
            }
            let elements = fields
                .iter()
                .zip(field_values.iter())
                .map(|(field, value)| schema_value_to_untyped_element(graph, &field.body, value))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(UntypedDataValue::Tuple(elements))
        }
        (
            SchemaType::List { element, .. },
            SchemaValue::List {
                elements: list_values,
            },
        ) => {
            let resolved_element = resolve_ref(graph, element)?;
            if let SchemaType::Union { spec, metadata } = resolved_element
                && metadata.role == Some(Role::Multimodal)
            {
                let elements = list_values
                    .iter()
                    .map(|elem| match elem {
                        SchemaValue::Union(UnionValuePayload { tag, body }) => {
                            let branch = spec.branches.iter().find(|b| &b.tag == tag).ok_or_else(
                                || {
                                    SchemaAdapterError::ValueShapeMismatch(format!(
                                        "multimodal element tag `{tag}` does not match any branch"
                                    ))
                                },
                            )?;
                            let untyped =
                                schema_value_to_untyped_element(graph, &branch.body, body)?;
                            Ok(UntypedNamedElementValue {
                                name: tag.clone(),
                                value: untyped,
                            })
                        }
                        other => Err(SchemaAdapterError::ValueShapeMismatch(format!(
                            "multimodal list element must be a Union value, got: {other:?}"
                        ))),
                    })
                    .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
                Ok(UntypedDataValue::Multimodal(elements))
            } else {
                // Non-multimodal list → single-element tuple carrying the
                // whole list as a component-model value.
                let untyped =
                    schema_value_to_untyped_element(graph, typed.root_type(), typed.value())?;
                Ok(UntypedDataValue::Tuple(vec![untyped]))
            }
        }
        _ => {
            let untyped = schema_value_to_untyped_element(graph, typed.root_type(), typed.value())?;
            Ok(UntypedDataValue::Tuple(vec![untyped]))
        }
    }
}

/// Project one `(SchemaType, SchemaValue)` position into an
/// [`UntypedElementValue`].
fn schema_value_to_untyped_element(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<UntypedElementValue, SchemaAdapterError> {
    let resolved = resolve_ref(graph, ty)?;
    match (resolved, value) {
        (SchemaType::Text { .. }, SchemaValue::Text(TextValuePayload { text, language })) => {
            let text_type = language.as_ref().map(|code| TextType {
                language_code: code.clone(),
            });
            Ok(UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource {
                    data: text.clone(),
                    text_type,
                }),
            }))
        }
        (
            SchemaType::Binary { .. },
            SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }),
        ) => {
            let mime = mime_type.clone().unwrap_or_default();
            Ok(UntypedElementValue::UnstructuredBinary(
                BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data: bytes.clone(),
                        binary_type: BinaryType { mime_type: mime },
                    }),
                },
            ))
        }
        _ => {
            let component_value = schema_value_to_value(graph, ty, value)?;
            Ok(UntypedElementValue::ComponentModel(component_value))
        }
    }
}
