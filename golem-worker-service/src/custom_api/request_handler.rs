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
use super::cors::{apply_cors_outgoing_middleware, handle_cors_preflight_behaviour};
use super::durable_streams::DurableStreamsHandler;
use super::error::RequestHandlerError;
use super::model::RichRouteBehaviour;
use super::oidc::handler::OidcHandler;
use super::route_resolver::{ResolvedRouteEntry, RouteResolver};
use super::session_from_header_security::apply_session_from_header_security_middleware;
use super::webhooks::WebhookCallbackHandler;
use super::{OidcCallbackBehaviour, ResponseBody, RouteExecutionResult};
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
    durable_streams_handler: Arc<DurableStreamsHandler>,
    oidc_handler: Arc<OidcHandler>,
    webhook_callback_handler: Arc<WebhookCallbackHandler>,
}

#[allow(irrefutable_let_patterns)]
impl RequestHandler {
    pub fn new(
        route_resolver: Arc<RouteResolver>,
        call_agent_handler: Arc<CallAgentHandler>,
        durable_streams_handler: Arc<DurableStreamsHandler>,
        oidc_handler: Arc<OidcHandler>,
        webhook_callback_handler: Arc<WebhookCallbackHandler>,
    ) -> Self {
        Self {
            route_resolver,
            call_agent_handler,
            durable_streams_handler,
            oidc_handler,
            webhook_callback_handler,
        }
    }

