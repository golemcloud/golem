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

use super::call_agent::CallAgentHandler;
use super::cors::{
    apply_cors_outgoing_middleware, denied_preflight, handle_selected_preflight, is_cors_preflight,
    requested_preflight_method,
};
use super::error::RequestHandlerError;
use super::model::RichRouteBehaviour;
use super::mounted_dispatch::{PendingMountBackend, dispatch_mount};
use super::oidc::handler::OidcHandler;
use super::route_resolver::{ResolvedRouteEntry, RouteResolver, RouteResolverError};
use super::session_from_header_security::apply_session_from_header_security_middleware;
use super::webhooks::WebhookCallbackHandler;
use super::{OidcCallbackBehaviour, ResponseBody, RichRouteSecurity, RouteExecutionResult};
use crate::custom_api::RichRequest;
use anyhow::anyhow;
use golem_schema::schema::render::json_value::to_json_value_redacted;
use golem_service_base::custom_api::OpenApiSpecBehaviour;
use golem_service_base::custom_api::OpenApiSpecFormat;
use http::StatusCode;
use poem::{Request, Response};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{Instrument, debug};

pub struct RequestHandler {
    route_resolver: Arc<RouteResolver>,
    call_agent_handler: Arc<CallAgentHandler>,
    oidc_handler: Arc<OidcHandler>,
    webhook_callback_handler: Arc<WebhookCallbackHandler>,
}

#[derive(Debug)]
pub struct RequestFailure {
    pub error: RequestHandlerError,
    pub cors_headers: http::HeaderMap,
}

impl From<RequestHandlerError> for RequestFailure {
    fn from(error: RequestHandlerError) -> Self {
        Self {
            error,
            cors_headers: http::HeaderMap::new(),
        }
    }
}

#[allow(irrefutable_let_patterns)]
impl RequestHandler {
    pub fn new(
        route_resolver: Arc<RouteResolver>,
        call_agent_handler: Arc<CallAgentHandler>,
        oidc_handler: Arc<OidcHandler>,
        webhook_callback_handler: Arc<WebhookCallbackHandler>,
    ) -> Self {
        Self {
            route_resolver,
            call_agent_handler,
            oidc_handler,
            webhook_callback_handler,
        }
    }

    pub async fn handle_request(&self, request: Request) -> Result<Response, RequestFailure> {
        debug!("Begin http request handling for request {request:?}");

        if is_cors_preflight(&request) {
            return handle_preflight(&self.route_resolver, request).await;
        }
        let matching_route = self
            .route_resolver
            .resolve_matching_route(&request)
            .await
            .map_err(RequestHandlerError::from)?;
        let mut request = RichRequest::new(request);
        let request_method = request.underlying.method().clone();

        let execution_result = require_available_security(&matching_route,
            self.execute_route_and_middlewares(&mut request, &matching_route))
            .instrument(tracing::span!(
                tracing::Level::INFO,
                "handle_route",
                domain = %matching_route.domain,
                method = %request_method,
                route = %matching_route.route.path.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("/")
            ))
            .await;

        finish_selected_response(execution_result, &request, &matching_route)
    }

    async fn execute_route_and_middlewares(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if let Some(short_circuit) = self
            .oidc_handler
            .apply_oidc_incoming_middleware(request, resolved_route)
            .await?
        {
            return Ok(short_circuit);
        }

        if let Some(short_circuit) =
            apply_session_from_header_security_middleware(request, resolved_route)?
        {
            return Ok(short_circuit);
        }

        self.execute_route(request, resolved_route).await
    }

    async fn execute_route(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        match &resolved_route.route.behavior {
            RichRouteBehaviour::CallAgent(behaviour) => {
                self.call_agent_handler
                    .handle_call_agent_behaviour(request, resolved_route, behaviour)
                    .await
            }

            RichRouteBehaviour::CorsPreflight(_) => Err(RequestHandlerError::invariant_violated(
                "OpenAPI preflight projection selected for dispatch",
            )),

            RichRouteBehaviour::OidcCallback(OidcCallbackBehaviour { security_scheme }) => {
                self.oidc_handler
                    .handle_oidc_callback_behaviour(request, security_scheme)
                    .await
            }

            RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { format }) => {
                let spec = resolved_route
                    .openapi_spec
                    .clone()
                    .ok_or(RequestHandlerError::OpenApiSpecGenerationFailed)?;

                Ok(RouteExecutionResult {
                    status: StatusCode::OK,
                    headers: HashMap::new(),
                    body: ResponseBody::OpenApiSchema {
                        spec,
                        format: *format,
                    },
                })
            }

            RichRouteBehaviour::WebhookCallback(behaviour) => {
                self.webhook_callback_handler
                    .handle_webhook_callback_behaviour(request, resolved_route, behaviour)
                    .await
            }
            RichRouteBehaviour::HttpRouter(_) | RichRouteBehaviour::AgentFilesystem(_) => {
                dispatch_mount(request, resolved_route, &mut PendingMountBackend).await
            }
        }
    }
}

