use super::*;
use crate::config::RouteResolverConfig;
use crate::custom_api::api_definition_lookup::{
    ApiDefinitionLookupError, HttpApiDefinitionsLookup,
};
use crate::custom_api::oidc::DefaultIdentityProvider;
use crate::custom_api::oidc::model::{
    AuthorizationCode, BearerToken, McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin,
    PendingPkceLogin, PkceAuthorizationCode, PkceBearerCredential, PkceBinding, SessionId,
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
    CompiledRoutes, CompiledSchema, CorsOptions, DurableStreamRepresentation,
    DurableStreamRoutePolicy, DurableStreamSlot, DurableStreamSlotDirection, OriginPattern,
    PathSegment, RequestBodySchema, RouteBehaviour, RouteSecurity, SessionFromHeaderRouteSecurity,
};
use std::collections::HashMap;
use std::time::Duration;
use test_r::test;

#[path = "live_files_tests.rs"]
mod live_files;

#[test]
async fn unavailable_security_fails_before_route_dispatch() {
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::security_scheme::SecuritySchemeId;
    use golem_service_base::custom_api::SecuritySchemeRouteSecurity;

    let mut route = test_route(1, "/protected", Some("GET"), "typed");
    route.security = RouteSecurity::SecurityScheme(SecuritySchemeRouteSecurity {
        security_scheme_id: SecuritySchemeId::new(),
    });
    let resolver = test_resolver(vec![route]);
    let request = poem::Request::builder()
        .uri("/protected".parse().unwrap())
        .header("host", "example.com")
        .finish();
    let selected = resolver.resolve_matching_route(&request).await.unwrap();
    let result = require_available_security(&selected, async {
        panic!("unavailable security must not dispatch the route")
    })
    .await
    .unwrap();
    assert_eq!(result.status, StatusCode::SERVICE_UNAVAILABLE);
}

struct StaticApiDefinitionsLookup;

