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

use super::cache_headers::{CacheRouteKind, add_vary_headers, apply_response_cache_headers};
use super::call_agent::CallAgentHandler;
use super::cors::{
    apply_cors_outgoing_middleware, denied_preflight, handle_selected_preflight, is_cors_preflight,
    requested_preflight_method,
};
use super::durable_streams::DurableStreamsHandler;
use super::error::RequestHandlerError;
use super::model::RichRouteBehaviour;
use super::mounted_dispatch::MountedDispatch;
use super::oidc::handler::OidcHandler;
use super::openapi::OpenApiService;
use super::raw_handler::RawHandler;
use super::route_resolver::{ResolvedRouteEntry, RouteResolver, RouteResolverError};
use super::session_from_header_security::apply_session_from_header_security_middleware;
use super::webhooks::WebhookCallbackHandler;
use super::{OidcCallbackBehaviour, ResponseBody, RichRouteSecurity, RouteExecutionResult};
use crate::custom_api::RichRequest;
use anyhow::anyhow;
use golem_schema::schema::render::json_value::to_json_value_redacted;
use golem_service_base::body_limit::RequestBodyLimit;
use golem_service_base::custom_api::OpenApiSpecFormat;
use golem_service_base::custom_api::{MountFileSource, OpenApiSpecBehaviour};
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use http::StatusCode;
use poem::{Request, Response};
use std::sync::Arc;
use tracing::{Instrument, debug};

pub struct RequestHandler {
    route_resolver: Arc<RouteResolver>,
    call_agent_handler: Arc<CallAgentHandler>,
    durable_streams_handler: Arc<DurableStreamsHandler>,
    oidc_handler: Arc<OidcHandler>,
    webhook_callback_handler: Arc<WebhookCallbackHandler>,
    raw_handler: RawHandler,
    mounted_dispatch: MountedDispatch,
    openapi_service: Arc<OpenApiService>,
    body_limit: RequestBodyLimit,
}

#[derive(Debug)]
pub struct RequestFailure {
    pub error: RequestHandlerError,
    pub response_headers: http::HeaderMap,
}

impl From<RequestHandlerError> for RequestFailure {
    fn from(error: RequestHandlerError) -> Self {
        let mut response_headers = http::HeaderMap::new();
        apply_response_cache_headers(
            CacheRouteKind::Error,
            &mut response_headers,
            false,
            false,
            false,
        );
        Self {
            error,
            response_headers,
        }
    }
}

#[allow(irrefutable_let_patterns)]
impl RequestHandler {
    pub fn new(
        route_resolver: Arc<RouteResolver>,
        call_agent_handler: Arc<CallAgentHandler>,
        durable_streams_handler: Arc<DurableStreamsHandler>,
        oidc_handler: Arc<OidcHandler>,
        webhook_callback_handler: Arc<WebhookCallbackHandler>,
        worker_service: Arc<crate::service::worker::WorkerService>,
        http_session_limits: crate::config::HttpSessionLimits,
        initial_files: Arc<InitialAgentFilesService>,
        openapi_service: Arc<OpenApiService>,
        max_body_size: usize,
    ) -> Self {
        Self {
            route_resolver,
            call_agent_handler,
            durable_streams_handler,
            oidc_handler,
            webhook_callback_handler,
            openapi_service,
            body_limit: RequestBodyLimit::new(max_body_size),
            mounted_dispatch: MountedDispatch::new(
                worker_service.clone(),
                &http_session_limits,
                initial_files,
            ),
            raw_handler: RawHandler::new(worker_service, http_session_limits),
        }
    }

