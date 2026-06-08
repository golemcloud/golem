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
use golem_common::model::agent::text_utils::{write_json_escaped, write_json_escaped_char};
use golem_common::schema::canonical;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, UnionValuePayload};
use heck::{ToLowerCamelCase, ToUpperCamelCase};
use std::fmt::Write;

pub(super) fn render_value_ts(graph: &SchemaGraph, ty: &SchemaType, value: &SchemaValue) -> String {
    let mut buf = String::new();
    render_cm_value(&mut buf, graph, ty, value);
    buf
}

fn render_cm_value(buf: &mut String, graph: &SchemaGraph, ty: &SchemaType, value: &SchemaValue) {
    let (resolved, _) = resolve_named_ref(graph, ty);
    match (resolved, value) {
        (SchemaType::Bool { .. }, SchemaValue::Bool(b)) => {
            buf.push_str(if *b { "true" } else { "false" });
        }
        (SchemaType::U8 { .. }, SchemaValue::U8(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::U16 { .. }, SchemaValue::U16(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::U32 { .. }, SchemaValue::U32(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::U64 { .. }, SchemaValue::U64(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::S8 { .. }, SchemaValue::S8(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::S16 { .. }, SchemaValue::S16(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::S32 { .. }, SchemaValue::S32(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::S64 { .. }, SchemaValue::S64(v)) => {
            write!(buf, "{v}").unwrap();
        }
        (SchemaType::F32 { .. }, SchemaValue::F32(v)) => render_f32(buf, *v),
        (SchemaType::F64 { .. }, SchemaValue::F64(v)) => render_f64(buf, *v),
        (SchemaType::Char { .. }, SchemaValue::Char(c)) => {
            buf.push('"');
            write_json_escaped_char(buf, *c);
            buf.push('"');
        }
        (SchemaType::String { .. }, SchemaValue::String(s)) => {
            buf.push('"');
            write_json_escaped(buf, s);
            buf.push('"');
        }
        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs }) => {
            buf.push_str("{ ");
            for (i, (field, val)) in fields.iter().zip(vs.iter()).enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let name = field.name.to_lower_camel_case();
                write!(buf, "{name}: ").unwrap();
                render_cm_value(buf, graph, &field.body, val);
            }
            buf.push_str(" }");
        }
        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: vs }) => {
            buf.push('[');
            for (i, (t, v)) in elements.iter().zip(vs.iter()).enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, graph, t, v);
            }
            buf.push(']');
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
        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(p)) => {
            render_variant(buf, graph, cases, p);
        }
        (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case }) => {
            let case_name = &cases[*case as usize];
            buf.push('"');
            write_json_escaped(buf, case_name);
            buf.push('"');
        }
        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: v }) => match v {
            Some(payload) => {
                let (inner_resolved, _) = resolve_named_ref(graph, inner);
                if matches!(inner_resolved, SchemaType::Option { .. }) {
                    buf.push_str("{ some: ");
                    render_cm_value(buf, graph, inner, payload);
                    buf.push_str(" }");
                } else {
                    render_cm_value(buf, graph, inner, payload);
                }
            }
            None => buf.push_str("undefined"),
        },
        (SchemaType::Result { spec, .. }, SchemaValue::Result(p)) => {
            render_result(buf, graph, spec, p);
        }
        (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits }) => {
            buf.push_str("{ ");
            let mut first = true;
            for (i, is_set) in bits.iter().enumerate() {
                if *is_set {
                    if !first {
                        buf.push_str(", ");
                    }
                    let name = flags[i].to_lower_camel_case();
                    write!(buf, "{name}: true").unwrap();
                    first = false;
                }
            }
            buf.push_str(" }");
        }
        // Rich semantic types render as constructor calls `Name("body")`,
        // close to how a TS SDK would construct them, and round-trip
        // through the shared lexer's quoted-string handling.
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
        _ => {
            buf.push_str("undefined");
        }
    }
}

