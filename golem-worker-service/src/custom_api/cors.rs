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
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use std::collections::BTreeSet;
use tracing::debug;

pub fn is_cors_preflight(request: &poem::Request) -> bool {
    request.method() == Method::OPTIONS
        && request.headers().contains_key(http::header::ORIGIN)
        && request
            .headers()
            .contains_key(http::header::ACCESS_CONTROL_REQUEST_METHOD)
}

pub fn handle_selected_preflight(
    request: &RichRequest,
    selected: &ResolvedRouteEntry,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    use super::{RichRouteBehaviour, RichRouteSecurity};
    let requested_method = requested_preflight_method(&request.underlying)?;
    let parameters = match &selected.route.behavior {
        RichRouteBehaviour::CallAgent(agent) => agent.method_parameters.as_slice(),
        _ => &[],
    };
    let session = match &selected.route.security {
        RichRouteSecurity::SessionFromHeader(s) => Some(s.header_name.as_str()),
        _ => None,
    };
    let mut allowed_headers = golem_service_base::custom_api::cors_allowed_request_headers(
        &selected.route.body,
        parameters,
        session,
    );
    match &selected.route.behavior {
        RichRouteBehaviour::CallAgent(agent)
            if agent.route_mode
                == golem_service_base::custom_api::AgentRouteMode::DurableStreams =>
        {
            allowed_headers.extend(
                golem_service_base::custom_api::DURABLE_STREAM_REQUEST_HEADERS
                    .iter()
                    .map(|header| (*header).to_owned()),
            );
        }
        RichRouteBehaviour::HttpRouter(_) => {
            allowed_headers.extend(requested_preflight_headers(request)?)
        }
        RichRouteBehaviour::AgentFilesystem(_) => allowed_headers.extend(
            [
                "range",
                "if-range",
                "if-match",
                "if-none-match",
                "if-modified-since",
                "if-unmodified-since",
            ]
            .map(str::to_string),
        ),
        _ => {}
    }
    handle_cors_preflight_behaviour(
        request,
        &CorsPreflightBehaviour {
            method_policies: vec![CorsPreflightMethodPolicy {
                method: HttpMethod::Custom(golem_common::model::agent::CustomHttpMethod {
                    value: requested_method.to_string(),
                }),
                allowed_origins: selected
                    .route
                    .cors
                    .allowed_patterns
                    .iter()
                    .cloned()
                    .collect(),
                allowed_headers,
            }],
        },
    )
}

pub fn denied_preflight() -> RouteExecutionResult {
    let mut headers = HeaderMap::new();
    merge_vary_header(
        &mut headers,
        &[
            "Origin",
            "Access-Control-Request-Method",
            "Access-Control-Request-Headers",
        ],
    );
    forbidden_response(headers)
}

pub fn handle_cors_preflight_behaviour(
    request: &RichRequest,
    cors_preflight: &CorsPreflightBehaviour,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let origin = request.origin()?.ok_or(RequestHandlerError::MissingValue {
        expected: "Origin header",
    })?;
    let requested_method = requested_preflight_method(&request.underlying)?;
    let requested_headers = requested_preflight_headers(request)?;
    let policy = requested_method_policy(cors_preflight, &requested_method)?;

    let mut headers = HeaderMap::new();
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
        HeaderValue::from_str(origin).map_err(anyhow::Error::from)?,
    );
    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_str(requested_method.as_str()).map_err(anyhow::Error::from)?,
    );
    if !requested_headers.is_empty() {
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_str(&requested_headers.into_iter().collect::<Vec<_>>().join(", "))
                .map_err(anyhow::Error::from)?,
        );
    }
    headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(
        http::header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("3600"),
    );

    Ok(RouteExecutionResult {
        status: StatusCode::NO_CONTENT,
        headers,
        body: ResponseBody::NoBody,
    })
}

