// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::RichRequest;
use super::error::RequestHandlerError;
use super::route_resolver::ResolvedRouteEntry;
use super::{ResponseBody, RouteExecutionResult};
use golem_common::model::agent::HttpMethod;
use golem_service_base::custom_api::{CorsPreflightBehaviour, CorsPreflightMethodPolicy};
use http::{HeaderName, Method, StatusCode};
use std::collections::{BTreeSet, HashMap};
use tracing::debug;

pub fn handle_cors_preflight_behaviour(
    request: &RichRequest,
    cors_preflight: &CorsPreflightBehaviour,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let origin = request.origin()?.ok_or(RequestHandlerError::MissingValue {
        expected: "Origin header",
    })?;
    let requested_method = requested_preflight_method(request)?;
    let requested_headers = requested_preflight_headers(request)?;
    let policy = requested_method_policy(cors_preflight, &requested_method)?;

    let mut headers = HashMap::new();
    merge_vary_header(
        &mut headers,
        &[
            "Origin",
            "Access-Control-Request-Method",
            "Access-Control-Request-Headers",
        ],
    );

    let Some(policy) = policy else {
        return Ok(forbidden_response(headers));
    };

    if !policy
        .allowed_origins
        .iter()
        .any(|pattern| pattern.matches(origin))
    {
        return Ok(forbidden_response(headers));
    }

    if !requested_headers
        .iter()
        .all(|header_name| policy.allowed_headers.contains(header_name))
    {
        return Ok(forbidden_response(headers));
    }

    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        origin.to_string(),
    );
    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_METHODS,
        requested_method.to_string(),
    );
    if !requested_headers.is_empty() {
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_HEADERS,
            requested_headers.into_iter().collect::<Vec<_>>().join(", "),
        );
    }
    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        "true".to_string(),
    );
    headers.insert(http::header::ACCESS_CONTROL_MAX_AGE, "3600".to_string());

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

    merge_vary_header(&mut result.headers, &["Origin"]);

    if let Some(origin) = request.origin()?
        && cors.allowed_patterns.iter().any(|p| p.matches(origin))
    {
        result.headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            origin.to_string(),
        );
        result.headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            "true".to_string(),
        );
    }

    Ok(())
}

fn forbidden_response(headers: HashMap<HeaderName, String>) -> RouteExecutionResult {
    RouteExecutionResult {
        status: StatusCode::FORBIDDEN,
        headers,
        body: ResponseBody::NoBody,
    }
}

fn requested_preflight_method(request: &RichRequest) -> Result<Method, RequestHandlerError> {
    let value = request
        .header_string_value("access-control-request-method")?
        .ok_or(RequestHandlerError::MissingValue {
            expected: "Access-Control-Request-Method header",
        })?;

    Method::from_bytes(value.as_bytes()).map_err(|_| RequestHandlerError::ValueParsingFailed {
        value: value.to_string(),
        expected: "HTTP method",
    })
}

fn requested_preflight_headers(
    request: &RichRequest,
) -> Result<BTreeSet<String>, RequestHandlerError> {
    let Some(value) = request.header_string_value("access-control-request-headers")? else {
        return Ok(BTreeSet::new());
    };

    value
        .split(',')
        .map(str::trim)
        .filter(|header_name| !header_name.is_empty())
        .map(|header_name| {
            HeaderName::from_bytes(header_name.as_bytes())
                .map(|header_name| header_name.as_str().to_string())
                .map_err(|_| RequestHandlerError::ValueParsingFailed {
                    value: header_name.to_string(),
                    expected: "valid HTTP header name",
                })
        })
        .collect()
}

fn requested_method_policy<'a>(
    cors_preflight: &'a CorsPreflightBehaviour,
    requested_method: &Method,
) -> Result<Option<&'a CorsPreflightMethodPolicy>, RequestHandlerError> {
    for policy in &cors_preflight.method_policies {
        if method_matches(&policy.method, requested_method)? {
            return Ok(Some(policy));
        }
    }

    Ok(None)
}

fn method_matches(
    method: &HttpMethod,
    requested_method: &Method,
) -> Result<bool, RequestHandlerError> {
    Ok(render_http_method(method)? == requested_method.as_str())
}

fn render_http_method(method: &HttpMethod) -> Result<String, RequestHandlerError> {
    let converted = http::Method::try_from(method.clone())
        .map_err(|_| RequestHandlerError::invariant_violated("HttpMethod conversion error"))?;

    Ok(converted.to_string())
}

