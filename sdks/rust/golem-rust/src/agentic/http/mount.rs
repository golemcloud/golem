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

use crate::agentic::http::path::parse_path;
use crate::golem_agentic::golem::agent::common::{
    AuthDetails, CorsOptions, HttpMountDetails, PathSegment,
};
use golem_schema::http::wit::wire as http;

use crate::agentic::http::validations::reject_query_param_in_string;

/// Parses the full POST path for an ID mount's create route.
pub fn get_http_create_route(path: &str) -> Result<Vec<PathSegment>, String> {
    reject_query_param_in_string(path, "HTTP create route")?;
    let segments = parse_path(path)?;
    if segments.iter().any(|segment| {
        !matches!(
            segment,
            PathSegment::SystemVariable(_) | PathSegment::Segment(http::PathSegment::Literal(_))
        )
    }) {
        return Err("HTTP create route can contain only literals and system variables".into());
    }
    Ok(segments)
}

pub fn get_http_mount_details(
    path: &str,
    auth: bool,
    phantom_agent: bool,
    cors_options: CorsOptions,
    web_suffix: Option<String>,
) -> Result<HttpMountDetails, String> {
    reject_query_param_in_string(path, "HTTP mount path")?;

    let segments = parse_path(path).map_err(|e| e.to_string())?;

    let web_suffix = match web_suffix {
        Some(suffix) => {
            reject_query_param_in_string(&suffix, "webhook_suffix")?;

            let parsed_suffix = parse_path(&suffix).map_err(|e| e.to_string())?;

            if parsed_suffix.is_empty() {
                return Err("webhook_suffix cannot be empty if provided".to_string());
            }

            parsed_suffix
        }
        None => vec![],
    };

    Ok(HttpMountDetails {
        path_prefix: segments,
        auth_details: Some(AuthDetails { required: auth }),
        phantom_agent,
        agent_id_binding: None,
        create_route: None,
        cors_options: cors_options.clone(),
        webhook_suffix: web_suffix,
        static_bindings: vec![],
        filesystem_bindings: vec![],
        file_response_headers: vec![],
        openapi_provider_method: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn create_path_is_full_and_has_no_application_capture() {
        assert!(get_http_create_route("/").unwrap().is_empty());
        let path = get_http_create_route("/chats/{agent-type}/create").unwrap();
        assert_eq!(path.len(), 3);
        assert!(matches!(path[1], PathSegment::SystemVariable(_)));
        for path in [
            "/chats/{id}",
            "/chats/{*rest}",
            "/chats?create=true",
            "chats",
            "/chats//create",
        ] {
            assert!(get_http_create_route(path).is_err(), "accepted {path}");
        }
    }
}
