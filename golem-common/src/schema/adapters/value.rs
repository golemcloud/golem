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

//! `Value` / `ValueAndType` ↔ `SchemaValue` / `TypedSchemaValue` conversion.
//!
//! `SchemaValue` is structurally driven by the surrounding `SchemaType`, so
//! the value-only adapter (`value_to_schema_value`) takes a paired type
//! argument. For the common pair case use
//! [`value_and_type_to_typed_schema_value`] /
//! [`typed_schema_value_to_value_and_type`].

use golem_wasm::Value;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::{
    AnalysedType, NameTypePair, TypeEnum, TypeFlags, TypeList, TypeOption, TypeRecord, TypeResult,
    TypeTuple, TypeVariant,
};

use crate::schema::adapters::analysed_type::{
    analysed_type_to_schema_graph, schema_graph_to_analysed_type,
};
use crate::schema::adapters::error::SchemaAdapterError;
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};

/// Convert a `(Value, AnalysedType)` pair into a [`SchemaValue`] driven by
/// the converted schema type. The schema is supplied externally because
/// `Value` carries no structural type tag of its own.
pub fn value_to_schema_value(
    value: &Value,
    ty: &AnalysedType,
) -> Result<SchemaValue, SchemaAdapterError> {
    walk_value(value, ty)
}

/// Convert a legacy [`ValueAndType`] into a [`TypedSchemaValue`]. The
/// resulting `SchemaGraph` preserves named legacy composites as defs.
pub fn value_and_type_to_typed_schema_value(
    vat: &ValueAndType,
) -> Result<TypedSchemaValue, SchemaAdapterError> {
    let graph = analysed_type_to_schema_graph(&vat.typ)?;
    let value = walk_value(&vat.value, &vat.typ)?;
    Ok(TypedSchemaValue::new(graph, value))
}

/// Reverse: convert a [`SchemaValue`] into a legacy [`Value`] given the
/// driving [`SchemaType`] and enclosing [`SchemaGraph`]. Fails on rich
/// scalars, unions, capabilities, and other shapes that lack a legacy
/// counterpart.
pub fn schema_value_to_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<Value, SchemaAdapterError> {
    let mut visiting: Vec<TypeId> = Vec::new();
    walk_schema_value(graph, ty, value, &mut visiting)
}

/// Reverse: convert a [`TypedSchemaValue`] into a [`ValueAndType`]. The
/// schema must be representable as legacy `AnalysedType` (see
/// [`schema_graph_to_analysed_type`]).
pub fn typed_schema_value_to_value_and_type(
    typed: &TypedSchemaValue,
) -> Result<ValueAndType, SchemaAdapterError> {
    let typ = schema_graph_to_analysed_type(typed.graph())?;
    let mut visiting: Vec<TypeId> = Vec::new();
    let value = walk_schema_value(
        typed.graph(),
        typed.root_type(),
        typed.value(),
        &mut visiting,
    )?;
    Ok(ValueAndType { value, typ })
}

// --------------------------------------------------------------------------
// Forward
// --------------------------------------------------------------------------

