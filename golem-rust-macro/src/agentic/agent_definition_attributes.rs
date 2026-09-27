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
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Error, Expr, ExprArray, ExprLit, Lit, Token};

pub struct AgentDefinitionAttributes {
    pub name: Option<syn::LitStr>,
    pub filesystem_mount: Option<syn::LitStr>,
    pub agent_kind: syn::Ident,
    pub agent_mode: TokenStream,
    pub agent_is_durable: bool,
    pub http_mount: Option<TokenStream>,
    pub snapshotting: TokenStream,
    pub snapshotting_enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AgentDefinitionKind {
    Regular,
    HttpRouter,
}

pub fn parse_agent_definition_attributes(
    attrs: TokenStream,
    definition_kind: AgentDefinitionKind,
) -> Result<AgentDefinitionAttributes, Error> {
    let mut name = None;
    let kind = match definition_kind {
        AgentDefinitionKind::Regular => syn::parse_quote!(Regular),
        AgentDefinitionKind::HttpRouter => syn::parse_quote!(HttpRouter),
    };
    let mut mode = quote! {
        golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable
    };
    let mut agent_is_durable = true;
    let mut snapshotting = quote! {
        golem_rust::golem_agentic::golem::agent::common::Snapshotting::Disabled
    };
    let mut snapshotting_enabled = false;
    let mut http = ParsedHttpMount {
        mount: None,
        cors: vec![],
        auth: false,
        phantom_agent: false,
        webhook_suffix: None,
        static_files: vec![],
        filesystem_bindings: vec![],
        openapi_provider_method: None,
    };

    if attrs.is_empty() {
        return Ok(AgentDefinitionAttributes {
            name,
            filesystem_mount: None,
            agent_kind: kind,
            agent_mode: mode,
            agent_is_durable,
            http_mount: None,
            snapshotting,
            snapshotting_enabled,
        });
    }

    let parser = Punctuated::<Expr, Token![,]>::parse_terminated;
    let exprs = parser.parse2(attrs)?;

    let mut seen = std::collections::HashSet::new();
    for expr in exprs.iter() {
        let key = match expr {
            Expr::Assign(assign) => match &*assign.left {
                Expr::Path(path) => Some(quote! { #path }.to_string()),
                _ => None,
            },
            Expr::Path(path)
                if path.path.is_ident("ephemeral") || path.path.is_ident("durable") =>
            {
                Some("mode".into())
            }
            _ => None,
        };
        if let Some(key) = key
            && !seen.insert(key)
        {
            return Err(Error::new_spanned(
                expr,
                "duplicate agent option; declare each option once",
            ));
        }
        if let Expr::Path(p) = expr {
            if p.path.is_ident("ephemeral") {
                mode = quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral };
                agent_is_durable = false;
                continue;
            } else if p.path.is_ident("durable") {
                mode =
                    quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable };
                agent_is_durable = true;
                continue;
            }
        }

        if let Expr::Assign(assign) = expr
            && let Expr::Path(left) = &*assign.left
        {
            if left.path.is_ident("name") {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(lit), ..
                }) = &*assign.right
                {
                    if lit.value().trim().is_empty() {
                        return Err(Error::new_spanned(lit, "agent name must not be empty"));
                    }
                    name = Some(lit.clone());
                    continue;
                }
                return Err(Error::new_spanned(
                    &assign.right,
                    "name must be a string literal",
                ));
            }
            if left.path.is_ident("mode") {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(lit), ..
                }) = &*assign.right
                {
                    mode = match lit.value().as_str() {
                        "ephemeral" => {
                            agent_is_durable = false;
                            quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Ephemeral }
                        }
                        "durable" => {
                            agent_is_durable = true;
                            quote! { golem_rust::golem_agentic::golem::agent::common::AgentMode::Durable }
                        }
                        other => {
                            return Err(Error::new_spanned(
                                lit,
                                format!("invalid mode `{}`", other),
                            ));
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
                    snapshotting_enabled = lit.value() != "disabled";
                    continue;
                } else {
                    return Err(Error::new_spanned(
                        &assign.right,
                        "snapshotting must be a string literal",
                    ));
                }
            }
        }
        parse_http_expr(expr, &mut http)?;
    }

    let has_filesystem = !http.filesystem_bindings.is_empty();
    if let Some(name) = &name
        && kind != "HttpRouter"
    {
        return Err(Error::new_spanned(
            name,
            "explicit names are only supported for router registrations",
        ));
    }
    if has_filesystem && (!agent_is_durable || kind == "HttpRouter" || http.phantom_agent) {
        return Err(Error::new(
            proc_macro2::Span::call_site(),
            "filesystem_bindings require a durable, regular, non-phantom agent",
        ));
    }
    if (!http.static_files.is_empty() || http.openapi_provider_method.is_some())
        && kind != "HttpRouter"
    {
        return Err(Error::new(
            proc_macro2::Span::call_site(),
            "static_files and openapi_provider_method require an HTTP router",
        ));
    }
    if (has_filesystem || !http.static_files.is_empty() || http.openapi_provider_method.is_some())
        && http.mount.is_none()
    {
        return Err(Error::new(
            proc_macro2::Span::call_site(),
            "file mappings and OpenAPI providers require a mount",
        ));
    }
    if kind == "HttpRouter" {
        if agent_is_durable || snapshotting_enabled || http.phantom_agent {
            return Err(Error::new(
                proc_macro2::Span::call_site(),
                "HTTP routers must be ephemeral, non-phantom, with snapshotting disabled",
            ));
        }
        let mount = http.mount.as_ref().ok_or_else(|| {
            Error::new(
                proc_macro2::Span::call_site(),
                "HTTP routers require a literal mount",
            )
        })?;
        let parsed = golem_schema::http::FileMapping::compile(&mount.value(), "/mount")
            .map_err(|_| Error::new_spanned(mount, "router mount must be a literal absolute path, without captures, query, wildcard, or trailing slash"))?;
        if !matches!(parsed, golem_schema::http::FileMapping::Exact { .. }) {
            return Err(Error::new_spanned(
                mount,
                "router mount cannot contain a wildcard",
            ));
        }
    }
    let filesystem_mount = has_filesystem.then(|| http.mount.clone()).flatten();
    let exposure_path = if kind == "HttpRouter" || has_filesystem {
        Some(compile_exposure_mount(
            http.mount.as_ref().unwrap(),
            has_filesystem,
        )?)
    } else {
        None
    };
    let http_tokens = http.mount.map(|mount| {
        let cors = http.cors;
        let auth = http.auth;
        let phantom_agent = http.phantom_agent;
        let exposure_path = exposure_path.map(|path| quote! { mount.path_prefix = #path; });
        let static_files = http.static_files;
        let filesystem_bindings = http.filesystem_bindings;
        let provider = match http.openapi_provider_method {
            Some(name) => quote! { Some(#name.to_string()) },
            None => quote! { None },
        };
        let webhook_suffix = if let Some(suffix) = http.webhook_suffix {
            quote! { Some(#suffix.to_string()) }
        } else {
            quote! { None }
        };

        quote! {
            {
            let mut mount = golem_rust::agentic::get_http_mount_details(
                #mount,
                #auth,
                #phantom_agent,
                golem_rust::golem_agentic::golem::agent::common::CorsOptions {
                    allowed_patterns: vec![ #( #cors.to_string() ),* ],
                },
                #webhook_suffix,
            ).expect("Invalid HTTP mount configuration");
            #exposure_path
            mount.static_bindings = vec![#(#static_files),*];
            mount.filesystem_bindings = vec![#(#filesystem_bindings),*];
            mount.openapi_provider_method = #provider;
            mount
            }
        }
    });

    Ok(AgentDefinitionAttributes {
        name,
        filesystem_mount,
        agent_kind: kind,
        agent_mode: mode,
        agent_is_durable,
        http_mount: http_tokens,
        snapshotting,
        snapshotting_enabled,
    })
}

struct ParsedHttpMount {
    mount: Option<syn::LitStr>,
    cors: Vec<syn::LitStr>,
    auth: bool,
    phantom_agent: bool,
    webhook_suffix: Option<syn::LitStr>,
    static_files: Vec<TokenStream>,
    filesystem_bindings: Vec<TokenStream>,
    openapi_provider_method: Option<syn::LitStr>,
}

fn parse_http_expr(expr: &Expr, out: &mut ParsedHttpMount) -> Result<(), Error> {
    if let Expr::Assign(assign) = expr
        && let Expr::Path(left) = &*assign.left
        && let Some(ident) = left.path.get_ident()
    {
        match ident.to_string().as_str() {
            "static_files" => {
                out.static_files = parse_file_mappings(&assign.right)?;
                return Ok(());
            }
            "filesystem_bindings" => {
                out.filesystem_bindings = parse_file_mappings(&assign.right)?;
                return Ok(());
            }
            "openapi_provider_method" => {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(lit), ..
                }) = &*assign.right
                {
                    out.openapi_provider_method = Some(lit.clone());
                    return Ok(());
                }
                return Err(Error::new_spanned(
                    &assign.right,
                    "openapi_provider_method must be a method name string",
                ));
            }
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
                };
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
                };
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
                };
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
                };
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
                };
            }
            _ => {}
        }
    }

    Err(Error::new_spanned(
        expr,
        "Unknown agent_definition parameter. Valid parameters are: name, mode, snapshotting, mount, auth, phantom_agent, cors, webhook_suffix, static_files, filesystem_bindings, openapi_provider_method",
    ))
}

