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

use colored::Colorize;
use itertools::Itertools;
use std::fmt::Display;

#[derive(Debug, Clone)]
pub struct AppValidationError {
    pub message: String,
    pub warns: Vec<String>,
    pub errors: Vec<String>,
}

impl Display for AppValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let warns = format_warns(&self.warns);
        let errors = format_errors(&self.errors);

        write!(f, "\n{}{}\n{}", warns, errors, &self.message)
    }
}

impl std::error::Error for AppValidationError {}

pub fn format_warns(warns: &[String]) -> String {
    let label = "warning:".yellow().bold().to_string();
    warns
        .iter()
        .map(|msg| ensure_ends_with_empty_new_line(format_message_with_level(&label, msg)))
        .join("")
}

pub fn format_errors(errors: &[String]) -> String {
    let label = "error:".red().bold().to_string();
    errors
        .iter()
        .map(|msg| ensure_ends_with_empty_new_line(format_message_with_level(&label, msg)))
        .join("")
}

fn format_message_with_level(level: &str, message: &str) -> String {
    if message.contains("\n") {
        format!(
            "{}\n{}",
            level,
            message.lines().map(|s| format!("  {s}")).join("\n")
        )
    } else {
        format!("{level} {message}")
    }
}

fn ensure_ends_with_empty_new_line(str: String) -> String {
    if str.ends_with("\n\n") {
        str
    } else if str.ends_with('\n') {
        str + "\n"
    } else {
        str + "\n\n"
    }
}

pub enum CustomCommandError {
    CommandNotFound,
    CommandError { error: anyhow::Error },
}
