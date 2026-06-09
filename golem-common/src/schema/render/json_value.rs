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

//! JSON value renderer: convert a `(SchemaGraph, SchemaType, SchemaValue)`
//! to and from a `serde_json::Value`.
//!
//! Rich scalars route through [`crate::schema::canonical`]; the renderer
//! never re-implements their text/JSON forms.

use crate::schema::canonical;
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::TypeId;
use crate::schema::render::error::RenderError;
use crate::schema::render::walker::{SchemaWalker, WalkerError, resolve_ref, walk};
use crate::schema::schema_type::{
    DiscriminatorRule, ResultSpec, SchemaType, UnionBranch, UnionSpec, VariantCaseType,
};
use crate::schema::schema_value::{
    ResultValuePayload, SchemaValue, UnionValuePayload, VariantValuePayload,
};
use serde_json::{Map, Number, Value};
use std::collections::HashSet;

/// Render a value tree to a `serde_json::Value`.
pub fn to_json_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<Value, RenderError> {
    let mut renderer = ToJsonRenderer {
        path: PathStack::new(),
    };
    drive(walk(&mut renderer, graph, ty, value))
}

/// Decode a `serde_json::Value` into a value tree typed by `ty` in `graph`.
pub fn from_json_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    json: &Value,
) -> Result<SchemaValue, RenderError> {
    let mut path = PathStack::new();
    let mut visited: HashSet<TypeId> = HashSet::new();
    from_json_inner(graph, ty, json, &mut path, &mut visited)
}

// --------------------------------------------------------------------- to

struct ToJsonRenderer {
    path: PathStack,
}

impl SchemaWalker for ToJsonRenderer {
    type Output = Value;
    type Error = RenderError;

    fn walk(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> Result<Value, RenderError> {
        encode(self, graph, ty, value)
    }
}

fn drive<T>(res: Result<T, WalkerError<RenderError>>) -> Result<T, RenderError> {
    match res {
        Ok(v) => Ok(v),
        Err(WalkerError::Walker(e)) => Err(e),
        Err(WalkerError::RefCycle(id)) => Err(RenderError::ValueMismatch {
            path: String::new(),
            reason: format!("reference cycle through `{id}`"),
        }),
        Err(WalkerError::DanglingRef(id)) => Err(RenderError::ValueMismatch {
            path: String::new(),
            reason: format!("dangling reference `{id}`"),
        }),
    }
}

fn encode(
    r: &mut ToJsonRenderer,
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<Value, RenderError> {
    match (ty, value) {
        (SchemaType::Ref { .. }, _) => unreachable!("walker resolves refs"),

        (SchemaType::Bool { .. }, SchemaValue::Bool(b)) => Ok(Value::Bool(*b)),
        (SchemaType::S8 { .. }, SchemaValue::S8(i)) => Ok(json_number_i64(*i as i64)),
        (SchemaType::S16 { .. }, SchemaValue::S16(i)) => Ok(json_number_i64(*i as i64)),
        (SchemaType::S32 { .. }, SchemaValue::S32(i)) => Ok(json_number_i64(*i as i64)),
        (SchemaType::S64 { .. }, SchemaValue::S64(i)) => Ok(json_number_i64(*i)),
        (SchemaType::U8 { .. }, SchemaValue::U8(u)) => Ok(json_number_u64(*u as u64)),
        (SchemaType::U16 { .. }, SchemaValue::U16(u)) => Ok(json_number_u64(*u as u64)),
        (SchemaType::U32 { .. }, SchemaValue::U32(u)) => Ok(json_number_u64(*u as u64)),
        (SchemaType::U64 { .. }, SchemaValue::U64(u)) => Ok(json_number_u64(*u)),
        (SchemaType::F32 { .. }, SchemaValue::F32(f)) => json_number_f64(*f as f64, &r.path),
        (SchemaType::F64 { .. }, SchemaValue::F64(f)) => json_number_f64(*f, &r.path),
        (SchemaType::Char { .. }, SchemaValue::Char(c)) => Ok(Value::String(c.to_string())),
        (SchemaType::String { .. }, SchemaValue::String(s)) => Ok(Value::String(s.clone())),

        (SchemaType::Text { .. }, SchemaValue::Text(p)) => Ok(canonical::text::to_json(p)),
        (SchemaType::Binary { .. }, SchemaValue::Binary(p)) => {
            canonical::binary::to_json(p).map_err(RenderError::from)
        }
        (SchemaType::Path { .. }, SchemaValue::Path { path }) => {
            canonical::path::to_json(path).map_err(RenderError::from)
        }
        (SchemaType::Url { .. }, SchemaValue::Url { url }) => {
            canonical::url::to_json(url).map_err(RenderError::from)
        }
        (SchemaType::Datetime { .. }, SchemaValue::Datetime { value }) => {
            canonical::datetime::to_json(value).map_err(RenderError::from)
        }
        (SchemaType::Duration { .. }, SchemaValue::Duration(p)) => {
            Ok(canonical::duration::to_json(p))
        }
        (SchemaType::Quantity { .. }, SchemaValue::Quantity(q)) => {
            Ok(canonical::quantity::to_json(q))
        }
        (SchemaType::Secret { .. }, SchemaValue::Secret(p)) => {
            canonical::secret::to_json(p).map_err(RenderError::from)
        }
        (SchemaType::QuotaToken { .. }, SchemaValue::QuotaToken(p)) => {
            canonical::quota_token::to_json(p).map_err(RenderError::from)
        }

        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs }) => {
            if fields.len() != vs.len() {
                return Err(r.mismatch(format!(
                    "record arity: expected {}, found {}",
                    fields.len(),
                    vs.len()
                )));
            }
            let mut obj = Map::new();
            for (field, value) in fields.iter().zip(vs.iter()) {
                r.path.push(PathSegment::Field(field.name.clone()));
                let rendered = drive(walk(r, graph, &field.body, value))?;
                r.path.pop();
                obj.insert(field.name.clone(), rendered);
            }
            Ok(Value::Object(obj))
        }

        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(vp)) => {
            let case_index = vp.case as usize;
            if case_index >= cases.len() {
                return Err(r.mismatch(format!(
                    "variant case index {} out of range (count {})",
                    vp.case,
                    cases.len()
                )));
            }
            let case: &VariantCaseType = &cases[case_index];
            match (&case.payload, &vp.payload) {
                (None, None) => Ok(Value::String(case.name.clone())),
                (Some(case_ty), Some(payload)) => {
                    r.path.push(PathSegment::Variant(case.name.clone()));
                    let rendered = drive(walk(r, graph, case_ty, payload))?;
                    r.path.pop();
                    let mut obj = Map::new();
                    obj.insert(case.name.clone(), rendered);
                    Ok(Value::Object(obj))
                }
                _ => Err(r.mismatch(format!(
                    "variant payload presence mismatch on case `{}`",
                    case.name
                ))),
            }
        }

