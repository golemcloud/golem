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

use golem_common::model::agent::{
    BinaryReference, BinarySource, ComponentModelElementValue, DataValue, ElementValue,
    NamedElementValues, TextReference, TextSource, UnstructuredBinaryElementValue,
    UnstructuredTextElementValue, text_utils::write_json_escaped,
};
use golem_wasm::Value;
use golem_wasm::analysis::{AnalysedType, TypeEnum, TypeFlags, TypeRecord, TypeTuple, TypeVariant};
use heck::{ToLowerCamelCase, ToUpperCamelCase};
use std::fmt::Write;

pub(super) fn render_value_and_type_scala(vat: &golem_wasm::ValueAndType) -> String {
    let mut buf = String::new();
    render_cm_value(&mut buf, &vat.value, &vat.typ);
    buf
}

pub fn render_data_value_scala(data_value: &DataValue) -> String {
    let mut buf = String::new();
    match data_value {
        DataValue::Tuple(elems) => {
            for (i, elem) in elems.elements.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_element_value(&mut buf, elem);
            }
        }
        DataValue::Multimodal(NamedElementValues { elements }) => {
            for (i, named) in elements.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let _ = write!(buf, "{} = ", named.name.to_lower_camel_case());
                render_element_value(&mut buf, &named.value);
            }
        }
    }
    buf
}

fn render_element_value(buf: &mut String, elem: &ElementValue) {
    match elem {
        ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
            render_cm_value(buf, &value.value, &value.typ);
        }
        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
            render_unstructured_text(buf, value);
        }
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            render_unstructured_binary(buf, value);
        }
    }
}

fn render_unstructured_text(buf: &mut String, value: &TextReference) {
    match value {
        TextReference::Url(url) => {
            buf.push_str("UnstructuredTextValue.Url(\"");
            write_json_escaped(buf, &url.value);
            buf.push_str("\")");
        }
        TextReference::Inline(TextSource {
            data,
            text_type: Some(tt),
        }) => {
            buf.push_str("UnstructuredTextValue.Inline(\"");
            write_json_escaped(buf, data);
            let _ = write!(buf, "\", Some(\"{}\"))", tt.language_code);
        }
        TextReference::Inline(TextSource {
            data,
            text_type: None,
        }) => {
            buf.push_str("UnstructuredTextValue.Inline(\"");
            write_json_escaped(buf, data);
            buf.push_str("\", None)");
        }
    }
}

fn render_unstructured_binary(buf: &mut String, value: &BinaryReference) {
    match value {
        BinaryReference::Url(url) => {
            buf.push_str("UnstructuredBinaryValue.Url(\"");
            write_json_escaped(buf, &url.value);
            buf.push_str("\")");
        }
        BinaryReference::Inline(BinarySource { data, binary_type }) => {
            buf.push_str("UnstructuredBinaryValue.Inline(Array[Byte](");
            for (i, b) in data.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let _ = write!(buf, "{b}");
            }
            buf.push_str("), \"");
            write_json_escaped(buf, &binary_type.mime_type);
            buf.push_str("\")");
        }
    }
}

