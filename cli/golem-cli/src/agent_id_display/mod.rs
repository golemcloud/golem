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
mod parse_moonbit;
mod parse_rust;
mod parse_scala;
mod parse_ts;
mod parse_type_moonbit;
mod parse_type_rust;
mod parse_type_scala;
mod parse_type_ts;
mod render_moonbit;
mod render_rust;
mod render_scala;
mod render_ts;

#[cfg(test)]
mod tests;

use golem_common::model::agent::text_utils::write_json_escaped;
use golem_common::schema::agent::{InputSchema, ParsedAgentId};
use golem_common::schema::graph::{SchemaGraph, TypedSchemaValue};
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;

pub use parse_common::ParseError;

/// Represents the source language of an agent component, used to select
/// language-specific rendering and parsing of agent IDs and data values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceLanguage {
    Rust,
    TypeScript,
    Scala,
    MoonBit,
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
            SourceLanguage::Rust
                | SourceLanguage::TypeScript
                | SourceLanguage::Scala
                | SourceLanguage::MoonBit
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
        } else if trimmed.eq_ignore_ascii_case("moonbit") {
            SourceLanguage::MoonBit
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
            SourceLanguage::MoonBit => write!(f, "moonbit"),
            SourceLanguage::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Render a paired schema graph + root type + value using language-specific
/// syntax. Capability values render as `<redacted>`.
pub fn render_typed_schema_value(
    typed: &TypedSchemaValue,
    source_language: &SourceLanguage,
) -> String {
    render_schema_value(
        typed.graph(),
        typed.root_type(),
        typed.value(),
        source_language,
    )
}

/// Render a schema-typed value using the given source language's native
/// syntax.
pub fn render_schema_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    source_language: &SourceLanguage,
) -> String {
    match source_language {
        SourceLanguage::Rust => render_rust::render_value_rust(graph, ty, value),
        SourceLanguage::Scala => render_scala::render_value_scala(graph, ty, value),
        SourceLanguage::MoonBit => render_moonbit::render_value_moonbit(graph, ty, value),
        SourceLanguage::TypeScript | SourceLanguage::Other(_) => {
            render_ts::render_value_ts(graph, ty, value)
        }
    }
}

/// Render a full agent ID string in the form `TypeName(params)[phantom]`.
///
/// The parameters are rendered using [`render_schema_value`] over each
/// `Record` field in declaration order.
pub fn render_agent_id(parsed: &ParsedAgentId, source_language: &SourceLanguage) -> String {
    let graph = parsed.parameters.graph();
    let root = parsed.parameters.root_type();
    let value = parsed.parameters.value();
    let rendered = match (root, value) {
        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs })
            if fields.len() == vs.len() =>
        {
            let mut parts = Vec::with_capacity(fields.len());
            for (field, val) in fields.iter().zip(vs.iter()) {
                parts.push(render_schema_value(
                    graph,
                    &field.body,
                    val,
                    source_language,
                ));
            }
            parts.join(", ")
        }
        _ => render_schema_value(graph, root, value, source_language),
    };
    let mut result = format!("{}({rendered})", parsed.agent_type);
    if let Some(uuid) = &parsed.phantom_id {
        result.push_str(&format!("[{uuid}]"));
    }
    result
}

/// Renders a [`SchemaType`] as a human-readable type expression using
/// language-specific syntax.
pub fn render_type_for_language(
    lang: &SourceLanguage,
    graph: &SchemaGraph,
    ty: &SchemaType,
    prefer_name: bool,
) -> String {
    match lang {
        SourceLanguage::Rust => render_rust::render_type_rust(graph, ty, prefer_name),
        SourceLanguage::Scala => render_scala::render_type_scala(graph, ty, prefer_name),
        SourceLanguage::MoonBit => render_moonbit::render_type_moonbit(graph, ty, prefer_name),
        SourceLanguage::TypeScript | SourceLanguage::Other(_) => {
            render_ts::render_type_ts(graph, ty, prefer_name)
        }
    }
}

/// Parses a type string using language-specific syntax into a
/// [`SchemaGraph`] + [`SchemaType`] pair.
///
/// For known source languages, attempts language-specific parsing.
/// For unknown languages, tries each known parser in turn.
pub fn parse_type_for_language(
    input: &str,
    source_language: &SourceLanguage,
) -> Result<(SchemaGraph, SchemaType), ParseError> {
    match source_language {
        SourceLanguage::Rust => parse_type_rust::parse_type_rust(input),
        SourceLanguage::TypeScript => parse_type_ts::parse_type_ts(input),
        SourceLanguage::Scala => parse_type_scala::parse_type_scala(input),
        SourceLanguage::MoonBit => parse_type_moonbit::parse_type_moonbit(input),
        SourceLanguage::Other(_) => parse_type_ts::parse_type_ts(input)
            .or_else(|_| parse_type_rust::parse_type_rust(input))
            .or_else(|_| parse_type_scala::parse_type_scala(input))
            .or_else(|_| parse_type_moonbit::parse_type_moonbit(input))
            .map_err(|_| ParseError {
                position: 0,
                message: format!("unrecognized type '{input}'"),
            }),
    }
}