        (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case }) => {
            let idx = *case as usize;
            if idx >= cases.len() {
                return Err(r.mismatch(format!(
                    "enum case index {} out of range (count {})",
                    case,
                    cases.len()
                )));
            }
            Ok(Value::String(cases[idx].clone()))
        }

        (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits }) => {
            if flags.len() != bits.len() {
                return Err(r.mismatch(format!(
                    "flags arity: expected {}, found {}",
                    flags.len(),
                    bits.len()
                )));
            }
            let selected: Vec<Value> = flags
                .iter()
                .zip(bits.iter())
                .filter(|(_, on)| **on)
                .map(|(name, _)| Value::String(name.clone()))
                .collect();
            Ok(Value::Array(selected))
        }

        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: vs }) => {
            if elements.len() != vs.len() {
                return Err(r.mismatch(format!(
                    "tuple arity: expected {}, found {}",
                    elements.len(),
                    vs.len()
                )));
            }
            let mut out = Vec::with_capacity(vs.len());
            for (i, (et, ev)) in elements.iter().zip(vs.iter()).enumerate() {
                r.path.push(PathSegment::Index(i));
                let rendered = drive(walk(r, graph, et, ev))?;
                r.path.pop();
                out.push(rendered);
            }
            Ok(Value::Array(out))
        }

        (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
            let mut out = Vec::with_capacity(elements.len());
            for (i, ev) in elements.iter().enumerate() {
                r.path.push(PathSegment::Index(i));
                let rendered = drive(walk(r, graph, element, ev))?;
                r.path.pop();
                out.push(rendered);
            }
            Ok(Value::Array(out))
        }

        (
            SchemaType::FixedList {
                element, length, ..
            },
            SchemaValue::FixedList { elements },
        ) => {
            if elements.len() as u32 != *length {
                return Err(r.mismatch(format!(
                    "fixed list length: expected {}, found {}",
                    length,
                    elements.len()
                )));
            }
            let mut out = Vec::with_capacity(elements.len());
            for (i, ev) in elements.iter().enumerate() {
                r.path.push(PathSegment::Index(i));
                let rendered = drive(walk(r, graph, element, ev))?;
                r.path.pop();
                out.push(rendered);
            }
            Ok(Value::Array(out))
        }

        (SchemaType::Map { key, value, .. }, SchemaValue::Map { entries }) => {
            let mut out = Vec::with_capacity(entries.len());
            for (i, (k, v)) in entries.iter().enumerate() {
                r.path.push(PathSegment::MapKey(i));
                let rk = drive(walk(r, graph, key, k))?;
                r.path.pop();
                r.path.push(PathSegment::MapValue(i));
                let rv = drive(walk(r, graph, value, v))?;
                r.path.pop();
                out.push(Value::Array(vec![rk, rv]));
            }
            Ok(Value::Array(out))
        }

        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: v }) => match v {
            None => Ok(Value::Null),
            Some(inner_value) => {
                r.path.push(PathSegment::OptionInner);
                let rendered = drive(walk(r, graph, inner, inner_value))?;
                r.path.pop();
                Ok(rendered)
            }
        },

        (SchemaType::Result { spec, .. }, SchemaValue::Result(payload)) => {
            encode_result(r, graph, spec, payload)
        }

        (SchemaType::Union { spec, metadata }, SchemaValue::Union(payload)) => {
            let multimodal = matches!(
                metadata.role,
                Some(crate::schema::metadata::Role::Multimodal)
            );
            encode_union(r, graph, spec, payload, multimodal)
        }

        (SchemaType::Future { .. }, _) | (SchemaType::Stream { .. }, _) => Err(
            RenderError::Unsupported("future/stream values have no JSON representation"),
        ),

        (ty, value) => Err(r.mismatch(format!(
            "shape mismatch: expected {}, found {}",
            type_name(ty),
            value_name(value)
        ))),
    }
}

