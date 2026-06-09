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

use super::{render_rich_constructor, render_rich_constructor2, resolve_named_ref};
use golem_common::model::agent::text_utils::write_json_escaped;
use golem_common::schema::canonical;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, UnionValuePayload};
use heck::{ToSnakeCase, ToUpperCamelCase};
use std::fmt::Write;

pub(super) fn render_value_rust(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> String {
    let mut buf = String::new();
    render_cm_value(&mut buf, graph, ty, value);
    buf
}

fn render_cm_value(buf: &mut String, graph: &SchemaGraph, ty: &SchemaType, value: &SchemaValue) {
    // Resolve refs first to recover the def-name and body together.
    let (resolved_ty, def_name) = resolve_named_ref(graph, ty);
    render_cm_value_inner(buf, graph, resolved_ty, def_name, value);
}

fn render_cm_value_inner(
    buf: &mut String,
    graph: &SchemaGraph,
    ty: &SchemaType,
    def_name: Option<&str>,
    value: &SchemaValue,
) {
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
        (SchemaType::Char { .. }, SchemaValue::Char(c)) => render_char(buf, *c),
        (SchemaType::String { .. }, SchemaValue::String(s)) => {
            buf.push('"');
            write_json_escaped(buf, s);
            buf.push('"');
        }
        (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
            buf.push('[');
            for (i, item) in elements.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, graph, element, item);
            }
            buf.push(']');
        }
        (SchemaType::FixedList { element, .. }, SchemaValue::FixedList { elements }) => {
            buf.push('[');
            for (i, item) in elements.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, graph, element, item);
            }
            buf.push(']');
        }
        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: vs }) => {
            buf.push('(');
            for (i, (t, v)) in elements.iter().zip(vs.iter()).enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, graph, t, v);
            }
            buf.push(')');
        }
        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs }) => {
            if let Some(name) = def_name {
                let _ = write!(buf, "{} ", name.to_upper_camel_case());
            }
            buf.push_str("{ ");
            for (i, (field, val)) in fields.iter().zip(vs.iter()).enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let _ = write!(buf, "{}: ", field.name.to_snake_case());
                render_cm_value(buf, graph, &field.body, val);
            }
            buf.push_str(" }");
        }
        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(p)) => {
            render_variant(buf, graph, def_name, cases, p);
        }
        (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case }) => {
            let case_name = cases[*case as usize].to_upper_camel_case();
            if let Some(name) = def_name {
                let _ = write!(buf, "{}::{case_name}", name.to_upper_camel_case());
            } else {
                buf.push_str(&case_name);
            }
        }
        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: v }) => match v {
            Some(payload) => {
                buf.push_str("Some(");
                render_cm_value(buf, graph, inner, payload);
                buf.push(')');
            }
            None => buf.push_str("None"),
        },
        (SchemaType::Result { spec, .. }, SchemaValue::Result(p)) => {
            render_result(buf, graph, spec, p);
        }
        (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits }) => {
            if let Some(name) = def_name {
                let _ = write!(buf, "{} ", name.to_upper_camel_case());
            }
            buf.push_str("{ ");
            let mut first = true;
            for (set, name) in bits.iter().zip(flags.iter()) {
                if *set {
                    if !first {
                        buf.push_str(", ");
                    }
                    buf.push_str(&name.to_snake_case());
                    first = false;
                }
            }
            buf.push_str(" }");
        }
        // Rich semantic types render as constructor calls — `Name("body")`
        // — using JSON-quoted bodies. This keeps the form close to what
        // an SDK would write in source (e.g. `Path::new("/tmp/a")` /
        // `Url::parse("https://...")`) and lets the parser round-trip
        // through the shared lexer without per-type tokenisation.
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
        (SchemaType::Secret { .. }, SchemaValue::Secret(_))
        | (SchemaType::QuotaToken { .. }, SchemaValue::QuotaToken(_)) => {
            buf.push_str("<redacted>");
        }
        (SchemaType::Union { spec, .. }, SchemaValue::Union(UnionValuePayload { tag, body })) => {
            if let Some(branch) = spec.branches.iter().find(|b| &b.tag == tag) {
                let _ = write!(buf, "{}(", tag);
                render_cm_value(buf, graph, &branch.body, body);
                buf.push(')');
            } else {
                buf.push_str("<unknown-union-branch>");
            }
        }
        (SchemaType::Map { key, value, .. }, SchemaValue::Map { entries }) => {
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

fn render_variant(
    buf: &mut String,
    graph: &SchemaGraph,
    def_name: Option<&str>,
    cases: &[VariantCaseType],
    payload: &golem_common::schema::schema_value::VariantValuePayload,
) {
    let case = &cases[payload.case as usize];
    let case_name = case.name.to_upper_camel_case();
    if let Some(name) = def_name {
        let _ = write!(buf, "{}::", name.to_upper_camel_case());
    }
    match (&payload.payload, &case.payload) {
        (Some(v), Some(t)) => {
            let _ = write!(buf, "{case_name}(");
            render_cm_value(buf, graph, t, v);
            buf.push(')');
        }
        _ => buf.push_str(&case_name),
    }
}

fn render_result(
    buf: &mut String,
    graph: &SchemaGraph,
    spec: &ResultSpec,
    payload: &ResultValuePayload,
) {
    match payload {
        ResultValuePayload::Ok { value } => match value {
            Some(v) => {
                buf.push_str("Ok(");
                if let Some(ok_ty) = &spec.ok {
                    render_cm_value(buf, graph, ok_ty, v);
                }
                buf.push(')');
            }
            None => buf.push_str("Ok(())"),
        },
        ResultValuePayload::Err { value } => match value {
            Some(v) => {
                buf.push_str("Err(");
                if let Some(err_ty) = &spec.err {
                    render_cm_value(buf, graph, err_ty, v);
                }
                buf.push(')');
            }
            None => buf.push_str("Err(())"),
        },
    }
}

pub fn render_type_rust(graph: &SchemaGraph, ty: &SchemaType, prefer_name: bool) -> String {
    let (resolved, def_name) = resolve_named_ref(graph, ty);
    render_type_rust_inner(graph, resolved, def_name, prefer_name)
}

fn render_type_rust_inner(
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
        return name.to_upper_camel_case();
    }
    match ty {
        SchemaType::String { .. } => "String".to_string(),
        SchemaType::Char { .. } => "char".to_string(),
        SchemaType::Bool { .. } => "bool".to_string(),
        SchemaType::U8 { .. } => "u8".to_string(),
        SchemaType::U16 { .. } => "u16".to_string(),
        SchemaType::U32 { .. } => "u32".to_string(),
        SchemaType::U64 { .. } => "u64".to_string(),
        SchemaType::S8 { .. } => "i8".to_string(),
        SchemaType::S16 { .. } => "i16".to_string(),
        SchemaType::S32 { .. } => "i32".to_string(),
        SchemaType::S64 { .. } => "i64".to_string(),
        SchemaType::F32 { .. } => "f32".to_string(),
        SchemaType::F64 { .. } => "f64".to_string(),
        SchemaType::Option { inner, .. } => {
            format!("Option<{}>", render_type_rust(graph, inner, prefer_name))
        }
        SchemaType::List { element, .. } => {
            format!("Vec<{}>", render_type_rust(graph, element, prefer_name))
        }
        SchemaType::FixedList {
            element, length, ..
        } => format!(
            "fixed-list<{}, {}>",
            render_type_rust(graph, element, prefer_name),
            length
        ),
        SchemaType::Map { key, value, .. } => format!(
            "map<{}, {}>",
            render_type_rust(graph, key, prefer_name),
            render_type_rust(graph, value, prefer_name)
        ),
        SchemaType::Result { spec, .. } => {
            let ok = spec
                .ok
                .as_deref()
                .map(|t| render_type_rust(graph, t, prefer_name))
                .unwrap_or_else(|| "()".to_string());
            let err = spec
                .err
                .as_deref()
                .map(|t| render_type_rust(graph, t, prefer_name))
                .unwrap_or_else(|| "()".to_string());
            format!("Result<{ok}, {err}>")
        }
        SchemaType::Tuple { elements, .. } => render_type_tuple_rust(graph, elements, prefer_name),
        SchemaType::Record { fields, .. } => render_type_record_rust(graph, fields, prefer_name),
        SchemaType::Variant { cases, .. } => render_type_variant_rust(graph, cases, prefer_name),
        SchemaType::Enum { cases, .. } => render_type_enum_rust(cases),
        SchemaType::Flags { flags, .. } => render_type_flags_rust(flags),
        SchemaType::Text { .. } => "text".to_string(),
        SchemaType::Binary { .. } => "binary".to_string(),
        SchemaType::Path { .. } => "path".to_string(),
        SchemaType::Url { .. } => "url".to_string(),
        SchemaType::Datetime { .. } => "datetime".to_string(),
        SchemaType::Duration { .. } => "duration".to_string(),
        SchemaType::Quantity { .. } => "quantity".to_string(),
        SchemaType::Secret { .. } => "secret".to_string(),
        SchemaType::QuotaToken { .. } => "quota-token".to_string(),
        SchemaType::Union { spec, .. } => {
            let inner = spec
                .branches
                .iter()
                .map(|b| {
                    format!(
                        "{}({})",
                        b.tag,
                        render_type_rust(graph, &b.body, prefer_name)
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            format!("union {{ {inner} }}")
        }
        SchemaType::Future { inner, .. } => match inner {
            None => "future".to_string(),
            Some(t) => format!("future<{}>", render_type_rust(graph, t, prefer_name)),
        },
        SchemaType::Stream { inner, .. } => match inner {
            None => "stream".to_string(),
            Some(t) => format!("stream<{}>", render_type_rust(graph, t, prefer_name)),
        },
        SchemaType::Ref { id, .. } => id.0.clone(),
    }
}

fn render_type_tuple_rust(
    graph: &SchemaGraph,
    elements: &[SchemaType],
    prefer_name: bool,
) -> String {
    let mut buf = String::from("(");
    for (i, item) in elements.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&render_type_rust(graph, item, prefer_name));
    }
    if elements.len() == 1 {
        buf.push(',');
    }
    buf.push(')');
    buf
}

fn render_type_record_rust(
    graph: &SchemaGraph,
    fields: &[NamedFieldType],
    prefer_name: bool,
) -> String {
    let mut buf = String::from("{ ");
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        let _ = write!(
            buf,
            "{}: {}",
            field.name.to_snake_case(),
            render_type_rust(graph, &field.body, prefer_name)
        );
    }
    buf.push_str(" }");
    buf
}