fn render_cm_value(buf: &mut String, value: &Value, typ: &AnalysedType) {
    match (value, typ) {
        (Value::Bool(b), _) => {
            let _ = write!(buf, "{b}");
        }
        (Value::U8(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::U16(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::U32(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::U64(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::S8(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::S16(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::S32(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::S64(v), _) => {
            let _ = write!(buf, "{v}");
        }
        (Value::F32(v), _) => render_f64(buf, *v as f64),
        (Value::F64(v), _) => render_f64(buf, *v),
        (Value::Char(c), _) => render_char(buf, *c),
        (Value::String(s), _) => {
            buf.push('"');
            write_json_escaped(buf, s);
            buf.push('"');
        }
        (Value::List(items), AnalysedType::List(tl)) => {
            buf.push_str("List(");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, item, &tl.inner);
            }
            buf.push(')');
        }
        (Value::Tuple(items), AnalysedType::Tuple(tt)) => {
            if tt.items.len() == 1 {
                buf.push_str("Tuple1(");
                if let Some(item) = items.first() {
                    render_cm_value(buf, item, &tt.items[0]);
                }
                buf.push(')');
            } else {
                buf.push('(');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    render_cm_value(buf, item, &tt.items[i]);
                }
                buf.push(')');
            }
        }
        (Value::Record(fields), AnalysedType::Record(tr)) => {
            if let Some(name) = &tr.name {
                let _ = write!(buf, "{}(", name.to_upper_camel_case());
                for (i, (val, field)) in fields.iter().zip(tr.fields.iter()).enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    let _ = write!(buf, "{} = ", field.name.to_lower_camel_case());
                    render_cm_value(buf, val, &field.typ);
                }
                buf.push(')');
            } else {
                buf.push_str("{ ");
                for (i, (val, field)) in fields.iter().zip(tr.fields.iter()).enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    let _ = write!(buf, "{} = ", field.name.to_lower_camel_case());
                    render_cm_value(buf, val, &field.typ);
                }
                buf.push_str(" }");
            }
        }
        (
            Value::Variant {
                case_idx,
                case_value,
            },
            AnalysedType::Variant(tv),
        ) => {
            let case = &tv.cases[*case_idx as usize];
            let case_name = case.name.to_upper_camel_case();
            if let Some(name) = &tv.name {
                let _ = write!(buf, "{}.", name.to_upper_camel_case());
            }
            match (case_value, &case.typ) {
                (Some(v), Some(t)) => {
                    let _ = write!(buf, "{case_name}(");
                    render_cm_value(buf, v, t);
                    buf.push(')');
                }
                _ => buf.push_str(&case_name),
            }
        }
        (Value::Enum(idx), AnalysedType::Enum(te)) => {
            let case_name = te.cases[*idx as usize].to_upper_camel_case();
            if let Some(name) = &te.name {
                let _ = write!(buf, "{}.{case_name}", name.to_upper_camel_case());
            } else {
                buf.push_str(&case_name);
            }
        }
        (Value::Option(inner), AnalysedType::Option(to)) => match inner {
            Some(v) => {
                buf.push_str("Some(");
                render_cm_value(buf, v, &to.inner);
                buf.push(')');
            }
            None => buf.push_str("None"),
        },
        (Value::Result(result), AnalysedType::Result(tr)) => match result {
            Ok(ok_val) => match ok_val {
                Some(v) => {
                    buf.push_str("WitResult.Ok(");
                    if let Some(ok_typ) = &tr.ok {
                        render_cm_value(buf, v, ok_typ);
                    }
                    buf.push(')');
                }
                None => buf.push_str("WitResult.Ok(())"),
            },
            Err(err_val) => match err_val {
                Some(v) => {
                    buf.push_str("WitResult.Err(");
                    if let Some(err_typ) = &tr.err {
                        render_cm_value(buf, v, err_typ);
                    }
                    buf.push(')');
                }
                None => buf.push_str("WitResult.Err(())"),
            },
        },
        (Value::Flags(flags), AnalysedType::Flags(tf)) => {
            if let Some(name) = &tf.name {
                let _ = write!(buf, "{}(", name.to_upper_camel_case());
                for (i, (set, flag_name)) in flags.iter().zip(tf.names.iter()).enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    let _ = write!(
                        buf,
                        "{} = {}",
                        flag_name.to_lower_camel_case(),
                        if *set { "true" } else { "false" }
                    );
                }
                buf.push(')');
            } else {
                buf.push_str("{ ");
                for (i, (set, flag_name)) in flags.iter().zip(tf.names.iter()).enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    let _ = write!(
                        buf,
                        "{} = {}",
                        flag_name.to_lower_camel_case(),
                        if *set { "true" } else { "false" }
                    );
                }
                buf.push_str(" }");
            }
        }
        _ => buf.push_str("<unknown>"),
    }
}