fn render_variant(
    buf: &mut String,
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
    payload: &golem_common::schema::schema_value::VariantValuePayload,
) {
    let idx = payload.case as usize;
    let case_name = &cases[idx].name;
    match (&cases[idx].payload, &payload.payload) {
        (Some(payload_type), Some(value)) => {
            buf.push_str("{ tag: \"");
            write_json_escaped(buf, case_name);
            buf.push_str("\", value: ");
            render_cm_value(buf, graph, payload_type, value);
            buf.push_str(" }");
        }
        _ => {
            buf.push_str("{ tag: \"");
            write_json_escaped(buf, case_name);
            buf.push_str("\" }");
        }
    }
}

fn render_result(
    buf: &mut String,
    graph: &SchemaGraph,
    spec: &ResultSpec,
    payload: &ResultValuePayload,
) {
    match payload {
        ResultValuePayload::Ok { value } => {
            buf.push_str("{ ok: ");
            match (value, &spec.ok) {
                (Some(v), Some(t)) => render_cm_value(buf, graph, t, v),
                _ => buf.push_str("undefined"),
            }
            buf.push_str(" }");
        }
        ResultValuePayload::Err { value } => {
            buf.push_str("{ error: ");
            match (value, &spec.err) {
                (Some(v), Some(t)) => render_cm_value(buf, graph, t, v),
                _ => buf.push_str("undefined"),
            }
            buf.push_str(" }");
        }
    }
}

pub fn render_type_ts(graph: &SchemaGraph, ty: &SchemaType, prefer_name: bool) -> String {
    let (resolved, def_name) = resolve_named_ref(graph, ty);
    render_type_ts_inner(graph, resolved, def_name, prefer_name)
}

fn render_type_ts_inner(
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
        SchemaType::String { .. } | SchemaType::Char { .. } => "string".to_string(),
        SchemaType::Bool { .. } => "boolean".to_string(),
        SchemaType::U8 { .. }
        | SchemaType::U16 { .. }
        | SchemaType::U32 { .. }
        | SchemaType::U64 { .. }
        | SchemaType::S8 { .. }
        | SchemaType::S16 { .. }
        | SchemaType::S32 { .. }
        | SchemaType::S64 { .. }
        | SchemaType::F32 { .. }
        | SchemaType::F64 { .. } => "number".to_string(),
        SchemaType::Option { inner, .. } => {
            format!("{} | undefined", render_type_ts(graph, inner, prefer_name))
        }
        SchemaType::List { element, .. } => {
            let (resolved_inner, _) = resolve_named_ref(graph, element);
            if matches!(resolved_inner, SchemaType::U8 { .. }) {
                return "Uint8Array".to_string();
            }
            let inner = render_type_ts(graph, element, prefer_name);
            if inner.contains('|') {
                format!("({inner})[]")
            } else {
                format!("{inner}[]")
            }
        }
        SchemaType::FixedList { element, .. } => {
            let inner = render_type_ts(graph, element, prefer_name);
            format!("{inner}[]")
        }
        SchemaType::Map { key, value, .. } => format!(
            "map<{}, {}>",
            render_type_ts(graph, key, prefer_name),
            render_type_ts(graph, value, prefer_name)
        ),
        SchemaType::Result { spec, .. } => {
            let ok = spec
                .ok
                .as_deref()
                .map(|t| render_type_ts(graph, t, prefer_name))
                .unwrap_or_else(|| "void".to_string());
            let err = spec
                .err
                .as_deref()
                .map(|t| render_type_ts(graph, t, prefer_name))
                .unwrap_or_else(|| "void".to_string());
            format!("Result<{ok}, {err}>")
        }
        SchemaType::Tuple { elements, .. } => render_type_tuple_ts(graph, elements, prefer_name),
        SchemaType::Record { fields, .. } => render_type_record_ts(graph, fields, prefer_name),
        SchemaType::Variant { cases, .. } => render_type_variant_ts(graph, cases, prefer_name),
        SchemaType::Enum { cases, .. } => render_type_enum_ts(cases),
        SchemaType::Flags { flags, .. } => render_type_flags_ts(flags),
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
                .map(|b| format!("{}({})", b.tag, render_type_ts(graph, &b.body, prefer_name)))
                .collect::<Vec<_>>()
                .join(" | ");
            format!("union {{ {inner} }}")
        }
        SchemaType::Future { inner, .. } => match inner {
            None => "future".to_string(),
            Some(t) => format!("future<{}>", render_type_ts(graph, t, prefer_name)),
        },
        SchemaType::Stream { inner, .. } => match inner {
            None => "stream".to_string(),
            Some(t) => format!("stream<{}>", render_type_ts(graph, t, prefer_name)),
        },
        SchemaType::Ref { id, .. } => id.0.clone(),
    }
}

