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

//! Canonical agent key text.
//!
//! This module implements the positional, structural-only text form of agent
//! constructor parameters. It is the canonical agent key representation: the
//! form users see, and the input of the keyed agent ID hash. No field, variant
//! or case names appear in it, so it is language-independent and a rename does
//! not change it.
//!
//! The formatter writes exactly one text for each value, so equal values give
//! equal text:
//!
//! - Integers are written in decimal without leading zeros; floats always have a
//!   decimal point, and NaN and infinities are rejected.
//! - Strings, chars and tagged strings use JSON escaping. Unicode code points are
//!   kept as given, with no normalization.
//! - A record is `(f1,f2,…)`; the root record has no parentheses. Trailing fields
//!   whose type is an option and whose value is `none` are omitted, and the
//!   parser reads missing trailing option fields as `none`. Appending an option
//!   field to a record therefore keeps the text of existing values.
//! - A tuple is `(e1,e2,…)` and keeps every element.
//! - A map is `m[(k,v),…]` with its entries sorted by the canonical text of their
//!   keys (byte order). The formatter and the parser reject duplicate keys, and
//!   the parser accepts entries in any order.
//! - A variant or enum is `v<case>` with an optional `(payload)`, and a result is
//!   `ok` or `err` with an optional `(payload)`. A value whose payload presence
//!   does not match its case is rejected.
//! - A union is `u<branch>(body)`. The body is omitted when its text is `()`, and
//!   the parser reads a missing body as `()`.
//! - A text value is `@t"…"`, or `@t[lang]"…"` with a language tag. A binary value
//!   is `@b[mime]"<base64>"`, with an empty tag when it has no MIME type. Inside
//!   the brackets `]` and `\` are escaped with a backslash and whitespace is kept.
//!   Language tags are ASCII-lowercased; a MIME type has the part before its first
//!   `;` ASCII-lowercased and its parameters kept as given. An empty tag is the
//!   same as no tag.
//!
//! The text carries no version. Any change to the formatter or the parser changes
//! the address of keyed agents, so it is a breaking change to every keyed agent.
//! The golden tests of this module guard it.
//!
//! The public entry points:
//! - [`format_structural_typed`] — serialize a `TypedSchemaValue` → canonical string
//! - [`parse_structural_typed`] — parse a canonical string + schema → `SchemaValue`
//! - [`normalize_structural`] — strip whitespace outside string literals and tags (no schema needed)

use crate::model::agent::text_utils::{
    write_json_escaped, write_json_escaped_char, write_with_decimal_point,
};
use crate::schema::canonical::{datetime, duration, quantity, uuid as canonical_uuid};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::schema_type::{NamedFieldType, SchemaType, UnionSpec};
use crate::schema::schema_value::{
    BinaryValuePayload, ResultValuePayload, SchemaValue, TextValuePayload, UnionValuePayload,
    VariantValuePayload,
};
use base64::Engine;
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
    #[error("Host-managed capability types cannot be serialized to agent IDs")]
    HostManagedCapability,
    #[error("Max nesting depth ({0}) exceeded")]
    MaxDepthExceeded(usize),
    #[error("Parse error at position {position}: {message}")]
    ParseError { position: usize, message: String },
    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),
    #[error("Duplicate map key: {0}")]
    DuplicateMapKey(String),
    #[error("Payload mismatch: {0}")]
    PayloadMismatch(String),
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Format a constructor/method parameter record as canonical structural text.
/// `parameters.root_type()` must resolve (via `parameters.graph()`) to a
/// `SchemaType::Record`; each field's value is formatted positionally,
/// comma-separated, with NO field names. The outer string has no enclosing
/// parentheses.
pub fn format_structural_typed(
    parameters: &TypedSchemaValue,
) -> Result<String, StructuralFormatError> {
    let graph = parameters.graph();
    let root = graph
        .resolve_ref(parameters.root_type())
        .map_err(|e| StructuralFormatError::SchemaMismatch(e.to_string()))?;
    let SchemaType::Record {
        fields: field_types,
        ..
    } = root
    else {
        return Err(StructuralFormatError::SchemaMismatch(
            "Root schema type must be a record".to_string(),
        ));
    };
    let SchemaValue::Record { fields } = parameters.value() else {
        return Err(StructuralFormatError::SchemaMismatch(
            "Root value must be a record".to_string(),
        ));
    };
    if fields.len() != field_types.len() {
        return Err(StructuralFormatError::SchemaMismatch(format!(
            "Record field count mismatch: value has {}, schema has {}",
            fields.len(),
            field_types.len()
        )));
    }

    let mut buf = String::new();
    format_record_fields(&mut buf, fields, field_types, graph, 0)?;
    Ok(buf)
}