pub(super) async fn handle_preflight(
    resolver: &RouteResolver,
    request: Request,
) -> Result<Response, RequestFailure> {
    let method = requested_preflight_method(&request)?;
    let result = match resolver
        .resolve_matching_route_for_method(&request, &method)
        .await
    {
        Ok(selected) => handle_selected_preflight(&RichRequest::new(request), &selected)?,
        Err(RouteResolverError::NoMatchingRoute) => denied_preflight(),
        Err(error) => return Err(RequestHandlerError::from(error).into()),
    };
    route_execution_result_to_response(result).map_err(Into::into)
}

async fn require_available_security(
    selected: &ResolvedRouteEntry,
    execute: impl std::future::Future<Output = Result<RouteExecutionResult, RequestHandlerError>>,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    if matches!(selected.route.security, RichRouteSecurity::Unavailable) {
        Ok(RouteExecutionResult {
            status: StatusCode::SERVICE_UNAVAILABLE,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        })
    } else {
        execute.await
    }
}

fn finish_selected_response(
    result: Result<RouteExecutionResult, RequestHandlerError>,
    request: &RichRequest,
    selected: &ResolvedRouteEntry,
) -> Result<Response, RequestFailure> {
    match result.and_then(route_execution_result_to_response) {
        Ok(mut response) => {
            apply_cors_outgoing_middleware(&mut response, request, selected)?;
            Ok(response)
        }
        Err(error) => {
            let mut response = Response::default();
            apply_cors_outgoing_middleware(&mut response, request, selected)?;
            Err(RequestFailure {
                error,
                cors_headers: response.headers().clone(),
            })
        }
    }
}