fn encode_result(
    r: &mut ToJsonRenderer,
    graph: &SchemaGraph,
    spec: &ResultSpec,
    payload: &ResultValuePayload,
) -> Result<Value, RenderError> {
    let mut obj = Map::new();
    match payload {
        ResultValuePayload::Ok {
            value: payload_value,
        } => {
            let v = match (spec.ok.as_deref(), payload_value.as_deref()) {
                (None, None) => Value::Null,
                (Some(ok_ty), Some(inner)) => {
                    r.path.push(PathSegment::Ok);
                    let v = drive(walk(r, graph, ok_ty, inner))?;
                    r.path.pop();
                    v
                }
                _ => {
                    return Err(r.mismatch("result ok payload presence mismatch".to_string()));
                }
            };
            obj.insert("ok".to_string(), v);
        }
        ResultValuePayload::Err {
            value: payload_value,
        } => {
            let v = match (spec.err.as_deref(), payload_value.as_deref()) {
                (None, None) => Value::Null,
                (Some(err_ty), Some(inner)) => {
                    r.path.push(PathSegment::Err);
                    let v = drive(walk(r, graph, err_ty, inner))?;
                    r.path.pop();
                    v
                }
                _ => {
                    return Err(r.mismatch("result err payload presence mismatch".to_string()));
                }
            };
            obj.insert("err".to_string(), v);
        }
    }
    Ok(Value::Object(obj))
}

fn encode_union(
    r: &mut ToJsonRenderer,
    graph: &SchemaGraph,
    spec: &UnionSpec,
    payload: &UnionValuePayload,
    multimodal: bool,
) -> Result<Value, RenderError> {
    let branch = find_branch(spec, &payload.tag)
        .ok_or_else(|| r.mismatch(format!("unknown union branch tag `{}`", payload.tag)))?;
    // Encode the body first; the on-wire union form is the body's JSON.
    r.path.push(PathSegment::Union(payload.tag.clone()));
    let rendered = drive(walk(r, graph, &branch.body, &payload.body))?;
    r.path.pop();
    // Sanity check: the produced JSON should match the branch's
    // discriminator rule. Validation should have caught a tag/body
    // disagreement at construction time; this is the runtime safety net.
    // Multimodal unions are positionally tagged in their outer envelope
    // and carry placeholder discriminator rules per branch, so the rule
    // check does not apply.
    if !multimodal && !rule_matches(&branch.discriminator, &rendered) {
        return Err(RenderError::UnionTagMismatch {
            tag: payload.tag.clone(),
            reason: format!(
                "encoded body does not satisfy discriminator {}",
                rule_label(&branch.discriminator)
            ),
        });
    }
    Ok(rendered)
}

fn find_branch<'a>(spec: &'a UnionSpec, tag: &str) -> Option<&'a UnionBranch> {
    spec.branches.iter().find(|b| b.tag == tag)
}

impl ToJsonRenderer {
    fn mismatch(&self, reason: String) -> RenderError {
        RenderError::ValueMismatch {
            path: self.path.render(),
            reason,
        }
    }
}

// --------------------------------------------------------------------- from

fn from_json_inner(
    graph: &SchemaGraph,
    ty: &SchemaType,
    json: &Value,
    path: &mut PathStack,
    visited: &mut HashSet<TypeId>,
) -> Result<SchemaValue, RenderError> {
    // Route through the shared ref-resolution helper so the decoder uses
    // the same cycle protection as the walker-based encoder.
    let res = resolve_ref::<_, SchemaValue, RenderError>(graph, ty, visited, |graph, body| {
        match from_json_body(graph, body, json, path, &mut HashSet::new()) {
            Ok(v) => Ok(v),
            Err(e) => Err(WalkerError::Walker(e)),
        }
    });
    drive_with_path(res, path)
}

fn drive_with_path(
    res: Result<SchemaValue, WalkerError<RenderError>>,
    path: &PathStack,
) -> Result<SchemaValue, RenderError> {
    match res {
        Ok(v) => Ok(v),
        Err(WalkerError::Walker(e)) => Err(e),
        Err(WalkerError::RefCycle(id)) => Err(RenderError::ValueMismatch {
            path: path.render(),
            reason: format!("reference cycle through `{id}`"),
        }),
        Err(WalkerError::DanglingRef(id)) => Err(RenderError::ValueMismatch {
            path: path.render(),
            reason: format!("dangling reference `{id}`"),
        }),
    }
}

