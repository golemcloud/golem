// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

//! Regression tests for the OpenAPI 3.1 emitter, one per protocol-policy
//! mapping (issue #3398). Routes are built directly from the compiled-route
//! types so the tests exercise the boundary adapter + emitter end-to-end
//! without a running environment.

use super::HttpApiOpenApiSpec;
use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour, RichRouteSecurity};
use golem_common::base_model::agent::{BinaryType, TextType};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::metadata::{MetadataEnvelope, TypeId};
use golem_common::schema::schema_type::{
    BinaryRestrictions, NamedFieldType, ResultSpec, TextRestrictions,
};
use golem_common::schema::unstructured::{
    unstructured_binary_schema_type, unstructured_text_schema_type,
};
use golem_common::schema::{
    InputSchema, OutputSchema, Role, SchemaGraph, SchemaType, SchemaTypeDef,
};
use golem_service_base::custom_api::{
    CallAgentBehaviour, CompiledInputSchema, CompiledOutputSchema, CompiledSchema, CorsOptions,
    CorsPreflightBehaviour, MethodParameter, OpenApiSpecBehaviour, OpenApiSpecFormat, PathSegment,
    PathSegmentType, QueryOrHeaderType, RequestBodySchema, WebhookCallbackBehaviour,
};
use golem_service_base::model::SafeIndex;
use http::Method;
use serde_json::{Value, json};
use test_r::test;

// --------------------------------------------------------------------------
// Route construction helpers
// --------------------------------------------------------------------------

fn agent_type_name(name: &str) -> golem_common::model::agent::AgentTypeName {
    golem_common::model::agent::AgentTypeName(name.to_string())
}

/// A single response carrying a component-model type. Named composites become
/// `defs` + a `Ref` root, so the emitter renders them via `$ref`.
fn cm_response(ty: SchemaType) -> CompiledOutputSchema {
    let graph = SchemaGraph::anonymous(ty);
    cm_response_graph(graph)
}

fn cm_response_graph(graph: SchemaGraph) -> CompiledOutputSchema {
    let root = graph.root.clone();
    CompiledOutputSchema {
        graph,
        output_schema: OutputSchema::Single(Box::new(root)),
    }
}

fn field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

fn record(fields: Vec<NamedFieldType>) -> SchemaType {
    SchemaType::record(fields)
}

fn str() -> SchemaType {
    SchemaType::string()
}

fn option(inner: SchemaType) -> SchemaType {
    SchemaType::option(inner)
}

fn result(ok: SchemaType, err: SchemaType) -> SchemaType {
    SchemaType::result(ResultSpec {
        ok: Some(Box::new(ok)),
        err: Some(Box::new(err)),
    })
}

fn result_ok(ok: SchemaType) -> SchemaType {
    SchemaType::result(ResultSpec {
        ok: Some(Box::new(ok)),
        err: None,
    })
}

fn result_err(err: SchemaType) -> SchemaType {
    SchemaType::result(ResultSpec {
        ok: None,
        err: Some(Box::new(err)),
    })
}

fn unit_response() -> CompiledOutputSchema {
    CompiledOutputSchema {
        graph: SchemaGraph::empty(),
        output_schema: OutputSchema::Unit,
    }
}

fn text_response(restrictions: Option<Vec<TextType>>) -> CompiledOutputSchema {
    let languages = restrictions.map(|r| r.into_iter().map(|t| t.language_code).collect());
    let ty = SchemaType::text(TextRestrictions {
        languages,
        ..Default::default()
    });
    CompiledOutputSchema {
        graph: SchemaGraph::anonymous(ty.clone()),
        output_schema: OutputSchema::Single(Box::new(ty)),
    }
}

fn binary_response(restrictions: Option<Vec<BinaryType>>) -> CompiledOutputSchema {
    let mime_types = restrictions.map(|r| r.into_iter().map(|t| t.mime_type).collect());
    let ty = SchemaType::binary(BinaryRestrictions {
        mime_types,
        ..Default::default()
    });
    CompiledOutputSchema {
        graph: SchemaGraph::anonymous(ty.clone()),
        output_schema: OutputSchema::Single(Box::new(ty)),
    }
}

/// Like [`text_response`], but the output schema is the canonical
/// unstructured-text `variant { inline, url }` wrapper (which the guest SDKs
/// publish) rather than a bare `Text` rich scalar.
fn wrapper_text_response(restrictions: Option<Vec<TextType>>) -> CompiledOutputSchema {
    let languages = restrictions.map(|r| r.into_iter().map(|t| t.language_code).collect());
    let ty = unstructured_text_schema_type(TextRestrictions {
        languages,
        ..Default::default()
    });
    CompiledOutputSchema {
        graph: SchemaGraph::anonymous(ty.clone()),
        output_schema: OutputSchema::Single(Box::new(ty)),
    }
}

/// Like [`binary_response`], but the output schema is the canonical
/// unstructured-binary `variant { inline, url }` wrapper.
fn wrapper_binary_response(restrictions: Option<Vec<BinaryType>>) -> CompiledOutputSchema {
    let mime_types = restrictions.map(|r| r.into_iter().map(|t| t.mime_type).collect());
    let ty = unstructured_binary_schema_type(BinaryRestrictions {
        mime_types,
        ..Default::default()
    });
    CompiledOutputSchema {
        graph: SchemaGraph::anonymous(ty.clone()),
        output_schema: OutputSchema::Single(Box::new(ty)),
    }
}

/// The structural multimodal form `list<variant<…>>` whose list node carries
/// `Role::Multimodal`, which the emitter renders as an opaque binary response.
fn multimodal_response() -> CompiledOutputSchema {
    let mut ty = SchemaType::list(SchemaType::variant(vec![]));
    ty.metadata_mut().role = Some(Role::Multimodal);
    CompiledOutputSchema {
        graph: SchemaGraph::anonymous(ty.clone()),
        output_schema: OutputSchema::Single(Box::new(ty)),
    }
}

fn json_body(ty: SchemaType) -> RequestBodySchema {
    RequestBodySchema::JsonBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(ty),
        },
    }
}

fn json_body_graph(graph: SchemaGraph) -> RequestBodySchema {
    RequestBodySchema::JsonBody {
        expected: CompiledSchema { graph },
    }
}

fn unrestricted_binary() -> RequestBodySchema {
    RequestBodySchema::BinaryBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions::default())),
        },
    }
}

fn restricted_binary(mime_types: Vec<String>) -> RequestBodySchema {
    RequestBodySchema::BinaryBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions {
                mime_types: Some(mime_types),
                ..Default::default()
            })),
        },
    }
}

fn unrestricted_text() -> RequestBodySchema {
    RequestBodySchema::TextBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(SchemaType::text(TextRestrictions::default())),
        },
    }
}

fn restricted_text(languages: Vec<String>) -> RequestBodySchema {
    RequestBodySchema::TextBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(SchemaType::text(TextRestrictions {
                languages: Some(languages),
                ..Default::default()
            })),
        },
    }
}

/// Like [`restricted_binary`], but the body root is the canonical
/// unstructured-binary `variant { inline, url }` wrapper rather than a bare
/// `Binary` rich scalar.
fn wrapper_restricted_binary(mime_types: Vec<String>) -> RequestBodySchema {
    RequestBodySchema::BinaryBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(unstructured_binary_schema_type(BinaryRestrictions {
                mime_types: Some(mime_types),
                ..Default::default()
            })),
        },
    }
}

