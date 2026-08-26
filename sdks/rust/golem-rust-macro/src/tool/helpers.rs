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

use proc_macro2::{Group, Ident, Span, TokenStream, TokenTree};
use std::collections::BTreeSet;
use syn::spanned::Spanned;
use syn::{Error, Expr, ExprArray, ExprLit, GenericArgument, Lit, PathArguments, Token, Type};

/// Direction of an SDK stream parameter in a tool method signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Input,
    Output,
}

/// Recognizes required `InputStream`/`OutputStream` parameters and their
/// optional `Option<...>` forms. The boolean is `true` for a required stream.
pub fn stream_type(ty: &Type) -> Option<(StreamKind, bool)> {
    if let Some(kind) = direct_stream_type(ty) {
        return Some((kind, true));
    }
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let inner = arguments.args.iter().find_map(|argument| match argument {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })?;
    direct_stream_type(inner).map(|kind| (kind, false))
}

/// Whether a tool parameter is a required or optional SDK stream.
pub fn is_stream_type(ty: &Type) -> bool {
    stream_type(ty).is_some()
}

fn direct_stream_type(ty: &Type) -> Option<StreamKind> {
    let Type::Path(path) = ty else {
        return None;
    };
    match path.path.segments.last()?.ident.to_string().as_str() {
        "InputStream" => Some(StreamKind::Input),
        "OutputStream" => Some(StreamKind::Output),
        _ => None,
    }
}

pub fn replace_ident(tokens: TokenStream, from: &Ident, to: &Ident) -> TokenStream {
    tokens
        .into_iter()
        .map(|token| match token {
            TokenTree::Group(group) => {
                let mut replacement =
                    Group::new(group.delimiter(), replace_ident(group.stream(), from, to));
                replacement.set_span(group.span());
                TokenTree::Group(replacement)
            }
            TokenTree::Ident(ident) if identifiers_match(&ident, from) => {
                TokenTree::Ident(to.clone())
            }
            token => token,
        })
        .collect()
}

pub fn fresh_internal_ident(tokens: &TokenStream, preferred: &str, span: Span) -> Ident {
    let mut candidate = preferred.to_string();
    let mut suffix = 0;
    while contains_marker(tokens.clone(), &candidate) {
        suffix += 1;
        candidate = format!("{preferred}_{suffix}");
    }
    Ident::new(&candidate, span)
}

fn contains_marker(tokens: TokenStream, marker: &str) -> bool {
    let spellings = marker_spellings(marker);
    tokens.into_iter().any(|token| match token {
        TokenTree::Group(group) => contains_marker(group.stream(), marker),
        TokenTree::Ident(ident) => spellings
            .iter()
            .any(|spelling| ident.to_string().contains(spelling)),
        TokenTree::Literal(literal) => {
            let lexical = literal.to_string();
            let semantic = match syn::parse_str::<syn::Lit>(&lexical) {
                Ok(syn::Lit::Str(value)) => value.value(),
                _ => lexical,
            };
            spellings.iter().any(|spelling| semantic.contains(spelling))
        }
        TokenTree::Punct(_) => false,
    })
}

fn identifiers_match(left: &Ident, right: &Ident) -> bool {
    if left == right {
        return true;
    }
    let left = left.to_string();
    let right = right.to_string();
    left.strip_prefix("r#").unwrap_or(&left) == right.strip_prefix("r#").unwrap_or(&right)
}

pub fn normalize_sdk_paths_in_item_trait(
    item: &mut syn::ItemTrait,
    resolved: &Ident,
    canonical: &Ident,
    preserved_canonical: &Ident,
) {
    if identifiers_match(resolved, canonical) {
        return;
    }
    CanonicalIdentProtector {
        canonical,
        preserved_canonical,
    }
    .visit_item_trait_mut(item);
    SdkPathNormalizer {
        resolved,
        canonical,
    }
    .visit_item_trait_mut(item);
}

pub fn normalize_sdk_paths_in_derive_input(
    item: &mut syn::DeriveInput,
    resolved: &Ident,
    canonical: &Ident,
    preserved_canonical: &Ident,
) {
    if identifiers_match(resolved, canonical) {
        return;
    }
    CanonicalIdentProtector {
        canonical,
        preserved_canonical,
    }
    .visit_derive_input_mut(item);
    SdkPathNormalizer {
        resolved,
        canonical,
    }
    .visit_derive_input_mut(item);
}

pub fn resolve_generated_sdk_paths(
    tokens: TokenStream,
    resolved: &Ident,
    canonical: &Ident,
    preserved_canonical: &Ident,
) -> TokenStream {
    if identifiers_match(resolved, canonical) {
        return tokens;
    }
    let tokens = replace_ident(tokens, canonical, resolved);
    restore_internal_marker(tokens, preserved_canonical, canonical)
}