fn walk_value(value: &Value, ty: &AnalysedType) -> Result<SchemaValue, SchemaAdapterError> {
    match (value, ty) {
        (Value::Bool(b), AnalysedType::Bool(_)) => Ok(SchemaValue::Bool(*b)),
        (Value::U8(v), AnalysedType::U8(_)) => Ok(SchemaValue::U8(*v)),
        (Value::U16(v), AnalysedType::U16(_)) => Ok(SchemaValue::U16(*v)),
        (Value::U32(v), AnalysedType::U32(_)) => Ok(SchemaValue::U32(*v)),
        (Value::U64(v), AnalysedType::U64(_)) => Ok(SchemaValue::U64(*v)),
        (Value::S8(v), AnalysedType::S8(_)) => Ok(SchemaValue::S8(*v)),
        (Value::S16(v), AnalysedType::S16(_)) => Ok(SchemaValue::S16(*v)),
        (Value::S32(v), AnalysedType::S32(_)) => Ok(SchemaValue::S32(*v)),
        (Value::S64(v), AnalysedType::S64(_)) => Ok(SchemaValue::S64(*v)),
        (Value::F32(v), AnalysedType::F32(_)) => Ok(SchemaValue::F32(*v)),
        (Value::F64(v), AnalysedType::F64(_)) => Ok(SchemaValue::F64(*v)),
        (Value::Char(c), AnalysedType::Chr(_)) => Ok(SchemaValue::Char(*c)),
        (Value::String(s), AnalysedType::Str(_)) => Ok(SchemaValue::String(s.clone())),
        (Value::List(items), AnalysedType::List(TypeList { inner, .. })) => {
            let elements = items
                .iter()
                .map(|v| walk_value(v, inner))
                .collect::<Result<_, _>>()?;
            Ok(SchemaValue::List { elements })
        }
        (Value::Tuple(items), AnalysedType::Tuple(TypeTuple { items: types, .. })) => {
            if items.len() != types.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "tuple arity mismatch: value has {} elements, schema declares {}",
                    items.len(),
                    types.len()
                )));
            }
            let elements = items
                .iter()
                .zip(types.iter())
                .map(|(v, t)| walk_value(v, t))
                .collect::<Result<_, _>>()?;
            Ok(SchemaValue::Tuple { elements })
        }
        (Value::Record(items), AnalysedType::Record(TypeRecord { fields, .. })) => {
            if items.len() != fields.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "record arity mismatch: value has {} fields, schema declares {}",
                    items.len(),
                    fields.len()
                )));
            }
            let fields = items
                .iter()
                .zip(fields.iter())
                .map(|(v, NameTypePair { typ, .. })| walk_value(v, typ))
                .collect::<Result<_, _>>()?;
            Ok(SchemaValue::Record { fields })
        }
        (
            Value::Variant {
                case_idx,
                case_value,
            },
            AnalysedType::Variant(TypeVariant { cases, .. }),
        ) => {
            let case_idx_usize = *case_idx as usize;
            let case = cases.get(case_idx_usize).ok_or_else(|| {
                SchemaAdapterError::ValueShapeMismatch(format!(
                    "variant case index {} out of range (variant has {} cases)",
                    case_idx,
                    cases.len()
                ))
            })?;
            let payload = match (case_value, &case.typ) {
                (Some(v), Some(t)) => Some(Box::new(walk_value(v, t)?)),
                (None, None) => None,
                (Some(_), None) => {
                    return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                        "variant case `{}` has no payload type but value carries a payload",
                        case.name
                    )));
                }
                (None, Some(_)) => {
                    return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                        "variant case `{}` declares a payload type but value has none",
                        case.name
                    )));
                }
            };
            Ok(SchemaValue::Variant(VariantValuePayload {
                case: *case_idx,
                payload,
            }))
        }
        (Value::Enum(case_idx), AnalysedType::Enum(TypeEnum { cases, .. })) => {
            if (*case_idx as usize) >= cases.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "enum case index {} out of range (enum has {} cases)",
                    case_idx,
                    cases.len()
                )));
            }
            Ok(SchemaValue::Enum { case: *case_idx })
        }
        (Value::Flags(bits), AnalysedType::Flags(TypeFlags { names, .. })) => {
            if bits.len() != names.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "flags bit count mismatch: value has {}, schema declares {}",
                    bits.len(),
                    names.len()
                )));
            }
            Ok(SchemaValue::Flags { bits: bits.clone() })
        }
        (
            Value::Option(inner),
            AnalysedType::Option(TypeOption {
                inner: ty_inner, ..
            }),
        ) => {
            let inner = match inner {
                Some(v) => Some(Box::new(walk_value(v, ty_inner)?)),
                None => None,
            };
            Ok(SchemaValue::Option { inner })
        }
        (Value::Result(payload), AnalysedType::Result(TypeResult { ok, err, .. })) => {
            let payload = match payload {
                Ok(opt) => {
                    let value = match (opt, ok) {
                        (Some(v), Some(t)) => Some(Box::new(walk_value(v, t)?)),
                        (None, None) => None,
                        (Some(_), None) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Ok` carries a value but schema declares unit ok".into(),
                            ));
                        }
                        (None, Some(_)) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Ok` declared a value type but the value is empty".into(),
                            ));
                        }
                    };
                    ResultValuePayload::Ok { value }
                }
                Err(opt) => {
                    let value = match (opt, err) {
                        (Some(v), Some(t)) => Some(Box::new(walk_value(v, t)?)),
                        (None, None) => None,
                        (Some(_), None) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Err` carries a value but schema declares unit err".into(),
                            ));
                        }
                        (None, Some(_)) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Err` declared a value type but the value is empty".into(),
                            ));
                        }
                    };
                    ResultValuePayload::Err { value }
                }
            };
            Ok(SchemaValue::Result(payload))
        }
        (Value::Handle { .. }, _) | (_, AnalysedType::Handle(_)) => {
            Err(SchemaAdapterError::LegacyHandle)
        }
        (v, t) => Err(SchemaAdapterError::ValueShapeMismatch(format!(
            "value/type combination not supported: {} / {}",
            value_kind(v),
            type_kind(t)
        ))),
    }
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "Bool",
        Value::U8(_) => "U8",
        Value::U16(_) => "U16",
        Value::U32(_) => "U32",
        Value::U64(_) => "U64",
        Value::S8(_) => "S8",
        Value::S16(_) => "S16",
        Value::S32(_) => "S32",
        Value::S64(_) => "S64",
        Value::F32(_) => "F32",
        Value::F64(_) => "F64",
        Value::Char(_) => "Char",
        Value::String(_) => "String",
        Value::List(_) => "List",
        Value::Tuple(_) => "Tuple",
        Value::Record(_) => "Record",
        Value::Variant { .. } => "Variant",
        Value::Enum(_) => "Enum",
        Value::Flags(_) => "Flags",
        Value::Option(_) => "Option",
        Value::Result(_) => "Result",
        Value::Handle { .. } => "Handle",
    }
}