/// Like [`restricted_text`], but the body root is the canonical
/// unstructured-text `variant { inline, url }` wrapper rather than a bare
/// `Text` rich scalar.
fn wrapper_restricted_text(languages: Vec<String>) -> RequestBodySchema {
    RequestBodySchema::TextBody {
        expected: CompiledSchema {
            graph: SchemaGraph::anonymous(unstructured_text_schema_type(TextRestrictions {
                languages: Some(languages),
                ..Default::default()
            })),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn call_agent_route(
    method: Method,
    path: Vec<PathSegment>,
    body: RequestBodySchema,
    method_parameters: Vec<MethodParameter>,
    response: CompiledOutputSchema,
    method_description: Option<String>,
) -> RichCompiledRoute {
    RichCompiledRoute {
        account_id: AccountId::new(),
        account_email: AccountEmail::new("test@golem.cloud"),
        environment_id: EnvironmentId::new(),
        deployment_revision: golem_common::model::deployment::DeploymentRevision::INITIAL,
        route_id: 0,
        route_match: test_route_match(method),
        path,
        behavior: RichRouteBehaviour::CallAgent(CallAgentBehaviour {
            route_mode: golem_service_base::custom_api::AgentRouteMode::Rest,
            base_path_variables: 0,
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            agent_type: agent_type_name("TestAgent"),
            agent_mode: AgentMode::Durable,
            constructor_input: CompiledInputSchema {
                graph: SchemaGraph::empty(),
                input_schema: InputSchema::Parameters(vec![]),
            },
            constructor_parameters: vec![],
            phantom: false,
            method_name: "test_method".to_string(),
            method_input: CompiledInputSchema {
                graph: SchemaGraph::empty(),
                input_schema: InputSchema::Parameters(vec![]),
            },
            body,
            method_parameters,
            expected_agent_response: response,
            method_description,
            read_only: None,
        }),
        security: RichRouteSecurity::None,
        cors: CorsOptions {
            allowed_patterns: vec![],
        },
    }
}

/// Build the OpenAPI document for a set of routes (panics on error).
fn spec_for(routes: Vec<RichCompiledRoute>) -> Value {
    HttpApiOpenApiSpec::from_routes(&routes.iter().collect::<Vec<_>>(), "https://example.com")
        .expect("spec generation succeeds")
}

#[test]
fn durable_stream_cors_exposes_producer_outcomes_only_for_stream_routes() {
    use crate::custom_api::RichRequest;
    use crate::custom_api::cors::apply_cors_outgoing_middleware;
    use crate::custom_api::route_resolver::ResolvedRouteEntry;
    use golem_common::model::agent::http_files::HttpRequestTarget;
    use golem_common::model::domain_registration::Domain;
    use golem_service_base::custom_api::{AgentRouteMode, OriginPattern};
    for mode in [AgentRouteMode::Rest, AgentRouteMode::DurableStreams] {
        for origin in ["https://allowed.example", "https://blocked.example"] {
            let mut route = call_agent_route(
                Method::POST,
                vec![],
                RequestBodySchema::Unused,
                vec![],
                unit_response(),
                None,
            );
            let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
                panic!()
            };
            call.route_mode = mode;
            route.cors.allowed_patterns = vec![OriginPattern("https://allowed.example".into())];
            let resolved = ResolvedRouteEntry {
                domain: Domain("example.com".into()),
                public_scheme: "https".into(),
                public_authority: "example.com".into(),
                route: std::sync::Arc::new(route),
                captured_path_parameters: vec![],
                request_target: HttpRequestTarget::parse("/").unwrap(),
                openapi_inputs: None,
            };
            let request = RichRequest::new(
                poem::Request::builder()
                    .header("Origin", origin)
                    .body(poem::Body::empty()),
            );
            let mut result = poem::Response::builder()
                .status(http::StatusCode::CONFLICT)
                .finish();
            apply_cors_outgoing_middleware(&mut result, &request, &resolved).unwrap();
            assert_eq!(
                result
                    .headers()
                    .contains_key(&http::header::ACCESS_CONTROL_ALLOW_ORIGIN),
                origin == "https://allowed.example"
            );
            assert_eq!(result.headers().get(&http::header::VARY).unwrap(), "Origin");
            if mode == AgentRouteMode::DurableStreams {
                let exposed: std::collections::BTreeSet<_> = result.headers()
                    [&http::header::ACCESS_CONTROL_EXPOSE_HEADERS]
                    .to_str()
                    .unwrap()
                    .split(", ")
                    .collect();
                for name in [
                    "Producer-Epoch",
                    "Producer-Seq",
                    "Producer-Expected-Seq",
                    "Producer-Received-Seq",
                    "Stream-Next-Offset",
                    "Stream-TTL",
                    "Stream-Expires-At",
                    "Location",
                    "Retry-After",
                ] {
                    assert!(exposed.contains(name), "missing {name}");
                }
            } else {
                assert!(
                    !result
                        .headers()
                        .contains_key(&http::header::ACCESS_CONTROL_EXPOSE_HEADERS)
                );
            }
        }
    }
}

#[test]
fn durable_stream_schema_distinguishes_slots_and_scalar_results() {
    use super::route_schema::build_document_schema;
    use golem_common::schema::NamedField;
    let mut route = call_agent_route(
        Method::PUT,
        vec![PathSegment::Literal {
            value: "streams".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        cm_response(record(vec![
            field("events", SchemaType::stream(Some(str()))),
            field(
                "bytes",
                SchemaType::stream(Some(SchemaType::ref_to("byte".into()))),
            ),
        ])),
        None,
    );
    let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
        panic!()
    };
    call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
    call.method_input.input_schema = InputSchema::parameters([
        NamedField::user_supplied("input", SchemaType::stream(Some(SchemaType::u32()))),
        NamedField::user_supplied("count", SchemaType::u32()),
    ]);
    call.expected_agent_response.graph.defs.push(SchemaTypeDef {
        id: "byte".into(),
        name: None,
        body: SchemaType::u8(),
    });
    let document = build_document_schema(&[&route]).unwrap();
    let slots = document.per_route[0]
        .call_agent
        .as_ref()
        .unwrap()
        .stream_slots
        .as_ref()
        .unwrap();
    assert_eq!(
        slots
            .iter()
            .map(|s| (s.name.as_str(), s.writable, s.binary))
            .collect::<Vec<_>>(),
        vec![
            ("input", true, false),
            ("events", false, false),
            ("bytes", false, true)
        ]
    );
    assert_eq!(slots[0].element, SchemaType::u32());
    assert_eq!(
        document.graph.resolve_ref(&slots[2].element).unwrap(),
        &SchemaType::u8()
    );

    let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
        panic!()
    };
    call.expected_agent_response = cm_response(SchemaType::u8());
    let document = build_document_schema(&[&route]).unwrap();
    let slots = document.per_route[0]
        .call_agent
        .as_ref()
        .unwrap()
        .stream_slots
        .as_ref()
        .unwrap();
    assert_eq!(
        (slots[1].name.as_str(), slots[1].binary),
        ("$result", false)
    );
    assert_eq!(slots[1].element, SchemaType::u8());

    let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
        panic!()
    };
    call.expected_agent_response = unit_response();
    let document = build_document_schema(&[&route]).unwrap();
    assert_eq!(
        document.per_route[0]
            .call_agent
            .as_ref()
            .unwrap()
            .stream_slots
            .as_ref()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn durable_stream_deployment_spec_snapshots() {
    use golem_common::schema::NamedField;
    for with_input in [false, true] {
        let mut route = call_agent_route(
            Method::PUT,
            vec![PathSegment::Literal {
                value: "stream".into(),
            }],
            RequestBodySchema::Unused,
            vec![],
            cm_response(SchemaType::stream(Some(str()))),
            None,
        );
        let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
            panic!()
        };
        call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
        if with_input {
            call.method_input.input_schema = InputSchema::parameters([NamedField::user_supplied(
                "input",
                SchemaType::stream(Some(str())),
            )]);
        }
        let spec = spec_for(vec![route]);
        let name = if with_input {
            "durable-stream-duplex.json"
        } else {
            "durable-stream-output.json"
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/custom_api/openapi/snapshots")
            .join(name);
        if std::env::var("UPDATE_GOLDENFILES").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                format!("{}\n", serde_json::to_string_pretty(&spec).unwrap()),
            )
            .unwrap();
        }
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(spec, expected, "deployment OpenAPI snapshot {name} changed");
    }
}

#[test]
fn durable_stream_routes_do_not_break_rest_openapi() {
    let rest = call_agent_route(
        Method::GET,
        vec![PathSegment::Literal {
            value: "rest".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        CompiledOutputSchema {
            graph: SchemaGraph::empty(),
            output_schema: OutputSchema::Unit,
        },
        None,
    );
    let mut stream = call_agent_route(
        Method::PUT,
        vec![PathSegment::Literal {
            value: "stream".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        CompiledOutputSchema {
            graph: SchemaGraph::anonymous(SchemaType::stream(Some(SchemaType::string()))),
            output_schema: OutputSchema::Single(Box::new(SchemaType::stream(Some(
                SchemaType::string(),
            )))),
        },
        None,
    );
    let RichRouteBehaviour::CallAgent(ref mut call) = stream.behavior else {
        panic!()
    };
    call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
    let mut shared = call_agent_route(
        Method::PUT,
        rest.path.clone(),
        RequestBodySchema::Unused,
        vec![],
        CompiledOutputSchema {
            graph: SchemaGraph::empty(),
            output_schema: OutputSchema::Unit,
        },
        None,
    );
    let RichRouteBehaviour::CallAgent(ref mut call) = shared.behavior else {
        panic!()
    };
    call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
    let mut routes = vec![rest, stream, shared];
    for path in [routes[0].path.clone(), routes[1].path.clone()] {
        let mut preflight = call_agent_route(
            Method::OPTIONS,
            path,
            RequestBodySchema::Unused,
            vec![],
            CompiledOutputSchema {
                graph: SchemaGraph::empty(),
                output_schema: OutputSchema::Unit,
            },
            None,
        );
        preflight.behavior = RichRouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
            method_policies: vec![],
        });
        routes.push(preflight);
    }
    let spec = spec_for(routes);
    assert!(spec["paths"]["/rest"]["get"].is_object());
    assert!(spec["paths"]["/rest"]["options"].is_object());
    assert!(spec["paths"]["/rest"]["put"].is_object());
    assert!(spec["paths"]["/stream"]["put"].is_object());
    assert!(spec["paths"]["/stream"]["options"].is_null());
}

#[test]
fn durable_stream_operations_have_concrete_typed_slots() {
    use golem_common::schema::NamedField;
    let make_route = || {
        let mut route = call_agent_route(
            Method::PUT,
            vec![PathSegment::Literal {
                value: "duplex".into(),
            }],
            RequestBodySchema::Unused,
            vec![],
            cm_response(SchemaType::stream(Some(str()))),
            None,
        );
        let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
            panic!()
        };
        call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
        call.method_input.input_schema = InputSchema::parameters([
            NamedField::user_supplied(
                "messages",
                SchemaType::stream(Some(SchemaType::list(SchemaType::u32()))),
            ),
            NamedField::user_supplied("bytes", SchemaType::stream(Some(SchemaType::u8()))),
        ]);
        route
    };
    let mut routes = vec![make_route()];
    // The compiler expands the family using additional untyped captures. They
    // must not be lowered as independent method bindings or emitted twice.
    for suffix in [
        vec!["invocations", "session"],
        vec!["invocations", "session", "streams", "slot"],
        vec!["forks", "fork", "invocations", "session"],
        vec!["forks", "fork", "invocations", "session", "streams", "slot"],
    ] {
        for method in [
            Method::PUT,
            Method::GET,
            Method::HEAD,
            Method::DELETE,
            Method::POST,
        ] {
            let mut generated = make_route();
            generated.route_match = test_route_match(method);
            generated.path.extend(suffix.iter().map(|part| {
                if ["session", "slot", "fork"].contains(part) {
                    PathSegment::Variable {
                        display_name: (*part).into(),
                    }
                } else {
                    PathSegment::Literal {
                        value: (*part).into(),
                    }
                }
            }));
            routes.push(generated);
        }
    }
    let spec = spec_for(routes);
    let paths = spec["paths"].as_object().unwrap();
    assert_eq!(paths.len(), 9);
    let session = "/duplex/invocations/{session}";
    let input = &paths[&format!("{session}/streams/messages")];
    let bytes = &paths[&format!("{session}/streams/bytes")];
    let output = &paths[&format!("{session}/streams/%24result")];
    assert!(output["post"].is_null());
    assert!(output["delete"].is_object());
    assert_eq!(
        output["get"]["responses"]["200"]["content"]["application/json"]["schema"],
        json!({"type":"array","items":{"type":"string"}})
    );
    assert!(bytes["post"]["requestBody"]["content"]["application/octet-stream"].is_object());
    assert!(bytes["post"]["requestBody"]["content"]["application/json"].is_null());
    let append = &input["post"]["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(
        append["anyOf"][0]["allOf"][1],
        json!({"not":{"type":"array"}})
    );
    assert_eq!(append["anyOf"][1]["items"]["type"], "array");
    assert_eq!(append["anyOf"][1]["minItems"], 1);
    assert_eq!(append["anyOf"][1]["maxItems"], 4096);
    assert_eq!(input["post"]["requestBody"]["required"], false);
    assert!(input["get"]["responses"]["200"]["content"]["text/event-stream"].is_object());
    assert!(input["head"]["responses"]["200"]["content"].is_null());
    assert!(paths[session]["get"]["responses"]["200"]["content"]["application/json"].is_object());
    let fork_session = "/duplex/forks/{fork}/invocations/{session}";
    for slot in ["messages", "bytes", "%24result"] {
        let original = &paths[&format!("{session}/streams/{slot}")];
        let fork = &paths[&format!("{fork_session}/streams/{slot}")];
        for method in ["get", "head", "post", "delete"] {
            assert_eq!(fork[method]["requestBody"], original[method]["requestBody"]);
            assert_eq!(fork[method]["responses"], original[method]["responses"]);
            if let Some(parameters) = fork[method]["parameters"].as_array() {
                let inherited: Vec<_> = parameters
                    .iter()
                    .filter(|p| p["name"] != "fork")
                    .cloned()
                    .collect();
                assert_eq!(json!(inherited), original[method]["parameters"]);
            }
        }
        assert!(fork["put"]["responses"]["201"].is_object());
        assert!(
            fork["put"]["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| { p["name"] == "Stream-Forked-From" && p["required"] == true })
        );
    }
    let mut ids = std::collections::HashSet::new();
    for item in paths.values() {
        assert_eq!(item["x-golem-route-mode"], "durable-streams");
        for method in ["put", "get", "head", "post", "delete"] {
            if let Some(operation) = item.get(method) {
                assert!(ids.insert(operation["operationId"].as_str().unwrap()));
                assert!(operation["responses"]["503"]["headers"]["Retry-After"].is_object());
            }
        }
    }
    fn check_refs(value: &Value, document: &Value) {
        match value {
            Value::Object(object) => {
                for (name, value) in object {
                    if name == "$ref" || name == "element-schema-ref" {
                        assert!(
                            document
                                .pointer(value.as_str().unwrap().strip_prefix('#').unwrap())
                                .is_some(),
                            "unresolved {value}"
                        );
                    } else {
                        check_refs(value, document);
                    }
                }
            }
            Value::Array(values) => values.iter().for_each(|value| check_refs(value, document)),
            _ => {}
        }
    }
    check_refs(&spec, &spec);
}

#[test]
fn durable_stream_fork_creation_documents_runtime_response_headers() {
    use golem_common::schema::NamedField;

    let mut route = call_agent_route(
        Method::PUT,
        vec![PathSegment::Literal {
            value: "fork-response-headers".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
        panic!()
    };
    call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
    call.method_input.input_schema = InputSchema::parameters([NamedField::user_supplied(
        "input",
        SchemaType::stream(Some(str())),
    )]);

    let spec = spec_for(vec![route]);
    let put = &spec["paths"]["/fork-response-headers/forks/{fork}/invocations/{session}/streams/input"]
        ["put"];
    let mut missing = Vec::new();
    for status in ["200", "201"] {
        if !put["responses"][status]["headers"]["Location"].is_object() {
            missing.push(format!("{status} Location"));
        }
    }
    if !put["responses"]["429"]["headers"]["Retry-After"].is_object() {
        missing.push("429 Retry-After".into());
    }
    assert!(
        missing.is_empty(),
        "missing runtime response headers: {missing:?}"
    );
}

#[test]
fn durable_stream_expiry_headers_are_documented_on_creation_and_head() {
    use golem_common::schema::NamedField;

    let mut route = call_agent_route(
        Method::PUT,
        vec![PathSegment::Literal {
            value: "expiry-headers".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
        panic!()
    };
    call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
    call.method_input.input_schema = InputSchema::parameters([NamedField::user_supplied(
        "input",
        SchemaType::stream(Some(str())),
    )]);

    let spec = spec_for(vec![route]);
    let paths = &spec["paths"];
    let cases = [
        ("/expiry-headers", "put"),
        ("/expiry-headers/invocations/{session}", "put"),
        ("/expiry-headers/invocations/{session}/streams/input", "put"),
        (
            "/expiry-headers/invocations/{session}/streams/input",
            "post",
        ),
        (
            "/expiry-headers/forks/{fork}/invocations/{session}/streams/input",
            "put",
        ),
    ];
    for (path, method) in cases {
        let names = paths[path][method]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|parameter| parameter["name"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(names.contains("Stream-TTL"), "{method} {path}");
        assert!(names.contains("Stream-Expires-At"), "{method} {path}");
    }

    for path in [
        "/expiry-headers/invocations/{session}",
        "/expiry-headers/invocations/{session}/streams/input",
    ] {
        let headers = &paths[path]["head"]["responses"]["200"]["headers"];
        assert!(headers["Stream-TTL"].is_object(), "HEAD {path}");
        assert!(headers["Stream-Expires-At"].is_object(), "HEAD {path}");
    }
    let get_headers = &paths["/expiry-headers/invocations/{session}/streams/input"]["get"]["responses"]
        ["200"]["headers"];
    assert!(get_headers["Stream-TTL"].is_null());
    assert!(get_headers["Stream-Expires-At"].is_null());
}

#[test]
fn durable_stream_slot_put_documents_optional_content_type() {
    use golem_common::schema::NamedField;

    let mut route = call_agent_route(
        Method::PUT,
        vec![PathSegment::Literal {
            value: "content-type".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
        panic!()
    };
    call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
    call.method_input.input_schema = InputSchema::parameters([NamedField::user_supplied(
        "input",
        SchemaType::stream(Some(str())),
    )]);

    let spec = spec_for(vec![route]);
    let operation = &spec["paths"]["/content-type/invocations/{session}/streams/input"]["put"];
    assert!(operation["requestBody"].is_null());
    assert!(
        operation["description"]
            .as_str()
            .unwrap()
            .contains("Content-Type is optional; when supplied it must be application/json")
    );
}

#[test]
fn durable_stream_slot_named_session_has_unique_operation_ids() {
    use golem_common::schema::NamedField;

    let mut route = call_agent_route(
        Method::PUT,
        vec![PathSegment::Literal {
            value: "operation-id-collision".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
        panic!()
    };
    call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
    call.method_input.input_schema = InputSchema::parameters([NamedField::user_supplied(
        "session",
        SchemaType::stream(Some(str())),
    )]);

    let spec = spec_for(vec![route]);
    let mut operation_ids = std::collections::HashSet::new();
    let mut duplicates = Vec::new();
    for path_item in spec["paths"].as_object().unwrap().values() {
        for method in ["put", "get", "head", "post", "delete"] {
            if let Some(operation) = path_item.get(method) {
                let operation_id = operation["operationId"].as_str().unwrap();
                if !operation_ids.insert(operation_id) {
                    duplicates.push(operation_id);
                }
            }
        }
    }
    assert!(
        duplicates.is_empty(),
        "duplicate operationIds: {duplicates:?}"
    );
}

#[test]
fn durable_stream_method_bindings_are_operation_specific() {
    use golem_common::schema::NamedField;
    for (body, binding, media) in [
        (RequestBodySchema::Unused, None, None),
        (
            json_body(record(vec![field("value", str())])),
            Some(MethodParameter::JsonObjectBodyField {
                field_index: SafeIndex::new(0),
            }),
            Some("application/json"),
        ),
        (
            unrestricted_text(),
            Some(MethodParameter::UnstructuredTextBody),
            Some("text/plain"),
        ),
        (
            unrestricted_binary(),
            Some(MethodParameter::UnstructuredBinaryBody),
            Some("*/*"),
        ),
        (
            RequestBodySchema::Unused,
            Some(MethodParameter::Header {
                header_name: "x-value".into(),
                parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::Str),
            }),
            None,
        ),
    ] {
        let url_bound = binding.is_none();
        let mut bindings = vec![
            MethodParameter::Path {
                path_segment_index: SafeIndex::new(0),
                parameter_type: PathSegmentType::Str,
            },
            MethodParameter::Query {
                query_parameter_name: "count".into(),
                parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::U32),
            },
        ];
        bindings.extend(binding);
        let mut route = call_agent_route(
            Method::PUT,
            vec![
                PathSegment::Literal {
                    value: "bound".into(),
                },
                PathSegment::Variable {
                    display_name: "session".into(),
                },
            ],
            body,
            bindings,
            unit_response(),
            None,
        );
        let RichRouteBehaviour::CallAgent(call) = &mut route.behavior else {
            panic!()
        };
        call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
        call.base_path_variables = 1;
        call.method_input.input_schema = InputSchema::parameters([NamedField::user_supplied(
            "input",
            SchemaType::stream(Some(str())),
        )]);
        let spec = spec_for(vec![route]);
        let paths = &spec["paths"];
        let session = "/bound/{session}/invocations/{ds_session}";
        let slot = format!("{session}/streams/input");
        for path in ["/bound/{session}", session] {
            let op = &paths[path]["put"];
            assert_eq!(
                op["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| p["name"] == "count")
                    .unwrap()["required"],
                true
            );
            if let Some(media) = media {
                assert!(op["requestBody"]["content"][media].is_object());
            } else {
                assert!(op["requestBody"].is_null());
            }
        }
        for method in ["put", "post", "get", "head", "delete"] {
            let op = &paths[&slot][method];
            let params = op["parameters"].as_array().unwrap();
            let count = params.iter().find(|p| p["name"] == "count");
            if url_bound && ["put", "post"].contains(&method) {
                assert_eq!(count.unwrap()["required"], method == "put");
            } else {
                assert!(count.is_none());
            }
            assert!(
                params
                    .iter()
                    .any(|p| p["name"] == "session" && p["required"] == true)
            );
            assert!(
                params
                    .iter()
                    .any(|p| p["name"] == "ds_session" && p["required"] == true)
            );
            assert!(
                !params
                    .iter()
                    .any(|p| p["name"] == "x-value" || p["name"] == "Content-Language")
            );
        }
        let fork = &paths["/bound/{session}/forks/{fork}/invocations/{ds_session}/streams/input"];
        for (method, expected) in [
            ("post", "Producer-Id"),
            ("put", "Stream-Forked-From"),
            ("get", "offset"),
        ] {
            let parameters = fork[method]["parameters"].as_array().unwrap();
            assert!(!parameters.iter().any(|p| p["name"] == "count"));
            assert!(parameters.iter().any(|p| p["name"] == expected));
        }
    }
}

#[test]
fn durable_stream_concrete_slot_yields_to_explicit_rest_route() {
    for (reverse, slot) in [
        (false, "messages"),
        (true, "messages"),
        (false, "$result"),
        (true, "$result"),
    ] {
        let mut stream = call_agent_route(
            Method::PUT,
            vec![PathSegment::Literal {
                value: "overlap".into(),
            }],
            RequestBodySchema::Unused,
            vec![],
            if slot == "$result" {
                cm_response(SchemaType::stream(Some(str())))
            } else {
                cm_response(record(vec![field(
                    "messages",
                    SchemaType::stream(Some(str())),
                )]))
            },
            None,
        );
        let RichRouteBehaviour::CallAgent(call) = &mut stream.behavior else {
            panic!()
        };
        call.route_mode = golem_service_base::custom_api::AgentRouteMode::DurableStreams;
        let rest = call_agent_route(
            Method::GET,
            vec![
                PathSegment::Literal {
                    value: "overlap".into(),
                },
                PathSegment::Literal {
                    value: "invocations".into(),
                },
                PathSegment::Variable {
                    display_name: "id".into(),
                },
                PathSegment::Literal {
                    value: "streams".into(),
                },
                PathSegment::Literal { value: slot.into() },
            ],
            RequestBodySchema::Unused,
            vec![MethodParameter::Path {
                path_segment_index: SafeIndex::new(0),
                parameter_type: PathSegmentType::Str,
            }],
            unit_response(),
            Some("Explicit REST route".into()),
        );
        let duplicate_routes = [&stream, &rest, &rest];
        assert!(
            HttpApiOpenApiSpec::from_routes(&duplicate_routes, "https://api.example.com")
                .unwrap_err()
                .contains("Duplicate generated operation")
        );
        let mut routes = vec![stream, rest];
        if reverse {
            routes.reverse();
        }
        let spec = spec_for(routes);
        let canonical = format!(
            "/overlap/invocations/{{session}}/streams/{}",
            slot.replace('$', "%24")
        );
        let item = &spec["paths"][&canonical];
        assert_eq!(item["get"]["description"], "Explicit REST route");
        assert_eq!(item["get"]["x-golem-route-mode"], "rest");
        assert_eq!(item["get"]["parameters"][0]["name"], "session");
        assert!(item["get"]["responses"]["204"].is_object());
        assert!(item["get"]["responses"]["200"].is_null());
        assert!(item["head"].is_object());
        assert!(spec["paths"][format!("/overlap/invocations/{{id}}/streams/{slot}")].is_null());
    }
}

/// Build the OpenAPI document for a single route and return its operation
/// object at the given path/method.
fn operation_for(route: RichCompiledRoute, path: &str, method: &str) -> Value {
    let spec = spec_for(vec![route]);
    spec["paths"][path][method].clone()
}

// --------------------------------------------------------------------------
// Document envelope
// --------------------------------------------------------------------------

#[test]
fn document_is_openapi_3_1_with_servers() {
    let route = call_agent_route(
        Method::GET,
        vec![PathSegment::Literal {
            value: "ping".to_string(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    let spec = spec_for(vec![route]);
    assert_eq!(spec["openapi"], json!("3.1.0"));
    assert_eq!(
        spec["info"]["title"],
        json!("Managed api provided by Golem")
    );
    assert_eq!(spec["servers"], json!([{"url":"https://example.com"}]));
    // No named types in these routes → components has no schemas.
    assert!(spec["components"].get("schemas").is_none());
}

#[test]
fn multiple_rest_bindings_omit_generated_operation_ids_and_preserve_trailing_slashes() {
    let route = || {
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "item".into(),
            }],
            RequestBodySchema::Unused,
            vec![],
            unit_response(),
            None,
        )
    };
    let plain = route();
    let mut slash = route();
    let golem_service_base::custom_api::RouteMatch::Method { trailing_slash, .. } =
        &mut slash.route_match
    else {
        unreachable!()
    };
    *trailing_slash = true;
    let spec = spec_for(vec![plain, slash]);
    for path in ["/item", "/item/"] {
        assert!(spec["paths"][path]["get"].is_object());
        assert!(spec["paths"][path]["get"].get("operationId").is_none());
        assert_eq!(spec["paths"][path]["get"]["security"], json!([]));
    }
}

#[test]
fn generated_security_never_treats_protected_routes_as_public() {
    let route = || {
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "item".into(),
            }],
            RequestBodySchema::Unused,
            vec![],
            unit_response(),
            None,
        )
    };
    let mut protected = route();
    protected.security = RichRouteSecurity::SessionFromHeader(
        golem_service_base::custom_api::SessionFromHeaderRouteSecurity {
            header_name: "X-Session".into(),
        },
    );
    let mut other = route();
    other.path = vec![PathSegment::Literal {
        value: "other".into(),
    }];
    other.security = RichRouteSecurity::SessionFromHeader(
        golem_service_base::custom_api::SessionFromHeaderRouteSecurity {
            header_name: "x-session".into(),
        },
    );
    let spec = spec_for(vec![protected, other]);
    assert_eq!(
        spec["paths"]["/item"]["get"]["security"],
        json!([{"golem-session-header-eC1zZXNzaW9u":[]}])
    );
    assert_eq!(
        spec["components"]["securitySchemes"]["golem-session-header-eC1zZXNzaW9u"],
        json!({"type":"apiKey","in":"header","name":"x-session"})
    );
    let mut unavailable = route();
    unavailable.security = RichRouteSecurity::Unavailable;
    assert!(HttpApiOpenApiSpec::from_routes(&[&unavailable], "https://example.com").is_err());
}

#[test]
fn generated_session_security_encodes_header_punctuation_in_component_keys() {
    let mut route = call_agent_route(
        Method::GET,
        vec![PathSegment::Literal {
            value: "item".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    route.security = RichRouteSecurity::SessionFromHeader(
        golem_service_base::custom_api::SessionFromHeaderRouteSecurity {
            header_name: "X-Session+Id".into(),
        },
    );

    let spec = spec_for(vec![route]);
    assert_eq!(
        spec["paths"]["/item"]["get"]["security"],
        json!([{"golem-session-header-eC1zZXNzaW9uK2lk":[]}])
    );
    assert_eq!(
        spec["components"]["securitySchemes"]["golem-session-header-eC1zZXNzaW9uK2lk"]["name"],
        "x-session+id"
    );
    assert!(super::provider_document::parse("generated", &spec.to_string()).is_ok());
}

#[test]
fn generated_duplicate_operations_fail_before_overwriting() {
    let route = || {
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "item".into(),
            }],
            RequestBodySchema::Unused,
            vec![],
            unit_response(),
            None,
        )
    };
    assert!(HttpApiOpenApiSpec::from_routes(&[&route(), &route()], "https://example.com").is_err());
}

#[test]
fn generated_literal_paths_do_not_turn_encoded_braces_into_templates() {
    let route = call_agent_route(
        Method::GET,
        vec![PathSegment::Literal {
            value: "{literal}%".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    let spec = spec_for(vec![route]);
    assert!(spec["paths"].get("/%7Bliteral%7D%25").is_some());
}

#[test]
fn generated_paths_preserve_safe_literal_punctuation() {
    let route = call_agent_route(
        Method::GET,
        vec![PathSegment::Literal {
            value: "@me:a,b+c!".into(),
        }],
        RequestBodySchema::Unused,
        vec![],
        unit_response(),
        None,
    );
    let spec = spec_for(vec![route]);
    assert!(spec["paths"].get("/@me:a,b+c!").is_some());
}

#[test]
fn distinct_single_binding_methods_with_colliding_ids_fail_instead_of_losing_ids() {
    let route = |path: &str, agent_type: &str, method_name: &str| {
        let mut route = call_agent_route(
            Method::GET,
            vec![PathSegment::Literal { value: path.into() }],
            RequestBodySchema::Unused,
            vec![],
            unit_response(),
            None,
        );
        let RichRouteBehaviour::CallAgent(inner) = &mut route.behavior else {
            unreachable!()
        };
        inner.agent_type = agent_type_name(agent_type);
        inner.method_name = method_name.into();
        route
    };
    assert!(
        HttpApiOpenApiSpec::from_routes(
            &[&route("one", "a-b", "c"), &route("two", "a", "b-c")],
            "https://example.com"
        )
        .is_err()
    );
}

// --------------------------------------------------------------------------
// Response protocol policies
// --------------------------------------------------------------------------

#[test]
fn unit_response_maps_to_204_no_content() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "unit".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            unit_response(),
            None,
        ),
        "/unit",
        "get",
    );
    assert_eq!(op["responses"]["204"]["description"], json!("Response 204"));
    assert!(op["responses"]["204"].get("content").is_none());
    assert!(op["responses"].get("200").is_none());
}

#[test]
fn option_response_maps_to_200_inner_plus_404() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "opt".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            cm_response(option(record(vec![field("value", str())]))),
            None,
        ),
        "/opt",
        "get",
    );
    // 200 renders the INNER record, not the option wrapper (no `oneOf`/null).
    let ok = &op["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(ok["type"], json!("object"));
    assert_eq!(ok["properties"]["value"]["type"], json!("string"));
    assert!(ok.get("oneOf").is_none());
    // 404 carries no body.
    assert_eq!(op["responses"]["404"]["description"], json!("Response 404"));
    assert!(op["responses"]["404"].get("content").is_none());
}

#[test]
fn result_ok_and_err_map_to_200_and_500() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "res".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            cm_response(result(
                record(vec![field("value", str())]),
                record(vec![field("error", str())]),
            )),
            None,
        ),
        "/res",
        "get",
    );
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["value"]["type"],
        json!("string")
    );
    assert_eq!(
        op["responses"]["500"]["content"]["application/json"]["schema"]["properties"]["error"]["type"],
        json!("string")
    );
}

#[test]
fn result_void_err_maps_500_to_no_content() {
    // result<ok, _> with no error type → 200 json (ok) + 500 no content.
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "rok".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            cm_response(result_ok(record(vec![field("value", str())]))),
            None,
        ),
        "/rok",
        "get",
    );
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        json!("object")
    );
    assert!(op["responses"]["500"].is_object());
    assert!(op["responses"]["500"].get("content").is_none());
}

