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

//! `DataSchema` ↔ `InputSchema` / `OutputSchema` conversion.
//!
//! Mapping (forward):
//!
//! - `DataSchema::Tuple(elements)` as **input** → [`InputSchema::Parameters`]:
//!   each named element becomes a [`NamedField`] with
//!   [`FieldSource::UserSupplied`].
//! - `DataSchema::Tuple(elements)` as **output** → [`OutputSchema`]:
//!   - empty → [`OutputSchema::Unit`]
//!   - single → [`OutputSchema::Single`] with the element's schema inline
//!   - many → error. Golem agent methods only ever return 0 or 1 element;
//!     multi-element output tuples are not supported.
//! - `DataSchema::Multimodal(elements)` as **input** →
//!   [`InputSchema::Parameters`] with a single synthetic
//!   [`MULTIMODAL_PARTS_FIELD_NAME`] field whose schema is the structural
//!   multimodal form `list<variant<… Role::Multimodal>>`. Multimodal is a
//!   valid input at the model level; consumers that cannot represent it
//!   (e.g. an agent constructor exposed over MCP) enforce that as a
//!   separate, consumer-specific validation rather than failing here.
//! - `DataSchema::Multimodal(elements)` as **output** →
//!   [`OutputSchema::Single`] wrapping a `list<variant<…>>` where the inner
//!   variant has one case per named element and carries
//!   [`Role::Multimodal`] on its metadata envelope. Multimodal is modelled
//!   as a tagged sum (`variant`), not an inferred-tag `union`, because each
//!   part carries its alternative name and the alternatives are not
//!   distinguishable by a structural discriminator.
//!
//! Reverse (`InputSchema` / `OutputSchema` → `DataSchema`) is partial:
//!
//! - [`InputSchema::Parameters`] with a single [`MULTIMODAL_PARTS_FIELD_NAME`]
//!   field carrying the structural multimodal form
//!   `list<variant<… Role::Multimodal>>` round-trips back to
//!   `DataSchema::Multimodal`.
//! - Any other [`InputSchema::Parameters`] only round-trips when every
//!   field's source is [`FieldSource::UserSupplied`] (legacy `DataSchema`
//!   has no notion of auto-injected fields).
//! - [`OutputSchema::Unit`] → empty tuple.
//! - [`OutputSchema::Single`] wrapping a `list<variant<…>>` where the variant
//!   carries `Role::Multimodal` → `DataSchema::Multimodal`.
//! - Any other [`OutputSchema::Single`] shape (including a real user-defined
//!   [`SchemaType::Record`]) round-trips as a single-element tuple with the
//!   synthetic name `"value"`. The single-element output is the only legal
//!   shape, so the reverse never flattens.

use crate::base_model::agent::{DataSchema, NamedElementSchema, NamedElementSchemas};
use crate::schema::adapters::element_schema::{
    element_schema_to_schema_type, schema_type_to_element_schema,
};
use crate::schema::adapters::error::{SchemaAdapterError, resolve_ref};
use crate::schema::agent::{FieldSource, InputSchema, NamedField, OutputSchema};
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::Role;
use crate::schema::schema_type::{SchemaType, VariantCaseType};

/// The synthetic name used when reverse-converting an
/// [`OutputSchema::Single`] back into a single-element [`DataSchema::Tuple`].
///
/// The new schema model carries no output element name (an agent method
/// returns 0 or 1 positional value, §4.7), so consumers that need a JSON
/// object key for the single return value (e.g. the MCP exporter, which must
/// advertise an `object` output schema) use this same name to stay in sync
/// with the reverse adapter.
pub const FALLBACK_OUTPUT_FIELD_NAME: &str = "value";

/// The synthetic parameter name used to carry a multimodal input as a single
/// field of the structural form `list<variant<… Role::Multimodal>>` inside an
/// [`InputSchema::Parameters`]. Shared with the consumers that render or
/// extract multimodal inputs (e.g. the MCP exporter's `parts` array) so the
/// name stays consistent across the forward conversion, the reverse
/// conversion, and the protocol surface.
pub const MULTIMODAL_PARTS_FIELD_NAME: &str = "parts";

