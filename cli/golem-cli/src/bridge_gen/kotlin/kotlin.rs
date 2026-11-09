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

use heck::{ToLowerCamelCase, ToUpperCamelCase};

/// Returns true if `name` is a plain (non-backticked) Kotlin identifier: a
/// leading ASCII letter or underscore followed by ASCII letters, digits or
/// underscores, and not a lone underscore.
pub fn is_valid_kotlin_ident(name: &str) -> bool {
    if name == "_" {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Escapes an arbitrary name into a usable Kotlin identifier.
///
/// A plain identifier that is not a hard keyword is returned verbatim. A hard
/// keyword is wrapped in backticks. Any other name has every character outside
/// `[A-Za-z0-9_]` replaced by `_`, and a leading digit (or an empty result) is
/// prefixed with `_`, so the result never needs a backtick that the JVM or
/// Wasm back ends would reject.
pub fn escape_kotlin_ident(name: &str) -> String {
    if is_valid_kotlin_ident(name) {
        return if is_kotlin_keyword(name) {
            format!("`{name}`")
        } else {
            name.to_string()
        };
    }
    let mut sanitized: String = name
        .chars()
        .map(|c| {
            if c == '_' || c.is_ascii_alphanumeric() {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() || sanitized.starts_with(|c: char| c.is_ascii_digit()) {
        sanitized.insert(0, '_');
    }
    if sanitized == "_" {
        sanitized = "__".to_string();
    }
    if is_kotlin_keyword(&sanitized) {
        format!("`{sanitized}`")
    } else {
        sanitized
    }
}

/// Converts a source name into a Kotlin term identifier (parameter, property or
/// function name). When `same_language` is true the name is already in
/// Kotlin's lowerCamelCase, so only escaping applies.
pub fn to_kotlin_term_ident(name: &str, same_language: bool) -> String {
    if same_language {
        escape_kotlin_ident(name)
    } else {
        escape_kotlin_ident(&name.to_lower_camel_case())
    }
}

/// Converts a source name into a Kotlin type identifier (class or object name).
pub fn to_kotlin_type_ident(name: &str, same_language: bool) -> String {
    if same_language {
        escape_kotlin_ident(name)
    } else {
        escape_kotlin_ident(&name.to_upper_camel_case())
    }
}

/// Disambiguates already-escaped identifiers so that each is unique within the
/// list, appending `_2`, `_3`, … to later duplicates. The first occurrence
/// keeps its name. Wire encoding is positional, so renaming a generated member
/// never changes the wire format.
pub fn unique_idents(idents: Vec<String>) -> Vec<String> {
    unique_idents_with_reserved(idents, &[])
}

/// Like [`unique_idents`], but additionally renames any identifier that
/// collides with one of the `reserved` names (generated helpers, locals and
/// members of synthesized classes). Collisions are detected on the semantic
/// identifier, with surrounding backticks removed.
pub fn unique_idents_with_reserved(idents: Vec<String>, reserved: &[&str]) -> Vec<String> {
    let mut used: std::collections::HashSet<String> =
        reserved.iter().map(|s| kotlin_ident_key(s)).collect();
    let mut result = Vec::with_capacity(idents.len());
    for ident in idents {
        let mut candidate = ident.clone();
        let mut n = 2;
        while used.contains(&kotlin_ident_key(&candidate)) {
            candidate = disambiguate_ident(&ident, n);
            n += 1;
        }
        used.insert(kotlin_ident_key(&candidate));
        result.push(candidate);
    }
    result
}

fn kotlin_ident_key(rendered: &str) -> String {
    rendered
        .strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .unwrap_or(rendered)
        .to_string()
}

fn disambiguate_ident(escaped: &str, n: usize) -> String {
    if escaped.len() >= 2 && escaped.starts_with('`') && escaped.ends_with('`') {
        // The suffix makes a keyword a plain identifier, so the backticks go.
        format!("{}_{n}", &escaped[1..escaped.len() - 1])
    } else {
        format!("{escaped}_{n}")
    }
}

/// Reserved words that are backtick-escaped in generated Kotlin source: the
/// hard keywords (always reserved) and `typeof`.
pub fn is_kotlin_keyword(name: &str) -> bool {
    KOTLIN_KEYWORDS.contains(&name)
}

const KOTLIN_KEYWORDS: &[&str] = &[
    "as",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "when",
    "while",
];

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn hard_keywords_are_backticked() {
        assert_eq!(escape_kotlin_ident("class"), "`class`");
        assert_eq!(escape_kotlin_ident("in"), "`in`");
        assert_eq!(escape_kotlin_ident("name"), "name");
        assert_eq!(escape_kotlin_ident("value"), "value");
    }

    #[test]
    fn illegal_characters_become_underscores_not_backticks() {
        assert_eq!(escape_kotlin_ident("foo-bar"), "foo_bar");
        assert_eq!(escape_kotlin_ident("a.b"), "a_b");
        assert_eq!(escape_kotlin_ident("1st"), "_1st");
        assert_eq!(escape_kotlin_ident(""), "__");
        assert_eq!(escape_kotlin_ident("_"), "__");
    }

    #[test]
    fn case_conversion_applies_only_across_languages() {
        assert_eq!(to_kotlin_term_ident("do-thing", false), "doThing");
        assert_eq!(to_kotlin_term_ident("doThing", true), "doThing");
        assert_eq!(to_kotlin_type_ident("my-type", false), "MyType");
        assert_eq!(to_kotlin_term_ident("fun", false), "`fun`");
    }

    #[test]
    fn duplicates_and_reserved_names_get_numeric_suffixes() {
        let idents = unique_idents_with_reserved(
            vec!["a".into(), "a".into(), "copy".into(), "`class`".into()],
            &["copy"],
        );
        assert_eq!(idents, vec!["a", "a_2", "copy_2", "`class`"]);
        assert_eq!(
            unique_idents_with_reserved(vec!["`class`".into(), "`class`".into()], &[]),
            vec!["`class`", "class_2"]
        );
    }
}