#[test]
fn result_void_ok_maps_204_to_no_content() {
    // result<_, err> with no ok type → 204 no content + 500 json (err).
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "rerr".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            cm_response(result_err(record(vec![field("error", str())]))),
            None,
        ),
        "/rerr",
        "get",
    );
    assert!(op["responses"]["204"].is_object());
    assert!(op["responses"]["204"].get("content").is_none());
    assert_eq!(
        op["responses"]["500"]["content"]["application/json"]["schema"]["type"],
        json!("object")
    );
}

#[test]
fn text_response_maps_to_text_plain_with_content_language_header() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "txt".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            text_response(Some(vec![TextType {
                language_code: "en".to_string(),
            }])),
            None,
        ),
        "/txt",
        "get",
    );
    assert_eq!(
        op["responses"]["200"]["content"]["text/plain"]["schema"],
        json!({ "type": "string" })
    );
    let header = &op["responses"]["200"]["headers"]["Content-Language"];
    assert_eq!(header["required"], json!(false));
    assert_eq!(header["schema"]["enum"], json!(["en"]));
}

#[test]
fn binary_response_uses_restricted_mime_or_octet_stream() {
    let restricted = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "bin".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            binary_response(Some(vec![BinaryType {
                mime_type: "image/png".to_string(),
            }])),
            None,
        ),
        "/bin",
        "get",
    );
    assert_eq!(
        restricted["responses"]["200"]["content"]["image/png"]["schema"],
        json!({ "type": "string", "format": "binary" })
    );

    let unrestricted = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "bin".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            binary_response(None),
            None,
        ),
        "/bin",
        "get",
    );
    assert!(
        unrestricted["responses"]["200"]["content"]["application/octet-stream"]["schema"]
            .is_object()
    );
}

