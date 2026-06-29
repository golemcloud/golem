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

//! TOON encoder (spec v3.3, default options: comma delimiter, 2-space indent,
//! no key folding).

use serde_json::{Map, Value};
use std::fmt;

/// Maximum nesting depth, to protect against stack overflow (also on 2 MiB
/// tokio worker thread stacks in debug builds). Far above any realistic
/// value: JSON payloads parsed with serde_json are limited to a nesting depth
/// of 128 by its recursion limit.
const MAX_DEPTH: usize = 1024;

const INDENT: &str = "  ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToonEncodeError {
    MaxDepthExceeded,
    Serialization(String),
}

impl fmt::Display for ToonEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToonEncodeError::MaxDepthExceeded => {
                write!(
                    f,
                    "the value is too deeply nested (more than {MAX_DEPTH} levels) to be rendered as TOON, use another format (e.g. --format json) instead"
                )
            }
            ToonEncodeError::Serialization(error) => {
                write!(f, "TOON encoding failed: {error}")
            }
        }
    }
}

impl std::error::Error for ToonEncodeError {}

/// Encode any serializable value to TOON.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<String, ToonEncodeError> {
    let value = serde_json::to_value(value)
        .map_err(|err| ToonEncodeError::Serialization(err.to_string()))?;
    encode_value(&value)
}

/// Encode a JSON value to TOON.
pub fn encode_value(value: &Value) -> Result<String, ToonEncodeError> {
    let mut out = String::new();
    match value {
        // An empty object at the root yields an empty document
        Value::Object(map) => write_object_fields(&mut out, map, 0, 0)?,
        Value::Array(arr) => {
            if arr.is_empty() {
                // Empty root array
                out.push_str("[]");
            } else {
                write_array(&mut out, None, arr, 0, 0, true)?;
            }
        }
        primitive => write_primitive(&mut out, primitive),
    }
    Ok(out)
}

fn check_depth(depth: usize) -> Result<(), ToonEncodeError> {
    if depth > MAX_DEPTH {
        Err(ToonEncodeError::MaxDepthExceeded)
    } else {
        Ok(())
    }
}

fn write_indent(out: &mut String, levels: usize) {
    for _ in 0..levels {
        out.push_str(INDENT);
    }
}

/// Writes the fields of an object, one line per field, each line at `indent`.
fn write_object_fields(
    out: &mut String,
    map: &Map<String, Value>,
    indent: usize,
    depth: usize,
) -> Result<(), ToonEncodeError> {
    check_depth(depth)?;
    for (i, (key, value)) in map.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        write_indent(out, indent);
        write_field(out, key, value, indent, depth)?;
    }
    Ok(())
}

/// Writes a single `key…` field of an object whose fields are at `indent` and
/// whose nesting depth is `depth`. The cursor is already at the indentation
/// position (or after a `- ` list item marker).
fn write_field(
    out: &mut String,
    key: &str,
    value: &Value,
    indent: usize,
    depth: usize,
) -> Result<(), ToonEncodeError> {
    match value {
        Value::Object(map) => {
            write_key(out, key);
            out.push(':');
            if !map.is_empty() {
                out.push('\n');
                write_object_fields(out, map, indent + 1, depth + 1)?;
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                // Empty array in object field position
                write_key(out, key);
                out.push_str(": []");
            } else {
                write_array(out, Some(key), arr, indent, depth + 1, true)?;
            }
        }
        primitive => {
            write_key(out, key);
            out.push_str(": ");
            write_primitive(out, primitive);
        }
    }
    Ok(())
}

/// Writes a non-empty array. The header is written at the current cursor
/// position; rows / list items are written at `indent + 1`.
///
/// `tabular_allowed` is false when the array itself is a list item, where the
/// spec mandates the expanded list form (§9.4).
fn write_array(
    out: &mut String,
    key: Option<&str>,
    arr: &[Value],
    indent: usize,
    depth: usize,
    tabular_allowed: bool,
) -> Result<(), ToonEncodeError> {
    check_depth(depth)?;

    if tabular_allowed && let Some(fields) = tabular_fields(arr) {
        // Tabular form: key[N]{f1,f2}: followed by one row per element
        write_array_header(out, key, arr.len(), Some(&fields));
        for row in arr {
            out.push('\n');
            write_indent(out, indent + 1);
            let row = row.as_object().expect("tabular rows are objects");
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_primitive(out, row.get(*field).expect("tabular rows have all fields"));
            }
        }
    } else if arr.iter().all(is_primitive) {
        // Inline form: key[N]: v1,v2,…
        write_array_header(out, key, arr.len(), None);
        out.push(' ');
        for (i, value) in arr.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_primitive(out, value);
        }
    } else {
        // Expanded list form: key[N]: followed by one `- ` item per element
        write_array_header(out, key, arr.len(), None);
        for item in arr {
            out.push('\n');
            write_indent(out, indent + 1);
            write_list_item(out, item, indent + 1, depth + 1)?;
        }
    }
    Ok(())
}

