// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use colored::Colorize;
use itertools::Itertools;
use std::fmt::{Display, Write};

#[derive(Debug, Clone)]
pub struct AppValidationError {
    pub message: String,
    pub warns: Vec<String>,
    pub errors: Vec<String>,
}

impl Display for AppValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn with_new_line_if_not_empty(mut str: String) -> String {
            if !str.is_empty() {
                str.write_char('\n').unwrap()
            }
            str
        }

        let warns = with_new_line_if_not_empty(format_warns(&self.warns));
        let errors = with_new_line_if_not_empty(format_errors(&self.errors));

        write!(f, "\n{}{}\n{}", warns, errors, &self.message)
    }
}

impl std::error::Error for AppValidationError {}

pub fn format_warns(warns: &[String]) -> String {
    let label = "warning".yellow();
    warns
        .iter()
        .map(|warn| format!("{}: {}", label, warn))
        .join("\n")
}

pub fn format_errors(errors: &[String]) -> String {
    let label = "error".red();
    errors
        .iter()
        .map(|error| format!("{}: {}", label, error))
        .join("\n")
}

pub enum CustomCommandError {
    CommandNotFound,
    CommandError { error: anyhow::Error },
}