    pub async fn handle_request(&self, request: Request) -> Result<Response, RequestFailure> {
        debug!(method = %request.method(), path = request.uri().path(), "Begin http request handling");

        if request.method() == http::Method::OPTIONS && request.uri().path() == "*" {
            return Ok(Response::builder().status(StatusCode::NO_CONTENT).finish());
        }
        if matches!(
            *request.method(),
            http::Method::CONNECT | http::Method::TRACE
        ) {
            return Err(RequestHandlerError::RawRequest(StatusCode::NOT_IMPLEMENTED).into());
        }
        if request
            .headers()
            .get_all(http::header::EXPECT)
            .iter()
            .any(|value| !value.as_bytes().eq_ignore_ascii_case(b"100-continue"))
        {
            return Err(RequestHandlerError::RawRequest(StatusCode::EXPECTATION_FAILED).into());
        }
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
        let body_limit = self.body_limit.apply(&mut request.underlying);
        if let Some(response) = body_limit.rejection() {
            return finish_selected_response(Ok(response), &request, &matching_route);
        }

        if matches!(matching_route.route.behavior, RichRouteBehaviour::Mount(_))
            && request
                .underlying
                .headers()
                .contains_key(http::header::UPGRADE)
        {
            return finish_selected_response(
                Err(RequestHandlerError::RawRequest(StatusCode::NOT_IMPLEMENTED)),
                &request,
                &matching_route,
            );
        }
        let execute = require_available_security(&matching_route,
                self.execute_route_and_middlewares(&mut request, &matching_route))
                .instrument(tracing::span!(
                    tracing::Level::INFO, "handle_route",
                    domain = %matching_route.domain, method = %request_method,
                    route = %matching_route.route.path.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("/")
                ));
        let execution_result = execute.await.and_then(route_execution_result_to_response);
        let execution_result = body_limit.rejection().map(Ok).unwrap_or(execution_result);
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
            RichRouteBehaviour::CallAgent(behaviour)
                if behaviour.route_mode
                    == golem_service_base::custom_api::AgentRouteMode::DurableStreams =>
            {
                self.durable_streams_handler
                    .handle(request, resolved_route, behaviour)
                    .await
                    .or_else(super::durable_streams::error_response)
            }
            RichRouteBehaviour::CallAgent(behaviour) => {
                self.call_agent_handler
                    .handle_call_agent_behaviour(request, resolved_route, behaviour)
                    .await
            }
            RichRouteBehaviour::CreateAgent(behaviour) => {
                self.mounted_dispatch
                    .create(request, resolved_route, behaviour)
                    .await
            }

            RichRouteBehaviour::CorsPreflight(_) => Err(RequestHandlerError::invariant_violated(
                "OpenAPI preflight projection selected for dispatch",
            )),

            RichRouteBehaviour::OidcCallback(OidcCallbackBehaviour { security_scheme }) => {
                self.oidc_handler
                    .handle_oidc_callback_behaviour(request, resolved_route, security_scheme)
                    .await
            }

            RichRouteBehaviour::OidcPkceAuthorize(behaviour) => {
                self.oidc_handler
                    .handle_pkce_authorization(request, resolved_route, &behaviour.security_scheme)
                    .await
            }

            RichRouteBehaviour::OidcPkceToken(behaviour) => {
                self.oidc_handler
                    .handle_pkce_token(request, resolved_route, &behaviour.security_scheme)
                    .await
            }

            RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { format, .. }) => {
                let inputs = resolved_route.openapi_inputs.clone().ok_or_else(|| {
                    RequestHandlerError::invariant_violated("Missing OpenAPI inputs")
                })?;
                let spec = self.openapi_service.generate(inputs).await?;

                Ok(RouteExecutionResult {
                    status: StatusCode::OK,
                    headers: http::HeaderMap::new(),
                    body: ResponseBody::OpenApiSchema {
                        spec,
                        format: *format,
                    },
                })
            }

            RichRouteBehaviour::WebhookCallback(_) => {
                self.webhook_callback_handler
                    .handle_webhook_callback_behaviour(request, resolved_route)
                    .await
            }
            RichRouteBehaviour::Mount(_) => {
                self.mounted_dispatch
                    .dispatch(&self.raw_handler, request, resolved_route)
                    .await
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
            headers: http::HeaderMap::new(),
            body: ResponseBody::NoBody,
        })
    } else {
        execute.await
    }
}