/// Writes a single list item. The cursor is at the indentation position of the
/// hyphen line (`indent`); fields of object items are at `indent + 1`.
fn write_list_item(
    out: &mut String,
    item: &Value,
    indent: usize,
    depth: usize,
) -> Result<(), ToonEncodeError> {
    check_depth(depth)?;
    match item {
        Value::Object(map) if map.is_empty() => {
            // Empty object list item: a single "-"
            out.push('-');
        }
        Value::Object(map) => {
            // First field on the hyphen line, remaining fields at indent + 1
            out.push_str("- ");
            for (i, (key, value)) in map.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    write_indent(out, indent + 1);
                }
                write_field(out, key, value, indent + 1, depth)?;
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                // Empty inner array list item
                out.push_str("- [0]:");
            } else {
                out.push_str("- ");
                write_array(out, None, arr, indent, depth, false)?;
            }
        }
        primitive => {
            out.push_str("- ");
            write_primitive(out, primitive);
        }
    }
    Ok(())
}

/// Returns the tabular field list when the array qualifies for tabular form
/// (§9.3): all elements are objects with at least one key, the same key set,
/// and primitive-only values.
fn tabular_fields(arr: &[Value]) -> Option<Vec<&str>> {
    let first = arr.first()?.as_object()?;
    if first.is_empty() {
        return None;
    }
    let fields: Vec<&str> = first.keys().map(String::as_str).collect();

    for value in arr {
        let object = value.as_object()?;
        if object.len() != fields.len()
            || !fields.iter().all(|field| object.contains_key(*field))
            || !object.values().all(is_primitive)
        {
            return None;
        }
    }
    Some(fields)
}

fn is_primitive(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
}

/// Writes `key?[N]{fields?}:`.
fn write_array_header(out: &mut String, key: Option<&str>, len: usize, fields: Option<&[&str]>) {
    if let Some(key) = key {
        write_key(out, key);
    }
    out.push('[');
    out.push_str(&len.to_string());
    out.push(']');
    if let Some(fields) = fields {
        out.push('{');
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_key(out, field);
        }
        out.push('}');
    }
    out.push(':');
}

fn write_primitive(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(out, number),
        Value::String(string) => {
            if needs_quoting(string) {
                write_quoted(out, string);
            } else {
                out.push_str(string);
            }
        }
        Value::Array(_) | Value::Object(_) => {
            unreachable!("write_primitive called with a non-primitive value")
        }
    }
}

/// Canonical number formatting (§2): plain decimal form without exponent for
/// zero and `1e-6 <= |n| < 1e21`, lowercase exponent notation otherwise.
fn write_number(out: &mut String, number: &serde_json::Number) {
    if let Some(i) = number.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = number.as_u64() {
        out.push_str(&u.to_string());
    } else {
        let f = number.as_f64().expect("serde_json numbers are i64/u64/f64");
        if f == 0.0 {
            // Covers -0.0, which must be normalized to 0
            out.push('0');
        } else {
            let abs = f.abs();
            if (1e-6..1e21).contains(&abs) {
                // `Display` never emits exponents, drops the trailing `.0` of
                // integral floats and emits the shortest round-tripping digits
                out.push_str(&format!("{f}"));
            } else {
                out.push_str(&format!("{f:e}"));
            }
        }
    }
}

/// Quoting rules for string values (§7.2), specialized to the comma delimiter.
fn needs_quoting(s: &str) -> bool {
    if s.is_empty() || s == "true" || s == "false" || s == "null" {
        return true;
    }
    if s.starts_with(char::is_whitespace) || s.ends_with(char::is_whitespace) {
        return true;
    }
    if s.starts_with('-') || is_numeric_like(s) {
        return true;
    }
    s.chars()
        .any(|c| matches!(c, ':' | '"' | '\\' | '[' | ']' | '{' | '}' | ',') || (c as u32) < 0x20)
}

/// Matches `/^-?\d+(?:\.\d+)?(?:e[+-]?\d+)?$/i` (§7.2).
fn is_numeric_like(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = usize::from(bytes.first() == Some(&b'-'));

    let int_digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    if int_digits == 0 {
        return false;
    }
    i += int_digits;

    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let frac_digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if frac_digits == 0 {
            return false;
        }
        i += frac_digits;
    }

    if matches!(bytes.get(i), Some(b'e') | Some(b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+') | Some(b'-')) {
            i += 1;
        }
        let exp_digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exp_digits == 0 {
            return false;
        }
        i += exp_digits;
    }

    i == bytes.len()
}

/// Keys are unquoted only if they match `^[A-Za-z_][A-Za-z0-9_.]*$` (§7.3).
fn write_key(out: &mut String, key: &str) {
    if is_valid_unquoted_key(key) {
        out.push_str(key);
    } else {
        write_quoted(out, key);
    }
}

fn is_valid_unquoted_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Escaping inside quoted strings and keys (§7.1).
fn write_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
