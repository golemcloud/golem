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

use syn::parse::Parser;

#[derive(Debug)]
pub struct ParsedHttpEndpointDetails {
    pub http_method: String,
    pub path_suffix: String,
    pub header_vars: Vec<(String, String)>,
    pub auth_details: Option<bool>,
    pub cors_options: Vec<String>,
    pub durable_streams: Option<ParsedDurableStreamOptions>,
}

#[derive(Debug)]
pub struct ParsedDurableStreamOptions {
    pub slots: Vec<ParsedDurableStreamSlot>,
    pub allow_external_writes: Option<bool>,
    pub allow_stream_delete: Option<bool>,
    pub allow_invocation_delete: Option<bool>,
    pub max_concurrent_readers_per_stream: Option<u32>,
    pub max_append_requests_per_second_per_stream: Option<u32>,
}

#[derive(Debug)]
pub struct ParsedDurableStreamSlot {
    pub source: ParsedDurableStreamSlotSource,
    pub slot: String,
    pub name: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Debug)]
pub enum ParsedDurableStreamSlotSource {
    Input,
    Output,
}

fn parse_durable_streams(
    tokens: proc_macro2::TokenStream,
) -> syn::Result<ParsedDurableStreamOptions> {
    let mut result = ParsedDurableStreamOptions {
        slots: vec![],
        allow_external_writes: None,
        allow_stream_delete: None,
        allow_invocation_delete: None,
        max_concurrent_readers_per_stream: None,
        max_append_requests_per_second_per_stream: None,
    };

    Parser::parse2(
        |input: syn::parse::ParseStream| {
            while !input.is_empty() {
                let key: syn::Ident = input.parse()?;
                if key == "input" || key == "output" {
                    let content;
                    syn::parenthesized!(content in input);
                    let slot: syn::LitStr = content.parse()?;
                    let mut name = None;
                    let mut content_type = None;
                    while !content.is_empty() {
                        content.parse::<syn::Token![,]>()?;
                        let field: syn::Ident = content.parse()?;
                        content.parse::<syn::Token![=]>()?;
                        let value: syn::LitStr = content.parse()?;
                        let destination = if field == "name" {
                            &mut name
                        } else if field == "content_type" {
                            &mut content_type
                        } else {
                            return Err(syn::Error::new_spanned(
                                field,
                                "Unknown durable stream slot field",
                            ));
                        };
                        if destination.replace(value.value()).is_some() {
                            return Err(syn::Error::new_spanned(
                                field,
                                "Duplicate durable stream slot field",
                            ));
                        }
                    }
                    result.slots.push(ParsedDurableStreamSlot {
                        source: if key == "input" {
                            ParsedDurableStreamSlotSource::Input
                        } else {
                            ParsedDurableStreamSlotSource::Output
                        },
                        slot: slot.value(),
                        name,
                        content_type,
                    });
                } else {
                    input.parse::<syn::Token![=]>()?;
                    if key == "allow_external_writes"
                        || key == "allow_stream_delete"
                        || key == "allow_invocation_delete"
                    {
                        let value: syn::LitBool = input.parse()?;
                        let destination = if key == "allow_external_writes" {
                            &mut result.allow_external_writes
                        } else if key == "allow_stream_delete" {
                            &mut result.allow_stream_delete
                        } else {
                            &mut result.allow_invocation_delete
                        };
                        if destination.replace(value.value).is_some() {
                            return Err(syn::Error::new_spanned(
                                key,
                                "Duplicate durable streams key",
                            ));
                        }
                    } else if key == "max_concurrent_readers_per_stream"
                        || key == "max_append_requests_per_second_per_stream"
                    {
                        let value: syn::LitInt = input.parse()?;
                        let value = value.base10_parse::<u32>().map_err(|_| {
                            syn::Error::new_spanned(&value, "Expected a positive u32 literal")
                        })?;
                        if value == 0 || (key == "max_concurrent_readers_per_stream" && value > 16)
                        {
                            return Err(syn::Error::new_spanned(
                                key,
                                if value == 0 {
                                    "Limit must be greater than zero"
                                } else {
                                    "Reader limit must be at most 16"
                                },
                            ));
                        }
                        let destination = if key == "max_concurrent_readers_per_stream" {
                            &mut result.max_concurrent_readers_per_stream
                        } else {
                            &mut result.max_append_requests_per_second_per_stream
                        };
                        if destination.replace(value).is_some() {
                            return Err(syn::Error::new_spanned(
                                key,
                                "Duplicate durable streams key",
                            ));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(key, "Unknown durable streams key"));
                    }
                }
                if !input.is_empty() {
                    input.parse::<syn::Token![,]>()?;
                }
            }
            Ok(())
        },
        tokens,
    )?;
    Ok(result)
}

