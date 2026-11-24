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
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, AgentTypeName, HttpMethod};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::{
    AgentConstructorSchema, InputSchema, OutputSchema, SchemaGraph, SchemaType,
};
use golem_schema::Empty;
use golem_service_base::custom_api::{
    AgentRouteMode, CallAgentBehaviour, CompiledInputSchema, CompiledOutputSchema, CompiledRoute,
    CompiledRoutes, CompiledSchema, CorsOptions, DurableStreamRepresentation,
    DurableStreamRoutePolicy, DurableStreamSlot, DurableStreamSlotDirection, MountAgentMethod,
    MountTarget, OriginPattern, PathSegment, RequestBodySchema, RouteBehaviour, RouteSecurity,
    SessionFromHeaderRouteSecurity,
};
use std::collections::HashMap;
use std::time::Duration;
use test_r::test;

#[path = "live_files_tests.rs"]
mod live_files;

fn compiled_input(input_schema: InputSchema) -> CompiledInputSchema {
    let graph = SchemaGraph::anonymous(SchemaType::record(
        input_schema
            .fields()
            .iter()
            .map(|field| golem_common::schema::NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    ));
    CompiledInputSchema {
        graph,
        input_schema,
    }
}

#[test]
async fn authenticated_finalization_preserves_repeated_no_store() {
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};

    for kind in ["typed", "router", "filesystem"] {
        for no_store_first in [false, true] {
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
            apply_session_from_header_security_middleware(&mut request, &selected).unwrap();
            assert!(request.authenticated_session().is_some());
            let mut response = Response::default();
            let values = if no_store_first {
                ["No-StOrE", "public, max-age=600"]
            } else {
                ["public, max-age=600", "No-StOrE"]
            };
            for value in values {
                response
                    .headers_mut()
                    .append(http::header::CACHE_CONTROL, value.parse().unwrap());
            }
            response.headers_mut().insert(
                http::header::EXPIRES,
                "Thu, 01 Dec 2094 16:00:00 GMT".parse().unwrap(),
            );
            let response = finish_selected_response(Ok(response), &request, &selected).unwrap();
            let directives = response
                .headers()
                .get_all(http::header::CACHE_CONTROL)
                .iter()
                .flat_map(|value| value.to_str().unwrap().split(','))
                .map(str::trim)
                .collect::<Vec<_>>();
            assert!(
                directives
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case("private"))
            );
            assert!(
                directives
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case("no-store"))
            );
            assert!(!directives.iter().any(|value| value.starts_with("max-age=")));
            assert!(!response.headers().contains_key(http::header::EXPIRES));
        }
    }
}

#[test]
async fn typed_cache_privacy_follows_authentication_not_principal_usage() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::{CachePolicy, CachePolicyTtl, ReadOnlyConfig};
    use golem_common::model::{AgentFingerprint, OplogIndex};

    for authenticated in [false, true] {
        for no_store in [false, true] {
            let harness = InvocationHarness::new(
                AgentInvocationOutput {
                    result: golem_common::model::AgentInvocationResult::AgentMethod {
                        output: golem_common::schema::SchemaValue::Tuple { elements: vec![] },
                    },
                    consumed_fuel: None,
                    invocation_status: None,
                    component_revision: None,
                    agent_id: None,
                    idempotency_key: None,
                    oplog_index: Some(OplogIndex::from_u64(17)),
                    agent_fingerprint: Some(AgentFingerprint(uuid::Uuid::new_v4())),
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
            let mut route = test_route(1, "/cache", Some("GET"), "typed");
            if authenticated {
                route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                    header_name: "x-session".into(),
                });
            }
            let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
                unreachable!()
            };
            call.component_id = harness.component_id;
            call.agent_type = AgentTypeName("mcp-agent".into());
            call.read_only = Some(ReadOnlyConfig {
                cache_policy: if no_store {
                    CachePolicy::NoCache(Empty {})
                } else {
                    CachePolicy::Ttl(CachePolicyTtl {
                        duration_nanos: 90_500_000_000,
                    })
                },
            });
            let mut post = test_route(2, "/cache", Some("POST"), "typed");
            post.behavior = RouteBehaviour::CallAgent(call.clone());
            let mut head = test_route(3, "/cache", Some("HEAD"), "typed");
            head.behavior = RouteBehaviour::CallAgent(call.clone());
            post.security = route.security.clone();
            head.security = route.security.clone();
            let endpoint = CustomApiPoemEndpoint::new(Arc::new(request_handler_with_worker(
                test_resolver(vec![route, post, head]),
                Arc::new(InitialAgentFilesService::new(Arc::new(
                    golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
                ))),
                harness.worker_service.clone(),
            )));
            for method in [http::Method::GET, http::Method::HEAD, http::Method::POST] {
                let response = endpoint
                    .execute(
                        Request::builder()
                            .method(method.clone())
                            .uri("/cache".parse().unwrap())
                            .header("host", "example.com")
                            .header("x-session", "{}")
                            .finish(),
                    )
                    .await;
                assert_eq!(response.status(), StatusCode::NO_CONTENT);
                let eligible = method != http::Method::POST;
                let expected = match (authenticated, no_store, eligible) {
                    (true, true, true) => Some("private, no-store"),
                    (true, _, _) => Some("private, no-cache"),
                    (false, true, true) => Some("no-store"),
                    (false, false, true) => Some("public, max-age=90"),
                    (false, _, false) => None,
                };
                assert_eq!(
                    response
                        .headers()
                        .get(http::header::CACHE_CONTROL)
                        .map(|v| v.to_str().unwrap()),
                    expected
                );
                assert_eq!(
                    response.headers().contains_key(http::header::ETAG),
                    eligible && !no_store
                );
                if !authenticated {
                    assert_eq!(
                        response.headers()[http::header::VARY],
                        "Authorization, Cookie, Golem-Token"
                    );
                }
                assert!(response.into_body().into_vec().await.unwrap().is_empty());
            }
        }
    }
}

