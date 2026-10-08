// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

//! Go syntax for values and types.
//!
//! Values read as Go composite literals with their types left out, the way Go
//! writes the elements of a slice or map literal: `{orderId: "o1", lines: {1,
//! 2}}`. Parsing is type-driven, so the schema supplies every type and the text
//! stays a compact id. Names — fields, flags, enum and variant cases — are
//! written exactly as the schema has them, never converted; one the lexer
//! cannot read as an identifier is quoted.

use super::{
    recursive_ref_display_name, render_rich_constructor, render_rich_constructor2,
    resolve_named_ref,
};
use golem_common::model::agent::text_utils::write_json_escaped;
use golem_common::schema::canonical;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::host_managed::HostManagedKind;
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, UnionValuePayload};
use std::fmt::Write;

pub(super) fn render_value_go(graph: &SchemaGraph, ty: &SchemaType, value: &SchemaValue) -> String {
    let mut buf = String::new();
    render_cm_value(&mut buf, graph, ty, value);
    buf
}

/// Words the lexer reads as something other than an identifier, or that a
/// Go value uses in a position a name could take. A name equal to one of them
/// is quoted so it cannot be mistaken for the keyword.
const RESERVED: &[&str] = &[
    "true",
    "false",
    "null",
    "undefined",
    "NaN",
    "Infinity",
    "nil",
    "Some",
    "None",
    "Ok",
    "Err",
    "golem",
];

/// True when `name` can be written bare: an ASCII identifier the shared lexer
/// reads as one, and not a reserved word.
pub(super) fn is_bare_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts_ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    starts_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') && !RESERVED.contains(&name)
}

/// Writes a schema name bare when it can be, quoted otherwise.
fn write_name(buf: &mut String, name: &str) {
    if is_bare_name(name) {
        buf.push_str(name);
    } else {
        buf.push('"');
        write_json_escaped(buf, name);
        buf.push('"');
    }
}

fn render_cm_value(buf: &mut String, graph: &SchemaGraph, ty: &SchemaType, value: &SchemaValue) {
    let (resolved_ty, _) = resolve_named_ref(graph, ty);
    render_cm_value_inner(buf, graph, resolved_ty, value);
}