fn finish_selected_response(
    result: Result<Response, RequestHandlerError>,
    request: &RichRequest,
    selected: &ResolvedRouteEntry,
) -> Result<Response, RequestFailure> {
    let live_files = matches!(
        &selected.route.behavior,
        RichRouteBehaviour::Mount(mount) if matches!(mount.files, MountFileSource::Live { .. })
    );
    let (mut response, mut error) = match result {
        Ok(response) => (response, None),
        Err(error) => (Response::default(), Some(error)),
    };
    if live_files {
        response.headers_mut().insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("no-store"),
        );
    }
    if let Err(cors_error) = apply_cors_outgoing_middleware(&mut response, request, selected) {
        response = Response::default();
        // Finalization can fail after an OAuth producer issued a credential.
        // Discard its body and headers without weakening its privacy floor.
        response.headers_mut().insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("private, no-store"),
        );
        error = Some(cors_error);
    }
    response.headers_mut().remove("golem-invocation-id");
    response.headers_mut().remove("golem-environment-id");
    if let Some(accepted) = &request.accepted_invocation {
        response.headers_mut().insert(
            "golem-invocation-id",
            accepted
                .id
                .to_string()
                .parse()
                .expect("invocation IDs are ASCII"),
        );
        response.headers_mut().insert(
            "golem-environment-id",
            accepted
                .environment_id
                .to_string()
                .parse()
                .expect("environment IDs are ASCII"),
        );
    }
    let kind = if error.is_some()
        || response.status().is_client_error()
        || response.status().is_server_error()
    {
        CacheRouteKind::Error
    } else {
        match &selected.route.behavior {
            RichRouteBehaviour::CallAgent(_) => CacheRouteKind::Typed(None),
            RichRouteBehaviour::Mount(mount) if mount.handler.is_some() => {
                CacheRouteKind::RawHandler
            }
            RichRouteBehaviour::Mount(_) => CacheRouteKind::File,
            RichRouteBehaviour::OpenApiSpec(_) => CacheRouteKind::OpenApi,
            _ => CacheRouteKind::Protocol,
        }
    };
    apply_response_cache_headers(
        kind,
        response.headers_mut(),
        request.authenticated_session().is_some(),
        false,
        false,
    );
    if let RichRouteSecurity::SessionFromHeader(security) = &selected.route.security {
        add_vary_headers(response.headers_mut(), &[&security.header_name]);
    }
    match error {
        Some(error) => Err(RequestFailure {
            error,
            response_headers: response.headers().clone(),
        }),
        None => Ok(response),
    }
}

fn route_execution_result_to_response(
    result: RouteExecutionResult,
) -> Result<Response, RequestHandlerError> {
    let mut response = Response::builder().status(result.status).finish();
    *response.headers_mut() = result.headers;

    match result.body {
        ResponseBody::NoBody => Ok(response),

        ResponseBody::PoemBody { body, content_type } => {
            response.set_body(body);
            Ok(match content_type {
                Some(content_type) => response.set_content_type(content_type),
                None => response,
            })
        }

        ResponseBody::ComponentModelJsonBody { body } => {
            let body = poem::Body::from_json(
                to_json_value_redacted(body.graph(), body.root_type(), body.value())
                    .map_err(|e| anyhow!("ComponentModelJsonBody conversion error: {e}"))?,
            )
            .map_err(anyhow::Error::from)?;

            response.set_body(body);
            Ok(response.set_content_type("application/json"))
        }

        ResponseBody::UnstructuredBinaryBody { body } => {
            response.set_body(body.data);
            Ok(response.set_content_type(body.binary_type.mime_type))
        }

        ResponseBody::UnstructuredTextBody { body } => {
            response = response.set_content_type("text/plain; charset=utf-8");
            if let Some(text_type) = &body.text_type {
                let trimmed = text_type.language_code.trim();
                if !trimmed.is_empty()
                    && !trimmed.contains(',')
                    && trimmed
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    response.headers_mut().insert(
                        http::header::CONTENT_LANGUAGE,
                        http::HeaderValue::from_str(trimmed).unwrap(),
                    );
                }
            }
            response.set_body(body.data);
            Ok(response)
        }

        ResponseBody::OpenApiSchema { spec: body, format } => {
            let response = match format {
                OpenApiSpecFormat::Json => {
                    response.set_body(body.json.clone());
                    response.set_content_type("application/json")
                }
                OpenApiSpecFormat::Yaml => {
                    response.set_body(body.yaml.clone());
                    response.set_content_type("application/yaml")
                }
            };

            Ok(response)
        }
        ResponseBody::Stream(body) => {
            response.set_body(body);
            Ok(response)
        }
    }
}

