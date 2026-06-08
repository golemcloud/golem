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
use golem_common::base_model::agent::{
    BinaryDescriptor, BinaryType, ComponentModelElementSchema, DataSchema, ElementSchema,
    NamedElementSchema, NamedElementSchemas, TextDescriptor, TextType,
};
use golem_common::model::account::AccountId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::custom_api::{
    CallAgentBehaviour, CorsOptions, MethodParameter, OpenApiSpecBehaviour, OpenApiSpecFormat,
    PathSegment, PathSegmentType, QueryOrHeaderType, RequestBodySchema, WebhookCallbackBehaviour,
};
use golem_service_base::model::SafeIndex;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::analysis::analysed_type::{field, option, record, result, result_err, str};
use http::Method;
use serde_json::{Value, json};
use test_r::test;

// --------------------------------------------------------------------------
// Route construction helpers
// --------------------------------------------------------------------------

fn agent_type_name(name: &str) -> golem_common::model::agent::AgentTypeName {
    golem_common::model::agent::AgentTypeName(name.to_string())
}

/// A single-element tuple response carrying a component-model type.
fn cm_response(ty: AnalysedType) -> DataSchema {
    DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: "body".to_string(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema { element_type: ty }),
        }],
    })
}

fn unit_response() -> DataSchema {
    DataSchema::Tuple(NamedElementSchemas { elements: vec![] })
}

fn text_response(restrictions: Option<Vec<TextType>>) -> DataSchema {
    DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: "body".to_string(),
            schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions }),
        }],
    })
}

fn binary_response(restrictions: Option<Vec<BinaryType>>) -> DataSchema {
    DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: "body".to_string(),
            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions }),
        }],
    })
}

fn multimodal_response() -> DataSchema {
    DataSchema::Multimodal(NamedElementSchemas { elements: vec![] })
}

#[allow(clippy::too_many_arguments)]
fn call_agent_route(
    method: Method,
    path: Vec<PathSegment>,
    body: RequestBodySchema,
    method_parameters: Vec<MethodParameter>,
    response: DataSchema,
    method_description: Option<String>,
) -> RichCompiledRoute {
    RichCompiledRoute {
        account_id: AccountId::new(),
        environment_id: EnvironmentId::new(),
        route_id: 0,
        method,
        path,
        body,
        behavior: RichRouteBehaviour::CallAgent(CallAgentBehaviour {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            agent_type: agent_type_name("TestAgent"),
            constructor_parameters: vec![],
            phantom: false,
            method_name: "test_method".to_string(),
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
    HttpApiOpenApiSpec::from_routes(&routes, &Domain("example.com".to_string()))
        .expect("spec generation succeeds")
        .0
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
    assert_eq!(spec["servers"][0]["url"], json!("https://example.com"));
    assert_eq!(spec["servers"][1]["url"], json!("http://example.com"));
    // No named types in these routes → components has no schemas.
    assert!(spec["components"].get("schemas").is_none());
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
fn result_void_err_maps_500_to_unknown_binary() {
    // result<ok, _> with no error type → 200 json (ok) + 500 `*/*` binary.
    let op = operation_for(
        call_agent_route(
            Method::GET,
            vec![PathSegment::Literal {
                value: "rok".to_string(),
            }],
            RequestBodySchema::Unused,
            vec![],
            cm_response(golem_wasm::analysis::analysed_type::result_ok(record(
                vec![field("value", str())],
            ))),
            None,
        ),
        "/rok",
        "get",
    );
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        json!("object")
    );
    assert_eq!(
        op["responses"]["500"]["content"]["*/*"]["schema"],
        json!({ "type": "string", "format": "binary" })
    );
}

#[test]
fn result_void_ok_maps_204_to_unknown_binary() {
    // result<_, err> with no ok type → 204 `*/*` binary + 500 json (err).
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
    assert_eq!(
        op["responses"]["204"]["content"]["*/*"]["schema"],
        json!({ "type": "string", "format": "binary" })
    );
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
            RequestBodySchema::JsonBody {
                expected_type: record(vec![field("name", str())]),
            },
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
            RequestBodySchema::UnrestrictedText,
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
            RequestBodySchema::RestrictedBinary {
                allowed_mime_types: vec!["image/gif".to_string()],
            },
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
    body: RequestBodySchema,
    behavior: RichRouteBehaviour,
) -> RichCompiledRoute {
    RichCompiledRoute {
        account_id: AccountId::new(),
        environment_id: EnvironmentId::new(),
        route_id: 0,
        method,
        path,
        body,
        behavior,
        security: RichRouteSecurity::None,
        cors: CorsOptions {
            allowed_patterns: vec![],
        },
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
            RequestBodySchema::UnrestrictedBinary,
            RichRouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
                component_id: ComponentId::new(),
            }),
        ),
        "/webhooks/{promise-id}",
        "post",
    );
    assert!(op["responses"]["204"].is_object());
    assert!(op["responses"]["404"].is_object());
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
            RequestBodySchema::Unused,
            RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Json,
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
fn named_type_shared_across_routes_appears_once_in_components() {
    // A named record used as both a request body and a response should appear
    // exactly once in components/schemas, referenced by `$ref`.
    let named = AnalysedType::Record(golem_wasm::analysis::TypeRecord {
        name: Some("User".to_string()),
        owner: None,
        fields: vec![field("id", str())],
    });

    let route_a = call_agent_route(
        Method::POST,
        vec![PathSegment::Literal {
            value: "a".to_string(),
        }],
        RequestBodySchema::JsonBody {
            expected_type: named.clone(),
        },
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
        cm_response(named),
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
        1,
        "named type should appear exactly once, got keys: {:?}",
        schemas.keys().collect::<Vec<_>>()
    );

    // Request body references the component via `$ref`.
    let body_schema =
        &spec["paths"]["/a"]["post"]["requestBody"]["content"]["application/json"]["schema"];
    assert!(
        body_schema["$ref"]
            .as_str()
            .unwrap()
            .starts_with("#/components/schemas/"),
        "request body should reference the component, got: {body_schema}"
    );
    // The response on the other route references the same component.
    let response_schema =
        &spec["paths"]["/b"]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(response_schema["$ref"], body_schema["$ref"]);
}

#[test]
fn restricted_text_body_content_language_lists_languages() {
    let op = operation_for(
        call_agent_route(
            Method::POST,
            vec![PathSegment::Literal {
                value: "rtxt".to_string(),
            }],
            RequestBodySchema::RestrictedText {
                allowed_language_codes: vec!["en".to_string(), "hu".to_string()],
            },
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
            RequestBodySchema::UnrestrictedBinary,
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
            RequestBodySchema::Unused,
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
