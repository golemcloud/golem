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

use crate::context::Context;
use crate::log::logln;
use crate::model::format::Format;
use crate::model::text::fmt::{to_colored_json, to_colored_yaml, NestedTextViewIndent, TextView};
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

    pub fn nested_text_view_indent(&self) -> NestedTextViewIndent {
        NestedTextViewIndent::new(self.ctx.format())
    }
}
