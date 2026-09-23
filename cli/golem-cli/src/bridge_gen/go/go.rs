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

//! Go identifiers.
//!
//! Two things make Go different from the other bridge targets. Its reserved set
//! is not just the 25 keywords: the predeclared identifiers (`any`, `error`,
//! `string`, `len`, `new`, `nil`, …) are ordinary names that can be shadowed,
//! so shadowing one is legal but silently takes the name away from the
//! generated code that needs it. And exportedness is spelled in the casing, so
//! whether a name is visible outside its package is decided here rather than by
//! a keyword.
//!
//! Wire encoding is positional — record field order, variant and enum case
//! index, union tag — so renaming a generated member never changes what travels.

use heck::{ToLowerCamelCase, ToUpperCamelCase};

/// The 25 Go keywords. A keyword cannot be used as an identifier at all.
const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

/// Go's predeclared identifiers. These are not keywords — shadowing one
/// compiles — but a generated file that names a local `len` or a field `error`
/// loses the builtin for the rest of that scope, which breaks code emitted
/// beside it. They are escaped rather than risked.
const PREDECLARED: &[&str] = &[
    // Types
    "any", "bool", "byte", "comparable", "complex64", "complex128", "error", "float32", "float64",
    "int", "int8", "int16", "int32", "int64", "rune", "string", "uint", "uint8", "uint16",
    "uint32", "uint64", "uintptr", // Constants
    "true", "false", "iota", // Zero value
    "nil", // Functions
    "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make", "max",
    "min", "new", "panic", "print", "println", "real", "recover",
];

/// True when `name` cannot be used as an identifier, or would shadow something
/// the generated code relies on.
pub fn is_reserved(name: &str) -> bool {
    KEYWORDS.contains(&name) || PREDECLARED.contains(&name)
}

/// An exported Go identifier: `UpperCamelCase`, which is what makes it visible
/// outside the generated package.
pub fn to_exported_ident(name: &str) -> String {
    escape(&upper_camel(name), Case::Exported)
}

/// An unexported Go identifier: `lowerCamelCase`.
pub fn to_unexported_ident(name: &str) -> String {
    escape(&lower_camel(name), Case::Unexported)
}

/// A struct field name. Fields are exported, because the wire codec and the
/// consuming program both live outside the generated package.
pub fn to_field_ident(name: &str) -> String {
    to_exported_ident(name)
}

/// A method parameter or local. These are unexported by definition — they have
/// no package-level visibility — so the only question is escaping.
pub fn to_param_ident(name: &str) -> String {
    to_unexported_ident(name)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Exported,
    Unexported,
}

/// Turns a candidate into a valid identifier of the given case, suffixing a
/// reserved word rather than mangling it, so the name a reader sees still
/// resembles the one the schema declared.
fn escape(candidate: &str, case: Case) -> String {
    // Anything outside Go's identifier alphabet becomes an underscore. A digit
    // is kept wherever it appears, because a leading one is repaired below
    // rather than erased — erasing it would silently merge `2fast` and `fast`.
    let filtered: String = candidate
        .chars()
        .map(|ch| if is_ident_char(ch) { ch } else { '_' })
        .collect();

    let base = if filtered.is_empty() {
        match case {
            Case::Exported => "Field".to_string(),
            Case::Unexported => "field".to_string(),
        }
    } else if filtered.starts_with(|ch: char| ch.is_ascii_digit()) {
        // A leading digit is not a valid start, and prefixing `_` would make an
        // exported name unexported, so the case is restated instead.
        match case {
            Case::Exported => format!("N{filtered}"),
            Case::Unexported => format!("n{filtered}"),
        }
    } else {
        filtered
    };

    // Only an unexported name can collide: every keyword and predeclared
    // identifier is lowercase, so an exported name is safe by construction.
    if is_reserved(&base) {
        format!("{base}_")
    } else {
        base
    }
}

fn is_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn upper_camel(name: &str) -> String {
    name.to_upper_camel_case()
}

fn lower_camel(name: &str) -> String {
    name.to_lower_camel_case()
}

/// Disambiguates already-escaped identifiers so each is unique within the list,
/// appending `2`, `3`, … to later duplicates. The first occurrence keeps its
/// name.
///
/// Distinct schema names normalize onto the same Go identifier more often than
/// in the other targets, because Go drops the separator: `first-name`,
/// `first_name` and `firstName` all become `FirstName`.
pub fn unique_idents(idents: Vec<String>) -> Vec<String> {
    unique_idents_with_reserved(idents, &[])
}

