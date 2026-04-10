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
use serde::Serialize;
use std::sync::Arc;

pub struct LogHandler {
    ctx: Arc<Context>,
}

impl LogHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn log_serializable<S: Serialize>(&self, value: &S) {
        match self.ctx.format() {
            Format::Json => {
                println!("{}", serde_json::to_string(value).unwrap());
            }
            Format::PrettyJson => {
                if self.ctx.should_colorize() {
                    println!("{}", to_colored_json(value).unwrap());
                } else {
                    println!("{}", serde_json::to_string_pretty(value).unwrap());
                }
            }
            Format::Yaml => {
                println!("---\n{}", serde_yaml::to_string(value).unwrap());
            }
            Format::PrettyYaml => {
                if self.ctx.should_colorize() {
                    println!("---\n{}", to_colored_yaml(value).unwrap());
                } else {
                    println!("---\n{}", serde_yaml::to_string(value).unwrap());
                }
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
            Format::Json => {
                println!("{}", serde_json::to_string(view).unwrap());
            }
            Format::PrettyJson => {
                if self.ctx.should_colorize() {
                    println!("{}", to_colored_json(view).unwrap());
                } else {
                    println!("{}", serde_json::to_string_pretty(view).unwrap());
                }
            }
            Format::Yaml => {
                println!("---\n{}", serde_yaml::to_string(view).unwrap());
            }
            Format::PrettyYaml => {
                if self.ctx.should_colorize() {
                    println!("---\n{}", to_colored_yaml(view).unwrap());
                } else {
                    println!("---\n{}", serde_yaml::to_string(view).unwrap());
                }
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
                let rendered = match self.ctx.format() {
                    Format::Json => serde_json::to_string(view).unwrap(),
                    Format::PrettyJson => {
                        if self.ctx.should_colorize() {
                            to_colored_json(view).unwrap()
                        } else {
                            serde_json::to_string_pretty(view).unwrap()
                        }
                    }
                    Format::Yaml => format!("---\n{}", serde_yaml::to_string(view).unwrap()),
                    Format::PrettyYaml => {
                        if self.ctx.should_colorize() {
                            format!("---\n{}", to_colored_yaml(view).unwrap())
                        } else {
                            format!("---\n{}", serde_yaml::to_string(view).unwrap())
                        }
                    }
                    Format::Text => unreachable!(),
                };
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