fn route_execution_result_to_response(
    result: RouteExecutionResult,
) -> Result<Response, RequestHandlerError> {
    let mut response_builder = Response::builder().status(result.status);

    for (name, value) in result.headers {
        response_builder = response_builder.header(name, value);
    }

    match result.body {
        ResponseBody::NoBody => Ok(response_builder.finish()),

        ResponseBody::ComponentModelJsonBody { body } => {
            let body = poem::Body::from_json(
                to_json_value_redacted(body.graph(), body.root_type(), body.value())
                    .map_err(|e| anyhow!("ComponentModelJsonBody conversion error: {e}"))?,
            )
            .map_err(anyhow::Error::from)?;

            Ok(response_builder
                .body(body)
                .set_content_type("application/json"))
        }

        ResponseBody::UnstructuredBinaryBody { body } => Ok(response_builder
            .body(body.data)
            .set_content_type(body.binary_type.mime_type)),

        ResponseBody::UnstructuredTextBody { body } => {
            let mut response_builder = response_builder.content_type("text/plain; charset=utf-8");
            if let Some(text_type) = &body.text_type {
                let trimmed = text_type.language_code.trim();
                if !trimmed.is_empty()
                    && !trimmed.contains(',')
                    && trimmed
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    response_builder =
                        response_builder.header(http::header::CONTENT_LANGUAGE, trimmed);
                }
            }
            Ok(response_builder.body(body.data))
        }

        ResponseBody::OpenApiSchema { spec: body, format } => {
            let response = match format {
                OpenApiSpecFormat::Json => {
                    let body_json = serde_json::to_vec(&body.0)
                        .map_err(|e| anyhow!("OpenApiSchema body serialization error: {e}"))?;

                    response_builder
                        .body(body_json)
                        .set_content_type("application/json")
                }
                OpenApiSpecFormat::Yaml => {
                    let body_yaml = serde_yaml::to_string(&body.0)
                        .map_err(|e| anyhow!("OpenApiSchema body serialization error: {e}"))?;

                    response_builder
                        .body(body_yaml)
                        .set_content_type("application/yaml")
                }
            };

            Ok(response)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::common::ApiEndpointError;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_service_base::custom_api::{
        OriginPattern, RouteSecurity, SecuritySchemeRouteSecurity, SessionFromHeaderRouteSecurity,
    };
    use poem::IntoResponse;
    use test_r::test;

    struct UnknownSiteLookup;

    #[async_trait::async_trait]
    impl crate::custom_api::api_definition_lookup::HttpApiDefinitionsLookup for UnknownSiteLookup {
        async fn get(
            &self,
            domain: &golem_common::model::domain_registration::Domain,
        ) -> Result<
            golem_service_base::custom_api::CompiledRoutes,
            crate::custom_api::api_definition_lookup::ApiDefinitionLookupError,
        > {
            Err(
                crate::custom_api::api_definition_lookup::ApiDefinitionLookupError::UnknownSite(
                    domain.clone(),
                ),
            )
        }
    }

    #[test]
    async fn preflight_terminal_responses_do_not_poll_body_or_authenticate() {
        let mut route = test_route(1, "/files", None, "filesystem");
        route.security = RouteSecurity::Unavailable;
        route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
        let resolver = test_resolver(vec![route]);
        let unknown = RouteResolver::new(
            &crate::config::RouteResolverConfig::default(),
            Arc::new(UnknownSiteLookup),
        );
        for (resolver, method, path, status) in [
            (&resolver, "GET", "/files/x", 204),
            (&resolver, "HEAD", "/files/x", 204),
            (&resolver, "POST", "/files/x", 403),
            (&resolver, "GET", "/missing", 403),
            (&resolver, "GET POST", "/files/x", 400),
            (&resolver, "GET", "/files/%2fprivate", 400),
            (&unknown, "GET", "/files/x", 404),
        ] {
            let body = poem::Body::from_bytes_stream(futures::stream::poll_fn(
                |_| -> std::task::Poll<Option<Result<bytes::Bytes, std::io::Error>>> {
                    panic!("Preflight must not consume a request body")
                },
            ));
            let request = Request::builder()
                .method(http::Method::OPTIONS)
                .uri(path.parse().unwrap())
                .header("host", "example.com")
                .header("origin", "https://client.example")
                .header("access-control-request-method", method)
                .body(body);
            let response = handle_preflight(resolver, request)
                .await
                .unwrap_or_else(|e| ApiEndpointError::from(e.error).into_response());
            assert_eq!(response.status().as_u16(), status, "{method} {path}");
            if status == 403 {
                assert_eq!(
                    response.headers()[http::header::VARY],
                    "Origin, Access-Control-Request-Method, Access-Control-Request-Headers"
                );
                assert!(
                    !response
                        .headers()
                        .contains_key(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                );
            }
        }
    }

    #[test]
    async fn unavailable_security_keeps_selected_barrier_and_cors_without_polling_dispatch() {
        for (method, kind) in [(None, "router"), (Some("GET"), "typed")] {
            for security in [
                RouteSecurity::Unavailable,
                RouteSecurity::SecurityScheme(SecuritySchemeRouteSecurity {
                    security_scheme_id: golem_common::model::security_scheme::SecuritySchemeId::new(
                    ),
                }),
            ] {
                let mut route = test_route(2, "/private", method, kind);
                route.security = security;
                route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
                let resolver = test_resolver(vec![test_route(1, "/", None, "router"), route]);
                let request = Request::builder()
                    .uri("/private".parse().unwrap())
                    .header("host", "example.com")
                    .header("origin", "https://client.example")
                    .finish();
                let selected = resolver.resolve_matching_route(&request).await.unwrap();
                assert_eq!(selected.route.route_id, 2);
                let result = require_available_security(&selected, async {
                    panic!("Unavailable route must not authenticate or dispatch")
                })
                .await;
                let response =
                    finish_selected_response(result, &RichRequest::new(request), &selected)
                        .unwrap();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(
                    response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                    "https://client.example"
                );
                assert_eq!(response.headers()[http::header::VARY], "Origin");
            }
        }
    }

    #[test]
    async fn selected_errors_and_auth_short_circuits_keep_cors_without_guest_override() {
        let mut route = test_route(1, "/", None, "router");
        route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
            header_name: "x-session".into(),
        });
        route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
        let resolver = test_resolver(vec![route]);
        let request = Request::builder()
            .uri("/".parse().unwrap())
            .header("host", "example.com")
            .header("origin", "https://client.example")
            .finish();
        let selected = resolver.resolve_matching_route(&request).await.unwrap();
        let mut request = RichRequest::new(request);
        let auth = apply_session_from_header_security_middleware(&mut request, &selected)
            .unwrap()
            .unwrap();
        assert_eq!(auth.status, StatusCode::UNAUTHORIZED);
        for (result, status) in [
            (Ok(auth), StatusCode::UNAUTHORIZED),
            (
                Err(RequestHandlerError::ValueParsingFailed {
                    value: "bad".into(),
                    expected: "u64",
                }),
                StatusCode::BAD_REQUEST,
            ),
            (
                Err(anyhow!("storage failed").into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                Ok(RouteExecutionResult {
                    status: StatusCode::FORBIDDEN,
                    headers: HashMap::from([
                        (
                            http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                            "https://evil.example".into(),
                        ),
                        (
                            http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
                            "x-secret".into(),
                        ),
                        (http::header::VARY, "Accept-Encoding".into()),
                    ]),
                    body: ResponseBody::NoBody,
                }),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let failed = result.is_err();
            let result = finish_selected_response(result, &request, &selected);
            assert_eq!(
                result.is_err(),
                failed,
                "Execution errors must retain failure accounting"
            );
            let response = result.unwrap_or_else(|failure| {
                let mut response = ApiEndpointError::from(failure.error).into_response();
                response.headers_mut().extend(failure.cors_headers);
                response
            });
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                "https://client.example"
            );
            assert!(
                !response
                    .headers()
                    .contains_key(http::header::ACCESS_CONTROL_EXPOSE_HEADERS)
            );
            assert!(
                response.headers()[http::header::VARY]
                    .to_str()
                    .unwrap()
                    .contains("Origin")
            );
        }
    }
}