#[test]
async fn http_method_arguments_use_the_deployed_schema() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::schema::{AgentMethodSchema, NamedField, SchemaValue};
    use golem_service_base::custom_api::{MethodParameter, PathSegmentType, QueryOrHeaderType};

    let input = InputSchema::parameters([
        NamedField::user_supplied("count", SchemaType::u32()),
        NamedField::user_supplied("message", SchemaType::string()),
        NamedField::user_supplied("suffix", SchemaType::string()),
    ]);
    let harness = route_harness(
        "oracle-agent",
        DURABLE_STREAM_COMPONENT,
        vec![AgentMethodSchema {
            name: "run".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters([NamedField::user_supplied(
                "count",
                SchemaType::string(),
            )]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        }],
    );
    let mut route = test_route(1, "/run", Some("POST"), "typed");
    let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
        unreachable!()
    };
    call.component_id = harness.component_id;
    call.agent_type = harness.agent_type.type_name.clone();
    call.method_input = compiled_input(input);
    call.body = RequestBodySchema::JsonBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(SchemaType::record_from_fields([
                ("text", SchemaType::string()),
                ("tail", SchemaType::string()),
            ])),
        },
    };
    call.method_parameters = vec![
        MethodParameter::Query {
            query_parameter_name: "wire".into(),
            parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::U32),
        },
        MethodParameter::JsonObjectBodyField {
            field_index: 0.into(),
        },
        MethodParameter::JsonObjectBodyField {
            field_index: 1.into(),
        },
    ];
    let durable_call = call.clone();
    let handler = request_handler_with_worker(
        test_resolver(vec![route]),
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
        harness.worker_service.clone(),
    );
    let endpoint = CustomApiPoemEndpoint::new(Arc::new(handler));
    let response = endpoint
        .execute(
            Request::builder()
                .method(http::Method::POST)
                .uri("/run?wire=7".parse().unwrap())
                .header("host", "example.com")
                .header("content-type", "application/json")
                .body(r#"{"tail":"!","text":"hello"}"#),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "{}",
        response.into_body().into_string().await.unwrap()
    );
    assert_eq!(
        harness.recorded_method_params(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(7),
                SchemaValue::String("hello".into()),
                SchemaValue::String("!".into())
            ],
        }
    );

    let mut route = test_route(1, "/run", Some("PUT"), "typed");
    route.behavior = RouteBehaviour::CallAgent(durable_call);
    let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
        unreachable!()
    };
    call.route_mode = AgentRouteMode::DurableStreams;
    call.method_input = compiled_input(InputSchema::parameters([
        NamedField::user_supplied("numbers", SchemaType::stream(Some(SchemaType::u32()))),
        NamedField::user_supplied("count", SchemaType::u32()),
        NamedField::user_supplied("message", SchemaType::string()),
        NamedField::user_supplied("labels", SchemaType::stream(Some(SchemaType::string()))),
        NamedField::user_supplied("suffix", SchemaType::string()),
    ]));
    call.durable_streams = Some(DurableStreamRoutePolicy {
        family_id: 1,
        slots: ["numbers", "labels"]
            .into_iter()
            .map(|name| DurableStreamSlot {
                canonical_name: name.into(),
                public_name: name.into(),
                direction: DurableStreamSlotDirection::Input,
                content_type: "application/json".into(),
                representation: DurableStreamRepresentation::Json,
            })
            .collect(),
        allow_external_writes: true,
        allow_stream_delete: false,
        allow_invocation_delete: false,
        load: None,
    });
    let handler = request_handler_with_worker(
        test_resolver(vec![route]),
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
        harness.worker_service.clone(),
    );
    let response = CustomApiPoemEndpoint::new(Arc::new(handler))
        .execute(
            Request::builder()
                .method(http::Method::PUT)
                .uri("/run?wire=7".parse().unwrap())
                .header("host", "example.com")
                .header("content-type", "application/json")
                .body(r#"{"tail":"!","text":"hello"}"#),
        )
        .await;
    // The harness records dispatch, then deliberately refuses session creation.
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let inputs = harness.recorded_method_inputs();
    assert_eq!(inputs.len(), 2);
    let Some(golem_common::model::agent::AgentMethodInput::Native(input)) = &inputs[1] else {
        panic!("expected deployed native input")
    };
    let value = input
        .decode(|ordinal, handle| {
            assert!(handle.is_none());
            Ok(golem_common::schema::stream::SchemaValueStream::from_host_endpoint(ordinal as u64))
        })
        .unwrap();
    let SchemaValue::Record { fields } = value else {
        panic!("expected deployed argument order")
    };
    assert_eq!(fields[1], SchemaValue::U32(7));
    assert_eq!(fields[2], SchemaValue::String("hello".into()));
    assert_eq!(fields[4], SchemaValue::String("!".into()));
    for (index, ordinal) in [(0, 0u64), (3, 1u64)] {
        let SchemaValue::Stream(stream) = &fields[index] else {
            panic!("expected interleaved stream")
        };
        assert_eq!(
            stream.with_host_endpoint::<u64, _>(|id| *id).unwrap(),
            ordinal
        );
    }
}

