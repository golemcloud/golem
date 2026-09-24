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
    CompiledRoutes, CompiledSchema, CorsOptions, DurableStreamRepresentation,
    DurableStreamRoutePolicy, DurableStreamSlot, DurableStreamSlotDirection, OriginPattern,
    PathSegment, RequestBodySchema, RouteBehaviour, RouteSecurity, SessionFromHeaderRouteSecurity,
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
        vec![
            CompiledRoute {
                route_id: route_id_offset + 1,
                method: HttpMethod::Post(Empty {}),
                path: vec![PathSegment::Literal {
                    value: prefix.into(),
                }],
                body: RequestBodySchema::JsonBody {
                    expected: CompiledSchema {
                        graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                    },
                },
                behavior: RouteBehaviour::CallAgent(behaviour.clone()),
                security: security.clone(),
                cors: cors.clone(),
            },
            CompiledRoute {
                route_id: route_id_offset + 2,
                method: HttpMethod::Get(Empty {}),
                path: slot_path.clone(),
                body: RequestBodySchema::Unused,
                behavior: RouteBehaviour::CallAgent(behaviour.clone()),
                security: security.clone(),
                cors: cors.clone(),
            },
            CompiledRoute {
                route_id: route_id_offset + 3,
                method: HttpMethod::Post(Empty {}),
                path: slot_path.clone(),
                body: RequestBodySchema::Unused,
                behavior: RouteBehaviour::CallAgent(behaviour.clone()),
                security: security.clone(),
                cors: cors.clone(),
            },
            CompiledRoute {
                route_id: route_id_offset + 4,
                method: HttpMethod::Delete(Empty {}),
                path: slot_path,
                body: RequestBodySchema::Unused,
                behavior: RouteBehaviour::CallAgent(behaviour.clone()),
                security: security.clone(),
                cors: cors.clone(),
            },
            CompiledRoute {
                route_id: route_id_offset + 5,
                method: HttpMethod::Delete(Empty {}),
                path: session_path,
                body: RequestBodySchema::Unused,
                behavior: RouteBehaviour::CallAgent(behaviour.clone()),
                security: security.clone(),
                cors: cors.clone(),
            },
            CompiledRoute {
                route_id: route_id_offset + 6,
                method: HttpMethod::Get(Empty {}),
                path: fork_slot_path.clone(),
                body: RequestBodySchema::Unused,
                behavior: RouteBehaviour::CallAgent(behaviour.clone()),
                security: security.clone(),
                cors: cors.clone(),
            },
            CompiledRoute {
                route_id: route_id_offset + 7,
                method: HttpMethod::Put(Empty {}),
                path: fork_slot_path,
                body: RequestBodySchema::Unused,
                behavior: RouteBehaviour::CallAgent(behaviour),
                security: security.clone(),
                cors: cors.clone(),
            },
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
    let worker_service = invocation_harness.worker_service.clone();
    let route_resolver = Arc::new(RouteResolver::new(
        &RouteResolverConfig {
            router_cache_max_capacity: 1,
            router_cache_ttl: Duration::from_secs(60),
            router_cache_eviction_period: Duration::from_secs(60),
        },
        Arc::new(StaticApiDefinitionsLookup),
    ));

    (
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
        ),
        invocation_harness,
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
