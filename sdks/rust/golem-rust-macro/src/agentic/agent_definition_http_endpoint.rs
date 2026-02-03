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

pub struct ParsedHttpEndpointDetails {
    pub http_method: String,
    pub path_suffix: String,
    pub header_vars: Vec<(String, String)>,
    pub auth_details: bool,
    pub cors_options: Vec<String>,
}

pub fn extract_http_endpoints(attrs: &[syn::Attribute]) -> Vec<ParsedHttpEndpointDetails> {
    let mut endpoints = Vec::new();

    for attr in attrs {
        if !attr.path().is_ident("endpoint") {
            continue;
        }

        let mut http_method: Option<String> = None;
        let mut path_suffix: Option<String> = None;
        let mut header_vars: Vec<(String, String)> = Vec::new();
        let mut auth_details: bool = false;
        let mut cors_options: Vec<String> = Vec::new();

        let _ = attr.parse_nested_meta(|meta| {
            let ident = meta.path.get_ident().map(|i| i.to_string());

            match ident.as_deref() {
                Some("get") | Some("post") | Some("put") | Some("delete") => {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    http_method = Some(ident.unwrap());
                    path_suffix = Some(value.value());
                    Ok(())
                }

                Some("auth") => {
                    let value: syn::LitBool = meta.value()?.parse()?;
                    auth_details = value.value;
                    Ok(())
                }

                // headers("X-Id" = "comment")
                Some("headers") => meta.parse_nested_meta(|header| {
                    let key = header.path.get_ident().unwrap().to_string();

                    let value: syn::LitStr = header.value()?.parse()?;

                    header_vars.push((key, value.value()));
                    Ok(())
                }),

                Some("cors") => {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    cors_options.push(value.value());
                    Ok(())
                }

                _ => Ok(()),
            }
        });

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
