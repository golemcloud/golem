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

//! The import paths of a Go module that `go mod tidy` resolves to required
//! modules: every import outside the standard library, from every package of
//! the module.

use std::collections::BTreeSet;
use std::path::Path;
use walkdir::WalkDir;

/// The non-standard-library import paths of the Go module at `module_root`. It
/// skips what the go command ignores: `vendor` and `testdata` directories,
/// names starting with `.` or `_`, and nested modules.
pub fn module_imports(module_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut imports = BTreeSet::new();
    let walker = WalkDir::new(module_root).into_iter().filter_entry(|entry| {
        if entry.depth() == 0 || !entry.file_type().is_dir() {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        !(name.starts_with('.')
            || name.starts_with('_')
            || name == "vendor"
            || name == "testdata"
            || entry.path().join("go.mod").exists())
    });
    for entry in walker {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy();
        if !entry.file_type().is_file()
            || !name.ends_with(".go")
            || name.starts_with('.')
            || name.starts_with('_')
        {
            continue;
        }
        let source = std::fs::read_to_string(entry.path())?;
        imports.extend(
            imports_of(&source)
                .into_iter()
                .filter(|path| is_third_party(path)),
        );
    }
    Ok(imports)
}

/// A standard-library path has no dot in its first element.
fn is_third_party(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|first| first.contains('.'))
}

/// The import paths a Go source file declares, read from its import
/// declarations after the package clause.
pub fn imports_of(source: &str) -> Vec<String> {
    let tokens = tokenize(source);
    let mut imports = Vec::new();
    let mut i = 0;
    while matches!(tokens.get(i), Some(Token::Semi)) {
        i += 1;
    }
    if tokens.get(i) != Some(&Token::Word("package".to_string())) {
        return imports;
    }
    i += 2;
    while matches!(tokens.get(i), Some(Token::Semi)) {
        i += 1;
    }
    while tokens.get(i) == Some(&Token::Word("import".to_string())) {
        i += 1;
        if tokens.get(i) == Some(&Token::Open) {
            i += 1;
            while let Some(token) = tokens.get(i) {
                match token {
                    Token::Close => break,
                    Token::Str(path) => imports.push(path.clone()),
                    _ => {}
                }
                i += 1;
            }
        } else {
            while let Some(token) = tokens.get(i) {
                match token {
                    Token::Str(path) => {
                        imports.push(path.clone());
                        break;
                    }
                    Token::Semi => break,
                    _ => i += 1,
                }
            }
        }
        i += 1;
        while matches!(tokens.get(i), Some(Token::Semi)) {
            i += 1;
        }
    }
    imports
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Str(String),
    Open,
    Close,
    Semi,
    Other,
}

/// A lexer for the head of a Go file: words, string literals, parentheses and
/// statement ends (newlines and semicolons). Comments are skipped.
fn tokenize(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            '\n' | ';' => {
                tokens.push(Token::Semi);
                i += 1;
            }
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            '"' | '`' => {
                let mut value = String::new();
                i += 1;
                while i < chars.len() && chars[i] != c {
                    if c == '"' && chars[i] == '\\' {
                        i += 1;
                    }
                    if let Some(&ch) = chars.get(i) {
                        value.push(ch);
                    }
                    i += 1;
                }
                i += 1;
                tokens.push(Token::Str(value));
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(Token::Word(chars[start..i].iter().collect()));
            }
            c if c.is_whitespace() => i += 1,
            _ => {
                tokens.push(Token::Other);
                i += 1;
            }
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::{imports_of, is_third_party};
    use test_r::test;

    #[test]
    fn reads_single_grouped_named_and_commented_imports() {
        let source = r#"// Package doc, mentioning import "nope".
package main // trailing

/* import "also-nope" */
import "fmt"
import alias "github.com/a/b"
import (
	"net/http"
	_ "example.com/side"
	. `example.org/dot`
	// "example.net/commented"
)

import "after/group"

func main() { _ = "github.com/not/an/import" }
"#;
        assert_eq!(
            imports_of(source),
            vec![
                "fmt",
                "github.com/a/b",
                "net/http",
                "example.com/side",
                "example.org/dot",
                "after/group",
            ]
        );
    }

    #[test]
    fn a_file_without_imports_has_none() {
        assert!(imports_of("package x\n\nfunc F() {}\n").is_empty());
        assert!(imports_of("not go").is_empty());
    }

    #[test]
    fn third_party_paths_have_a_dot_in_the_first_element() {
        assert!(is_third_party("github.com/a/b"));
        assert!(!is_third_party("net/http"));
        assert!(!is_third_party("agent-sdk-go/agents/counter"));
    }
}
