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
    UnstructuredTextElementValue,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::Value;
use heck::ToLowerCamelCase;
use std::fmt::Write;

pub(super) fn render_value_and_type_ts(vat: &golem_wasm::ValueAndType) -> String {
    let mut buf = String::new();
    render_cm_value(&mut buf, &vat.value, &vat.typ);
    buf
}

pub fn render_data_value_ts(data_value: &DataValue) -> String {
    let mut buf = String::new();
    match data_value {
        DataValue::Tuple(elems) => {
            for (i, elem) in elems.elements.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_element(&mut buf, elem);
            }
        }
        DataValue::Multimodal(NamedElementValues { elements }) => {
            for (i, named) in elements.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let key = named.name.to_lower_camel_case();
                write!(buf, "{key}: ").unwrap();
                render_element(&mut buf, &named.value);
            }
        }
    }
    buf
}

fn render_element(buf: &mut String, elem: &ElementValue) {
    match elem {
        ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
            render_cm_value(buf, &value.value, &value.typ);
        }
        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
            render_text_element(buf, value);
        }
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            render_binary_element(buf, value);
        }
    }
}

fn render_text_element(buf: &mut String, text_ref: &TextReference) {
    match text_ref {
        TextReference::Url(url) => {
            buf.push_str("{ tag: \"url\", val: \"");
            write_json_escaped(buf, &url.value);
            buf.push_str("\" }");
        }
        TextReference::Inline(TextSource { data, text_type }) => match text_type {
            Some(tt) => {
                buf.push_str("{ tag: \"inline\", val: \"");
                write_json_escaped(buf, data);
                buf.push_str("\", lang: \"");
                write_json_escaped(buf, &tt.language_code);
                buf.push_str("\" }");
            }
            None => {
                buf.push_str("{ tag: \"inline\", val: \"");
                write_json_escaped(buf, data);
                buf.push_str("\" }");
            }
        },
    }
}

fn render_binary_element(buf: &mut String, bin_ref: &BinaryReference) {
    match bin_ref {
        BinaryReference::Url(url) => {
            buf.push_str("{ tag: \"url\", val: \"");
            write_json_escaped(buf, &url.value);
            buf.push_str("\" }");
        }
        BinaryReference::Inline(BinarySource { data, binary_type }) => {
            buf.push_str("{ tag: \"inline\", val: Uint8Array([");
            for (i, b) in data.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write!(buf, "{b}").unwrap();
            }
            buf.push_str("]), mime: \"");
            write_json_escaped(buf, &binary_type.mime_type);
            buf.push_str("\" }");
        }
    }
}

