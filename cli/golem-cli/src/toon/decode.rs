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

//! Strict, test-only TOON decoder (spec v3.3, comma delimiter only).
//!
//! Used as the oracle for round-trip property tests of the encoder. It
//! enforces strict mode rules (exact indentation, array counts, escape
//! sequences and duplicate keys) so that malformed encoder output fails
//! loudly.

use serde_json::{Map, Number, Value};

/// Decode a TOON document into a JSON value.
pub fn decode(input: &str) -> Result<Value, String> {
    let lines = parse_lines(input)?;
    let mut parser = Parser { lines, pos: 0 };

    let Some(first) = parser.peek() else {
        // Empty document decodes to an empty object
        return Ok(Value::Object(Map::new()));
    };
    if first.indent != 0 {
        return Err("first line must not be indented".to_string());
    }

    let value = if parser.lines.len() == 1 && first.content == "[]" {
        parser.next();
        Value::Array(Vec::new())
    } else if first.content.starts_with('[') {
        // Root array header
        let line = parser.next().unwrap();
        let (len, fields, after) = parse_array_header(line.content)?;
        parse_array_body(&mut parser, len, fields, after, 0)?
    } else if parser.lines.len() == 1 && !is_field_line(first.content) {
        // Single primitive document
        let line = parser.next().unwrap();
        parse_scalar(line.content)?
    } else {
        Value::Object(parse_object_fields(&mut parser, 0)?)
    };

    match parser.peek() {
        None => Ok(value),
        Some(line) => Err(format!("unexpected trailing line: {:?}", line.content)),
    }
}

#[derive(Clone, Copy)]
struct Line<'a> {
    indent: usize,
    content: &'a str,
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<Line<'a>> {
        self.lines.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<Line<'a>> {
        let line = self.peek();
        if line.is_some() {
            self.pos += 1;
        }
        line
    }
}

fn parse_lines(input: &str) -> Result<Vec<Line<'_>>, String> {
    let mut lines = Vec::new();
    let mut blank_seen = false;
    for raw in input.split('\n') {
        if raw.trim().is_empty() {
            // Strict mode: blank lines are only tolerated at the end of the
            // document (trailing newline)
            blank_seen = true;
            continue;
        }
        if blank_seen {
            return Err("blank line inside document".to_string());
        }
        let spaces = raw.len() - raw.trim_start_matches(' ').len();
        let content = &raw[spaces..];
        if content.starts_with('\t') {
            return Err("tab used for indentation".to_string());
        }
        if content.ends_with(' ') {
            return Err(format!("trailing whitespace on line: {raw:?}"));
        }
        if !spaces.is_multiple_of(2) {
            return Err(format!("indentation is not a multiple of 2: {raw:?}"));
        }
        lines.push(Line {
            indent: spaces / 2,
            content,
        });
    }
    Ok(lines)
}

/// Whether a depth-0 line is a key-value line or array header line (used for
/// root form discovery, §5).
fn is_field_line(content: &str) -> bool {
    match parse_key(content) {
        Ok((_, rest)) => rest.starts_with(':') || rest.starts_with('['),
        Err(_) => false,
    }
}

fn parse_object_fields(parser: &mut Parser, indent: usize) -> Result<Map<String, Value>, String> {
    let mut map = Map::new();
    while let Some(line) = parser.peek() {
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(format!("unexpected indentation: {:?}", line.content));
        }
        if line.content.starts_with('-') {
            return Err(format!("unexpected list item: {:?}", line.content));
        }
        parser.next();
        let (key, rest) = parse_key(line.content)?;
        let value = parse_field_value(parser, rest, indent)?;
        if map.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate key: {key:?}"));
        }
    }
    Ok(map)
}