fn from_json_body(
    graph: &SchemaGraph,
    ty: &SchemaType,
    json: &Value,
    path: &mut PathStack,
    _local_visited: &mut HashSet<TypeId>,
) -> Result<SchemaValue, RenderError> {
    // The `visited` HashSet for nested references is provided by the caller
    // through `resolve_ref`. For sub-recursions we re-enter `from_json_inner`
    // with a fresh set per sibling traversal because ref protection is
    // scoped to the active stack of references, not the entire walk.
    let mut visited: HashSet<TypeId> = HashSet::new();
    match ty {
        SchemaType::Ref { .. } => unreachable!("ref resolved by resolve_ref"),
        SchemaType::Bool { .. } => match json.as_bool() {
            Some(b) => Ok(SchemaValue::Bool(b)),
            None => Err(mismatch(path, "expected JSON boolean".to_string())),
        },
        SchemaType::S8 { .. } => {
            check_int_range::<i8>(json, path)?;
            Ok(SchemaValue::S8(json_i64(json, path)? as i8))
        }
        SchemaType::S16 { .. } => {
            check_int_range::<i16>(json, path)?;
            Ok(SchemaValue::S16(json_i64(json, path)? as i16))
        }
        SchemaType::S32 { .. } => {
            check_int_range::<i32>(json, path)?;
            Ok(SchemaValue::S32(json_i64(json, path)? as i32))
        }
        SchemaType::S64 { .. } => Ok(SchemaValue::S64(json_i64(json, path)?)),
        SchemaType::U8 { .. } => {
            check_int_range::<u8>(json, path)?;
            Ok(SchemaValue::U8(json_u64(json, path)? as u8))
        }
        SchemaType::U16 { .. } => {
            check_int_range::<u16>(json, path)?;
            Ok(SchemaValue::U16(json_u64(json, path)? as u16))
        }
        SchemaType::U32 { .. } => {
            check_int_range::<u32>(json, path)?;
            Ok(SchemaValue::U32(json_u64(json, path)? as u32))
        }
        SchemaType::U64 { .. } => Ok(SchemaValue::U64(json_u64(json, path)?)),
        SchemaType::F32 { .. } => {
            let value = json_f64(json, path)?;
            check_f32_in_range(value, path)?;
            Ok(SchemaValue::F32(value as f32))
        }
        SchemaType::F64 { .. } => {
            // `serde_json::Number` only stores finite floats, but assert
            // it here as a defensive insurance so round-tripping never
            // emits a non-finite number.
            let value = json_f64(json, path)?;
            if !value.is_finite() {
                return Err(mismatch(path, "f64 must be finite".to_string()));
            }
            Ok(SchemaValue::F64(value))
        }
        SchemaType::Char { .. } => match json.as_str() {
            Some(s) => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(SchemaValue::Char(c)),
                    _ => Err(mismatch(
                        path,
                        "expected JSON string with exactly one char".to_string(),
                    )),
                }
            }
            None => Err(mismatch(path, "expected JSON string".to_string())),
        },
        SchemaType::String { .. } => match json.as_str() {
            Some(s) => Ok(SchemaValue::String(s.to_string())),
            None => Err(mismatch(path, "expected JSON string".to_string())),
        },

        SchemaType::Text { .. } => {
            let p = canonical::text::from_json(json)?;
            Ok(SchemaValue::Text(p))
        }
        SchemaType::Binary { .. } => {
            let p = canonical::binary::from_json(json)?;
            Ok(SchemaValue::Binary(p))
        }
        SchemaType::Path { .. } => {
            let s = canonical::path::from_json(json)?;
            Ok(SchemaValue::Path { path: s })
        }
        SchemaType::Url { .. } => {
            let s = canonical::url::from_json(json)?;
            Ok(SchemaValue::Url { url: s })
        }
        SchemaType::Datetime { .. } => {
            let dt = canonical::datetime::from_json(json)?;
            Ok(SchemaValue::Datetime { value: dt })
        }
        SchemaType::Duration { .. } => {
            let p = canonical::duration::from_json(json)?;
            Ok(SchemaValue::Duration(p))
        }
        SchemaType::Quantity { .. } => {
            let q = canonical::quantity::from_json(json)?;
            Ok(SchemaValue::Quantity(q))
        }
        SchemaType::Secret { .. } => {
            let p = canonical::secret::from_json(json)?;
            Ok(SchemaValue::Secret(p))
        }
        SchemaType::QuotaToken { .. } => {
            let p = canonical::quota_token::from_json(json)?;
            Ok(SchemaValue::QuotaToken(p))
        }

        SchemaType::Record { fields, .. } => {
            let obj = json
                .as_object()
                .ok_or_else(|| mismatch(path, "expected JSON object for record".to_string()))?;
            // Strict boundary check: every JSON field must be declared by
            // the schema record.
            let known: HashSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            for key in obj.keys() {
                if !known.contains(key.as_str()) {
                    return Err(RenderError::UnexpectedField {
                        record: "anonymous".to_string(),
                        field: key.clone(),
                    });
                }
            }
            let mut out = Vec::with_capacity(fields.len());
            for field in fields.iter() {
                let value = obj.get(&field.name).ok_or_else(|| {
                    mismatch(path, format!("missing record field `{}`", field.name))
                })?;
                path.push(PathSegment::Field(field.name.clone()));
                let v = from_json_inner(graph, &field.body, value, path, &mut visited)?;
                path.pop();
                out.push(v);
            }
            Ok(SchemaValue::Record { fields: out })
        }

        SchemaType::Variant { cases, .. } => match json {
            Value::String(name) => {
                let (idx, case) = find_variant_case(cases, name)
                    .ok_or_else(|| mismatch(path, format!("unknown variant case `{name}`")))?;
                if case.payload.is_some() {
                    return Err(mismatch(
                        path,
                        format!("variant case `{name}` requires a payload"),
                    ));
                }
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: idx as u32,
                    payload: None,
                }))
            }
            Value::Object(obj) if obj.len() == 1 => {
                let (name, payload_json) = obj.iter().next().unwrap();
                let (idx, case) = find_variant_case(cases, name)
                    .ok_or_else(|| mismatch(path, format!("unknown variant case `{name}`")))?;
                let payload_ty = case.payload.as_ref().ok_or_else(|| {
                    mismatch(
                        path,
                        format!("variant case `{name}` does not accept a payload"),
                    )
                })?;
                path.push(PathSegment::Variant(name.clone()));
                let inner = from_json_inner(graph, payload_ty, payload_json, path, &mut visited)?;
                path.pop();
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: idx as u32,
                    payload: Some(Box::new(inner)),
                }))
            }
            _ => Err(mismatch(
                path,
                "expected JSON string or single-entry object for variant".to_string(),
            )),
        },

        SchemaType::Enum { cases, .. } => {
            let name = json
                .as_str()
                .ok_or_else(|| mismatch(path, "expected JSON string for enum".to_string()))?;
            let idx = cases
                .iter()
                .position(|c| c == name)
                .ok_or_else(|| mismatch(path, format!("unknown enum case `{name}`")))?;
            Ok(SchemaValue::Enum { case: idx as u32 })
        }

        SchemaType::Flags { flags, .. } => {
            let arr = json
                .as_array()
                .ok_or_else(|| mismatch(path, "expected JSON array for flags".to_string()))?;
            let mut selected: HashSet<&str> = HashSet::new();
            for item in arr {
                let name = item.as_str().ok_or_else(|| {
                    mismatch(path, "expected JSON string in flags array".to_string())
                })?;
                if !flags.iter().any(|f| f == name) {
                    return Err(mismatch(path, format!("unknown flag `{name}`")));
                }
                if !selected.insert(name) {
                    return Err(RenderError::DuplicateFlag {
                        flag: name.to_string(),
                    });
                }
            }
            let bits: Vec<bool> = flags
                .iter()
                .map(|f| selected.contains(f.as_str()))
                .collect();
            Ok(SchemaValue::Flags { bits })
        }

        SchemaType::Tuple { elements, .. } => {
            let arr = json
                .as_array()
                .ok_or_else(|| mismatch(path, "expected JSON array for tuple".to_string()))?;
            if arr.len() != elements.len() {
                return Err(mismatch(
                    path,
                    format!(
                        "tuple arity: expected {}, found {}",
                        elements.len(),
                        arr.len()
                    ),
                ));
            }
            let mut out = Vec::with_capacity(arr.len());
            for (i, (et, ev)) in elements.iter().zip(arr.iter()).enumerate() {
                path.push(PathSegment::Index(i));
                let v = from_json_inner(graph, et, ev, path, &mut visited)?;
                path.pop();
                out.push(v);
            }
            Ok(SchemaValue::Tuple { elements: out })
        }

        SchemaType::List { element, .. } => {
            let arr = json
                .as_array()
                .ok_or_else(|| mismatch(path, "expected JSON array for list".to_string()))?;
            let mut out = Vec::with_capacity(arr.len());
            for (i, ev) in arr.iter().enumerate() {
                path.push(PathSegment::Index(i));
                let v = from_json_inner(graph, element, ev, path, &mut visited)?;
                path.pop();
                out.push(v);
            }
            Ok(SchemaValue::List { elements: out })
        }

        SchemaType::FixedList {
            element, length, ..
        } => {
            let arr = json
                .as_array()
                .ok_or_else(|| mismatch(path, "expected JSON array for fixed list".to_string()))?;
            if arr.len() as u32 != *length {
                return Err(mismatch(
                    path,
                    format!(
                        "fixed list length: expected {}, found {}",
                        length,
                        arr.len()
                    ),
                ));
            }
            let mut out = Vec::with_capacity(arr.len());
            for (i, ev) in arr.iter().enumerate() {
                path.push(PathSegment::Index(i));
                let v = from_json_inner(graph, element, ev, path, &mut visited)?;
                path.pop();
                out.push(v);
            }
            Ok(SchemaValue::FixedList { elements: out })
        }

        SchemaType::Map { key, value, .. } => {
            let arr = json
                .as_array()
                .ok_or_else(|| mismatch(path, "expected JSON array for map".to_string()))?;
            let mut out: Vec<(SchemaValue, SchemaValue)> = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let pair = item.as_array().ok_or_else(|| {
                    mismatch(path, "expected `[k, v]` array for map entry".to_string())
                })?;
                if pair.len() != 2 {
                    return Err(mismatch(
                        path,
                        format!(
                            "map entry must have exactly 2 elements, found {}",
                            pair.len()
                        ),
                    ));
                }
                path.push(PathSegment::MapKey(i));
                let k = from_json_inner(graph, key, &pair[0], path, &mut visited)?;
                path.pop();
                path.push(PathSegment::MapValue(i));
                let v = from_json_inner(graph, value, &pair[1], path, &mut visited)?;
                path.pop();
                out.push((k, v));
            }
            Ok(SchemaValue::Map { entries: out })
        }

        SchemaType::Option { inner, .. } => match json {
            Value::Null => Ok(SchemaValue::Option { inner: None }),
            other => {
                path.push(PathSegment::OptionInner);
                let v = from_json_inner(graph, inner, other, path, &mut visited)?;
                path.pop();
                Ok(SchemaValue::Option {
                    inner: Some(Box::new(v)),
                })
            }
        },

        SchemaType::Result { spec, .. } => decode_result(graph, spec, json, path, &mut visited),

        SchemaType::Union { spec, metadata } => decode_union(graph, spec, metadata, json, path),

        SchemaType::Future { .. } | SchemaType::Stream { .. } => Err(RenderError::Unsupported(
            "future/stream values have no JSON representation",
        )),
    }
}