/// Build the structural form of a multimodal schema: a `list<variant<…>>`
/// whose inner [`SchemaType::Variant`] carries [`Role::Multimodal`] on its
/// metadata, with one case per named element. Shared by the input and
/// output multimodal conversions.
///
/// Multimodal is a *tagged* sum (each part carries its alternative name), so
/// it is modelled as a [`SchemaType::Variant`] rather than a
/// [`SchemaType::Union`]: a variant is self-contained and round-trips through
/// the generic value codec for any payload type, whereas an inferred-tag
/// union would need every alternative to be distinguishable by a structural
/// discriminator (which multimodal parts are not). The [`Role::Multimodal`]
/// marker only distinguishes a multimodal variant from an ordinary
/// user-defined `list<variant<…>>`; it does not change codec or validation
/// behaviour.
fn multimodal_elements_to_list_variant(
    elements: &[NamedElementSchema],
) -> Result<SchemaType, SchemaAdapterError> {
    if elements.is_empty() {
        return Err(SchemaAdapterError::LossySchemaType(
            "multimodal DataSchema has no alternatives".into(),
        ));
    }
    let cases = elements
        .iter()
        .map(|e| {
            let body = element_schema_to_schema_type(&e.schema)?;
            Ok(VariantCaseType {
                name: e.name.clone(),
                payload: Some(body),
                metadata: Default::default(),
            })
        })
        .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
    let mut variant = SchemaType::variant(cases);
    variant.metadata_mut().role = Some(Role::Multimodal);
    Ok(SchemaType::list(variant))
}

/// If `ty` (resolved against `graph`) is the structural multimodal form
/// `list<variant<… Role::Multimodal>>`, return the variant's cases (one per
/// named alternative, with the case `name` carrying the alternative name).
///
/// Public, graph-aware detector used by consumers (e.g. the MCP exporter)
/// that need to special-case multimodal schemas.
pub fn multimodal_variant_cases<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a [VariantCaseType]>, SchemaAdapterError> {
    as_multimodal_list_variant(graph, ty)
}