#[test]
async fn phantom_selector_runs_after_auth_and_before_etag_revalidation() {
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::{CachePolicy, OwnerKind, ReadOnlyConfig};
    use golem_common::model::worker::AgentMetadataDto;
    use golem_common::model::{AgentFingerprint, AgentId, AgentStatus, OplogIndex, Timestamp};
    use golem_service_base::custom_api::PhantomSelection;
    use poem::IntoResponse;
    let p = "550e8400-e29b-41d4-a716-446655440000";
    let q = "8badf00d-1234-4567-89ab-0123456789ab";
    let harness = InvocationHarness::new(
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
        vec![golem_common::schema::AgentMethodSchema {
            name: "run".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        }],
    );
    let fingerprint = AgentFingerprint(uuid::Uuid::new_v4());
    let mut route = test_route(1, "/selected", Some("GET"), "typed");
    route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
        header_name: "x-golem-session".into(),
    });
    let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
        unreachable!()
    };
    call.component_id = harness.component_id;
    call.agent_type = AgentTypeName("mcp-agent".into());
    call.phantom_selection = PhantomSelection::Query {
        name: "instance".into(),
        optional: true,
    };
    call.read_only = Some(ReadOnlyConfig {
        cache_policy: CachePolicy::UntilWrite(Empty {}),
        uses_principal: false,
    });
    let bound_call = call.clone();
    let mut ids = vec![];
    for phantom in [None, Some(p), Some(q)] {
        let id = AgentId {
            component_id: harness.component_id,
            agent_id: phantom
                .map(|p| format!("mcp-agent()[{p}]"))
                .unwrap_or_else(|| "mcp-agent()".into()),
        };
        harness.metadata.lock().unwrap().insert(
            id.clone(),
            AgentMetadataDto {
                agent_id: id.clone(),
                owner_kind: OwnerKind::ComponentAgent,
                environment_id: harness.environment_id,
                created_by: harness.account_id,
                env: Default::default(),
                config: vec![],
                status: AgentStatus::Idle,
                component_revision: ComponentRevision::INITIAL,
                retry_count: 0,
                pending_invocation_count: 0,
                updates: vec![],
                created_at: Timestamp::now_utc(),
                last_error: None,
                last_error_kind: None,
                component_size: 0,
                total_linear_memory_size: 0,
                exported_resource_instances: vec![],
                active_plugins: Default::default(),
                skipped_regions: vec![],
                deleted_regions: vec![],
                last_oplog_index: OplogIndex::from_u64(17),
                fingerprint,
            },
        );
        ids.push(id);
    }
    let mut routes = vec![route];
    for (id, path, selection) in [
        (
            2,
            "/selected-path/{instance}",
            PhantomSelection::Path { index: 0.into() },
        ),
        (3, "/original", PhantomSelection::Original),
        (
            4,
            "/required",
            PhantomSelection::Query {
                name: "instance".into(),
                optional: false,
            },
        ),
    ] {
        let mut route = test_route(id, path, Some("GET"), "typed");
        let mut call = bound_call.clone();
        call.phantom_selection = selection;
        route.behavior = RouteBehaviour::CallAgent(call);
        route.security = routes[0].security.clone();
        routes.push(route);
    }
    let handler = request_handler_with_worker(
        test_resolver(routes),
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
        harness.worker_service.clone(),
    );
    let etag = |id: &AgentId| {
        format!(
            "\"{}/{}/{fingerprint}:17\"",
            id.component_id,
            id.agent_name_encoded()
        )
    };
    for (query, authorized, expected) in [
        ("instance=invalid".into(), false, StatusCode::UNAUTHORIZED),
        ("instance=invalid".into(), true, StatusCode::BAD_REQUEST),
        ("instance=".into(), true, StatusCode::BAD_REQUEST),
        (
            format!("instance={p}&instance={p}"),
            true,
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("instance={p}&instance={q}"),
            true,
            StatusCode::BAD_REQUEST,
        ),
        (format!("instance={p}"), true, StatusCode::NOT_MODIFIED),
        (
            format!("instance={}", p.to_uppercase()),
            true,
            StatusCode::NOT_MODIFIED,
        ),
        ("".into(), true, StatusCode::NOT_MODIFIED),
    ] {
        harness.metadata_reads.lock().unwrap().clear();
        let mut request = Request::builder()
            .method(http::Method::GET)
            .uri(format!("/selected?{query}").parse().unwrap())
            .header("host", "example.com")
            .header(
                http::header::IF_NONE_MATCH,
                etag(&ids[usize::from(!query.is_empty())]),
            );
        if authorized {
            request = request.header("x-golem-session", "{}");
        }
        let response = match handler.handle_request(request.finish()).await {
            Ok(response) => response,
            Err(failure) => {
                crate::api::common::ApiEndpointError::from(failure.error).into_response()
            }
        };
        assert_eq!(response.status(), expected, "{query}");
        assert!(harness.contexts.lock().unwrap().is_empty());
        let reads = harness.metadata_reads.lock().unwrap();
        if expected == StatusCode::NOT_MODIFIED {
            assert_eq!(*reads, vec![ids[usize::from(!query.is_empty())].clone()]);
        } else {
            assert!(reads.is_empty());
        }
    }
    for (uri, selected, expected) in [
        (format!("/selected-path/{p}"), 1, StatusCode::NOT_MODIFIED),
        ("/selected-path/invalid".into(), 1, StatusCode::BAD_REQUEST),
        ("/selected-path".into(), 1, StatusCode::NOT_FOUND),
        ("/original".into(), 0, StatusCode::NOT_MODIFIED),
        ("/required".into(), 1, StatusCode::BAD_REQUEST),
    ] {
        harness.metadata_reads.lock().unwrap().clear();
        let request = Request::builder()
            .method(http::Method::GET)
            .uri(uri.parse().unwrap())
            .header("host", "example.com")
            .header("x-golem-session", "{}")
            .header(http::header::IF_NONE_MATCH, etag(&ids[selected]))
            .finish();
        let response = match handler.handle_request(request).await {
            Ok(response) => response,
            Err(failure) => {
                crate::api::common::ApiEndpointError::from(failure.error).into_response()
            }
        };
        assert_eq!(response.status(), expected, "{uri}");
        assert!(harness.contexts.lock().unwrap().is_empty());
        let reads = harness.metadata_reads.lock().unwrap();
        if expected == StatusCode::NOT_MODIFIED {
            assert_eq!(*reads, vec![ids[selected].clone()]);
        } else {
            assert!(reads.is_empty());
        }
    }
    for (index, source) in [&ids[0], &ids[1]].into_iter().enumerate() {
        harness.metadata_reads.lock().unwrap().clear();
        let request = Request::builder()
            .method(http::Method::GET)
            .uri(format!("/selected?instance={q}").parse().unwrap())
            .header("host", "example.com")
            .header("x-golem-session", "{}")
            .header(http::header::IF_NONE_MATCH, etag(source))
            .finish();
        let response = handler.handle_request(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(harness.contexts.lock().unwrap().len(), index + 1);
        assert_eq!(harness.recorded_agent_id(), ids[2]);
        assert!(
            harness
                .metadata_reads
                .lock()
                .unwrap()
                .iter()
                .all(|id| id == &ids[2])
        );
    }
}

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

    async fn store_pending_pkce_login(
        &self,
        _: &str,
        _: PendingPkceLogin,
    ) -> Result<(), SessionStoreError> {
        unreachable!()
    }

    async fn take_pending_pkce_login(
        &self,
        _: &str,
    ) -> Result<Option<PendingPkceLogin>, SessionStoreError> {
        unreachable!()
    }

    async fn store_authorization_code(
        &self,
        _: &AuthorizationCode,
        _: PkceAuthorizationCode,
    ) -> Result<(), SessionStoreError> {
        unreachable!()
    }

    async fn take_authorization_code(
        &self,
        _: &str,
    ) -> Result<Option<PkceAuthorizationCode>, SessionStoreError> {
        unreachable!()
    }

    async fn store_bearer_credential(
        &self,
        _: &BearerToken,
        _: PkceBearerCredential,
    ) -> Result<(), SessionStoreError> {
        unreachable!()
    }

    async fn get_bearer_credential(
        &self,
        _: &str,
        _: &PkceBinding,
    ) -> Result<Option<PkceBearerCredential>, SessionStoreError> {
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
    let behaviour = CallAgentBehaviour {
        route_mode: AgentRouteMode::DurableStreams,
        base_path_variables: 0,
        durable_streams: Some(DurableStreamRoutePolicy {
            slots: vec![
                DurableStreamSlot {
                    canonical_name: "$result".into(),
                    public_name: "responses".into(),
                    direction: DurableStreamSlotDirection::Output,
                    content_type: "application/json".into(),
                    representation: DurableStreamRepresentation::Json,
                },
                DurableStreamSlot {
                    canonical_name: "input".into(),
                    public_name: "requests".into(),
                    direction: DurableStreamSlotDirection::Input,
                    content_type: "application/vnd.golem.fragment".into(),
                    representation: DurableStreamRepresentation::Bytes,
                },
            ],
            allow_external_writes: false,
            allow_stream_delete: false,
            allow_invocation_delete: false,
            load: None,
        }),
        component_id: ComponentId::new(),
        component_revision: ComponentRevision::INITIAL,
        agent_type: AgentTypeName("oracle-agent".to_string()),
        agent_mode: AgentMode::Durable,
        constructor_input: CompiledInputSchema {
            graph: SchemaGraph::empty(),
            input_schema: InputSchema::Parameters(vec![]),
        },
        constructor_parameters: vec![],
        phantom_selection: golem_service_base::custom_api::PhantomSelection::Policy {
            phantom: false,
        },
        method_name: "stream".to_string(),
        method_input: CompiledInputSchema {
            graph: SchemaGraph::empty(),
            input_schema: InputSchema::Parameters(vec![]),
        },
        body: RequestBodySchema::Unused,
        method_parameters: vec![],
        expected_agent_response: CompiledOutputSchema {
            graph: SchemaGraph::empty(),
            output_schema: OutputSchema::Unit,
        },
        method_description: None,
        read_only: None,
    };
    let security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
        header_name: "x-golem-session".to_string(),
    });
    let cors = CorsOptions {
        allowed_patterns: vec![OriginPattern("https://client.example".to_string())],
    };
    let route_family = |prefix: &str, route_id_offset: i32, behaviour: CallAgentBehaviour| {
        let session_path = vec![
            PathSegment::Literal {
                value: prefix.into(),
            },
            PathSegment::Literal {
                value: "invocations".into(),
            },
            PathSegment::Variable {
                display_name: "session".into(),
            },
        ];
        let mut slot_path = session_path.clone();
        slot_path.extend([
            PathSegment::Literal {
                value: "streams".into(),
            },
            PathSegment::Variable {
                display_name: "slot".into(),
            },
        ]);
        let fork_slot_path = vec![
            PathSegment::Literal {
                value: prefix.into(),
            },
            PathSegment::Literal {
                value: "forks".into(),
            },
            PathSegment::Variable {
                display_name: "fork".into(),
            },
            PathSegment::Literal {
                value: "invocations".into(),
            },
            PathSegment::Variable {
                display_name: "session".into(),
            },
            PathSegment::Literal {
                value: "streams".into(),
            },
            PathSegment::Variable {
                display_name: "slot".into(),
            },
        ];
        let route =
            |route_id, method: HttpMethod, path, body, mut behaviour: CallAgentBehaviour| {
                behaviour.body = body;
                CompiledRoute {
                    route_id,
                    route_match: method.into(),
                    path,
                    behavior: RouteBehaviour::CallAgent(behaviour),
                    security: security.clone(),
                    cors: cors.clone(),
                }
            };
        vec![
            route(
                route_id_offset + 1,
                HttpMethod::Post(Empty {}),
                vec![PathSegment::Literal {
                    value: prefix.into(),
                }],
                RequestBodySchema::JsonBody {
                    expected: CompiledSchema {
                        graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                    },
                },
                behaviour.clone(),
            ),
            route(
                route_id_offset + 2,
                HttpMethod::Get(Empty {}),
                slot_path.clone(),
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 3,
                HttpMethod::Post(Empty {}),
                slot_path.clone(),
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 4,
                HttpMethod::Delete(Empty {}),
                slot_path,
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 5,
                HttpMethod::Delete(Empty {}),
                session_path,
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 6,
                HttpMethod::Get(Empty {}),
                fork_slot_path.clone(),
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 7,
                HttpMethod::Put(Empty {}),
                fork_slot_path,
                RequestBodySchema::Unused,
                behaviour,
            ),
        ]
    };
    let mut permissive_behaviour = behaviour.clone();
    let policy = permissive_behaviour.durable_streams.as_mut().unwrap();
    policy.allow_external_writes = true;
    policy.allow_stream_delete = true;
    policy.allow_invocation_delete = true;
    let mut routes = route_family("stream", 0, behaviour);
    routes.extend(route_family("writable", 10, permissive_behaviour));

    CompiledRoutes {
        account_id: AccountId::new(),
        account_email: AccountEmail::new("oracle@golem.cloud"),
        environment_id: EnvironmentId::new(),
        deployment_revision: DeploymentRevision::INITIAL,
        security_schemes: HashMap::new(),
        routes,
    }
}