fn merge_vary_header(headers: &mut HashMap<HeaderName, String>, values: &[&str]) {
    let Some(existing) = headers.get(&http::header::VARY).cloned() else {
        headers.insert(http::header::VARY, values.join(", "));
        return;
    };

    let mut merged: Vec<String> = existing
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .collect();

    if merged.iter().any(|value| value == "*") {
        return;
    }

    for value in values {
        if !merged
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(value))
        {
            merged.push((*value).to_string());
        }
    }

    headers.insert(http::header::VARY, merged.join(", "));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_api::model::{RichCompiledRoute, RichRouteBehaviour, RichRouteSecurity};
    use golem_common::model::Empty;
    use golem_common::model::account::AccountId;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::custom_api::{
        CorsOptions, OpenApiSpecBehaviour, OriginPattern, PathSegment, RequestBodySchema,
    };
    use poem::{Body, Request};
    use std::sync::Arc;
    use test_r::test;

    #[test]
    fn preflight_validates_requested_headers_and_sets_credential_headers() {
        let request = Request::builder()
            .method(Method::OPTIONS)
            .header(http::header::ORIGIN, "https://frontend.example.com")
            .header(http::header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(http::header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
            .body(Body::empty());
        let request = RichRequest::new(request);

        let result = handle_cors_preflight_behaviour(
            &request,
            &CorsPreflightBehaviour {
                method_policies: vec![CorsPreflightMethodPolicy {
                    method: HttpMethod::Post(Empty {}),
                    allowed_origins: BTreeSet::from([OriginPattern(
                        "https://frontend.example.com".to_string(),
                    )]),
                    allowed_headers: BTreeSet::from(["content-type".to_string()]),
                }],
            },
        )
        .unwrap();

        assert_eq!(result.status, StatusCode::NO_CONTENT);
        assert_eq!(
            result
                .headers
                .get(&http::header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&"https://frontend.example.com".to_string())
        );
        assert_eq!(
            result
                .headers
                .get(&http::header::ACCESS_CONTROL_ALLOW_HEADERS),
            Some(&"content-type".to_string())
        );
        assert_eq!(
            result
                .headers
                .get(&http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
            Some(&"true".to_string())
        );
        assert_eq!(
            result.headers.get(&http::header::VARY),
            Some(
                &"Origin, Access-Control-Request-Method, Access-Control-Request-Headers"
                    .to_string(),
            )
        );
    }

    #[test]
    fn preflight_rejects_unknown_requested_headers_and_keeps_vary() {
        let request = Request::builder()
            .method(Method::OPTIONS)
            .header(http::header::ORIGIN, "https://frontend.example.com")
            .header(http::header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(http::header::ACCESS_CONTROL_REQUEST_HEADERS, "x-unexpected")
            .body(Body::empty());
        let request = RichRequest::new(request);

        let result = handle_cors_preflight_behaviour(
            &request,
            &CorsPreflightBehaviour {
                method_policies: vec![CorsPreflightMethodPolicy {
                    method: HttpMethod::Post(Empty {}),
                    allowed_origins: BTreeSet::from([OriginPattern(
                        "https://frontend.example.com".to_string(),
                    )]),
                    allowed_headers: BTreeSet::from(["content-type".to_string()]),
                }],
            },
        )
        .unwrap();

        assert_eq!(result.status, StatusCode::FORBIDDEN);
        assert_eq!(
            result.headers.get(&http::header::VARY),
            Some(
                &"Origin, Access-Control-Request-Method, Access-Control-Request-Headers"
                    .to_string(),
            )
        );
    }

    #[test]
    async fn outgoing_middleware_merges_vary_and_keeps_it_for_non_matching_origins() {
        let request = Request::builder()
            .method(Method::GET)
            .header(http::header::ORIGIN, "https://blocked.example.com")
            .body(Body::empty());
        let request = RichRequest::new(request);
        let mut result = RouteExecutionResult {
            status: StatusCode::OK,
            headers: HashMap::from([(http::header::VARY, "Accept-Encoding".to_string())]),
            body: ResponseBody::NoBody,
        };
        let resolved_route = resolved_route_with_cors(vec![OriginPattern(
            "https://frontend.example.com".to_string(),
        )]);

        apply_cors_outgoing_middleware(&mut result, &request, &resolved_route)
            .await
            .unwrap();

        assert_eq!(
            result.headers.get(&http::header::VARY),
            Some(&"Accept-Encoding, Origin".to_string())
        );
        assert!(
            !result
                .headers
                .contains_key(&http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }

    #[test]
    fn preflight_uses_method_specific_origins() {
        let request = Request::builder()
            .method(Method::OPTIONS)
            .header(http::header::ORIGIN, "https://other.example.com")
            .header(http::header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .body(Body::empty());
        let request = RichRequest::new(request);

        let result = handle_cors_preflight_behaviour(
            &request,
            &CorsPreflightBehaviour {
                method_policies: vec![
                    CorsPreflightMethodPolicy {
                        method: HttpMethod::Get(Empty {}),
                        allowed_origins: BTreeSet::from([OriginPattern("*".to_string())]),
                        allowed_headers: BTreeSet::new(),
                    },
                    CorsPreflightMethodPolicy {
                        method: HttpMethod::Post(Empty {}),
                        allowed_origins: BTreeSet::from([OriginPattern(
                            "https://frontend.example.com".to_string(),
                        )]),
                        allowed_headers: BTreeSet::new(),
                    },
                ],
            },
        )
        .unwrap();

        assert_eq!(result.status, StatusCode::FORBIDDEN);
        assert!(
            !result
                .headers
                .contains_key(&http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }

    fn resolved_route_with_cors(allowed_patterns: Vec<OriginPattern>) -> ResolvedRouteEntry {
        ResolvedRouteEntry {
            domain: golem_common::model::domain_registration::Domain("example.com".to_string()),
            route: Arc::new(RichCompiledRoute {
                account_id: AccountId(uuid::Uuid::nil()),
                environment_id: EnvironmentId(uuid::Uuid::nil()),
                route_id: 1,
                method: Method::GET,
                path: vec![PathSegment::Literal {
                    value: "notes".to_string(),
                }],
                body: RequestBodySchema::Unused,
                behavior: RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {}),
                security: RichRouteSecurity::None,
                cors: CorsOptions { allowed_patterns },
            }),
            captured_path_parameters: vec![],
            openapi_spec: None,
        }
    }
}