#[test]
fn unstructured_text_wrapper_response_matches_bare_text_response() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "txt".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            wrapper_text_response(Some(vec![TextType {
                language_code: "en".to_string(),
            }])),
            None,
        ),
        "/txt",
        "get",
    );
    assert_eq!(
        op["responses"]["200"]["content"]["text/plain"]["schema"],
        json!({ "type": "string" })
    );
    let header = &op["responses"]["200"]["headers"]["Content-Language"];
    assert_eq!(header["required"], json!(false));
    assert_eq!(header["schema"]["enum"], json!(["en"]));
}

#[test]
fn unstructured_binary_wrapper_response_matches_bare_binary_response() {
    let restricted = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "bin".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            wrapper_binary_response(Some(vec![BinaryType {
                mime_type: "image/png".to_string(),
            }])),
            None,
        ),
        "/bin",
        "get",
    );
    assert_eq!(
        restricted["responses"]["200"]["content"]["image/png"]["schema"],
        json!({ "type": "string", "format": "binary" })
    );

    let unrestricted = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "bin".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            wrapper_binary_response(None),
            None,
        ),
        "/bin",
        "get",
    );
    assert!(
        unrestricted["responses"]["200"]["content"]["application/octet-stream"]["schema"]
            .is_object()
    );
}