fn request_handler_and_harness() -> (RequestHandler, InvocationHarness) {
    let harness = invocation_harness();
    let handler = request_handler_with_worker(
        RouteResolver::new(
            &RouteResolverConfig {
                router_cache_max_capacity: 1,
                router_cache_ttl: Duration::from_secs(60),
                router_cache_eviction_period: Duration::from_secs(60),
                trusted_ingress_addresses: vec![],
            },
            Arc::new(StaticApiDefinitionsLookup),
        ),
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
        harness.worker_service.clone(),
    );
    (handler, harness)
}

fn request_handler_with(
    route_resolver: RouteResolver,
    initial_files: Arc<InitialAgentFilesService>,
) -> RequestHandler {
    request_handler_with_worker(
        route_resolver,
        initial_files,
        invocation_harness().worker_service,
    )
}

fn invocation_harness() -> InvocationHarness {
    InvocationHarness::new(
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
    )
}

fn request_handler_with_worker(
    route_resolver: RouteResolver,
    initial_files: Arc<InitialAgentFilesService>,
    worker_service: Arc<crate::service::worker::WorkerService>,
) -> RequestHandler {
    RequestHandler::new(
        Arc::new(route_resolver),
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
        Arc::new(WebhookCallbackHandler::new(worker_service.clone(), vec![])),
        worker_service.clone(),
        Default::default(),
        initial_files,
        Arc::new(OpenApiService::new(worker_service)),
    )
}