/// Parse canonical structural text into a `SchemaValue::Record` whose fields
/// match `root` (resolved against `graph`), which must resolve to a
/// `SchemaType::Record`. Returns the record `SchemaValue`.
pub fn parse_structural_typed(
    s: &str,
    graph: &SchemaGraph,
    root: &SchemaType,
) -> Result<SchemaValue, StructuralFormatError> {
    let root = graph
        .resolve_ref(root)
        .map_err(|e| StructuralFormatError::SchemaMismatch(e.to_string()))?;
    let SchemaType::Record {
        fields: field_types,
        ..
    } = root
    else {
        return Err(StructuralFormatError::SchemaMismatch(
            "Root schema type must be a record".to_string(),
        ));
    };

    let mut parser = Parser::new(s);
    let fields = parser.parse_record_fields(field_types, None, graph, 0)?;
    if parser.pos < parser.input.len() {
        return Err(parser.error(&format!(
            "Unexpected trailing input: {:?}",
            &parser.input[parser.pos..]
        )));
    }
    Ok(SchemaValue::Record { fields })
}

/// Normalize a canonical structural string by stripping whitespace outside string literals
/// and outside the `@t[…]` and `@b[…]` tags. Does not require a schema.
pub fn normalize_structural(s: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Outside,
        InString,
        InStringEscape,
        InTag,
        InTagEscape,
    }

    let bytes = s.as_bytes();
    let mut result: Option<Vec<u8>> = None;
    let mut state = State::Outside;
    let mut last_kept = [0u8; 2];

    for (i, &b) in bytes.iter().enumerate() {
        let keep = match state {
            State::Outside => {
                if matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
                    false
                } else {
                    if b == b'"' {
                        state = State::InString;
                    } else if b == b'[' && matches!(last_kept, [b'@', b't' | b'b']) {
                        state = State::InTag;
                    }
                    last_kept = [last_kept[1], b];
                    true
                }
            }
            State::InString => {
                match b {
                    b'\\' => state = State::InStringEscape,
                    b'"' => state = State::Outside,
                    _ => {}
                }
                true
            }
            State::InStringEscape => {
                state = State::InString;
                true
            }
            State::InTag => {
                match b {
                    b'\\' => state = State::InTagEscape,
                    b']' => {
                        state = State::Outside;
                        last_kept = [0, b']'];
                    }
                    _ => {}
                }
                true
            }
            State::InTagEscape => {
                state = State::InTag;
                true
            }
        };
        match (&mut result, keep) {
            (Some(result), true) => result.push(b),
            (None, false) => result = Some(bytes[..i].to_vec()),
            _ => {}
        }
    }

    match result {
        None => s.to_owned(),
        // SAFETY: Only ASCII whitespace bytes were removed from valid UTF-8, which keeps it valid UTF-8.
        Some(result) => unsafe { String::from_utf8_unchecked(result) },
    }
}

// ── Formatter internals ─────────────────────────────────────────────────────

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

// ── Schema-native formatter internals ───────────────────────────────────────

fn schema_mismatch(msg: impl Into<String>) -> StructuralFormatError {
    StructuralFormatError::SchemaMismatch(msg.into())
}

fn format_tagged_string(buf: &mut String, tag: &str, value: &str) {
    buf.push('@');
    buf.push_str(tag);
    buf.push('"');
    write_json_escaped(buf, value);
    buf.push('"');
}

fn canonical_err(e: impl std::fmt::Display) -> StructuralFormatError {
    StructuralFormatError::SchemaMismatch(e.to_string())
}