/// Whether `ty` (resolved against `graph`) is the structural multimodal form
/// `list<variant<… Role::Multimodal>>`.
pub fn is_multimodal_schema_type(
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<bool, SchemaAdapterError> {
    Ok(as_multimodal_list_variant(graph, ty)?.is_some())
}

/// If `ty` (resolved against `graph`) is the structural multimodal form
/// `list<variant<… Role::Multimodal>>`, return the variant's cases.
pub(crate) fn as_multimodal_list_variant<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a [VariantCaseType]>, SchemaAdapterError> {
    if let SchemaType::List { element, .. } = resolve_ref(graph, ty)?
        && let SchemaType::Variant { cases, metadata } = resolve_ref(graph, element)?
        && metadata.role == Some(Role::Multimodal)
    {
        return Ok(Some(cases));
    }
    Ok(None)
}

// --------------------------------------------------------------------------
// Forward: DataSchema → InputSchema / OutputSchema
// --------------------------------------------------------------------------

/// Convert a [`DataSchema`] in input position into an [`InputSchema`].
///
/// - `Tuple` → [`InputSchema::Parameters`] with one user-supplied field per
///   named element.
/// - `Multimodal` → [`InputSchema::Parameters`] carrying a single
///   user-supplied [`MULTIMODAL_PARTS_FIELD_NAME`] field whose schema is the
///   structural form `list<variant<… Role::Multimodal>>`.
pub fn data_schema_to_input_schema(schema: &DataSchema) -> Result<InputSchema, SchemaAdapterError> {
    match schema {
        DataSchema::Tuple(NamedElementSchemas { elements }) => {
            let fields = elements
                .iter()
                .map(|e| {
                    Ok(NamedField::user_supplied(
                        e.name.clone(),
                        element_schema_to_schema_type(&e.schema)?,
                    ))
                })
                .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
            Ok(InputSchema::Parameters(fields))
        }
        DataSchema::Multimodal(NamedElementSchemas { elements }) => {
            let parts = multimodal_elements_to_list_variant(elements)?;
            Ok(InputSchema::Parameters(vec![NamedField::user_supplied(
                MULTIMODAL_PARTS_FIELD_NAME,
                parts,
            )]))
        }
    }
}

/// Convert a [`DataSchema`] in output position into an [`OutputSchema`].
///
/// - `Tuple` arity 0 → [`OutputSchema::Unit`].
/// - `Tuple` arity 1 → [`OutputSchema::Single`] containing the element's
///   schema directly.
/// - `Tuple` arity ≥ 2 → error. Golem agent methods only ever return 0 or 1
///   element; multi-element output tuples are not supported.
/// - `Multimodal` (any arity) → [`OutputSchema::Single`] wrapping a
///   `list<variant<…>>` whose inner [`SchemaType::Variant`] metadata carries
///   [`Role::Multimodal`].
pub fn data_schema_to_output_schema(
    schema: &DataSchema,
) -> Result<OutputSchema, SchemaAdapterError> {
    match schema {
        DataSchema::Tuple(NamedElementSchemas { elements }) => match elements.as_slice() {
            [] => Ok(OutputSchema::Unit),
            [single] => Ok(OutputSchema::Single(Box::new(
                element_schema_to_schema_type(&single.schema)?,
            ))),
            many => Err(SchemaAdapterError::ValueShapeMismatch(format!(
                "output DataSchema with {} tuple elements is not supported; \
                 Golem agent methods must declare 0 or 1 output element",
                many.len()
            ))),
        },
        DataSchema::Multimodal(NamedElementSchemas { elements }) => Ok(OutputSchema::Single(
            Box::new(multimodal_elements_to_list_variant(elements)?),
        )),
    }
}

// --------------------------------------------------------------------------
// Reverse: InputSchema / OutputSchema → DataSchema
// --------------------------------------------------------------------------

/// Reverse: project an [`InputSchema`] back into a legacy [`DataSchema`].
///
/// Only [`FieldSource::UserSupplied`] fields are projected; auto-injected
/// fields (e.g. the host-provided [`Principal`](crate::schema::agent::AutoInjectedKind::Principal))
/// are out-of-band and are **omitted** from the legacy `DataSchema`, since the
/// legacy data model has no representation for them and they are filled in by
/// the host at invocation time rather than supplied by the caller.
pub fn input_schema_to_data_schema(
    graph: &SchemaGraph,
    input: &InputSchema,
) -> Result<DataSchema, SchemaAdapterError> {
    match input {
        InputSchema::Parameters(fields) => {
            // Structural multimodal input: a single user-supplied field whose
            // schema is `list<variant<… Role::Multimodal>>` projects back to a
            // legacy multimodal `DataSchema` (one alternative per case).
            if let [field] = fields.as_slice()
                && matches!(field.source, FieldSource::UserSupplied)
                && let Some(cases) = as_multimodal_list_variant(graph, &field.schema)?
            {
                let elements = multimodal_cases_to_elements(graph, cases)?;
                return Ok(DataSchema::Multimodal(NamedElementSchemas { elements }));
            }
            let elements = fields
                .iter()
                .filter(|f| matches!(f.source, FieldSource::UserSupplied))
                .map(|f| {
                    let element_schema = schema_type_to_element_schema(graph, &f.schema)?;
                    Ok(NamedElementSchema {
                        name: f.name.clone(),
                        schema: element_schema,
                    })
                })
                .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
            Ok(DataSchema::Tuple(NamedElementSchemas { elements }))
        }
    }
}

/// Reverse: project an [`OutputSchema`] back into a legacy [`DataSchema`].
///
/// - `Unit` → empty `DataSchema::Tuple`.
/// - `Single(list<variant<…>>)` whose inner variant metadata role is
///   [`Role::Multimodal`] → `DataSchema::Multimodal` (one alternative per
///   variant case, using the case's name as the alternative name).
/// - any other `Single(_)` (including a real user-defined
///   [`SchemaType::Record`]) → `DataSchema::Tuple` with a single
///   [`FALLBACK_OUTPUT_FIELD_NAME`] element. This is inherently lossy
///   because the schema layer carries no field name for the single output,
///   so single outputs all rehydrate under the same synthetic name.
pub fn output_schema_to_data_schema(
    graph: &SchemaGraph,
    output: &OutputSchema,
) -> Result<DataSchema, SchemaAdapterError> {
    match output {
        OutputSchema::Unit => Ok(DataSchema::Tuple(NamedElementSchemas { elements: vec![] })),
        OutputSchema::Single(top_ty) => match resolve_ref(graph, top_ty)? {
            SchemaType::List { element, .. } => {
                // Multimodal output: `list<variant<...> with Role::Multimodal>`.
                if let SchemaType::Variant { cases, metadata } = resolve_ref(graph, element)?
                    && metadata.role == Some(Role::Multimodal)
                {
                    let elements = multimodal_cases_to_elements(graph, cases)?;
                    return Ok(DataSchema::Multimodal(NamedElementSchemas { elements }));
                }
                synthetic_single_element(graph, top_ty)
            }
            _ => synthetic_single_element(graph, top_ty),
        },
    }
}

/// Project the cases of a multimodal `list<variant<… Role::Multimodal>>`
/// back into legacy [`NamedElementSchema`]s: each case becomes an alternative
/// named after the case, schema taken from the case payload. A payload-less
/// case cannot occur in a well-formed multimodal schema (every alternative
/// carries an element schema), so it is rejected.
fn multimodal_cases_to_elements(
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
) -> Result<Vec<NamedElementSchema>, SchemaAdapterError> {
    cases
        .iter()
        .map(|case| {
            let body = case.payload.as_ref().ok_or_else(|| {
                SchemaAdapterError::LossySchemaType(format!(
                    "multimodal variant case `{}` has no payload; legacy DataSchema \
                     multimodal alternatives must carry an element schema",
                    case.name
                ))
            })?;
            Ok(NamedElementSchema {
                name: case.name.clone(),
                schema: schema_type_to_element_schema(graph, body)?,
            })
        })
        .collect::<Result<Vec<_>, SchemaAdapterError>>()
}

fn synthetic_single_element(
    graph: &SchemaGraph,
    body: &SchemaType,
) -> Result<DataSchema, SchemaAdapterError> {
    let element_schema = schema_type_to_element_schema(graph, body)?;
    Ok(DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: FALLBACK_OUTPUT_FIELD_NAME.to_string(),
            schema: element_schema,
        }],
    }))
}
