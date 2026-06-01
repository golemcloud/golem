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
//!   - many → [`OutputSchema::Single`] wrapped in a [`SchemaType::Record`]
//! - `DataSchema::Multimodal(elements)` as **input** → error
//!   (multimodal is an output-only concern).
//! - `DataSchema::Multimodal(elements)` as **output** →
//!   [`OutputSchema::Single`] wrapping a `list<union<…>>` where the inner
//!   union has one branch per named element and carries
//!   [`Role::Multimodal`] on its metadata envelope. This matches §4.1 of
//!   the design doc (multimodal = composable `list<union<…>>` with the
//!   intent annotation on the inner element type).
//!
//! Reverse (`InputSchema` / `OutputSchema` → `DataSchema`) is partial:
//!
//! - [`InputSchema::Parameters`] only round-trips when every field's source
//!   is [`FieldSource::UserSupplied`] (legacy `DataSchema` has no notion of
//!   auto-injected fields).
//! - [`OutputSchema::Unit`] → empty tuple.
//! - [`OutputSchema::Single`] with a [`SchemaType::Record`] → tuple form.
//! - [`OutputSchema::Single`] wrapping a `list<union<…>>` where the union
//!   carries `Role::Multimodal` → `DataSchema::Multimodal`.
//! - Other [`OutputSchema::Single`] shapes round-trip as a single-element
//!   tuple with the synthetic name `"value"`.

use crate::base_model::agent::{DataSchema, NamedElementSchema, NamedElementSchemas};
use crate::schema::adapters::element_schema::{
    element_schema_to_schema_type, schema_type_to_element_schema,
};
use crate::schema::adapters::error::{SchemaAdapterError, resolve_ref};
use crate::schema::agent::{FieldSource, InputSchema, NamedField, OutputSchema};
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::Role;
use crate::schema::schema_type::{
    DiscriminatorRule, NamedFieldType, SchemaType, UnionBranch, UnionSpec,
};

/// The synthetic name used when reverse-converting an
/// [`OutputSchema::Single`] whose body is not a record back into a
/// single-element [`DataSchema::Tuple`].
const FALLBACK_OUTPUT_FIELD_NAME: &str = "value";

// --------------------------------------------------------------------------
// Forward: DataSchema → InputSchema / OutputSchema
// --------------------------------------------------------------------------

/// Convert a [`DataSchema`] in input position into an [`InputSchema`].
///
/// Fails if `schema` is [`DataSchema::Multimodal`]; multimodal is an
/// output-only concept.
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
        DataSchema::Multimodal(_) => Err(SchemaAdapterError::LossySchemaType(
            "multimodal DataSchema is output-only and cannot become an InputSchema".into(),
        )),
    }
}

/// Convert a [`DataSchema`] in output position into an [`OutputSchema`].
///
/// - `Tuple` arity 0 → [`OutputSchema::Unit`].
/// - `Tuple` arity 1 → [`OutputSchema::Single`] containing the element's
///   schema directly.
/// - `Tuple` arity ≥ 2 → [`OutputSchema::Single`] wrapping a
///   [`SchemaType::Record`].
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
            many => {
                let fields = many
                    .iter()
                    .map(|e| {
                        Ok(NamedFieldType {
                            name: e.name.clone(),
                            body: element_schema_to_schema_type(&e.schema)?,
                            metadata: Default::default(),
                        })
                    })
                    .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
                Ok(OutputSchema::Single(Box::new(SchemaType::record(fields))))
            }
        },
        DataSchema::Multimodal(NamedElementSchemas { elements }) => {
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
                        // inferred-tag discriminator pipeline: the
                        // alternative is carried positionally inside the
                        // outer `list` envelope. The validator special-cases
                        // `Role::Multimodal` and skips per-branch structural
                        // discriminator checks, so this slot only needs to
                        // satisfy the type; pick the cheapest legal rule.
                        discriminator: DiscriminatorRule::FieldAbsent {
                            field_name: String::new(),
                        },
                        metadata: Default::default(),
                    })
                })
                .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
            let mut union = SchemaType::union(UnionSpec { branches });
            union.metadata_mut().role = Some(Role::Multimodal);
            Ok(OutputSchema::Single(Box::new(SchemaType::list(union))))
        }
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
/// - `Single(Record)` → `DataSchema::Tuple` with one named element per
///   record field.
/// - `Single(list<union<…>>)` whose inner union metadata role is
///   [`Role::Multimodal`] → `DataSchema::Multimodal` (one alternative per
///   union branch, using the branch's tag as the alternative name).
/// - any other `Single(_)` → `DataSchema::Tuple` with a single
///   [`FALLBACK_OUTPUT_FIELD_NAME`] element. This is inherently lossy
///   because non-record single outputs in the schema layer carry no field
///   name, so they all rehydrate under the same synthetic name.
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
            SchemaType::Record { fields, .. } => {
                let elements = fields
                    .iter()
                    .map(|f| {
                        Ok(NamedElementSchema {
                            name: f.name.clone(),
                            schema: schema_type_to_element_schema(graph, &f.body)?,
                        })
                    })
                    .collect::<Result<Vec<_>, SchemaAdapterError>>()?;
                Ok(DataSchema::Tuple(NamedElementSchemas { elements }))
            }
            other => synthetic_single_element(graph, other),
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
