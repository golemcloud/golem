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

pub(super) fn parse_type_rust(input: &str) -> Result<AnalysedType, ParseError> {
    let result = parse_type_inner(input.trim())?;
    Ok(result)
}

fn parse_type_inner(s: &str) -> Result<AnalysedType, ParseError> {
    let s = s.trim();

    if let Some(inner) = strip_generic(s, "Vec", '<', '>') {
        return Ok(analysed_type::list(parse_type_inner(inner)?));
    }
    if let Some(inner) = strip_generic(s, "Option", '<', '>') {
        return Ok(analysed_type::option(parse_type_inner(inner)?));
    }
    if let Some(inner) = strip_generic(s, "Result", '<', '>') {
        let (ok_str, err_str) = split_at_top_level_comma(inner)?;
        return Ok(analysed_type::result(
            parse_type_inner(ok_str)?,
            parse_type_inner(err_str)?,
        ));
    }

    match s {
        "String" => Ok(analysed_type::str()),
        "char" => Ok(analysed_type::chr()),
        "bool" => Ok(analysed_type::bool()),
        "u8" => Ok(analysed_type::u8()),
        "u16" => Ok(analysed_type::u16()),
        "u32" => Ok(analysed_type::u32()),
        "u64" => Ok(analysed_type::u64()),
        "i8" => Ok(analysed_type::s8()),
        "i16" => Ok(analysed_type::s16()),
        "i32" => Ok(analysed_type::s32()),
        "i64" => Ok(analysed_type::s64()),
        "f32" => Ok(analysed_type::f32()),
        "f64" => Ok(analysed_type::f64()),
        _ => Err(ParseError {
            position: 0,
            message: format!("unrecognized Rust type '{s}'"),
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
            '<' | '[' => depth += 1,
            '>' | ']' => depth -= 1,
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