fn decode_result(
    graph: &SchemaGraph,
    spec: &ResultSpec,
    json: &Value,
    path: &mut PathStack,
    visited: &mut HashSet<TypeId>,
) -> Result<SchemaValue, RenderError> {
    let obj = json
        .as_object()
        .ok_or_else(|| mismatch(path, "expected JSON object for result".to_string()))?;
    if obj.len() != 1 {
        return Err(mismatch(
            path,
            "result object must have exactly one key `ok` or `err`".to_string(),
        ));
    }
    let (key, payload_json) = obj.iter().next().unwrap();
    match key.as_str() {
        "ok" => {
            let value = match (spec.ok.as_deref(), payload_json) {
                (None, Value::Null) => None,
                (Some(ok_ty), other) => {
                    path.push(PathSegment::Ok);
                    let v = from_json_inner(graph, ok_ty, other, path, visited)?;
                    path.pop();
                    Some(Box::new(v))
                }
                _ => {
                    return Err(mismatch(
                        path,
                        "result ok payload presence mismatch".to_string(),
                    ));
                }
            };
            Ok(SchemaValue::Result(ResultValuePayload::Ok { value }))
        }
        "err" => {
            let value = match (spec.err.as_deref(), payload_json) {
                (None, Value::Null) => None,
                (Some(err_ty), other) => {
                    path.push(PathSegment::Err);
                    let v = from_json_inner(graph, err_ty, other, path, visited)?;
                    path.pop();
                    Some(Box::new(v))
                }
                _ => {
                    return Err(mismatch(
                        path,
                        "result err payload presence mismatch".to_string(),
                    ));
                }
            };
            Ok(SchemaValue::Result(ResultValuePayload::Err { value }))
        }
        other => Err(mismatch(path, format!("unexpected result key `{other}`"))),
    }
}