fn format_schema_value(
    buf: &mut String,
    value: &SchemaValue,
    typ: &SchemaType,
    graph: &SchemaGraph,
    depth: usize,
) -> Result<(), StructuralFormatError> {
    if depth >= MAX_DEPTH {
        return Err(StructuralFormatError::MaxDepthExceeded(MAX_DEPTH));
    }
    let typ = graph
        .resolve_ref(typ)
        .map_err(|e| StructuralFormatError::SchemaMismatch(e.to_string()))?;

    match (value, typ) {
        (SchemaValue::Bool(b), SchemaType::Bool { .. }) => {
            buf.push_str(if *b { "true" } else { "false" })
        }
        (SchemaValue::U8(v), SchemaType::U8 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::U16(v), SchemaType::U16 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::U32(v), SchemaType::U32 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::U64(v), SchemaType::U64 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::S8(v), SchemaType::S8 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::S16(v), SchemaType::S16 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::S32(v), SchemaType::S32 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::S64(v), SchemaType::S64 { .. }) => write!(buf, "{v}").unwrap(),
        (SchemaValue::F32(v), SchemaType::F32 { .. }) => format_float_f32(buf, *v)?,
        (SchemaValue::F64(v), SchemaType::F64 { .. }) => format_float_f64(buf, *v)?,
        (SchemaValue::Char(c), SchemaType::Char { .. }) => {
            buf.push_str("c\"");
            write_json_escaped_char(buf, *c);
            buf.push('"');
        }
        (SchemaValue::String(s), SchemaType::String { .. }) => {
            buf.push('"');
            write_json_escaped(buf, s);
            buf.push('"');
        }
        (SchemaValue::Record { fields }, SchemaType::Record { fields: types, .. }) => {
            if fields.len() != types.len() {
                return Err(schema_mismatch(format!(
                    "Record field count mismatch: value has {}, schema has {}",
                    fields.len(),
                    types.len()
                )));
            }
            buf.push('(');
            format_record_fields(buf, fields, types, graph, depth + 1)?;
            buf.push(')');
        }
        (
            SchemaValue::Tuple { elements },
            SchemaType::Tuple {
                elements: types, ..
            },
        ) => {
            if elements.len() != types.len() {
                return Err(schema_mismatch(format!(
                    "Tuple element count mismatch: value has {}, schema has {}",
                    elements.len(),
                    types.len()
                )));
            }
            buf.push('(');
            for (i, (v, t)) in elements.iter().zip(types.iter()).enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                format_schema_value(buf, v, t, graph, depth + 1)?;
            }
            buf.push(')');
        }
        (SchemaValue::List { elements }, SchemaType::List { element, .. })
        | (SchemaValue::FixedList { elements }, SchemaType::FixedList { element, .. }) => {
            buf.push('[');
            for (i, v) in elements.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                format_schema_value(buf, v, element, graph, depth + 1)?;
            }
            buf.push(']');
        }
        (SchemaValue::FixedList { elements }, SchemaType::List { element, .. }) => {
            buf.push('[');
            for (i, v) in elements.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                format_schema_value(buf, v, element, graph, depth + 1)?;
            }
            buf.push(']');
        }
        (SchemaValue::Map { entries }, SchemaType::Map { key, value, .. }) => {
            let mut formatted = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let mut key_text = String::new();
                format_schema_value(&mut key_text, k, key, graph, depth + 1)?;
                let mut value_text = String::new();
                format_schema_value(&mut value_text, v, value, graph, depth + 1)?;
                formatted.push((key_text, value_text));
            }
            formatted.sort_by(|a, b| a.0.cmp(&b.0));
            if let Some(duplicate) = formatted.windows(2).find(|w| w[0].0 == w[1].0) {
                return Err(StructuralFormatError::DuplicateMapKey(
                    duplicate[0].0.clone(),
                ));
            }
            buf.push_str("m[");
            for (i, (k, v)) in formatted.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                buf.push('(');
                buf.push_str(k);
                buf.push(',');
                buf.push_str(v);
                buf.push(')');
            }
            buf.push(']');
        }
        (
            SchemaValue::Variant(VariantValuePayload { case, payload }),
            SchemaType::Variant { cases, .. },
        ) => {
            let idx = *case as usize;
            let case_type = cases.get(idx).ok_or_else(|| {
                schema_mismatch(format!(
                    "Variant case index {idx} out of range ({})",
                    cases.len()
                ))
            })?;
            write!(buf, "v{case}").unwrap();
            match (&case_type.payload, payload) {
                (Some(t), Some(v)) => {
                    buf.push('(');
                    format_schema_value(buf, v, t, graph, depth + 1)?;
                    buf.push(')');
                }
                (None, None) => {}
                (None, Some(_)) => {
                    return Err(StructuralFormatError::PayloadMismatch(format!(
                        "Variant case {idx} has no payload type but value has a payload"
                    )));
                }
                (Some(_), None) => {
                    return Err(StructuralFormatError::PayloadMismatch(format!(
                        "Variant case {idx} expects payload but value has none"
                    )));
                }
            }
        }
        (SchemaValue::Enum { case }, SchemaType::Enum { cases, .. }) => {
            if *case as usize >= cases.len() {
                return Err(schema_mismatch("Enum case index out of range"));
            }
            write!(buf, "v{case}").unwrap();
        }
        (SchemaValue::Flags { bits }, SchemaType::Flags { flags, .. }) => {
            if bits.len() != flags.len() {
                return Err(schema_mismatch("Flags bit count mismatch"));
            }
            buf.push_str("f(");
            let mut first = true;
            for (i, set) in bits.iter().enumerate() {
                if *set {
                    if !first {
                        buf.push(',');
                    }
                    write!(buf, "{i}").unwrap();
                    first = false;
                }
            }
            buf.push(')');
        }
        (SchemaValue::Option { inner }, SchemaType::Option { inner: t, .. }) => match inner {
            Some(v) => {
                buf.push_str("s(");
                format_schema_value(buf, v, t, graph, depth + 1)?;
                buf.push(')');
            }
            None => buf.push('n'),
        },
        (
            SchemaValue::Result(ResultValuePayload::Ok { value }),
            SchemaType::Result { spec, .. },
        ) => format_result_arm(
            buf,
            "ok",
            value.as_deref(),
            spec.ok.as_deref(),
            graph,
            depth,
        )?,
        (
            SchemaValue::Result(ResultValuePayload::Err { value }),
            SchemaType::Result { spec, .. },
        ) => format_result_arm(
            buf,
            "err",
            value.as_deref(),
            spec.err.as_deref(),
            graph,
            depth,
        )?,
        (SchemaValue::Text(TextValuePayload { text, language }), SchemaType::Text { .. }) => {
            buf.push_str("@t");
            if let Some(language) = language.as_deref().filter(|l| !l.is_empty()) {
                buf.push('[');
                write_escaped_tag(buf, &canonical_language_tag(language));
                buf.push(']');
            }
            buf.push('"');
            write_json_escaped(buf, text);
            buf.push('"');
        }
        (
            SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }),
            SchemaType::Binary { .. },
        ) => {
            buf.push_str("@b[");
            if let Some(mime_type) = mime_type {
                write_escaped_tag(buf, &canonical_mime_type(mime_type));
            }
            buf.push_str("]\"");
            base64::engine::general_purpose::STANDARD.encode_string(bytes, buf);
            buf.push('"');
        }
        (SchemaValue::Path { path }, SchemaType::Path { .. }) => {
            format_tagged_string(buf, "p", path)
        }
        (SchemaValue::Url { url }, SchemaType::Url { .. }) => format_tagged_string(buf, "u", url),
        (SchemaValue::Uuid(value), SchemaType::Uuid { .. }) => {
            format_tagged_string(buf, "uuid", &canonical_uuid::to_text(value))
        }
        (SchemaValue::Datetime { value }, SchemaType::Datetime { .. }) => {
            format_tagged_string(buf, "dt", &datetime::to_text(value).map_err(canonical_err)?)
        }
        (SchemaValue::Duration(v), SchemaType::Duration { .. }) => {
            format_tagged_string(buf, "dur", &duration::to_text(v))
        }
        (SchemaValue::Quantity(v), SchemaType::Quantity { .. }) => {
            format_tagged_string(buf, "qty", &quantity::to_text(v).map_err(canonical_err)?)
        }
        (
            _,
            SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::PermissionCard { .. },
        ) => return Err(StructuralFormatError::HostManagedCapability),
        (SchemaValue::Union(UnionValuePayload { tag, body }), SchemaType::Union { spec, .. }) => {
            let (idx, branch) = union_branch(spec, tag)?;
            write!(buf, "u{idx}").unwrap();
            let mut body_text = String::new();
            format_schema_value(&mut body_text, body, &branch.body, graph, depth + 1)?;
            if body_text != "()" {
                buf.push('(');
                buf.push_str(&body_text);
                buf.push(')');
            }
        }
        (_, SchemaType::Future { .. } | SchemaType::Stream { .. }) => {
            return Err(StructuralFormatError::HandleType);
        }
        _ => {
            return Err(schema_mismatch(format!(
                "SchemaValue/SchemaType mismatch: {:?} vs {:?}",
                std::mem::discriminant(value),
                std::mem::discriminant(typ)
            )));
        }
    }
    Ok(())
}