    pub async fn handle_request(&self, request: Request) -> Result<Response, RequestHandlerError> {
        debug!("Begin http request handling for request {request:?}");

        let matching_route = self.route_resolver.resolve_matching_route(&request).await?;
        let mut request = RichRequest::new(request);

        let execution_result = self
            .execute_route_and_middlewares(&mut request, &matching_route)
            .instrument(tracing::span!(
                tracing::Level::INFO,
                "handle_route",
                domain = %matching_route.domain,
                method = %matching_route.route.method,
                route = %matching_route.route.path.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("/")
            ))
            .await?;

        let response = route_execution_result_to_response(execution_result)?;

        Ok(response)
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

        let mut result = self.execute_route(request, resolved_route).await?;

        apply_cors_outgoing_middleware(&mut result, request, resolved_route).await?;

        Ok(result)
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

            RichRouteBehaviour::CorsPreflight(cors_preflight) => {
                handle_cors_preflight_behaviour(request, cors_preflight)
            }

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

        ResponseBody::PoemBody { body, content_type } => {
            let response = response_builder.body(body);
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
    use crate::config::RouteResolverConfig;
    use crate::custom_api::api_definition_lookup::{
        ApiDefinitionLookupError, HttpApiDefinitionsLookup,
    };
    use crate::custom_api::oidc::DefaultIdentityProvider;
    use crate::custom_api::oidc::model::{
        McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin, SessionId,
    };
    use crate::custom_api::oidc::session_store::{SessionStore, SessionStoreError};
    use crate::mcp::InvocationHarness;
    use async_trait::async_trait;
    use golem_common::base_model::Empty;
    use golem_common::model::AgentInvocationOutput;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{AgentMode, AgentTypeName, HttpMethod};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::schema::{
        AgentConstructorSchema, InputSchema, OutputSchema, SchemaGraph, SchemaType,
    };
    use golem_service_base::custom_api::{
        AgentRouteMode, CallAgentBehaviour, CompiledInputSchema, CompiledOutputSchema,
        CompiledRoute, CompiledRoutes, CompiledSchema, CorsOptions, OriginPattern, PathSegment,
        RequestBodySchema, RouteBehaviour, RouteSecurity, SessionFromHeaderRouteSecurity,
    };
    use std::time::Duration;
    use test_r::test;

    struct StaticApiDefinitionsLookup;

    #[async_trait]
    impl HttpApiDefinitionsLookup for StaticApiDefinitionsLookup {
        async fn get(
            &self,
            _domain: &golem_common::model::domain_registration::Domain,
        ) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
            Ok(durable_stream_routes())
        }
    }

    struct UnusedSessionStore;

    #[async_trait]
    impl SessionStore for UnusedSessionStore {
        async fn store_pending_oidc_login(
            &self,
            _: &str,
            _: PendingOidcLogin,
        ) -> Result<(), SessionStoreError> {
            unreachable!()
        }

        async fn take_pending_oidc_login(
            &self,
            _: &str,
        ) -> Result<Option<PendingOidcLogin>, SessionStoreError> {
            unreachable!()
        }

        async fn store_authenticated_session(
            &self,
            _: &SessionId,
            _: crate::custom_api::OidcSession,
        ) -> Result<(), SessionStoreError> {
            unreachable!()
        }

        async fn get_authenticated_session(
            &self,
            _: &SessionId,
        ) -> Result<Option<crate::custom_api::OidcSession>, SessionStoreError> {
            unreachable!()
        }

        async fn store_mcp_pending_auth(
            &self,
            _: &str,
            _: McpPendingAuth,
        ) -> Result<(), SessionStoreError> {
            unreachable!()
        }

        async fn take_mcp_pending_auth(
            &self,
            _: &str,
        ) -> Result<Option<McpPendingAuth>, SessionStoreError> {
            unreachable!()
        }

        async fn store_mcp_proxy_code(
            &self,
            _: &str,
            _: McpProxyCodeEntry,
        ) -> Result<(), SessionStoreError> {
            unreachable!()
        }

        async fn take_mcp_proxy_code(
            &self,
            _: &str,
        ) -> Result<Option<McpProxyCodeEntry>, SessionStoreError> {
            unreachable!()
        }
    }

    fn durable_stream_routes() -> CompiledRoutes {
        CompiledRoutes {
            account_id: AccountId::new(),
            account_email: AccountEmail::new("oracle@golem.cloud"),
            environment_id: EnvironmentId::new(),
            deployment_revision: DeploymentRevision::INITIAL,
            security_schemes: HashMap::new(),
            routes: vec![CompiledRoute {
                route_id: 1,
                method: HttpMethod::Post(Empty {}),
                path: vec![PathSegment::Literal {
                    value: "stream".to_string(),
                }],
                body: RequestBodySchema::JsonBody {
                    expected: CompiledSchema {
                        graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                    },
                },
                behavior: RouteBehaviour::CallAgent(CallAgentBehaviour {
                    route_mode: AgentRouteMode::DurableStreams,
                    base_path_variables: 0,
                    component_id: ComponentId::new(),
                    component_revision: ComponentRevision::INITIAL,
                    agent_type: AgentTypeName("oracle-agent".to_string()),
                    agent_mode: AgentMode::Durable,
                    constructor_input: CompiledInputSchema {
                        graph: SchemaGraph::empty(),
                        input_schema: InputSchema::Parameters(vec![]),
                    },
                    constructor_parameters: vec![],
                    phantom: false,
                    method_name: "stream".to_string(),
                    method_input: CompiledInputSchema {
                        graph: SchemaGraph::empty(),
                        input_schema: InputSchema::Parameters(vec![]),
                    },
                    method_parameters: vec![],
                    expected_agent_response: CompiledOutputSchema {
                        graph: SchemaGraph::empty(),
                        output_schema: OutputSchema::Unit,
                    },
                    method_description: None,
                    read_only: None,
                }),
                security: RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                    header_name: "x-golem-session".to_string(),
                }),
                cors: CorsOptions {
                    allowed_patterns: vec![OriginPattern("https://client.example".to_string())],
                },
            }],
        }
    }

    fn request_handler() -> RequestHandler {
        let invocation_harness = InvocationHarness::new(
            AgentInvocationOutput {
                result: golem_common::model::AgentInvocationResult::AgentInitialization,
                consumed_fuel: None,
                invocation_status: None,
                component_revision: None,
                agent_id: None,
                idempotency_key: None,
                oplog_index: None,
                agent_fingerprint: None,
            },
            AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            vec![],
        );
        let worker_service = invocation_harness.worker_service;
        let route_resolver = Arc::new(RouteResolver::new(
            &RouteResolverConfig {
                router_cache_max_capacity: 1,
                router_cache_ttl: Duration::from_secs(60),
                router_cache_eviction_period: Duration::from_secs(60),
            },
            Arc::new(StaticApiDefinitionsLookup),
        ));

        RequestHandler::new(
            route_resolver,
            Arc::new(CallAgentHandler::new(worker_service.clone())),
            Arc::new(DurableStreamsHandler::new(
                worker_service.clone(),
                Arc::new(CallAgentHandler::new(worker_service.clone())),
                &crate::config::DurableStreamsConfig::default(),
            )),
            Arc::new(OidcHandler::new(
                Arc::new(UnusedSessionStore),
                Arc::new(DefaultIdentityProvider),
            )),
            Arc::new(WebhookCallbackHandler::new(worker_service, vec![])),
        )
    }

    fn request(body: &'static str, session: bool, origin: bool) -> Request {
        let mut builder = Request::builder()
            .method(http::Method::POST)
            .uri("http://ds.example/stream".parse().unwrap())
            .header(http::header::HOST, "ds.example")
            .content_type("application/json");
        if session {
            builder = builder.header("x-golem-session", "{}");
        }
        if origin {
            builder = builder.header(http::header::ORIGIN, "https://client.example");
        }
        builder.body(body)
    }

    #[test]
    async fn durable_stream_route_is_guarded_at_the_request_handler_boundary() {
        let handler = request_handler();

        let response = handler
            .handle_request(request("not-json", true, false))
            .await
            .expect("durable-stream dispatch must not parse the REST body");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response.headers().get(http::header::ALLOW),
            Some(&"PUT, HEAD, GET".parse().unwrap())
        );

        let response = handler
            .handle_request(request("not-json", false, false))
            .await
            .expect("missing session header is a response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = handler
            .handle_request(request("not-json", true, true))
            .await
            .expect("valid session reaches durable-stream dispatch");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response
                .headers()
                .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&"https://client.example".parse().unwrap())
        );
        assert_eq!(
            response
                .headers()
                .get(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
            Some(&"true".parse().unwrap())
        );
        assert_eq!(
            response.headers().get(http::header::VARY),
            Some(&"Origin".parse().unwrap())
        );
    }
}
