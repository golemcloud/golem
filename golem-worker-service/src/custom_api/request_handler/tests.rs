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
    AgentRouteMode, CallAgentBehaviour, CompiledInputSchema, CompiledOutputSchema, CompiledRoute,
    CompiledRoutes, CompiledSchema, CorsOptions, OriginPattern, PathSegment, RequestBodySchema,
    RouteBehaviour, RouteSecurity, SessionFromHeaderRouteSecurity,
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
        Some(&"PUT, HEAD, GET, DELETE".parse().unwrap())
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
