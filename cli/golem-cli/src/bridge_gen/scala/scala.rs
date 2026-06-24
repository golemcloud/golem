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

/// Returns true if `name` is a plain (non-backticked) Scala identifier: a
/// leading letter or underscore followed by letters, digits, or underscores.
///
/// This is deliberately conservative — it only accepts the ASCII alphanumeric
/// form the generators actually emit, not the full Unicode / operator
/// identifier grammar Scala allows. Anything outside this shape is escaped with
/// backticks by [`escape_scala_ident`].
pub fn is_valid_scala_ident(name: &str) -> bool {
    // A bare underscore is the wildcard/discard symbol in Scala, not a usable
    // referenceable name, so it must be backtick-escaped.
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

/// Escapes an arbitrary name into a usable Scala term identifier.
///
/// A name that is already a valid plain identifier and is not a reserved word is
/// returned verbatim. Everything else (reserved words, names containing illegal
/// characters such as `-`, names starting with a digit, …) is wrapped in
/// backticks, which Scala accepts as a literal identifier. Any backticks present
/// in the input are stripped first so the escaping cannot be broken out of.
pub fn escape_scala_ident(name: &str) -> String {
    if name.is_empty() {
        return "`_`".to_string();
    }
    if is_valid_scala_ident(name) && !is_scala_keyword(name) {
        return name.to_string();
    }
    let sanitized = name.replace('`', "");
    let sanitized = if sanitized.is_empty() {
        "_"
    } else {
        &sanitized
    };
    format!("`{sanitized}`")
}

/// Converts a source name into a Scala term identifier (parameter, field, or
/// method name).
///
/// When `same_language` is true the name is already in Scala's native
/// lowerCamelCase, so only keyword/illegal-character escaping is applied.
/// Otherwise the name may originate from WIT (kebab-case) or another language,
/// so it is converted to lowerCamelCase first.
pub fn to_scala_term_ident(name: &str, same_language: bool) -> String {
    if same_language {
        escape_scala_ident(name)
    } else {
        escape_scala_ident(&name.to_lower_camel_case())
    }
}

/// Converts a source name into a Scala type identifier (class, trait, or type
/// alias name).
///
/// When `same_language` is true the name is already in Scala's native
/// UpperCamelCase, so only keyword/illegal-character escaping is applied.
/// Otherwise the name may originate from WIT (kebab-case) or another language,
/// so it is converted to UpperCamelCase first.
pub fn to_scala_type_ident(name: &str, same_language: bool) -> String {
    if same_language {
        escape_scala_ident(name)
    } else {
        escape_scala_ident(&name.to_upper_camel_case())
    }
}

/// Disambiguates a list of already-escaped Scala identifiers so that each is
/// unique within the list, appending `_2`, `_3`, … to later duplicates. The
/// first occurrence keeps its name.
///
/// Distinct schema names can normalize to the same Scala identifier (e.g.
/// `foo-bar` and `foo_bar` both become `fooBar`); this keeps the generated
/// record fields / case names compilable. Wire encoding is always positional
/// (record field order, variant/enum case index, union tag), so renaming a
/// generated member never changes the wire format.
pub fn unique_idents(idents: Vec<String>) -> Vec<String> {
    unique_idents_with_reserved(idents, &[])
}

/// Like [`unique_idents`], but additionally renames any identifier that would
/// collide with one of the `reserved` names (e.g. internal locals, helper
/// methods, or inherited trait members emitted alongside the user identifiers).
/// The reserved names themselves are never emitted; only the user identifiers
/// are returned, disambiguated away from both each other and the reserved set.
///
/// Collision detection is on the *semantic* identifier, i.e. with any
/// surrounding backtick escaping stripped, so a backtick-escaped name such as
/// `` `ec` `` is recognized as the same Scala symbol as the plain `ec` (and as
/// the reserved `ec`). Reserved names may be given either escaped or unescaped.
pub fn unique_idents_with_reserved(idents: Vec<String>, reserved: &[&str]) -> Vec<String> {
    let mut used: std::collections::HashSet<String> =
        reserved.iter().map(|s| scala_ident_key(s)).collect();
    let mut result = Vec::with_capacity(idents.len());
    for ident in idents {
        let mut candidate = ident.clone();
        let mut n = 2;
        while used.contains(&scala_ident_key(&candidate)) {
            candidate = disambiguate_ident(&ident, n);
            n += 1;
        }
        used.insert(scala_ident_key(&candidate));
        result.push(candidate);
    }
    result
}

/// The semantic identifier denoted by a rendered Scala identifier: the name
/// with any surrounding backtick escaping removed. Plain identifiers (and bare
/// reserved-word strings) are returned unchanged. Two rendered identifiers
/// denote the same Scala symbol iff their keys are equal.
fn scala_ident_key(rendered: &str) -> String {
    rendered
        .strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .unwrap_or(rendered)
        .to_string()
}

/// Appends a numeric suffix to an (escaped) identifier, placing the suffix
/// inside the backticks when the identifier is backtick-escaped.
fn disambiguate_ident(escaped: &str, n: usize) -> String {
    if escaped.len() >= 2 && escaped.starts_with('`') && escaped.ends_with('`') {
        format!("`{}_{n}`", &escaped[1..escaped.len() - 1])
    } else {
        format!("{escaped}_{n}")
    }
}

/// Reserved words to backtick-escape in generated Scala source.
///
/// The set is the union of the Scala 2.13 and Scala 3 hard keywords plus the
/// Scala 3 soft/contextual keywords. Soft keywords are only contextually
/// reserved, but backtick-escaping them is always safe and keeps the generated
/// code parser-proof under both dialects of the cross-build, so they are
/// included to avoid context-sensitive surprises.
pub fn is_scala_keyword(name: &str) -> bool {
    SCALA_KEYWORDS.contains(&name)
}

const SCALA_KEYWORDS: &[&str] = &[
    // Scala 2.13 / Scala 3 hard keywords
    "abstract",
    "as",
    "case",
    "catch",
    "class",
    "def",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "final",
    "finally",
    "for",
    "forSome",
    "given",
    "if",
    "implicit",
    "import",
    "lazy",
    "macro",
    "match",
    "new",
    "null",
    "object",
    "override",
    "package",
    "private",
    "protected",
    "return",
    "sealed",
    "super",
    "then",
    "this",
    "throw",
    "trait",
    "true",
    "try",
    "type",
    "val",
    "var",
    "while",
    "with",
    "yield",
    // Scala 3 soft / contextual keywords (escaped defensively)
    "derives",
    "end",
    "extension",
    "infix",
    "inline",
    "opaque",
    "open",
    "transparent",
    "using",
];
