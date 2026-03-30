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

//! Canonical structural format for agent IDs.
//!
//! This module implements a positional, structural-only encoding for agent-ID
//! parameter values. No field/variant/case names appear in the canonical form —
//! everything is positional, making the encoding language-independent.
//!
//! The three public entry points:
//! - [`format_structural`] — serialize a `DataValue` + schema → canonical string
//! - [`parse_structural`] — parse a canonical string + schema → `DataValue`
//! - [`normalize_structural`] — strip whitespace outside string literals (no schema needed)

use crate::model::agent::text_utils::{
    write_json_escaped, write_json_escaped_char, write_with_decimal_point,
};
use crate::model::agent::{
    BinaryReference, BinarySource, BinaryType, ComponentModelElementSchema,
    ComponentModelElementValue, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementSchemas, NamedElementValue, NamedElementValues, TextReference, TextSource,
    TextType, UnstructuredBinaryElementValue, UnstructuredTextElementValue, Url,
};
use base64::Engine;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use std::fmt::Write;
use thiserror::Error;

// ── Constants ───────────────────────────────────────────────────────────────

const MAX_DEPTH: usize = 32;

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq)]
pub enum StructuralFormatError {
    #[error("Rejected NaN/Infinity float value")]
    RejectedFloat,
    #[error("Handle types cannot be serialized to agent IDs")]
    HandleType,
    #[error("Max nesting depth ({0}) exceeded")]
    MaxDepthExceeded(usize),
    #[error("Parse error at position {position}: {message}")]
    ParseError { position: usize, message: String },
    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Format a `DataValue` into canonical structural form.
pub fn format_structural(data_value: &DataValue) -> Result<String, StructuralFormatError> {
    let mut buf = String::new();
    match data_value {
        DataValue::Tuple(elems) => {
            format_tuple_elems(&mut buf, elems, 0)?;
        }
        DataValue::Multimodal(elems) => {
            format_multimodal_elems(&mut buf, elems)?;
        }
    }
    Ok(buf)
}

/// Parse a canonical structural string back into a `DataValue` using the given schema.
pub fn parse_structural(s: &str, schema: &DataSchema) -> Result<DataValue, StructuralFormatError> {
    let mut parser = Parser::new(s);
    let result = match schema {
        DataSchema::Tuple(schemas) => {
            let elems = parser.parse_tuple_elems(schemas, 0)?;
            DataValue::Tuple(elems)
        }
        DataSchema::Multimodal(schemas) => {
            let elems = parser.parse_multimodal_elems(schemas)?;
            DataValue::Multimodal(elems)
        }
    };
    if parser.pos < parser.input.len() {
        return Err(parser.error(&format!(
            "Unexpected trailing input: {:?}",
            &parser.input[parser.pos..]
        )));
    }
    Ok(result)
}

/// Normalize a canonical structural string by stripping whitespace outside string literals.
/// Does not require a schema.
pub fn normalize_structural(s: &str) -> String {
    let bytes = s.as_bytes();

    // Fast path: check if there is any whitespace outside strings to strip
    {
        let mut in_string = false;
        let mut escape_next = false;
        let mut has_outside_ws = false;
        for &b in bytes {
            if escape_next {
                escape_next = false;
                continue;
            }
            if in_string {
                if b == b'\\' {
                    escape_next = true;
                } else if b == b'"' {
                    in_string = false;
                }
                continue;
            }
            if matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
                has_outside_ws = true;
                break;
            }
            if b == b'"' {
                in_string = true;
            }
        }
        if !has_outside_ws {
            return s.to_owned();
        }
    }

    let mut result = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut escape_next = false;

    for &b in bytes {
        if escape_next {
            result.push(b);
            escape_next = false;
            continue;
        }
        if in_string {
            result.push(b);
            if b == b'\\' {
                escape_next = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        if matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
            continue;
        }
        if b == b'"' {
            in_string = true;
        }
        result.push(b);
    }
    // SAFETY: We only removed ASCII whitespace bytes from valid UTF-8, which is still valid UTF-8.
    unsafe { String::from_utf8_unchecked(result) }
}

// ── Formatter internals ─────────────────────────────────────────────────────

fn format_tuple_elems(
    buf: &mut String,
    elems: &ElementValues,
    depth: usize,
) -> Result<(), StructuralFormatError> {
    for (i, elem) in elems.elements.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        format_element(buf, elem, depth)?;
    }
    Ok(())
}

fn format_multimodal_elems(
    buf: &mut String,
    elems: &NamedElementValues,
) -> Result<(), StructuralFormatError> {
    for (i, named_elem) in elems.elements.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        write!(buf, "{}(", named_elem.schema_index).unwrap();
        format_element(buf, &named_elem.value, 0)?;
        buf.push(')');
    }
    Ok(())
}

