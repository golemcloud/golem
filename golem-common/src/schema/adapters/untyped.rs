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

//! `UntypedDataValue` ↔ typed-value conversions.
//!
//! `UntypedDataValue` is the legacy untyped agent-method payload (a tuple
//! or multimodal list of inline component-model values plus inline
//! text / binary blobs). The new typed values are driven by the
//! [`SchemaType`] / [`SchemaValue`] world:
//!
//! - **Inputs** are natively a parameter list ([`InputSchema::Parameters`]
//!   per §4.7 of the value-type-refactor design), paired by position with
//!   a `Vec<SchemaValue>`. No single root is required, so no synthetic
//!   wrapper is needed.
//! - **Outputs** are a single value (or `Unit`), and travel as a
//!   [`TypedSchemaValue`] (single root [`SchemaType`] inside a
//!   self-contained [`SchemaGraph`]).
//!
//! ## Forward (legacy → typed)
//!
//! - [`untyped_data_value_to_typed_input`] treats the legacy `DataSchema`
//!   as method input, mirroring
//!   [`super::data_schema::data_schema_to_input_schema`]. Tuple schemas
//!   produce a pair `(InputSchema::Parameters, Vec<SchemaValue>)` where the
//!   value vector is positionally aligned with the parameter list.
//!   Multimodal schemas produce a single synthetic `parts` parameter whose
//!   value is a `list<variant<…>>` of one `SchemaValue::Variant` per element.
//! - [`untyped_data_value_to_typed_schema_output`] treats the legacy
//!   `DataSchema` as method output, mirroring
//!   [`super::data_schema::data_schema_to_output_schema`]:
//!   - empty tuple → empty [`SchemaType::Tuple`] (the typed pair cannot
//!     model `OutputSchema::Unit` directly, so the canonical empty form
//!     is used);
//!   - single tuple element → the element's schema and value inline;
//!   - multi-element tuple → error. Golem agent methods only ever return 0
//!     or 1 element, so a multi-element output tuple is rejected;
//!   - multimodal → `list<variant<…>>` with the inner variant flagged
//!     [`Role::Multimodal`].
//!
//! For every position:
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
//! ## Reverse (typed → legacy)
//!
//! - [`typed_input_to_untyped_data_value`] projects an
//!   `(InputSchema::Parameters, &[SchemaValue])` pair back into a legacy
//!   `UntypedDataValue::Tuple(...)` with one element per parameter, in
//!   declaration order.
//! - [`typed_schema_value_to_untyped_data_value`] projects a
//!   [`TypedSchemaValue`] (only ever an output-shaped value) back into a
//!   legacy [`UntypedDataValue`]:
//!   - `Tuple { elements: [] }` → `UntypedDataValue::Tuple(vec![])`.
//!   - `List { element: Variant with Role::Multimodal }` →
//!     `UntypedDataValue::Multimodal(...)`.
//!   - any other root (including real user-defined records that are
//!     returned as a single-element method output) →
//!     `UntypedDataValue::Tuple(vec![single])`.
//!
//! Because inputs no longer travel as `TypedSchemaValue`, the reverse path
//! never has to disambiguate a "synthetic input wrapper record" from a real
//! user-defined record output — that ambiguity (and the marker role it
//! used to require) is gone.
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
    DataSchema, ElementSchema, NamedElementSchema, NamedElementSchemas, TextReference,
    TextReferenceValue, TextSource, TextType, UntypedDataValue, UntypedElementValue,
    UntypedNamedElementValue,
};
use crate::schema::adapters::data_schema::{
    as_multimodal_list_variant, data_schema_to_input_schema, data_schema_to_output_schema,
};
use crate::schema::adapters::error::{SchemaAdapterError, resolve_ref};
use crate::schema::adapters::value::{schema_value_to_value, value_to_schema_value};
use crate::schema::agent::{FieldSource, InputSchema, OutputSchema};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::metadata::Role;
use crate::schema::schema_type::{SchemaType, VariantCaseType};
use crate::schema::schema_value::{
    BinaryValuePayload, SchemaValue, TextValuePayload, VariantValuePayload,
};

// ===========================================================================
// Forward: UntypedDataValue → typed
// ===========================================================================

