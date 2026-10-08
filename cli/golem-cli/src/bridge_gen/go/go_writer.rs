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

//! An indentation-aware line writer for emitting Go source.
//!
//! Go makes one demand the other targets do not: an unused import is a compile
//! error, not a warning, and imports must be declared before any code. So the
//! writer collects them as the body is written and emits the block when the
//! file is assembled — a generator that emits a type conditionally cannot know
//! up front which packages that type will need.

use std::collections::BTreeMap;

/// One import: the package path, and the alias it is bound to when the path's
/// last segment is not the package name, or when two paths would otherwise
/// bind the same name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Import {
    path: String,
    alias: Option<String>,
}

#[derive(Default)]
pub struct GoWriter {
    lines: Vec<String>,
    indent: usize,
    /// Keyed by path so importing the same package twice is idempotent, which
    /// lets each emitter declare what it needs without coordinating.
    imports: BTreeMap<String, Import>,
}

impl GoWriter {
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

    /// Records that the emitted code needs `path`, bound to its own package
    /// name. Importing the same path again is a no-op.
    pub fn import(&mut self, path: impl Into<String>) {
        let path = path.into();
        self.imports
            .entry(path.clone())
            .or_insert(Import { path, alias: None });
    }

    /// Records an import bound to an explicit name. Used where the path's last
    /// segment is not the package name, or would read ambiguously — the guest
    /// SDK's generated bindings, for instance.
    pub fn import_as(&mut self, alias: impl Into<String>, path: impl Into<String>) {
        let path = path.into();
        let alias = alias.into();
        self.imports.insert(
            path.clone(),
            Import {
                path,
                alias: Some(alias),
            },
        );
    }

    /// Writes one or more lines at the current indentation. A multi-line input
    /// is split on `\n`, and an empty line stays empty rather than becoming
    /// trailing whitespace — gofmt would strip it, and a diff against gofmt
    /// output is how the generator is tested.
    pub fn line(&mut self, content: impl AsRef<str>) {
        let prefix = "\t".repeat(self.indent);
        for line in content.as_ref().split('\n') {
            if line.is_empty() {
                self.lines.push(String::new());
            } else {
                self.lines.push(format!("{prefix}{line}"));
            }
        }
    }

    /// Writes a Go doc comment, if non-empty. Go has no separate doc syntax:
    /// a `//` comment directly above a declaration is its documentation.
    pub fn doc(&mut self, doc: &str) {
        let doc = doc.trim_end();
        if doc.is_empty() {
            return;
        }
        for line in doc.lines() {
            if line.trim().is_empty() {
                self.line("//");
            } else {
                self.line(format!("// {line}"));
            }
        }
    }

    pub fn blank(&mut self) {
        self.lines.push(String::new());
    }

    /// Assembles the file: licence header, package clause, import block, body.
    pub fn finish(mut self, package: &str) -> String {
        while self.lines.last().is_some_and(|line| line.is_empty()) {
            self.lines.pop();
        }

        let mut out = String::from(GENERATED_HEADER);
        out.push_str(&format!("package {package}\n"));

        if !self.imports.is_empty() {
            out.push('\n');
            out.push_str(&self.import_block());
        }

        if !self.lines.is_empty() {
            out.push('\n');
            out.push_str(&self.lines.join("\n"));
            out.push('\n');
        }
        out
    }

    /// The import block, standard library first and third-party after, split by
    /// a blank line — the grouping gofmt preserves and every Go reader expects.
    fn import_block(&self) -> String {
        let (std, other): (Vec<_>, Vec<_>) = self
            .imports
            .values()
            .partition(|import| is_std_package(&import.path));

        if std.len() + other.len() == 1 {
            let only = std.first().or_else(|| other.first()).expect("non-empty");
            return format!("import {}\n", render_import(only));
        }

        let mut out = String::from("import (\n");
        for import in &std {
            out.push_str(&format!("\t{}\n", render_import(import)));
        }
        if !std.is_empty() && !other.is_empty() {
            out.push('\n');
        }
        for import in &other {
            out.push_str(&format!("\t{}\n", render_import(import)));
        }
        out.push_str(")\n");
        out
    }
}

