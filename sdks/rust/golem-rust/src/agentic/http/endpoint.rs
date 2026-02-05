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

use crate::agentic::http::path::parse_path;
use crate::agentic::http::query::parse_query;
use crate::golem_agentic::golem::agent::common::{
    AuthDetails, CorsOptions, HeaderVariable, HttpEndpointDetails, HttpMethod,
};

pub fn get_http_endpoint_details(
    method: &str,
    path: &str,
    auth: Option<bool>,
    cors_options: Vec<String>,
    http_headers: Vec<(String, String)>,
) -> Result<HttpEndpointDetails, String> {
    let PathAndQuery { path, query } = split_path_and_query(path);

    let path_suffix = parse_path(&path).map_err(|e| e.to_string())?;

    let query_vars = match query {
        Some(q) => parse_query(q.as_str()).map_err(|e| e.to_string())?,
        None => vec![],
    };

    let http_method = match method {
        "get" => HttpMethod::Get,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "delete" => HttpMethod::Delete,
        "patch" => HttpMethod::Patch,
        "head" => HttpMethod::Head,
        "options" => HttpMethod::Options,
        "connect" => HttpMethod::Connect,
        "trace" => HttpMethod::Trace,
        other => return Err(format!("Unsupported HTTP method: {}", other)),
    };

    let header_vars: Vec<HeaderVariable> = http_headers
        .into_iter()
        .map(|(header_name, variable_name)| HeaderVariable {
            header_name,
            variable_name,
        })
        .collect();

    Ok(HttpEndpointDetails {
        http_method,
        path_suffix,
        header_vars,
        query_vars,
        auth_details: auth.map(|required| AuthDetails { required }),
        cors_options: CorsOptions {
            allowed_patterns: cors_options,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathAndQuery {
    path: String,
    query: Option<String>,
}

fn split_path_and_query(path_with_query: &str) -> PathAndQuery {
    let mut parts = path_with_query.splitn(2, '?');

    PathAndQuery {
        path: parts.next().unwrap_or("").to_string(),
        query: parts.next().map(|q| q.to_string()),
    }
}