#[test]
fn multimodal_response_maps_to_unknown_binary() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "mm".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            multimodal_response(),
            None,
        ),
        "/mm",
        "get",
    );
    assert_eq!(
        op["responses"]["200"]["content"]["*/*"]["schema"],
        json!({ "type": "string", "format": "binary" })
    );
}

// --------------------------------------------------------------------------
// Request bodies
// --------------------------------------------------------------------------

#[test]
fn json_request_body_renders_application_json() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "json".to_string(),
            }],
            json_body(record(vec![field("name", str())])),
            vec![],
            unit_response(),
            None,
        ),
        "/json",
        "post",
    );
    let body = &op["requestBody"];
    assert_eq!(body["description"], json!("JSON body"));
    assert_eq!(body["required"], json!(true));
    assert_eq!(
        body["content"]["application/json"]["schema"]["properties"]["name"]["type"],
        json!("string")
    );
}

#[test]
fn unrestricted_text_body_adds_content_language_parameter() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "txtbody".to_string(),
            }],
            unrestricted_text(),
            vec![],
            unit_response(),
            None,
        ),
        "/txtbody",
        "post",
    );
    assert_eq!(
        op["requestBody"]["content"]["text/plain"]["schema"],
        json!({ "type": "string" })
    );
    let params = op["parameters"].as_array().expect("parameters");
    let content_language = params
        .iter()
        .find(|p| p["name"] == json!("Content-Language"))
        .expect("Content-Language header parameter present");
    assert_eq!(content_language["in"], json!("header"));
    assert_eq!(content_language["required"], json!(false));
}