/// Parses the value of a field given the remainder of the line after the key
/// (starting with `:` or `[`). Nested content lines are at `indent + 1`.
fn parse_field_value(parser: &mut Parser, rest: &str, indent: usize) -> Result<Value, String> {
    if rest.starts_with('[') {
        let (len, fields, after) = parse_array_header(rest)?;
        return parse_array_body(parser, len, fields, after, indent);
    }

    let Some(rest) = rest.strip_prefix(':') else {
        return Err(format!("expected ':' or '[' after key, got: {rest:?}"));
    };

    if rest.is_empty() {
        // Nested or empty object
        return Ok(Value::Object(parse_object_fields(parser, indent + 1)?));
    }

    let Some(token) = rest.strip_prefix(' ') else {
        return Err(format!("expected a space after ':', got: {rest:?}"));
    };
    if token == "[]" {
        return Ok(Value::Array(Vec::new()));
    }
    parse_scalar(token)
}

/// Parses `[N]`, optional `{fields}` and the `:`; returns the length, the
/// optional tabular field names and the remainder of the line after the `:`.
fn parse_array_header(rest: &str) -> Result<(usize, Option<Vec<String>>, &str), String> {
    let rest = rest
        .strip_prefix('[')
        .expect("array header starts with '['");
    let Some((len_str, rest)) = rest.split_once(']') else {
        return Err("unterminated array length bracket".to_string());
    };
    if len_str.is_empty()
        || !len_str.bytes().all(|b| b.is_ascii_digit())
        || (len_str.starts_with('0') && len_str != "0")
    {
        return Err(format!("invalid array length: {len_str:?}"));
    }
    let len: usize = len_str
        .parse()
        .map_err(|_| format!("invalid array length: {len_str:?}"))?;

    let (fields, rest) = if let Some(fields_rest) = rest.strip_prefix('{') {
        let Some((fields_str, rest)) = split_once_unquoted(fields_rest, '}') else {
            return Err("unterminated tabular field list".to_string());
        };
        let mut fields = Vec::new();
        for raw_field in split_cells(fields_str)? {
            fields.push(parse_field_name(&raw_field)?);
        }
        if fields.is_empty() {
            return Err("empty tabular field list".to_string());
        }
        (Some(fields), rest)
    } else {
        (None, rest)
    };

    let Some(rest) = rest.strip_prefix(':') else {
        return Err(format!("expected ':' after array header, got: {rest:?}"));
    };
    Ok((len, fields, rest))
}

/// Parses the body of an array given its parsed header. `after` is the
/// remainder of the header line after the `:`. The header line is at `indent`.
fn parse_array_body(
    parser: &mut Parser,
    len: usize,
    fields: Option<Vec<String>>,
    after: &str,
    indent: usize,
) -> Result<Value, String> {
    if let Some(fields) = fields {
        // Tabular form: rows at indent + 1
        if !after.is_empty() {
            return Err(format!(
                "unexpected content after tabular header: {after:?}"
            ));
        }
        let mut rows = Vec::with_capacity(len);
        for _ in 0..len {
            let Some(line) = parser.peek().filter(|line| line.indent == indent + 1) else {
                return Err(format!("expected {len} tabular rows"));
            };
            parser.next();
            let cells = split_cells(line.content)?;
            if cells.len() != fields.len() {
                return Err(format!(
                    "tabular row has {} cells, expected {}: {:?}",
                    cells.len(),
                    fields.len(),
                    line.content
                ));
            }
            let mut row = Map::new();
            for (field, cell) in fields.iter().zip(cells) {
                if row.insert(field.clone(), parse_scalar(&cell)?).is_some() {
                    return Err(format!("duplicate tabular field: {field:?}"));
                }
            }
            rows.push(Value::Object(row));
        }
        return Ok(Value::Array(rows));
    }

    if let Some(inline) = after.strip_prefix(' ') {
        // Inline primitive array
        let cells = split_cells(inline)?;
        if cells.len() != len {
            return Err(format!(
                "inline array has {} values, expected {len}: {inline:?}",
                cells.len()
            ));
        }
        let values = cells
            .iter()
            .map(|cell| parse_scalar(cell))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Value::Array(values));
    }

    if !after.is_empty() {
        return Err(format!("unexpected content after array header: {after:?}"));
    }
    if len == 0 {
        // Legacy `key[0]:` empty array form
        return Ok(Value::Array(Vec::new()));
    }

    // Expanded list form: items at indent + 1
    let mut items = Vec::with_capacity(len);
    for _ in 0..len {
        let Some(line) = parser
            .peek()
            .filter(|line| line.indent == indent + 1 && line.content.starts_with('-'))
        else {
            return Err(format!("expected {len} list items"));
        };
        parser.next();
        items.push(parse_list_item(parser, line.content, indent + 1)?);
    }
    Ok(Value::Array(items))
}

