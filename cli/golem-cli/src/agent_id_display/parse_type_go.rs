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

//! Reads Go type expressions: the builtin scalars, `*T`, `[]T`, `[N]T`,
//! `map[K]V` and `Result[T, E]`, with `golem.Option[T]` and `golem.Result`
//! accepted as the SDK spells them.

use super::parse_common::ParseError;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{ResultSpec, SchemaType};

pub(super) fn parse_type_go(input: &str) -> Result<(SchemaGraph, SchemaType), ParseError> {
    let ty = parse_type_inner(input.trim())?;
    Ok((SchemaGraph::anonymous(ty.clone()), ty))
}

fn err(message: String) -> ParseError {
    ParseError {
        position: 0,
        message,
    }
}

fn parse_type_inner(s: &str) -> Result<SchemaType, ParseError> {
    let s = s.trim();
    let s = s.strip_prefix("golem.").unwrap_or(s);

    if let Some(inner) = s.strip_prefix('*') {
        return Ok(SchemaType::option(parse_type_inner(inner)?));
    }
    if let Some(inner) = s.strip_prefix("[]") {
        return Ok(SchemaType::list(parse_type_inner(inner)?));
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (len, element) = rest
            .split_once(']')
            .ok_or_else(|| err(format!("unterminated array length in '{s}'")))?;
        let length = len
            .trim()
            .parse::<u32>()
            .map_err(|_| err(format!("invalid array length '{len}'")))?;
        return Ok(SchemaType::fixed_list(parse_type_inner(element)?, length));
    }
    if let Some(rest) = s.strip_prefix("map[") {
        let close = matching_bracket(rest)
            .ok_or_else(|| err(format!("unterminated map key type in '{s}'")))?;
        return Ok(SchemaType::map(
            parse_type_inner(&rest[..close])?,
            parse_type_inner(&rest[close + 1..])?,
        ));
    }
    if let Some(inner) = generic(s, "Option") {
        return Ok(SchemaType::option(parse_type_inner(inner)?));
    }
    if let Some(inner) = generic(s, "Result") {
        let (ok, e) = split_at_top_level_comma(inner)?;
        return Ok(SchemaType::result(ResultSpec {
            ok: Some(Box::new(parse_type_inner(ok)?)),
            err: Some(Box::new(parse_type_inner(e)?)),
        }));
    }

    match s {
        "string" => Ok(SchemaType::string()),
        "rune" => Ok(SchemaType::char()),
        "bool" => Ok(SchemaType::bool()),
        "uint8" | "byte" => Ok(SchemaType::u8()),
        "uint16" => Ok(SchemaType::u16()),
        "uint32" => Ok(SchemaType::u32()),
        "uint64" => Ok(SchemaType::u64()),
        "int8" => Ok(SchemaType::s8()),
        "int16" => Ok(SchemaType::s16()),
        "int32" => Ok(SchemaType::s32()),
        "int64" => Ok(SchemaType::s64()),
        "float32" => Ok(SchemaType::f32()),
        "float64" => Ok(SchemaType::f64()),
        _ => Err(err(format!("unrecognized Go type '{s}'"))),
    }
}

/// The type argument of `Name[T]`.
fn generic<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(name)?.strip_prefix('[')?;
    rest.strip_suffix(']')
}

/// Index of the `]` closing a bracket whose `[` was already consumed.
fn matching_bracket(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' if depth == 0 => return Some(i),
            ']' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn split_at_top_level_comma(s: &str) -> Result<(&str, &str), ParseError> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            ',' if depth == 0 => return Ok((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    Err(err(
        "expected a comma separating type parameters".to_string()
    ))
}