#[test]
fn restricted_binary_body_lists_each_mime_type() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "binbody".to_string(),
            }],
            restricted_binary(vec!["image/gif".to_string()]),
            vec![],
            unit_response(),
            None,
        ),
        "/binbody",
        "post",
    );
    assert_eq!(
        op["requestBody"]["description"],
        json!("Restricted binary body")
    );
    assert!(
        op["requestBody"]["content"]["image/gif"]["schema"]
            .as_object()
            .is_some()
    );
}

#[test]
fn unstructured_binary_wrapper_request_body_lists_each_mime_type() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "binbody".to_string(),
            }],
            wrapper_restricted_binary(vec!["image/gif".to_string()]),
            vec![],
            unit_response(),
            None,
        ),
        "/binbody",
        "post",
    );
    assert_eq!(
        op["requestBody"]["description"],
        json!("Restricted binary body")
    );
    assert!(
        op["requestBody"]["content"]["image/gif"]["schema"]
            .as_object()
            .is_some()
    );
}

#[test]
fn unstructured_text_wrapper_request_body_lists_languages() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "txtbody".to_string(),
            }],
            wrapper_restricted_text(vec!["en".to_string(), "de".to_string()]),
            vec![],
            unit_response(),
            None,
        ),
        "/txtbody",
        "post",
    );
    assert_eq!(
        op["requestBody"]["content"]["text/plain"]["schema"],
        json!({ "type": "string" })
    );
    let params = op["parameters"].as_array().expect("parameters");
    let content_language = params
        .iter()
        .find(|p| p["name"] == json!("Content-Language"))
        .expect("Content-Language header parameter present");
    assert_eq!(content_language["in"], json!("header"));
    assert_eq!(content_language["schema"]["enum"], json!(["en", "de"]));
}

