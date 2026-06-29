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
use golem_common::schema::host_managed::HostManagedKind;
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, UnionValuePayload};
use heck::{ToSnakeCase, ToUpperCamelCase};
use std::fmt::Write;

pub(super) fn render_value_moonbit(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> String {
    let mut buf = String::new();
    render_cm_value(&mut buf, graph, ty, value);
    buf
}

fn render_cm_value(buf: &mut String, graph: &SchemaGraph, ty: &SchemaType, value: &SchemaValue) {
    let (resolved, def_name) = resolve_named_ref(graph, ty);
    render_cm_value_inner(buf, graph, resolved, def_name, value);
}

fn render_cm_value_inner(
    buf: &mut String,
    graph: &SchemaGraph,
    ty: &SchemaType,
    def_name: Option<&str>,
    value: &SchemaValue,
) {
    // Host-managed capability values never render their raw payload; classify
    // via `HostManagedKind` so future capability kinds redact automatically.
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
                let _ = write!(buf, "{}::", name.to_upper_camel_case());
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
            let case = &cases[p.case as usize];
            let case_name = case.name.to_upper_camel_case();
            if let Some(name) = def_name {
                let _ = write!(buf, "{}::", name.to_upper_camel_case());
            }
            match (&p.payload, &case.payload) {
                (Some(v), Some(t)) => {
                    let _ = write!(buf, "{case_name}(");
                    render_cm_value(buf, graph, t, v);
                    buf.push(')');
                }
                _ => buf.push_str(&case_name),
            }
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
                let _ = write!(buf, "{}::", name.to_upper_camel_case());
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
        // Rich semantic types render as constructor calls `Name("body")`,
        // matching MoonBit constructor-call syntax and round-tripping
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

pub fn render_type_moonbit(graph: &SchemaGraph, ty: &SchemaType, prefer_name: bool) -> String {
    let (resolved, def_name) = resolve_named_ref(graph, ty);
    render_type_moonbit_inner(graph, resolved, def_name, prefer_name)
}

fn render_type_moonbit_inner(
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
        SchemaType::Bool { .. } => "Bool".to_string(),
        SchemaType::S8 { .. } | SchemaType::S16 { .. } | SchemaType::S32 { .. } => {
            "Int".to_string()
        }
        SchemaType::S64 { .. } => "Int64".to_string(),
        SchemaType::U8 { .. } | SchemaType::U16 { .. } | SchemaType::U32 { .. } => {
            "UInt".to_string()
        }
        SchemaType::U64 { .. } => "UInt64".to_string(),
        SchemaType::F32 { .. } => "Float".to_string(),
        SchemaType::F64 { .. } => "Double".to_string(),
        SchemaType::Char { .. } => "Char".to_string(),
        SchemaType::String { .. } => "String".to_string(),
        SchemaType::Option { inner, .. } => {
            format!("{}?", render_type_moonbit(graph, inner, prefer_name))
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            format!(
                "Array[{}]",
                render_type_moonbit(graph, element, prefer_name)
            )
        }
        SchemaType::Map { key, value, .. } => format!(
            "Map[{}, {}]",
            render_type_moonbit(graph, key, prefer_name),
            render_type_moonbit(graph, value, prefer_name)
        ),
        SchemaType::Result { spec, .. } => {
            let ok = spec
                .ok
                .as_deref()
                .map(|t| render_type_moonbit(graph, t, prefer_name))
                .unwrap_or_else(|| "Unit".to_string());
            let err = spec
                .err
                .as_deref()
                .map(|t| render_type_moonbit(graph, t, prefer_name))
                .unwrap_or_else(|| "Unit".to_string());
            format!("Result[{ok}, {err}]")
        }
        SchemaType::Tuple { elements, .. } => {
            render_type_tuple_moonbit(graph, elements, prefer_name)
        }
        SchemaType::Record { fields, .. } => render_type_record_moonbit(graph, fields, prefer_name),
        SchemaType::Variant { cases, .. } => render_type_variant_moonbit(graph, cases, prefer_name),
        SchemaType::Enum { cases, .. } => render_type_enum_moonbit(cases),
        SchemaType::Flags { flags, .. } => render_type_flags_moonbit(flags),
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
                        render_type_moonbit(graph, &b.body, prefer_name)
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            format!("union {{ {inner} }}")
        }
        SchemaType::Future { inner, .. } => match inner {
            None => "future".to_string(),
            Some(t) => format!("future<{}>", render_type_moonbit(graph, t, prefer_name)),
        },
        SchemaType::Stream { inner, .. } => match inner {
            None => "stream".to_string(),
            Some(t) => format!("stream<{}>", render_type_moonbit(graph, t, prefer_name)),
        },
        SchemaType::Ref { id, .. } => id.0.clone(),
    }
}

fn render_type_tuple_moonbit(
    graph: &SchemaGraph,
    elements: &[SchemaType],
    prefer_name: bool,
) -> String {
    let mut buf = String::from("(");
    for (i, item) in elements.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&render_type_moonbit(graph, item, prefer_name));
    }
    buf.push(')');
    buf
}

fn render_type_record_moonbit(
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
            render_type_moonbit(graph, &field.body, prefer_name)
        );
    }
    buf.push_str(" }");
    buf
}

fn render_type_variant_moonbit(
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
    prefer_name: bool,
) -> String {
    let mut parts = Vec::new();
    for case in cases {
        let case_name = case.name.to_upper_camel_case();
        if let Some(t) = &case.payload {
            parts.push(format!(
                "{case_name}({})",
                render_type_moonbit(graph, t, prefer_name)
            ));
        } else {
            parts.push(case_name);
        }
    }
    parts.join(" | ")
}

fn render_type_enum_moonbit(cases: &[String]) -> String {
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

fn render_type_flags_moonbit(flags: &[String]) -> String {
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
