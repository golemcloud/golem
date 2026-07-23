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

pub fn escape_js_ident(ident: impl AsRef<str>) -> String {
    let escaped = ident
        .as_ref()
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            if (idx == 0 && is_js_identifier_start(ch)) || (idx > 0 && is_js_identifier_part(ch)) {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    let escaped = if escaped.is_empty() || escaped.starts_with(|ch: char| ch.is_ascii_digit()) {
        format!("_{escaped}")
    } else {
        escaped
    };

    if KEYWORDS.contains(&escaped.as_str()) {
        format!("{escaped}_")
    } else {
        escaped
    }
}

fn is_js_identifier_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_js_identifier_part(ch: char) -> bool {
    is_js_identifier_start(ch) || ch.is_ascii_digit()
}

const KEYWORDS: &[&str] = &[
    "any",
    "await",
    "bigint",
    "break",
    "case",
    "catch",
    "boolean",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "eval",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "never",
    "null",
    "number",
    "object",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "string",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "unknown",
    "var",
    "void",
    "while",
    "with",
    "yield",
];
