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

mod lexer;
mod parse_rust;
mod parse_ts;
mod render_rust;
mod render_ts;

#[cfg(test)]
mod tests;

use golem_common::model::agent::structural_format::{format_structural, parse_structural};
use golem_common::model::agent::{DataSchema, DataValue, ParsedAgentId};
use golem_wasm::ValueAndType;

/// Represents the source language of an agent component, used to select
/// language-specific rendering and parsing of agent IDs and data values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceLanguage {
    Rust,
    TypeScript,
    Other(String),
}

impl Default for SourceLanguage {
    fn default() -> Self {
        SourceLanguage::Other(String::new())
    }
}

impl SourceLanguage {
    /// Returns true if this is a known language with specialized rendering/parsing support.
    pub fn is_known(&self) -> bool {
        matches!(self, SourceLanguage::Rust | SourceLanguage::TypeScript)
    }
}

impl From<&str> for SourceLanguage {
    fn from(s: &str) -> Self {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("rust") {
            SourceLanguage::Rust
        } else if trimmed.eq_ignore_ascii_case("typescript")
            || trimmed.eq_ignore_ascii_case("ts")
        {
            SourceLanguage::TypeScript
        } else {
            SourceLanguage::Other(trimmed.to_string())
        }
    }
}

impl From<String> for SourceLanguage {
    fn from(s: String) -> Self {
        SourceLanguage::from(s.as_str())
    }
}

impl std::fmt::Display for SourceLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceLanguage::Rust => write!(f, "rust"),
            SourceLanguage::TypeScript => write!(f, "typescript"),
            SourceLanguage::Other(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub position: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error at position {}: {}", self.position, self.message)
    }
}

impl std::error::Error for ParseError {}

impl From<parse_rust::ParseError> for ParseError {
    fn from(e: parse_rust::ParseError) -> Self {
        Self { position: e.position, message: e.message }
    }
}

impl From<parse_ts::ParseError> for ParseError {
    fn from(e: parse_ts::ParseError) -> Self {
        Self { position: e.position, message: e.message }
    }
}

/// Renders a [`DataValue`] as a human-readable string using language-specific syntax.
///
/// For [`SourceLanguage::Rust`], produces Rust literal syntax (e.g. `"hello"`, `Some(42)`).
/// For [`SourceLanguage::TypeScript`], produces TypeScript/JSON-like syntax (e.g. `{ ok: 42 }`).
/// For other languages, falls back to the canonical structural format.
pub fn render_data_value(data_value: &DataValue, source_language: &SourceLanguage) -> String {
    match source_language {
        SourceLanguage::Rust => render_rust::render_data_value_rust(data_value),
        SourceLanguage::TypeScript => render_ts::render_data_value_ts(data_value),
        _ => format_structural(data_value).unwrap_or_else(|_| format!("{data_value:?}")),
    }
}

/// Renders a single [`ValueAndType`] using language-specific syntax.
///
/// This is useful for displaying individual component model values (a subtree of [`DataValue`])
/// in the source language's native format.
pub fn render_value_and_type(
    vat: &ValueAndType,
    source_language: &SourceLanguage,
) -> String {
    match source_language {
        SourceLanguage::Rust => render_rust::render_value_and_type_rust(vat),
        SourceLanguage::TypeScript => render_ts::render_value_and_type_ts(vat),
        _ => format!("{:?}", vat.value),
    }
}

/// Renders a full agent ID string in the form `TypeName(params)[phantom]`.
///
/// The parameters are rendered using [`render_data_value`] with the given source language.
pub fn render_agent_id(
    parsed: &ParsedAgentId,
    source_language: &SourceLanguage,
) -> String {
    let rendered = render_data_value(&parsed.parameters, source_language);
    let mut result = format!("{}({rendered})", parsed.agent_type);
    if let Some(uuid) = &parsed.phantom_id {
        result.push_str(&format!("[{uuid}]"));
    }
    result
}

/// Parses the parameter portion of an agent ID string into a [`DataValue`].
///
/// First attempts parsing using the canonical structural format. If that fails,
/// falls back to language-specific parsing based on `source_language`.
pub fn parse_agent_id_params(
    input: &str,
    schema: &DataSchema,
    source_language: &SourceLanguage,
) -> Result<DataValue, ParseError> {
    if let Ok(value) = parse_structural(input, schema) {
        return Ok(value);
    }
    match source_language {
        SourceLanguage::Rust => parse_rust::parse_data_value_rust(input, schema).map_err(Into::into),
        SourceLanguage::TypeScript => parse_ts::parse_data_value_ts(input, schema).map_err(Into::into),
        _ => parse_structural(input, schema).map_err(|e| ParseError {
            position: 0,
            message: e.to_string(),
        }),
    }
}