// --------------------------------------------------------------------------
// Parameters
// --------------------------------------------------------------------------

#[test]
fn path_parameter_is_required() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![
                PathSegment::Literal {
                    value: "items".to_string(),
                },
                PathSegment::Variable {
                    display_name: "id".to_string(),
                },
            ],
            RequestBodySchema::Unused,
            vec![MethodParameter::Path {
                path_segment_index: SafeIndex::new(0),
                parameter_type: PathSegmentType::Str,
            }],
            unit_response(),
            None,
        ),
        "/items/{id}",
        "get",
    );
    let params = op["parameters"].as_array().expect("parameters");
    let id = params.iter().find(|p| p["name"] == json!("id")).unwrap();
    assert_eq!(id["in"], json!("path"));
    assert_eq!(id["required"], json!(true));
    assert_eq!(id["schema"]["type"], json!("string"));
}

#[test]
fn catchall_path_parameter_has_description() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![
                PathSegment::Literal {
                    value: "rest".to_string(),
                },
                PathSegment::CatchAll {
                    display_name: "tail".to_string(),
                },
            ],
            RequestBodySchema::Unused,
            vec![MethodParameter::Path {
                path_segment_index: SafeIndex::new(0),
                parameter_type: PathSegmentType::Str,
            }],
            unit_response(),
            None,
        ),
        "/rest/{tail}",
        "get",
    );
    let params = op["parameters"].as_array().expect("parameters");
    let tail = params.iter().find(|p| p["name"] == json!("tail")).unwrap();
    assert_eq!(
        tail["schema"]["description"],
        json!("Parameter represents the remaining path, including slashes.")
    );
}

#[test]
fn optional_query_parameter_is_not_required() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "q".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![
                MethodParameter::Query {
                    query_parameter_name: "limit".to_string(),
                    parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::U64),
                },
                MethodParameter::Query {
                    query_parameter_name: "cursor".to_string(),
                    parameter_type: QueryOrHeaderType::Option {
                        name: None,
                        owner: None,
                        inner: Box::new(PathSegmentType::Str),
                    },
                },
            ],
            unit_response(),
            None,
        ),
        "/q",
        "get",
    );
    let params = op["parameters"].as_array().expect("parameters");
    let limit = params.iter().find(|p| p["name"] == json!("limit")).unwrap();
    assert_eq!(limit["in"], json!("query"));
    assert_eq!(limit["required"], json!(true));
    assert_eq!(limit["style"], json!("form"));
    assert_eq!(limit["allowEmptyValue"], json!(false));

    let cursor = params
        .iter()
        .find(|p| p["name"] == json!("cursor"))
        .unwrap();
    assert_eq!(cursor["required"], json!(false));
}

// --------------------------------------------------------------------------
// operationId / description
// --------------------------------------------------------------------------

#[test]
fn call_agent_operation_has_id_and_description() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "desc".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            unit_response(),
            Some("Does a thing".to_string()),
        ),
        "/desc",
        "get",
    );
    assert_eq!(op["operationId"], json!("TestAgent-test_method"));
    assert_eq!(op["description"], json!("Does a thing"));
}

// --------------------------------------------------------------------------
// Non-CallAgent behaviours
// --------------------------------------------------------------------------

fn raw_route(
    method: Method,
    path: Vec<PathSegment>,
    behavior: RichRouteBehaviour,
) -> RichCompiledRoute {
    RichCompiledRoute {
        account_id: AccountId::new(),
        account_email: AccountEmail::new("test@golem.cloud"),
        environment_id: EnvironmentId::new(),
        deployment_revision: golem_common::model::deployment::DeploymentRevision::INITIAL,
        route_id: 0,
        route_match: test_route_match(method),
        path,
        behavior,
        security: RichRouteSecurity::None,
        cors: CorsOptions {
            allowed_patterns: vec![],
        },
    }
}

fn test_route_match(method: Method) -> golem_service_base::custom_api::RouteMatch {
    use golem_common::model::Empty;
    use golem_common::model::agent::HttpMethod;
    match method {
        Method::GET => HttpMethod::Get(Empty {}).into(),
        Method::POST => HttpMethod::Post(Empty {}).into(),
        Method::PUT => HttpMethod::Put(Empty {}).into(),
        Method::DELETE => HttpMethod::Delete(Empty {}).into(),
        Method::PATCH => HttpMethod::Patch(Empty {}).into(),
        Method::HEAD => HttpMethod::Head(Empty {}).into(),
        Method::OPTIONS => HttpMethod::Options(Empty {}).into(),
        other => HttpMethod::Custom(golem_common::model::agent::CustomHttpMethod {
            value: other.to_string(),
        })
        .into(),
    }
}

