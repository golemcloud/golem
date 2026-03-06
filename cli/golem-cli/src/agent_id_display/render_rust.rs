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
    text_utils::write_json_escaped, BinaryReference, BinarySource, ComponentModelElementValue,
    DataValue, ElementValue, NamedElementValues, TextReference, TextSource,
    UnstructuredBinaryElementValue, UnstructuredTextElementValue,
};
use golem_wasm::analysis::{AnalysedType, TypeEnum, TypeFlags, TypeRecord, TypeTuple, TypeVariant};
use golem_wasm::Value;
use heck::{ToSnakeCase, ToUpperCamelCase};
use std::fmt::Write;

pub(super) fn render_value_and_type_rust(vat: &golem_wasm::ValueAndType) -> String {
    let mut buf = String::new();
    render_cm_value(&mut buf, &vat.value, &vat.typ);
    buf
}

pub fn render_data_value_rust(data_value: &DataValue) -> String {
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
                let _ = write!(buf, "{}: ", named.name.to_snake_case());
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
            buf.push_str("UnstructuredText::Url(\"");
            write_json_escaped(buf, &url.value);
            buf.push_str("\")");
        }
        TextReference::Inline(TextSource {
            data,
            text_type: None,
        }) => {
            buf.push_str("UnstructuredText::from_inline_any(\"");
            write_json_escaped(buf, data);
            buf.push_str("\")");
        }
        TextReference::Inline(TextSource {
            data,
            text_type: Some(tt),
        }) => {
            buf.push_str("UnstructuredText::from_inline(\"");
            write_json_escaped(buf, data);
            let _ = write!(buf, "\", Languages::{})", tt.language_code);
        }
    }
}

fn render_unstructured_binary(buf: &mut String, value: &BinaryReference) {
    match value {
        BinaryReference::Url(url) => {
            buf.push_str("UnstructuredBinary::from_url(\"");
            write_json_escaped(buf, &url.value);
            buf.push_str("\")");
        }
        BinaryReference::Inline(BinarySource { data, binary_type }) => {
            buf.push_str("UnstructuredBinary::from_inline(vec![");
            for (i, b) in data.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let _ = write!(buf, "{b}");
            }
            let _ = write!(buf, "], MimeTypes::{})", binary_type.mime_type);
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
            buf.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, item, &tl.inner);
            }
            buf.push(']');
        }
        (Value::Tuple(items), AnalysedType::Tuple(tt)) => {
            buf.push('(');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, item, &tt.items[i]);
            }
            buf.push(')');
        }
        (Value::Record(fields), AnalysedType::Record(tr)) => {
            if let Some(name) = &tr.name {
                let _ = write!(buf, "{} ", name.to_upper_camel_case());
            }
            buf.push_str("{ ");
            for (i, (val, field)) in fields.iter().zip(tr.fields.iter()).enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let _ = write!(buf, "{}: ", field.name.to_snake_case());
                render_cm_value(buf, val, &field.typ);
            }
            buf.push_str(" }");
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
                let _ = write!(buf, "{}::", name.to_upper_camel_case());
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
                let _ = write!(buf, "{}::{case_name}", name.to_upper_camel_case());
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
                    buf.push_str("Ok(");
                    if let Some(ok_typ) = &tr.ok {
                        render_cm_value(buf, v, ok_typ);
                    }
                    buf.push(')');
                }
                None => buf.push_str("Ok(())"),
            },
            Err(err_val) => match err_val {
                Some(v) => {
                    buf.push_str("Err(");
                    if let Some(err_typ) = &tr.err {
                        render_cm_value(buf, v, err_typ);
                    }
                    buf.push(')');
                }
                None => buf.push_str("Err(())"),
            },
        },
        (Value::Flags(flags), AnalysedType::Flags(tf)) => {
            if let Some(name) = &tf.name {
                let _ = write!(buf, "{} ", name.to_upper_camel_case());
            }
            buf.push_str("{ ");
            let mut first = true;
            for (set, flag_name) in flags.iter().zip(tf.names.iter()) {
                if *set {
                    if !first {
                        buf.push_str(", ");
                    }
                    buf.push_str(&flag_name.to_snake_case());
                    first = false;
                }
            }
            buf.push_str(" }");
        }
        _ => buf.push_str("<unknown>"),
    }
}