/// Convert a legacy `(UntypedDataValue, DataSchema)` pair representing
/// method **inputs** into the natural typed form of an input parameter
/// list: a paired [`InputSchema::Parameters`] and a positionally aligned
/// `Vec<SchemaValue>`.
///
/// The shape mirrors `InputSchema = Parameters(Vec<NamedField>)` (§4.7 of
/// the value-type-refactor design): inputs are a list of named parameters,
/// not a single rooted value, so no synthetic wrapper is introduced.
///
/// Multimodal inputs are supported: a [`DataSchema::Multimodal`] schema
/// paired with an [`UntypedDataValue::Multimodal`] value produces a single
/// `parts` parameter (per [`data_schema_to_input_schema`]) whose value is a
/// `list<variant<…>>` of one [`SchemaValue::Variant`] per multimodal element.
///
/// Fails if:
/// - the value's shape (tuple/multimodal) does not match the schema's shape;
/// - a tuple value/schema have mismatched arity;
/// - any element carries a URL text/binary reference (no schema-layer
///   counterpart, see [`SchemaAdapterError::LossySchemaType`]);
/// - any element's component-model value does not match its declared
///   [`ElementSchema`].
pub fn untyped_data_value_to_typed_input(
    value: UntypedDataValue,
    schema: &DataSchema,
) -> Result<(InputSchema, Vec<SchemaValue>), SchemaAdapterError> {
    let input_schema = data_schema_to_input_schema(schema)?;
    match (schema, value) {
        (
            DataSchema::Tuple(NamedElementSchemas {
                elements: schema_elements,
            }),
            UntypedDataValue::Tuple(untyped_elements),
        ) => {
            if untyped_elements.len() != schema_elements.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "input tuple arity mismatch: value has {} elements, schema declares {}",
                    untyped_elements.len(),
                    schema_elements.len()
                )));
            }
            let values = untyped_elements
                .into_iter()
                .zip(schema_elements.iter())
                .map(|(untyped, schema_element)| {
                    untyped_element_to_schema_value(untyped, &schema_element.schema)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((input_schema, values))
        }
        (
            DataSchema::Multimodal(NamedElementSchemas {
                elements: schema_elements,
            }),
            UntypedDataValue::Multimodal(untyped_elements),
        ) => {
            let parts = multimodal_untyped_to_list_value(schema_elements, untyped_elements)?;
            Ok((input_schema, vec![parts]))
        }
        (DataSchema::Tuple(_), UntypedDataValue::Multimodal(_))
        | (DataSchema::Multimodal(_), UntypedDataValue::Tuple(_)) => {
            Err(SchemaAdapterError::ValueShapeMismatch(
                "input UntypedDataValue shape (tuple/multimodal) does not match schema".into(),
            ))
        }
    }
}

/// Build the `list<variant<…>>` value for a multimodal payload: one
/// [`SchemaValue::Variant`] per element, matching each
/// [`UntypedNamedElementValue`] to its legacy alternative [`ElementSchema`] by
/// name. The variant case index is the alternative's position in
/// `schema_elements` (the same order in which the structural variant is built
/// by `multimodal_elements_to_list_variant`). Shared by the input and output
/// multimodal forward conversions.
fn multimodal_untyped_to_list_value(
    schema_elements: &[NamedElementSchema],
    untyped_elements: Vec<UntypedNamedElementValue>,
) -> Result<SchemaValue, SchemaAdapterError> {
    let values = untyped_elements
        .into_iter()
        .map(
            |UntypedNamedElementValue {
                 name,
                 value: untyped,
             }| {
                let (index, schema_element) = schema_elements
                    .iter()
                    .enumerate()
                    .find(|(_, e)| e.name == name)
                    .ok_or_else(|| {
                        SchemaAdapterError::ValueShapeMismatch(format!(
                            "multimodal element `{name}` has no matching schema alternative"
                        ))
                    })?;
                let inner = untyped_element_to_schema_value(untyped, &schema_element.schema)?;
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: index as u32,
                    payload: Some(Box::new(inner)),
                }))
            },
        )
        .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
    Ok(SchemaValue::List { elements: values })
}

