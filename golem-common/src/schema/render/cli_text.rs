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

//! Concise text renderers for [`SchemaType`] and [`SchemaValue`], used in
//! CLI help text, error messages and doc emission.
//!
//! Rich scalars route through [`crate::schema::canonical`] for their text
//! forms.

use crate::schema::canonical;
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::TypeId;
use crate::schema::render::error::RenderError;
use crate::schema::render::walker::{SchemaWalker, walk};
use crate::schema::schema_type::{
    DiscriminatorRule, ResultSpec, SchemaType, UnionBranch, VariantCaseType,
};
use crate::schema::schema_value::{ResultValuePayload, SchemaValue, UnionValuePayload};
use std::collections::HashSet;

/// Render a [`SchemaType`] as a concise text description.
pub fn type_to_cli_text(graph: &SchemaGraph, ty: &SchemaType) -> String {
    let mut visited = HashSet::new();
    type_to_text_inner(graph, ty, &mut visited)
}

fn type_to_text_inner(
    graph: &SchemaGraph,
    ty: &SchemaType,
    visited: &mut HashSet<TypeId>,
) -> String {
    let base = match ty {
        SchemaType::Ref { id, .. } => {
            if !visited.insert(id.clone()) {
                return decorate_with_metadata(id.0.clone(), ty);
            }
            let out = match graph.lookup(id) {
                Some(def) => def
                    .name
                    .clone()
                    .unwrap_or_else(|| type_to_text_inner(graph, &def.body, visited)),
                None => id.0.clone(),
            };
            visited.remove(id);
            out
        }
        SchemaType::Bool { .. } => "bool".to_string(),
        SchemaType::S8 { .. } => "s8".to_string(),
        SchemaType::S16 { .. } => "s16".to_string(),
        SchemaType::S32 { .. } => "s32".to_string(),
        SchemaType::S64 { .. } => "s64".to_string(),
        SchemaType::U8 { .. } => "u8".to_string(),
        SchemaType::U16 { .. } => "u16".to_string(),
        SchemaType::U32 { .. } => "u32".to_string(),
        SchemaType::U64 { .. } => "u64".to_string(),
        SchemaType::F32 { .. } => "f32".to_string(),
        SchemaType::F64 { .. } => "f64".to_string(),
        SchemaType::Char { .. } => "char".to_string(),
        SchemaType::String { .. } => "string".to_string(),
        SchemaType::Record { fields, .. } => {
            let inner = fields
                .iter()
                .map(|f| {
                    format!(
                        "{}: {}",
                        f.name,
                        type_to_text_inner(graph, &f.body, visited)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("record {{ {inner} }}")
        }
        SchemaType::Variant { cases, .. } => {
            let inner = cases
                .iter()
                .map(|c: &VariantCaseType| match &c.payload {
                    None => c.name.clone(),
                    Some(p) => {
                        format!("{}({})", c.name, type_to_text_inner(graph, p, visited))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("variant {{ {inner} }}")
        }
        SchemaType::Enum { cases, .. } => {
            format!("enum {{ {} }}", cases.join(", "))
        }
        SchemaType::Flags { flags, .. } => {
            format!("flags {{ {} }}", flags.join(", "))
        }
        SchemaType::Tuple { elements, .. } => {
            let inner = elements
                .iter()
                .map(|e| type_to_text_inner(graph, e, visited))
                .collect::<Vec<_>>()
                .join(", ");
            format!("tuple<{inner}>")
        }
        SchemaType::List { element, .. } => {
            format!("list<{}>", type_to_text_inner(graph, element, visited))
        }
        SchemaType::FixedList {
            element, length, ..
        } => format!(
            "fixed-list<{}, {}>",
            type_to_text_inner(graph, element, visited),
            length
        ),
        SchemaType::Map { key, value, .. } => format!(
            "map<{}, {}>",
            type_to_text_inner(graph, key, visited),
            type_to_text_inner(graph, value, visited)
        ),
        SchemaType::Option { inner, .. } => {
            format!("option<{}>", type_to_text_inner(graph, inner, visited))
        }
        SchemaType::Result {
            spec: ResultSpec { ok, err },
            ..
        } => {
            let ok_s = ok
                .as_deref()
                .map(|t| type_to_text_inner(graph, t, visited))
                .unwrap_or_else(|| "_".to_string());
            let err_s = err
                .as_deref()
                .map(|t| type_to_text_inner(graph, t, visited))
                .unwrap_or_else(|| "_".to_string());
            format!("result<{ok_s}, {err_s}>")
        }
        SchemaType::Text { restrictions: r, .. } => {
            let mut parts = Vec::new();
            if let Some(min) = r.min_length {
                parts.push(format!("min={min}"));
            }
            if let Some(max) = r.max_length {
                parts.push(format!("max={max}"));
            }
            if let Some(re) = &r.regex {
                parts.push(format!("regex={re:?}"));
            }
            if let Some(langs) = &r.languages {
                parts.push(format!("languages=[{}]", langs.join(", ")));
            }
            if parts.is_empty() {
                "text".to_string()
            } else {
                format!("text({})", parts.join(", "))
            }
        }
        SchemaType::Binary { restrictions: r, .. } => {
            let mut parts = Vec::new();
            if let Some(min) = r.min_bytes {
                parts.push(format!("min={min}"));
            }
            if let Some(max) = r.max_bytes {
                parts.push(format!("max={max}"));
            }
            if let Some(mimes) = &r.mime_types {
                parts.push(format!("mime_types=[{}]", mimes.join(", ")));
            }
            if parts.is_empty() {
                "binary".to_string()
            } else {
                format!("binary({})", parts.join(", "))
            }
        }
        SchemaType::Path { spec, .. } => {
            let direction = match spec.direction {
                crate::schema::schema_type::PathDirection::Input => "in",
                crate::schema::schema_type::PathDirection::Output => "out",
                crate::schema::schema_type::PathDirection::InOut => "in-out",
            };
            let kind = match spec.kind {
                crate::schema::schema_type::PathKind::File => "file",
                crate::schema::schema_type::PathKind::Directory => "directory",
                crate::schema::schema_type::PathKind::Any => "any",
            };
            let mut parts = vec![direction.to_string(), kind.to_string()];
            if let Some(exts) = &spec.allowed_extensions {
                parts.push(format!("extensions=[{}]", exts.join(", ")));
            }
            if let Some(mimes) = &spec.allowed_mime_types {
                parts.push(format!("mime_types=[{}]", mimes.join(", ")));
            }
            format!("path({})", parts.join(", "))
        }
        SchemaType::Url { restrictions: spec, .. } => {
            let mut parts = Vec::new();
            if let Some(schemes) = &spec.allowed_schemes {
                parts.push(format!("schemes=[{}]", schemes.join(", ")));
            }
            if let Some(hosts) = &spec.allowed_hosts {
                parts.push(format!("hosts=[{}]", hosts.join(", ")));
            }
            if parts.is_empty() {
                "url".to_string()
            } else {
                format!("url({})", parts.join(", "))
            }
        }
        SchemaType::Datetime { .. } => "datetime".to_string(),
        SchemaType::Duration { .. } => "duration".to_string(),
        SchemaType::Quantity { spec, .. } => {
            let mut parts = vec![spec.base_unit.clone()];
            if !spec.allowed_suffixes.is_empty() {
                parts.push(format!("suffixes=[{}]", spec.allowed_suffixes.join(", ")));
            }
            if let Some(min) = &spec.min {
                parts.push(format!("min={}e-{} {}", min.mantissa, min.scale, min.unit));
            }
            if let Some(max) = &spec.max {
                parts.push(format!("max={}e-{} {}", max.mantissa, max.scale, max.unit));
            }
            format!("quantity({})", parts.join(", "))
        }
        SchemaType::Union { spec, .. } => {
            let inner = spec
                .branches
                .iter()
                .map(|b: &UnionBranch| {
                    format!("{} <- {}", b.tag, discriminator_text(&b.discriminator))
                })
                .collect::<Vec<_>>()
                .join(" | ");
            format!("union {{ {inner} }}")
        }
        SchemaType::Secret { .. } => "secret".to_string(),
        SchemaType::QuotaToken { .. } => "quota-token".to_string(),
        SchemaType::Future { inner, .. } => match inner {
            None => "future".to_string(),
            Some(t) => format!("future<{}>", type_to_text_inner(graph, t, visited)),
        },
        SchemaType::Stream { inner, .. } => match inner {
            None => "stream".to_string(),
            Some(t) => format!("stream<{}>", type_to_text_inner(graph, t, visited)),
        },
    };
    decorate_with_metadata(base, ty)
}

/// Append per-node metadata annotations (currently `(deprecated)`) so the
/// rendered text reflects the inline metadata envelope of any
/// [`SchemaType`] node.
fn decorate_with_metadata(base: String, ty: &SchemaType) -> String {
    let meta = ty.metadata();
    if meta.deprecated.is_some() {
        format!("{base} (deprecated)")
    } else {
        base
    }
}

fn discriminator_text(rule: &DiscriminatorRule) -> String {
    match rule {
        DiscriminatorRule::Prefix { prefix } => format!("prefix({prefix:?})"),
        DiscriminatorRule::Suffix { suffix } => format!("suffix({suffix:?})"),
        DiscriminatorRule::Contains { substring } => format!("contains({substring:?})"),
        DiscriminatorRule::Regex { regex } => format!("regex({regex:?})"),
        DiscriminatorRule::FieldEquals(d) => match &d.literal {
            Some(lit) => format!("field({}={lit:?})", d.field_name),
            None => format!("field({})", d.field_name),
        },
        DiscriminatorRule::FieldAbsent { field_name } => format!("no-field({field_name})"),
    }
}

/// Render a [`SchemaValue`] using the schema for context.
///
/// Capability values (`Secret`, `QuotaToken`) are emitted as `<redacted>`.
/// Use [`value_to_cli_text_unredacted`] only when the caller needs the raw
/// canonical encoding (admin tooling, test fixtures).
pub fn value_to_cli_text(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<String, RenderError> {
    let mut renderer = CliTextRenderer { redact: true };
    drive(walk(&mut renderer, graph, ty, value))
}

/// Render a [`SchemaValue`] without redacting capability material.
///
/// **Warning:** emits the canonical text form of every value, including
/// `Secret` and `QuotaToken`. Intended for admin tooling and tests that
/// need to round-trip the raw payload; never use this on the user-facing
/// output path.
pub fn value_to_cli_text_unredacted(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<String, RenderError> {
    let mut renderer = CliTextRenderer { redact: false };
    drive(walk(&mut renderer, graph, ty, value))
}

struct CliTextRenderer {
    redact: bool,
}

impl SchemaWalker for CliTextRenderer {
    type Output = String;
    type Error = RenderError;

    fn walk(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> Result<String, RenderError> {
        render_value(self, graph, ty, value)
    }
}

fn drive<T>(
    res: Result<T, crate::schema::render::walker::WalkerError<RenderError>>,
) -> Result<T, RenderError> {
    use crate::schema::render::walker::WalkerError;
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

fn render_value(
    r: &mut CliTextRenderer,
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<String, RenderError> {
    match (ty, value) {
        (SchemaType::Bool { .. }, SchemaValue::Bool(b)) => Ok(b.to_string()),
        (SchemaType::S8 { .. }, SchemaValue::S8(i)) => Ok(i.to_string()),
        (SchemaType::S16 { .. }, SchemaValue::S16(i)) => Ok(i.to_string()),
        (SchemaType::S32 { .. }, SchemaValue::S32(i)) => Ok(i.to_string()),
        (SchemaType::S64 { .. }, SchemaValue::S64(i)) => Ok(i.to_string()),
        (SchemaType::U8 { .. }, SchemaValue::U8(i)) => Ok(i.to_string()),
        (SchemaType::U16 { .. }, SchemaValue::U16(i)) => Ok(i.to_string()),
        (SchemaType::U32 { .. }, SchemaValue::U32(i)) => Ok(i.to_string()),
        (SchemaType::U64 { .. }, SchemaValue::U64(i)) => Ok(i.to_string()),
        (SchemaType::F32 { .. }, SchemaValue::F32(f)) => Ok(f.to_string()),
        (SchemaType::F64 { .. }, SchemaValue::F64(f)) => Ok(f.to_string()),
        (SchemaType::Char { .. }, SchemaValue::Char(c)) => Ok(format!("'{c}'")),
        (SchemaType::String { .. }, SchemaValue::String(s)) => Ok(format!("{s:?}")),

        (SchemaType::Text { .. }, SchemaValue::Text(p)) => Ok(canonical::text::to_text(p)),
        (SchemaType::Binary { .. }, SchemaValue::Binary(p)) => Ok(canonical::binary::to_text(p)?),
        (SchemaType::Path { .. }, SchemaValue::Path { path }) => Ok(canonical::path::to_text(path)?),
        (SchemaType::Url { .. }, SchemaValue::Url { url }) => Ok(canonical::url::to_text(url)?),
        (SchemaType::Datetime { .. }, SchemaValue::Datetime { value }) => {
            Ok(canonical::datetime::to_text(value)?)
        }
        (SchemaType::Duration { .. }, SchemaValue::Duration(p)) => Ok(canonical::duration::to_text(p)),
        (SchemaType::Quantity { .. }, SchemaValue::Quantity(q)) => Ok(canonical::quantity::to_text(q)?),
        (SchemaType::Secret { .. }, SchemaValue::Secret(p)) => {
            if r.redact {
                Ok("<redacted>".to_string())
            } else {
                Ok(canonical::secret::to_text(p)?)
            }
        }
        (SchemaType::QuotaToken { .. }, SchemaValue::QuotaToken(p)) => {
            if r.redact {
                Ok("<redacted>".to_string())
            } else {
                Ok(canonical::quota_token::to_text(p)?)
            }
        }

        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs }) => {
            if fields.len() != vs.len() {
                return Err(RenderError::ValueMismatch {
                    path: String::new(),
                    reason: "record arity mismatch".to_string(),
                });
            }
            let parts: Result<Vec<String>, RenderError> = fields
                .iter()
                .zip(vs.iter())
                .map(|(f, v)| {
                    let rendered = drive(walk(r, graph, &f.body, v))?;
                    Ok(format!("{}: {}", f.name, rendered))
                })
                .collect();
            Ok(format!("{{ {} }}", parts?.join(", ")))
        }

        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(vp)) => {
            let case_index = vp.case as usize;
            if case_index >= cases.len() {
                return Err(RenderError::ValueMismatch {
                    path: String::new(),
                    reason: "variant case out of range".to_string(),
                });
            }
            let case = &cases[case_index];
            match (&case.payload, &vp.payload) {
                (None, None) => Ok(case.name.clone()),
                (Some(case_ty), Some(payload)) => {
                    let inner = drive(walk(r, graph, case_ty, payload))?;
                    Ok(format!("{}({inner})", case.name))
                }
                _ => Err(RenderError::ValueMismatch {
                    path: String::new(),
                    reason: "variant payload presence mismatch".to_string(),
                }),
            }
        }

        (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case }) => {
            let idx = *case as usize;
            if idx >= cases.len() {
                return Err(RenderError::ValueMismatch {
                    path: String::new(),
                    reason: "enum case out of range".to_string(),
                });
            }
            Ok(cases[idx].clone())
        }

        (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits }) => {
            if flags.len() != bits.len() {
                return Err(RenderError::ValueMismatch {
                    path: String::new(),
                    reason: "flags arity mismatch".to_string(),
                });
            }
            let selected: Vec<&str> = flags
                .iter()
                .zip(bits.iter())
                .filter(|(_, on)| **on)
                .map(|(name, _)| name.as_str())
                .collect();
            Ok(format!("{{{}}}", selected.join(", ")))
        }

        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: vs }) => {
            if elements.len() != vs.len() {
                return Err(RenderError::ValueMismatch {
                    path: String::new(),
                    reason: "tuple arity mismatch".to_string(),
                });
            }
            let parts: Result<Vec<String>, RenderError> = elements
                .iter()
                .zip(vs.iter())
                .map(|(t, v)| drive(walk(r, graph, t, v)))
                .collect();
            Ok(format!("({})", parts?.join(", ")))
        }

        (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
            let parts: Result<Vec<String>, RenderError> = elements
                .iter()
                .map(|v| drive(walk(r, graph, element, v)))
                .collect();
            Ok(format!("[{}]", parts?.join(", ")))
        }

        (
            SchemaType::FixedList {
                element, length, ..
            },
            SchemaValue::FixedList { elements },
        ) => {
            if elements.len() as u32 != *length {
                return Err(RenderError::ValueMismatch {
                    path: String::new(),
                    reason: "fixed list length mismatch".to_string(),
                });
            }
            let parts: Result<Vec<String>, RenderError> = elements
                .iter()
                .map(|v| drive(walk(r, graph, element, v)))
                .collect();
            Ok(format!("[{}]", parts?.join(", ")))
        }

        (SchemaType::Map { key, value, .. }, SchemaValue::Map { entries }) => {
            let parts: Result<Vec<String>, RenderError> = entries
                .iter()
                .map(|(k, v)| {
                    let rk = drive(walk(r, graph, key, k))?;
                    let rv = drive(walk(r, graph, value, v))?;
                    Ok(format!("{rk} => {rv}"))
                })
                .collect();
            Ok(format!("{{{}}}", parts?.join(", ")))
        }

        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: v }) => match v {
            None => Ok("none".to_string()),
            Some(v) => {
                let inner_s = drive(walk(r, graph, inner, v))?;
                Ok(format!("some({inner_s})"))
            }
        },

        (SchemaType::Result { spec, .. }, SchemaValue::Result(payload)) => match payload {
            ResultValuePayload::Ok { value: v } => {
                let inner = match (spec.ok.as_deref(), v.as_deref()) {
                    (None, None) => String::new(),
                    (Some(ok_ty), Some(inner)) => drive(walk(r, graph, ok_ty, inner))?,
                    _ => {
                        return Err(RenderError::ValueMismatch {
                            path: String::new(),
                            reason: "result ok payload presence mismatch".to_string(),
                        });
                    }
                };
                if inner.is_empty() {
                    Ok("ok".to_string())
                } else {
                    Ok(format!("ok({inner})"))
                }
            }
            ResultValuePayload::Err { value: v } => {
                let inner = match (spec.err.as_deref(), v.as_deref()) {
                    (None, None) => String::new(),
                    (Some(err_ty), Some(inner)) => drive(walk(r, graph, err_ty, inner))?,
                    _ => {
                        return Err(RenderError::ValueMismatch {
                            path: String::new(),
                            reason: "result err payload presence mismatch".to_string(),
                        });
                    }
                };
                if inner.is_empty() {
                    Ok("err".to_string())
                } else {
                    Ok(format!("err({inner})"))
                }
            }
        },

        (SchemaType::Union { spec, .. }, SchemaValue::Union(payload)) => {
            render_union(r, graph, spec, payload)
        }

        (SchemaType::Future { .. }, _) | (SchemaType::Stream { .. }, _) => Err(
            RenderError::Unsupported("future/stream values have no CLI text"),
        ),

        _ => Err(RenderError::ValueMismatch {
            path: String::new(),
            reason: "shape mismatch".to_string(),
        }),
    }
}

fn render_union(
    r: &mut CliTextRenderer,
    graph: &SchemaGraph,
    spec: &crate::schema::schema_type::UnionSpec,
    payload: &UnionValuePayload,
) -> Result<String, RenderError> {
    let branch = spec
        .branches
        .iter()
        .find(|b| b.tag == payload.tag)
        .ok_or_else(|| RenderError::ValueMismatch {
            path: String::new(),
            reason: format!("unknown union branch `{}`", payload.tag),
        })?;
    let inner = drive(walk(r, graph, &branch.body, &payload.body))?;
    Ok(format!("{}({inner})", payload.tag))
}