pub fn extract_http_endpoints(
    attrs: &[syn::Attribute],
) -> syn::Result<Vec<ParsedHttpEndpointDetails>> {
    let mut endpoints = Vec::new();

    for attr in attrs {
        if !attr.path().is_ident("endpoint") {
            continue;
        }

        let syn::Meta::List(list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                attr,
                "Expected #[endpoint(...)] attribute",
            ));
        };

        let mut http_method: Option<String> = None;
        let mut path_suffix: Option<String> = None;
        let mut header_vars: Vec<(String, String)> = Vec::new();
        let mut auth_details: Option<bool> = None;
        let mut cors_options: Vec<String> = Vec::new();
        let mut durable_streams = None;
        let mut cors_seen = false;
        let mut headers_seen = false;

        let parser = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;

        let items = parser.parse2(list.tokens.clone()).map_err(|e| {
            syn::Error::new_spanned(&list.tokens, format!("Failed to parse #[endpoint]: {}", e))
        })?;

        for item in items {
            match item {
                syn::Meta::NameValue(nv)
                    if nv.path.is_ident("get")
                        || nv.path.is_ident("post")
                        || nv.path.is_ident("put")
                        || nv.path.is_ident("delete")
                        || nv.path.is_ident("patch") =>
                {
                    if http_method.is_some() {
                        return Err(syn::Error::new_spanned(
                            nv.path,
                            "Duplicate HTTP method key",
                        ));
                    }
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = nv.value
                    {
                        http_method = Some(nv.path.get_ident().unwrap().to_string());
                        path_suffix = Some(s.value());
                    } else {
                        return Err(syn::Error::new_spanned(
                            nv.value,
                            "Expected string literal for HTTP path",
                        ));
                    }
                }

                syn::Meta::NameValue(nv) if nv.path.is_ident("auth") => {
                    if auth_details.is_some() {
                        return Err(syn::Error::new_spanned(nv.path, "Duplicate auth key"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Bool(b),
                        ..
                    }) = nv.value
                    {
                        auth_details = Some(b.value);
                    } else {
                        return Err(syn::Error::new_spanned(
                            nv.value,
                            "Expected boolean literal for auth",
                        ));
                    }
                }

                syn::Meta::NameValue(nv) if nv.path.is_ident("cors") => {
                    if cors_seen {
                        return Err(syn::Error::new_spanned(nv.path, "Duplicate cors key"));
                    }
                    cors_seen = true;
                    if let syn::Expr::Array(arr) = nv.value {
                        for elem in arr.elems {
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(s),
                                ..
                            }) = elem
                            {
                                cors_options.push(s.value());
                            } else {
                                return Err(syn::Error::new_spanned(
                                    elem,
                                    "Expected string literal in CORS array",
                                ));
                            }
                        }
                    } else {
                        return Err(syn::Error::new_spanned(nv.value, "Expected array for cors"));
                    }
                }

                syn::Meta::List(ml) if ml.path.is_ident("headers") => {
                    if headers_seen {
                        return Err(syn::Error::new_spanned(ml.path, "Duplicate headers key"));
                    }
                    headers_seen = true;
                    Parser::parse2(
                        |input: syn::parse::ParseStream| {
                            while !input.is_empty() {
                                let key: syn::LitStr = input.parse().map_err(|e| {
                                    syn::Error::new(
                                        input.span(),
                                        format!("Invalid header key: {}", e),
                                    )
                                })?;
                                input.parse::<syn::Token![=]>().map_err(|e| {
                                    syn::Error::new(input.span(), format!("Expected '=': {}", e))
                                })?;
                                let value: syn::LitStr = input.parse().map_err(|e| {
                                    syn::Error::new(
                                        input.span(),
                                        format!("Invalid header value: {}", e),
                                    )
                                })?;
                                header_vars.push((key.value(), value.value()));

                                let _ = input.parse::<syn::Token![,]>();
                            }
                            Ok(())
                        },
                        ml.tokens.clone(),
                    )?;
                }

                syn::Meta::List(ml) if ml.path.is_ident("durable_streams") => {
                    if durable_streams.is_some() {
                        return Err(syn::Error::new_spanned(ml, "Duplicate durable_streams key"));
                    }
                    durable_streams = Some(parse_durable_streams(ml.tokens)?);
                }

                _ => {
                    return Err(syn::Error::new_spanned(
                        item,
                        "Unexpected attribute item in #[endpoint]",
                    ));
                }
            }
        }

        let (method, path) = match (http_method, path_suffix) {
            (Some(m), Some(p)) => (m, p),
            _ => {
                return Err(syn::Error::new_spanned(
                    list,
                    "Endpoint must specify HTTP method and path",
                ));
            }
        };

        endpoints.push(ParsedHttpEndpointDetails {
            http_method: method,
            path_suffix: path,
            header_vars,
            auth_details,
            cors_options,
            durable_streams,
        });
    }

    Ok(endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn parse(tokens: proc_macro2::TokenStream) -> syn::Result<Vec<ParsedHttpEndpointDetails>> {
        let attr: syn::Attribute = syn::parse_quote!(#[endpoint(#tokens)]);
        extract_http_endpoints(&[attr])
    }

    #[test]
    fn parses_durable_stream_options() {
        let endpoints = parse(syn::parse_quote!(
            post = "/process",
            durable_streams(
                input("input", name = "messages"),
                output(
                    "$result",
                    name = "results",
                    content_type = "application/vnd.golem.events"
                ),
                allow_external_writes = true,
                allow_stream_delete = false,
                allow_invocation_delete = false,
                max_concurrent_readers_per_stream = 8,
                max_append_requests_per_second_per_stream = 25,
            )
        ))
        .unwrap();
        let options = endpoints[0].durable_streams.as_ref().unwrap();
        assert_eq!(options.slots.len(), 2);
        assert_eq!(options.allow_external_writes, Some(true));
        assert_eq!(options.allow_stream_delete, Some(false));
        assert_eq!(options.allow_invocation_delete, Some(false));
        assert_eq!(options.max_concurrent_readers_per_stream, Some(8));
        assert_eq!(options.max_append_requests_per_second_per_stream, Some(25));
    }

    #[test]
    fn rejects_invalid_durable_stream_syntax() {
        let invalid: Vec<proc_macro2::TokenStream> = vec![
            syn::parse_quote!(post = "/", durable_streams(unknown("x"))),
            syn::parse_quote!(post = "/", durable_streams(input("x", unknown = "y"))),
            syn::parse_quote!(
                post = "/",
                durable_streams(input("x", name = "a", name = "b"))
            ),
            syn::parse_quote!(
                post = "/",
                durable_streams(allow_stream_delete = true, allow_stream_delete = false)
            ),
            syn::parse_quote!(
                post = "/",
                durable_streams(max_concurrent_readers_per_stream = 0)
            ),
            syn::parse_quote!(
                post = "/",
                durable_streams(max_concurrent_readers_per_stream = 17)
            ),
            syn::parse_quote!(
                post = "/",
                durable_streams(max_append_requests_per_second_per_stream = 0)
            ),
            syn::parse_quote!(
                post = "/",
                durable_streams(max_append_requests_per_second_per_stream = 4294967296)
            ),
            syn::parse_quote!(
                post = "/",
                durable_streams(input(name = "missing selector"))
            ),
        ];
        for tokens in invalid {
            assert!(parse(tokens).is_err());
        }
    }
}