fn format_result_arm(
    buf: &mut String,
    tag: &str,
    value: Option<&SchemaValue>,
    typ: Option<&SchemaType>,
    graph: &SchemaGraph,
    depth: usize,
) -> Result<(), StructuralFormatError> {
    buf.push_str(tag);
    match (value, typ) {
        (Some(v), Some(t)) => {
            buf.push('(');
            format_schema_value(buf, v, t, graph, depth + 1)?;
            buf.push(')');
        }
        (None, None) => {}
        (Some(_), None) => {
            return Err(StructuralFormatError::PayloadMismatch(format!(
                "Result {tag} type has no payload but value has one"
            )));
        }
        (None, Some(_)) => {
            return Err(StructuralFormatError::PayloadMismatch(format!(
                "Result {tag} type requires payload but value has none"
            )));
        }
    }
    Ok(())
}

/// Writes the fields of a record, comma-separated, without its trailing `none` option fields.
fn format_record_fields(
    buf: &mut String,
    fields: &[SchemaValue],
    types: &[NamedFieldType],
    graph: &SchemaGraph,
    depth: usize,
) -> Result<(), StructuralFormatError> {
    let mut present = fields.len();
    while present > 0
        && matches!(fields[present - 1], SchemaValue::Option { inner: None })
        && is_option_type(&types[present - 1].body, graph)?
    {
        present -= 1;
    }
    for (i, (v, t)) in fields[..present].iter().zip(types).enumerate() {
        if i > 0 {
            buf.push(',');
        }
        format_schema_value(buf, v, &t.body, graph, depth)?;
    }
    Ok(())
}