#[cfg(test)]
mod mounted_tests {
    use super::*;
    use crate::api::common::ApiEndpointError;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_service_base::custom_api::{
        OriginPattern, RouteSecurity, SecuritySchemeRouteSecurity, SessionFromHeaderRouteSecurity,
    };
    use poem::IntoResponse;
    use test_r::test;

    #[test]
    async fn response_boundary_preserves_duplicate_and_raw_headers() {
        let mut headers = http::HeaderMap::new();
        headers.append(
            http::header::SET_COOKIE,
            http::HeaderValue::from_static("first=1"),
        );
        headers.append(
            http::header::SET_COOKIE,
            http::HeaderValue::from_static("second=2"),
        );
        headers.insert(
            http::HeaderName::from_static("x-raw"),
            http::HeaderValue::from_bytes(&[0x80, 0xff]).unwrap(),
        );

        let response = route_execution_result_to_response(RouteExecutionResult {
            status: StatusCode::OK,
            headers,
            body: ResponseBody::NoBody,
        })
        .unwrap();

        assert_eq!(
            response
                .headers()
                .get_all(http::header::SET_COOKIE)
                .iter()
                .map(|value| value.as_bytes())
                .collect::<Vec<_>>(),
            vec![b"first=1".as_slice(), b"second=2".as_slice()]
        );
        assert_eq!(response.headers()["x-raw"].as_bytes(), &[0x80, 0xff]);
    }