fn format_element(
    buf: &mut String,
    elem: &ElementValue,
    depth: usize,
) -> Result<(), StructuralFormatError> {
    match elem {
        ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
            format_cm_value(buf, &value.value, &value.typ, depth)?;
        }
        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
            format_text_element(buf, value);
        }
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            format_binary_element(buf, value);
        }
    }
    Ok(())
}

fn format_cm_value(
    buf: &mut String,
    value: &Value,
    typ: &AnalysedType,
    depth: usize,
) -> Result<(), StructuralFormatError> {
    if depth >= MAX_DEPTH {
        return Err(StructuralFormatError::MaxDepthExceeded(MAX_DEPTH));
    }

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
        (Value::F32(v), AnalysedType::F32(_)) => {
            format_float_f32(buf, *v)?;
        }
        (Value::F64(v), AnalysedType::F64(_)) => {
            format_float_f64(buf, *v)?;
        }
        (Value::Char(c), AnalysedType::Chr(_)) => {
            buf.push_str("c\"");
            write_json_escaped_char(buf, *c);
            buf.push('"');
        }
        (Value::String(s), AnalysedType::Str(_)) => {
            buf.push('"');
            write_json_escaped(buf, s);
            buf.push('"');
        }
        (Value::Record(fields), AnalysedType::Record(type_record)) => {
            buf.push('(');
            for (i, (field_val, field_type)) in
                fields.iter().zip(type_record.fields.iter()).enumerate()
            {
                if i > 0 {
                    buf.push(',');
                }
                format_cm_value(buf, field_val, &field_type.typ, depth + 1)?;
            }
            if fields.is_empty() {
                // empty record is ()
            }
            buf.push(')');
        }
        (Value::Tuple(items), AnalysedType::Tuple(type_tuple)) => {
            buf.push('(');
            for (i, (item_val, item_type)) in items.iter().zip(type_tuple.items.iter()).enumerate()
            {
                if i > 0 {
                    buf.push(',');
                }
                format_cm_value(buf, item_val, item_type, depth + 1)?;
            }
            buf.push(')');
        }
        (Value::List(items), AnalysedType::List(type_list)) => {
            buf.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                format_cm_value(buf, item, &type_list.inner, depth + 1)?;
            }
            buf.push(']');
        }
        (
            Value::Variant {
                case_idx,
                case_value,
            },
            AnalysedType::Variant(type_variant),
        ) => {
            let idx = *case_idx as usize;
            if idx >= type_variant.cases.len() {
                return Err(StructuralFormatError::SchemaMismatch(format!(
                    "Variant case index {} out of range ({})",
                    idx,
                    type_variant.cases.len()
                )));
            }
            write!(buf, "v{case_idx}").unwrap();
            match (&type_variant.cases[idx].typ, case_value) {
                (Some(payload_type), Some(payload)) => {
                    buf.push('(');
                    format_cm_value(buf, payload, payload_type, depth + 1)?;
                    buf.push(')');
                }
                (None, None) | (None, Some(_)) => {}
                (Some(_), None) => {
                    return Err(StructuralFormatError::SchemaMismatch(format!(
                        "Variant case {} expects payload but value has none",
                        idx
                    )));
                }
            }
        }
        (Value::Enum(case_idx), AnalysedType::Enum(_)) => {
            write!(buf, "v{case_idx}").unwrap();
        }
        (Value::Option(opt), AnalysedType::Option(type_opt)) => match opt {
            Some(inner) => {
                buf.push_str("s(");
                format_cm_value(buf, inner, &type_opt.inner, depth + 1)?;
                buf.push(')');
            }
            None => {
                buf.push('n');
            }
        },
        (Value::Result(res), AnalysedType::Result(type_res)) => match res {
            Ok(ok_val) => {
                if let Some(ok_val) = ok_val {
                    if let Some(ref ok_type) = type_res.ok {
                        buf.push_str("ok(");
                        format_cm_value(buf, ok_val, ok_type, depth + 1)?;
                        buf.push(')');
                    } else {
                        buf.push_str("ok");
                    }
                } else {
                    buf.push_str("ok");
                }
            }
            Err(err_val) => {
                if let Some(err_val) = err_val {
                    if let Some(ref err_type) = type_res.err {
                        buf.push_str("err(");
                        format_cm_value(buf, err_val, err_type, depth + 1)?;
                        buf.push(')');
                    } else {
                        buf.push_str("err");
                    }
                } else {
                    buf.push_str("err");
                }
            }
        },
        (Value::Flags(flags), AnalysedType::Flags(_)) => {
            buf.push_str("f(");
            let mut first = true;
            for (i, is_set) in flags.iter().enumerate() {
                if *is_set {
                    if !first {
                        buf.push(',');
                    }
                    write!(buf, "{i}").unwrap();
                    first = false;
                }
            }
            buf.push(')');
        }
        (Value::Handle { .. }, AnalysedType::Handle(_)) => {
            return Err(StructuralFormatError::HandleType);
        }
        _ => {
            return Err(StructuralFormatError::SchemaMismatch(format!(
                "Value/AnalysedType mismatch: {:?} vs {:?}",
                std::mem::discriminant(value),
                std::mem::discriminant(typ)
            )));
        }
    }
    Ok(())
}