#[test]
fn webhook_route_emits_204_404_and_promise_id_param() {
    let op = operation_for(
        raw_route(
            Method::POST,
            vec![
                PathSegment::Literal {
                    value: "webhooks".to_string(),
                },
                PathSegment::Variable {
                    display_name: "promise-id".to_string(),
                },
            ],
            RichRouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
                component_id: ComponentId::new(),
            }),
        ),
        "/webhooks/{promise-id}",
        "post",
    );
    assert!(op["responses"]["204"].is_object());
    assert!(op["responses"]["404"].is_object());
    assert_eq!(
        op["requestBody"],
        json!({
            "description": "Unrestricted binary body",
            "required": true,
            "content": {
                "*/*": {
                    "schema": {
                        "type": "string",
                        "format": "binary"
                    }
                }
            }
        })
    );
    let params = op["parameters"].as_array().expect("parameters");
    let promise = params
        .iter()
        .find(|p| p["name"] == json!("promise-id"))
        .expect("promise-id path parameter present");
    assert_eq!(promise["in"], json!("path"));
    assert_eq!(promise["schema"]["type"], json!("string"));
    // No operationId for non-CallAgent routes.
    assert!(op.get("operationId").is_none());
}

#[test]
fn openapi_spec_route_returns_object_with_additional_properties() {
    let op = operation_for(
        raw_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "openapi.json".to_string(),
            }],
            RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Json,
                scheme: Default::default(),
            }),
        ),
        "/openapi.json",
        "get",
    );
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["schema"],
        json!({ "type": "object", "additionalProperties": true })
    );
}

// --------------------------------------------------------------------------
// Named types → components/schemas (deduplicated across routes)
// --------------------------------------------------------------------------

#[test]
fn named_type_shared_across_routes_appears_once_per_direction_in_components() {
    // A named record used as both a request body and a response should appear
    // once per direction in components/schemas, referenced by `$ref`.
    let named = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId("User".to_string()),
            name: Some("User".to_string()),
            body: record(vec![field("id", str())]),
        }],
        root: SchemaType::ref_to(TypeId("User".to_string())),
    };

    let route_a = call_agent_route(
        Method::POST,
        vec![PathSegment::Literal {
            value: "a".to_string(),
        }],
        json_body_graph(named.clone()),
        vec![],
        unit_response(),
        None,
    );
    let route_b = call_agent_route(
        Method::GET,
        vec![PathSegment::Literal {
            value: "b".to_string(),
        }],
        RequestBodySchema::Unused,
        vec![],
        cm_response_graph(named),
        None,
    );

    let spec = spec_for(vec![route_a, route_b]);

    let schemas = spec["components"]["schemas"]
        .as_object()
        .expect("components.schemas present");
    let user_keys: Vec<_> = schemas
        .keys()
        .filter(|k| k.contains("User") || k.ends_with("User"))
        .collect();
    assert_eq!(
        user_keys.len(),
        2,
        "named type should appear once per direction, got keys: {:?}",
        schemas.keys().collect::<Vec<_>>()
    );

    // Request body references the component via `$ref`.
    let body_schema =
        &spec["paths"]["/a"]["post"]["requestBody"]["content"]["application/json"]["schema"];
    assert!(
        body_schema["$ref"]
            .as_str()
            .unwrap()
            .starts_with("#/components/schemas/Input_"),
        "request body should reference the component, got: {body_schema}"
    );
    // The response references the output component for the same named type.
    let response_schema =
        &spec["paths"]["/b"]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(
        response_schema["$ref"],
        body_schema["$ref"]
            .as_str()
            .unwrap()
            .replacen("/Input_", "/Output_", 1)
    );
}

#[test]
fn restricted_text_body_content_language_lists_languages() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "rtxt".to_string(),
            }],
            restricted_text(vec!["en".to_string(), "hu".to_string()]),
            vec![],
            unit_response(),
            None,
        ),
        "/rtxt",
        "post",
    );
    assert_eq!(
        op["requestBody"]["content"]["text/plain"]["schema"],
        json!({ "type": "string" })
    );
    let params = op["parameters"].as_array().expect("parameters");
    let content_language = params
        .iter()
        .find(|p| p["name"] == json!("Content-Language"))
        .expect("Content-Language header parameter present");
    assert_eq!(content_language["required"], json!(false));
    assert_eq!(content_language["schema"]["enum"], json!(["en", "hu"]));
}

#[test]
fn unrestricted_binary_body_uses_wildcard_media_type() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "ubin".to_string(),
            }],
            unrestricted_binary(),
            vec![],
            unit_response(),
            None,
        ),
        "/ubin",
        "post",
    );
    assert_eq!(
        op["requestBody"]["description"],
        json!("Unrestricted binary body")
    );
    assert_eq!(
        op["requestBody"]["content"]["*/*"]["schema"],
        json!({ "type": "string", "format": "binary" })
    );
}

#[test]
fn optional_header_parameter_is_not_required() {
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "hdr".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![
                MethodParameter::Header {
                    header_name: "x-required".to_string(),
                    parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::Str),
                },
                MethodParameter::Header {
                    header_name: "x-optional".to_string(),
                    parameter_type: QueryOrHeaderType::Option {
                        name: None,
                        owner: None,
                        inner: Box::new(PathSegmentType::Str),
                    },
                },
            ],
            unit_response(),
            None,
        ),
        "/hdr",
        "get",
    );
    let params = op["parameters"].as_array().expect("parameters");
    let required = params
        .iter()
        .find(|p| p["name"] == json!("x-required"))
        .unwrap();
    assert_eq!(required["in"], json!("header"));
    assert_eq!(required["required"], json!(true));
    let optional = params
        .iter()
        .find(|p| p["name"] == json!("x-optional"))
        .unwrap();
    assert_eq!(optional["required"], json!(false));
}

#[test]
fn cors_preflight_emits_204_and_cors_headers() {
    use golem_common::base_model::Empty;
    use golem_common::base_model::agent::HttpMethod;
    use golem_service_base::custom_api::{CorsPreflightBehaviour, CorsPreflightMethodPolicy};
    use std::collections::BTreeSet;

    let op = operation_for(
        raw_route(
            Method::OPTIONS,
            vec![PathSegment::Literal {
                value: "cors".to_string(),
            }],
            RichRouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
                method_policies: vec![CorsPreflightMethodPolicy {
                    method: HttpMethod::Get(Empty {}),
                    allowed_origins: BTreeSet::new(),
                    allowed_headers: BTreeSet::new(),
                }],
            }),
        ),
        "/cors",
        "options",
    );
    assert!(op["responses"]["204"].is_object());
    assert!(op["responses"]["204"].get("content").is_none());
    let headers = &op["responses"]["204"]["headers"];
    for name in [
        "Access-Control-Allow-Origin",
        "Access-Control-Allow-Headers",
        "Access-Control-Allow-Credentials",
    ] {
        assert_eq!(headers[name]["required"], json!(true), "{name}");
        assert_eq!(
            headers[name]["schema"],
            json!({ "type": "string" }),
            "{name}"
        );
    }
    assert_eq!(
        headers["Access-Control-Allow-Methods"]["schema"]["enum"],
        json!(["GET"])
    );
}
