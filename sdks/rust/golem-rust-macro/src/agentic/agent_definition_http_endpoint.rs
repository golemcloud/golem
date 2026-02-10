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

use syn::parse::Parser;

#[derive(Debug)]
pub struct ParsedHttpEndpointDetails {
    pub http_method: String,
    pub path_suffix: String,
    pub header_vars: Vec<(String, String)>,
    pub auth_details: Option<bool>,
    pub cors_options: Vec<String>,
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
                        || nv.path.is_ident("delete") =>
                {
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
                ))
            }
        };

        endpoints.push(ParsedHttpEndpointDetails {
            http_method: method,
            path_suffix: path,
            header_vars,
            auth_details,
            cors_options,
        });
    }

    Ok(endpoints)
}