/// Parses a single list item line (starting with `-`) at `indent`.
fn parse_list_item(parser: &mut Parser, content: &str, indent: usize) -> Result<Value, String> {
    if content == "-" {
        // Empty object list item
        return Ok(Value::Object(Map::new()));
    }
    let Some(rest) = content.strip_prefix("- ") else {
        return Err(format!("invalid list item: {content:?}"));
    };

    if rest.starts_with('[') {
        // Inner array list item; nested items are at indent + 1 (§9.4) and
        // the tabular form is not available in this position
        let (len, fields, after) = parse_array_header(rest)?;
        if fields.is_some() {
            return Err(format!(
                "tabular form is not allowed as a list item: {rest:?}"
            ));
        }
        return parse_array_body(parser, len, None, after, indent);
    }

    if let Ok((key, key_rest)) = parse_key(rest)
        && (key_rest.starts_with(':') || key_rest.starts_with('['))
    {
        // Object list item: first field on the hyphen line (logically at
        // indent + 1), remaining fields at indent + 1
        let mut map = Map::new();
        let value = parse_field_value(parser, key_rest, indent + 1)?;
        map.insert(key, value);
        while let Some(line) = parser.peek() {
            if line.indent != indent + 1 || line.content.starts_with('-') {
                break;
            }
            parser.next();
            let (key, key_rest) = parse_key(line.content)?;
            let value = parse_field_value(parser, key_rest, indent + 1)?;
            if map.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate key: {key:?}"));
            }
        }
        return Ok(Value::Object(map));
    }

    // Primitive list item
    parse_scalar(rest)
}

/// Parses a quoted or unquoted key; returns the key and the remainder of the
/// line (which must start with `:` or `[` for valid field lines).
fn parse_key(content: &str) -> Result<(String, &str), String> {
    if content.starts_with('"') {
        let (key, rest) = parse_quoted(content)?;
        return Ok((key, rest));
    }
    let end = content
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(content.len());
    let key = &content[..end];
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return Err(format!("invalid key: {content:?}")),
    }
    Ok((key.to_string(), &content[end..]))
}

/// Parses a tabular field name, which is a quoted or unquoted key (§6).
fn parse_field_name(token: &str) -> Result<String, String> {
    let (name, rest) = parse_key(token)?;
    if !rest.is_empty() {
        return Err(format!("invalid tabular field name: {token:?}"));
    }
    Ok(name)
}

/// Parses a single scalar token: quoted string, keyword, number or plain
/// string.
fn parse_scalar(token: &str) -> Result<Value, String> {
    if token.starts_with('"') {
        let (value, rest) = parse_quoted(token)?;
        if !rest.is_empty() {
            return Err(format!("unexpected content after quoted string: {token:?}"));
        }
        return Ok(Value::String(value));
    }
    if token.contains('"') {
        return Err(format!("unquoted token contains a quote: {token:?}"));
    }
    match token {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "null" => return Ok(Value::Null),
        _ => {}
    }
    if let Some(number) = parse_number(token)? {
        return Ok(Value::Number(number));
    }
    Ok(Value::String(token.to_string()))
}