fn format_float_f32(buf: &mut String, v: f32) -> Result<(), StructuralFormatError> {
    if v.is_nan() || v.is_infinite() {
        return Err(StructuralFormatError::RejectedFloat);
    }
    if v == 0.0 {
        buf.push_str("0.0");
        return Ok(());
    }
    let s = format!("{v}");
    write_with_decimal_point(buf, &s);
    Ok(())
}

fn format_float_f64(buf: &mut String, v: f64) -> Result<(), StructuralFormatError> {
    if v.is_nan() || v.is_infinite() {
        return Err(StructuralFormatError::RejectedFloat);
    }
    if v == 0.0 {
        buf.push_str("0.0");
        return Ok(());
    }
    let s = format!("{v}");
    write_with_decimal_point(buf, &s);
    Ok(())
}

fn format_text_element(buf: &mut String, text_ref: &TextReference) {
    match text_ref {
        TextReference::Url(url) => {
            buf.push_str("@tu\"");
            write_json_escaped(buf, &url.value);
            buf.push('"');
        }
        TextReference::Inline(TextSource { data, text_type }) => match text_type {
            Some(TextType { language_code }) => {
                buf.push_str("@t[");
                buf.push_str(language_code);
                buf.push_str("]\"");
                write_json_escaped(buf, data);
                buf.push('"');
            }
            None => {
                buf.push_str("@t\"");
                write_json_escaped(buf, data);
                buf.push('"');
            }
        },
    }
}

fn format_binary_element(buf: &mut String, bin_ref: &BinaryReference) {
    match bin_ref {
        BinaryReference::Url(url) => {
            buf.push_str("@bu\"");
            write_json_escaped(buf, &url.value);
            buf.push('"');
        }
        BinaryReference::Inline(BinarySource { data, binary_type }) => {
            buf.push_str("@b[");
            buf.push_str(&binary_type.mime_type);
            buf.push_str("]\"");
            base64::engine::general_purpose::STANDARD.encode_string(data, buf);
            buf.push('"');
        }
    }
}