pub fn render_type_scala(typ: &AnalysedType, prefer_name: bool) -> String {
    match typ {
        AnalysedType::Bool(_) => "Boolean".to_string(),
        AnalysedType::S8(_) => "Byte".to_string(),
        AnalysedType::S16(_) => "Short".to_string(),
        AnalysedType::S32(_) => "Int".to_string(),
        AnalysedType::S64(_) => "Long".to_string(),
        AnalysedType::U8(_) => "Byte".to_string(),
        AnalysedType::U16(_) => "Short".to_string(),
        AnalysedType::U32(_) => "Int".to_string(),
        AnalysedType::U64(_) => "Long".to_string(),
        AnalysedType::F32(_) => "Float".to_string(),
        AnalysedType::F64(_) => "Double".to_string(),
        AnalysedType::Chr(_) => "Char".to_string(),
        AnalysedType::Str(_) => "String".to_string(),
        AnalysedType::Option(to) => {
            format!("Option[{}]", render_type_scala(&to.inner, prefer_name))
        }
        AnalysedType::List(tl) => {
            format!("List[{}]", render_type_scala(&tl.inner, prefer_name))
        }
        AnalysedType::Result(tr) => {
            let ok = tr
                .ok
                .as_ref()
                .map(|t| render_type_scala(t, prefer_name))
                .unwrap_or_else(|| "Unit".to_string());
            let err = tr
                .err
                .as_ref()
                .map(|t| render_type_scala(t, prefer_name))
                .unwrap_or_else(|| "Unit".to_string());
            format!("WitResult[{ok}, {err}]")
        }
        AnalysedType::Tuple(tt) => render_type_tuple_scala(tt, prefer_name),
        AnalysedType::Record(tr) => render_type_record_scala(tr, prefer_name),
        AnalysedType::Variant(tv) => render_type_variant_scala(tv, prefer_name),
        AnalysedType::Enum(te) => render_type_enum_scala(te, prefer_name),
        AnalysedType::Flags(tf) => render_type_flags_scala(tf, prefer_name),
        AnalysedType::Handle(_) => {
            panic!("Handle types are not supported in type rendering")
        }
    }
}

fn render_type_tuple_scala(tt: &TypeTuple, prefer_name: bool) -> String {
    if tt.items.len() == 1 {
        let inner = render_type_scala(&tt.items[0], prefer_name);
        format!("Tuple1[{inner}]")
    } else {
        let mut buf = String::from("(");
        for (i, item) in tt.items.iter().enumerate() {
            if i > 0 {
                buf.push_str(", ");
            }
            buf.push_str(&render_type_scala(item, prefer_name));
        }
        buf.push(')');
        buf
    }
}

fn render_type_record_scala(tr: &TypeRecord, prefer_name: bool) -> String {
    if prefer_name && let Some(name) = &tr.name {
        return name.to_upper_camel_case();
    }
    let mut buf = String::from("{ ");
    for (i, field) in tr.fields.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        let _ = write!(
            buf,
            "{}: {}",
            field.name.to_lower_camel_case(),
            render_type_scala(&field.typ, prefer_name)
        );
    }
    buf.push_str(" }");
    buf
}

fn render_type_variant_scala(tv: &TypeVariant, prefer_name: bool) -> String {
    if prefer_name && let Some(name) = &tv.name {
        return name.to_upper_camel_case();
    }
    let mut parts = Vec::new();
    for case in &tv.cases {
        let case_name = case.name.to_upper_camel_case();
        if let Some(t) = &case.typ {
            parts.push(format!("{case_name}({})", render_type_scala(t, prefer_name)));
        } else {
            parts.push(case_name);
        }
    }
    parts.join(" | ")
}

fn render_type_enum_scala(te: &TypeEnum, prefer_name: bool) -> String {
    if prefer_name && let Some(name) = &te.name {
        return name.to_upper_camel_case();
    }
    te.cases
        .iter()
        .map(|c| {
            let mut escaped = String::new();
            write_json_escaped(&mut escaped, c);
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn render_type_flags_scala(tf: &TypeFlags, prefer_name: bool) -> String {
    if prefer_name && let Some(name) = &tf.name {
        return name.to_upper_camel_case();
    }
    let mut buf = String::from("flags { ");
    for (i, flag) in tf.names.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&flag.to_lower_camel_case());
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
            let _ = write!(buf, "\\u{:04X}", c as u32);
        }
        c => buf.push(c),
    }
    buf.push('\'');
}