fn compile_exposure_mount(mount: &syn::LitStr, variables: bool) -> Result<TokenStream, Error> {
    let value = mount.value();
    let path = value
        .strip_prefix('/')
        .ok_or_else(|| Error::new_spanned(mount, "mount must be an absolute path"))?;
    let common = quote! { golem_rust::golem_agentic::golem::agent::common };
    let segments = path.split('/').filter(|_| !path.is_empty()).map(|segment| {
        if variables && let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if name.is_empty() || name.contains(['{', '}', '*']) {
                return Err(Error::new_spanned(mount, "filesystem mount captures must name constructor parameters"));
            }
            let system = match name {
                "agent-type" => Some(quote! { AgentType }),
                "agent-version" => Some(quote! { AgentVersion }),
                _ => None,
            };
            if let Some(system) = system {
                return Ok(quote! { #common::PathSegment::SystemVariable(#common::SystemVariable::#system) });
            }
            return Ok(quote! { #common::PathSegment::PathVariable(#common::PathVariable { variable_name: #name.to_string() }) });
        }
        let parsed = golem_schema::http::FileMapping::compile(&format!("/{segment}"), "/mount")
            .map_err(|_| Error::new_spanned(mount, "mount contains an unsafe or non-literal path segment"))?;
        match parsed {
            golem_schema::http::FileMapping::Exact { public_path, .. } if public_path.len() == 1 => {
                let segment = &public_path[0];
                Ok(quote! { #common::PathSegment::Literal(#segment.to_string()) })
            },
            _ => Err(Error::new_spanned(mount, "mount cannot contain empty segments or wildcards")),
        }
    }).collect::<Result<Vec<_>, _>>()?;
    Ok(quote! { vec![#(#segments),*] })
}

fn parse_file_mappings(expr: &Expr) -> Result<Vec<TokenStream>, Error> {
    let Expr::Array(array) = expr else {
        return Err(Error::new_spanned(
            expr,
            "file mappings must be an array of (source, target) string pairs",
        ));
    };
    let mut seen = std::collections::HashSet::new();
    array.elems.iter().map(|entry| {
        let Expr::Tuple(pair) = entry else {
            return Err(Error::new_spanned(entry, "expected (source, target)"));
        };
        if pair.elems.len() != 2 { return Err(Error::new_spanned(entry, "expected exactly two strings: (source, target)")); }
        let strings = pair.elems.iter().map(|value| match value {
            Expr::Lit(ExprLit { lit: Lit::Str(lit), .. }) => Ok(lit.value()),
            _ => Err(Error::new_spanned(value, "mapping paths must be string literals")),
        }).collect::<Result<Vec<_>, _>>()?;
        let mapping = golem_schema::http::FileMapping::compile(&strings[0], &strings[1])
            .map_err(|error| Error::new_spanned(entry, format!("invalid file mapping ({error}); use an absolute source and canonical target, with terminal /* and /$1 for subtrees")))?;
        if !seen.insert(mapping.clone()) { return Err(Error::new_spanned(entry, "duplicate compiled source + target mapping; remove this pair")); }
        let common = quote! { golem_rust::golem_agentic::golem::agent::common };
        Ok(match mapping {
            golem_schema::http::FileMapping::Exact { public_path, file_path } => quote! {
                #common::FileMapping::Exact(#common::ExactFileMapping {
                    public_path: vec![#(#public_path.to_string()),*], file_path: #file_path.to_string(),
                })
            },
            golem_schema::http::FileMapping::Subtree { public_prefix, filesystem_root } => quote! {
                #common::FileMapping::Subtree(#common::SubtreeFileMapping {
                    public_prefix: vec![#(#public_prefix.to_string()),*], filesystem_root: #filesystem_root.to_string(),
                })
            },
        })
    }).collect()
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
                    format!(
                        "invalid snapshotting value `{}`. Valid values are: disabled, enabled, periodic(<duration>), every(<count>)",
                        other
                    ),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use test_r::test;

    #[test]
    fn mapping_corpus_emits_structural_metadata_in_order() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        for case in corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["suite"] == "mapping")
        {
            let pairs = case["input"]["mappings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| {
                    let source = pair[0].as_str().unwrap();
                    let target = pair[1].as_str().unwrap();
                    quote! { (#source, #target) }
                });
            let expression = syn::parse2(quote! { [#(#pairs),*] }).unwrap();
            let actual = parse_file_mappings(&expression);
            if case["expect"].get("error").is_some() {
                assert!(actual.is_err(), "{}", case["id"]);
                continue;
            }
            let actual = actual.unwrap();
            let expected = case["expect"]["compiled"].as_array().unwrap();
            assert_eq!(actual.len(), expected.len(), "{}", case["id"]);
            for (actual, expected) in actual.iter().zip(expected) {
                let (variant, record, segments_key, target_key) = if expected.get("Exact").is_some()
                {
                    ("Exact", "ExactFileMapping", "public_path", "file_path")
                } else {
                    (
                        "Subtree",
                        "SubtreeFileMapping",
                        "public_prefix",
                        "filesystem_root",
                    )
                };
                let variant_ident = syn::Ident::new(variant, proc_macro2::Span::call_site());
                let record_ident = syn::Ident::new(record, proc_macro2::Span::call_site());
                let segments_ident = syn::Ident::new(segments_key, proc_macro2::Span::call_site());
                let target_ident = syn::Ident::new(target_key, proc_macro2::Span::call_site());
                let segments = expected[variant][segments_key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap());
                let target = expected[variant][target_key].as_str().unwrap();
                let expected = quote! { golem_rust::golem_agentic::golem::agent::common::FileMapping::#variant_ident(
                    golem_rust::golem_agentic::golem::agent::common::#record_ident {
                        #segments_ident: vec![#(#segments.to_string()),*], #target_ident: #target.to_string(),
                    }
                ) };
                assert_eq!(actual.to_string(), expected.to_string(), "{}", case["id"]);
            }
        }
    }

    #[test]
    fn exposure_mount_and_owner_diagnostics() {
        for (kind, attrs) in [
            (AgentDefinitionKind::HttpRouter, quote! { mount = "/" }),
            (
                AgentDefinitionKind::HttpRouter,
                quote! { ephemeral, mount = "/", snapshotting = "enabled" },
            ),
            (
                AgentDefinitionKind::HttpRouter,
                quote! { ephemeral, mount = "/{id}" },
            ),
            (
                AgentDefinitionKind::Regular,
                quote! { ephemeral, mount = "/files", filesystem_bindings = [("/*", "/public/$1")] },
            ),
            (
                AgentDefinitionKind::Regular,
                quote! { phantom_agent = true, mount = "/files", filesystem_bindings = [("/*", "/public/$1")] },
            ),
            (
                AgentDefinitionKind::Regular,
                quote! { mount = "/files//", filesystem_bindings = [("/*", "/public/$1")] },
            ),
            (
                AgentDefinitionKind::Regular,
                quote! { mount = "/files/*", filesystem_bindings = [("/*", "/public/$1")] },
            ),
            (
                AgentDefinitionKind::Regular,
                quote! { mount = "/", filesystem_bindings = [("/*", "/a/$1")], filesystem_bindings = [] },
            ),
        ] {
            assert!(
                parse_agent_definition_attributes(attrs.clone(), kind).is_err(),
                "{attrs}"
            );
        }
        let mount: syn::LitStr = syn::parse_quote!("/%66iles/{id}");
        let compiled = compile_exposure_mount(&mount, true).unwrap().to_string();
        assert!(compiled.contains("\"files\""));
        assert!(!compiled.contains("%66"));
    }

    #[test]
    fn public_agent_definition_rejects_kind() {
        let error = parse_agent_definition_attributes(
            quote! { kind = "http-router" },
            AgentDefinitionKind::Regular,
        )
        .err()
        .expect("kind must not be publicly settable");
        assert!(
            error
                .to_string()
                .contains("Unknown agent_definition parameter")
        );
    }
}
