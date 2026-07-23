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

/// A minimal indentation-aware line writer for emitting MoonBit source.
#[derive(Default)]
pub struct MoonBitWriter {
    lines: Vec<String>,
    indent: usize,
}

impl MoonBitWriter {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            indent: 0,
        }
    }

    pub fn indent(&mut self) {
        self.indent += 1;
    }

    pub fn dedent(&mut self) {
        if self.indent > 0 {
            self.indent -= 1;
        }
    }

    /// Writes one or more lines at the current indentation. A multi-line input
    /// is split on `\n`; empty lines are kept empty (no trailing whitespace).
    pub fn line(&mut self, content: impl AsRef<str>) {
        let prefix = "  ".repeat(self.indent);
        for line in content.as_ref().split('\n') {
            if line.is_empty() {
                self.lines.push(String::new());
            } else {
                self.lines.push(format!("{prefix}{line}"));
            }
        }
    }

    /// Writes a block of documentation as a MoonBit `///` doc comment, if
    /// non-empty.
    pub fn doc(&mut self, doc: &str) {
        let doc = doc.trim_end();
        if doc.is_empty() {
            return;
        }
        for line in doc.lines() {
            if line.trim().is_empty() {
                self.line("///");
            } else {
                self.line(format!("/// {line}"));
            }
        }
    }

    pub fn blank(&mut self) {
        self.lines.push(String::new());
    }

    pub fn finish(mut self) -> String {
        // Collapse trailing blank lines into a single trailing newline.
        while self.lines.last().is_some_and(|line| line.is_empty()) {
            self.lines.pop();
        }
        let mut result = self.lines.join("\n");
        result.push('\n');
        result
    }
}
