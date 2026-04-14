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
mod parse_common;
mod parse_rust;
mod parse_scala;
mod parse_ts;
mod render_rust;
mod render_scala;
mod render_ts;

#[cfg(test)]
mod tests;

use golem_common::model::agent::structural_format::parse_structural;
use golem_common::model::agent::{DataSchema, DataValue, ParsedAgentId};
use golem_wasm::ValueAndType;

pub use parse_common::ParseError;

/// Represents the source language of an agent component, used to select
/// language-specific rendering and parsing of agent IDs and data values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceLanguage {
    Rust,
    TypeScript,
    Scala,
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
        matches!(
            self,
            SourceLanguage::Rust | SourceLanguage::TypeScript | SourceLanguage::Scala
        )
    }
}

impl From<&str> for SourceLanguage {
    fn from(s: &str) -> Self {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("rust") {
            SourceLanguage::Rust
        } else if trimmed.eq_ignore_ascii_case("typescript") || trimmed.eq_ignore_ascii_case("ts") {
            SourceLanguage::TypeScript
        } else if trimmed.eq_ignore_ascii_case("scala") {
            SourceLanguage::Scala
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
            SourceLanguage::Scala => write!(f, "scala"),
            SourceLanguage::Other(s) => write!(f, "{s}"),
        }
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
        SourceLanguage::Scala => render_scala::render_data_value_scala(data_value),
        SourceLanguage::TypeScript | SourceLanguage::Other(_) => {
            render_ts::render_data_value_ts(data_value)
        }
    }
}

/// Renders a single [`ValueAndType`] using language-specific syntax.
///
/// This is useful for displaying individual component model values (a subtree of [`DataValue`])
/// in the source language's native format.
pub fn render_value_and_type(vat: &ValueAndType, source_language: &SourceLanguage) -> String {
    match source_language {
        SourceLanguage::Rust => render_rust::render_value_and_type_rust(vat),
        SourceLanguage::Scala => render_scala::render_value_and_type_scala(vat),
        SourceLanguage::TypeScript | SourceLanguage::Other(_) => {
            render_ts::render_value_and_type_ts(vat)
        }
    }
}

/// Renders a full agent ID string in the form `TypeName(params)[phantom]`.
///
/// The parameters are rendered using [`render_data_value`] with the given source language.
pub fn render_agent_id(parsed: &ParsedAgentId, source_language: &SourceLanguage) -> String {
    let rendered = render_data_value(&parsed.parameters, source_language);
    let mut result = format!("{}({rendered})", parsed.agent_type);
    if let Some(uuid) = &parsed.phantom_id {
        result.push_str(&format!("[{uuid}]"));
    }
    result
}

/// Renders an [`AnalysedType`] as a human-readable type expression using language-specific syntax.
///
/// When `prefer_name` is true and the type has a name (e.g., named records, variants),
/// the name is used instead of the inline structural representation.
pub fn render_type_for_language(
    lang: &SourceLanguage,
    typ: &golem_wasm::analysis::AnalysedType,
    prefer_name: bool,
) -> String {
    match lang {
        SourceLanguage::Rust => render_rust::render_type_rust(typ, prefer_name),
        SourceLanguage::Scala => render_scala::render_type_scala(typ, prefer_name),
        SourceLanguage::TypeScript | SourceLanguage::Other(_) => {
            render_ts::render_type_ts(typ, prefer_name)
        }
    }
}

/// Parses the parameter portion of an agent ID string into a [`DataValue`].
///
/// For known source languages (Rust, TypeScript), first attempts language-specific
/// parsing. If that fails, falls back to canonical structural parsing. If both fail,
/// returns a combined error message showing both failures.
///
/// For unknown source languages, uses canonical structural parsing directly.
pub fn parse_agent_id_params(
    input: &str,
    schema: &DataSchema,
    source_language: &SourceLanguage,
) -> Result<DataValue, ParseError> {
    match source_language {
        SourceLanguage::Rust => match parse_rust::parse_data_value_rust(input, schema) {
            Ok(value) => Ok(value),
            Err(lang_err) => parse_structural(input, schema).map_err(|structural_err| ParseError {
                position: 0,
                message: format!(
                    "Rust parser: {}; Structural parser: {}",
                    lang_err, structural_err
                ),
            }),
        },
        SourceLanguage::TypeScript => match parse_ts::parse_data_value_ts(input, schema) {
            Ok(value) => Ok(value),
            Err(lang_err) => parse_structural(input, schema).map_err(|structural_err| ParseError {
                position: 0,
                message: format!(
                    "TypeScript parser: {}; Structural parser: {}",
                    lang_err, structural_err
                ),
            }),
        },
        SourceLanguage::Scala => match parse_scala::parse_data_value_scala(input, schema) {
            Ok(value) => Ok(value),
            Err(lang_err) => parse_structural(input, schema).map_err(|structural_err| ParseError {
                position: 0,
                message: format!(
                    "Scala parser: {}; Structural parser: {}",
                    lang_err, structural_err
                ),
            }),
        },
        _ => parse_structural(input, schema).map_err(|e| ParseError {
            position: 0,
            message: e.to_string(),
        }),
    }
}
