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

/// A minimal indentation-aware line writer for emitting Kotlin source.
#[derive(Default)]
pub struct KotlinWriter {
    lines: Vec<String>,
    indent: usize,
}

impl KotlinWriter {
    pub fn new() -> Self {
        Self::default()
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
        let prefix = "    ".repeat(self.indent);
        for line in content.as_ref().split('\n') {
            if line.is_empty() {
                self.lines.push(String::new());
            } else {
                self.lines.push(format!("{prefix}{line}"));
            }
        }
    }

    /// Writes documentation as a KDoc comment, if non-empty. A `*/` inside the
    /// text would end the comment early, so it is broken up.
    pub fn doc(&mut self, doc: &str) {
        let doc = doc.trim_end();
        if doc.is_empty() {
            return;
        }
        self.line("/**");
        for line in doc.lines() {
            let line = line.replace("*/", "*&#47;").replace("/*", "&#47;*");
            if line.trim().is_empty() {
                self.line(" *");
            } else {
                self.line(format!(" * {line}"));
            }
        }
        self.line(" */");
    }

    pub fn blank(&mut self) {
        self.lines.push(String::new());
    }

    pub fn finish(mut self) -> String {
        while self.lines.last().is_some_and(|line| line.is_empty()) {
            self.lines.pop();
        }
        let mut result = self.lines.join("\n");
        result.push('\n');
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn lines_are_indented_by_four_spaces_and_blank_lines_stay_empty() {
        let mut w = KotlinWriter::new();
        w.line("class A {");
        w.indent();
        w.line("val x = 1\n\nval y = 2");
        w.dedent();
        w.line("}");
        assert_eq!(w.finish(), "class A {\n    val x = 1\n\n    val y = 2\n}\n");
    }

    #[test]
    fn a_comment_terminator_in_documentation_cannot_end_the_comment() {
        let mut w = KotlinWriter::new();
        w.doc("a */ b /* c");
        let out = w.finish();
        assert_eq!(out.matches("*/").count(), 1);
        assert!(!out.contains("/* c"));
    }
}