    #[test]
    async fn stream_response_is_not_collected_at_the_boundary() {
        use futures::{StreamExt, stream};
        use std::sync::atomic::{AtomicBool, Ordering};

        let polled = Arc::new(AtomicBool::new(false));
        let stream_polled = polled.clone();
        let body = poem::Body::from_bytes_stream(stream::poll_fn(move |_| {
            stream_polled.store(true, Ordering::SeqCst);
            std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(bytes::Bytes::from_static(
                b"first",
            ))))
        }));
        let response = route_execution_result_to_response(RouteExecutionResult {
            status: StatusCode::OK,
            headers: http::HeaderMap::new(),
            body: ResponseBody::Stream(body),
        })
        .unwrap();

        assert!(!polled.load(Ordering::SeqCst));
        let mut stream = response.into_body().into_bytes_stream();
        assert_eq!(stream.next().await.unwrap().unwrap(), "first");
        assert!(polled.load(Ordering::SeqCst));
    }

    #[test]
    async fn shared_auth_and_unsafe_path_corpus_through_request_handler() {
        use crate::custom_api::oidc::DefaultIdentityProvider;
        use crate::mcp::InvocationHarness;
        use golem_common::model::{AgentInvocationOutput, AgentInvocationResult};
        use golem_common::schema::{AgentConstructorSchema, InputSchema, SchemaValue};
        use golem_service_base::custom_api::{RouteBehaviour, RouterFileIndexEntry};
        let (session_store, _db) =
            crate::custom_api::oidc::session_store::sqlite_test_session_store().await;
        let oidc = Arc::new(OidcHandler::new(
            Arc::new(session_store),
            Arc::new(DefaultIdentityProvider),
        ));
        let harness = InvocationHarness::new(
            AgentInvocationOutput {
                result: AgentInvocationResult::AgentMethod {
                    output: SchemaValue::Tuple { elements: vec![] },
                },
                consumed_fuel: None,
                invocation_status: None,
                component_revision: None,
                agent_id: None,
                idempotency_key: None,
                oplog_index: None,
                agent_fingerprint: None,
                background_failure: None,
            },
            AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            vec![],
        );
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        for id in ["route-auth-before-file", "route-unsafe-before-auth"] {
            let case = corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .unwrap();
            let input = &case["input"];
            let mount = &input["mounts"][0];
            let mut route = test_route(1, mount["path"].as_str().unwrap(), None, "router");
            route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                header_name: "x-session".into(),
            });
            let RouteBehaviour::Mount(router) = &mut route.behavior else {
                unreachable!()
            };
            router.file_bindings = golem_schema::http::FileMapping::compile_list(
                mount["mappings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| (m[0].as_str().unwrap(), m[1].as_str().unwrap())),
            )
            .unwrap();
            if let Some(files) = input["files"].as_object() {
                let MountFileSource::Router { file_index, .. } = &mut router.files else {
                    unreachable!()
                };
                for (path, state) in files {
                    assert_eq!(state, "found", "{id}");
                    file_index.push(RouterFileIndexEntry {
                        path: path.clone(),
                        blob_key: golem_common::model::agent::AgentFileContentHash(
                            golem_common::model::diff::Hash::empty(),
                        ),
                        size: 3,
                    });
                }
            }
            let handler = RequestHandler::new(
                Arc::new(test_resolver(vec![route])),
                Arc::new(CallAgentHandler::new(harness.worker_service.clone())),
                Arc::new(DurableStreamsHandler::new(
                    harness.worker_service.clone(),
                    Arc::new(CallAgentHandler::new(harness.worker_service.clone())),
                    &crate::config::DurableStreamsConfig::default(),
                )),
                oidc.clone(),
                Arc::new(WebhookCallbackHandler::new(
                    harness.worker_service.clone(),
                    vec![],
                )),
                harness.worker_service.clone(),
                Default::default(),
                Arc::new(InitialAgentFilesService::new(Arc::new(
                    golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
                ))),
                Arc::new(OpenApiService::new(harness.worker_service.clone())),
                crate::config::RequestBodyLimits::default().http_api,
            );
            let request = Request::builder()
                .uri(input["target"].as_str().unwrap().parse().unwrap())
                .method(input["method"].as_str().unwrap().parse().unwrap())
                .header("host", "example.com")
                .finish();
            let response = handler
                .handle_request(request)
                .await
                .unwrap_or_else(|e| ApiEndpointError::from(e.error).into_response());
            assert_eq!(
                u64::from(response.status().as_u16()),
                case["expect"]["status"].as_u64().unwrap(),
                "{id}"
            );
        }
    }

    struct UnknownSiteLookup;

    #[async_trait::async_trait]
    #[golem_async_trait::region_simple]
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
                let response = finish_selected_response(
                    result.and_then(route_execution_result_to_response),
                    &RichRequest::new(request),
                    &selected,
                )
                .unwrap();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(
                    response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                    "https://client.example"
                );
                assert_eq!(
                    response.headers()[http::header::VARY],
                    "Origin, Authorization, Cookie, Golem-Token"
                );
                assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-cache");
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
                    headers: {
                        let mut headers = http::HeaderMap::from_iter([
                            (
                                http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                                http::HeaderValue::from_static("https://evil.example"),
                            ),
                            (
                                http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
                                http::HeaderValue::from_static("x-secret"),
                            ),
                            (
                                http::header::VARY,
                                http::HeaderValue::from_static("Accept-Encoding"),
                            ),
                        ]);
                        headers.append(
                            http::header::SET_COOKIE,
                            http::HeaderValue::from_static("first=1"),
                        );
                        headers.append(
                            http::header::SET_COOKIE,
                            http::HeaderValue::from_static("second=2"),
                        );
                        headers.insert(
                            http::HeaderName::from_static("x-raw"),
                            http::HeaderValue::from_bytes(&[0x80, 0xff]).unwrap(),
                        );
                        headers
                    },
                    body: ResponseBody::NoBody,
                }),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let failed = result.is_err();
            let result = finish_selected_response(
                result.and_then(route_execution_result_to_response),
                &request,
                &selected,
            );
            assert_eq!(
                result.is_err(),
                failed,
                "Execution errors must retain failure accounting"
            );
            let response = result.unwrap_or_else(|failure| {
                let mut response = ApiEndpointError::from(failure.error).into_response();
                response.headers_mut().extend(failure.response_headers);
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
            if status == StatusCode::FORBIDDEN {
                assert_eq!(
                    response
                        .headers()
                        .get_all(http::header::SET_COOKIE)
                        .iter()
                        .map(|value| value.as_bytes())
                        .collect::<Vec<_>>(),
                    vec![b"first=1".as_slice(), b"second=2".as_slice()]
                );
                assert_eq!(response.headers()["x-raw"].as_bytes(), &[0x80, 0xff]);
            }
        }
    }

    #[test]
    async fn selected_cache_policy_covers_authenticated_successes_errors_and_stream_heads() {
        for kind in ["typed", "router", "filesystem"] {
            for authenticated in [false, true] {
                let mut route = test_route(
                    1,
                    "/",
                    if kind == "typed" { Some("GET") } else { None },
                    kind,
                );
                route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                    header_name: "x-session".into(),
                });
                let resolver = test_resolver(vec![route]);
                let request = Request::builder()
                    .uri("/".parse().unwrap())
                    .header("host", "example.com")
                    .header("x-session", "{}")
                    .finish();
                let selected = resolver.resolve_matching_route(&request).await.unwrap();
                let mut request = RichRequest::new(request);
                if authenticated {
                    assert!(
                        apply_session_from_header_security_middleware(&mut request, &selected)
                            .unwrap()
                            .is_none()
                    );
                }
                for status in [
                    StatusCode::OK,
                    StatusCode::NOT_MODIFIED,
                    StatusCode::BAD_REQUEST,
                ] {
                    let mut response =
                        Response::builder()
                            .status(status)
                            .body(poem::Body::from_bytes_stream(futures::stream::poll_fn(
                                |_| -> std::task::Poll<
                                    Option<Result<bytes::Bytes, std::io::Error>>,
                                > {
                                    panic!("Cache finalization must not poll streaming bodies")
                                },
                            )));
                    response.headers_mut().insert(
                        http::header::CACHE_CONTROL,
                        "public, max-age=60".parse().unwrap(),
                    );
                    response
                        .headers_mut()
                        .insert(http::header::VARY, "Accept-Language".parse().unwrap());
                    let response =
                        finish_selected_response(Ok(response), &request, &selected).unwrap();
                    let expected = match (kind, authenticated) {
                        ("filesystem", true) => "private, no-store",
                        ("filesystem", false) => "no-store",
                        (_, true) => "private, no-cache",
                        (_, false) => "public, max-age=60",
                    };
                    assert_eq!(
                        response.headers()[http::header::CACHE_CONTROL],
                        expected,
                        "{kind} {status}"
                    );
                    if !authenticated {
                        assert_eq!(
                            response.headers()[http::header::VARY],
                            "Accept-Language, Authorization, Cookie, Golem-Token, x-session"
                        );
                    }
                }
                let failure = finish_selected_response(
                    Err(anyhow!("storage failed").into()),
                    &request,
                    &selected,
                )
                .unwrap_err();
                assert_eq!(
                    failure.response_headers[http::header::CACHE_CONTROL],
                    match (kind, authenticated) {
                        ("filesystem", true) => "private, no-store",
                        ("filesystem", false) => "no-store",
                        (_, true) => "private, no-cache",
                        (_, false) => "no-cache",
                    }
                );
            }
        }
    }

    #[test]
    async fn admission_rejection_before_lazy_stream_creation_has_no_invocation_headers() {
        use crate::config::DurableStreamsConfig;
        use golem_common::model::agent::AgentMode;
        use golem_service_base::custom_api::{
            AgentRouteMode, DurableStreamRepresentation, DurableStreamRoutePolicy,
            DurableStreamSlot, DurableStreamSlotDirection, RouteBehaviour,
        };

        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let mut config = DurableStreamsConfig::default();
        config.load.max_append_requests_per_second_per_stream = 0;
        let handler = DurableStreamsHandler::new(
            harness.worker_service.clone(),
            Arc::new(CallAgentHandler::new(harness.worker_service.clone())),
            &config,
        );
        let mut route = test_route(
            1,
            "/run/invocations/{session}/streams/{slot}",
            Some("POST"),
            "typed",
        );
        let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
            unreachable!()
        };
        call.component_id = harness.component_id;
        call.agent_type = harness.agent_type.type_name.clone();
        call.method_parameters.clear();
        call.route_mode = AgentRouteMode::DurableStreams;
        call.durable_streams = Some(DurableStreamRoutePolicy {
            family_id: 1,
            slots: vec![DurableStreamSlot {
                canonical_name: "events".into(),
                public_name: "events".into(),
                direction: DurableStreamSlotDirection::Input,
                content_type: "application/json".into(),
                representation: DurableStreamRepresentation::Json,
            }],
            allow_external_writes: true,
            allow_stream_delete: false,
            allow_invocation_delete: false,
            load: None,
        });
        let resolver = test_resolver(vec![route]);
        let mut request = RichRequest::new(
            Request::builder()
                .method(http::Method::POST)
                .uri(
                    "/run/invocations/new-session/streams/events"
                        .parse()
                        .unwrap(),
                )
                .header("host", "example.com")
                .finish(),
        );
        let selected = resolver
            .resolve_matching_route(&request.underlying)
            .await
            .unwrap();
        let RichRouteBehaviour::CallAgent(call) = &selected.route.behavior else {
            unreachable!()
        };
        let result = handler.handle(&mut request, &selected, call).await;
        assert!(request.accepted_invocation.is_none());
        assert!(harness.contexts.lock().unwrap().is_empty());
        let response = finish_selected_response(
            result.and_then(route_execution_result_to_response),
            &request,
            &selected,
        )
        .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[http::header::RETRY_AFTER], "1");
        for name in ["Golem-Invocation-Id", "Golem-Environment-Id"] {
            assert!(!response.headers().contains_key(name));
        }
    }

    #[test]
    async fn invocation_headers_require_acceptance_and_override_guest_values() {
        use crate::service::worker::AcceptedInvocation;
        use golem_common::model::environment::EnvironmentId;
        use golem_common::model::{AgentId, InvocationId, OwnedInvocationId};
        use golem_common::schema::{OutputSchema, SchemaGraph, SchemaType};
        use golem_service_base::custom_api::{
            CompiledOutputSchema, MountAgentMethod, MountTarget, RouteBehaviour, RouterMethod,
        };

        let receipt = AcceptedInvocation {
            id: OwnedInvocationId::new(
                AgentId::from_parts(7, 19),
                InvocationId(uuid::Uuid::from_u128(91)),
            ),
            environment_id: EnvironmentId::new(),
        };
        for kind in ["typed", "router", "filesystem"] {
            let mut route = test_route(1, "/", (kind == "typed").then_some("GET"), kind);
            route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
            if kind == "router" {
                let RouteBehaviour::Mount(mount) = &mut route.behavior else {
                    unreachable!()
                };
                mount.handler = Some(MountTarget::AgentMethod(MountAgentMethod {
                    agent: mount.files.agent().clone(),
                    method: RouterMethod {
                        method_name: "handle".into(),
                        input: mount.files.agent().constructor_input.clone(),
                        output: CompiledOutputSchema {
                            graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                            output_schema: OutputSchema::Unit,
                        },
                    },
                }));
            }
            let resolver = test_resolver(vec![route]);
            for accepted in [false, true] {
                if accepted && kind == "filesystem" {
                    continue;
                }
                for outcome in 0..5 {
                    let request = Request::builder()
                        .uri("/".parse().unwrap())
                        .header("host", "example.com")
                        .header("origin", "https://client.example")
                        .finish();
                    let selected = resolver.resolve_matching_route(&request).await.unwrap();
                    let mut request = RichRequest::new(request);
                    request.accepted_invocation = accepted.then(|| receipt.clone());
                    if outcome == 4 {
                        request.underlying.headers_mut().insert(
                            http::header::ORIGIN,
                            http::HeaderValue::from_bytes(&[0xff]).unwrap(),
                        );
                    }
                    let mut response = Response::builder()
                        .status(match outcome {
                            1 => StatusCode::BAD_REQUEST,
                            2 => StatusCode::NOT_MODIFIED,
                            _ => StatusCode::OK,
                        })
                        .finish();
                    for name in ["golem-invocation-id", "golem-environment-id"] {
                        response
                            .headers_mut()
                            .append(name, http::HeaderValue::from_static("spoofed"));
                        response
                            .headers_mut()
                            .append(name, http::HeaderValue::from_static("also-spoofed"));
                    }
                    let result = if outcome == 3 {
                        Err(RequestHandlerError::MissingValue { expected: "result" })
                    } else {
                        Ok(response)
                    };
                    let headers = match finish_selected_response(result, &request, &selected) {
                        Ok(response) => response.headers().clone(),
                        Err(failure) => failure.response_headers,
                    };
                    for (name, expected) in [
                        ("golem-invocation-id", receipt.id.to_string()),
                        ("golem-environment-id", receipt.environment_id.to_string()),
                    ] {
                        let values: Vec<_> = headers.get_all(name).iter().collect();
                        if accepted {
                            assert_eq!(
                                values,
                                vec![&http::HeaderValue::from_str(&expected).unwrap()]
                            );
                        } else {
                            assert!(values.is_empty());
                        }
                    }
                    if outcome != 4 && kind != "filesystem" {
                        let exposed = headers[http::header::ACCESS_CONTROL_EXPOSE_HEADERS]
                            .to_str()
                            .unwrap();
                        for name in ["Golem-Invocation-Id", "Golem-Environment-Id"] {
                            assert!(exposed.split(", ").any(|value| value == name));
                        }
                    }
                }
            }
        }
    }

    #[test]
    async fn cors_finalization_failures_remain_private_without_original_response_headers() {
        for kind in ["router", "filesystem"] {
            let mut route = test_route(1, "/", None, kind);
            route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
            let resolver = test_resolver(vec![route]);
            let request = Request::builder()
                .uri("/".parse().unwrap())
                .header("host", "example.com")
                .finish();
            let selected = resolver.resolve_matching_route(&request).await.unwrap();
            let mut request = RichRequest::new(request);
            request.underlying.headers_mut().insert(
                http::header::ORIGIN,
                http::HeaderValue::from_bytes(&[0xff]).unwrap(),
            );
            let response = Response::builder()
                .header("x-application", "discard")
                .header("cache-control", "private, no-store")
                .finish();
            let failure = finish_selected_response(Ok(response), &request, &selected).unwrap_err();
            assert_eq!(
                failure.response_headers[http::header::CACHE_CONTROL],
                "private, no-store"
            );
            assert!(!failure.response_headers.contains_key("x-application"));
        }
    }
}

#[cfg(test)]
mod tests;
