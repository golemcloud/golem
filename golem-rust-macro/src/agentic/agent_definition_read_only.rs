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

use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Attribute, Error, Expr, ExprLit, Lit, Token};

/// Parsed form of `#[read_only(...)]`.
pub struct ParsedReadOnly {
    /// Token stream that constructs a `CachePolicy` value.
    pub cache_policy: TokenStream,
}

/// Looks for a `#[read_only]` or `#[read_only(...)]` attribute on a method.
///
/// Returns:
/// - `Ok(None)` if no `#[read_only]` attribute is present.
/// - `Ok(Some(ParsedReadOnly))` if a valid `#[read_only]` attribute is found.
/// - `Err(_)` if the attribute is malformed.
pub fn extract_read_only(attrs: &[Attribute]) -> Result<Option<ParsedReadOnly>, Error> {
    let mut found: Option<ParsedReadOnly> = None;

    for attr in attrs {
        if !attr.path().is_ident("read_only") {
            continue;
        }

        let mut cache_policy = quote! {
            golem_rust::golem_agentic::golem::agent::common::CachePolicy::UntilWrite
        };

        // Bare `#[read_only]` (no parens) means default cache = until_write.
        if matches!(attr.meta, syn::Meta::Path(_)) {
            found = Some(ParsedReadOnly { cache_policy });
            continue;
        }

        let parser = Punctuated::<Expr, Token![,]>::parse_terminated;
        let exprs = attr.parse_args_with(parser)?;

        let mut explicit_cache: Option<TokenStream> = None;
        let mut explicit_ttl: Option<u64> = None;

        for expr in exprs.iter() {
            if let Expr::Assign(assign) = expr
                && let Expr::Path(left) = &*assign.left
                && let Some(ident) = left.path.get_ident()
            {
                match ident.to_string().as_str() {
                    "cache" => {
                        if let Expr::Lit(ExprLit {
                            lit: Lit::Str(lit), ..
                        }) = &*assign.right
                        {
                            explicit_cache = Some(parse_cache_value(lit)?);
                            continue;
                        } else {
                            return Err(Error::new_spanned(
                                &assign.right,
                                "cache must be a string literal: \"no_cache\", \"until_write\", or \"ttl\"",
                            ));
                        }
                    }
                    "ttl" => {
                        if let Expr::Lit(ExprLit {
                            lit: Lit::Str(lit), ..
                        }) = &*assign.right
                        {
                            let duration =
                                lit.value().parse::<humantime::Duration>().map_err(|e| {
                                    Error::new_spanned(
                                        lit,
                                        format!("invalid ttl duration `{}`: {}", lit.value(), e),
                                    )
                                })?;
                            explicit_ttl = Some(duration.as_nanos() as u64);
                            continue;
                        } else {
                            return Err(Error::new_spanned(
                                &assign.right,
                                "ttl must be a string literal duration (e.g. \"30s\")",
                            ));
                        }
                    }
                    _ => {}
                }
            }
            return Err(Error::new_spanned(
                expr,
                "Unknown read_only parameter. Valid parameters are: cache, ttl",
            ));
        }

        match (explicit_cache, explicit_ttl) {
            (Some(c), None) => {
                // If the user wrote cache = "ttl" without ttl=, that's an error
                let c_str = c.to_string();
                if c_str.contains("Ttl") {
                    return Err(Error::new_spanned(
                        attr,
                        "cache = \"ttl\" requires a ttl = \"<duration>\" parameter",
                    ));
                }
                cache_policy = c;
            }
            (None, Some(nanos)) => {
                cache_policy = quote! {
                    golem_rust::golem_agentic::golem::agent::common::CachePolicy::Ttl(#nanos)
                };
            }
            (Some(c), Some(nanos)) => {
                let c_str = c.to_string();
                if !c_str.contains("Ttl") {
                    return Err(Error::new_spanned(
                        attr,
                        "ttl parameter is only valid with cache = \"ttl\"",
                    ));
                }
                cache_policy = quote! {
                    golem_rust::golem_agentic::golem::agent::common::CachePolicy::Ttl(#nanos)
                };
            }
            (None, None) => {}
        }

        found = Some(ParsedReadOnly { cache_policy });
    }

    Ok(found)
}

/// Parses a `cache = "..."` string literal into a `CachePolicy` token stream.
/// For `"ttl"` the caller must supplement with a `ttl = "..."` value.
fn parse_cache_value(lit: &syn::LitStr) -> Result<TokenStream, Error> {
    match lit.value().as_str() {
        "no_cache" | "no-cache" => Ok(quote! {
            golem_rust::golem_agentic::golem::agent::common::CachePolicy::NoCache
        }),
        "until_write" | "until-write" => Ok(quote! {
            golem_rust::golem_agentic::golem::agent::common::CachePolicy::UntilWrite
        }),
        "ttl" => Ok(quote! {
            golem_rust::golem_agentic::golem::agent::common::CachePolicy::Ttl(0)
        }),
        other => Err(Error::new_spanned(
            lit,
            format!(
                "invalid cache value `{}`. Valid values are: no_cache, until_write, ttl",
                other
            ),
        )),
    }
}