/// Like [`unique_idents`], but also renames anything colliding with `reserved`
/// — the locals and helpers emitted beside the user identifiers. The reserved
/// names are not returned.
pub fn unique_idents_with_reserved(idents: Vec<String>, reserved: &[&str]) -> Vec<String> {
    let mut used: std::collections::HashSet<String> =
        reserved.iter().map(|s| s.to_string()).collect();
    let mut result = Vec::with_capacity(idents.len());
    for ident in idents {
        let mut candidate = ident.clone();
        let mut n = 2;
        while used.contains(&candidate) {
            candidate = format!("{ident}{n}");
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
    fn schema_names_become_go_casing() {
        assert_eq!(to_exported_ident("first-name"), "FirstName");
        assert_eq!(to_exported_ident("first_name"), "FirstName");
        assert_eq!(to_exported_ident("firstName"), "FirstName");
        assert_eq!(to_param_ident("first-name"), "firstName");
        assert_eq!(to_param_ident("FirstName"), "firstName");
    }

    #[test]
    fn keywords_are_escaped() {
        assert_eq!(to_param_ident("type"), "type_");
        assert_eq!(to_param_ident("range"), "range_");
        assert_eq!(to_param_ident("func"), "func_");
        // An exported name cannot collide: every keyword is lowercase.
        assert_eq!(to_exported_ident("type"), "Type");
    }

    /// Predeclared identifiers are not keywords, so a parameter named `len` or
    /// `error` compiles — and quietly takes the builtin away from the rest of
    /// the function, including code emitted beside it.
    #[test]
    fn predeclared_identifiers_are_escaped() {
        assert_eq!(to_param_ident("error"), "error_");
        assert_eq!(to_param_ident("len"), "len_");
        assert_eq!(to_param_ident("any"), "any_");
        assert_eq!(to_param_ident("nil"), "nil_");
        assert_eq!(to_param_ident("string"), "string_");
        assert_eq!(to_param_ident("new"), "new_");
        assert_eq!(to_exported_ident("error"), "Error");
    }

    #[test]
    fn invalid_characters_and_leading_digits_are_repaired() {
        assert_eq!(to_exported_ident(""), "Field");
        assert_eq!(to_param_ident(""), "field");
        // A leading digit is prefixed rather than dropped, so `2fast` and
        // `fast` stay distinct names.
        assert_eq!(to_exported_ident("2fast"), "N2fast");
        assert_eq!(to_param_ident("2fast"), "n2fast");
        assert_ne!(to_exported_ident("2fast"), to_exported_ident("fast"));
        // A character outside the identifier alphabet becomes an underscore.
        assert_eq!(to_exported_ident("a.b"), "AB");
        assert_eq!(to_exported_ident("+"), "Field");
    }

    /// A leading underscore would make an exported name unexported, which is a
    /// visibility change, not an escaping one.
    #[test]
    fn repairing_a_name_never_changes_its_visibility() {
        for name in ["2fast", "", "9", "123abc"] {
            let exported = to_exported_ident(name);
            assert!(
                exported.starts_with(|ch: char| ch.is_ascii_uppercase()),
                "{name} produced {exported}, which is not exported"
            );
            let unexported = to_param_ident(name);
            assert!(
                unexported.starts_with(|ch: char| ch.is_ascii_lowercase() || ch == '_'),
                "{name} produced {unexported}, which is not unexported"
            );
        }
    }

    /// Go drops the separator, so names that stay distinct in every other
    /// target collide here.
    #[test]
    fn colliding_names_are_disambiguated() {
        let idents = vec![
            to_field_ident("first-name"),
            to_field_ident("first_name"),
            to_field_ident("firstName"),
        ];
        assert_eq!(
            unique_idents(idents),
            vec![
                "FirstName".to_string(),
                "FirstName2".to_string(),
                "FirstName3".to_string()
            ]
        );
    }

    #[test]
    fn reserved_locals_are_avoided() {
        assert_eq!(
            unique_idents_with_reserved(vec!["ctx".into()], &["ctx"]),
            vec!["ctx2".to_string()]
        );
    }
}