fn render_import(import: &Import) -> String {
    match &import.alias {
        Some(alias) => format!("{alias} \"{}\"", import.path),
        None => format!("\"{}\"", import.path),
    }
}

/// A standard library path has no dot in its first segment — the rule the Go
/// toolchain itself uses to tell a module path from a standard one.
fn is_std_package(path: &str) -> bool {
    !path.split('/').next().unwrap_or_default().contains('.')
}

/// The marker `gofmt`, `go vet`, linters and reviewers all recognise. It must
/// match Go's convention exactly — `^// Code generated .* DO NOT EDIT\.$` — or
/// tooling treats the file as hand-written.
const GENERATED_HEADER: &str = "\
// Code generated by golem-cli. DO NOT EDIT.

";

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn a_file_has_a_generated_marker_and_a_package_clause() {
        let mut writer = GoWriter::new();
        writer.line("type Order struct{}");
        assert_eq!(
            writer.finish("client"),
            "// Code generated by golem-cli. DO NOT EDIT.\n\npackage client\n\ntype Order struct{}\n"
        );
    }

    #[test]
    fn indentation_uses_tabs_and_leaves_blank_lines_bare() {
        let mut writer = GoWriter::new();
        writer.line("func f() {");
        writer.indent();
        writer.line("a()\n\nb()");
        writer.dedent();
        writer.line("}");
        assert_eq!(
            writer.finish("client"),
            "// Code generated by golem-cli. DO NOT EDIT.\n\npackage client\n\n\
             func f() {\n\ta()\n\n\tb()\n}\n"
        );
    }

    #[test]
    fn a_single_import_is_written_without_a_block() {
        let mut writer = GoWriter::new();
        writer.import("context");
        writer.line("var _ = context.Background");
        assert!(writer.finish("client").contains("import \"context\"\n"));
    }

    /// The standard library comes first and third-party after, separated by a
    /// blank line: the grouping gofmt preserves.
    #[test]
    fn imports_are_grouped_and_sorted() {
        let mut writer = GoWriter::new();
        writer.import("github.com/golemcloud/golem/sdks/go/bridge");
        writer.import("context");
        writer.import_as("values", "github.com/golemcloud/golem/sdks/go/core/values");
        writer.import("time");
        writer.line("var _ = 1");

        let rendered = writer.finish("client");
        let block = rendered
            .split("import (\n")
            .nth(1)
            .and_then(|rest| rest.split(")\n").next())
            .expect("an import block");
        assert_eq!(
            block,
            "\t\"context\"\n\t\"time\"\n\n\
             \t\"github.com/golemcloud/golem/sdks/go/bridge\"\n\
             \tvalues \"github.com/golemcloud/golem/sdks/go/core/values\"\n"
        );
    }

    /// An emitter declares what it needs without knowing what others declared,
    /// so importing the same package twice has to be harmless — a repeated
    /// import is a compile error in Go.
    #[test]
    fn importing_the_same_package_twice_is_idempotent() {
        let mut writer = GoWriter::new();
        writer.import("context");
        writer.import("context");
        writer.line("var _ = 1");
        assert_eq!(writer.finish("client").matches("\"context\"").count(), 1);
    }

    #[test]
    fn a_file_with_no_body_is_still_valid_go() {
        let writer = GoWriter::new();
        assert_eq!(
            writer.finish("client"),
            "// Code generated by golem-cli. DO NOT EDIT.\n\npackage client\n"
        );
    }

    #[test]
    fn docs_become_line_comments_and_an_empty_doc_writes_nothing() {
        let mut writer = GoWriter::new();
        writer.doc("");
        writer.doc("Order is a line.\n\nIt has two paragraphs.");
        writer.line("type Order struct{}");
        let rendered = writer.finish("client");
        assert!(rendered.contains("// Order is a line.\n//\n// It has two paragraphs.\n"));
    }
}