fn request_handler() -> RequestHandler {
    request_handler_and_harness().0
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
async fn typed_route_can_decline_h2c_upgrade_and_dispatch_over_http1() {
    let mut request = request("not-json", true, false);
    request
        .headers_mut()
        .insert(http::header::UPGRADE, "h2c".parse().unwrap());
    request.headers_mut().insert(
        http::header::CONNECTION,
        "Upgrade, HTTP2-Settings".parse().unwrap(),
    );
    request.headers_mut().insert(
        "http2-settings",
        "AAEAAEAAAAIAAAABAAMAAABk".parse().unwrap(),
    );
    let response = request_handler().handle_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()[http::header::ALLOW], "PUT");
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
        Some(&"PUT".parse().unwrap())
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

#[test]
async fn durable_stream_unknown_and_canonical_slot_names_are_not_probed() {
    let handler = request_handler();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();

    for slot in ["unknown", "$result"] {
        let request = Request::builder()
            .method(http::Method::GET)
            .uri(
                format!("http://ds.example/stream/invocations/{session}/streams/{slot}")
                    .parse()
                    .unwrap(),
            )
            .header(http::header::HOST, "ds.example")
            .header("x-golem-session", "{}")
            .finish();
        let response = handler.handle_request(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[test]
async fn durable_stream_disabled_operations_are_rejected_before_worker_calls() {
    let handler = request_handler();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    for (method, suffix) in [
        (http::Method::POST, "/streams/requests"),
        (http::Method::DELETE, "/streams/responses"),
        (http::Method::DELETE, ""),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(
                format!("http://ds.example/stream/invocations/{session}{suffix}")
                    .parse()
                    .unwrap(),
            )
            .header(http::header::HOST, "ds.example")
            .header("x-golem-session", "{}")
            .body("this body must not be read");
        let response = handler.handle_request(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response.headers().get(http::header::ALLOW),
            Some(&"PUT, HEAD, GET".parse().unwrap())
        );
    }
}

#[test]
async fn durable_stream_operation_policy_is_isolated_per_route_family() {
    let (handler, harness) = request_handler_and_harness();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let request = |family: &str| {
        Request::builder()
            .method(http::Method::DELETE)
            .uri(
                format!("http://ds.example/{family}/invocations/{session}/streams/responses")
                    .parse()
                    .unwrap(),
            )
            .header(http::header::HOST, "ds.example")
            .header("x-golem-session", "{}")
            .finish()
    };

    let response = handler.handle_request(request("stream")).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response.headers().get(http::header::ALLOW),
        Some(&"PUT, HEAD, GET".parse().unwrap())
    );
    assert!(harness.recorded_durable_stream_controls().is_empty());

    let response = handler.handle_request(request("writable")).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let controls = harness.recorded_durable_stream_controls();
    assert_eq!(controls.len(), 1);
    let control = controls[0].export_control.as_ref().unwrap();
    assert_eq!(control.session, session);
    assert_eq!(control.slot.as_deref(), Some("$result"));

    let response = handler.handle_request(request("stream")).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(harness.recorded_durable_stream_controls().len(), 1);
}

#[test]
async fn durable_stream_write_disabled_fork_rejects_initial_content_before_worker_call() {
    let handler = request_handler();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let fork = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let source = format!("/stream/invocations/{session}/streams/requests");
    for (body, closed) in [("[\"initial\"]", false), ("", true)] {
        let mut request = Request::builder()
            .method(http::Method::PUT)
            .uri(
                format!(
                    "http://ds.example/stream/forks/{fork}/invocations/{session}/streams/requests"
                )
                .parse()
                .unwrap(),
            )
            .header(http::header::HOST, "ds.example")
            .header("x-golem-session", "{}")
            .header("stream-forked-from", &source)
            .content_type("application/json");
        if closed {
            request = request.header("stream-closed", "true");
        }

        let response = handler.handle_request(request.body(body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[test]
async fn durable_stream_write_disabled_fork_rejects_oversized_initial_content_as_read_only() {
    let handler = request_handler();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let fork = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let source = format!("/stream/invocations/{session}/streams/requests");
    let request = Request::builder()
        .method(http::Method::PUT)
        .uri(
            format!("http://ds.example/stream/forks/{fork}/invocations/{session}/streams/requests")
                .parse()
                .unwrap(),
        )
        .header(http::header::HOST, "ds.example")
        .header("x-golem-session", "{}")
        .header("stream-forked-from", source)
        .content_type("application/json")
        .body(vec![
            b'x';
            crate::config::DurableStreamsConfig::default()
                .max_append_body_bytes
                + 1
        ]);

    let response = handler.handle_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[test]
async fn durable_stream_selector_keeps_named_forks_in_the_selected_root() {
    use crate::custom_api::route_resolver::tests::test_resolver;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        ForkStreamSlotSuccess, ReadStreamSlotSuccess,
    };
    use golem_common::model::OplogIndex;
    use golem_common::model::durable_stream::StreamOffset;
    use golem_service_base::custom_api::PhantomSelection;
    let p = "550e8400-e29b-41d4-a716-446655440000";
    let q = "8badf00d-1234-4567-89ab-0123456789ab";
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let target_fork =
        golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let source_fork =
        golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let named_fork = |root: &str, name: &str| {
        let hash = blake3::hash(format!("{root}\0{name}").as_bytes());
        let mut bytes: [u8; 16] = hash.as_bytes()[..16].try_into().unwrap();
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        format!("oracle-agent()[{}]", uuid::Uuid::from_bytes(bytes))
    };
    for path_selector in [false, true] {
        for root in [None, Some(p), Some(q)] {
            if path_selector && root.is_none() {
                continue;
            }
            for nested in [false, true] {
                let harness = invocation_harness();
                let mut routes = durable_stream_routes().routes;
                for route in &mut routes {
                    if let RouteBehaviour::CallAgent(call) = &mut route.behavior {
                        call.phantom_selection = if path_selector {
                            PhantomSelection::Path { index: 0.into() }
                        } else {
                            PhantomSelection::Query {
                                name: "instance".into(),
                                optional: true,
                            }
                        };
                        if path_selector {
                            route.path.insert(
                                1,
                                PathSegment::Variable {
                                    display_name: "instance".into(),
                                },
                            );
                            call.base_path_variables += 1;
                        }
                    }
                }
                let handler = request_handler_with_worker(
                    test_resolver(routes),
                    Arc::new(InitialAgentFilesService::new(Arc::new(
                        golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
                    ))),
                    harness.worker_service.clone(),
                );
                let base = if path_selector {
                    format!("/writable/{}", root.unwrap())
                } else {
                    "/writable".into()
                };
                let source = if nested {
                    format!("{base}/forks/{source_fork}/invocations/{session}/streams/requests")
                } else {
                    format!("{base}/invocations/{session}/streams/requests")
                };
                let target =
                    format!("{base}/forks/{target_fork}/invocations/{session}/streams/requests");
                let uri = if path_selector {
                    target.replace(root.unwrap(), &root.unwrap().to_uppercase())
                } else {
                    format!(
                        "{target}?{}",
                        root.map(|id| format!("instance={}&count=17", id.to_uppercase()))
                            .unwrap_or_default()
                    )
                };
                let mut invalid_sources = vec![
                    format!("{source}?instance={p}"),
                    format!("{source}#fragment"),
                ];
                if path_selector {
                    let other = if root == Some(p) { q } else { p };
                    invalid_sources.push(source.replace(root.unwrap(), other));
                }
                for source in invalid_sources {
                    let response = handler
                        .handle_request(
                            Request::builder()
                                .method(http::Method::PUT)
                                .uri(uri.parse().unwrap())
                                .header("host", "example.com")
                                .header("x-golem-session", "{}")
                                .header("stream-forked-from", source)
                                .finish(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
                    assert!(harness.recorded_durable_stream_forks().is_empty());
                }
                let offset = StreamOffset::new(OplogIndex::from_u64(42), 0)
                    .as_bytes()
                    .to_vec();
                harness.script_durable_stream_fork_success(
                    ForkStreamSlotSuccess::default(),
                    vec![ReadStreamSlotSuccess {
                        content_type: "application/octet-stream".into(),
                        next_offset: offset.clone(),
                        head_offset: offset,
                        stream_identity: "selected-fork".into(),
                        writable: true,
                        ..Default::default()
                    }],
                );
                let response = handler
                    .handle_request(
                        Request::builder()
                            .method(http::Method::PUT)
                            .uri(uri.parse().unwrap())
                            .header("host", "example.com")
                            .header("x-golem-session", "{}")
                            .header("stream-forked-from", &source)
                            .finish(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::CREATED);
                let expected_url = if path_selector {
                    target.clone()
                } else {
                    format!(
                        "{target}{}",
                        root.map(|id| format!("?instance={id}")).unwrap_or_default()
                    )
                };
                assert_eq!(response.headers()[http::header::LOCATION], expected_url);
                let forks = harness.recorded_durable_stream_forks();
                assert_eq!(forks.len(), 1);
                let root_id = root
                    .map(|id| format!("oracle-agent()[{id}]"))
                    .unwrap_or_else(|| "oracle-agent()".into());
                assert_eq!(
                    forks[0].source_agent_id.as_ref().unwrap().name,
                    if nested {
                        named_fork(&root_id, &source_fork)
                    } else {
                        root_id.clone()
                    }
                );
                assert_eq!(
                    forks[0].target_agent_id.as_ref().unwrap().name,
                    named_fork(&root_id, &target_fork)
                );
                assert_eq!(forks[0].source_path, source);
            }
        }
    }
}

#[test]
async fn durable_stream_fork_validates_public_mime_and_translates_executor_representation() {
    let (handler, harness) = request_handler_and_harness();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let source = format!("/writable/invocations/{session}/streams/requests");
    let request = |fork: &str, content_type: Option<&str>, body: Vec<u8>| {
        let mut builder = Request::builder()
            .method(http::Method::PUT)
            .uri(
                format!(
                    "http://ds.example/writable/forks/{fork}/invocations/{session}/streams/requests"
                )
                .parse()
                .unwrap(),
            )
            .header(http::header::HOST, "ds.example")
            .header("x-golem-session", "{}")
            .header("stream-forked-from", &source)
            .header("stream-fork-sub-offset", "2");
        if let Some(content_type) = content_type {
            builder = builder.content_type(content_type);
        }
        builder.body(body)
    };

    let mismatch_fork =
        golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let response = handler
        .handle_request(request(
            &mismatch_fork,
            Some("application/octet-stream"),
            vec![1],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(harness.recorded_durable_stream_forks().is_empty());

    let explicit_fork =
        golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let response = handler
        .handle_request(request(
            &explicit_fork,
            Some("Application/Vnd.Golem.Fragment; version=1"),
            vec![1],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let forks = harness.recorded_durable_stream_forks();
    assert_eq!(forks.len(), 1);
    assert_eq!(forks[0].slot, "input");
    assert_eq!(forks[0].source_path, source);
    assert_eq!(forks[0].sub_offset, 2);
    assert_eq!(
        forks[0].content_type.as_deref(),
        Some("application/octet-stream")
    );

    let inherited_fork =
        golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let response = handler
        .handle_request(request(&inherited_fork, None, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let forks = harness.recorded_durable_stream_forks();
    assert_eq!(forks.len(), 2);
    assert_eq!(forks[1].content_type, None);
}

#[test]
async fn durable_stream_close_only_fork_ignores_content_type() {
    let (handler, harness) = request_handler_and_harness();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let fork = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let source = format!("/writable/invocations/{session}/streams/requests");
    let request = Request::builder()
        .method(http::Method::PUT)
        .uri(
            format!(
                "http://ds.example/writable/forks/{fork}/invocations/{session}/streams/requests"
            )
            .parse()
            .unwrap(),
        )
        .header(http::header::HOST, "ds.example")
        .header("x-golem-session", "{}")
        .header("stream-forked-from", source)
        .header("stream-closed", "true")
        .content_type("application/not-the-slot-type")
        .finish();

    let response = handler.handle_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let forks = harness.recorded_durable_stream_forks();
    assert_eq!(forks.len(), 1);
    assert!(forks[0].closed);
    assert_eq!(forks[0].content_type, None);
}

#[test]
async fn durable_stream_successful_fork_preserves_partial_utf8_byte_prefix() {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        ForkStreamSlotSuccess, ReadStreamSlotSuccess, StreamSlotItem, stream_slot_item,
    };
    use golem_common::model::OplogIndex;
    use golem_common::model::durable_stream::StreamOffset;

    let (handler, harness) = request_handler_and_harness();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let fork = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let source = format!("/writable/invocations/{session}/streams/requests");
    let origin = StreamOffset::new(OplogIndex::from_u64(42), 0);
    let metadata = ReadStreamSlotSuccess {
        content_type: "application/octet-stream".into(),
        next_offset: origin.as_bytes().to_vec(),
        head_offset: origin.as_bytes().to_vec(),
        stream_identity: "forked-stream".into(),
        writable: true,
        ..Default::default()
    };
    let page = ReadStreamSlotSuccess {
        items: vec![StreamSlotItem {
            offset: origin.as_bytes().to_vec(),
            content: Some(stream_slot_item::Content::PackedU8(vec![0xe2, 0x82])),
        }],
        up_to_date: true,
        ..metadata.clone()
    };
    harness.script_durable_stream_fork_success(
        ForkStreamSlotSuccess {
            source_path: source.clone(),
            fork_offset: origin.as_bytes().to_vec(),
            sub_offset: 2,
            ..Default::default()
        },
        vec![metadata, page],
    );

    let fork_path = format!("/writable/forks/{fork}/invocations/{session}/streams/requests");
    let response = handler
        .handle_request(
            Request::builder()
                .method(http::Method::PUT)
                .uri(format!("http://ds.example{fork_path}").parse().unwrap())
                .header(http::header::HOST, "ds.example")
                .header("x-golem-session", "{}")
                .header("stream-forked-from", &source)
                .header("stream-fork-offset", origin.to_string())
                .header("stream-fork-sub-offset", "2")
                .finish(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response.headers().get(http::header::CONTENT_TYPE),
        Some(&"application/vnd.golem.fragment".parse().unwrap())
    );
    let forks = harness.recorded_durable_stream_forks();
    assert_eq!(forks.len(), 1);
    assert_eq!(forks[0].fork_offset, Some(origin.as_bytes().to_vec()));
    assert_eq!(forks[0].sub_offset, 2);
    assert_eq!(forks[0].slot, "input");
    assert_eq!(forks[0].content_type, None);

    let response = handler
        .handle_request(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("http://ds.example{fork_path}").parse().unwrap())
                .header(http::header::HOST, "ds.example")
                .header("x-golem-session", "{}")
                .finish(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(http::header::CONTENT_TYPE),
        Some(&"application/vnd.golem.fragment".parse().unwrap())
    );
    assert_eq!(
        response.into_body().into_bytes().await.unwrap().as_ref(),
        [0xe2, 0x82]
    );
}

#[test]
async fn immutable_file_corpus_through_request_handler() {
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::FileMapping;
    use golem_service_base::custom_api::RouterFileIndexEntry;
    use golem_service_base::replayable_stream::ReplayableStream;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;

    fn decode(value: &serde_json::Value) -> Vec<u8> {
        value
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let mut tested = 0;
    for case in corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["suite"] == "files" && case["input"]["mode"] == "immutable")
    {
        let id = case["id"].as_str().unwrap();
        let input = &case["input"];
        let expect = &case["expect"];
        let files = Arc::new(InitialAgentFilesService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        let mut route = test_route(1, "/", None, "router");
        route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
        let RouteBehaviour::HttpRouter(router) = &mut route.behavior else {
            unreachable!()
        };
        router.static_bindings = FileMapping::compile_list([("/*", "/$1")]).unwrap();
        let mut provisions = vec![];
        if input["body_hex"].is_string() {
            provisions.push((decode(&input["body_hex"]), true));
        } else if let Some(revisions) = input["revisions"].as_object() {
            router.component_revision =
                ComponentRevision::try_from(input["selected_revision"].as_u64().unwrap()).unwrap();
            for (revision, file) in revisions {
                provisions.push((
                    decode(&file["body_hex"]),
                    revision.parse::<u64>().unwrap()
                        == input["selected_revision"].as_u64().unwrap(),
                ));
            }
        } else if let Some(indexes) = input["indexes"].as_array() {
            for index in indexes {
                provisions.push((
                    decode(&index["body_hex"]),
                    index["owner"] == input["selected_owner"],
                ));
            }
        } else {
            assert_eq!(input["provisioned"]["read_only"], false, "{id}");
            provisions.push((decode(&input["provisioned"]["body_hex"]), false));
        }
        for (bytes, indexed) in provisions {
            let size = bytes.len() as u64;
            let key = files
                .put_if_not_exists(
                    EnvironmentId(uuid::Uuid::nil()),
                    bytes
                        .map_item(|item| item.map_err(anyhow::Error::from))
                        .map_error(anyhow::Error::from),
                )
                .await
                .unwrap();
            if indexed {
                router.file_index.push(RouterFileIndexEntry {
                    path: input["path"].as_str().unwrap().into(),
                    blob_key: key,
                    size,
                });
            }
        }
        let handler = request_handler_with(test_resolver(vec![route]), files);
        let mut request = Request::builder()
            .uri(input["path"].as_str().unwrap().parse().unwrap())
            .method(input["method"].as_str().unwrap().parse().unwrap())
            .header("host", "example.com")
            .header("origin", "https://client.example");
        for header in input["headers"].as_array().into_iter().flatten() {
            request = request.header(header[0].as_str().unwrap(), header[1].as_str().unwrap());
        }
        let response = handler
            .handle_request(request.finish())
            .await
            .unwrap_or_else(|error| panic!("{id}: {error:?}"));
        let status = expect["status"]
            .as_u64()
            .unwrap_or(if expect["lookup"] == "absent" {
                404
            } else {
                200
            });
        assert_eq!(u64::from(response.status().as_u16()), status, "{id}");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://client.example",
            "{id}"
        );
        for header in expect["headers"].as_array().into_iter().flatten() {
            assert_eq!(
                response.headers().get(header[0].as_str().unwrap()).unwrap(),
                header[1].as_str().unwrap(),
                "{id}"
            );
        }
        for header in expect["absent_headers"].as_array().into_iter().flatten() {
            assert!(
                !response.headers().contains_key(header.as_str().unwrap()),
                "{id}"
            );
        }
        if !expect["body_hex"].is_null() {
            assert_eq!(
                response.into_body().into_vec().await.unwrap(),
                decode(&expect["body_hex"]),
                "{id}"
            );
        }
        tested += 1;
    }
    assert_eq!(tested, 11);
}

#[test]
#[test_r::timeout("20s")]
async fn immutable_files_over_http1_and_http2_do_not_cache_contents() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::FileMapping;
    use golem_service_base::custom_api::RouterFileIndexEntry;
    use golem_service_base::replayable_stream::ReplayableStream;
    use golem_service_base::storage::blob::{
        BlobStorage, BlobStorageNamespace, memory::InMemoryBlobStorage,
    };
    use tokio_util::task::AbortOnDropHandle;

    for http2 in [false, true] {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let files = Arc::new(InitialAgentFilesService::new(storage.clone()));
        let environment_id = EnvironmentId(uuid::Uuid::nil());
        let key = files
            .put_if_not_exists(
                environment_id,
                b"abcdefg"
                    .to_vec()
                    .map_item(|item| item.map_err(anyhow::Error::from))
                    .map_error(anyhow::Error::from),
            )
            .await
            .unwrap();
        let mut route = test_route(1, "/", None, "router");
        route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
        let RouteBehaviour::HttpRouter(router) = &mut route.behavior else {
            unreachable!()
        };
        router.static_bindings = FileMapping::compile_list([("/asset", "/private/a.txt")]).unwrap();
        router.file_index.push(RouterFileIndexEntry {
            path: "/private/a.txt".into(),
            blob_key: key,
            size: 7,
        });
        let handler = Arc::new(request_handler_with(test_resolver(vec![route]), files));
        let endpoint = CustomApiPoemEndpoint::new(handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
        let _server =
            AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(acceptor, endpoint)));
        let builder = reqwest::Client::builder().timeout(Duration::from_secs(5));
        let client = if http2 {
            builder.http2_prior_knowledge()
        } else {
            builder.http1_only()
        }
        .build()
        .unwrap();
        for (method, conditional, status, length, bytes) in [
            (
                reqwest::Method::GET,
                false,
                200,
                Some("7"),
                b"abcdefg".as_slice(),
            ),
            (reqwest::Method::HEAD, false, 200, Some("7"), b"".as_slice()),
            (reqwest::Method::GET, true, 304, None, b"".as_slice()),
            (reqwest::Method::HEAD, true, 304, None, b"".as_slice()),
        ] {
            let mut request = client.request(method, format!("http://{address}/asset"));
            if conditional {
                request = request.header("if-none-match", "*");
            }
            let response = request.send().await.unwrap();
            assert_eq!(
                response.version(),
                if http2 {
                    http::Version::HTTP_2
                } else {
                    http::Version::HTTP_11
                }
            );
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response
                    .headers()
                    .get("content-length")
                    .map(|v| v.to_str().unwrap()),
                length
            );
            assert_eq!(response.bytes().await.unwrap().as_ref(), bytes);
        }
        storage
            .delete(
                "test",
                "delete",
                BlobStorageNamespace::InitialAgentFiles { environment_id },
                &std::path::PathBuf::from(key.0.to_string()),
            )
            .await
            .unwrap();
        for (method, conditional) in [
            (reqwest::Method::GET, false),
            (reqwest::Method::GET, true),
            (reqwest::Method::HEAD, true),
        ] {
            let mut request = client
                .request(method, format!("http://{address}/asset"))
                .header("origin", "https://client.example");
            if conditional {
                request = request.header("if-none-match", "*");
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(
                response.headers()["access-control-allow-origin"],
                "https://client.example"
            );
            let body = response.text().await.unwrap();
            assert!(
                !body.contains("private") && !body.contains("blob"),
                "{body}"
            );
        }
    }
}

struct MovingOpenApiLookup(std::sync::atomic::AtomicBool);

#[async_trait]
impl HttpApiDefinitionsLookup for MovingOpenApiLookup {
    async fn get(
        &self,
        _: &golem_common::model::domain_registration::Domain,
    ) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
        use crate::custom_api::route_resolver::tests::test_route;
        let moved = self.0.load(std::sync::atomic::Ordering::SeqCst);
        let mut router = test_route(1, "/router", None, "router");
        let RouteBehaviour::HttpRouter(behavior) = &mut router.behavior else {
            unreachable!()
        };
        behavior.openapi_provider_method = Some(golem_service_base::custom_api::RouterMethod {
            method_name: "describe".into(),
            input: CompiledInputSchema {
                graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                input_schema: InputSchema::Parameters(vec![]),
            },
            output: CompiledOutputSchema {
                graph: SchemaGraph::anonymous(SchemaType::string()),
                output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
            },
        });
        Ok(CompiledRoutes {
            account_id: AccountId(uuid::Uuid::nil()),
            account_email: AccountEmail::new("test@golem"),
            environment_id: EnvironmentId(uuid::Uuid::nil()),
            deployment_revision: if moved {
                DeploymentRevision::new(3).unwrap()
            } else {
                DeploymentRevision::INITIAL
            },
            security_schemes: HashMap::new(),
            routes: vec![
                test_route(
                    0,
                    if moved {
                        "/new/openapi.json"
                    } else {
                        "/old/openapi.json"
                    },
                    Some("GET"),
                    "reserved",
                ),
                router,
            ],
        })
    }
}

fn openapi_request(path: &str) -> Request {
    Request::builder()
        .uri(path.parse().unwrap())
        .header("host", "example.com")
        .finish()
}

#[test]
#[test_r::timeout("30s")]
async fn in_flight_openapi_snapshot_can_finish_after_endpoint_moves() {
    use crate::custom_api::openapi::test_support::{controlled, document};
    let lookup = Arc::new(MovingOpenApiLookup(std::sync::atomic::AtomicBool::new(
        false,
    )));
    let resolver = RouteResolver::new(&RouteResolverConfig::default(), lookup.clone());
    let mut handler = request_handler_with(
        resolver,
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
    );
    let (service, mut calls) = controlled();
    handler.openapi_service = Arc::new(service);
    let handler = Arc::new(handler);
    let task = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .handle_request(openapi_request("/old/openapi.json"))
                .await
        }
    });
    let (_, old_reply) = calls.recv().await.unwrap();
    lookup.0.store(true, std::sync::atomic::Ordering::SeqCst);
    handler.route_resolver.clear_all().await;
    old_reply.send(Ok(document())).unwrap();
    assert_eq!(task.await.unwrap().unwrap().status(), StatusCode::OK);

    assert!(matches!(
        handler
            .handle_request(openapi_request("/old/openapi.json"))
            .await
            .unwrap_err()
            .error,
        RequestHandlerError::ResolvingRouteFailed(RouteResolverError::NoMatchingRoute)
    ));
    let current = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .handle_request(openapi_request("/new/openapi.json"))
                .await
        }
    });
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    assert_eq!(current.await.unwrap().unwrap().status(), StatusCode::OK);
    let ordinary = handler
        .handle_request(openapi_request("/router/unmapped"))
        .await
        .unwrap();
    assert_eq!(ordinary.status(), StatusCode::NOT_FOUND);
    assert!(calls.try_recv().is_err());
}

#[test]
async fn new_openapi_snapshot_uses_a_new_generation_without_a_provider_deadline() {
    use crate::custom_api::openapi::test_support::{controlled, document};
    let lookup = Arc::new(MovingOpenApiLookup(std::sync::atomic::AtomicBool::new(
        false,
    )));
    let resolver = RouteResolver::new(&RouteResolverConfig::default(), lookup.clone());
    let mut handler = request_handler_with(
        resolver,
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
    );
    let (service, mut calls) = controlled();
    handler.openapi_service = Arc::new(service);
    let handler = Arc::new(handler);

    let first = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .handle_request(openapi_request("/old/openapi.json"))
                .await
        }
    });
    calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);

    lookup.0.store(true, std::sync::atomic::Ordering::SeqCst);
    handler.route_resolver.clear_all().await;
    let second = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .handle_request(openapi_request("/new/openapi.json"))
                .await
        }
    });
    let (_, reply) = calls.recv().await.unwrap();
    assert!(!reply.is_closed());
    reply.send(Ok(document())).unwrap();
    assert_eq!(second.await.unwrap().unwrap().status(), StatusCode::OK);
}