fn render_type_tuple_ts(graph: &SchemaGraph, elements: &[SchemaType], prefer_name: bool) -> String {
    let mut buf = String::from("[");
    for (i, item) in elements.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&render_type_ts(graph, item, prefer_name));
    }
    buf.push(']');
    buf
}

fn render_type_record_ts(
    graph: &SchemaGraph,
    fields: &[NamedFieldType],
    prefer_name: bool,
) -> String {
    let mut buf = String::from("{ ");
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            buf.push(' ');
        }
        let key = field.name.to_lower_camel_case();
        let (resolved, _) = resolve_named_ref(graph, &field.body);
        if let SchemaType::Option { inner, .. } = resolved {
            let inner_rendered = render_type_ts(graph, inner, prefer_name);
            let _ = write!(buf, "{key}?: {inner_rendered};");
        } else {
            let _ = write!(
                buf,
                "{key}: {};",
                render_type_ts(graph, &field.body, prefer_name)
            );
        }
    }
    buf.push_str(" }");
    buf
}

fn render_type_variant_ts(
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
    prefer_name: bool,
) -> String {
    if cases.is_empty() {
        return "never".to_string();
    }
    let mut parts = Vec::new();
    for case in cases {
        let mut tag = String::new();
        write_json_escaped(&mut tag, &case.name);
        if let Some(t) = &case.payload {
            parts.push(format!(
                "{{ tag: \"{tag}\"; value: {} }}",
                render_type_ts(graph, t, prefer_name)
            ));
        } else {
            parts.push(format!("{{ tag: \"{tag}\" }}"));
        }
    }
    parts.join(" | ")
}

fn render_type_enum_ts(cases: &[String]) -> String {
    if cases.is_empty() {
        return "never".to_string();
    }
    cases
        .iter()
        .map(|c| {
            let mut escaped = String::new();
            write_json_escaped(&mut escaped, c);
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn render_type_flags_ts(flags: &[String]) -> String {
    let mut buf = String::from("{ ");
    for (i, flag) in flags.iter().enumerate() {
        if i > 0 {
            buf.push(' ');
        }
        let _ = write!(buf, "{}?: true;", flag.to_lower_camel_case());
    }
    buf.push_str(" }");
    buf
}

fn render_f32(buf: &mut String, v: f32) {
    if v.is_nan() {
        buf.push_str("NaN");
    } else if v == f32::INFINITY {
        buf.push_str("Infinity");
    } else if v == f32::NEG_INFINITY {
        buf.push_str("-Infinity");
    } else if v == 0.0 && v.is_sign_negative() {
        buf.push_str("-0");
    } else {
        write!(buf, "{v}").unwrap();
    }
}

fn render_f64(buf: &mut String, v: f64) {
    if v.is_nan() {
        buf.push_str("NaN");
    } else if v == f64::INFINITY {
        buf.push_str("Infinity");
    } else if v == f64::NEG_INFINITY {
        buf.push_str("-Infinity");
    } else if v == 0.0 && v.is_sign_negative() {
        buf.push_str("-0");
    } else {
        write!(buf, "{v}").unwrap();
    }
}