fn is_option_type(typ: &SchemaType, graph: &SchemaGraph) -> Result<bool, StructuralFormatError> {
    Ok(matches!(
        graph
            .resolve_ref(typ)
            .map_err(|e| StructuralFormatError::SchemaMismatch(e.to_string()))?,
        SchemaType::Option { .. }
    ))
}

fn write_escaped_tag(buf: &mut String, tag: &str) {
    for ch in tag.chars() {
        if matches!(ch, ']' | '\\') {
            buf.push('\\');
        }
        buf.push(ch);
    }
}

fn canonical_language_tag(language: &str) -> String {
    language.to_ascii_lowercase()
}

/// Lowercases the type and subtype of a MIME type and keeps its parameters as given.
fn canonical_mime_type(mime_type: &str) -> String {
    match mime_type.split_once(';') {
        Some((essence, parameters)) => format!("{};{parameters}", essence.to_ascii_lowercase()),
        None => mime_type.to_ascii_lowercase(),
    }
}

fn union_branch<'a>(
    spec: &'a UnionSpec,
    tag: &str,
) -> Result<(usize, &'a crate::schema::schema_type::UnionBranch), StructuralFormatError> {
    spec.branches
        .iter()
        .enumerate()
        .find(|(_, b)| b.tag == tag)
        .ok_or_else(|| schema_mismatch(format!("Union branch tag {tag:?} not found")))
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

    // ── Top-level parse methods ─────────────────────────────────────────

    // ── Component model value parsing ───────────────────────────────────

    // ── Unstructured element parsing ────────────────────────────────────

    // ── Schema-native value parsing ──────────────────────────────────────

    fn parse_schema_value(
        &mut self,
        typ: &SchemaType,
        graph: &SchemaGraph,
        depth: usize,
    ) -> Result<SchemaValue, StructuralFormatError> {
        if depth >= MAX_DEPTH {
            return Err(StructuralFormatError::MaxDepthExceeded(MAX_DEPTH));
        }
        let typ = graph
            .resolve_ref(typ)
            .map_err(|e| StructuralFormatError::SchemaMismatch(e.to_string()))?;
        match typ {
            SchemaType::Bool { .. } => {
                if self.eat_str("true") {
                    Ok(SchemaValue::Bool(true))
                } else if self.eat_str("false") {
                    Ok(SchemaValue::Bool(false))
                } else {
                    Err(self.error("Expected 'true' or 'false'"))
                }
            }
            SchemaType::U8 { .. } => Ok(SchemaValue::U8(self.parse_unsigned()?)),
            SchemaType::U16 { .. } => Ok(SchemaValue::U16(self.parse_unsigned()?)),
            SchemaType::U32 { .. } => Ok(SchemaValue::U32(self.parse_unsigned()?)),
            SchemaType::U64 { .. } => Ok(SchemaValue::U64(self.parse_unsigned()?)),
            SchemaType::S8 { .. } => Ok(SchemaValue::S8(self.parse_signed()?)),
            SchemaType::S16 { .. } => Ok(SchemaValue::S16(self.parse_signed()?)),
            SchemaType::S32 { .. } => Ok(SchemaValue::S32(self.parse_signed()?)),
            SchemaType::S64 { .. } => Ok(SchemaValue::S64(self.parse_signed()?)),
            SchemaType::F32 { .. } => Ok(SchemaValue::F32(self.parse_float()?)),
            SchemaType::F64 { .. } => Ok(SchemaValue::F64(self.parse_float()?)),
            SchemaType::Char { .. } => {
                if !self.eat_str("c\"") {
                    return Err(self.error("Expected c\" for char literal"));
                }
                let ch = self.parse_single_json_char()?;
                self.expect('"')?;
                Ok(SchemaValue::Char(ch))
            }
            SchemaType::String { .. } => {
                self.expect('"')?;
                let s = self.parse_json_string_contents()?;
                self.expect('"')?;
                Ok(SchemaValue::String(s))
            }
            SchemaType::Record { fields, .. } => {
                self.expect('(')?;
                let fields = self.parse_record_fields(fields, Some(')'), graph, depth + 1)?;
                self.expect(')')?;
                Ok(SchemaValue::Record { fields })
            }
            SchemaType::Tuple { elements, .. } => {
                self.expect('(')?;
                let elements = self.parse_schema_sequence(elements.iter(), ')', graph, depth)?;
                Ok(SchemaValue::Tuple { elements })
            }
            SchemaType::List { element, .. } => {
                self.expect('[')?;
                let elements = self.parse_homogeneous_list(element, graph, depth)?;
                Ok(SchemaValue::List { elements })
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                self.expect('[')?;
                let elements = self.parse_homogeneous_list(element, graph, depth)?;
                if elements.len() != *length as usize {
                    return Err(self.error(&format!(
                        "FixedList length mismatch: got {}, expected {length}",
                        elements.len()
                    )));
                }
                Ok(SchemaValue::FixedList { elements })
            }
            SchemaType::Map { key, value, .. } => self.parse_schema_map(key, value, graph, depth),
            SchemaType::Variant { cases, .. } => {
                if !self.eat('v') {
                    return Err(self.error("Expected 'v' for variant"));
                }
                let idx = self.parse_nat()?;
                let case = cases.get(idx).ok_or_else(|| {
                    self.error(&format!(
                        "Variant case index {idx} out of range ({} cases)",
                        cases.len()
                    ))
                })?;
                let payload = if self.eat('(') {
                    let t = case.payload.as_ref().ok_or_else(|| {
                        self.error("Variant case has no payload type but got '('")
                    })?;
                    let v = self.parse_schema_value(t, graph, depth + 1)?;
                    self.expect(')')?;
                    Some(Box::new(v))
                } else if case.payload.is_some() {
                    return Err(self.error("Variant case requires payload but none provided"));
                } else {
                    None
                };
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: idx as u32,
                    payload,
                }))
            }
            SchemaType::Enum { cases, .. } => {
                if !self.eat('v') {
                    return Err(self.error("Expected 'v' for enum"));
                }
                let case = self.parse_nat()?;
                if case >= cases.len() {
                    return Err(self.error(&format!(
                        "Enum case index {case} out of range ({} cases)",
                        cases.len()
                    )));
                }
                Ok(SchemaValue::Enum { case: case as u32 })
            }
            SchemaType::Flags { flags, .. } => self.parse_schema_flags(flags.len()),
            SchemaType::Option { inner, .. } => {
                if self.eat_str("s(") {
                    let v = self.parse_schema_value(inner, graph, depth + 1)?;
                    self.expect(')')?;
                    Ok(SchemaValue::Option {
                        inner: Some(Box::new(v)),
                    })
                } else if self.eat('n') {
                    Ok(SchemaValue::Option { inner: None })
                } else {
                    Err(self.error("Expected 's(' or 'n' for option"))
                }
            }
            SchemaType::Result { spec, .. } => {
                self.parse_schema_result(spec.ok.as_deref(), spec.err.as_deref(), graph, depth)
            }
            SchemaType::Text { .. } => self.parse_schema_text(),
            SchemaType::Binary { .. } => self.parse_schema_binary(),
            SchemaType::Path { .. } => Ok(SchemaValue::Path {
                path: self.parse_tagged_string("p")?,
            }),
            SchemaType::Url { .. } => Ok(SchemaValue::Url {
                url: self.parse_tagged_string("u")?,
            }),
            SchemaType::Uuid { .. } => Ok(SchemaValue::Uuid(
                canonical_uuid::from_text(&self.parse_tagged_string("uuid")?)
                    .map_err(|e| self.error(&format!("Invalid UUID: {e}")))?,
            )),
            SchemaType::Datetime { .. } => Ok(SchemaValue::Datetime {
                value: datetime::from_text(&self.parse_tagged_string("dt")?)
                    .map_err(|e| self.error(&format!("Invalid datetime: {e}")))?,
            }),
            SchemaType::Duration { .. } => Ok(SchemaValue::Duration(
                duration::from_text(&self.parse_tagged_string("dur")?)
                    .map_err(|e| self.error(&format!("Invalid duration: {e}")))?,
            )),
            SchemaType::Quantity { .. } => Ok(SchemaValue::Quantity(
                quantity::from_text(&self.parse_tagged_string("qty")?)
                    .map_err(|e| self.error(&format!("Invalid quantity: {e}")))?,
            )),
            SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::PermissionCard { .. } => {
                Err(StructuralFormatError::HostManagedCapability)
            }
            SchemaType::Union { spec, .. } => self.parse_schema_union(spec, graph, depth),
            SchemaType::Future { .. } | SchemaType::Stream { .. } => {
                Err(StructuralFormatError::HandleType)
            }
            SchemaType::Ref { .. } => unreachable!("resolved above"),
        }
    }

    /// Parses comma-separated record fields up to `terminator`, or to the end of the
    /// input when it is `None`, without consuming it. Missing trailing fields are
    /// accepted only when they are options, and become `none`.
    fn parse_record_fields(
        &mut self,
        types: &[NamedFieldType],
        terminator: Option<char>,
        graph: &SchemaGraph,
        field_depth: usize,
    ) -> Result<Vec<SchemaValue>, StructuralFormatError> {
        let mut values = Vec::with_capacity(types.len());
        for (i, t) in types.iter().enumerate() {
            if self.peek() == terminator {
                break;
            }
            if i > 0 {
                self.expect(',')?;
            }
            values.push(self.parse_schema_value(&t.body, graph, field_depth)?);
        }
        for t in &types[values.len()..] {
            if !is_option_type(&t.body, graph)? {
                return Err(self.error(&format!(
                    "Missing record field {:?}; only trailing option fields may be omitted",
                    t.name
                )));
            }
            values.push(SchemaValue::Option { inner: None });
        }
        Ok(values)
    }

    fn parse_schema_sequence<'b, I>(
        &mut self,
        types: I,
        terminator: char,
        graph: &SchemaGraph,
        depth: usize,
    ) -> Result<Vec<SchemaValue>, StructuralFormatError>
    where
        I: IntoIterator<Item = &'b SchemaType>,
    {
        let types: Vec<&SchemaType> = types.into_iter().collect();
        let mut values = Vec::with_capacity(types.len());
        for (i, t) in types.iter().enumerate() {
            if i > 0 {
                self.expect(',')?;
            }
            values.push(self.parse_schema_value(t, graph, depth + 1)?);
        }
        self.expect(terminator)?;
        Ok(values)
    }

    fn parse_homogeneous_list(
        &mut self,
        element: &SchemaType,
        graph: &SchemaGraph,
        depth: usize,
    ) -> Result<Vec<SchemaValue>, StructuralFormatError> {
        let mut elements = Vec::new();
        if !self.eat(']') {
            loop {
                elements.push(self.parse_schema_value(element, graph, depth + 1)?);
                if !self.eat(',') {
                    break;
                }
            }
            self.expect(']')?;
        }
        Ok(elements)
    }

    fn parse_schema_map(
        &mut self,
        key: &SchemaType,
        value: &SchemaType,
        graph: &SchemaGraph,
        depth: usize,
    ) -> Result<SchemaValue, StructuralFormatError> {
        if !self.eat_str("m[") {
            return Err(self.error("Expected 'm[' for map"));
        }
        let mut entries = Vec::new();
        if !self.eat(']') {
            loop {
                self.expect('(')?;
                let k = self.parse_schema_value(key, graph, depth + 1)?;
                self.expect(',')?;
                let v = self.parse_schema_value(value, graph, depth + 1)?;
                self.expect(')')?;
                entries.push((k, v));
                if !self.eat(',') {
                    break;
                }
            }
            self.expect(']')?;
        }
        let mut keyed = Vec::with_capacity(entries.len());
        for (k, v) in entries {
            let mut key_text = String::new();
            format_schema_value(&mut key_text, &k, key, graph, depth + 1)?;
            keyed.push((key_text, (k, v)));
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(duplicate) = keyed.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(StructuralFormatError::DuplicateMapKey(
                duplicate[0].0.clone(),
            ));
        }
        Ok(SchemaValue::Map {
            entries: keyed.into_iter().map(|(_, entry)| entry).collect(),
        })
    }

    fn parse_schema_flags(&mut self, count: usize) -> Result<SchemaValue, StructuralFormatError> {
        if !self.eat_str("f(") {
            return Err(self.error("Expected 'f(' for flags"));
        }
        let mut bits = vec![false; count];
        if !self.eat(')') {
            let mut prev = None;
            loop {
                let idx = self.parse_nat()?;
                if idx >= count {
                    return Err(
                        self.error(&format!("Flag index {idx} out of range ({count} flags)"))
                    );
                }
                if let Some(p) = prev
                    && idx <= p
                {
                    return Err(self.error("Flag indices must be strictly increasing"));
                }
                bits[idx] = true;
                prev = Some(idx);
                if !self.eat(',') {
                    break;
                }
            }
            self.expect(')')?;
        }
        Ok(SchemaValue::Flags { bits })
    }

    fn parse_schema_result(
        &mut self,
        ok: Option<&SchemaType>,
        err: Option<&SchemaType>,
        graph: &SchemaGraph,
        depth: usize,
    ) -> Result<SchemaValue, StructuralFormatError> {
        if self.eat_str("ok") {
            let value = if self.eat('(') {
                let t = ok.ok_or_else(|| self.error("Result ok type is unit but got '('"))?;
                let v = self.parse_schema_value(t, graph, depth + 1)?;
                self.expect(')')?;
                Some(Box::new(v))
            } else if ok.is_some() {
                return Err(self.error("Result ok type requires payload but got bare 'ok'"));
            } else {
                None
            };
            Ok(SchemaValue::Result(ResultValuePayload::Ok { value }))
        } else if self.eat_str("err") {
            let value = if self.eat('(') {
                let t = err.ok_or_else(|| self.error("Result err type is unit but got '('"))?;
                let v = self.parse_schema_value(t, graph, depth + 1)?;
                self.expect(')')?;
                Some(Box::new(v))
            } else if err.is_some() {
                return Err(self.error("Result err type requires payload but got bare 'err'"));
            } else {
                None
            };
            Ok(SchemaValue::Result(ResultValuePayload::Err { value }))
        } else {
            Err(self.error("Expected 'ok' or 'err' for result"))
        }
    }

    fn parse_schema_text(&mut self) -> Result<SchemaValue, StructuralFormatError> {
        if !self.eat_str("@t") {
            return Err(self.error("Expected '@t' for text"));
        }
        let language = if self.eat('[') {
            Some(self.parse_bracket_content()?)
                .filter(|l| !l.is_empty())
                .map(|l| canonical_language_tag(&l))
        } else {
            None
        };
        self.expect('"')?;
        let text = self.parse_json_string_contents()?;
        self.expect('"')?;
        Ok(SchemaValue::Text(TextValuePayload { text, language }))
    }

    fn parse_schema_binary(&mut self) -> Result<SchemaValue, StructuralFormatError> {
        if !self.eat_str("@b[") {
            return Err(self.error("Expected '@b[' for binary"));
        }
        let mime = self.parse_bracket_content()?;
        self.expect('"')?;
        let s = self.parse_json_string_contents()?;
        self.expect('"')?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(|e| self.error(&format!("Invalid base64: {e}")))?;
        Ok(SchemaValue::Binary(BinaryValuePayload {
            bytes,
            mime_type: if mime.is_empty() {
                None
            } else {
                Some(canonical_mime_type(&mime))
            },
        }))
    }

    fn parse_tagged_string(&mut self, tag: &str) -> Result<String, StructuralFormatError> {
        if !self.eat('@') {
            return Err(self.error(&format!("Expected '@{tag}'")));
        }
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_lowercase() {
                self.advance();
            } else {
                break;
            }
        }
        let actual = &self.input[start..self.pos];
        if actual != tag {
            return Err(self.error(&format!("Expected tag '@{tag}', got '@{actual}'")));
        }
        self.expect('"')?;
        let s = self.parse_json_string_contents()?;
        self.expect('"')?;
        Ok(s)
    }

    fn parse_schema_union(
        &mut self,
        spec: &UnionSpec,
        graph: &SchemaGraph,
        depth: usize,
    ) -> Result<SchemaValue, StructuralFormatError> {
        if !self.eat('u') {
            return Err(self.error("Expected 'u' for union"));
        }
        let idx = self.parse_nat()?;
        let branch = spec.branches.get(idx).ok_or_else(|| {
            self.error(&format!(
                "Union case index {idx} out of range ({} branches)",
                spec.branches.len()
            ))
        })?;
        let body = if self.eat('(') {
            let v = self.parse_schema_value(&branch.body, graph, depth + 1)?;
            self.expect(')')?;
            v
        } else {
            let mut empty = Parser::new("()");
            empty
                .parse_schema_value(&branch.body, graph, depth + 1)
                .map_err(|e| self.error(&format!("Union branch {idx} requires a body: {e}")))?
        };
        Ok(SchemaValue::Union(UnionValuePayload {
            tag: branch.tag.clone(),
            body: Box::new(body),
        }))
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
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some(c @ (']' | '\\')) => {
                            content.push(c);
                            self.advance();
                        }
                        other => {
                            return Err(self.error(&format!("Invalid escape in tag: \\{other:?}")));
                        }
                    }
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

#[cfg(test)]
mod tests;