pub fn render_type_rust(typ: &AnalysedType, prefer_name: bool) -> String {
    match typ {
        AnalysedType::Str(_) => "String".to_string(),
        AnalysedType::Chr(_) => "char".to_string(),
        AnalysedType::Bool(_) => "bool".to_string(),
        AnalysedType::U8(_) => "u8".to_string(),
        AnalysedType::U16(_) => "u16".to_string(),
        AnalysedType::U32(_) => "u32".to_string(),
        AnalysedType::U64(_) => "u64".to_string(),
        AnalysedType::S8(_) => "i8".to_string(),
        AnalysedType::S16(_) => "i16".to_string(),
        AnalysedType::S32(_) => "i32".to_string(),
        AnalysedType::S64(_) => "i64".to_string(),
        AnalysedType::F32(_) => "f32".to_string(),
        AnalysedType::F64(_) => "f64".to_string(),
        AnalysedType::Option(to) => {
            format!("Option<{}>", render_type_rust(&to.inner, prefer_name))
        }
        AnalysedType::List(tl) => {
            format!("Vec<{}>", render_type_rust(&tl.inner, prefer_name))
        }
        AnalysedType::Result(tr) => {
            let ok = tr
                .ok
                .as_ref()
                .map(|t| render_type_rust(t, prefer_name))
                .unwrap_or_else(|| "()".to_string());
            let err = tr
                .err
                .as_ref()
                .map(|t| render_type_rust(t, prefer_name))
                .unwrap_or_else(|| "()".to_string());
            format!("Result<{ok}, {err}>")
        }
        AnalysedType::Tuple(tt) => render_type_tuple_rust(tt, prefer_name),
        AnalysedType::Record(tr) => render_type_record_rust(tr, prefer_name),
        AnalysedType::Variant(tv) => render_type_variant_rust(tv, prefer_name),
        AnalysedType::Enum(te) => render_type_enum_rust(te, prefer_name),
        AnalysedType::Flags(tf) => render_type_flags_rust(tf, prefer_name),
        AnalysedType::Handle(_) => {
            panic!("Handle types are not supported in type rendering")
        }
    }
}

fn render_type_tuple_rust(tt: &TypeTuple, prefer_name: bool) -> String {
    let mut buf = String::from("(");
    for (i, item) in tt.items.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&render_type_rust(item, prefer_name));
    }
    if tt.items.len() == 1 {
        buf.push(',');
    }
    buf.push(')');
    buf
}

fn render_type_record_rust(tr: &TypeRecord, prefer_name: bool) -> String {
    if prefer_name {
        if let Some(name) = &tr.name {
            return name.to_upper_camel_case();
        }
    }
    let mut buf = String::from("{ ");
    for (i, field) in tr.fields.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        let _ = write!(
            buf,
            "{}: {}",
            field.name.to_snake_case(),
            render_type_rust(&field.typ, prefer_name)
        );
    }
    buf.push_str(" }");
    buf
}

fn render_type_variant_rust(tv: &TypeVariant, prefer_name: bool) -> String {
    if prefer_name {
        if let Some(name) = &tv.name {
            return name.to_upper_camel_case();
        }
    }
    let mut buf = String::from("enum { ");
    for (i, case) in tv.cases.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&case.name.to_upper_camel_case());
        if let Some(t) = &case.typ {
            let _ = write!(buf, "({})", render_type_rust(t, prefer_name));
        }
    }
    buf.push_str(" }");
    buf
}

fn render_type_enum_rust(te: &TypeEnum, prefer_name: bool) -> String {
    if prefer_name {
        if let Some(name) = &te.name {
            return name.to_upper_camel_case();
        }
    }
    let mut buf = String::from("enum { ");
    for (i, case) in te.cases.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(&case.to_upper_camel_case());
    }
    buf.push_str(" }");
    buf
}

fn render_type_flags_rust(tf: &TypeFlags, prefer_name: bool) -> String {
    if prefer_name {
        if let Some(name) = &tf.name {
            return name.to_upper_camel_case();
        }
    }
    let mut buf = String::from("flags { ");
    for (i, flag) in tf.names.iter().enumerate() {
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
