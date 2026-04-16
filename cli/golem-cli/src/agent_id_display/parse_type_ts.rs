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
use golem_wasm::analysis::analysed_type;
use golem_wasm::analysis::AnalysedType;

pub(super) fn parse_type_ts(input: &str) -> Result<AnalysedType, ParseError> {
    let result = parse_type_inner(input.trim())?;
    Ok(result)
}

fn parse_type_inner(s: &str) -> Result<AnalysedType, ParseError> {
    let s = s.trim();

    // Check for T | undefined (option) — split at top-level pipe
    if let Some(left) = strip_pipe_undefined(s) {
        return Ok(analysed_type::option(parse_type_inner(left)?));
    }

    // Check for array suffix T[]
    if let Some(inner) = s.strip_suffix("[]") {
        return Ok(analysed_type::list(parse_type_inner(inner)?));
    }

    match s {
        "string" => Ok(analysed_type::str()),
        "boolean" => Ok(analysed_type::bool()),
        "u8" => Ok(analysed_type::u8()),
        "u16" => Ok(analysed_type::u16()),
        "u32" => Ok(analysed_type::u32()),
        "u64" => Ok(analysed_type::u64()),
        "s8" => Ok(analysed_type::s8()),
        "s16" => Ok(analysed_type::s16()),
        "s32" => Ok(analysed_type::s32()),
        "s64" => Ok(analysed_type::s64()),
        "f32" => Ok(analysed_type::f32()),
        "f64" => Ok(analysed_type::f64()),
        _ => Err(ParseError {
            position: 0,
            message: format!("unrecognized TypeScript type '{s}'"),
        }),
    }
}

/// If `s` ends with `| undefined` at the top level (not inside brackets),
/// returns the left-hand side trimmed.
fn strip_pipe_undefined(s: &str) -> Option<&str> {
    if !s.ends_with("undefined") {
        return None;
    }
    let trimmed = s.trim_end();
    let candidate = trimmed.strip_suffix("undefined")?.trim_end();
    let candidate = candidate.strip_suffix('|')?.trim_end();

    // Verify the pipe is at top level by checking bracket depth of candidate
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