pub fn apply_cors_outgoing_middleware(
    response: &mut poem::Response,
    request: &RichRequest,
    resolved_route: &ResolvedRouteEntry,
) -> Result<(), RequestHandlerError> {
    debug!("Begin executing SetCorsResponseHeadersMiddleware");

    for name in [
        http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        http::header::ACCESS_CONTROL_ALLOW_METHODS,
        http::header::ACCESS_CONTROL_MAX_AGE,
        http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
    ] {
        response.headers_mut().remove(name);
    }
    if matches!(&resolved_route.route.behavior, super::RichRouteBehaviour::CallAgent(behaviour)
        if behaviour.route_mode == golem_service_base::custom_api::AgentRouteMode::DurableStreams)
    {
        response.headers_mut().insert(
            http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("Stream-Next-Offset, Stream-Closed, Stream-Cancelled, Stream-Up-To-Date, Stream-Cursor, Stream-SSE-Data-Encoding, Producer-Epoch, Producer-Seq, Producer-Expected-Seq, Producer-Received-Seq, ETag, Location, Retry-After"),
        );
    }
    let cors = &resolved_route.route.cors;

    if cors.allowed_patterns.is_empty() {
        return Ok(());
    }

    let mut headers = HeaderMap::new();
    let vary = response
        .headers()
        .get_all(http::header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    headers.insert(
        http::header::VARY,
        HeaderValue::from_str(&vary).map_err(anyhow::Error::from)?,
    );
    merge_vary_header(&mut headers, &["Origin"]);

    if let Some(origin) = request.origin()?
        && cors.allowed_patterns.iter().any(|p| p.matches(origin))
    {
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_str(origin).map_err(anyhow::Error::from)?,
        );
        headers.insert(
            http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }

    for (name, value) in &headers {
        response.headers_mut().insert(name, value.clone());
    }
    Ok(())
}

fn forbidden_response(headers: HeaderMap) -> RouteExecutionResult {
    RouteExecutionResult {
        status: StatusCode::FORBIDDEN,
        headers,
        body: ResponseBody::NoBody,
    }
}

pub fn requested_preflight_method(request: &poem::Request) -> Result<Method, RequestHandlerError> {
    let mut values = request
        .headers()
        .get_all(http::header::ACCESS_CONTROL_REQUEST_METHOD)
        .iter();
    let value = values.next().ok_or(RequestHandlerError::MissingValue {
        expected: "Access-Control-Request-Method header",
    })?;
    let value = value
        .to_str()
        .map_err(|_| RequestHandlerError::ValueParsingFailed {
            value: "non-ASCII".into(),
            expected: "HTTP method",
        })?;
    if values.next().is_some() {
        return Err(RequestHandlerError::ValueParsingFailed {
            value: "multiple values".into(),
            expected: "one HTTP method",
        });
    }
    Method::from_bytes(value.as_bytes()).map_err(|_| RequestHandlerError::ValueParsingFailed {
        value: value.to_string(),
        expected: "HTTP method",
    })
}

fn requested_preflight_headers(
    request: &RichRequest,
) -> Result<BTreeSet<String>, RequestHandlerError> {
    let mut headers = BTreeSet::new();
    for value in request
        .headers()
        .get_all(http::header::ACCESS_CONTROL_REQUEST_HEADERS)
    {
        let value = value
            .to_str()
            .map_err(|_| RequestHandlerError::HeaderIsNotAscii {
                header_name: "Access-Control-Request-Headers".into(),
            })?;
        for header_name in value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let name = HeaderName::from_bytes(header_name.as_bytes())
                .map(|header_name| header_name.as_str().to_string())
                .map_err(|_| RequestHandlerError::ValueParsingFailed {
                    value: header_name.to_string(),
                    expected: "valid HTTP header name",
                })?;
            headers.insert(name);
        }
    }
    Ok(headers)
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

fn merge_vary_header(headers: &mut HeaderMap, values: &[&str]) {
    let Some(existing) = headers
        .get(&http::header::VARY)
        .and_then(|value| value.to_str().ok())
    else {
        headers.insert(
            http::header::VARY,
            HeaderValue::from_str(&values.join(", ")).unwrap(),
        );
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

    headers.insert(
        http::header::VARY,
        HeaderValue::from_str(&merged.join(", ")).unwrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_api::model::{RichCompiledRoute, RichRouteBehaviour, RichRouteSecurity};
    use golem_common::model::Empty;
    use golem_common::model::account::AccountId;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::custom_api::{
        CorsOptions, OpenApiSpecBehaviour, OpenApiSpecFormat, OriginPattern, PathSegment,
        RequestBodySchema,
    };
    use poem::{Body, Request};
    use std::sync::Arc;
    use test_r::test;

    #[test]
    async fn selected_durable_stream_preflight_allows_protocol_headers_only_on_stream_routes() {
        use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
        use golem_service_base::custom_api::{AgentRouteMode, RouteBehaviour};

        for (mode, headers, expected_status) in [
            (
                AgentRouteMode::DurableStreams,
                "Producer-Id, Producer-Epoch, Producer-Seq, Stream-Closed, Stream-Ttl",
                StatusCode::NO_CONTENT,
            ),
            (
                AgentRouteMode::DurableStreams,
                "If-None-Match, Stream-Expires-At, Stream-Forked-From, Content-Type",
                StatusCode::NO_CONTENT,
            ),
            (AgentRouteMode::Rest, "Producer-Id", StatusCode::FORBIDDEN),
            (
                AgentRouteMode::DurableStreams,
                "X-Unexpected",
                StatusCode::FORBIDDEN,
            ),
        ] {
            let mut route = test_route(1, "/stream", Some("POST"), "typed");
            route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
            let RouteBehaviour::CallAgent(agent) = &mut route.behavior else {
                panic!("expected typed route");
            };
            agent.route_mode = mode;
            let resolver = test_resolver(vec![route]);
            let request = Request::builder()
                .uri("/stream".parse().unwrap())
                .method(Method::OPTIONS)
                .header("host", "example.com")
                .header("origin", "https://client.example")
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", headers)
                .finish();
            let response = crate::custom_api::request_handler::handle_preflight(&resolver, request)
                .await
                .unwrap();
            assert_eq!(response.status(), expected_status, "{mode:?}: {headers}");
            if expected_status == StatusCode::NO_CONTENT {
                assert_eq!(
                    response.headers()[http::header::ACCESS_CONTROL_ALLOW_METHODS],
                    "POST"
                );
                let allowed = response.headers()[http::header::ACCESS_CONTROL_ALLOW_HEADERS]
                    .to_str()
                    .unwrap();
                for header in headers.split(", ") {
                    assert!(
                        allowed
                            .split(", ")
                            .any(|name| name == header.to_ascii_lowercase())
                    );
                }
            }
        }
    }

    #[test]
    async fn requested_method_selects_one_policy_without_parent_inheritance() {
        use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
        use golem_service_base::custom_api::{
            RouteBehaviour, RouteSecurity, SessionFromHeaderRouteSecurity,
        };
        let mut parent = test_route(1, "/", None, "router");
        parent.cors.allowed_patterns = vec![OriginPattern("*".into())];
        let mut typed = test_route(2, "/a/{id}", Some("POST"), "typed");
        typed.cors.allowed_patterns = vec![OriginPattern("https://typed.example".into())];
        typed.body = RequestBodySchema::JsonBody {
            expected: golem_service_base::custom_api::CompiledSchema {
                graph: golem_common::schema::SchemaGraph::anonymous(
                    golem_common::schema::SchemaType::string(),
                ),
            },
        };
        typed.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
            header_name: "X-Session".into(),
        });
        let mut files = test_route(3, "/a/fixed", None, "filesystem");
        files.cors.allowed_patterns = vec![OriginPattern("https://files.example".into())];
        let mut options = test_route(4, "/a/fixed", Some("OPTIONS"), "typed");
        options.cors.allowed_patterns = vec![OriginPattern("https://options.example".into())];
        let mut projection = test_route(5, "/a/{id}", Some("OPTIONS"), "typed");
        projection.behavior = RouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
            method_policies: vec![],
        });
        let resolver = test_resolver(vec![parent, typed, files, options, projection]);
        for (method, origin, headers, selected, status) in [
            (
                "POST",
                "https://typed.example",
                "Content-Type, X-Session",
                2,
                204,
            ),
            ("POST", "https://files.example", "", 2, 403),
            ("POST", "https://typed.example", "x-extra", 2, 403),
            ("GET", "https://files.example", "Range, If-Range", 3, 204),
            ("HEAD", "https://files.example", "If-None-Match", 3, 204),
            ("GET", "https://files.example", "x-extra", 3, 403),
            ("OPTIONS", "https://options.example", "", 4, 204),
            ("pUrGe", "https://router.example", "X-Extra, Range", 1, 204),
        ] {
            let request = Request::builder()
                .uri("/a/fixed".parse().unwrap())
                .method(Method::OPTIONS)
                .header("host", "example.com")
                .header("origin", origin)
                .header("access-control-request-method", method)
                .header("access-control-request-headers", headers)
                .finish();
            assert!(is_cors_preflight(&request));
            let route = resolver
                .resolve_matching_route_for_method(
                    &request,
                    &requested_preflight_method(&request).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(route.route.route_id, selected);
            let result = handle_selected_preflight(&RichRequest::new(request), &route).unwrap();
            assert_eq!(
                result.status.as_u16(),
                status,
                "{method} {origin} {headers}"
            );
            assert_eq!(
                result.headers[&http::header::VARY],
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers"
            );
            if status == 204 {
                assert_eq!(
                    result.headers[&http::header::ACCESS_CONTROL_ALLOW_METHODS],
                    method
                );
                assert_eq!(
                    result.headers[&http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                    origin
                );
                assert_eq!(
                    result.headers[&http::header::ACCESS_CONTROL_MAX_AGE],
                    "3600"
                );
            } else {
                assert!(
                    !result
                        .headers
                        .contains_key(&http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                );
            }
        }
        for (method, expected) in [(Method::GET, 3), (Method::POST, 1)] {
            let request = Request::builder()
                .uri("/a/fixed/".parse().unwrap())
                .header("host", "example.com")
                .finish();
            assert_eq!(
                resolver
                    .resolve_matching_route_for_method(&request, &method)
                    .await
                    .unwrap()
                    .route
                    .route_id,
                expected
            );
        }
        for (path, id) in [("/a/fixed", 4), ("/a/other", 1)] {
            let request = Request::builder()
                .uri(path.parse().unwrap())
                .method(Method::OPTIONS)
                .header("host", "example.com")
                .finish();
            assert!(!is_cors_preflight(&request));
            assert_eq!(
                resolver
                    .resolve_matching_route(&request)
                    .await
                    .unwrap()
                    .route
                    .route_id,
                id
            );
        }
    }

    #[test]
    async fn shared_cors_not_any_corpus() {
        use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "route-cors-not-any")
            .unwrap();
        let id = case["id"].as_str().unwrap();
        let input = &case["input"];
        let mount = &input["mounts"][0];
        let mut route = test_route(1, mount["path"].as_str().unwrap(), None, "router");
        route.cors.allowed_patterns = mount["cors_origins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| OriginPattern(s.as_str().unwrap().into()))
            .collect();
        let resolver = test_resolver(vec![route]);
        let mut request = Request::builder()
            .uri(input["target"].as_str().unwrap().parse().unwrap())
            .method(input["method"].as_str().unwrap().parse().unwrap())
            .header("host", "example.com");
        for header in input["headers"].as_array().unwrap() {
            request = request.header(header[0].as_str().unwrap(), header[1].as_str().unwrap());
        }
        let request = request.finish();
        assert!(is_cors_preflight(&request), "{id}");
        let response = crate::custom_api::request_handler::handle_preflight(&resolver, request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{id}");
        for header in case["expect"]["headers"].as_array().unwrap() {
            assert_eq!(
                response.headers()[header[0].as_str().unwrap()],
                header[1].as_str().unwrap(),
                "{id}"
            );
        }
    }

    #[test]
    fn preflight_requires_both_headers_and_a_single_case_sensitive_method() {
        for (origin, method, expected) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            let mut request = Request::builder().method(Method::OPTIONS);
            if origin {
                request = request.header("origin", "https://client.example");
            }
            if method {
                request = request.header("access-control-request-method", "pUrGe");
            }
            let request = request.finish();
            assert_eq!(is_cors_preflight(&request), expected);
            if expected {
                assert_eq!(
                    requested_preflight_method(&request).unwrap().as_str(),
                    "pUrGe"
                );
            }
        }
        for method in ["GET, POST", "GET POST", ""] {
            let request = Request::builder()
                .header("access-control-request-method", method)
                .finish();
            assert!(requested_preflight_method(&request).is_err());
        }
        let mut request = Request::builder()
            .header("access-control-request-method", "GET")
            .finish();
        request.headers_mut().append(
            http::header::ACCESS_CONTROL_REQUEST_METHOD,
            "POST".parse().unwrap(),
        );
        assert!(requested_preflight_method(&request).is_err());
    }

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
                .get(&http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("https://frontend.example.com")
        );
        assert_eq!(
            result
                .headers
                .get(&http::header::ACCESS_CONTROL_ALLOW_HEADERS)
                .and_then(|value| value.to_str().ok()),
            Some("content-type")
        );
        assert_eq!(
            result
                .headers
                .get(&http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
        assert_eq!(
            result
                .headers
                .get(&http::header::VARY)
                .and_then(|value| value.to_str().ok()),
            Some("Origin, Access-Control-Request-Method, Access-Control-Request-Headers")
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
            result
                .headers
                .get(&http::header::VARY)
                .and_then(|value| value.to_str().ok()),
            Some("Origin, Access-Control-Request-Method, Access-Control-Request-Headers")
        );
    }

    #[test]
    async fn outgoing_middleware_merges_vary_and_keeps_it_for_non_matching_origins() {
        let request = Request::builder()
            .method(Method::GET)
            .header(http::header::ORIGIN, "https://blocked.example.com")
            .body(Body::empty());
        let request = RichRequest::new(request);
        let mut result = poem::Response::builder()
            .header(http::header::VARY, "Accept-Encoding")
            .finish();
        let resolved_route = resolved_route_with_cors(vec![OriginPattern(
            "https://frontend.example.com".to_string(),
        )]);

        apply_cors_outgoing_middleware(&mut result, &request, &resolved_route).unwrap();

        assert_eq!(
            result.headers().get(&http::header::VARY).unwrap(),
            "Accept-Encoding, Origin"
        );
        assert!(
            !result
                .headers()
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
            public_scheme: "http".to_string(),
            public_authority: "example.com".to_string(),
            route: Arc::new(RichCompiledRoute {
                account_id: AccountId(uuid::Uuid::nil()),
                account_email: golem_common::model::account::AccountEmail::new("test@golem"),
                environment_id: EnvironmentId(uuid::Uuid::nil()),
                deployment_revision: golem_common::model::deployment::DeploymentRevision::INITIAL,
                route_id: 1,
                route_match: HttpMethod::Get(Empty {}).into(),
                path: vec![PathSegment::Literal {
                    value: "notes".to_string(),
                }],
                body: RequestBodySchema::Unused,
                behavior: RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                    format: OpenApiSpecFormat::Json,
                }),
                security: RichRouteSecurity::None,
                cors: CorsOptions { allowed_patterns },
            }),
            captured_path_parameters: vec![],
            request_target: golem_common::model::agent::http_files::HttpRequestTarget::parse(
                "/notes",
            )
            .unwrap(),
            openapi_spec: None,
        }
    }
}