fn render_type_variant_rust(
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
    prefer_name: bool,
) -> String {
    let mut buf = String::from("enum { ");
    for (i, case) in cases.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&case.name.to_upper_camel_case());
        if let Some(t) = &case.payload {
            let _ = write!(buf, "({})", render_type_rust(graph, t, prefer_name));
        }
    }
    buf.push_str(" }");
    buf
}

fn render_type_enum_rust(cases: &[String]) -> String {
    let mut buf = String::from("enum { ");
    for (i, case) in cases.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&case.to_upper_camel_case());
    }
    buf.push_str(" }");
    buf
}

fn render_type_flags_rust(flags: &[String]) -> String {
    let mut buf = String::from("flags { ");
    for (i, flag) in flags.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&flag.to_snake_case());
    }
    buf.push_str(" }");
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
        if s.contains('.') || s.contains('e') || s.contains('E') {
            buf.push_str(&s);
        } else {
            buf.push_str(&s);
            buf.push_str(".0");
        }
    }
}

fn render_char(buf: &mut String, c: char) {
    buf.push('\'');
    match c {
        '\'' => buf.push_str("\\'"),
        '\\' => buf.push_str("\\\\"),
        '\n' => buf.push_str("\\n"),
        '\r' => buf.push_str("\\r"),
        '\t' => buf.push_str("\\t"),
        c if c.is_control() => {
            let _ = write!(buf, "\\u{{{:04X}}}", c as u32);
        }
        c => buf.push(c),
    }
    buf.push('\'');
}
