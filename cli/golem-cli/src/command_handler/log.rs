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

use crate::context::Context;
use crate::log::print_line;
use crate::model::cli_output::raw_json_text::{compact, pretty, yaml_flow};
use crate::model::cli_output::{
    StructuredOutput, marked_value_to_toon, take_raw_json, to_marked_structured_output_value_masked,
};
use crate::model::format::Format;
use crate::model::masking::MaskingConfig;
use crate::model::text_format::{
    DecoratedIndent, TextOutput, TruncatableTextOutput, colorize_json, colorize_yaml,
    to_colored_json, to_colored_yaml, truncate_rendered,
};
use crate::toon;
use serde::Serialize;
use std::sync::Arc;

pub const TOON_FRAME_START: &str = "@toon";
pub const TOON_FRAME_END: &str = "@end";

pub struct LogHandler {
    ctx: Arc<Context>,
}

pub trait CommandOutput: TextOutput + StructuredOutput {}

impl<T> CommandOutput for T where T: TextOutput + StructuredOutput {}

impl LogHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn log_output<Output: CommandOutput>(&self, output: Output) -> anyhow::Result<()> {
        let masking = self.ctx.masking_config();
        match self.ctx.format() {
            Format::Json
            | Format::PrettyJson
            | Format::Yaml
            | Format::PrettyYaml
            | Format::Toon => {
                print_line(render_command_output_document_masked(
                    self.ctx.format(),
                    self.ctx.should_colorize(),
                    masking,
                    output,
                )?);
            }
            Format::Text => {
                output.log_masked(masking)?;
            }
        }
        Ok(())
    }

    pub fn render_view_truncated<View: TruncatableTextOutput + CommandOutput>(
        &self,
        view: View,
        max_lines: usize,
    ) -> anyhow::Result<String> {
        match self.ctx.format() {
            Format::Text => view.render_truncated_masked(
                max_lines,
                self.ctx.should_colorize(),
                self.ctx.masking_config(),
            ),
            _ => {
                let rendered = render_command_output_document_masked(
                    self.ctx.format(),
                    self.ctx.should_colorize(),
                    self.ctx.masking_config(),
                    view,
                )?;
                Ok(truncate_rendered(rendered, max_lines))
            }
        }
    }

    pub fn decorated_indent_primary(&self) -> DecoratedIndent {
        DecoratedIndent::new_primary(self.ctx.format())
    }

    pub fn decorated_indent_secondary(&self) -> DecoratedIndent {
        DecoratedIndent::new_secondary(self.ctx.format())
    }
}

pub fn render_toon_document<S: Serialize>(value: &S) -> anyhow::Result<String> {
    Ok(format!(
        "{TOON_FRAME_START}\n{}\n{TOON_FRAME_END}",
        toon::encode(value)?
    ))
}

pub fn render_structured_document<S: Serialize>(
    format: Format,
    colorize: bool,
    value: &S,
) -> anyhow::Result<String> {
    match format {
        Format::Json => Ok(serde_json::to_string(value)?),
        Format::PrettyJson => {
            if colorize {
                to_colored_json(value)
            } else {
                Ok(serde_json::to_string_pretty(value)?)
            }
        }
        Format::Yaml => Ok(format!("---\n{}", serde_yaml::to_string(value)?)),
        Format::PrettyYaml => {
            if colorize {
                Ok(format!("---\n{}", to_colored_yaml(value)?))
            } else {
                Ok(format!("---\n{}", serde_yaml::to_string(value)?))
            }
        }
        Format::Toon => render_toon_document(value),
        Format::Text => unreachable!("text is not a structured output format"),
    }
}

pub fn render_command_output_document<Output: StructuredOutput>(
    format: Format,
    colorize: bool,
    output: Output,
) -> anyhow::Result<String> {
    render_command_output_document_masked(format, colorize, MaskingConfig::hide_secrets(), output)
}

pub fn render_command_output_document_masked<Output: StructuredOutput>(
    format: Format,
    colorize: bool,
    config: MaskingConfig,
    output: Output,
) -> anyhow::Result<String> {
    let value = to_marked_structured_output_value_masked(output, config)?;
    render_marked_value(format, colorize, value)
}

