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

pub fn extract_http_endpoints(attrs: &[syn::Attribute]) -> Vec<ParsedHttpEndpointDetails> {
    let mut endpoints = Vec::new();

    for attr in attrs {
        if !attr.path().is_ident("endpoint") {
            continue;
        }

        let syn::Meta::List(list) = &attr.meta else {
            continue;
        };

        let mut http_method: Option<String> = None;
        let mut path_suffix: Option<String> = None;
        let mut header_vars: Vec<(String, String)> = Vec::new();
        let mut auth_details: Option<bool> = None;
        let mut cors_options: Vec<String> = Vec::new();

        let parser = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;

        let Ok(items) = parser.parse2(list.tokens.clone()) else {
            continue;
        };

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
                    }
                }

                syn::Meta::NameValue(nv) if nv.path.is_ident("auth") => {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Bool(b),
                        ..
                    }) = nv.value
                    {
                        auth_details = Some(b.value);
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
                            }
                        }
                    }
                }

                syn::Meta::List(ml) if ml.path.is_ident("headers") => {
                    let _ = Parser::parse2(
                        |input: syn::parse::ParseStream| {
                            while !input.is_empty() {
                                let key: syn::LitStr = input.parse()?;
                                input.parse::<syn::Token![=]>()?;
                                let value: syn::LitStr = input.parse()?;
                                header_vars.push((key.value(), value.value()));

                                let _ = input.parse::<syn::Token![,]>();
                            }
                            Ok(())
                        },
                        ml.tokens.clone(),
                    );
                }

                _ => {}
            }
        }

        if let (Some(method), Some(path)) = (http_method, path_suffix) {
            endpoints.push(ParsedHttpEndpointDetails {
                http_method: method,
                path_suffix: path,
                header_vars,
                auth_details,
                cors_options,
            });
        }
    }

    endpoints
}
