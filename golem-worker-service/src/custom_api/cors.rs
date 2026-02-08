// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::error::RequestHandlerError;
use super::RichRequest;
use super::route_resolver::ResolvedRouteEntry;
use super::{ResponseBody, RouteExecutionResult};
use golem_common::model::agent::HttpMethod;
use golem_service_base::custom_api::OriginPattern;
use http::StatusCode;
use std::collections::{BTreeSet, HashMap};
use tracing::debug;

pub fn handle_cors_preflight_behaviour(
    request: &RichRequest,
    allowed_origins: &BTreeSet<OriginPattern>,
    allowed_methods: &BTreeSet<HttpMethod>,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let origin = request.origin()?.ok_or(RequestHandlerError::MissingValue {
        expected: "Origin header",
    })?;

    let origin_allowed = allowed_origins
        .iter()
        .any(|pattern| pattern.matches(origin));
    if !origin_allowed {
        return Ok(RouteExecutionResult {
            status: StatusCode::FORBIDDEN,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        });
    }

    let allow_methods = allowed_methods
        .iter()
        .map(|m| {
            let converted = http::Method::try_from(m.clone()).map_err(|_| {
                RequestHandlerError::invariant_violated("HttpMethod conversion error")
            })?;
            let rendered = converted.to_string();
            Ok::<_, RequestHandlerError>(rendered)
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");

    let mut headers = HashMap::new();

    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        origin.to_string(),
    );
    headers.insert(http::header::ACCESS_CONTROL_ALLOW_METHODS, allow_methods);
    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        "Content-Type, Authorization".to_string(),
    );
    headers.insert(http::header::ACCESS_CONTROL_MAX_AGE, "3600".to_string());
    headers.insert(http::header::VARY, "Origin".to_string());

    Ok(RouteExecutionResult {
        status: StatusCode::NO_CONTENT,
        headers,
        body: ResponseBody::NoBody,
    })
}

pub async fn apply_cors_outgoing_middleware(
    result: &mut RouteExecutionResult,
    request: &RichRequest,
    resolved_route: &ResolvedRouteEntry,
) -> Result<(), RequestHandlerError> {
    debug!("Begin executing SetCorsResponseHeadersMiddleware");

    let cors = &resolved_route.route.cors;

    if cors.allowed_patterns.is_empty() {
        return Ok(());
    }

    if let Some(origin) = request.origin()?
        && cors.allowed_patterns.iter().any(|p| p.matches(origin))
    {
        result.headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            origin.to_string(),
        );
        result
            .headers
            .insert(http::header::VARY, "Origin".to_string());
        result.headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            "true".to_string(),
        );
    }

    Ok(())
}