/// Parses a single value of the given schema type using language-specific
/// syntax.
pub fn parse_value_for_language(
    input: &str,
    graph: &SchemaGraph,
    ty: &SchemaType,
    source_language: &SourceLanguage,
) -> Result<SchemaValue, ParseError> {
    fn try_parse<D: parse_common::Dialect>(
        input: &str,
        graph: &SchemaGraph,
        ty: &SchemaType,
    ) -> Result<SchemaValue, ParseError> {
        let mut lexer = lexer::Lexer::new(input);
        let result = parse_common::parse_cm_value::<D>(&mut lexer, graph, ty)?;
        let (tok, pos, _) = lexer.next_token()?;
        if tok != lexer::Token::Eof {
            return Err(ParseError {
                position: pos,
                message: format!("expected end of input, got {tok:?}"),
            });
        }
        Ok(result)
    }

    match source_language {
        SourceLanguage::Rust => try_parse::<parse_rust::RustDialect>(input, graph, ty),
        SourceLanguage::TypeScript => try_parse::<parse_ts::TsDialect>(input, graph, ty),
        SourceLanguage::Scala => try_parse::<parse_scala::ScalaDialect>(input, graph, ty),
        SourceLanguage::MoonBit => try_parse::<parse_moonbit::MoonBitDialect>(input, graph, ty),
        SourceLanguage::Other(_) => try_parse::<parse_ts::TsDialect>(input, graph, ty)
            .or_else(|_| try_parse::<parse_rust::RustDialect>(input, graph, ty))
            .or_else(|_| try_parse::<parse_scala::ScalaDialect>(input, graph, ty))
            .or_else(|_| try_parse::<parse_moonbit::MoonBitDialect>(input, graph, ty))
            .map_err(|_| ParseError {
                position: 0,
                message: format!("could not parse value '{input}'"),
            }),
    }
}

/// Parses the parameter portion of an agent ID string into a
/// [`SchemaValue::Record`] aligned with the supplied [`InputSchema`].
pub fn parse_agent_id_params(
    input: &str,
    graph: &SchemaGraph,
    input_schema: &InputSchema,
    source_language: &SourceLanguage,
) -> Result<SchemaValue, ParseError> {
    let InputSchema::Parameters(fields) = input_schema;
    match source_language {
        SourceLanguage::Rust => {
            parse_common::parse_input_schema_params::<parse_rust::RustDialect>(input, graph, fields)
        }
        SourceLanguage::TypeScript => {
            parse_common::parse_input_schema_params::<parse_ts::TsDialect>(input, graph, fields)
        }
        SourceLanguage::Scala => {
            parse_common::parse_input_schema_params::<parse_scala::ScalaDialect>(
                input, graph, fields,
            )
        }
        SourceLanguage::MoonBit => parse_common::parse_input_schema_params::<
            parse_moonbit::MoonBitDialect,
        >(input, graph, fields),
        SourceLanguage::Other(_) => {
            parse_common::parse_input_schema_params::<parse_ts::TsDialect>(input, graph, fields)
                .or_else(|_| {
                    parse_common::parse_input_schema_params::<parse_rust::RustDialect>(
                        input, graph, fields,
                    )
                })
                .or_else(|_| {
                    parse_common::parse_input_schema_params::<parse_scala::ScalaDialect>(
                        input, graph, fields,
                    )
                })
                .or_else(|_| {
                    parse_common::parse_input_schema_params::<parse_moonbit::MoonBitDialect>(
                        input, graph, fields,
                    )
                })
                .map_err(|_| ParseError {
                    position: 0,
                    message: format!("could not parse agent-id parameters '{input}'"),
                })
        }
    }
}

/// Resolve a [`SchemaType::Ref`] against the supplied graph, returning
/// the def body and the def name when present. Non-ref types pass through
/// with `None` def name.
pub(crate) fn resolve_named_ref<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> (&'a SchemaType, Option<&'a str>) {
    match ty {
        SchemaType::Ref { id, .. } => match graph.lookup(id) {
            Some(def) => (&def.body, def.name.as_deref()),
            None => (ty, None),
        },
        _ => (ty, None),
    }
}

/// Render a rich-scalar constructor of the form `Name("payload")` (or
/// `Name("payload", "extra")` when `extra` is `Some`). Bodies are
/// JSON-escaped so the per-language parsers can absorb arbitrary
/// canonical text (URLs, base64, RFC 3339 timestamps, paths) via the
/// shared lexer.
pub(crate) fn render_rich_constructor(buf: &mut String, name: &str, body: &str) {
    buf.push_str(name);
    buf.push_str("(\"");
    write_json_escaped(buf, body);
    buf.push_str("\")");
}

/// Like [`render_rich_constructor`] but with an optional second string
/// argument (used for `Text("body", "language")`).
pub(crate) fn render_rich_constructor2(
    buf: &mut String,
    name: &str,
    body: &str,
    extra: Option<&str>,
) {
    buf.push_str(name);
    buf.push_str("(\"");
    write_json_escaped(buf, body);
    if let Some(extra) = extra {
        buf.push_str("\", \"");
        write_json_escaped(buf, extra);
    }
    buf.push_str("\")");
}