/// Parses a token as a JSON-grammar number. Tokens with forbidden leading
/// zeros in the integer part are strings, not numbers (§4).
fn parse_number(token: &str) -> Result<Option<Number>, String> {
    let bytes = token.as_bytes();
    let mut i = usize::from(bytes.first() == Some(&b'-'));

    let int_start = i;
    while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
        i += 1;
    }
    let int_digits = &token[int_start..i];
    if int_digits.is_empty() || (int_digits.starts_with('0') && int_digits.len() > 1) {
        return Ok(None);
    }

    let mut is_integral = true;
    if bytes.get(i) == Some(&b'.') {
        is_integral = false;
        i += 1;
        let frac_start = i;
        while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
            i += 1;
        }
        if i == frac_start {
            return Ok(None);
        }
    }
    if matches!(bytes.get(i), Some(b'e') | Some(b'E')) {
        is_integral = false;
        i += 1;
        if matches!(bytes.get(i), Some(b'+') | Some(b'-')) {
            i += 1;
        }
        let exp_start = i;
        while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
            i += 1;
        }
        if i == exp_start {
            return Ok(None);
        }
    }
    if i != bytes.len() {
        return Ok(None);
    }

    if is_integral {
        if let Ok(i) = token.parse::<i64>() {
            return Ok(Some(Number::from(i)));
        }
        if let Ok(u) = token.parse::<u64>() {
            return Ok(Some(Number::from(u)));
        }
    }
    let f: f64 = token
        .parse()
        .map_err(|_| format!("invalid number: {token:?}"))?;
    if !f.is_finite() {
        return Err(format!("non-finite number: {token:?}"));
    }
    Number::from_f64(f)
        .map(Some)
        .ok_or_else(|| format!("invalid number: {token:?}"))
}

/// Parses a quoted string (with §7.1 escapes) at the start of `s`; returns the
/// unescaped value and the remainder.
fn parse_quoted(s: &str) -> Result<(String, &str), String> {
    let mut chars = s.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return Err(format!("expected a quoted string: {s:?}")),
    }
    let mut value = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((value, &s[i + 1..])),
            '\\' => {
                let Some((_, escape)) = chars.next() else {
                    return Err(format!("unterminated escape sequence: {s:?}"));
                };
                match escape {
                    '\\' => value.push('\\'),
                    '"' => value.push('"'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    'u' => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            let Some((_, hex)) = chars.next() else {
                                return Err(format!("invalid unicode escape: {s:?}"));
                            };
                            let Some(digit) = hex.to_digit(16) else {
                                return Err(format!("invalid unicode escape: {s:?}"));
                            };
                            code = code * 16 + digit;
                        }
                        let Some(c) = char::from_u32(code) else {
                            return Err(format!("invalid unicode escape (lone surrogate): {s:?}"));
                        };
                        value.push(c);
                    }
                    other => return Err(format!("invalid escape sequence: \\{other}")),
                }
            }
            c => value.push(c),
        }
    }
    Err(format!("unterminated quoted string: {s:?}"))
}

/// Splits a line on unquoted commas, trimming surrounding spaces around each
/// cell (§11.2).
fn split_cells(s: &str) -> Result<Vec<String>, String> {
    let mut cells = Vec::new();
    let mut rest = s;
    loop {
        match split_once_unquoted(rest, ',') {
            Some((cell, after)) => {
                cells.push(cell.trim_matches(' ').to_string());
                rest = after;
            }
            None => {
                cells.push(rest.trim_matches(' ').to_string());
                return Ok(cells);
            }
        }
    }
}

/// Splits at the first occurrence of `separator` that is not inside a quoted
/// string.
fn split_once_unquoted(s: &str, separator: char) -> Option<(&str, &str)> {
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            c if c == separator && !in_quotes => {
                return Some((&s[..i], &s[i + c.len_utf8()..]));
            }
            _ => {}
        }
    }
    None
}
