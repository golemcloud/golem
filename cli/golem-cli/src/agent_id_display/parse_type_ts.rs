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

use super::parse_common::ParseError;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;

pub(super) fn parse_type_ts(input: &str) -> Result<(SchemaGraph, SchemaType), ParseError> {
    let ty = parse_type_inner(input.trim())?;
    Ok((SchemaGraph::anonymous(ty.clone()), ty))
}

fn parse_type_inner(s: &str) -> Result<SchemaType, ParseError> {
    let s = s.trim();

    // `(...)` grouping — the TS renderer wraps union elements in parens
    // when their element type contains `|` (e.g. `(number | undefined)[]`).
    if let Some(inner) = strip_balanced_parens(s) {
        return parse_type_inner(inner);
    }

    if let Some(left) = strip_pipe_undefined(s) {
        return Ok(SchemaType::option(parse_type_inner(left)?));
    }

    if let Some(inner) = s.strip_suffix("[]") {
        return Ok(SchemaType::list(parse_type_inner(inner)?));
    }

    match s {
        // `Uint8Array` is the TS renderer's preferred form for `list<u8>`.
        "Uint8Array" => Ok(SchemaType::list(SchemaType::u8())),
        "string" => Ok(SchemaType::string()),
        "boolean" => Ok(SchemaType::bool()),
        "u8" => Ok(SchemaType::u8()),
        "u16" => Ok(SchemaType::u16()),
        "u32" => Ok(SchemaType::u32()),
        "u64" => Ok(SchemaType::u64()),
        "s8" => Ok(SchemaType::s8()),
        "s16" => Ok(SchemaType::s16()),
        "s32" => Ok(SchemaType::s32()),
        "s64" => Ok(SchemaType::s64()),
        "f32" => Ok(SchemaType::f32()),
        "f64" => Ok(SchemaType::f64()),
        _ => Err(ParseError {
            position: 0,
            message: format!("unrecognized TypeScript type '{s}'"),
        }),
    }
}

/// If `s` is `(inner)` with balanced parentheses surrounding the whole
/// string, return the trimmed `inner`. Returns `None` otherwise (so
/// `(a)[]` and `(a) | (b)` are left for the other rules to handle).
fn strip_balanced_parens(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'(') || bytes.last() != Some(&b')') {
        return None;
    }
    let mut depth = 0i32;
    for (i, ch) in bytes.iter().enumerate() {
        match ch {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 && i + 1 != bytes.len() {
                    // closing paren is not the last character → not a
                    // wrapping grouping (e.g. `(a)[]` or `(a) | (b)`).
                    return None;
                }
            }
            _ => {}
        }
    }
    Some(s[1..bytes.len() - 1].trim())
}

fn strip_pipe_undefined(s: &str) -> Option<&str> {
    if !s.ends_with("undefined") {
        return None;
    }
    let trimmed = s.trim_end();
    let candidate = trimmed.strip_suffix("undefined")?.trim_end();
    let candidate = candidate.strip_suffix('|')?.trim_end();
    let mut depth = 0i32;
    for c in candidate.chars() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(candidate)
}
