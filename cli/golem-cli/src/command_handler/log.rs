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
use crate::log::logln;
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

    pub fn log_serializable<S: Serialize>(&self, value: &S) {
        match self.ctx.format() {
            Format::Json
            | Format::PrettyJson
            | Format::Yaml
            | Format::PrettyYaml
            | Format::Toon => {
                print_structured_document(self.ctx.format(), self.ctx.should_colorize(), value);
            }
            Format::Text => {
                let formatted = if self.ctx.should_colorize() {
                    to_colored_json(value).unwrap()
                } else {
                    serde_json::to_string_pretty(value).unwrap()
                };
                for line in formatted.lines() {
                    logln(line);
                }
            }
        }
    }

    pub fn log_view<View: TextView + Serialize>(&self, view: &View) {
        match self.ctx.format() {
            Format::Json
            | Format::PrettyJson
            | Format::Yaml
            | Format::PrettyYaml
            | Format::Toon => {
                print_structured_document(self.ctx.format(), self.ctx.should_colorize(), view);
            }
            Format::Text => {
                view.log();
            }
        }
    }

    pub fn render_view_truncated<View: TruncatableTextView + Serialize>(
        &self,
        view: &View,
        max_lines: usize,
    ) -> String {
        match self.ctx.format() {
            Format::Text => view.render_truncated(max_lines, self.ctx.should_colorize()),
            _ => {
                let rendered =
                    render_structured_document(self.ctx.format(), self.ctx.should_colorize(), view);
                truncate_rendered(rendered, max_lines)
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

pub fn render_toon_document<S: Serialize>(value: &S) -> String {
    format!(
        "{TOON_FRAME_START}\n{}\n{TOON_FRAME_END}",
        toon::encode(value).unwrap()
    )
}

pub fn render_structured_document<S: Serialize>(
    format: Format,
    colorize: bool,
    value: &S,
) -> String {
    match format {
        Format::Json => serde_json::to_string(value).unwrap(),
        Format::PrettyJson => {
            if colorize {
                to_colored_json(value).unwrap()
            } else {
                serde_json::to_string_pretty(value).unwrap()
            }
        }
        Format::Yaml => format!("---\n{}", serde_yaml::to_string(value).unwrap()),
        Format::PrettyYaml => {
            if colorize {
                format!("---\n{}", to_colored_yaml(value).unwrap())
            } else {
                format!("---\n{}", serde_yaml::to_string(value).unwrap())
            }
        }
        Format::Toon => render_toon_document(value),
        Format::Text => unreachable!("text is not a structured output format"),
    }
}

pub fn print_structured_document<S: Serialize>(format: Format, colorize: bool, value: &S) {
    println!("{}", render_structured_document(format, colorize, value));
}

#[cfg(test)]
mod tests {
    use super::{TOON_FRAME_END, TOON_FRAME_START, render_toon_document};
    use serde_json::json;

    #[test_r::test]
    fn render_toon_document_wraps_payload_in_frame_markers() {
        let rendered = render_toon_document(&json!({ "status": "ok" }));

        assert!(rendered.starts_with(TOON_FRAME_START));
        assert!(rendered.ends_with(TOON_FRAME_END));
        assert!(rendered.contains("status: ok"));
    }
}