#[test]
async fn bound_input_errors_preserve_codes_names_and_protocol_bodies() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::schema::NamedField;
    use golem_common::schema::schema_type::{NumericBound, NumericRestrictions};
    use golem_service_base::custom_api::{MethodParameter, PathSegmentType, QueryOrHeaderType};
    use golem_service_base::model::SafeIndex;

    let mut failures = Vec::new();
    for durable in [false, true] {
        for binding in ["query", "header", "path", "map", "optional-map"] {
            if durable && binding == "path" {
                continue;
            }
            let harness = invocation_harness();
            let mut route = test_route(
                1,
                if binding == "path" { "/{wire}" } else { "/run" },
                Some(if durable { "PUT" } else { "GET" }),
                "typed",
            );
            let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
                unreachable!()
            };
            call.component_id = harness.component_id;
            call.agent_type = harness.agent_type.type_name.clone();
            let map = binding.ends_with("map");
            let input_type = if map {
                let ty = SchemaType::map(SchemaType::string(), SchemaType::bool());
                if binding == "optional-map" {
                    SchemaType::option(ty)
                } else {
                    ty
                }
            } else {
                SchemaType::U32 {
                    restrictions: Some(NumericRestrictions {
                        min: Some(NumericBound::Unsigned(2)),
                        max: None,
                        unit: None,
                    }),
                    metadata: Default::default(),
                }
            };
            call.method_input = CompiledInputSchema {
                graph: SchemaGraph::empty(),
                input_schema: InputSchema::parameters([NamedField::user_supplied(
                    "amount", input_type,
                )]),
            };
            call.method_input.graph = SchemaGraph::anonymous(SchemaType::record(
                call.method_input
                    .input_schema
                    .fields()
                    .iter()
                    .map(|field| golem_common::schema::NamedFieldType {
                        name: field.name.clone(),
                        body: field.schema.clone(),
                        metadata: field.metadata.clone(),
                    })
                    .collect(),
            ));
            call.method_parameters = vec![match binding {
                "map" | "optional-map" => {
                    let ty = QueryOrHeaderType::Map {
                        key: PathSegmentType::Str,
                        value: PathSegmentType::Bool,
                    };
                    MethodParameter::Query {
                        query_parameter_name: "wire".into(),
                        parameter_type: if binding == "optional-map" {
                            QueryOrHeaderType::Option {
                                owner: None,
                                name: None,
                                inner: Box::new(ty),
                            }
                        } else {
                            ty
                        },
                    }
                }
                "query" => MethodParameter::Query {
                    query_parameter_name: "wire".into(),
                    parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::U32),
                },
                "header" => MethodParameter::Header {
                    header_name: "x-wire".into(),
                    parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::U32),
                },
                _ => MethodParameter::Path {
                    path_segment_index: SafeIndex::new(0),
                    parameter_type: PathSegmentType::U32,
                },
            }];
            if durable {
                call.route_mode = AgentRouteMode::DurableStreams;
                call.durable_streams = Some(DurableStreamRoutePolicy {
                    family_id: 1,
                    slots: vec![DurableStreamSlot {
                        canonical_name: "events".into(),
                        public_name: "events".into(),
                        direction: DurableStreamSlotDirection::Output,
                        content_type: "application/json".into(),
                        representation: DurableStreamRepresentation::Json,
                    }],
                    allow_external_writes: false,
                    allow_stream_delete: false,
                    allow_invocation_delete: false,
                    load: None,
                });
            }
            let handler = request_handler_with_worker(
                test_resolver(vec![route]),
                Arc::new(InitialAgentFilesService::new(Arc::new(
                    golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
                ))),
                harness.worker_service.clone(),
            );
            let endpoint = CustomApiPoemEndpoint::new(Arc::new(handler));
            let cases: &[(&str, &[&str], &str, &str)] = match binding {
                "map" | "optional-map" => &[
                    ("/run?wire=x", &[], "REQUEST_VALUE_PARSING_FAILED", "amount"),
                    (
                        "/run?w%69re=x&wire[env]=true",
                        &[],
                        "REQUEST_VALUE_PARSING_FAILED",
                        "amount",
                    ),
                ],
                "query" => &[
                    ("/run", &[], "REQUEST_MISSING_VALUE", "amount"),
                    (
                        "/run?wire=3&wire=4",
                        &[],
                        "REQUEST_TOO_MANY_VALUES",
                        "amount",
                    ),
                    (
                        "/run?wire=bad",
                        &[],
                        "REQUEST_VALUE_PARSING_FAILED",
                        "amount",
                    ),
                    ("/run?wire=1", &[], "REQUEST_VALUE_PARSING_FAILED", "amount"),
                    (
                        "/run?surprise=3",
                        &[],
                        "REQUEST_VALUE_PARSING_FAILED",
                        "surprise",
                    ),
                ],
                "header" => &[
                    ("/run", &[], "REQUEST_MISSING_VALUE", "amount"),
                    ("/run", &["3", "4"], "REQUEST_TOO_MANY_VALUES", "amount"),
                    ("/run", &["bad"], "REQUEST_VALUE_PARSING_FAILED", "amount"),
                    ("/run", &["1"], "REQUEST_VALUE_PARSING_FAILED", "amount"),
                ],
                _ => &[
                    ("/bad", &[], "REQUEST_VALUE_PARSING_FAILED", "amount"),
                    ("/1", &[], "REQUEST_VALUE_PARSING_FAILED", "amount"),
                ],
            };
            for (target, headers, code, detail) in cases {
                let mut request = Request::builder()
                    .method(if durable {
                        http::Method::PUT
                    } else {
                        http::Method::GET
                    })
                    .uri(target.parse().unwrap())
                    .header("host", "example.com");
                for value in *headers {
                    request = request.header("x-wire", *value);
                }
                let response = endpoint.execute(request.finish()).await;
                if response.status() != StatusCode::BAD_REQUEST {
                    let status = response.status();
                    panic!(
                        "{durable} {binding} {target}: expected 400, got {status}: {}",
                        response.into_body().into_string().await.unwrap()
                    );
                }
                if durable {
                    let has_content_type =
                        response.headers().contains_key(http::header::CONTENT_TYPE);
                    let body = response.into_body().into_bytes().await.unwrap();
                    if has_content_type || !body.is_empty() {
                        failures.push(format!(
                            "DurableStreams {binding} {target}: expected empty 400, got {body:?}"
                        ));
                    }
                } else {
                    let body: serde_json::Value = response.into_body().into_json().await.unwrap();
                    if body["code"] != *code {
                        failures.push(format!("{binding} {target}: expected {code}, got {body}"));
                    }
                    assert!(
                        body["errors"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|error| error.as_str().unwrap().contains(detail)),
                        "{body}"
                    );
                    if map {
                        assert!(
                            body["errors"].as_array().unwrap().iter().any(|error| {
                                error.as_str().unwrap().contains("maps require name[key]")
                            }),
                            "{body}"
                        );
                    }
                }
            }
            assert!(harness.contexts.lock().unwrap().is_empty());
            assert!(harness.metadata_reads.lock().unwrap().is_empty());
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
async fn http_api_request_validation_errors_preserve_json_details_and_cors() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};

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
        vec![],
    );
    let mut route = test_route(1, "/email", Some("POST"), "typed");
    route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
    let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
        unreachable!()
    };
    call.component_id = harness.component_id;
    call.agent_type = AgentTypeName("mcp-agent".into());
    call.body = RequestBodySchema::JsonBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(SchemaType::record_from_fields([(
                "email",
                SchemaType::string(),
            )])),
        },
    };
    let handler = request_handler_with_worker(
        test_resolver(vec![route]),
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
        harness.worker_service.clone(),
    );
    let endpoint = CustomApiPoemEndpoint::new(Arc::new(handler));

    for (body, code, detail) in [
        (
            r#"{"email":"ada@example.com","additionalProp1":{}}"#,
            "REQUEST_JSON_BODY_PARSING_FAILED",
            "additionalProp1",
        ),
        (
            r#"{"email":"ada@example.com""#,
            "REQUEST_BODY_INVALID_JSON",
            "Request body was not valid json",
        ),
    ] {
        let response = endpoint
            .execute(
                Request::builder()
                    .method(http::Method::POST)
                    .uri("/email".parse().unwrap())
                    .header("host", "example.com")
                    .header("origin", "https://client.example")
                    .content_type("application/json")
                    .body(body),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-cache");
        assert!(
            response.headers()[http::header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        assert_eq!(
            response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let body: serde_json::Value = response.into_body().into_json().await.unwrap();
        assert_eq!(body["code"], code);
        assert!(
            body["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains(detail))
        );
    }
    assert!(harness.contexts.lock().unwrap().is_empty());
}

#[test]
async fn http_api_body_limit_rejects_before_invocation_and_keeps_cors() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use poem::Body;

    const LIMIT: usize = 16;
    for declared in [false, true] {
        for size in [LIMIT + 1, LIMIT] {
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
            let mut route = test_route(1, "/body", Some("POST"), "typed");
            route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
            let RouteBehaviour::CallAgent(call) = &mut route.behavior else {
                unreachable!()
            };
            call.component_id = harness.component_id;
            call.agent_type = AgentTypeName("mcp-agent".into());
            call.body = RequestBodySchema::JsonBody {
                expected: CompiledSchema {
                    graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                },
            };
            let mut handler = request_handler_with_worker(
                test_resolver(vec![route]),
                Arc::new(InitialAgentFilesService::new(Arc::new(
                    golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
                ))),
                harness.worker_service.clone(),
            );
            handler.body_limit = RequestBodyLimit::new(LIMIT);
            let endpoint = CustomApiPoemEndpoint::new(Arc::new(handler));
            let mut request = Request::builder()
                .method(http::Method::POST)
                .uri("/body".parse().unwrap())
                .header("host", "example.com")
                .header("origin", "https://client.example")
                .content_type("application/json");
            let mut body = vec![b' '; size];
            body[..2].copy_from_slice(b"{}");
            let body = if declared {
                request = request.header("content-length", size.to_string());
                Body::from(body)
            } else {
                Body::from_bytes_stream(futures::stream::iter(
                    body.chunks(4)
                        .map(|chunk| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(chunk)))
                        .collect::<Vec<_>>(),
                ))
            };
            let response = endpoint.execute(request.body(body)).await;
            assert_eq!(
                response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                "https://client.example"
            );
            if size > LIMIT {
                assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
                let body: serde_json::Value = response.into_body().into_json().await.unwrap();
                assert_eq!(body["code"], "REQUEST_PAYLOAD_TOO_LARGE");
                assert!(harness.contexts.lock().unwrap().is_empty());
            } else {
                assert_eq!(
                    response.status(),
                    StatusCode::NO_CONTENT,
                    "{}",
                    response.into_body().into_string().await.unwrap()
                );
                assert_eq!(harness.contexts.lock().unwrap().len(), 1);
            }
        }
    }
}

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

fn create_endpoint(
    harness: &InvocationHarness,
    kind: &str,
    authenticated: bool,
) -> crate::custom_api::poem_endpoint::CustomApiPoemEndpoint {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    let mut route = test_route(1, "/agents/create", Some("POST"), kind);
    if authenticated {
        route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
            header_name: "x-golem-session".into(),
        });
    }
    route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
    let RouteBehaviour::CreateAgent(create) = &mut route.behavior else {
        unreachable!()
    };
    create.agent.component_id = harness.component_id;
    create.agent.agent_type = harness.agent_type.type_name.clone();
    CustomApiPoemEndpoint::new(Arc::new(request_handler_with_worker(
        test_resolver(vec![route]),
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
        harness.worker_service.clone(),
    )))
}