fn decode_union(
    graph: &SchemaGraph,
    spec: &UnionSpec,
    metadata: &crate::schema::metadata::MetadataEnvelope,
    json: &Value,
    path: &mut PathStack,
) -> Result<SchemaValue, RenderError> {
    // Multimodal unions are positionally tagged in their outer envelope
    // (a `list<union<…>>` whose element index picks the branch). The bare
    // union body cannot be decoded by this generic discriminator-based
    // pipeline because every branch carries a placeholder discriminator
    // rule. Picking a branch by body shape would silently mis-tag values
    // whenever two branches accept the same JSON shape, so we refuse
    // explicitly and let multimodal-aware callers decode through their
    // own envelope.
    if matches!(
        metadata.role,
        Some(crate::schema::metadata::Role::Multimodal)
    ) {
        return Err(RenderError::Unsupported(
            "multimodal union JSON decoding requires an external multimodal envelope",
        ));
    }
    // First: find every branch whose discriminator rule matches the
    // incoming JSON value. Validation rules out multi-match at construction
    // time; a runtime safety net catches the case where the value is bad.
    let mut matched: Vec<&UnionBranch> = Vec::new();
    for branch in spec.branches.iter() {
        if rule_matches(&branch.discriminator, json) {
            matched.push(branch);
        }
    }
    match matched.as_slice() {
        [] => Err(RenderError::UnionNoMatch),
        [branch] => {
            // Then: decode the body against the matched branch.
            path.push(PathSegment::Union(branch.tag.clone()));
            let body = from_json_inner(graph, &branch.body, json, path, &mut HashSet::new())?;
            path.pop();
            Ok(SchemaValue::Union(UnionValuePayload {
                tag: branch.tag.clone(),
                body: Box::new(body),
            }))
        }
        multiple => Err(RenderError::UnionAmbiguous {
            matched: multiple.iter().map(|b| b.tag.clone()).collect(),
        }),
    }
}