/// Renders a structured output value that can hold marked raw JSON values, such as the claims of
/// a user. Each raw JSON value is printed as the JSON value that its text holds, with every
/// number as it is in the text: the recorded text goes into the JSON formats, YAML gets a flow
/// node on one line, and TOON gets a value that keeps the member order and the number texts.
/// Everything else is printed as [`render_structured_document`] prints it.
fn render_marked_value(
    format: Format,
    colorize: bool,
    mut value: serde_json::Value,
) -> anyhow::Result<String> {
    if format == Format::Toon {
        return Ok(format!(
            "{TOON_FRAME_START}\n{}\n{TOON_FRAME_END}",
            toon::encode_toon_value(&marked_value_to_toon(&value)?)?
        ));
    }
    let raw_json = take_raw_json(&mut value);
    if raw_json.is_empty() {
        return render_structured_document(format, colorize, &value);
    }
    match format {
        Format::Json => {
            let mut text = serde_json::to_string(&value)?;
            for (token, raw) in &raw_json {
                text = put_in_place(&text, &format!("\"{token}\""), |_| Ok(compact(raw)))?;
            }
            Ok(text)
        }
        Format::PrettyJson => {
            let mut text = serde_json::to_string_pretty(&value)?;
            for (token, raw) in &raw_json {
                // The value starts on the line of its member name, which has two spaces for
                // each level.
                text = put_in_place(&text, &format!("\"{token}\""), |line_start| {
                    let spaces = line_start.len() - line_start.trim_start_matches(' ').len();
                    Ok(pretty(raw, spaces / 2))
                })?;
            }
            if colorize {
                colorize_json(&text)
            } else {
                Ok(text)
            }
        }
        Format::Yaml | Format::PrettyYaml => {
            let mut text = serde_yaml::to_string(&value)?;
            for (token, raw) in &raw_json {
                // The token has only letters, digits and hyphens, so YAML prints it as it is.
                text = put_in_place(&text, token, |_| yaml_flow(raw))?;
            }
            if colorize && format == Format::PrettyYaml {
                text = colorize_yaml(&text)?;
            }
            Ok(format!("---\n{text}"))
        }
        Format::Toon | Format::Text => {
            unreachable!("TOON is rendered above, and text is not a structured output format")
        }
    }
}

/// Replaces the one occurrence of `token` in `text`. `replacement` gets the part of the token's
/// line that comes before the token.
fn put_in_place(
    text: &str,
    token: &str,
    replacement: impl FnOnce(&str) -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    let at = text
        .find(token)
        .ok_or_else(|| anyhow::anyhow!("a raw JSON value is missing from the rendered output"))?;
    let line_start = text[..at].rfind('\n').map_or(0, |end| end + 1);
    Ok(format!(
        "{}{}{}",
        &text[..at],
        replacement(&text[line_start..at])?,
        &text[at + token.len()..]
    ))
}

pub fn print_command_output_document<Output: StructuredOutput>(
    format: Format,
    colorize: bool,
    output: Output,
) -> anyhow::Result<()> {
    print_line(render_command_output_document(format, colorize, output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::command_handler::log::{
        TOON_FRAME_END, TOON_FRAME_START, render_command_output_document, render_toon_document,
    };
    use crate::model::cli_output::{CLI_OUTPUT_TYPE_FIELD, StructuredOutput};
    use crate::model::format::Format;
    use serde::Serialize;
    use serde_json::json;

    #[test_r::test]
    fn render_toon_document_wraps_payload_in_frame_markers() {
        let rendered = render_toon_document(&json!({ "status": "ok" })).unwrap();

        assert!(rendered.starts_with(TOON_FRAME_START));
        assert!(rendered.ends_with(TOON_FRAME_END));
        assert!(rendered.contains("status: ok"));
    }

    #[test_r::test]
    fn render_command_output_document_injects_type() {
        #[derive(Serialize)]
        struct TestOutput {
            ok: bool,
        }

        impl StructuredOutput for TestOutput {
            const KIND: &'static str = "test.output";
        }

        let rendered = render_command_output_document(Format::Json, false, TestOutput { ok: true })
            .expect("render output");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rendered).unwrap(),
            json!({
                CLI_OUTPUT_TYPE_FIELD: "test.output",
                "ok": true,
            })
        );
    }

    #[test_r::test]
    fn render_command_output_document_rejects_reserved_type_field() {
        #[derive(Serialize)]
        struct BadOutput {
            #[serde(rename = "$type")]
            output_type: String,
        }

        impl StructuredOutput for BadOutput {
            const KIND: &'static str = "test.bad-output";
        }

        let error = render_command_output_document(
            Format::Json,
            false,
            BadOutput {
                output_type: "custom".to_string(),
            },
        )
        .expect_err("reserved field should be rejected");

        assert!(
            error
                .to_string()
                .contains("must not define reserved field $type")
        );
    }
}
