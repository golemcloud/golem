// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use crate::agentic::agent_definition_http_endpoint::{
    ParsedDurableStreamSlotSource, extract_http_endpoints,
};
use crate::tool::helpers::expr_str;
use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::{Attribute, Error, Meta};

pub fn mount_tokens(mount: Option<&str>, command: &str) -> TokenStream {
    mount.map_or_else(|| quote! { None }, |path| quote! {
        Some(golem_rust::schema::tool::ToolHttpMount {
            path_prefix: golem_rust::agentic::parse_tool_http_path(#path)
                .map_err(|detail| golem_rust::agentic::ToolBuildError::InvalidHttpDeclaration {
                    command: #command.to_string(), input: None,
                    detail: format!("invalid tool HTTP mount: {detail}"),
                })?,
        })
    })
}

pub fn command_http_tokens(attr: Option<&Attribute>, command: &str) -> Result<TokenStream, Error> {
    let Some(attr) = attr else {
        return Ok(quote! { None });
    };
    let items = attr
        .parse_args_with(syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated)?;
    let mut retained = Vec::new();
    let mut prefix = None;
    let mut route_id = None;
    let mut response_headers = Vec::new();
    let mut headers_seen = false;
    for item in items {
        match &item {
            Meta::NameValue(nv) if nv.path.is_ident("prefix") => {
                if prefix.replace(expr_str(&nv.value, "prefix")?).is_some() {
                    return Err(Error::new_spanned(item, "Duplicate prefix"));
                }
            }
            Meta::NameValue(nv) if nv.path.is_ident("route_id") => {
                if route_id.replace(expr_str(&nv.value, "route_id")?).is_some() {
                    return Err(Error::new_spanned(item, "Duplicate route_id"));
                }
            }
            Meta::List(list) if list.path.is_ident("response_headers") => {
                if headers_seen {
                    return Err(Error::new_spanned(item, "Duplicate response_headers"));
                }
                headers_seen = true;
                Parser::parse2(
                    |input: syn::parse::ParseStream| {
                        while !input.is_empty() {
                            let name: syn::LitStr = input.parse()?;
                            input.parse::<syn::Token![=]>()?;
                            let value: syn::LitStr = input.parse()?;
                            response_headers.push(quote! { golem_rust::golem_schema::http::FileResponseHeader { name: #name.to_string(), value: #value.to_string() } });
                            if !input.is_empty() {
                                input.parse::<syn::Token![,]>()?;
                            }
                        }
                        Ok(())
                    },
                    list.tokens.clone(),
                )?;
            }
            Meta::NameValue(nv) if nv.path.is_ident("auth") || nv.path.is_ident("cors") => {
                return Err(Error::new_spanned(
                    item,
                    "Tool HTTP admission and CORS belong to the export",
                ));
            }
            _ => retained.push(item),
        }
    }
    if let Some(path) = prefix {
        if !retained.is_empty() || route_id.is_some() || headers_seen {
            return Err(Error::new_spanned(
                attr,
                "A prefix cannot declare endpoint options",
            ));
        }
        return Ok(
            quote! { Some(golem_rust::schema::tool::CommandHttp::Prefix(golem_rust::schema::tool::ToolHttpPrefix {
                path_suffix: golem_rust::agentic::parse_tool_http_path(#path)
                    .map_err(|detail| golem_rust::agentic::ToolBuildError::InvalidHttpDeclaration {
                        command: #command.to_string(), input: None,
                        detail: format!("invalid tool HTTP prefix: {detail}"),
                    })?,
            })) },
        );
    }
    let filtered: Attribute = syn::parse_quote! { #[endpoint(#(#retained),*)] };
    let parsed = extract_http_endpoints(&[filtered])?.remove(0);
    if parsed.http_method == "any" {
        return Err(Error::new_spanned(
            attr,
            "Tools require a concrete HTTP method",
        ));
    }
    let method = parsed.http_method;
    let path = parsed.path_suffix;
    let headers = parsed
        .header_vars
        .iter()
        .map(|(k, v)| quote! { (#k.to_string(), #v.to_string()) });
    let route_id = string_option(route_id.as_deref());
    let mode = parsed.mode.as_deref().map_or_else(
        || quote! { None },
        |value| {
            let variant = match value {
                "durableStreams" => quote! { DurableStreams },
                "plain" => quote! { Plain },
                "sse" => quote! { Sse },
                "webSocket" => quote! { WebSocket },
                _ => unreachable!(),
            };
            quote! { Some(golem_rust::golem_schema::http::HttpRouteMode::#variant) }
        },
    );
    let durable_streams = parsed.durable_streams.map_or_else(|| quote! { None }, |options| {
        let slots = options.slots.iter().map(|slot| {
            let selector = &slot.slot;
            let source = match slot.source {
                ParsedDurableStreamSlotSource::Input => quote! {
                    golem_rust::golem_schema::http::DurableStreamSlotSource::Input(
                        golem_rust::golem_schema::http::DurableStreamInputSlotSource { name: #selector.to_string() }
                    )
                },
                ParsedDurableStreamSlotSource::Output => quote! {
                    golem_rust::golem_schema::http::DurableStreamSlotSource::Output(
                        golem_rust::golem_schema::http::DurableStreamOutputSlotSource { name: #selector.to_string() }
                    )
                },
            };
            let name = string_option(slot.name.as_deref());
            let content_type = string_option(slot.content_type.as_deref());
            quote! { golem_rust::golem_schema::http::DurableStreamSlotOptions {
                source: #source, name: #name, content_type: #content_type,
            } }
        });
        let readers = u32_option(options.max_concurrent_readers_per_stream);
        let appends = u32_option(options.max_append_requests_per_second_per_stream);
        let load = if options.max_concurrent_readers_per_stream.is_some() || options.max_append_requests_per_second_per_stream.is_some() {
            quote! { Some(golem_rust::golem_schema::http::DurableStreamRouteLoadOptions { max_concurrent_readers_per_stream: #readers, max_append_requests_per_second_per_stream: #appends }) }
        } else { quote! { None } };
        quote! { Some(golem_rust::golem_schema::http::DurableStreamRouteOptions { slots: vec![#(#slots),*], load: #load }) }
    });
    Ok(quote! { Some({
        let mut endpoint = golem_rust::agentic::get_tool_http_endpoint(#method, #path, vec![#(#headers),*])
            .map_err(|detail| golem_rust::agentic::ToolBuildError::InvalidHttpDeclaration {
                command: #command.to_string(), input: None,
                detail: format!("invalid tool HTTP endpoint: {detail}"),
            })?;
        endpoint.route_id = #route_id;
        endpoint.response_headers = vec![#(#response_headers),*];
        endpoint.mode = #mode;
        endpoint.durable_streams = #durable_streams;
        golem_rust::schema::tool::CommandHttp::Endpoint(endpoint)
    }) })
}

fn string_option(value: Option<&str>) -> TokenStream {
    value.map_or_else(|| quote! { None }, |v| quote! { Some(#v.to_string()) })
}

fn u32_option(value: Option<u32>) -> TokenStream {
    value.map_or_else(|| quote! { None }, |v| quote! { Some(#v) })
}