fn render_cm_value(buf: &mut String, value: &Value, typ: &AnalysedType) {
    match (value, typ) {
        (Value::Bool(b), AnalysedType::Bool(_)) => {
            buf.push_str(if *b { "true" } else { "false" });
        }
        (Value::U8(v), AnalysedType::U8(_)) => write!(buf, "{v}").unwrap(),
        (Value::U16(v), AnalysedType::U16(_)) => write!(buf, "{v}").unwrap(),
        (Value::U32(v), AnalysedType::U32(_)) => write!(buf, "{v}").unwrap(),
        (Value::U64(v), AnalysedType::U64(_)) => write!(buf, "{v}").unwrap(),
        (Value::S8(v), AnalysedType::S8(_)) => write!(buf, "{v}").unwrap(),
        (Value::S16(v), AnalysedType::S16(_)) => write!(buf, "{v}").unwrap(),
        (Value::S32(v), AnalysedType::S32(_)) => write!(buf, "{v}").unwrap(),
        (Value::S64(v), AnalysedType::S64(_)) => write!(buf, "{v}").unwrap(),
        (Value::F32(v), AnalysedType::F32(_)) => render_f32(buf, *v),
        (Value::F64(v), AnalysedType::F64(_)) => render_f64(buf, *v),
        (Value::Char(c), AnalysedType::Chr(_)) => {
            buf.push('"');
            write_json_escaped_char(buf, *c);
            buf.push('"');
        }
        (Value::String(s), AnalysedType::Str(_)) => {
            buf.push('"');
            write_json_escaped(buf, s);
            buf.push('"');
        }
        (Value::Record(fields), AnalysedType::Record(type_record)) => {
            buf.push_str("{ ");
            for (i, (field_val, field_type)) in
                fields.iter().zip(type_record.fields.iter()).enumerate()
            {
                if i > 0 {
                    buf.push_str(", ");
                }
                let name = field_type.name.to_lower_camel_case();
                write!(buf, "{name}: ").unwrap();
                render_cm_value(buf, field_val, &field_type.typ);
            }
            buf.push_str(" }");
        }
        (Value::Tuple(items), AnalysedType::Tuple(type_tuple)) => {
            buf.push('[');
            for (i, (item_val, item_type)) in
                items.iter().zip(type_tuple.items.iter()).enumerate()
            {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, item_val, item_type);
            }
            buf.push(']');
        }
        (Value::List(items), AnalysedType::List(type_list)) => {
            buf.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                render_cm_value(buf, item, &type_list.inner);
            }
            buf.push(']');
        }
        (Value::Variant { case_idx, case_value }, AnalysedType::Variant(type_variant)) => {
            let idx = *case_idx as usize;
            let case_name = &type_variant.cases[idx].name;
            match (&type_variant.cases[idx].typ, case_value) {
                (Some(payload_type), Some(payload)) => {
                    buf.push_str("{ tag: \"");
                    write_json_escaped(buf, case_name);
                    buf.push_str("\", value: ");
                    render_cm_value(buf, payload, payload_type);
                    buf.push_str(" }");
                }
                _ => {
                    buf.push_str("{ tag: \"");
                    write_json_escaped(buf, case_name);
                    buf.push_str("\" }");
                }
            }
        }
        (Value::Enum(case_idx), AnalysedType::Enum(type_enum)) => {
            let case_name = &type_enum.cases[*case_idx as usize];
            buf.push('"');
            write_json_escaped(buf, case_name);
            buf.push('"');
        }
        (Value::Option(opt), AnalysedType::Option(type_opt)) => match opt {
            Some(inner) => {
                if matches!(&*type_opt.inner, AnalysedType::Option(_)) {
                    // Nested Option<Option<…>>: wrap Some in `{ some: <inner> }`
                    // so it can be distinguished from None (`null`) at any depth.
                    buf.push_str("{ some: ");
                    render_cm_value(buf, inner, &type_opt.inner);
                    buf.push_str(" }");
                } else {
                    render_cm_value(buf, inner, &type_opt.inner);
                }
            }
            None => buf.push_str("null"),
        },
        (Value::Result(res), AnalysedType::Result(type_res)) => match res {
            Ok(ok_val) => {
                buf.push_str("{ ok: ");
                match (ok_val, &type_res.ok) {
                    (Some(v), Some(t)) => render_cm_value(buf, v, t),
                    _ => buf.push_str("null"),
                }
                buf.push_str(" }");
            }
            Err(err_val) => {
                buf.push_str("{ error: ");
                match (err_val, &type_res.err) {
                    (Some(v), Some(t)) => render_cm_value(buf, v, t),
                    _ => buf.push_str("null"),
                }
                buf.push_str(" }");
            }
        },
        (Value::Flags(flags), AnalysedType::Flags(type_flags)) => {
            buf.push_str("{ ");
            let mut first = true;
            for (i, is_set) in flags.iter().enumerate() {
                if *is_set {
                    if !first {
                        buf.push_str(", ");
                    }
                    let name = type_flags.names[i].to_lower_camel_case();
                    write!(buf, "{name}: true").unwrap();
                    first = false;
                }
            }
            buf.push_str(" }");
        }
        _ => {
            buf.push_str("null");
        }
    }
}

fn render_f32(buf: &mut String, v: f32) {
    if v.is_nan() {
        buf.push_str("NaN");
    } else if v == f32::INFINITY {
        buf.push_str("Infinity");
    } else if v == f32::NEG_INFINITY {
        buf.push_str("-Infinity");
    } else if v == 0.0 && v.is_sign_negative() {
        buf.push_str("-0.0");
    } else {
        let s = format!("{v}");
        write_with_decimal_point(buf, &s);
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
        buf.push_str("-0.0");
    } else {
        let s = format!("{v}");
        write_with_decimal_point(buf, &s);
    }
}

fn write_with_decimal_point(buf: &mut String, s: &str) {
    if s.contains('.') {
        buf.push_str(s);
    } else if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        buf.push_str(&s[..e_pos]);
        buf.push_str(".0");
        buf.push_str(&s[e_pos..]);
    } else {
        buf.push_str(s);
        buf.push_str(".0");
    }
}

fn write_json_escaped(buf: &mut String, s: &str) {
    for ch in s.chars() {
        write_json_escaped_char(buf, ch);
    }
}

fn write_json_escaped_char(buf: &mut String, ch: char) {
    match ch {
        '"' => buf.push_str("\\\""),
        '\\' => buf.push_str("\\\\"),
        '\n' => buf.push_str("\\n"),
        '\r' => buf.push_str("\\r"),
        '\t' => buf.push_str("\\t"),
        '\u{08}' => buf.push_str("\\b"),
        '\u{0C}' => buf.push_str("\\f"),
        c if c.is_control() => {
            let mut code_units = [0u16; 2];
            let encoded = c.encode_utf16(&mut code_units);
            for unit in encoded {
                write!(buf, "\\u{:04x}", unit).unwrap();
            }
        }
        c => buf.push(c),
    }
}
