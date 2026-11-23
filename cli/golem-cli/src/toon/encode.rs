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

use serde_json::Value;
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

/// A value for the encoder. It is a JSON value, but an object keeps the order of its members, and
/// a number is the text that the encoder writes.
///
/// `From<&serde_json::Value>` gives each number its canonical form (§2). A caller that has the
/// exact text of a number, which a `serde_json::Value` cannot always hold, builds the value itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToonValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<ToonValue>),
    Object(Vec<(String, ToonValue)>),
}

impl From<&Value> for ToonValue {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(value) => Self::Bool(*value),
            Value::Number(number) => Self::Number(canonical_number(number)),
            Value::String(string) => Self::String(string.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from).collect()),
            Value::Object(members) => Self::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from(value)))
                    .collect(),
            ),
        }
    }
}

/// Encode a JSON value to TOON.
pub fn encode_value(value: &Value) -> Result<String, ToonEncodeError> {
    encode_toon_value(&ToonValue::from(value))
}

/// Encode a [`ToonValue`] to TOON.
pub fn encode_toon_value(value: &ToonValue) -> Result<String, ToonEncodeError> {
    let mut out = String::new();
    match value {
        // An empty object at the root yields an empty document
        ToonValue::Object(map) => write_object_fields(&mut out, map, 0, 0)?,
        ToonValue::Array(arr) => {
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
    map: &[(String, ToonValue)],
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
    value: &ToonValue,
    indent: usize,
    depth: usize,
) -> Result<(), ToonEncodeError> {
    match value {
        ToonValue::Object(map) => {
            write_key(out, key);
            out.push(':');
            if !map.is_empty() {
                out.push('\n');
                write_object_fields(out, map, indent + 1, depth + 1)?;
            }
        }
        ToonValue::Array(arr) => {
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
    arr: &[ToonValue],
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
            let ToonValue::Object(row) = row else {
                unreachable!("tabular rows are objects")
            };
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_primitive(
                    out,
                    member(row, field).expect("tabular rows have all fields"),
                );
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
    item: &ToonValue,
    indent: usize,
    depth: usize,
) -> Result<(), ToonEncodeError> {
    check_depth(depth)?;
    match item {
        ToonValue::Object(map) if map.is_empty() => {
            // Empty object list item: a single "-"
            out.push('-');
        }
        ToonValue::Object(map) => {
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
        ToonValue::Array(arr) => {
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
fn tabular_fields(arr: &[ToonValue]) -> Option<Vec<&str>> {
    let ToonValue::Object(first) = arr.first()? else {
        return None;
    };
    if first.is_empty() {
        return None;
    }
    let fields: Vec<&str> = first.iter().map(|(key, _)| key.as_str()).collect();
    // A member name that an object holds more than once has no single column.
    if fields
        .iter()
        .enumerate()
        .any(|(i, field)| fields[..i].contains(field))
    {
        return None;
    }

    for value in arr {
        let ToonValue::Object(object) = value else {
            return None;
        };
        if object.len() != fields.len()
            || !fields.iter().all(|field| member(object, field).is_some())
            || !object.iter().all(|(_, value)| is_primitive(value))
        {
            return None;
        }
    }
    Some(fields)
}

/// The value of the first member with this name.
fn member<'a>(object: &'a [(String, ToonValue)], name: &str) -> Option<&'a ToonValue> {
    object
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn is_primitive(value: &ToonValue) -> bool {
    !matches!(value, ToonValue::Array(_) | ToonValue::Object(_))
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

fn write_primitive(out: &mut String, value: &ToonValue) {
    match value {
        ToonValue::Null => out.push_str("null"),
        ToonValue::Bool(true) => out.push_str("true"),
        ToonValue::Bool(false) => out.push_str("false"),
        ToonValue::Number(number) => out.push_str(number),
        ToonValue::String(string) => {
            if needs_quoting(string) {
                write_quoted(out, string);
            } else {
                out.push_str(string);
            }
        }
        ToonValue::Array(_) | ToonValue::Object(_) => {
            unreachable!("write_primitive called with a non-primitive value")
        }
    }
}

/// Canonical number formatting (§2): plain decimal form without exponent for
/// zero and `1e-6 <= |n| < 1e21`, lowercase exponent notation otherwise.
fn canonical_number(number: &serde_json::Number) -> String {
    if let Some(i) = number.as_i64() {
        i.to_string()
    } else if let Some(u) = number.as_u64() {
        u.to_string()
    } else {
        let f = number.as_f64().expect("serde_json numbers are i64/u64/f64");
        if f == 0.0 {
            // Covers -0.0, which must be normalized to 0
            "0".to_string()
        } else {
            let abs = f.abs();
            if (1e-6..1e21).contains(&abs) {
                // `Display` never emits exponents, drops the trailing `.0` of
                // integral floats and emits the shortest round-tripping digits
                format!("{f}")
            } else {
                format!("{f:e}")
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