fn create_request(authenticated: bool) -> Request {
    let mut request = Request::builder()
        .method(http::Method::POST)
        .uri("/agents/create".parse().unwrap())
        .header("host", "example.com")
        .header("origin", "https://client.example")
        .header("idempotency-key", "same-create-key")
        .content_type("application/json");
    if authenticated {
        request = request.header("x-golem-session", r#"{"subject":"create-user"}"#);
    }
    request.body("{}")
}

#[test]
async fn create_dispatch_rejects_array_envelope_required_to_be_object_by_openapi() {
    let body = r#"[{"kind":"record","value":{"fields":[]}},false]"#;
    let mut observed = Vec::new();
    for kind in ["create-path", "create-query"] {
        for authenticated in [false, true] {
            let harness = invocation_harness();
            let endpoint = create_endpoint(&harness, kind, authenticated);
            harness
                .creates
                .responses
                .lock()
                .unwrap()
                .push_back(Box::pin(async {
                    Ok(golem_common::model::AgentFingerprint::new())
                }));
            let mut request = create_request(authenticated);
            request.set_body(body);
            let response = endpoint.execute(request).await;
            observed.push((kind, authenticated, response.status()));
        }
    }
    assert_eq!(
        observed,
        vec![
            ("create-path", false, StatusCode::BAD_REQUEST),
            ("create-path", true, StatusCode::BAD_REQUEST),
            ("create-query", false, StatusCode::BAD_REQUEST),
            ("create-query", true, StatusCode::BAD_REQUEST),
        ],
        "OpenAPI requires an object envelope, not a positional array: {body}"
    );
}

#[test]
async fn create_dispatch_rejects_overrides_before_creating_an_agent() {
    for kind in ["create-path", "create-query"] {
        for authenticated in [false, true] {
            let harness = invocation_harness();
            let endpoint = create_endpoint(&harness, kind, authenticated);
            for (field, value) in [
                (
                    "env",
                    serde_json::json!({"PAYMENT_API_URL": "https://untrusted.example"}),
                ),
                ("env", serde_json::json!({})),
                ("env", serde_json::Value::Null),
                (
                    "config",
                    serde_json::json!([{"path": ["admin"], "value": true}]),
                ),
                ("config", serde_json::json!([])),
                ("config", serde_json::Value::Null),
                ("keyed", serde_json::json!(true)),
                ("keyed", serde_json::json!(false)),
                (
                    "parameters",
                    serde_json::json!({"kind": "record", "value": {"fields": []}}),
                ),
                ("unexpected", serde_json::json!(true)),
            ] {
                let mut body = serde_json::json!({});
                body[field] = value;
                harness
                    .creates
                    .responses
                    .lock()
                    .unwrap()
                    .push_back(Box::pin(async {
                        Ok(golem_common::model::AgentFingerprint::new())
                    }));
                let mut request = create_request(authenticated);
                request.set_body(poem::Body::from_json(&body).unwrap());
                let response = endpoint.execute(request).await;
                assert_eq!(
                    response.status(),
                    StatusCode::BAD_REQUEST,
                    "{kind}: {authenticated}: {body}"
                );
                assert!(!response.headers().contains_key(http::header::LOCATION));
                let error: serde_json::Value = response.into_body().into_json().await.unwrap();
                assert!(error.to_string().contains(field), "{error}");
                assert!(harness.creates.calls.lock().unwrap().is_empty());
            }
        }
    }
}

#[test]
async fn create_dispatch_returns_reference_and_selector_location_without_caller_scope() {
    use crate::api::agents::AgentReference;
    use golem_common::model::AgentFingerprint;
    use golem_service_base::model::auth::AuthCtx;

    for kind in ["create-path", "create-query"] {
        for authenticated in [false, true] {
            let harness = invocation_harness();
            let endpoint = create_endpoint(&harness, kind, authenticated);
            let mut ids = Vec::new();
            for index in 0..2 {
                let fingerprint = AgentFingerprint::new();
                harness
                    .creates
                    .responses
                    .lock()
                    .unwrap()
                    .push_back(Box::pin(async move { Ok(fingerprint) }));
                let response = endpoint.execute(create_request(authenticated)).await;
                assert_eq!(
                    response.status(),
                    StatusCode::CREATED,
                    "{kind}: {authenticated}"
                );
                assert_eq!(
                    response.headers()[http::header::CONTENT_TYPE],
                    "application/json"
                );
                assert_eq!(
                    response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                    "https://client.example"
                );
                assert!(
                    response.headers()[http::header::ACCESS_CONTROL_EXPOSE_HEADERS]
                        .to_str()
                        .unwrap()
                        .split(',')
                        .any(|name| name.trim().eq_ignore_ascii_case("location"))
                );
                let location = response.headers()[http::header::LOCATION]
                    .to_str()
                    .unwrap()
                    .to_owned();
                let reference: AgentReference = response.into_body().into_json().await.unwrap();
                assert_eq!(reference.agent_fingerprint, fingerprint);
                assert_eq!(reference.agent_id.type_tag(), harness.type_id.get());
                let expected = if kind == "create-path" {
                    format!("/agents/{}", reference.agent_id)
                } else {
                    format!("/agents?instance={}", reference.agent_id)
                };
                assert_eq!(location, expected);
                let calls = harness.creates.calls.lock().unwrap();
                assert_eq!(calls.len(), index + 1);
                let call = &calls[index];
                assert_eq!(call.target.agent_id, reference.agent_id);
                assert_eq!(call.environment_id, harness.environment_id);
                assert!(matches!(call.auth, AuthCtx::System));
                if authenticated {
                    use golem_common::model::caller::{Pair, Principal};
                    let Principal::User(user) = &call.caller.original else {
                        panic!("an authenticated constructor must receive its session user");
                    };
                    assert_eq!(user.pair, Pair::new("http://test-idp.com", "create-user"));
                    assert!(call.caller.original.equals(&call.caller.immediate));
                } else {
                    assert_eq!(
                        call.caller,
                        golem_common::model::caller::Caller::anonymous()
                    );
                }
                assert!(call.auth.idempotency_scope().is_none());
                assert!(!call.ignore_already_existing);
                assert!(call.environment_variables.is_empty());
                assert!(call.config.is_empty());
                assert!(call.idempotency_key.is_some());
                let (target, constructor, _) = call.target.invoke_fields();
                assert!(matches!(
                    target.target,
                    Some(golem_api_grpc::proto::golem::worker::agent_target::Target::Id(_))
                ));
                assert!(
                    constructor.is_some(),
                    "an unkeyed create carries its constructor"
                );
                ids.push(reference.agent_id);
            }
            assert_ne!(
                ids[0], ids[1],
                "an authenticated HTTP session is not an idempotency scope"
            );
            let calls = harness.creates.calls.lock().unwrap();
            assert_ne!(calls[0].idempotency_key, calls[1].idempotency_key);
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn create_dispatch_waits_for_worker_completion_and_preserves_worker_failure() {
    use crate::api::agents::AgentReference;
    use crate::service::worker::WorkerServiceError;
    use golem_common::model::AgentFingerprint;

    for kind in ["create-path", "create-query"] {
        for success in [false, true] {
            let harness = invocation_harness();
            let endpoint = Arc::new(create_endpoint(&harness, kind, true));
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (complete_tx, complete_rx) = tokio::sync::oneshot::channel();
            harness
                .creates
                .responses
                .lock()
                .unwrap()
                .push_back(Box::pin(async move {
                    entered_tx.send(()).unwrap();
                    complete_rx.await.unwrap()
                }));
            let pending = tokio::spawn(async move { endpoint.execute(create_request(true)).await });
            entered_rx.await.unwrap();
            assert_eq!(harness.creates.calls.lock().unwrap().len(), 1);
            assert!(
                !pending.is_finished(),
                "201 must await worker create completion"
            );
            let fingerprint = AgentFingerprint::new();
            complete_tx
                .send(if success {
                    Ok(fingerprint)
                } else {
                    Err(WorkerServiceError::TypeChecker(
                        "worker refused create".into(),
                    ))
                })
                .unwrap();
            let response = pending.await.unwrap();
            assert_eq!(
                response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                "https://client.example"
            );
            if success {
                assert_eq!(response.status(), StatusCode::CREATED);
                assert!(response.headers().contains_key(http::header::LOCATION));
                let reference: AgentReference = response.into_body().into_json().await.unwrap();
                assert_eq!(reference.agent_fingerprint, fingerprint);
                assert_eq!(
                    reference.agent_id,
                    harness.creates.calls.lock().unwrap()[0].target.agent_id
                );
            } else {
                assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
                assert!(!response.headers().contains_key(http::header::LOCATION));
                let body: serde_json::Value = response.into_body().into_json().await.unwrap();
                assert_eq!(body["code"], "INTERNAL_AGENT_EXECUTION_FAILED");
                assert!(body.to_string().contains("worker refused create"));
            }
        }
    }
}

#[test]
async fn oidc_create_dispatch_does_not_turn_a_session_into_an_idempotency_scope() {
    use crate::api::agents::AgentReference;
    use crate::custom_api::model::OidcSession;
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::test_route;
    use golem_common::model::AgentFingerprint;
    use golem_common::model::caller::{Pair, Principal};
    use golem_common::model::security_scheme::{
        Provider, SecuritySchemeId, SecuritySchemeLogin, SecuritySchemeName, SecuritySchemeRevision,
    };
    use golem_service_base::custom_api::{SecuritySchemeDetails, SecuritySchemeRouteSecurity};

    struct Lookup(std::sync::Mutex<Option<CompiledRoutes>>);
    #[async_trait]
    impl HttpApiDefinitionsLookup for Lookup {
        async fn get(
            &self,
            _: &golem_common::model::domain_registration::Domain,
        ) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
            Ok(self.0.lock().unwrap().take().unwrap())
        }
    }

    for kind in ["create-path", "create-query"] {
        let harness = invocation_harness();
        let (store, _db) =
            crate::custom_api::oidc::session_store::sqlite_test_session_store().await;
        let session_id = SessionId(uuid::Uuid::new_v4());
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(1);
        store
            .store_authenticated_session(
                &session_id,
                OidcSession {
                    subject: "create-user".into(),
                    issuer: "https://idp.example".into(),
                    email: None,
                    name: None,
                    email_verified: None,
                    given_name: None,
                    family_name: None,
                    picture: None,
                    preferred_username: None,
                    claims: openidconnect::IdTokenClaims::new(
                        openidconnect::IssuerUrl::new("https://idp.example".into()).unwrap(),
                        vec![],
                        expires_at,
                        chrono::Utc::now(),
                        openidconnect::StandardClaims::new(openidconnect::SubjectIdentifier::new(
                            "create-user".into(),
                        )),
                        openidconnect::EmptyAdditionalClaims {},
                    ),
                    scopes: Default::default(),
                    expires_at,
                },
            )
            .await
            .unwrap();
        let scheme_id = SecuritySchemeId::new();
        let scheme = SecuritySchemeDetails {
            id: scheme_id,
            revision: SecuritySchemeRevision::INITIAL,
            name: SecuritySchemeName("create-login".into()),
            provider_type: Provider::Google(Empty {}),
            client_id: openidconnect::ClientId::new("test".into()),
            client_secret: openidconnect::ClientSecret::new("test".into()),
            redirect_url: openidconnect::RedirectUrl::new("https://example.com/callback".into())
                .unwrap(),
            scopes: vec![],
            login: SecuritySchemeLogin::Cookie(Empty {}),
        };
        let mut route = test_route(1, "/agents/create", Some("POST"), kind);
        route.security = RouteSecurity::SecurityScheme(SecuritySchemeRouteSecurity {
            security_scheme_id: scheme_id,
        });
        let RouteBehaviour::CreateAgent(create) = &mut route.behavior else {
            unreachable!()
        };
        create.agent.component_id = harness.component_id;
        create.agent.agent_type = harness.agent_type.type_name.clone();
        let resolver = RouteResolver::new(
            &RouteResolverConfig::default(),
            Arc::new(Lookup(std::sync::Mutex::new(Some(CompiledRoutes {
                account_id: harness.account_id,
                account_email: harness.account_email.clone(),
                environment_id: harness.environment_id,
                deployment_revision: DeploymentRevision::INITIAL,
                security_schemes: HashMap::from([(scheme_id, scheme)]),
                routes: vec![route],
            })))),
        );
        let mut handler = request_handler_with_worker(
            resolver,
            Arc::new(InitialAgentFilesService::new(Arc::new(
                golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
            ))),
            harness.worker_service.clone(),
        );
        handler.oidc_handler = Arc::new(OidcHandler::new(
            Arc::new(store),
            Arc::new(DefaultIdentityProvider),
        ));
        let endpoint = CustomApiPoemEndpoint::new(Arc::new(handler));
        let mut ids = Vec::new();
        for _ in 0..2 {
            harness
                .creates
                .responses
                .lock()
                .unwrap()
                .push_back(Box::pin(async { Ok(AgentFingerprint::new()) }));
            let mut request = create_request(false);
            request.headers_mut().insert(
                http::header::COOKIE,
                format!("golem_session_id={}", session_id.0)
                    .parse()
                    .unwrap(),
            );
            let response = endpoint.execute(request).await;
            assert_eq!(response.status(), StatusCode::CREATED);
            let reference: AgentReference = response.into_body().into_json().await.unwrap();
            ids.push(reference.agent_id);
        }
        assert_ne!(ids[0], ids[1]);
        let calls = harness.creates.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(
            calls
                .iter()
                .all(|call| call.auth.is_system() && call.auth.idempotency_scope().is_none())
        );
        for call in calls.iter() {
            let Principal::User(user) = &call.caller.original else {
                panic!("an authenticated HTTP constructor must receive its session user");
            };
            assert_eq!(user.pair, Pair::new("https://idp.example", "create-user"));
            assert!(call.caller.original.equals(&call.caller.immediate));
        }
        assert_ne!(calls[0].idempotency_key, calls[1].idempotency_key);
    }
}

#[test]
async fn create_dispatch_inherits_auth_and_cors_and_does_not_require_an_incoming_id() {
    for kind in ["create-path", "create-query"] {
        let harness = invocation_harness();
        let endpoint = create_endpoint(&harness, kind, true);
        for (authorized, body, status, code) in [
            (false, "not json", StatusCode::UNAUTHORIZED, None),
            (
                true,
                "not json",
                StatusCode::BAD_REQUEST,
                Some("REQUEST_BODY_INVALID_JSON"),
            ),
            // The route's constructor declares no parameters, so a field it does not declare is
            // rejected before the create service is called.
            (
                true,
                r#"{"name":"bad"}"#,
                StatusCode::BAD_REQUEST,
                Some("REQUEST_JSON_BODY_PARSING_FAILED"),
            ),
        ] {
            let mut request = Request::builder()
                .method(http::Method::POST)
                .uri("/agents/create".parse().unwrap())
                .header("host", "example.com")
                .header("origin", "https://client.example")
                .content_type("application/json");
            if authorized {
                request = request.header("x-golem-session", "{}");
            }
            let response = endpoint.execute(request.body(body)).await;
            assert_eq!(response.status(), status, "{kind}: {body}");
            assert_eq!(
                response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                "https://client.example"
            );
            assert!(!response.headers().contains_key(http::header::LOCATION));
            if let Some(code) = code {
                let body: serde_json::Value = response.into_body().into_json().await.unwrap();
                assert_eq!(body["code"], code, "{kind}: {body}");
            }
        }
        for (headers, expected) in [
            ("content-type,idempotency-key", StatusCode::NO_CONTENT),
            ("content-type,x-unknown", StatusCode::FORBIDDEN),
        ] {
            let response = endpoint
                .execute(
                    Request::builder()
                        .method(http::Method::OPTIONS)
                        .uri("/agents/create".parse().unwrap())
                        .header("host", "example.com")
                        .header("origin", "https://client.example")
                        .header(http::header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                        .header(http::header::ACCESS_CONTROL_REQUEST_HEADERS, headers)
                        .finish(),
                )
                .await;
            assert_eq!(response.status(), expected);
            if expected == StatusCode::NO_CONTENT {
                assert_eq!(
                    response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
                    "https://client.example"
                );
                let allowed = response.headers()[http::header::ACCESS_CONTROL_ALLOW_HEADERS]
                    .to_str()
                    .unwrap();
                assert!(allowed.split(',').any(|name| name.trim() == "content-type"));
                assert!(
                    allowed
                        .split(',')
                        .any(|name| name.trim() == "idempotency-key")
                );
            }
        }
        assert!(harness.targets.lock().unwrap().is_empty());
        assert!(harness.contexts.lock().unwrap().is_empty());
        assert!(harness.creates.calls.lock().unwrap().is_empty());
    }
}

#[test]
async fn agent_selector_runs_after_auth_and_before_etag_revalidation() {
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::AgentMetadataDto;
    use golem_common::model::agent::{CachePolicy, OwnerKind, ReadOnlyConfig};
    use golem_common::model::{
        AgentFingerprint, AgentId, AgentStatus, OplogIndex, OwnedAgentId, Timestamp,
    };
    use golem_service_base::custom_api::AgentSelection;
    use poem::IntoResponse;
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
    call.agent_selection = AgentSelection::Query {
        name: "instance".into(),
    };
    call.read_only = Some(ReadOnlyConfig {
        cache_policy: CachePolicy::UntilWrite(Empty {}),
    });
    let bound_call = call.clone();
    let keyed = AgentId::keyed(
        harness.environment_id,
        harness.type_id,
        &golem_common::model::agent::AgentKey::new(
            AgentTypeName("mcp-agent".into()),
            golem_common::schema::TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(vec![])),
                golem_common::schema::SchemaValue::Record { fields: vec![] },
            ),
        )
        .unwrap(),
    );
    let selected_p = AgentId::from_parts(harness.type_id.get(), 1);
    let selected_q = AgentId::from_parts(harness.type_id.get(), 2);
    let other_type = AgentId::from_parts(harness.type_id.get() + 1, 1);
    let p = selected_p.to_string();
    let q = selected_q.to_string();
    let mut ids = vec![];
    for id in [keyed, selected_p, selected_q] {
        harness.metadata.lock().unwrap().insert(
            id,
            AgentMetadataDto {
                agent_id: id,
                owner_kind: OwnerKind::ComponentAgent,
                agent_type_name: Some(AgentTypeName("mcp-agent".into())),
                canonical_key: None,
                origin: golem_common::model::agent::AgentOrigin::Created,
                constructor_parameters: None,
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
                underlying_status: None,
            },
        );
        ids.push(id);
    }
    let mut routes = vec![route];
    for (id, path, selection) in [
        (
            2,
            "/selected-path/{instance}",
            AgentSelection::Path { index: 0.into() },
        ),
        (3, "/keyed", AgentSelection::Key),
        (
            4,
            "/required",
            AgentSelection::Query {
                name: "instance".into(),
            },
        ),
    ] {
        let mut route = test_route(id, path, Some("GET"), "typed");
        let mut call = bound_call.clone();
        call.agent_selection = selection;
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
    // The session of the authorized requests. Its times are fixed, so every request has the same
    // caller: a session without them is a new sign-in on each request.
    const SESSION: &str =
        r#"{"issued_at":"2026-01-01T00:00:00Z","expires_at":"2100-01-01T00:00:00Z"}"#;
    // The ETag names the caller, so take the caller of that session as the handler derives it:
    // the session middleware, then the caller of the request.
    let session_tag = {
        let mut session_route = test_route(9, "/session", Some("GET"), "typed");
        session_route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
            header_name: "x-golem-session".into(),
        });
        let resolver = test_resolver(vec![session_route]);
        let request = Request::builder()
            .uri("/session".parse().unwrap())
            .header("host", "example.com")
            .header("x-golem-session", SESSION)
            .finish();
        let selected = resolver.resolve_matching_route(&request).await.unwrap();
        let mut request = crate::custom_api::RichRequest::new(request);
        assert!(
            crate::custom_api::session_from_header_security::apply_session_from_header_security_middleware(
                &mut request,
                &selected,
            )
            .unwrap()
            .is_none()
        );
        let caller = crate::custom_api::call_agent::caller_from_request(&request).unwrap();
        crate::custom_api::call_agent::cache_headers::caller_etag_tag(&caller)
    };
    let etag = |id: &AgentId| {
        let route_agent = OwnedAgentId::new(EnvironmentId(uuid::Uuid::nil()), id);
        format!(
            "\"{}/{}/{fingerprint}/{session_tag}:17\"",
            route_agent.environment_id, route_agent.agent_id
        )
    };
    for (query, authorized, expected) in [
        ("v=3".into(), false, StatusCode::UNAUTHORIZED),
        ("v=3".into(), true, StatusCode::BAD_REQUEST),
        (format!("instance={p}&v=3"), true, StatusCode::BAD_REQUEST),
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
        ("".into(), true, StatusCode::BAD_REQUEST),
        (
            format!("instance={other_type}"),
            true,
            StatusCode::NOT_FOUND,
        ),
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
            request = request.header("x-golem-session", SESSION);
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
            assert_eq!(*reads, vec![ids[usize::from(!query.is_empty())]]);
        } else {
            assert!(reads.is_empty());
        }
    }
    for (uri, selected, expected) in [
        (format!("/selected-path/{p}"), 1, StatusCode::NOT_MODIFIED),
        ("/selected-path/invalid".into(), 1, StatusCode::BAD_REQUEST),
        ("/selected-path".into(), 1, StatusCode::NOT_FOUND),
        ("/keyed".into(), 0, StatusCode::NOT_MODIFIED),
        ("/required".into(), 1, StatusCode::BAD_REQUEST),
    ] {
        harness.metadata_reads.lock().unwrap().clear();
        let request = Request::builder()
            .method(http::Method::GET)
            .uri(uri.parse().unwrap())
            .header("host", "example.com")
            .header("x-golem-session", SESSION)
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
            assert_eq!(*reads, vec![ids[selected]]);
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
            .header("x-golem-session", SESSION)
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
#[golem_async_trait::region_simple]
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
            family_id: 1,
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
        component_id: DURABLE_STREAM_COMPONENT,
        component_revision: ComponentRevision::INITIAL,
        agent_type: AgentTypeName("oracle-agent".to_string()),
        agent_mode: AgentMode::Durable,
        constructor_input: CompiledInputSchema {
            graph: SchemaGraph::empty(),
            input_schema: InputSchema::Parameters(vec![]),
        },
        constructor_parameters: vec![],
        agent_selection: golem_service_base::custom_api::AgentSelection::Key,
        method_name: "stream".to_string(),
        method_input: compiled_input(InputSchema::parameters([
            golem_common::schema::NamedField::user_supplied(
                "input",
                SchemaType::stream(Some(SchemaType::u8())),
            ),
        ])),
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
    let route_family = |prefix: &str, route_id_offset: i32, mut behaviour: CallAgentBehaviour| {
        behaviour.durable_streams.as_mut().unwrap().family_id = route_id_offset + 1;
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
                HttpMethod::Method(golem_schema::http::HttpMethod::Post(Default::default())),
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
                HttpMethod::Method(golem_schema::http::HttpMethod::Get(Default::default())),
                slot_path.clone(),
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 3,
                HttpMethod::Method(golem_schema::http::HttpMethod::Post(Default::default())),
                slot_path.clone(),
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 4,
                HttpMethod::Method(golem_schema::http::HttpMethod::Delete(Default::default())),
                slot_path,
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 5,
                HttpMethod::Method(golem_schema::http::HttpMethod::Delete(Default::default())),
                session_path,
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 6,
                HttpMethod::Method(golem_schema::http::HttpMethod::Get(Default::default())),
                fork_slot_path.clone(),
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 7,
                HttpMethod::Method(golem_schema::http::HttpMethod::Put(Default::default())),
                fork_slot_path.clone(),
                RequestBodySchema::Unused,
                behaviour.clone(),
            ),
            route(
                route_id_offset + 8,
                HttpMethod::Method(golem_schema::http::HttpMethod::Delete(Default::default())),
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

/// The component that implements `oracle-agent` for the routes of [`durable_stream_routes`].
const DURABLE_STREAM_COMPONENT: ComponentId = ComponentId(uuid::Uuid::from_u128(0xd5));

/// A harness whose component `component_id` implements the durable agent type
/// `agent_type`, without constructor parameters, in the nil environment of the test routes.
fn route_harness(
    agent_type: &str,
    component_id: ComponentId,
    methods: Vec<golem_common::schema::AgentMethodSchema>,
) -> InvocationHarness {
    InvocationHarness::with_agent_type(
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
        golem_common::schema::AgentTypeSchema {
            kind: golem_common::schema::agent::AgentTypeKind::Regular,
            type_name: AgentTypeName(agent_type.to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            methods,
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: golem_common::model::agent::Snapshotting::Disabled(Empty {}),
            config: vec![],
        },
        component_id,
        EnvironmentId(uuid::Uuid::nil()),
    )
}

/// A harness that implements the `oracle-agent` of [`durable_stream_routes`].
fn invocation_harness() -> InvocationHarness {
    route_harness(
        "oracle-agent",
        DURABLE_STREAM_COMPONENT,
        vec![golem_common::schema::AgentMethodSchema {
            name: "stream".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        }],
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
        crate::config::RequestBodyLimits::default().http_api,
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
async fn early_rejections_are_not_fresh_and_do_not_authenticate() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    let endpoint = CustomApiPoemEndpoint::new(Arc::new(request_handler()));
    for (method, path, expect, status) in [
        ("CONNECT", "/stream", "100-continue", 501),
        ("TRACE", "/stream", "100-continue", 501),
        ("GET", "/stream", "unsupported", 417),
        ("GET", "/%2funsafe", "100-continue", 400),
        ("GET", "/missing", "100-continue", 404),
    ] {
        let response = endpoint
            .execute(
                Request::builder()
                    .method(method.parse().unwrap())
                    .uri(format!("http://ds.example{path}").parse().unwrap())
                    .header("host", "ds.example")
                    .header("expect", expect)
                    .header("x-golem-session", "invalid session must not be parsed")
                    .finish(),
            )
            .await;
        assert_eq!(response.status().as_u16(), status, "{method} {path}");
        assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-cache");
    }
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
        Some(
            &"Origin, Authorization, Cookie, Golem-Token, x-golem-session"
                .parse()
                .unwrap()
        )
    );
    assert_eq!(
        response.headers()[http::header::CACHE_CONTROL],
        "private, no-store"
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
async fn invalid_lazy_appends_do_not_create_sessions() {
    use crate::custom_api::route_resolver::tests::test_resolver;

    let cases = vec![
        (
            vec![("producer-id", "p")],
            Some("application/json"),
            "1".into(),
            StatusCode::BAD_REQUEST,
        ),
        (
            vec![
                ("producer-id", "p"),
                ("producer-epoch", "x"),
                ("producer-seq", "0"),
            ],
            Some("application/json"),
            "1".into(),
            StatusCode::BAD_REQUEST,
        ),
        (
            vec![
                ("producer-id", "p"),
                ("producer-id", "q"),
                ("producer-epoch", "0"),
                ("producer-seq", "0"),
            ],
            Some("application/json"),
            "1".into(),
            StatusCode::BAD_REQUEST,
        ),
        (vec![], None, "1".into(), StatusCode::BAD_REQUEST),
        (vec![], Some("text/plain"), "1".into(), StatusCode::CONFLICT),
        (
            vec![],
            Some("application/json"),
            r#""not-a-number""#.into(),
            StatusCode::BAD_REQUEST,
        ),
        (
            vec![],
            Some("application/json"),
            r#"[7,"not-a-number",19]"#.into(),
            StatusCode::BAD_REQUEST,
        ),
        (
            vec![],
            Some("application/json"),
            "[".into(),
            StatusCode::BAD_REQUEST,
        ),
        (
            vec![],
            Some("application/json"),
            "[]".into(),
            StatusCode::BAD_REQUEST,
        ),
        (
            vec![],
            Some("application/json"),
            String::new(),
            StatusCode::BAD_REQUEST,
        ),
        (
            vec![],
            Some("application/json"),
            format!("[{}]", vec!["1"; 4097].join(",")),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            vec![],
            Some("application/json"),
            " ".repeat(1024 * 1024 + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ];
    for (headers, content_type, body, expected) in cases {
        let harness = invocation_harness();
        let mut routes = durable_stream_routes().routes;
        for route in &mut routes {
            if let RouteBehaviour::CallAgent(call) = &mut route.behavior {
                call.method_input = compiled_input(InputSchema::parameters([
                    golem_common::schema::NamedField::user_supplied(
                        "input",
                        SchemaType::stream(Some(SchemaType::u32())),
                    ),
                ]));
                for slot in &mut call.durable_streams.as_mut().unwrap().slots {
                    if slot.public_name == "requests" {
                        slot.content_type = "application/json".into();
                        slot.representation = DurableStreamRepresentation::Json;
                    }
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
        harness.script_durable_stream_reads(vec![None]);
        let session =
            golem_common::model::invocation_session_public::new_durable_stream_session_id();
        let mut builder = Request::builder()
            .method(http::Method::POST)
            .uri(
                format!("http://example.com/writable/invocations/{session}/streams/requests")
                    .parse()
                    .unwrap(),
            )
            .header(http::header::HOST, "example.com")
            .header("x-golem-session", "{}");
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        if let Some(content_type) = content_type {
            builder = builder.content_type(content_type);
        }
        let response = handler.handle_request(builder.body(body)).await.unwrap();
        assert_eq!(response.status(), expected);
        let log = harness.native_sessions.lock().unwrap();
        assert!(log.starts.is_empty());
        assert!(log.appends.is_empty());
    }
}

#[test]
async fn valid_lazy_append_creates_once_and_preserves_body() {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateStreamSessionSuccess, ReadStreamSlotSuccess, append_to_stream_slot_request::Payload,
    };

    let (handler, harness) = request_handler_and_harness();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    harness
        .native_sessions
        .lock()
        .unwrap()
        .public_session_creation = Some(CreateStreamSessionSuccess {
        session: session.clone(),
        ..Default::default()
    });
    harness.script_durable_stream_reads(vec![
        None,
        None,
        Some(ReadStreamSlotSuccess {
            writable: true,
            element_schema: Some(SchemaGraph::anonymous(SchemaType::u8()).into()),
            ..Default::default()
        }),
    ]);
    let response = handler
        .handle_request(
            Request::builder()
                .method(http::Method::POST)
                .uri(
                    format!("http://ds.example/writable/invocations/{session}/streams/requests")
                        .parse()
                        .unwrap(),
                )
                .header(http::header::HOST, "ds.example")
                .header("x-golem-session", "{}")
                .content_type("application/vnd.golem.fragment")
                .body(vec![7u8, 19, 41]),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let log = harness.native_sessions.lock().unwrap();
    assert_eq!(log.starts.len(), 1);
    assert_eq!(log.appends.len(), 1);
    assert_eq!(
        log.appends[0].payload,
        Some(Payload::PackedU8(vec![7, 19, 41]))
    );
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
    let (environment, revision, route): (String, String, String) =
        serde_json::from_str(&control.route_scope).unwrap();
    let expected_environment: EnvironmentId = controls[0]
        .producer_environment_id
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(environment, expected_environment.to_string());
    assert_eq!(revision, DeploymentRevision::INITIAL.to_string());
    assert_eq!(route, "11");

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
    use golem_common::model::AgentId;
    use golem_common::model::OplogIndex;
    use golem_common::model::durable_stream::StreamOffset;
    use golem_service_base::custom_api::AgentSelection;
    let selected_p = AgentId::from_parts(42, 1);
    let selected_q = AgentId::from_parts(42, 2);
    let p_text = selected_p.to_string();
    let q_text = selected_q.to_string();
    let p = p_text.as_str();
    let q = q_text.as_str();
    let session = golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let target_fork =
        golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let source_fork =
        golem_common::model::invocation_session_public::new_durable_stream_session_id();
    let environment_id = EnvironmentId(uuid::Uuid::nil());
    let named_fork =
        |root: AgentId, name: &str| AgentId::durable_stream_fork(environment_id, root, name);
    for path_selector in [false, true] {
        for root in [p, q] {
            for nested in [false, true] {
                let harness = invocation_harness();
                assert_eq!(harness.type_id.get(), 42);
                let mut routes = durable_stream_routes().routes;
                for route in &mut routes {
                    if let RouteBehaviour::CallAgent(call) = &mut route.behavior {
                        call.agent_selection = if path_selector {
                            AgentSelection::Path { index: 0.into() }
                        } else {
                            AgentSelection::Query {
                                name: "instance".into(),
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
                    format!("/writable/{root}")
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
                    target.replace(root, &root.to_uppercase())
                } else {
                    format!("{target}?instance={}", root.to_uppercase())
                };
                let mut invalid_sources = vec![
                    format!("{source}?instance={p}"),
                    format!("{source}#fragment"),
                ];
                if path_selector {
                    let other = if root == p { q } else { p };
                    invalid_sources.push(source.replace(root, other));
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
                    format!("{target}?instance={root}")
                };
                assert_eq!(response.headers()[http::header::LOCATION], expected_url);
                let forks = harness.recorded_durable_stream_forks();
                assert_eq!(forks.len(), 1);
                let root_id = root.parse::<AgentId>().unwrap();
                let source_agent_id = AgentId::try_from(forks[0].source_agent_id.unwrap()).unwrap();
                let target_agent_id = AgentId::try_from(forks[0].target_agent_id.unwrap()).unwrap();
                assert_eq!(
                    source_agent_id,
                    if nested {
                        named_fork(root_id, &source_fork)
                    } else {
                        root_id
                    }
                );
                assert_eq!(target_agent_id, named_fork(root_id, &target_fork));
                assert_eq!(target_agent_id.type_tag(), root_id.type_tag());
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

    let response = handler
        .handle_request(
            Request::builder()
                .method(http::Method::DELETE)
                .uri(format!("http://ds.example{fork_path}").parse().unwrap())
                .header(http::header::HOST, "ds.example")
                .header("x-golem-session", "{}")
                .finish(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let controls = harness.recorded_durable_stream_controls();
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0].producer_agent_id, forks[0].target_agent_id);
    let control = controls[0].export_control.as_ref().unwrap();
    assert_eq!(control.route_scope, forks[0].route_scope);
    assert_eq!(control.session, session);
    assert_eq!(control.slot.as_deref(), Some("input"));
}

#[test]
async fn immutable_file_corpus_through_request_handler() {
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_schema::http::FileMapping;
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
        let RouteBehaviour::Mount(router) = &mut route.behavior else {
            unreachable!()
        };
        router.file_bindings = FileMapping::compile_list([("/*", "/$1")]).unwrap();
        let MountFileSource::Router { agent, file_index } = &mut router.files else {
            unreachable!()
        };
        let mut provisions = vec![];
        if input["body_hex"].is_string() {
            provisions.push((decode(&input["body_hex"]), true));
        } else if let Some(revisions) = input["revisions"].as_object() {
            agent.component_revision =
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
                file_index.push(RouterFileIndexEntry {
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
    use golem_schema::http::FileMapping;
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
        let RouteBehaviour::Mount(router) = &mut route.behavior else {
            unreachable!()
        };
        router.file_bindings = FileMapping::compile_list([("/asset", "/private/a.txt")]).unwrap();
        let MountFileSource::Router { file_index, .. } = &mut router.files else {
            unreachable!()
        };
        file_index.push(RouterFileIndexEntry {
            path: "/private/a.txt".into(),
            blob_key: key,
            size: 7,
        });
        let handler = Arc::new(request_handler_with(test_resolver(vec![route]), files));
        let endpoint = CustomApiPoemEndpoint::new(handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
        let _server = AbortOnDropHandle::new(tokio::spawn(golem_service_base::http_server::run(
            acceptor,
            endpoint,
            Default::default(),
        )));
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
        let RouteBehaviour::Mount(behavior) = &mut router.behavior else {
            unreachable!()
        };
        behavior.openapi_provider = Some(MountTarget::AgentMethod(MountAgentMethod {
            agent: behavior.files.agent().clone(),
            method: golem_service_base::custom_api::RouterMethod {
                method_name: "describe".into(),
                input: CompiledInputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                    input_schema: InputSchema::Parameters(vec![]),
                },
                output: CompiledOutputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::string()),
                    output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
                },
            },
        }));
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
    use golem_common::model::filesystem::FileReadHead;
    use golem_schema::http::FileMapping;
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
    let RouteBehaviour::Mount(router) = &mut routes[1].behavior else {
        unreachable!()
    };
    router.file_bindings = FileMapping::compile_list([("/static", "/asset")]).unwrap();
    let MountFileSource::Router { file_index, .. } = &mut router.files else {
        unreachable!()
    };
    file_index.push(RouterFileIndexEntry {
        path: "/asset".into(),
        blob_key: key,
        size: 11,
    });
    let mut live = test_route(2, "/live", None, "filesystem");
    let RouteBehaviour::Mount(filesystem) = &mut live.behavior else {
        unreachable!()
    };
    filesystem.file_bindings = FileMapping::compile_list([("/*", "/$1")]).unwrap();
    routes.push(live);
    let harness = route_harness("agent2", ComponentId(uuid::Uuid::nil()), vec![]);
    let key = golem_common::model::agent::AgentKey::new(
        AgentTypeName("agent2".into()),
        golem_common::schema::TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(vec![])),
            golem_common::schema::SchemaValue::Record { fields: vec![] },
        ),
    )
    .unwrap();
    harness.add_keyed_agent(&key);
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