/// Convert a legacy `(UntypedDataValue, DataSchema)` pair representing
/// a method **output** into a [`TypedSchemaValue`].
///
/// The resulting root [`SchemaType`] mirrors
/// [`super::data_schema::data_schema_to_output_schema`]:
/// - empty tuple → empty [`SchemaType::Tuple`];
/// - single tuple element → the element's schema and value inline;
/// - multi-element tuple → error. Golem agent methods only ever return 0 or
///   1 element, so a multi-element output tuple is rejected;
/// - multimodal → `list<variant<…>>` with the inner variant flagged
///   [`Role::Multimodal`].
///
/// Failure modes mirror [`untyped_data_value_to_typed_input`].
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
                // Multi-element output tuples are rejected by
                // `data_schema_to_output_schema` above, so this branch is
                // unreachable.
                _ => unreachable!("multi-element output tuples are rejected at the schema layer"),
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
            let list_value = multimodal_untyped_to_list_value(schema_elements, untyped_elements)?;
            Ok(TypedSchemaValue::new(
                SchemaGraph::anonymous(*root_type),
                list_value,
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
// Reverse: typed → UntypedDataValue
// ===========================================================================

/// Project an input parameter list `(InputSchema::Parameters, &[SchemaValue])`
/// back into a legacy [`UntypedDataValue`].
///
/// A single user-supplied `parts` field of type `list<variant<… Role::Multimodal>>`
/// projects back into [`UntypedDataValue::Multimodal`]; every other parameter
/// list projects into [`UntypedDataValue::Tuple`].
///
/// The two halves must have the same length and are zipped positionally:
/// the i-th `SchemaValue` is projected against the i-th parameter's
/// declared [`crate::schema::schema_type::SchemaType`] (resolved through
/// the optional schema graph carried by `schema`'s [`NamedField`]s — for
/// the legacy bridge, each field body is inline and self-contained, so an
/// anonymous graph is sufficient).
///
/// Per-element projection mirrors the forward direction: `SchemaValue::Text`
/// → `UnstructuredText`, `SchemaValue::Binary` → `UnstructuredBinary`,
/// everything else lowered via
/// [`super::value::schema_value_to_value`] and wrapped in
/// `ComponentModel`.
pub fn typed_input_to_untyped_data_value(
    schema: &InputSchema,
    values: &[SchemaValue],
) -> Result<UntypedDataValue, SchemaAdapterError> {
    let InputSchema::Parameters(fields) = schema;
    if fields.len() != values.len() {
        return Err(SchemaAdapterError::ValueShapeMismatch(format!(
            "input parameter arity mismatch: schema declares {} parameters, value has {}",
            fields.len(),
            values.len()
        )));
    }
    // The legacy `UntypedDataValue::Tuple` carries one inline element per
    // parameter; each parameter's `SchemaType` is self-contained, so an
    // anonymous graph for `schema_value_to_untyped_element` is sufficient.
    let graph = SchemaGraph::anonymous(SchemaType::tuple(Vec::new()));
    // Structural multimodal input: a single user-supplied `parts` field of
    // type `list<variant<… Role::Multimodal>>` projects back into a legacy
    // `UntypedDataValue::Multimodal` (one element per list entry).
    if let [field] = fields.as_slice()
        && matches!(field.source, FieldSource::UserSupplied)
        && let Some(cases) = as_multimodal_list_variant(&graph, &field.schema)?
        && let [
            SchemaValue::List {
                elements: list_values,
            },
        ] = values
    {
        let elements = multimodal_list_value_to_untyped(&graph, cases, list_values)?;
        return Ok(UntypedDataValue::Multimodal(elements));
    }
    let elements = fields
        .iter()
        .zip(values.iter())
        .map(|(field, value)| schema_value_to_untyped_element(&graph, &field.schema, value))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UntypedDataValue::Tuple(elements))
}

/// Project a multimodal `list<variant<…>>` value back into legacy
/// [`UntypedNamedElementValue`]s, matching each [`SchemaValue::Variant`] to its
/// declared case by index. Shared by the input and output multimodal reverse
/// conversions. A payload-less case cannot occur in a well-formed multimodal
/// value (each alternative carries an element value), so it is rejected.
fn multimodal_list_value_to_untyped(
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
    list_values: &[SchemaValue],
) -> Result<Vec<UntypedNamedElementValue>, SchemaAdapterError> {
    list_values
        .iter()
        .map(|elem| match elem {
            SchemaValue::Variant(VariantValuePayload { case, payload }) => {
                let case_ty = cases.get(*case as usize).ok_or_else(|| {
                    SchemaAdapterError::ValueShapeMismatch(format!(
                        "multimodal element case index `{case}` is out of range \
                         (variant has {} cases)",
                        cases.len()
                    ))
                })?;
                let (body_ty, body) = match (&case_ty.payload, payload) {
                    (Some(body_ty), Some(body)) => (body_ty, body),
                    _ => {
                        return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                            "multimodal element case `{}` must carry a payload",
                            case_ty.name
                        )));
                    }
                };
                let untyped = schema_value_to_untyped_element(graph, body_ty, body)?;
                Ok(UntypedNamedElementValue {
                    name: case_ty.name.clone(),
                    value: untyped,
                })
            }
            other => Err(SchemaAdapterError::ValueShapeMismatch(format!(
                "multimodal list element must be a Variant value, got: {other:?}"
            ))),
        })
        .collect::<Result<Vec<_>, SchemaAdapterError>>()
}

/// Project a [`TypedSchemaValue`] (always an **output**-shaped value, since
/// inputs travel as `(InputSchema, Vec<SchemaValue>)` instead) back into a
/// legacy [`UntypedDataValue`].
///
/// The decision between [`UntypedDataValue::Tuple`] and
/// [`UntypedDataValue::Multimodal`] is taken from the root [`SchemaType`]:
///
/// - `Tuple { elements: [] }` → `Tuple(vec![])` (canonical empty output).
/// - `List { element: Variant { metadata.role = Multimodal } }` →
///   `Multimodal(...)` with one [`UntypedNamedElementValue`] per list
///   element.
/// - any other root, including real user-defined records that are returned
///   as a single-element method output, → `Tuple(vec![single])` carrying the
///   whole value.
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
            SchemaType::List { element, .. },
            SchemaValue::List {
                elements: list_values,
            },
        ) => {
            let resolved_element = resolve_ref(graph, element)?;
            if let SchemaType::Variant { cases, metadata } = resolved_element
                && metadata.role == Some(Role::Multimodal)
            {
                let elements = multimodal_list_value_to_untyped(graph, cases, list_values)?;
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
