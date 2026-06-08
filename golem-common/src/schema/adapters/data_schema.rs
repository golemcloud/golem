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
//!   multimodal form `list<union<… Role::Multimodal>>`. Multimodal is a
//!   valid input at the model level; consumers that cannot represent it
//!   (e.g. an agent constructor exposed over MCP) enforce that as a
//!   separate, consumer-specific validation rather than failing here.
//! - `DataSchema::Multimodal(elements)` as **output** →
//!   [`OutputSchema::Single`] wrapping a `list<union<…>>` where the inner
//!   union has one branch per named element and carries
//!   [`Role::Multimodal`] on its metadata envelope. This matches §4.1 of
//!   the design doc (multimodal = composable `list<union<…>>` with the
//!   intent annotation on the inner element type).
//!
//! Reverse (`InputSchema` / `OutputSchema` → `DataSchema`) is partial:
//!
//! - [`InputSchema::Parameters`] with a single [`MULTIMODAL_PARTS_FIELD_NAME`]
//!   field carrying the structural multimodal form
//!   `list<union<… Role::Multimodal>>` round-trips back to
//!   `DataSchema::Multimodal`.
//! - Any other [`InputSchema::Parameters`] only round-trips when every
//!   field's source is [`FieldSource::UserSupplied`] (legacy `DataSchema`
//!   has no notion of auto-injected fields).
//! - [`OutputSchema::Unit`] → empty tuple.
//! - [`OutputSchema::Single`] wrapping a `list<union<…>>` where the union
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
use crate::schema::schema_type::{DiscriminatorRule, SchemaType, UnionBranch, UnionSpec};

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
/// field of the structural form `list<union<… Role::Multimodal>>` inside an
/// [`InputSchema::Parameters`]. Shared with the consumers that render or
/// extract multimodal inputs (e.g. the MCP exporter's `parts` array) so the
/// name stays consistent across the forward conversion, the reverse
/// conversion, and the protocol surface.
pub const MULTIMODAL_PARTS_FIELD_NAME: &str = "parts";

/// Build the structural form of a multimodal schema: a `list<union<…>>`
/// whose inner [`SchemaType::Union`] carries [`Role::Multimodal`] on its
/// metadata, with one branch per named element. Shared by the input and
/// output multimodal conversions.
fn multimodal_elements_to_list_union(
    elements: &[NamedElementSchema],
) -> Result<SchemaType, SchemaAdapterError> {
    if elements.is_empty() {
        return Err(SchemaAdapterError::LossySchemaType(
            "multimodal DataSchema has no alternatives".into(),
        ));
    }
    let branches = elements
        .iter()
        .map(|e| {
            let body = element_schema_to_schema_type(&e.schema)?;
            Ok(UnionBranch {
                tag: e.name.clone(),
                body,
                // Multimodal unions are not resolved by the generic
                // inferred-tag discriminator pipeline: the alternative is
                // carried positionally inside the outer `list` envelope. The
                // validator special-cases `Role::Multimodal` and skips
                // per-branch structural discriminator checks, so this slot
                // only needs to satisfy the type; pick the cheapest legal
                // rule.
                discriminator: DiscriminatorRule::FieldAbsent {
                    field_name: String::new(),
                },
                metadata: Default::default(),
            })
        })
        .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
    let mut union = SchemaType::union(UnionSpec { branches });
    union.metadata_mut().role = Some(Role::Multimodal);
    Ok(SchemaType::list(union))
}

/// If `ty` (resolved against `graph`) is the structural multimodal form
/// `list<union<… Role::Multimodal>>`, return the union's branches (one per
/// named alternative, with the branch `tag` carrying the alternative name).
///
/// Public, graph-aware detector used by consumers (e.g. the MCP exporter)
/// that need to special-case multimodal schemas.
pub fn multimodal_union_branches<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a [UnionBranch]>, SchemaAdapterError> {
    as_multimodal_list_union(graph, ty)
}

/// Whether `ty` (resolved against `graph`) is the structural multimodal form
/// `list<union<… Role::Multimodal>>`.
pub fn is_multimodal_schema_type(
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<bool, SchemaAdapterError> {
    Ok(as_multimodal_list_union(graph, ty)?.is_some())
}

/// If `ty` (resolved against `graph`) is the structural multimodal form
/// `list<union<… Role::Multimodal>>`, return the union's branches.
pub(crate) fn as_multimodal_list_union<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a [UnionBranch]>, SchemaAdapterError> {
    if let SchemaType::List { element, .. } = resolve_ref(graph, ty)?
        && let SchemaType::Union { spec, metadata } = resolve_ref(graph, element)?
        && metadata.role == Some(Role::Multimodal)
    {
        return Ok(Some(&spec.branches));
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
///   structural form `list<union<… Role::Multimodal>>`.
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
            let parts = multimodal_elements_to_list_union(elements)?;
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
///   `list<union<…>>` whose inner [`SchemaType::Union`] metadata carries
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
            Box::new(multimodal_elements_to_list_union(elements)?),
        )),
    }
}

// --------------------------------------------------------------------------
// Reverse: InputSchema / OutputSchema → DataSchema
// --------------------------------------------------------------------------

/// Reverse: project an [`InputSchema`] back into a legacy [`DataSchema`].
///
/// All fields must use [`FieldSource::UserSupplied`]; the legacy data model
/// has no representation for auto-injected fields.
pub fn input_schema_to_data_schema(
    graph: &SchemaGraph,
    input: &InputSchema,
) -> Result<DataSchema, SchemaAdapterError> {
    match input {
        InputSchema::Parameters(fields) => {
            // Structural multimodal input: a single user-supplied field whose
            // schema is `list<union<… Role::Multimodal>>` projects back to a
            // legacy multimodal `DataSchema` (one alternative per branch).
            if let [field] = fields.as_slice()
                && matches!(field.source, FieldSource::UserSupplied)
                && let Some(branches) = as_multimodal_list_union(graph, &field.schema)?
            {
                let elements = branches
                    .iter()
                    .map(|UnionBranch { tag, body, .. }| {
                        Ok(NamedElementSchema {
                            name: tag.clone(),
                            schema: schema_type_to_element_schema(graph, body)?,
                        })
                    })
                    .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
                return Ok(DataSchema::Multimodal(NamedElementSchemas { elements }));
            }
            let elements = fields
                .iter()
                .map(|f| {
                    if !matches!(f.source, FieldSource::UserSupplied) {
                        return Err(SchemaAdapterError::LossySchemaType(format!(
                            "InputSchema field `{}` is auto-injected; legacy DataSchema cannot \
                             encode auto-injected fields",
                            f.name
                        )));
                    }
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
/// - `Single(list<union<…>>)` whose inner union metadata role is
///   [`Role::Multimodal`] → `DataSchema::Multimodal` (one alternative per
///   union branch, using the branch's tag as the alternative name).
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
                // Multimodal output: `list<union<...> with Role::Multimodal>`.
                if let SchemaType::Union { spec, metadata } = resolve_ref(graph, element)?
                    && metadata.role == Some(Role::Multimodal)
                {
                    let elements = spec
                        .branches
                        .iter()
                        .map(|UnionBranch { tag, body, .. }| {
                            Ok(NamedElementSchema {
                                name: tag.clone(),
                                schema: schema_type_to_element_schema(graph, body)?,
                            })
                        })
                        .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
                    return Ok(DataSchema::Multimodal(NamedElementSchemas { elements }));
                }
                synthetic_single_element(graph, top_ty)
            }
            _ => synthetic_single_element(graph, top_ty),
        },
    }
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