// ── Parser ──────────────────────────────────────────────────────────────────

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    #[inline]
    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    #[inline]
    fn advance(&mut self) {
        if let Some(ch) = self.peek() {
            self.pos += ch.len_utf8();
        }
    }

    #[inline]
    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), StructuralFormatError> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("Expected '{}', got {:?}", expected, self.peek())))
        }
    }

    #[inline]
    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    #[inline]
    fn starts_with(&self, prefix: &str) -> bool {
        self.remaining().starts_with(prefix)
    }

    #[inline]
    fn eat_str(&mut self, s: &str) -> bool {
        if self.starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn error(&self, message: &str) -> StructuralFormatError {
        StructuralFormatError::ParseError {
            position: self.pos,
            message: message.to_string(),
        }
    }

    #[inline]
    fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    // ── Top-level parse methods ─────────────────────────────────────────

    fn parse_tuple_elems(
        &mut self,
        schemas: &NamedElementSchemas,
        depth: usize,
    ) -> Result<ElementValues, StructuralFormatError> {
        if schemas.elements.is_empty() {
            return Ok(ElementValues {
                elements: Vec::new(),
            });
        }
        let mut elements = Vec::with_capacity(schemas.elements.len());
        for (i, schema) in schemas.elements.iter().enumerate() {
            if i > 0 {
                if self.at_end() || self.peek() != Some(',') {
                    // Check if all remaining schemas (i..len) are optional CM types
                    let remaining = &schemas.elements[i..];
                    if remaining
                        .iter()
                        .all(|s| is_option_element_schema(&s.schema))
                    {
                        for s in remaining {
                            elements.push(default_option_element(&s.schema)?);
                        }
                        break;
                    }
                    self.expect(',')?; // will fail with a proper error message
                }
                self.expect(',')?;
            }
            elements.push(self.parse_element(&schema.schema, depth)?);
        }
        Ok(ElementValues { elements })
    }

    fn parse_multimodal_elems(
        &mut self,
        schemas: &NamedElementSchemas,
    ) -> Result<NamedElementValues, StructuralFormatError> {
        if self.at_end() {
            return Ok(NamedElementValues {
                elements: Vec::new(),
            });
        }
        let mut elements = Vec::new();
        let mut first = true;
        loop {
            if !first && !self.eat(',') {
                break;
            }
            first = false;

            if self.at_end() {
                break;
            }

            let idx = self.parse_nat()?;
            if idx >= schemas.elements.len() {
                return Err(self.error(&format!(
                    "Multimodal element index {} out of range ({} elements)",
                    idx,
                    schemas.elements.len()
                )));
            }
            self.expect('(')?;
            let schema = &schemas.elements[idx];
            let value = self.parse_element(&schema.schema, 0)?;
            self.expect(')')?;

            elements.push(NamedElementValue {
                name: schema.name.clone(),
                value,
                schema_index: idx as u32,
            });
        }
        Ok(NamedElementValues { elements })
    }

    fn parse_element(
        &mut self,
        schema: &ElementSchema,
        depth: usize,
    ) -> Result<ElementValue, StructuralFormatError> {
        match schema {
            ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
                let value = self.parse_cm_value(element_type, depth)?;
                Ok(ElementValue::ComponentModel(ComponentModelElementValue {
                    value,
                }))
            }
            ElementSchema::UnstructuredText(descriptor) => {
                let value = self.parse_text_element()?;
                Ok(ElementValue::UnstructuredText(
                    UnstructuredTextElementValue {
                        value,
                        descriptor: descriptor.clone(),
                    },
                ))
            }
            ElementSchema::UnstructuredBinary(descriptor) => {
                let value = self.parse_binary_element()?;
                Ok(ElementValue::UnstructuredBinary(
                    UnstructuredBinaryElementValue {
                        value,
                        descriptor: descriptor.clone(),
                    },
                ))
            }
        }
    }

    // ── Component model value parsing ───────────────────────────────────

    fn parse_cm_value(
        &mut self,
        typ: &AnalysedType,
        depth: usize,
    ) -> Result<ValueAndType, StructuralFormatError> {
        if depth >= MAX_DEPTH {
            return Err(StructuralFormatError::MaxDepthExceeded(MAX_DEPTH));
        }

        match typ {
            AnalysedType::Bool(_) => {
                if self.eat_str("true") {
                    Ok(ValueAndType::new(Value::Bool(true), typ.clone()))
                } else if self.eat_str("false") {
                    Ok(ValueAndType::new(Value::Bool(false), typ.clone()))
                } else {
                    Err(self.error("Expected 'true' or 'false'"))
                }
            }
            AnalysedType::U8(_) => {
                let v = self.parse_unsigned::<u8>()?;
                Ok(ValueAndType::new(Value::U8(v), typ.clone()))
            }
            AnalysedType::U16(_) => {
                let v = self.parse_unsigned::<u16>()?;
                Ok(ValueAndType::new(Value::U16(v), typ.clone()))
            }
            AnalysedType::U32(_) => {
                let v = self.parse_unsigned::<u32>()?;
                Ok(ValueAndType::new(Value::U32(v), typ.clone()))
            }
            AnalysedType::U64(_) => {
                let v = self.parse_unsigned::<u64>()?;
                Ok(ValueAndType::new(Value::U64(v), typ.clone()))
            }
            AnalysedType::S8(_) => {
                let v = self.parse_signed::<i8>()?;
                Ok(ValueAndType::new(Value::S8(v), typ.clone()))
            }
            AnalysedType::S16(_) => {
                let v = self.parse_signed::<i16>()?;
                Ok(ValueAndType::new(Value::S16(v), typ.clone()))
            }
            AnalysedType::S32(_) => {
                let v = self.parse_signed::<i32>()?;
                Ok(ValueAndType::new(Value::S32(v), typ.clone()))
            }
            AnalysedType::S64(_) => {
                let v = self.parse_signed::<i64>()?;
                Ok(ValueAndType::new(Value::S64(v), typ.clone()))
            }
            AnalysedType::F32(_) => {
                let v = self.parse_float::<f32>()?;
                Ok(ValueAndType::new(Value::F32(v), typ.clone()))
            }
            AnalysedType::F64(_) => {
                let v = self.parse_float::<f64>()?;
                Ok(ValueAndType::new(Value::F64(v), typ.clone()))
            }
            AnalysedType::Chr(_) => {
                if !self.eat_str("c\"") {
                    return Err(self.error("Expected c\" for char literal"));
                }
                let ch = self.parse_single_json_char()?;
                self.expect('"')?;
                Ok(ValueAndType::new(Value::Char(ch), typ.clone()))
            }
            AnalysedType::Str(_) => {
                self.expect('"')?;
                let s = self.parse_json_string_contents()?;
                self.expect('"')?;
                Ok(ValueAndType::new(Value::String(s), typ.clone()))
            }
            AnalysedType::Record(type_record) => {
                self.expect('(')?;
                let mut fields = Vec::with_capacity(type_record.fields.len());
                for (i, field_type) in type_record.fields.iter().enumerate() {
                    if i > 0 {
                        if self.peek() == Some(')') {
                            // Check if all remaining fields (i..len) are Option types
                            let remaining = &type_record.fields[i..];
                            if remaining.iter().all(|f| is_option_type(&f.typ)) {
                                for _ in remaining {
                                    fields.push(Value::Option(None));
                                }
                                break;
                            }
                        }
                        self.expect(',')?;
                    }
                    let vt = self.parse_cm_value(&field_type.typ, depth + 1)?;
                    fields.push(vt.value);
                }
                self.expect(')')?;
                Ok(ValueAndType::new(Value::Record(fields), typ.clone()))
            }
            AnalysedType::Tuple(type_tuple) => {
                self.expect('(')?;
                let mut items = Vec::with_capacity(type_tuple.items.len());
                for (i, item_type) in type_tuple.items.iter().enumerate() {
                    if i > 0 {
                        if self.peek() == Some(')') {
                            let remaining = &type_tuple.items[i..];
                            if remaining.iter().all(is_option_type) {
                                for _ in remaining {
                                    items.push(Value::Option(None));
                                }
                                break;
                            }
                        }
                        self.expect(',')?;
                    }
                    let vt = self.parse_cm_value(item_type, depth + 1)?;
                    items.push(vt.value);
                }
                self.expect(')')?;
                Ok(ValueAndType::new(Value::Tuple(items), typ.clone()))
            }
            AnalysedType::List(type_list) => {
                self.expect('[')?;
                let mut items = Vec::with_capacity(8);
                if !self.eat(']') {
                    loop {
                        let vt = self.parse_cm_value(&type_list.inner, depth + 1)?;
                        items.push(vt.value);
                        if !self.eat(',') {
                            break;
                        }
                    }
                    self.expect(']')?;
                }
                Ok(ValueAndType::new(Value::List(items), typ.clone()))
            }
            AnalysedType::Variant(type_variant) => {
                if !self.eat('v') {
                    return Err(self.error("Expected 'v' for variant"));
                }
                let case_idx = self.parse_nat()?;
                if case_idx >= type_variant.cases.len() {
                    return Err(self.error(&format!(
                        "Variant case index {} out of range ({} cases)",
                        case_idx,
                        type_variant.cases.len()
                    )));
                }
                let has_payload_type = type_variant.cases[case_idx].typ.is_some();
                let case_value = if self.eat('(') {
                    if let Some(ref payload_type) = type_variant.cases[case_idx].typ {
                        let vt = self.parse_cm_value(payload_type, depth + 1)?;
                        self.expect(')')?;
                        Some(Box::new(vt.value))
                    } else {
                        return Err(self.error(&format!(
                            "Variant case {} has no payload type but got '('",
                            case_idx
                        )));
                    }
                } else if has_payload_type {
                    return Err(self.error(&format!(
                        "Variant case {} requires payload but none provided",
                        case_idx
                    )));
                } else {
                    None
                };
                Ok(ValueAndType::new(
                    Value::Variant {
                        case_idx: case_idx as u32,
                        case_value,
                    },
                    typ.clone(),
                ))
            }
            AnalysedType::Enum(type_enum) => {
                if !self.eat('v') {
                    return Err(self.error("Expected 'v' for enum"));
                }
                let case_idx = self.parse_nat()?;
                if case_idx >= type_enum.cases.len() {
                    return Err(self.error(&format!(
                        "Enum case index {} out of range ({} cases)",
                        case_idx,
                        type_enum.cases.len()
                    )));
                }
                Ok(ValueAndType::new(Value::Enum(case_idx as u32), typ.clone()))
            }
            AnalysedType::Option(type_opt) => {
                if self.eat_str("s(") {
                    let vt = self.parse_cm_value(&type_opt.inner, depth + 1)?;
                    self.expect(')')?;
                    Ok(ValueAndType::new(
                        Value::Option(Some(Box::new(vt.value))),
                        typ.clone(),
                    ))
                } else if self.eat('n') {
                    Ok(ValueAndType::new(Value::Option(None), typ.clone()))
                } else {
                    Err(self.error("Expected 's(' or 'n' for option"))
                }
            }
            AnalysedType::Result(type_res) => {
                if self.eat_str("ok") {
                    if self.eat('(') {
                        if let Some(ref ok_type) = type_res.ok {
                            let vt = self.parse_cm_value(ok_type, depth + 1)?;
                            self.expect(')')?;
                            Ok(ValueAndType::new(
                                Value::Result(Ok(Some(Box::new(vt.value)))),
                                typ.clone(),
                            ))
                        } else {
                            Err(self.error("Result ok type is unit but got '('"))
                        }
                    } else if type_res.ok.is_some() {
                        Err(self.error("Result ok type requires payload but got bare 'ok'"))
                    } else {
                        Ok(ValueAndType::new(Value::Result(Ok(None)), typ.clone()))
                    }
                } else if self.eat_str("err") {
                    if self.eat('(') {
                        if let Some(ref err_type) = type_res.err {
                            let vt = self.parse_cm_value(err_type, depth + 1)?;
                            self.expect(')')?;
                            Ok(ValueAndType::new(
                                Value::Result(Err(Some(Box::new(vt.value)))),
                                typ.clone(),
                            ))
                        } else {
                            Err(self.error("Result err type is unit but got '('"))
                        }
                    } else if type_res.err.is_some() {
                        Err(self.error("Result err type requires payload but got bare 'err'"))
                    } else {
                        Ok(ValueAndType::new(Value::Result(Err(None)), typ.clone()))
                    }
                } else {
                    Err(self.error("Expected 'ok' or 'err' for result"))
                }
            }
            AnalysedType::Flags(type_flags) => {
                if !self.eat_str("f(") {
                    return Err(self.error("Expected 'f(' for flags"));
                }
                let mut flags = vec![false; type_flags.names.len()];
                if !self.eat(')') {
                    let mut prev_idx: Option<usize> = None;
                    loop {
                        let idx = self.parse_nat()?;
                        if idx >= type_flags.names.len() {
                            return Err(self.error(&format!(
                                "Flag index {} out of range ({} flags)",
                                idx,
                                type_flags.names.len()
                            )));
                        }
                        if let Some(prev) = prev_idx
                            && idx <= prev
                        {
                            return Err(self.error(&format!(
                                "Flag indices must be strictly increasing, got {} after {}",
                                idx, prev
                            )));
                        }
                        flags[idx] = true;
                        prev_idx = Some(idx);
                        if !self.eat(',') {
                            break;
                        }
                    }
                    self.expect(')')?;
                }
                Ok(ValueAndType::new(Value::Flags(flags), typ.clone()))
            }
            AnalysedType::Handle(_) => Err(StructuralFormatError::HandleType),
        }
    }

    // ── Unstructured element parsing ────────────────────────────────────

    fn parse_text_element(&mut self) -> Result<TextReference, StructuralFormatError> {
        if !self.eat_str("@t") {
            return Err(self.error("Expected '@t' for text element"));
        }
        if self.eat('u') {
            // @tu"url"
            self.expect('"')?;
            let url = self.parse_json_string_contents()?;
            self.expect('"')?;
            Ok(TextReference::Url(Url { value: url }))
        } else if self.eat('[') {
            // @t[lang]"string"
            let lang = self.parse_bracket_content()?;
            self.expect('"')?;
            let data = self.parse_json_string_contents()?;
            self.expect('"')?;
            Ok(TextReference::Inline(TextSource {
                data,
                text_type: Some(TextType {
                    language_code: lang,
                }),
            }))
        } else {
            // @t"string"
            self.expect('"')?;
            let data = self.parse_json_string_contents()?;
            self.expect('"')?;
            Ok(TextReference::Inline(TextSource {
                data,
                text_type: None,
            }))
        }
    }

    fn parse_binary_element(&mut self) -> Result<BinaryReference, StructuralFormatError> {
        if !self.eat_str("@b") {
            return Err(self.error("Expected '@b' for binary element"));
        }
        if self.eat('u') {
            // @bu"url"
            self.expect('"')?;
            let url = self.parse_json_string_contents()?;
            self.expect('"')?;
            Ok(BinaryReference::Url(Url { value: url }))
        } else if self.eat('[') {
            // @b[mime]"base64"
            let mime = self.parse_bracket_content()?;
            self.expect('"')?;
            let base64_str = self.parse_json_string_contents()?;
            self.expect('"')?;
            let data = base64::engine::general_purpose::STANDARD
                .decode(base64_str.as_bytes())
                .map_err(|e| self.error(&format!("Invalid base64: {e}")))?;
            Ok(BinaryReference::Inline(BinarySource {
                data,
                binary_type: BinaryType { mime_type: mime },
            }))
        } else {
            Err(self.error("Expected 'u' or '[' after @b"))
        }
    }

    // ── Primitive parsing helpers ────────────────────────────────────────

    fn parse_nat(&mut self) -> Result<usize, StructuralFormatError> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.error("Expected natural number"));
        }
        let s = &self.input[start..self.pos];
        // No leading zeros (except "0" itself)
        if s.len() > 1 && s.starts_with('0') {
            return Err(self.error("Leading zeros not allowed in natural number"));
        }
        s.parse::<usize>()
            .map_err(|e| self.error(&format!("Invalid natural number: {e}")))
    }

    fn parse_unsigned<T: std::str::FromStr>(&mut self) -> Result<T, StructuralFormatError>
    where
        T::Err: std::fmt::Display,
    {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.error("Expected unsigned integer"));
        }
        let s = &self.input[start..self.pos];
        if s.len() > 1 && s.starts_with('0') {
            return Err(self.error("Leading zeros not allowed"));
        }
        s.parse::<T>()
            .map_err(|e| self.error(&format!("Invalid unsigned integer: {e}")))
    }

    fn parse_signed<T: std::str::FromStr>(&mut self) -> Result<T, StructuralFormatError>
    where
        T::Err: std::fmt::Display,
    {
        let start = self.pos;
        let negative = self.eat('-');
        let digit_start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        if self.pos == digit_start {
            return Err(self.error("Expected signed integer"));
        }
        let digits = &self.input[digit_start..self.pos];
        if digits.len() > 1 && digits.starts_with('0') {
            return Err(self.error("Leading zeros not allowed"));
        }
        if negative && digits == "0" {
            return Err(self.error("Negative zero not allowed for integers"));
        }
        let s = &self.input[start..self.pos];
        s.parse::<T>()
            .map_err(|e| self.error(&format!("Invalid signed integer: {e}")))
    }

    fn parse_float<T: std::str::FromStr>(&mut self) -> Result<T, StructuralFormatError>
    where
        T::Err: std::fmt::Display,
    {
        let start = self.pos;
        // optional negative sign
        self.eat('-');
        // integer part
        let int_start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        if self.pos == int_start {
            return Err(self.error("Expected float"));
        }
        let int_part = &self.input[int_start..self.pos];
        if int_part.len() > 1 && int_part.starts_with('0') {
            return Err(self.error("Leading zeros not allowed in float"));
        }
        // mandatory decimal point
        if !self.eat('.') {
            return Err(self.error("Float must contain decimal point"));
        }
        // fractional part
        let frac_start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        if self.pos == frac_start {
            return Err(self.error("Expected digits after decimal point"));
        }
        // optional exponent
        if let Some(ch) = self.peek()
            && (ch == 'e' || ch == 'E')
        {
            self.advance();
            if let Some(ch) = self.peek()
                && (ch == '+' || ch == '-')
            {
                self.advance();
            }
            let exp_start = self.pos;
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
            if self.pos == exp_start {
                return Err(self.error("Expected digits in exponent"));
            }
        }
        let s = &self.input[start..self.pos];
        s.parse::<T>()
            .map_err(|e| self.error(&format!("Invalid float: {e}")))
    }

    // ── String parsing ──────────────────────────────────────────────────

    fn parse_json_string_contents(&mut self) -> Result<String, StructuralFormatError> {
        // Fast path: scan bytes for the first `"` or `\\` to see if escapes exist
        let bytes = self.input.as_bytes();
        let start = self.pos;
        let mut scan = self.pos;
        while scan < bytes.len() {
            match bytes[scan] {
                b'"' => {
                    // No escapes — return the slice directly
                    let result = self.input[start..scan].to_owned();
                    self.pos = scan;
                    return Ok(result);
                }
                b'\\' => break, // has escapes, fall through to slow path
                _ => scan += 1,
            }
        }
        if scan >= bytes.len() {
            return Err(self.error("Unterminated string"));
        }

        // Slow path: build result with escape handling
        let remaining_estimate = bytes.len() - start;
        let mut result = String::with_capacity(remaining_estimate.min(256));
        // Copy the prefix we already scanned (up to the first backslash)
        result.push_str(&self.input[start..scan]);
        self.pos = scan;

        loop {
            match self.peek() {
                None => return Err(self.error("Unterminated string")),
                Some('"') => break,
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some('"') => {
                            result.push('"');
                            self.advance();
                        }
                        Some('\\') => {
                            result.push('\\');
                            self.advance();
                        }
                        Some('/') => {
                            result.push('/');
                            self.advance();
                        }
                        Some('b') => {
                            result.push('\u{08}');
                            self.advance();
                        }
                        Some('f') => {
                            result.push('\u{0C}');
                            self.advance();
                        }
                        Some('n') => {
                            result.push('\n');
                            self.advance();
                        }
                        Some('r') => {
                            result.push('\r');
                            self.advance();
                        }
                        Some('t') => {
                            result.push('\t');
                            self.advance();
                        }
                        Some('u') => {
                            self.advance();
                            let ch = self.parse_unicode_escape()?;
                            result.push(ch);
                        }
                        Some(c) => {
                            return Err(self.error(&format!("Invalid escape sequence: \\{c}")));
                        }
                        None => return Err(self.error("Unterminated escape sequence")),
                    }
                }
                Some(c) => {
                    result.push(c);
                    self.advance();
                }
            }
        }
        Ok(result)
    }

    fn parse_single_json_char(&mut self) -> Result<char, StructuralFormatError> {
        match self.peek() {
            None => Err(self.error("Unterminated char literal")),
            Some('\\') => {
                self.advance();
                match self.peek() {
                    Some('"') => {
                        self.advance();
                        Ok('"')
                    }
                    Some('\\') => {
                        self.advance();
                        Ok('\\')
                    }
                    Some('/') => {
                        self.advance();
                        Ok('/')
                    }
                    Some('b') => {
                        self.advance();
                        Ok('\u{08}')
                    }
                    Some('f') => {
                        self.advance();
                        Ok('\u{0C}')
                    }
                    Some('n') => {
                        self.advance();
                        Ok('\n')
                    }
                    Some('r') => {
                        self.advance();
                        Ok('\r')
                    }
                    Some('t') => {
                        self.advance();
                        Ok('\t')
                    }
                    Some('u') => {
                        self.advance();
                        self.parse_unicode_escape()
                    }
                    Some(c) => Err(self.error(&format!("Invalid escape sequence: \\{c}"))),
                    None => Err(self.error("Unterminated escape sequence")),
                }
            }
            Some('"') => Err(self.error("Empty char literal")),
            Some(c) => {
                self.advance();
                Ok(c)
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, StructuralFormatError> {
        let hex = self.parse_hex4()?;
        let code_unit = u16::from_str_radix(&hex, 16)
            .map_err(|e| self.error(&format!("Invalid unicode escape: {e}")))?;

        // Check for surrogate pair
        if (0xD800..=0xDBFF).contains(&code_unit) {
            // High surrogate — expect \u followed by low surrogate
            if !self.eat_str("\\u") {
                return Err(self.error("Expected low surrogate after high surrogate"));
            }
            let hex2 = self.parse_hex4()?;
            let low = u16::from_str_radix(&hex2, 16)
                .map_err(|e| self.error(&format!("Invalid low surrogate: {e}")))?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(self.error("Expected low surrogate (DC00-DFFF)"));
            }
            let code_point = 0x10000 + ((code_unit as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
            char::from_u32(code_point)
                .ok_or_else(|| self.error("Invalid surrogate pair code point"))
        } else {
            char::from_u32(code_unit as u32).ok_or_else(|| self.error("Invalid unicode code point"))
        }
    }

    fn parse_hex4(&mut self) -> Result<String, StructuralFormatError> {
        let mut hex = String::with_capacity(4);
        for _ in 0..4 {
            match self.peek() {
                Some(c) if c.is_ascii_hexdigit() => {
                    hex.push(c);
                    self.advance();
                }
                _ => return Err(self.error("Expected 4 hex digits in \\uXXXX")),
            }
        }
        Ok(hex)
    }

    fn parse_bracket_content(&mut self) -> Result<String, StructuralFormatError> {
        let mut content = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("Unterminated bracket content")),
                Some(']') => {
                    self.advance();
                    break;
                }
                Some(c) => {
                    content.push(c);
                    self.advance();
                }
            }
        }
        Ok(content)
    }
}

fn is_option_type(typ: &AnalysedType) -> bool {
    matches!(typ, AnalysedType::Option(_))
}

fn is_option_element_schema(schema: &ElementSchema) -> bool {
    matches!(
        schema,
        ElementSchema::ComponentModel(ComponentModelElementSchema {
            element_type: AnalysedType::Option(_),
        })
    )
}

fn default_option_element(schema: &ElementSchema) -> Result<ElementValue, StructuralFormatError> {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema {
            element_type: opt_type @ AnalysedType::Option(_),
        }) => Ok(ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Option(None), opt_type.clone()),
        })),
        _ => Err(StructuralFormatError::SchemaMismatch(
            "Expected option type for trailing default".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests;