fn render_cm_value_inner(
    buf: &mut String,
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) {
    if let Some(kind) = HostManagedKind::from_value(value) {
        buf.push_str(kind.redacted_placeholder());
        return;
    }

    match (ty, value) {
        (SchemaType::Bool { .. }, SchemaValue::Bool(b)) => {
            let _ = write!(buf, "{b}");
        }
        (SchemaType::U8 { .. }, SchemaValue::U8(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::U16 { .. }, SchemaValue::U16(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::U32 { .. }, SchemaValue::U32(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::U64 { .. }, SchemaValue::U64(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::S8 { .. }, SchemaValue::S8(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::S16 { .. }, SchemaValue::S16(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::S32 { .. }, SchemaValue::S32(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::S64 { .. }, SchemaValue::S64(v)) => {
            let _ = write!(buf, "{v}");
        }
        (SchemaType::F32 { .. }, SchemaValue::F32(v)) => render_f64(buf, *v as f64),
        (SchemaType::F64 { .. }, SchemaValue::F64(v)) => render_f64(buf, *v),
        (SchemaType::Char { .. }, SchemaValue::Char(c)) => render_rune(buf, *c),
        (SchemaType::String { .. }, SchemaValue::String(s)) => {
            buf.push('"');
            write_json_escaped(buf, s);
            buf.push('"');
        }
        (SchemaType::List { element, .. }, SchemaValue::List { elements })
        | (SchemaType::FixedList { element, .. }, SchemaValue::FixedList { elements }) => {
            buf.push('{');
            for (i, item) in elements.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, graph, element, item);
            }
            buf.push('}');
        }
        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: vs }) => {
            buf.push('{');
            for (i, (t, v)) in elements.iter().zip(vs.iter()).enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, graph, t, v);
            }
            buf.push('}');
        }
        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs }) => {
            render_record(buf, graph, fields, vs);
        }
        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(p)) => {
            let case = &cases[p.case as usize];
            write_name(buf, &case.name);
            if let (Some(v), Some(t)) = (&p.payload, &case.payload) {
                let (resolved, _) = resolve_named_ref(graph, t);
                match (resolved, v.as_ref()) {
                    // A case with a record payload is written as Go writes the
                    // case's struct: the name followed by its fields.
                    (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs }) => {
                        render_record(buf, graph, fields, vs);
                    }
                    _ => {
                        buf.push('(');
                        render_cm_value(buf, graph, t, v);
                        buf.push(')');
                    }
                }
            }
        }
        (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case }) => {
            write_name(buf, &cases[*case as usize]);
        }
        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: v }) => match v {
            Some(payload) => {
                // A present value is written bare, as a pointer option is. Only
                // when the inner type is itself an option is it wrapped, so the
                // two levels stay apart.
                let (inner_resolved, _) = resolve_named_ref(graph, inner);
                if matches!(inner_resolved, SchemaType::Option { .. }) {
                    buf.push_str("Some(");
                    render_cm_value(buf, graph, inner, payload);
                    buf.push(')');
                } else {
                    render_cm_value(buf, graph, inner, payload);
                }
            }
            None => buf.push_str("nil"),
        },
        (SchemaType::Result { spec, .. }, SchemaValue::Result(p)) => {
            render_result(buf, graph, spec, p);
        }
        (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits }) => {
            buf.push('{');
            let mut first = true;
            for (set, name) in bits.iter().zip(flags.iter()) {
                if *set {
                    if !first {
                        buf.push_str(", ");
                    }
                    write_name(buf, name);
                    buf.push_str(": true");
                    first = false;
                }
            }
            buf.push('}');
        }
        (SchemaType::Text { .. }, SchemaValue::Text(p)) => {
            render_rich_constructor2(buf, "Text", &p.text, p.language.as_deref());
        }
        (SchemaType::Binary { .. }, SchemaValue::Binary(p)) => {
            let s = canonical::binary::to_text(p).unwrap_or_else(|_| "<binary>".to_string());
            render_rich_constructor(buf, "Binary", &s);
        }
        (SchemaType::Path { .. }, SchemaValue::Path { path }) => {
            let s = canonical::path::to_text(path).unwrap_or_else(|_| path.clone());
            render_rich_constructor(buf, "Path", &s);
        }
        (SchemaType::Url { .. }, SchemaValue::Url { url }) => {
            let s = canonical::url::to_text(url).unwrap_or_else(|_| url.clone());
            render_rich_constructor(buf, "Url", &s);
        }
        (SchemaType::Uuid { .. }, SchemaValue::Uuid(uuid)) => {
            render_rich_constructor(buf, "Uuid", &uuid.to_string());
        }
        (SchemaType::Datetime { .. }, SchemaValue::Datetime { value }) => {
            let s = canonical::datetime::to_text(value).unwrap_or_else(|_| value.to_string());
            render_rich_constructor(buf, "Datetime", &s);
        }
        (SchemaType::Duration { .. }, SchemaValue::Duration(p)) => {
            render_rich_constructor(buf, "Duration", &canonical::duration::to_text(p));
        }
        (SchemaType::Quantity { .. }, SchemaValue::Quantity(q)) => {
            let s = canonical::quantity::to_text(q).unwrap_or_else(|_| "<quantity>".to_string());
            render_rich_constructor(buf, "Quantity", &s);
        }
        (SchemaType::Union { spec, .. }, SchemaValue::Union(UnionValuePayload { tag, body })) => {
            if let Some(branch) = spec.branches.iter().find(|b| &b.tag == tag) {
                write_name(buf, tag);
                buf.push('(');
                render_cm_value(buf, graph, &branch.body, body);
                buf.push(')');
            } else {
                buf.push_str("<unknown-union-branch>");
            }
        }
        (SchemaType::Map { key, value, .. }, SchemaValue::Map { entries }) => {
            if entries.is_empty() {
                buf.push_str("{}");
                return;
            }
            buf.push_str("{ ");
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, graph, key, k);
                buf.push_str(" => ");
                render_cm_value(buf, graph, value, v);
            }
            buf.push_str(" }");
        }
        _ => buf.push_str("<unknown>"),
    }
}