// ----------------------------------------------------------- discriminators

/// Whether a [`DiscriminatorRule`] matches a raw JSON value.
fn rule_matches(rule: &DiscriminatorRule, json: &Value) -> bool {
    match rule {
        DiscriminatorRule::Prefix { prefix } => json
            .as_str()
            .map(|s| s.starts_with(prefix.as_str()))
            .unwrap_or(false),
        DiscriminatorRule::Suffix { suffix } => json
            .as_str()
            .map(|s| s.ends_with(suffix.as_str()))
            .unwrap_or(false),
        DiscriminatorRule::Contains { substring } => json
            .as_str()
            .map(|s| s.contains(substring.as_str()))
            .unwrap_or(false),
        DiscriminatorRule::Regex { regex } => match (json.as_str(), regex::Regex::new(regex)) {
            (Some(s), Ok(re)) => re.is_match(s),
            _ => false,
        },
        DiscriminatorRule::FieldEquals(disc) => json
            .as_object()
            .and_then(|obj| obj.get(disc.field_name.as_str()))
            .map(|v| match &disc.literal {
                None => true,
                Some(lit) => v.as_str().map(|s| s == lit.as_str()).unwrap_or(false),
            })
            .unwrap_or(false),
        DiscriminatorRule::FieldAbsent { field_name } => json
            .as_object()
            .map(|obj| !obj.contains_key(field_name.as_str()))
            .unwrap_or(false),
    }
}

fn rule_label(rule: &DiscriminatorRule) -> String {
    match rule {
        DiscriminatorRule::Prefix { prefix } => format!("prefix `{prefix}`"),
        DiscriminatorRule::Suffix { suffix } => format!("suffix `{suffix}`"),
        DiscriminatorRule::Contains { substring } => format!("contains `{substring}`"),
        DiscriminatorRule::Regex { regex } => format!("regex `{regex}`"),
        DiscriminatorRule::FieldEquals(disc) => match &disc.literal {
            None => format!("field `{}` present", disc.field_name),
            Some(lit) => format!("field `{}` == `{lit}`", disc.field_name),
        },
        DiscriminatorRule::FieldAbsent { field_name } => format!("field `{field_name}` absent"),
    }
}

// -------------------------------------------------------------- small bits

fn find_variant_case<'a>(
    cases: &'a [VariantCaseType],
    name: &str,
) -> Option<(usize, &'a VariantCaseType)> {
    cases.iter().enumerate().find(|(_, c)| c.name == name)
}

fn json_i64(json: &Value, path: &mut PathStack) -> Result<i64, RenderError> {
    json.as_i64()
        .ok_or_else(|| mismatch(path, "expected JSON integer".to_string()))
}

fn json_u64(json: &Value, path: &mut PathStack) -> Result<u64, RenderError> {
    json.as_u64()
        .ok_or_else(|| mismatch(path, "expected JSON non-negative integer".to_string()))
}

fn json_f64(json: &Value, path: &mut PathStack) -> Result<f64, RenderError> {
    json.as_f64()
        .ok_or_else(|| mismatch(path, "expected JSON number".to_string()))
}

fn json_number_i64(value: i64) -> Value {
    Value::Number(Number::from(value))
}

fn json_number_u64(value: u64) -> Value {
    Value::Number(Number::from(value))
}

fn json_number_f64(value: f64, path: &PathStack) -> Result<Value, RenderError> {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| RenderError::Json(format!("non-finite float at {}", path.render())))
}

/// Reject f64 inputs whose magnitude exceeds the f32 range, where `value as
/// f32` would silently saturate to `±inf`. NaN is rejected because the JSON
/// number type cannot represent it on round-trip.
fn check_f32_in_range(value: f64, path: &mut PathStack) -> Result<(), RenderError> {
    if !value.is_finite() {
        return Err(mismatch(path, "f32 must be finite".to_string()));
    }
    if value.abs() > f32::MAX as f64 {
        return Err(mismatch(path, "f32 out of range".to_string()));
    }
    Ok(())
}

fn check_int_range<T: TryFrom<i128>>(
    json: &Value,
    path: &mut PathStack,
) -> Result<(), RenderError> {
    let n = json
        .as_i64()
        .map(i128::from)
        .or_else(|| json.as_u64().map(i128::from))
        .ok_or_else(|| mismatch(path, "expected JSON integer".to_string()))?;
    T::try_from(n)
        .map(|_| ())
        .map_err(|_| mismatch(path, "integer out of range".to_string()))
}

fn mismatch(path: &PathStack, reason: String) -> RenderError {
    RenderError::ValueMismatch {
        path: path.render(),
        reason,
    }
}