fn type_kind(t: &AnalysedType) -> &'static str {
    match t {
        AnalysedType::Bool(_) => "Bool",
        AnalysedType::U8(_) => "U8",
        AnalysedType::U16(_) => "U16",
        AnalysedType::U32(_) => "U32",
        AnalysedType::U64(_) => "U64",
        AnalysedType::S8(_) => "S8",
        AnalysedType::S16(_) => "S16",
        AnalysedType::S32(_) => "S32",
        AnalysedType::S64(_) => "S64",
        AnalysedType::F32(_) => "F32",
        AnalysedType::F64(_) => "F64",
        AnalysedType::Chr(_) => "Chr",
        AnalysedType::Str(_) => "Str",
        AnalysedType::List(_) => "List",
        AnalysedType::Tuple(_) => "Tuple",
        AnalysedType::Record(_) => "Record",
        AnalysedType::Variant(_) => "Variant",
        AnalysedType::Enum(_) => "Enum",
        AnalysedType::Flags(_) => "Flags",
        AnalysedType::Option(_) => "Option",
        AnalysedType::Result(_) => "Result",
        AnalysedType::Handle(_) => "Handle",
    }
}

// --------------------------------------------------------------------------
// Reverse
// --------------------------------------------------------------------------

fn walk_schema_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    visiting: &mut Vec<TypeId>,
) -> Result<Value, SchemaAdapterError> {
    // Resolve refs first so caller code can ignore the indirection. Detect
    // cycles defensively so recursive schemas surface as an error rather than
    // a stack overflow even if the caller paired a recursive graph with a
    // (necessarily ill-typed, but still possible) value tree.
    if let SchemaType::Ref { id, .. } = ty {
        if visiting.iter().any(|x| x == id) {
            return Err(SchemaAdapterError::RecursiveRef(id.clone()));
        }
        let def = graph
            .lookup(id)
            .ok_or_else(|| SchemaAdapterError::DanglingRef(id.clone()))?;
        visiting.push(id.clone());
        let result = walk_schema_value(graph, &def.body, value, visiting);
        visiting.pop();
        return result;
    }

    match (value, ty) {
        (SchemaValue::Bool(b), SchemaType::Bool { .. }) => Ok(Value::Bool(*b)),
        (SchemaValue::U8(v), SchemaType::U8 { .. }) => Ok(Value::U8(*v)),
        (SchemaValue::U16(v), SchemaType::U16 { .. }) => Ok(Value::U16(*v)),
        (SchemaValue::U32(v), SchemaType::U32 { .. }) => Ok(Value::U32(*v)),
        (SchemaValue::U64(v), SchemaType::U64 { .. }) => Ok(Value::U64(*v)),
        (SchemaValue::S8(v), SchemaType::S8 { .. }) => Ok(Value::S8(*v)),
        (SchemaValue::S16(v), SchemaType::S16 { .. }) => Ok(Value::S16(*v)),
        (SchemaValue::S32(v), SchemaType::S32 { .. }) => Ok(Value::S32(*v)),
        (SchemaValue::S64(v), SchemaType::S64 { .. }) => Ok(Value::S64(*v)),
        (SchemaValue::F32(v), SchemaType::F32 { .. }) => Ok(Value::F32(*v)),
        (SchemaValue::F64(v), SchemaType::F64 { .. }) => Ok(Value::F64(*v)),
        (SchemaValue::Char(c), SchemaType::Char { .. }) => Ok(Value::Char(*c)),
        (SchemaValue::String(s), SchemaType::String { .. }) => Ok(Value::String(s.clone())),
        (SchemaValue::List { elements }, SchemaType::List { element, .. }) => {
            let items = elements
                .iter()
                .map(|v| walk_schema_value(graph, element, v, visiting))
                .collect::<Result<_, _>>()?;
            Ok(Value::List(items))
        }
        (
            SchemaValue::Tuple { elements },
            SchemaType::Tuple {
                elements: types, ..
            },
        ) => {
            if elements.len() != types.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "tuple arity mismatch: value has {} elements, schema declares {}",
                    elements.len(),
                    types.len()
                )));
            }
            let items = elements
                .iter()
                .zip(types.iter())
                .map(|(v, t)| walk_schema_value(graph, t, v, visiting))
                .collect::<Result<_, _>>()?;
            Ok(Value::Tuple(items))
        }
        (SchemaValue::Record { fields }, SchemaType::Record { fields: types, .. }) => {
            if fields.len() != types.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "record arity mismatch: value has {} fields, schema declares {}",
                    fields.len(),
                    types.len()
                )));
            }
            let items = fields
                .iter()
                .zip(types.iter())
                .map(|(v, f)| walk_schema_value(graph, &f.body, v, visiting))
                .collect::<Result<_, _>>()?;
            Ok(Value::Record(items))
        }
        (
            SchemaValue::Variant(VariantValuePayload { case, payload }),
            SchemaType::Variant { cases, .. },
        ) => {
            let case_meta = cases.get(*case as usize).ok_or_else(|| {
                SchemaAdapterError::ValueShapeMismatch(format!(
                    "variant case index {} out of range (variant has {} cases)",
                    case,
                    cases.len()
                ))
            })?;
            let case_value = match (payload, &case_meta.payload) {
                (Some(v), Some(t)) => Some(Box::new(walk_schema_value(graph, t, v, visiting)?)),
                (None, None) => None,
                (Some(_), None) => {
                    return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                        "variant case `{}` has no payload type but value carries one",
                        case_meta.name
                    )));
                }
                (None, Some(_)) => {
                    return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                        "variant case `{}` declares a payload type but value has none",
                        case_meta.name
                    )));
                }
            };
            Ok(Value::Variant {
                case_idx: *case,
                case_value,
            })
        }
        (SchemaValue::Enum { case }, SchemaType::Enum { cases, .. }) => {
            if (*case as usize) >= cases.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "enum case index {} out of range (enum has {} cases)",
                    case,
                    cases.len()
                )));
            }
            Ok(Value::Enum(*case))
        }
        (SchemaValue::Flags { bits }, SchemaType::Flags { flags, .. }) => {
            if bits.len() != flags.len() {
                return Err(SchemaAdapterError::ValueShapeMismatch(format!(
                    "flags bit count mismatch: value has {}, schema declares {}",
                    bits.len(),
                    flags.len()
                )));
            }
            Ok(Value::Flags(bits.clone()))
        }
        (
            SchemaValue::Option { inner },
            SchemaType::Option {
                inner: ty_inner, ..
            },
        ) => {
            let inner = match inner {
                Some(v) => Some(Box::new(walk_schema_value(graph, ty_inner, v, visiting)?)),
                None => None,
            };
            Ok(Value::Option(inner))
        }
        (SchemaValue::Result(payload), SchemaType::Result { spec, .. }) => {
            let payload = match payload {
                ResultValuePayload::Ok { value } => {
                    let value = match (value, &spec.ok) {
                        (Some(v), Some(t)) => {
                            Some(Box::new(walk_schema_value(graph, t, v, visiting)?))
                        }
                        (None, None) => None,
                        (Some(_), None) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Ok` carries a value but schema declares unit ok".into(),
                            ));
                        }
                        (None, Some(_)) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Ok` declared a value type but the value is empty".into(),
                            ));
                        }
                    };
                    Ok(value)
                }
                ResultValuePayload::Err { value } => {
                    let value = match (value, &spec.err) {
                        (Some(v), Some(t)) => {
                            Some(Box::new(walk_schema_value(graph, t, v, visiting)?))
                        }
                        (None, None) => None,
                        (Some(_), None) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Err` carries a value but schema declares unit err".into(),
                            ));
                        }
                        (None, Some(_)) => {
                            return Err(SchemaAdapterError::ValueShapeMismatch(
                                "result `Err` declared a value type but the value is empty".into(),
                            ));
                        }
                    };
                    Err(value)
                }
            };
            Ok(Value::Result(payload))
        }
        // Rich scalars / unions / capabilities have no legacy counterpart.
        (SchemaValue::Text(_), _)
        | (SchemaValue::Binary(_), _)
        | (SchemaValue::Path { .. }, _)
        | (SchemaValue::Url { .. }, _)
        | (SchemaValue::Datetime { .. }, _)
        | (SchemaValue::Duration(_), _)
        | (SchemaValue::Quantity(_), _)
        | (SchemaValue::Union(_), _)
        | (SchemaValue::Secret(_), _)
        | (SchemaValue::QuotaToken(_), _)
        | (SchemaValue::FixedList { .. }, _)
        | (SchemaValue::Map { .. }, _) => Err(SchemaAdapterError::LossySchemaType(
            "schema value carries a rich/capability node with no legacy counterpart".into(),
        )),
        (v, t) => Err(SchemaAdapterError::ValueShapeMismatch(format!(
            "schema-value / schema-type combination not supported: {:?} / {:?}",
            schema_value_kind(v),
            schema_type_kind(t),
        ))),
    }
}

fn schema_value_kind(v: &SchemaValue) -> &'static str {
    match v {
        SchemaValue::Bool(_) => "Bool",
        SchemaValue::S8(_) => "S8",
        SchemaValue::S16(_) => "S16",
        SchemaValue::S32(_) => "S32",
        SchemaValue::S64(_) => "S64",
        SchemaValue::U8(_) => "U8",
        SchemaValue::U16(_) => "U16",
        SchemaValue::U32(_) => "U32",
        SchemaValue::U64(_) => "U64",
        SchemaValue::F32(_) => "F32",
        SchemaValue::F64(_) => "F64",
        SchemaValue::Char(_) => "Char",
        SchemaValue::String(_) => "String",
        SchemaValue::Record { .. } => "Record",
        SchemaValue::Variant(_) => "Variant",
        SchemaValue::Enum { .. } => "Enum",
        SchemaValue::Flags { .. } => "Flags",
        SchemaValue::Tuple { .. } => "Tuple",
        SchemaValue::List { .. } => "List",
        SchemaValue::FixedList { .. } => "FixedList",
        SchemaValue::Map { .. } => "Map",
        SchemaValue::Option { .. } => "Option",
        SchemaValue::Result(_) => "Result",
        SchemaValue::Text(_) => "Text",
        SchemaValue::Binary(_) => "Binary",
        SchemaValue::Path { .. } => "Path",
        SchemaValue::Url { .. } => "Url",
        SchemaValue::Datetime { .. } => "Datetime",
        SchemaValue::Duration(_) => "Duration",
        SchemaValue::Quantity(_) => "Quantity",
        SchemaValue::Union(_) => "Union",
        SchemaValue::Secret(_) => "Secret",
        SchemaValue::QuotaToken(_) => "QuotaToken",
    }
}

fn schema_type_kind(t: &SchemaType) -> &'static str {
    match t {
        SchemaType::Ref { .. } => "Ref",
        SchemaType::Bool { .. } => "Bool",
        SchemaType::S8 { .. } => "S8",
        SchemaType::S16 { .. } => "S16",
        SchemaType::S32 { .. } => "S32",
        SchemaType::S64 { .. } => "S64",
        SchemaType::U8 { .. } => "U8",
        SchemaType::U16 { .. } => "U16",
        SchemaType::U32 { .. } => "U32",
        SchemaType::U64 { .. } => "U64",
        SchemaType::F32 { .. } => "F32",
        SchemaType::F64 { .. } => "F64",
        SchemaType::Char { .. } => "Char",
        SchemaType::String { .. } => "String",
        SchemaType::Record { .. } => "Record",
        SchemaType::Variant { .. } => "Variant",
        SchemaType::Enum { .. } => "Enum",
        SchemaType::Flags { .. } => "Flags",
        SchemaType::Tuple { .. } => "Tuple",
        SchemaType::List { .. } => "List",
        SchemaType::FixedList { .. } => "FixedList",
        SchemaType::Map { .. } => "Map",
        SchemaType::Option { .. } => "Option",
        SchemaType::Result { .. } => "Result",
        SchemaType::Text { .. } => "Text",
        SchemaType::Binary { .. } => "Binary",
        SchemaType::Path { .. } => "Path",
        SchemaType::Url { .. } => "Url",
        SchemaType::Datetime { .. } => "Datetime",
        SchemaType::Duration { .. } => "Duration",
        SchemaType::Quantity { .. } => "Quantity",
        SchemaType::Union { .. } => "Union",
        SchemaType::Secret { .. } => "Secret",
        SchemaType::QuotaToken { .. } => "QuotaToken",
        SchemaType::Future { .. } => "Future",
        SchemaType::Stream { .. } => "Stream",
    }
}