fn render_record(
    buf: &mut String,
    graph: &SchemaGraph,
    fields: &[NamedFieldType],
    values: &[SchemaValue],
) {
    buf.push('{');
    for (i, (field, val)) in fields.iter().zip(values.iter()).enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        write_name(buf, &field.name);
        buf.push_str(": ");
        render_cm_value(buf, graph, &field.body, val);
    }
    buf.push('}');
}

fn render_result(
    buf: &mut String,
    graph: &SchemaGraph,
    spec: &ResultSpec,
    payload: &ResultValuePayload,
) {
    let (name, value, ty) = match payload {
        ResultValuePayload::Ok { value } => ("Ok", value, &spec.ok),
        ResultValuePayload::Err { value } => ("Err", value, &spec.err),
    };
    buf.push_str(name);
    buf.push('(');
    if let (Some(v), Some(t)) = (value, ty) {
        render_cm_value(buf, graph, t, v);
    }
    buf.push(')');
}

pub fn render_type_go(graph: &SchemaGraph, ty: &SchemaType, prefer_name: bool) -> String {
    if let Some(name) = recursive_ref_display_name(graph, ty) {
        return name.to_string();
    }
    let (resolved, def_name) = resolve_named_ref(graph, ty);
    render_type_go_inner(graph, resolved, def_name, prefer_name)
}

/// A type in a position where an alternation (`a | b`) would be ambiguous,
/// such as the element of a slice: parenthesised when it is one.
fn render_type_nested(graph: &SchemaGraph, ty: &SchemaType, prefer_name: bool) -> String {
    let rendered = render_type_go(graph, ty, prefer_name);
    let (resolved, def_name) = resolve_named_ref(graph, ty);
    let named = prefer_name && def_name.is_some();
    if !named && matches!(resolved, SchemaType::Variant { .. }) && rendered.contains(" | ") {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn render_type_go_inner(
    graph: &SchemaGraph,
    ty: &SchemaType,
    def_name: Option<&str>,
    prefer_name: bool,
) -> String {
    if prefer_name
        && let Some(name) = def_name
        && matches!(
            ty,
            SchemaType::Record { .. }
                | SchemaType::Variant { .. }
                | SchemaType::Enum { .. }
                | SchemaType::Flags { .. }
        )
    {
        return name.to_string();
    }
    match ty {
        SchemaType::String { .. } => "string".to_string(),
        SchemaType::Char { .. } => "rune".to_string(),
        SchemaType::Bool { .. } => "bool".to_string(),
        SchemaType::U8 { .. } => "uint8".to_string(),
        SchemaType::U16 { .. } => "uint16".to_string(),
        SchemaType::U32 { .. } => "uint32".to_string(),
        SchemaType::U64 { .. } => "uint64".to_string(),
        SchemaType::S8 { .. } => "int8".to_string(),
        SchemaType::S16 { .. } => "int16".to_string(),
        SchemaType::S32 { .. } => "int32".to_string(),
        SchemaType::S64 { .. } => "int64".to_string(),
        SchemaType::F32 { .. } => "float32".to_string(),
        SchemaType::F64 { .. } => "float64".to_string(),
        SchemaType::Option { inner, .. } => {
            format!("*{}", render_type_nested(graph, inner, prefer_name))
        }
        SchemaType::List { element, .. } => {
            format!("[]{}", render_type_nested(graph, element, prefer_name))
        }
        SchemaType::FixedList {
            element, length, ..
        } => format!(
            "[{length}]{}",
            render_type_nested(graph, element, prefer_name)
        ),
        SchemaType::Map { key, value, .. } => format!(
            "map[{}]{}",
            render_type_go(graph, key, prefer_name),
            render_type_nested(graph, value, prefer_name)
        ),
        SchemaType::Result { spec, .. } => {
            let side = |t: &Option<Box<SchemaType>>| {
                t.as_deref()
                    .map(|t| render_type_go(graph, t, prefer_name))
                    .unwrap_or_else(|| "Unit".to_string())
            };
            format!("Result[{}, {}]", side(&spec.ok), side(&spec.err))
        }
        SchemaType::Tuple { elements, .. } => {
            let items = elements
                .iter()
                .map(|t| render_type_go(graph, t, prefer_name))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Tuple{}[{items}]", elements.len())
        }
        SchemaType::Record { fields, .. } => {
            let mut buf = String::from("struct{");
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    buf.push_str("; ");
                }
                write_name(&mut buf, &field.name);
                let _ = write!(buf, " {}", render_type_go(graph, &field.body, prefer_name));
            }
            buf.push('}');
            buf
        }
        SchemaType::Variant { cases, .. } => render_type_variant_go(graph, cases, prefer_name),
        SchemaType::Enum { cases, .. } => {
            let mut buf = String::from("enum{");
            for (i, case) in cases.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_name(&mut buf, case);
            }
            buf.push('}');
            buf
        }
        SchemaType::Flags { flags, .. } => {
            let mut buf = String::from("struct{");
            for (i, flag) in flags.iter().enumerate() {
                if i > 0 {
                    buf.push_str("; ");
                }
                write_name(&mut buf, flag);
                buf.push_str(" bool");
            }
            buf.push('}');
            buf
        }
        SchemaType::Text { .. } => "Text".to_string(),
        SchemaType::Binary { .. } => "Binary".to_string(),
        SchemaType::Path { .. } => "Path".to_string(),
        SchemaType::Url { .. } => "URL".to_string(),
        SchemaType::Uuid { .. } => "UUID".to_string(),
        SchemaType::Datetime { .. } => "time.Time".to_string(),
        SchemaType::Duration { .. } => "time.Duration".to_string(),
        SchemaType::Quantity { .. } => "Quantity".to_string(),
        SchemaType::Secret { spec, .. } => {
            format!(
                "Secret[{}]",
                render_type_go(graph, &spec.inner, prefer_name)
            )
        }
        SchemaType::QuotaToken { .. } => "QuotaToken".to_string(),
        SchemaType::PermissionCard { .. } => "PermissionCard".to_string(),
        SchemaType::Union { spec, .. } => {
            let inner = spec
                .branches
                .iter()
                .map(|b| format!("{}({})", b.tag, render_type_go(graph, &b.body, prefer_name)))
                .collect::<Vec<_>>()
                .join(" | ");
            format!("union {{ {inner} }}")
        }
        SchemaType::Future { inner, .. } => match inner {
            None => "future".to_string(),
            Some(t) => format!("future<{}>", render_type_go(graph, t, prefer_name)),
        },
        SchemaType::Stream { inner, .. } => match inner {
            None => "AgentStream".to_string(),
            Some(t) => format!("AgentStream[{}]", render_type_go(graph, t, prefer_name)),
        },
        SchemaType::Ref { id, .. } => id.0.clone(),
    }
}

