// Copyright 2024-2025 Golem Cloud
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
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Error, Expr, ExprArray, ExprLit, Lit, Token};

pub struct AgentDefinitionAttributes {
    pub agent_mode: TokenStream,
    pub http_mount: Option<TokenStream>,
    pub snapshotting: TokenStream,
}

pub fn parse_agent_definition_attributes(
    attrs: proc_macro::TokenStream,
) -> Result<AgentDefinitionAttributes, Error> {
    let mut mode = quote! {
        golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable
    };
    let mut snapshotting = quote! {
        golem_rust::golem_agentic::golem::agent::common::Snapshotting::Disabled
    };
    let mut http = ParsedHttpMount {
        mount: None,
        cors: vec![],
        auth: false,
        phantom_agent: false,
        webhook_suffix: None,
    };

    if attrs.is_empty() {
        return Ok(AgentDefinitionAttributes {
            agent_mode: mode,
            http_mount: None,
            snapshotting,
        });
    }

    let parser = Punctuated::<Expr, Token![,]>::parse_terminated;
    let exprs = parser.parse(attrs)?;

    for expr in exprs.iter() {
        if let Expr::Path(p) = expr {
            if p.path.is_ident("ephemeral") {
                mode = quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral };
                continue;
            } else if p.path.is_ident("durable") {
                mode =
                    quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable };
                continue;
            }
        }

        if let Expr::Assign(assign) = expr {
            if let Expr::Path(left) = &*assign.left {
                if left.path.is_ident("mode") {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(lit), ..
                    }) = &*assign.right
                    {
                        mode = match lit.value().as_str() {
                            "ephemeral" => {
                                quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral }
                            }
                            "durable" => {
                                quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable }
                            }
                            other => {
                                return Err(Error::new_spanned(
                                    lit,
                                    format!("invalid mode `{}`", other),
                                ))
                            }
                        };
                        continue;
                    } else {
                        return Err(Error::new_spanned(
                            &assign.right,
                            "mode must be a string literal",
                        ));
                    }
                }

                if left.path.is_ident("snapshotting") {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(lit), ..
                    }) = &*assign.right
                    {
                        snapshotting = parse_snapshotting_value(lit)?;
                        continue;
                    } else {
                        return Err(Error::new_spanned(
                            &assign.right,
                            "snapshotting must be a string literal",
                        ));
                    }
                }
            }
        }

        parse_http_expr(expr, &mut http)?;
    }

    let http_tokens = http.mount.map(|mount| {
        let cors = http.cors;
        let auth = http.auth;
        let phantom_agent = http.phantom_agent;
        let webhook_suffix = if let Some(suffix) = http.webhook_suffix {
            quote! { Some(#suffix.to_string()) }
        } else {
            quote! { None }
        };

        quote! {
            golem_rust::agentic::get_http_mount_details(
                #mount,
                #auth,
                #phantom_agent,
                golem_rust::golem_agentic::golem::agent::common::CorsOptions {
                    allowed_patterns: vec![ #( #cors.to_string() ),* ],
                },
                #webhook_suffix,
            ).expect("Invalid HTTP mount configuration")
        }
    });

    Ok(AgentDefinitionAttributes {
        agent_mode: mode,
        http_mount: http_tokens,
        snapshotting,
    })
}

struct ParsedHttpMount {
    mount: Option<syn::LitStr>,
    cors: Vec<syn::LitStr>,
    auth: bool,
    phantom_agent: bool,
    webhook_suffix: Option<syn::LitStr>,
}

fn parse_http_expr(expr: &Expr, out: &mut ParsedHttpMount) -> Result<(), Error> {
    if let Expr::Assign(assign) = expr {
        if let Expr::Path(left) = &*assign.left {
            if let Some(ident) = left.path.get_ident() {
                match ident.to_string().as_str() {
                    "mount" => {
                        return if let Expr::Lit(ExprLit {
                            lit: Lit::Str(lit), ..
                        }) = &*assign.right
                        {
                            out.mount = Some(lit.clone());
                            Ok(())
                        } else {
                            Err(Error::new_spanned(
                                &assign.right,
                                "mount must be a string literal",
                            ))
                        }
                    }
                    "webhook_suffix" => {
                        return if let Expr::Lit(ExprLit {
                            lit: Lit::Str(lit), ..
                        }) = &*assign.right
                        {
                            out.webhook_suffix = Some(lit.clone());
                            Ok(())
                        } else {
                            Err(Error::new_spanned(
                                &assign.right,
                                "webhook-suffix must be a string literal",
                            ))
                        }
                    }
                    "auth" => {
                        return if let Expr::Lit(ExprLit {
                            lit: Lit::Bool(b), ..
                        }) = &*assign.right
                        {
                            out.auth = b.value;
                            Ok(())
                        } else {
                            Err(Error::new_spanned(
                                &assign.right,
                                "auth must be a boolean literal",
                            ))
                        }
                    }
                    "phantom_agent" => {
                        return if let Expr::Lit(ExprLit {
                            lit: Lit::Bool(b), ..
                        }) = &*assign.right
                        {
                            out.phantom_agent = b.value;
                            Ok(())
                        } else {
                            Err(Error::new_spanned(
                                &assign.right,
                                "phantom-agent must be a boolean literal",
                            ))
                        }
                    }
                    "cors" => {
                        return if let Expr::Array(ExprArray { elems, .. }) = &*assign.right {
                            for elem in elems {
                                if let Expr::Lit(ExprLit {
                                    lit: Lit::Str(lit), ..
                                }) = elem
                                {
                                    out.cors.push(lit.clone());
                                } else {
                                    return Err(Error::new_spanned(
                                        elem,
                                        "cors entries must be string literals",
                                    ));
                                }
                            }
                            Ok(())
                        } else {
                            Err(Error::new_spanned(
                                &assign.right,
                                "cors must be an array of string literals",
                            ))
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Err(Error::new_spanned(
        expr,
        "Unknown agent_definition parameter. Valid parameters are: mode, snapshotting, mount, auth, phantom-agent, cors, webhook-suffix",
    ))
}

fn parse_snapshotting_value(lit: &syn::LitStr) -> Result<TokenStream, Error> {
    let value = lit.value();
    match value.as_str() {
        "disabled" => Ok(quote! {
            golem_rust::golem_agentic::golem::agent::common::Snapshotting::Disabled
        }),
        "enabled" => Ok(quote! {
            golem_rust::golem_agentic::golem::agent::common::Snapshotting::Enabled(
                golem_rust::golem_agentic::golem::agent::common::SnapshottingConfig::Default
            )
        }),
        other => {
            if let Some(inner) = other
                .strip_prefix("periodic(")
                .and_then(|s| s.strip_suffix(')'))
            {
                let duration = inner.parse::<humantime::Duration>().map_err(|e| {
                    Error::new_spanned(
                        lit,
                        format!("invalid duration in periodic(`{}`): {}", inner, e),
                    )
                })?;
                let nanos: u64 = duration.as_nanos() as u64;
                Ok(quote! {
                    golem_rust::golem_agentic::golem::agent::common::Snapshotting::Enabled(
                        golem_rust::golem_agentic::golem::agent::common::SnapshottingConfig::Periodic(#nanos)
                    )
                })
            } else if let Some(inner) = other
                .strip_prefix("every(")
                .and_then(|s| s.strip_suffix(')'))
            {
                let count: u16 = inner.parse().map_err(|_| {
                    Error::new_spanned(
                        lit,
                        format!("invalid count in every(`{}`), expected a u16 value", inner),
                    )
                })?;
                Ok(quote! {
                    golem_rust::golem_agentic::golem::agent::common::Snapshotting::Enabled(
                        golem_rust::golem_agentic::golem::agent::common::SnapshottingConfig::EveryNInvocation(#count)
                    )
                })
            } else {
                Err(Error::new_spanned(
                    lit,
                    format!("invalid snapshotting value `{}`. Valid values are: disabled, enabled, periodic(<duration>), every(<count>)", other),
                ))
            }
        }
    }
}
