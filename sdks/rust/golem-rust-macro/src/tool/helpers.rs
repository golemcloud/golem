// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Small literal-extraction helpers shared by the tool attribute parsers.

use std::collections::BTreeSet;
use syn::spanned::Spanned;
use syn::{Error, Expr, ExprArray, ExprLit, Lit, Token};

/// Tracks the keyword keys seen within a single helper attribute so that a
/// repeated key produces a clean compile error instead of silently keeping the
/// last value. Every kwarg-style tool parser (`#[arg]`, `#[command]`,
/// `#[result]`, `#[example]`, `#[tool_error]`) records each key through this.
#[derive(Default)]
pub struct SeenKeys(BTreeSet<String>);

impl SeenKeys {
    /// Records `key`, returning a `duplicate key` error on its second occurrence.
    pub fn insert(&mut self, key: &syn::Ident) -> Result<(), Error> {
        if !self.0.insert(key.to_string()) {
            return Err(Error::new(key.span(), format!("duplicate key `{key}`")));
        }
        Ok(())
    }
}

/// Returns `true` if `expr` is a metadata-time literal: a literal, a negated
/// numeric literal, or an array/tuple/parenthesized group built only from such
/// literals. These are the only forms that can be interpreted into a schema
/// value at metadata-synthesis time, used by `#[arg(default = …)]` and the
/// literal side of a `value_is(…)` constraint ref.
pub fn is_metadata_literal(expr: &Expr) -> bool {
    match expr {
        // Only the literal kinds that map onto a schema value: string, integer,
        // float, bool, and char. Byte strings, byte, and C-string literals are
        // not supported metadata literals.
        Expr::Lit(ExprLit { lit, .. }) => matches!(
            lit,
            Lit::Str(_) | Lit::Int(_) | Lit::Float(_) | Lit::Bool(_) | Lit::Char(_)
        ),
        // Unary negation is only meaningful on a numeric literal (`-5`, `-1.5`);
        // `-"x"` / `-true` are not literals.
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => matches!(
            &*u.expr,
            Expr::Lit(ExprLit {
                lit: Lit::Int(_) | Lit::Float(_),
                ..
            })
        ),
        Expr::Group(g) => is_metadata_literal(&g.expr),
        Expr::Paren(p) => is_metadata_literal(&p.expr),
        Expr::Array(a) => a.elems.iter().all(is_metadata_literal),
        Expr::Tuple(t) => t.elems.iter().all(is_metadata_literal),
        _ => false,
    }
}

/// Errors unless `expr` is a metadata-time literal (see [`is_metadata_literal`]).
pub fn require_metadata_literal(expr: &Expr, what: &str) -> Result<(), Error> {
    if is_metadata_literal(expr) {
        Ok(())
    } else {
        Err(Error::new(
            expr.span(),
            format!(
                "{what} must be a literal value (string, number, bool, char, or an array/tuple of literals)"
            ),
        ))
    }
}

/// Extracts a string literal value from an expression.
pub fn expr_str(expr: &Expr, what: &str) -> Result<String, Error> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.value()),
        other => Err(Error::new(
            other.span(),
            format!("{what} must be a string literal"),
        )),
    }
}

/// Extracts a boolean literal value from an expression.
pub fn expr_bool(expr: &Expr, what: &str) -> Result<bool, Error> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Bool(b), ..
        }) => Ok(b.value),
        other => Err(Error::new(
            other.span(),
            format!("{what} must be a boolean literal"),
        )),
    }
}

/// Extracts a `char` literal value from an expression.
pub fn expr_char(expr: &Expr, what: &str) -> Result<char, Error> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Char(c), ..
        }) => Ok(c.value()),
        other => Err(Error::new(
            other.span(),
            format!("{what} must be a character literal"),
        )),
    }
}

/// Extracts a non-negative integer literal value from an expression.
pub fn expr_u32(expr: &Expr, what: &str) -> Result<u32, Error> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse::<u32>(),
        other => Err(Error::new(
            other.span(),
            format!("{what} must be a non-negative integer literal"),
        )),
    }
}

/// Extracts a `u8` integer literal value from an expression.
pub fn expr_u8(expr: &Expr, what: &str) -> Result<u8, Error> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse::<u8>(),
        other => Err(Error::new(
            other.span(),
            format!("{what} must be an integer literal in 0..=255"),
        )),
    }
}

/// Extracts an array of string literals (`["a", "b"]`) from an expression.
pub fn expr_str_array(expr: &Expr, what: &str) -> Result<Vec<String>, Error> {
    match expr {
        Expr::Array(ExprArray { elems, .. }) => elems
            .iter()
            .map(|e| expr_str(e, &format!("each entry of {what}")))
            .collect(),
        other => Err(Error::new(
            other.span(),
            format!("{what} must be an array of string literals"),
        )),
    }
}

/// Parses the comma-separated argument list of a helper attribute
/// (`#[arg(...)]`, `#[command(...)]`, ...) into a sequence of expressions.
pub fn parse_attr_exprs(
    attr: &syn::Attribute,
) -> Result<syn::punctuated::Punctuated<Expr, Token![,]>, Error> {
    let parser = syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated;
    attr.parse_args_with(parser)
}

/// Converts a Rust identifier (`snake_case`, `camelCase`, or `PascalCase`) to
/// the canonical kebab-case used for every tool-facing name, matching the WIT
/// identifier regex `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`. Acronym runs collapse
/// (`HTTPServer` -> `http-server`) and underscores become single dashes.
pub fn to_kebab_case(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            continue;
        }
        if c.is_ascii_uppercase() {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = chars.get(i + 1).copied();
            let boundary = match (prev, next) {
                (Some(p), _) if p.is_ascii_lowercase() || p.is_ascii_digit() => true,
                (Some(p), Some(n)) if p.is_ascii_uppercase() && n.is_ascii_lowercase() => true,
                _ => false,
            };
            if boundary && !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::to_kebab_case;

    #[test]
    fn kebab_cases() {
        assert_eq!(to_kebab_case("Grep"), "grep");
        assert_eq!(to_kebab_case("BadPattern"), "bad-pattern");
        assert_eq!(to_kebab_case("case_sensitive"), "case-sensitive");
        assert_eq!(to_kebab_case("HTTPServer"), "http-server");
        assert_eq!(to_kebab_case("parseHTML"), "parse-html");
        assert_eq!(to_kebab_case("remote"), "remote");
        assert_eq!(to_kebab_case("log2"), "log2");
    }
}