fn render_type_variant_go(
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
    prefer_name: bool,
) -> String {
    let mut buf = String::new();
    for (i, case) in cases.iter().enumerate() {
        if i > 0 {
            buf.push_str(" | ");
        }
        write_name(&mut buf, &case.name);
        if let Some(t) = &case.payload {
            let _ = write!(buf, "({})", render_type_go(graph, t, prefer_name));
        }
    }
    buf
}

fn render_f64(buf: &mut String, v: f64) {
    if v.is_nan() {
        buf.push_str("NaN");
    } else if v.is_infinite() {
        if v.is_sign_negative() {
            buf.push_str("-Infinity");
        } else {
            buf.push_str("Infinity");
        }
    } else if v == 0.0 && v.is_sign_negative() {
        buf.push_str("-0.0");
    } else {
        let s = format!("{v}");
        buf.push_str(&s);
        if !(s.contains('.') || s.contains('e') || s.contains('E')) {
            buf.push_str(".0");
        }
    }
}

/// A Go rune literal. Control characters use the four-digit `\uXXXX` escape,
/// which is valid in Go (unlike Rust's braced `\u{…}`).
fn render_rune(buf: &mut String, c: char) {
    buf.push('\'');
    match c {
        '\'' => buf.push_str("\\'"),
        '\\' => buf.push_str("\\\\"),
        '\n' => buf.push_str("\\n"),
        '\r' => buf.push_str("\\r"),
        '\t' => buf.push_str("\\t"),
        c if c.is_control() => {
            let _ = write!(buf, "\\u{:04x}", c as u32);
        }
        c => buf.push(c),
    }
    buf.push('\'');
}
