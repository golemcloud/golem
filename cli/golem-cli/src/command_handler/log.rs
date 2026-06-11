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
use crate::model::cli_output::{CliOutput, to_cli_output_value};
use crate::model::format::Format;
use crate::model::text::fmt::{
    DecoratedIndent, TextView, TruncatableTextView, to_colored_json, to_colored_yaml,
    truncate_rendered,
};
use crate::toon;
use serde::Serialize;
use std::sync::Arc;

pub const TOON_FRAME_START: &str = "@toon";
pub const TOON_FRAME_END: &str = "@end";

pub struct LogHandler {
    ctx: Arc<Context>,
}

impl LogHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn log_view<View: TextView + CliOutput>(&self, view: &View) -> anyhow::Result<()> {
        match self.ctx.format() {
            Format::Json
            | Format::PrettyJson
            | Format::Yaml
            | Format::PrettyYaml
            | Format::Toon => {
                print_cli_output_document(self.ctx.format(), self.ctx.should_colorize(), view)?;
            }
            Format::Text => {
                view.log();
            }
        }
        Ok(())
    }

    pub fn render_view_truncated<View: TruncatableTextView + Serialize>(
        &self,
        view: &View,
        max_lines: usize,
    ) -> anyhow::Result<String> {
        match self.ctx.format() {
            Format::Text => Ok(view.render_truncated(max_lines, self.ctx.should_colorize())),
            _ => {
                let rendered = render_structured_document(
                    self.ctx.format(),
                    self.ctx.should_colorize(),
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

pub fn render_cli_output_document<Output: CliOutput>(
    format: Format,
    colorize: bool,
    output: &Output,
) -> anyhow::Result<String> {
    let value = to_cli_output_value(output)?;
    render_structured_document(format, colorize, &value)
}

pub fn print_cli_output_document<Output: CliOutput>(
    format: Format,
    colorize: bool,
    output: &Output,
) -> anyhow::Result<()> {
    println!("{}", render_cli_output_document(format, colorize, output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::command_handler::log::{
        TOON_FRAME_END, TOON_FRAME_START, render_cli_output_document, render_toon_document,
    };
    use crate::model::cli_output::{CLI_OUTPUT_TYPE_FIELD, CliOutput};
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
    fn render_cli_output_document_injects_type() {
        #[derive(Serialize)]
        struct TestOutput {
            ok: bool,
        }

        impl CliOutput for TestOutput {
            const KIND: &'static str = "test.output";
        }

        let rendered = render_cli_output_document(Format::Json, false, &TestOutput { ok: true })
            .expect("render output");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rendered).unwrap(),
            json!({
                CLI_OUTPUT_TYPE_FIELD: "test.output@1",
                "ok": true,
            })
        );
    }

    #[test_r::test]
    fn render_cli_output_document_rejects_reserved_type_field() {
        #[derive(Serialize)]
        struct BadOutput {
            #[serde(rename = "$type")]
            output_type: String,
        }

        impl CliOutput for BadOutput {
            const KIND: &'static str = "test.bad-output";
        }

        let error = render_cli_output_document(
            Format::Json,
            false,
            &BadOutput {
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
