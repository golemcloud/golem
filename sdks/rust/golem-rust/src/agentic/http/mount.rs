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
use crate::golem_agentic::golem::agent::common::{AuthDetails, CorsOptions, HttpMountDetails};

use crate::agentic::http::validations::reject_query_param_in_string;

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
        cors_options: cors_options.clone(),
        webhook_suffix: web_suffix,
    })
}