fn restore_internal_marker(
    tokens: TokenStream,
    preserved: &Ident,
    canonical: &Ident,
) -> TokenStream {
    let preserved = preserved.to_string();
    let canonical = canonical.to_string();
    let replacements = marker_replacements(&preserved, &canonical);

    restore_markers(tokens, &replacements)
}

fn restore_markers(tokens: TokenStream, replacements: &[(String, String)]) -> TokenStream {
    tokens
        .into_iter()
        .map(|token| match token {
            TokenTree::Group(group) => {
                let mut replacement = Group::new(
                    group.delimiter(),
                    restore_markers(group.stream(), replacements),
                );
                replacement.set_span(group.span());
                TokenTree::Group(replacement)
            }
            TokenTree::Ident(ident) => {
                let original = ident.to_string();
                let text = restore_marker_text(original.clone(), replacements);
                if text == original {
                    TokenTree::Ident(ident)
                } else {
                    let mut replacement = if let Some(raw) = text.strip_prefix("r#") {
                        Ident::new_raw(raw, ident.span())
                    } else {
                        Ident::new(&text, ident.span())
                    };
                    replacement.set_span(ident.span());
                    TokenTree::Ident(replacement)
                }
            }
            TokenTree::Literal(literal) => {
                let Ok(syn::Lit::Str(value)) = syn::parse_str::<syn::Lit>(&literal.to_string())
                else {
                    return TokenTree::Literal(literal);
                };
                let original = value.value();
                let restored = restore_marker_text(original.clone(), replacements);
                if restored == original {
                    TokenTree::Literal(literal)
                } else {
                    let mut replacement = proc_macro2::Literal::string(&restored);
                    replacement.set_span(literal.span());
                    TokenTree::Literal(replacement)
                }
            }
            TokenTree::Punct(punct) => TokenTree::Punct(punct),
        })
        .collect()
}

fn restore_marker_text(mut text: String, replacements: &[(String, String)]) -> String {
    for (from, to) in replacements {
        text = text.replace(from, to);
    }
    text
}

fn marker_spellings(marker: &str) -> Vec<String> {
    let mut spellings = vec![
        marker.to_string(),
        marker.to_lowercase(),
        to_kebab_case(marker),
        to_pascal_case(marker),
    ];
    spellings.sort_by_key(|spelling| std::cmp::Reverse(spelling.len()));
    spellings.dedup();
    spellings
}

fn marker_replacements(preserved: &str, canonical: &str) -> Vec<(String, String)> {
    let mut replacements = vec![
        (preserved.to_string(), canonical.to_string()),
        (preserved.to_lowercase(), canonical.to_lowercase()),
        (to_kebab_case(preserved), to_kebab_case(canonical)),
        (to_pascal_case(preserved), to_pascal_case(canonical)),
    ];
    replacements.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    replacements.dedup_by(|left, right| left.0 == right.0);
    replacements
}

fn to_pascal_case(input: &str) -> String {
    let mut output = String::new();
    let mut capitalize = true;
    for character in input.chars() {
        if character == '_' || character == '-' {
            capitalize = true;
        } else if capitalize {
            output.extend(character.to_uppercase());
            capitalize = false;
        } else {
            output.push(character);
        }
    }
    output
}

struct SdkPathNormalizer<'a> {
    resolved: &'a Ident,
    canonical: &'a Ident,
}

struct CanonicalIdentProtector<'a> {
    canonical: &'a Ident,
    preserved_canonical: &'a Ident,
}

impl VisitMut for CanonicalIdentProtector<'_> {
    fn visit_ident_mut(&mut self, ident: &mut Ident) {
        if identifiers_match(ident, self.canonical) {
            *ident = self.preserved_canonical.clone();
        }
    }

    fn visit_token_stream_mut(&mut self, tokens: &mut TokenStream) {
        *tokens = replace_ident(tokens.clone(), self.canonical, self.preserved_canonical);
    }
}

impl VisitMut for SdkPathNormalizer<'_> {
    fn visit_path_mut(&mut self, path: &mut syn::Path) {
        syn::visit_mut::visit_path_mut(self, path);
        if self.resolved == self.canonical {
            return;
        }
        let Some(first) = path.segments.first_mut() else {
            return;
        };
        if first.ident == *self.resolved {
            first.ident = self.canonical.clone();
        }
    }
}

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
    use super::{StreamKind, stream_type, to_kebab_case};

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

    #[test]
    fn recognizes_required_and_optional_stream_types() {
        let input = syn::parse_str("golem_rust::agentic::InputStream").unwrap();
        let optional_output = syn::parse_str("Option<golem_rust::agentic::OutputStream>").unwrap();
        let nested_optional = syn::parse_str("Option<Option<InputStream>>").unwrap();

        assert_eq!(stream_type(&input), Some((StreamKind::Input, true)));
        assert_eq!(
            stream_type(&optional_output),
            Some((StreamKind::Output, false))
        );
        assert_eq!(stream_type(&nested_optional), None);
    }
}
