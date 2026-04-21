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
use golem_wasm::analysis::AnalysedType;
use golem_wasm::analysis::analysed_type;

pub(super) fn parse_type_moonbit(input: &str) -> Result<AnalysedType, ParseError> {
    let result = parse_type_inner(input.trim())?;
    Ok(result)
}

fn parse_type_inner(s: &str) -> Result<AnalysedType, ParseError> {
    let s = s.trim();

    // Check for T? suffix (option shorthand)
    if let Some(inner) = s.strip_suffix('?') {
        return Ok(analysed_type::option(parse_type_inner(inner)?));
    }

    // Check for tuple (T1, T2, ...)
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        let parts = split_all_top_level_commas(inner)?;
        let types = parts
            .into_iter()
            .map(parse_type_inner)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(analysed_type::tuple(types));
    }

    if let Some(inner) = strip_generic(s, "Array", '[', ']') {
        return Ok(analysed_type::list(parse_type_inner(inner)?));
    }
    if let Some(inner) = strip_generic(s, "Option", '[', ']') {
        return Ok(analysed_type::option(parse_type_inner(inner)?));
    }
    if let Some(inner) = strip_generic(s, "Result", '[', ']') {
        let (ok_str, err_str) = split_at_top_level_comma(inner)?;
        return Ok(analysed_type::result(
            parse_type_inner(ok_str)?,
            parse_type_inner(err_str)?,
        ));
    }

    match s {
        "String" => Ok(analysed_type::str()),
        "Bool" => Ok(analysed_type::bool()),
        "Char" => Ok(analysed_type::chr()),
        "Byte" => Ok(analysed_type::u8()),
        "Int" => Ok(analysed_type::s32()),
        "Int64" => Ok(analysed_type::s64()),
        "UInt" => Ok(analysed_type::u32()),
        "UInt64" => Ok(analysed_type::u64()),
        "Float" => Ok(analysed_type::f32()),
        "Double" => Ok(analysed_type::f64()),
        _ => Err(ParseError {
            position: 0,
            message: format!("unrecognized MoonBit type '{s}'"),
        }),
    }
}

fn strip_generic<'a>(s: &'a str, prefix: &str, open: char, close: char) -> Option<&'a str> {
    let rest = s.strip_prefix(prefix)?.trim_start();
    let rest = rest.strip_prefix(open)?;
    let rest = rest.strip_suffix(close)?;
    Some(rest)
}

fn split_at_top_level_comma(s: &str) -> Result<(&str, &str), ParseError> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                return Ok((&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
    }
    Err(ParseError {
        position: 0,
        message: "expected comma separating type parameters".to_string(),
    })
}

fn split_all_top_level_commas(s: &str) -> Result<Vec<&str>, ParseError> {
    let mut depth = 0i32;
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    Ok(parts)
}
