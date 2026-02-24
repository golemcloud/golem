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

use anyhow::anyhow;
use std::cmp::Reverse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

impl TextEdit {
    pub fn new(start: usize, end: usize, replacement: impl Into<String>) -> Self {
        Self {
            start,
            end,
            replacement: replacement.into(),
        }
    }
}

pub fn apply_edits(source: &str, mut edits: Vec<TextEdit>) -> anyhow::Result<String> {
    if edits.is_empty() {
        return Ok(source.to_string());
    }

    edits.sort_by_key(|edit| Reverse(edit.start));

    let mut output = source.to_string();
    for edit in edits {
        if edit.start > edit.end || edit.end > output.len() {
            return Err(anyhow!("Invalid edit range {}..{}", edit.start, edit.end));
        }
        output.replace_range(edit.start..edit.end, &edit.replacement);
    }

    Ok(output)
}