#[test]
async fn ordinary_traffic_lazy_openapi_corpus() {
    use crate::custom_api::openapi::test_support::{controlled, provider_routes};
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use futures::{FutureExt, StreamExt};
    use golem_common::model::agent::FileMapping;
    use golem_common::model::filesystem::FileReadHead;
    use golem_service_base::custom_api::RouterFileIndexEntry;
    use golem_service_base::model::FileReadResponse;
    use golem_service_base::replayable_stream::ReplayableStream;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let case = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "cache-ordinary-traffic-lazy")
        .unwrap();
    let files = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let bytes = b"static-body".to_vec();
    let key = files
        .put_if_not_exists(
            EnvironmentId(uuid::Uuid::nil()),
            bytes
                .map_item(|item| item.map_err(anyhow::Error::from))
                .map_error(anyhow::Error::from),
        )
        .await
        .unwrap();
    let mut routes = provider_routes(1);
    let RouteBehaviour::HttpRouter(router) = &mut routes[1].behavior else {
        unreachable!()
    };
    router.static_bindings = FileMapping::compile_list([("/static", "/asset")]).unwrap();
    router.file_index.push(RouterFileIndexEntry {
        path: "/asset".into(),
        blob_key: key,
        size: 11,
    });
    let mut live = test_route(2, "/live", None, "filesystem");
    let RouteBehaviour::AgentFilesystem(filesystem) = &mut live.behavior else {
        unreachable!()
    };
    filesystem.filesystem_bindings = FileMapping::compile_list([("/*", "/$1")]).unwrap();
    routes.push(live);
    let harness = invocation_harness();
    harness.file_reads.responses.lock().unwrap().push_back(
        std::future::ready(Ok(FileReadResponse {
            head: FileReadHead::Absent,
            body: futures::stream::empty().boxed(),
        }))
        .boxed(),
    );
    let mut handler =
        request_handler_with_worker(test_resolver(routes), files, harness.worker_service.clone());
    let (service, mut calls) = controlled();
    handler.openapi_service = Arc::new(service);
    for event in case["input"]["events"].as_array().unwrap() {
        let (path, status) = match event.as_str().unwrap() {
            "request-handler" => ("/r1/missing", StatusCode::NOT_FOUND),
            "request-static" => ("/r1/static", StatusCode::OK),
            "request-live-files" => ("/live/file", StatusCode::NOT_FOUND),
            other => panic!("unhandled ordinary event {other}"),
        };
        let response = handler.handle_request(openapi_request(path)).await.unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            assert_eq!(
                response.into_body().into_vec().await.unwrap(),
                b"static-body"
            );
        }
    }
    let file_reads = harness.file_reads.calls.lock().unwrap();
    assert_eq!(file_reads.len(), 1);
    assert_eq!(file_reads[0].path, "/file");
    let mut observed = 0;
    while calls.try_recv().is_ok() {
        observed += 1;
    }
    assert_eq!(
        serde_json::json!(observed),
        case["expect"]["provider_calls"]
    );
}
