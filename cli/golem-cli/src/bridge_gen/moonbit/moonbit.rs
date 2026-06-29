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

use heck::{ToSnakeCase, ToUpperCamelCase};

/// Returns true if `name` is a plain MoonBit term identifier: a leading
/// lowercase letter or underscore followed by letters, digits, or underscores.
///
/// MoonBit term identifiers (fields, parameters, methods, locals) must start
/// with a lowercase letter or underscore. Anything else is sanitized by
/// [`to_moonbit_term_ident`].
pub fn is_valid_term_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || (c.is_ascii_lowercase()) => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Reserved words in MoonBit source. A term identifier equal to one of these is
/// suffixed with `_` so the generated code parses.
pub fn is_moonbit_keyword(name: &str) -> bool {
    MOONBIT_KEYWORDS.contains(&name)
}

const MOONBIT_KEYWORDS: &[&str] = &[
    "as",
    "async",
    "break",
    "const",
    "continue",
    "defer",
    "derive",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "fnalias",
    "for",
    "guard",
    "if",
    "impl",
    "import",
    "in",
    "is",
    "let",
    "letrec",
    "loop",
    "match",
    "mut",
    "priv",
    "pub",
    "raise",
    "return",
    "self",
    "struct",
    "suberror",
    "super",
    "test",
    "then",
    "trait",
    "traitalias",
    "true",
    "try",
    "type",
    "typealias",
    "typeof",
    "unsafe",
    "using",
    "while",
    "with",
];

/// MoonBit builtin / prelude type names a generated type must not take, so it
/// never shadows a builtin in the generated client package. Reserved in the
/// [`TypeNaming`](crate::bridge_gen::type_naming::TypeNaming) walker so colliding
/// generated types are disambiguated by location instead.
pub const RESERVED_TYPE_NAMES: &[&str] = &[
    "Int",
    "Int16",
    "Int64",
    "UInt",
    "UInt16",
    "UInt64",
    "Byte",
    "Bytes",
    "Bool",
    "Char",
    "String",
    "Float",
    "Double",
    "Unit",
    "Array",
    "ArrayView",
    "FixedArray",
    "Map",
    "Set",
    "Option",
    "Result",
    "Json",
    "Iter",
    "Iter2",
    "StringBuilder",
    "Ref",
    "Error",
    "BigInt",
];

/// Builtin sum-type constructor names a generated enum/variant case must not
/// take, so a generated constructor never shadows a prelude constructor.
const RESERVED_CONSTRUCTOR_NAMES: &[&str] = &["Some", "None", "Ok", "Err", "True", "False"];

/// Converts a source name into a MoonBit term identifier (field, parameter,
/// method, or local name).
///
/// When `same_language` is true and the name is already a valid MoonBit term
/// identifier, it is preserved verbatim (only keyword/illegal escaping is
/// applied). Identifiers that originate from WIT (kebab-case, e.g. a `get-shape`
/// method or the `high-bits` field of `uuid`) are not valid MoonBit identifiers
/// even for MoonBit agents, so they still go through `to_snake_case`. When
/// `same_language` is false the name may come from WIT or another language, so
/// `to_snake_case` is always applied.
pub fn to_moonbit_term_ident(name: &str, same_language: bool) -> String {
    let candidate = if same_language && is_valid_term_ident(name) {
        name.to_string()
    } else {
        name.to_snake_case()
    };
    escape_term_ident(&candidate)
}

/// Escapes an arbitrary candidate into a usable MoonBit term identifier: a
/// keyword or otherwise-invalid candidate is suffixed/prefixed so the result is
/// a valid lowercase identifier. Wire encoding is positional (record field
/// order, etc.), so a renamed member never changes the wire format.
pub fn escape_term_ident(candidate: &str) -> String {
    if candidate.is_empty() {
        return "field_".to_string();
    }
    let escaped = if is_valid_term_ident(candidate) {
        candidate.to_string()
    } else {
        // Prefix with underscore if it doesn't start with a valid leading char
        // (e.g. starts with a digit or uppercase); the body chars are already
        // ASCII alphanumeric / underscore after snake_case normalization.
        format!("_{candidate}")
    };
    if is_moonbit_keyword(&escaped) {
        format!("{escaped}_")
    } else {
        escaped
    }
}

/// Converts a source name into a MoonBit constructor identifier (the cases of a
/// generated enum/variant/union).
///
/// Constructor names are UpperCamelCase regardless of source language. A name
/// colliding with a prelude constructor is suffixed so it stays distinct.
pub fn to_moonbit_constructor_ident(name: &str, _same_language: bool) -> String {
    let base = name.to_upper_camel_case();
    let base = if base.is_empty() {
        "Case".to_string()
    } else {
        base
    };
    if RESERVED_CONSTRUCTOR_NAMES.contains(&base.as_str()) {
        format!("{base}_")
    } else {
        base
    }
}

/// Disambiguates a list of already-escaped MoonBit identifiers so that each is
/// unique within the list, appending `_2`, `_3`, … to later duplicates. The
/// first occurrence keeps its name.
///
/// Distinct schema names can normalize to the same MoonBit identifier (e.g.
/// `foo-bar` and `foo_bar` both become `foo_bar`); this keeps the generated
/// record fields / case names compilable. Wire encoding is always positional
/// (record field order, variant/enum case index, union tag), so renaming a
/// generated member never changes the wire format.
pub fn unique_idents(idents: Vec<String>) -> Vec<String> {
    unique_idents_with_reserved(idents, &[])
}

/// Like [`unique_idents`], but additionally renames any identifier that would
/// collide with one of the `reserved` names (internal locals or helper names
/// emitted alongside the user identifiers). The reserved names themselves are
/// never returned; only the user identifiers are returned, disambiguated away
/// from both each other and the reserved set.
pub fn unique_idents_with_reserved(idents: Vec<String>, reserved: &[&str]) -> Vec<String> {
    let mut used: std::collections::HashSet<String> =
        reserved.iter().map(|s| s.to_string()).collect();
    let mut result = Vec::with_capacity(idents.len());
    for ident in idents {
        let mut candidate = ident.clone();
        let mut n = 2;
        while used.contains(&candidate) {
            candidate = format!("{ident}_{n}");
            n += 1;
        }
        used.insert(candidate.clone());
        result.push(candidate);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn keywords_are_escaped() {
        assert_eq!(to_moonbit_term_ident("type", true), "type_");
        assert_eq!(to_moonbit_term_ident("match", true), "match_");
        assert_eq!(to_moonbit_term_ident("foo", true), "foo");
        assert_eq!(to_moonbit_term_ident("first-name", false), "first_name");
        assert_eq!(to_moonbit_term_ident("firstName", false), "first_name");
    }

    #[test]
    fn constructors_are_upper_camel_and_deconflicted() {
        assert_eq!(to_moonbit_constructor_ident("home", false), "Home");
        assert_eq!(
            to_moonbit_constructor_ident("shape-circle", false),
            "ShapeCircle"
        );
        assert_eq!(to_moonbit_constructor_ident("some", false), "Some_");
        assert_eq!(to_moonbit_constructor_ident("ok", false), "Ok_");
    }

    #[test]
    fn unique_idents_disambiguate() {
        assert_eq!(
            unique_idents(vec!["a".into(), "a".into(), "b".into()]),
            vec!["a".to_string(), "a_2".to_string(), "b".to_string()]
        );
        assert_eq!(
            unique_idents_with_reserved(vec!["self".into()], &["self"]),
            vec!["self_2".to_string()]
        );
    }
}