fn type_name(ty: &SchemaType) -> &'static str {
    match ty {
        SchemaType::Ref { .. } => "ref",
        SchemaType::Bool { .. } => "bool",
        SchemaType::S8 { .. } => "s8",
        SchemaType::S16 { .. } => "s16",
        SchemaType::S32 { .. } => "s32",
        SchemaType::S64 { .. } => "s64",
        SchemaType::U8 { .. } => "u8",
        SchemaType::U16 { .. } => "u16",
        SchemaType::U32 { .. } => "u32",
        SchemaType::U64 { .. } => "u64",
        SchemaType::F32 { .. } => "f32",
        SchemaType::F64 { .. } => "f64",
        SchemaType::Char { .. } => "char",
        SchemaType::String { .. } => "string",
        SchemaType::Record { .. } => "record",
        SchemaType::Variant { .. } => "variant",
        SchemaType::Enum { .. } => "enum",
        SchemaType::Flags { .. } => "flags",
        SchemaType::Tuple { .. } => "tuple",
        SchemaType::List { .. } => "list",
        SchemaType::FixedList { .. } => "fixed-list",
        SchemaType::Map { .. } => "map",
        SchemaType::Option { .. } => "option",
        SchemaType::Result { .. } => "result",
        SchemaType::Text { .. } => "text",
        SchemaType::Binary { .. } => "binary",
        SchemaType::Path { .. } => "path",
        SchemaType::Url { .. } => "url",
        SchemaType::Datetime { .. } => "datetime",
        SchemaType::Duration { .. } => "duration",
        SchemaType::Quantity { .. } => "quantity",
        SchemaType::Union { .. } => "union",
        SchemaType::Secret { .. } => "secret",
        SchemaType::QuotaToken { .. } => "quota-token",
        SchemaType::Future { .. } => "future",
        SchemaType::Stream { .. } => "stream",
    }
}

fn value_name(value: &SchemaValue) -> &'static str {
    match value {
        SchemaValue::Bool(_) => "bool",
        SchemaValue::S8(_) => "s8",
        SchemaValue::S16(_) => "s16",
        SchemaValue::S32(_) => "s32",
        SchemaValue::S64(_) => "s64",
        SchemaValue::U8(_) => "u8",
        SchemaValue::U16(_) => "u16",
        SchemaValue::U32(_) => "u32",
        SchemaValue::U64(_) => "u64",
        SchemaValue::F32(_) => "f32",
        SchemaValue::F64(_) => "f64",
        SchemaValue::Char(_) => "char",
        SchemaValue::String(_) => "string",
        SchemaValue::Record { .. } => "record",
        SchemaValue::Variant(_) => "variant",
        SchemaValue::Enum { .. } => "enum",
        SchemaValue::Flags { .. } => "flags",
        SchemaValue::Tuple { .. } => "tuple",
        SchemaValue::List { .. } => "list",
        SchemaValue::FixedList { .. } => "fixed-list",
        SchemaValue::Map { .. } => "map",
        SchemaValue::Option { .. } => "option",
        SchemaValue::Result(_) => "result",
        SchemaValue::Text(_) => "text",
        SchemaValue::Binary(_) => "binary",
        SchemaValue::Path { .. } => "path",
        SchemaValue::Url { .. } => "url",
        SchemaValue::Datetime { .. } => "datetime",
        SchemaValue::Duration(_) => "duration",
        SchemaValue::Quantity(_) => "quantity",
        SchemaValue::Union(_) => "union",
        SchemaValue::Secret(_) => "secret",
        SchemaValue::QuotaToken(_) => "quota-token",
    }
}

// ---------------------------------------------------------------- path stack

#[derive(Clone, Debug)]
enum PathSegment {
    Field(String),
    Variant(String),
    Index(usize),
    OptionInner,
    Ok,
    Err,
    Union(String),
    MapKey(usize),
    MapValue(usize),
}

struct PathStack {
    segments: Vec<PathSegment>,
}

impl PathStack {
    fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    fn push(&mut self, segment: PathSegment) {
        self.segments.push(segment);
    }

    fn pop(&mut self) {
        self.segments.pop();
    }

    fn render(&self) -> String {
        let mut s = String::new();
        for seg in &self.segments {
            match seg {
                PathSegment::Field(n) => {
                    s.push('.');
                    s.push_str(n);
                }
                PathSegment::Variant(n) => {
                    s.push('.');
                    s.push_str(n);
                }
                PathSegment::Index(i) => {
                    s.push('[');
                    s.push_str(&i.to_string());
                    s.push(']');
                }
                PathSegment::OptionInner => s.push_str(".some"),
                PathSegment::Ok => s.push_str(".ok"),
                PathSegment::Err => s.push_str(".err"),
                PathSegment::Union(n) => {
                    s.push('.');
                    s.push_str(n);
                }
                PathSegment::MapKey(i) => {
                    s.push_str(".key[");
                    s.push_str(&i.to_string());
                    s.push(']');
                }
                PathSegment::MapValue(i) => {
                    s.push_str(".value[");
                    s.push_str(&i.to_string());
                    s.push(']');
                }
            }
        }
        if s.is_empty() { "$".to_string() } else { s }
    }
}
