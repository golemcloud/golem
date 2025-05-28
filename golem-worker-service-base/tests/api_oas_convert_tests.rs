// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use chrono::Utc;
use golem_service_base::auth::DefaultNamespace;
use golem_worker_service_base::gateway_api_definition::http::api_oas_convert::OpenApiHttpApiDefinitionResponse;
use golem_worker_service_base::gateway_api_definition::http::{
    AllPathPatterns, CompiledHttpApiDefinition, CompiledRoute, MethodPattern,
};
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::gateway_binding::{
    gateway_binding_compiled::GatewayBindingCompiled, StaticBinding, SwaggerUiBinding,
};
use golem_worker_service_base::gateway_middleware::HttpCors;
use std::str::FromStr;

// Helper function to assert basic OpenAPI properties
fn assert_basic_openapi_properties(yaml: &serde_yaml::Value, id: &str, version: &str) {
    assert_eq!(yaml["openapi"], "3.0.0");
    assert_eq!(yaml["info"]["title"], id);
    assert_eq!(yaml["info"]["version"], version);
    assert_eq!(yaml["x-golem-api-definition-id"], id);
    assert_eq!(yaml["x-golem-api-definition-version"], version);
}

#[test]
fn test_simple_conversion() {
    // Create a simple CompiledHttpApiDefinition with no routes
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("shopping-cart".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "shopping-cart", "0.0.1");

    // Verify empty paths
    assert!(yaml_value["paths"].is_mapping());
    assert!(yaml_value["paths"].as_mapping().unwrap().is_empty());
}

// Test that the conversion works for a CORS preflight route
// Test cors-preflight is converted to rib valid response string
#[test]
fn test_cors_preflight_response_formatting() {
    // Create a CORS configuration using the constructor
    let cors = HttpCors::new(
        "*",
        "GET, POST, PUT, DELETE, OPTIONS",
        "Content-Type, Authorization",
        Some("X-Request-ID"),
        Some(true),
        Some(8400),
    );

    // Create a compiled route with CORS preflight binding
    let route = CompiledRoute {
        method: MethodPattern::Options,
        path: AllPathPatterns::from_str("/v0.1.0/api/resource").unwrap(),
        binding: GatewayBindingCompiled::Static(StaticBinding::HttpCorsPreflight(Box::new(cors))),
        middlewares: None,
    };

    // Create a CompiledHttpApiDefinition with the CORS route
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("api-with-cors".to_string()),
        version: ApiVersion("0.1.0".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify CORS binding
    let cors_binding =
        &yaml_value["paths"]["/v0.1.0/api/resource"]["options"]["x-golem-api-gateway-binding"];
    assert_eq!(cors_binding["binding-type"], "cors-preflight");

    // Verify the entire CORS response string
    // The response is properly within path/binding,
    // and converts the cors-preflight data to a string
    let expected_response = r#"{
  Access-Control-Allow-Headers: "Content-Type, Authorization",
  Access-Control-Allow-Methods: "GET, POST, PUT, DELETE, OPTIONS",
  Access-Control-Allow-Origin: "*",
  Access-Control-Expose-Headers: "X-Request-ID",
  Access-Control-Allow-Credentials: true,
  Access-Control-Max-Age: 8400u64
}"#;
    assert_eq!(
        cors_binding["response"].as_str().unwrap(),
        expected_response
    );

    // Verify the response structure matches the YAML example
    let responses = &yaml_value["paths"]["/v0.1.0/api/resource"]["options"]["responses"];
    assert!(responses.is_mapping());
    assert!(responses["200"].is_mapping());
    assert_eq!(responses["200"]["description"], "OK");

    // CORS preflight responses should not have content since there's no response body
    assert!(!responses["200"]
        .as_mapping()
        .unwrap()
        .contains_key("content"));
}

// Test that the conversion works for a swagger-ui binding type
#[test]
fn test_swagger_ui_binding() {
    // Create a compiled route with SwaggerUI binding
    let route = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/v0.1.0/swagger-ui").unwrap(),
        binding: GatewayBindingCompiled::SwaggerUi(SwaggerUiBinding::default()),
        middlewares: None,
    };

    // Create a CompiledHttpApiDefinition with the SwaggerUI route
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("swagger-api".to_string()),
        version: ApiVersion("0.1.0".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "swagger-api", "0.1.0");

    // Verify SwaggerUI binding
    let swagger_binding =
        &yaml_value["paths"]["/v0.1.0/swagger-ui"]["get"]["x-golem-api-gateway-binding"];
    assert_eq!(swagger_binding["binding-type"], "swagger-ui");

    // Verify the response structure matches GET 200
    let responses = &yaml_value["paths"]["/v0.1.0/swagger-ui"]["get"]["responses"];
    assert!(responses.is_mapping());
    assert!(responses["200"].is_mapping());
    assert_eq!(responses["200"]["description"], "OK");

    // SwaggerUI responses should not have content since there's no response body schema
    assert!(!responses["200"]
        .as_mapping()
        .unwrap()
        .contains_key("content"));
}

// Test basic worker binding with path parameters
#[test]
fn test_basic_worker_binding_with_path_parameters() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr};
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create RIB input type info for worker name (path parameters)
    let worker_name_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "path".to_string(),
                    typ: path_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create worker name compiled
    let worker_name_compiled = WorkerNameCompiled {
        worker_name: Expr::literal("worker-test"),
        compiled_worker_name: RibByteCode::default(),
        rib_input_type_info: worker_name_input,
    };

    // Create response mapping compiled
    let response_compiled = ResponseMappingCompiled {
        response_mapping_expr: Expr::literal("{status: 200, body: \"success\"}"),
        response_mapping_compiled: RibByteCode::default(),
        rib_input: RibInputTypeInfo {
            types: HashMap::new(),
        },
        worker_calls: None,
        rib_output: None,
    };

    // Create component ID using a UUID
    let component_id = VersionedComponentId {
        component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
        version: 0,
    };

    // Create worker binding
    let worker_binding = WorkerBindingCompiled {
        component_id,
        worker_name_compiled: Some(worker_name_compiled),
        idempotency_key_compiled: None,
        response_compiled,
        invocation_context_compiled: None,
    };

    // Create a compiled route with worker binding
    let route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.1.0/{user}/action").unwrap(),
        binding: GatewayBindingCompiled::Worker(worker_binding),
        middlewares: None,
    };

    // Create a CompiledHttpApiDefinition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("test-worker-api".to_string()),
        version: ApiVersion("0.1.0".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "test-worker-api", "0.1.0");

    // Verify path exists
    assert!(yaml_value["paths"]["/v0.1.0/{user}/action"].is_mapping());

    // Verify POST operation
    let post_op = &yaml_value["paths"]["/v0.1.0/{user}/action"]["post"];
    assert!(post_op.is_mapping());

    // Verify path parameters
    let parameters = &post_op["parameters"];
    assert!(parameters.is_sequence());
    assert_eq!(parameters[0]["name"], "user");
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(parameters[0]["schema"]["type"], "string");

    // Verify binding information
    let binding = &post_op["x-golem-api-gateway-binding"];
    assert_eq!(binding["binding-type"], "default");
    assert_eq!(
        binding["component-name"],
        "550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(binding["component-version"], 0);
    assert_eq!(binding["worker-name"], "\"worker-test\"");
    assert_eq!(binding["response"], "\"{status: 200, body: \"success\"}\"");
}

// Test for empty routes but with verification of all basic OpenAPI structure
#[test]
fn test_empty_api_with_complete_structure_verification() {
    // Create a simple CompiledHttpApiDefinition with no routes
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("empty-api".to_string()),
        version: ApiVersion("1.0.0".to_string()),
        routes: vec![],
        draft: true, // Test with draft = true
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify all basic OpenAPI 3.0.0 structure
    assert_eq!(yaml_value["openapi"], "3.0.0");

    // Verify info section
    assert!(yaml_value["info"].is_mapping());
    assert_eq!(yaml_value["info"]["title"], "empty-api");
    assert_eq!(yaml_value["info"]["version"], "1.0.0");

    // Verify Golem extensions
    assert_eq!(yaml_value["x-golem-api-definition-id"], "empty-api");
    assert_eq!(yaml_value["x-golem-api-definition-version"], "1.0.0");

    // Verify paths section exists and is empty
    assert!(yaml_value["paths"].is_mapping());
    assert!(yaml_value["paths"].as_mapping().unwrap().is_empty());

    // Verify components section exists
    assert!(yaml_value["components"].is_mapping());

    // Verify no security schemes when there are no routes
    let components = yaml_value["components"].as_mapping().unwrap();
    if components.contains_key("securitySchemes") {
        assert!(components["securitySchemes"]
            .as_mapping()
            .unwrap()
            .is_empty());
    }

    // Verify no global security when there are no routes
    if yaml_value.as_mapping().unwrap().contains_key("security") {
        assert!(yaml_value["security"].as_sequence().unwrap().is_empty());
    }
}

// Test 6: Basic types and record conversion
#[test]
fn test_basic_types_and_record_conversion() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{
        AnalysedType, NameTypePair, TypeBool, TypeEnum, TypeRecord, TypeStr, TypeU32, TypeU64,
    };
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Helper function to create RIB input with body field
    let create_body_input = |body_type: AnalysedType| -> RibInputTypeInfo {
        let mut types = HashMap::new();
        let body_record = AnalysedType::Record(TypeRecord {
            fields: vec![NameTypePair {
                name: "input".to_string(),
                typ: body_type,
            }],
        });
        let request_record = AnalysedType::Record(TypeRecord {
            fields: vec![NameTypePair {
                name: "body".to_string(),
                typ: body_record,
            }],
        });
        types.insert("request".to_string(), request_record);
        RibInputTypeInfo { types }
    };

    // Helper function to create RIB output with status and body
    let create_output = |body_type: AnalysedType| -> RibOutputTypeInfo {
        RibOutputTypeInfo {
            analysed_type: AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "status".to_string(),
                        typ: AnalysedType::U64(TypeU64),
                    },
                    NameTypePair {
                        name: "body".to_string(),
                        typ: body_type,
                    },
                ],
            }),
        }
    };

    // Create path parameter input (user parameter)
    let path_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "path".to_string(),
                    typ: path_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Test routes for different types
    let mut routes = Vec::new();

    // U64 route
    let u64_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("worker-test"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: path_input.clone(),
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: create_body_input(AnalysedType::U64(TypeU64)),
            worker_calls: None,
            rib_output: Some(create_output(AnalysedType::U64(TypeU64))),
        },
        invocation_context_compiled: None,
    };

    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/u64").unwrap(),
        binding: GatewayBindingCompiled::Worker(u64_binding),
        middlewares: None,
    });

    // Bool route
    let bool_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("worker-test"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: path_input.clone(),
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: create_body_input(AnalysedType::Bool(TypeBool)),
            worker_calls: None,
            rib_output: Some(create_output(AnalysedType::Bool(TypeBool))),
        },
        invocation_context_compiled: None,
    };

    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/bool").unwrap(),
        binding: GatewayBindingCompiled::Worker(bool_binding),
        middlewares: None,
    });

    // Record route
    let record_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "name".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "active".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
        ],
    });

    let record_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("worker-test"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: path_input.clone(),
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: create_body_input(record_type.clone()),
            worker_calls: None,
            rib_output: Some(create_output(record_type)),
        },
        invocation_context_compiled: None,
    };

    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/record").unwrap(),
        binding: GatewayBindingCompiled::Worker(record_binding),
        middlewares: None,
    });

    // Enum route
    let enum_type = AnalysedType::Enum(TypeEnum {
        cases: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
    });

    let enum_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("worker-test"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: path_input.clone(),
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: create_body_input(enum_type.clone()),
            worker_calls: None,
            rib_output: Some(create_output(enum_type)),
        },
        invocation_context_compiled: None,
    };

    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/priority").unwrap(),
        binding: GatewayBindingCompiled::Worker(enum_binding),
        middlewares: None,
    });

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("simple-echo".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes,
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "simple-echo", "0.0.1");

    // Verify u64 route
    let u64_route = &yaml_value["paths"]["/v0.0.1/{user}/u64"]["post"];
    let u64_schema = &u64_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(u64_schema["properties"]["input"]["type"], "integer");
    assert_eq!(u64_schema["properties"]["input"]["format"], "int64");

    let u64_response = &u64_route["responses"]["201"]["content"]["application/json"]["schema"];
    assert_eq!(u64_response["type"], "integer");
    assert_eq!(u64_response["format"], "int64");

    // Verify bool route
    let bool_route = &yaml_value["paths"]["/v0.0.1/{user}/bool"]["post"];
    let bool_schema = &bool_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(bool_schema["properties"]["input"]["type"], "boolean");

    let bool_response = &bool_route["responses"]["201"]["content"]["application/json"]["schema"];
    assert_eq!(bool_response["type"], "boolean");

    // Verify record route
    let record_route = &yaml_value["paths"]["/v0.0.1/{user}/record"]["post"];
    let record_schema = &record_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(record_schema["properties"]["input"]["type"], "object");
    assert_eq!(
        record_schema["properties"]["input"]["properties"]["id"]["type"],
        "integer"
    );
    assert_eq!(
        record_schema["properties"]["input"]["properties"]["id"]["format"],
        "int32"
    );
    assert_eq!(
        record_schema["properties"]["input"]["properties"]["name"]["type"],
        "string"
    );
    assert_eq!(
        record_schema["properties"]["input"]["properties"]["active"]["type"],
        "boolean"
    );

    // Verify enum route
    let enum_route = &yaml_value["paths"]["/v0.0.1/{user}/priority"]["post"];
    let enum_schema = &enum_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(enum_schema["properties"]["input"]["type"], "string");
    assert!(enum_schema["properties"]["input"]["enum"].is_sequence());
    let enum_values = enum_schema["properties"]["input"]["enum"]
        .as_sequence()
        .unwrap();
    assert_eq!(enum_values.len(), 3);
    assert!(enum_values.contains(&serde_yaml::Value::String("low".to_string())));
    assert!(enum_values.contains(&serde_yaml::Value::String("medium".to_string())));
    assert!(enum_values.contains(&serde_yaml::Value::String("high".to_string())));
}

// Test 7: Complete todo structure with optional and oneOf
#[test]
fn test_complete_todo_structure_with_optional_and_oneof() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{
        AnalysedType, NameTypePair, TypeEnum, TypeOption, TypeRecord, TypeResult, TypeS64, TypeStr,
        TypeU64,
    };
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create path parameter input
    let path_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "path".to_string(),
                    typ: path_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create request body with optional field
    let request_body_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "title".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "priority".to_string(),
                typ: AnalysedType::Enum(TypeEnum {
                    cases: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
                }),
            },
            NameTypePair {
                name: "deadline".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    });

    let request_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let body_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "input".to_string(),
                    typ: request_body_type,
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "body".to_string(),
                    typ: body_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create response with Result type (oneOf)
    let todo_record = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "title".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "priority".to_string(),
                typ: AnalysedType::Enum(TypeEnum {
                    cases: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
                }),
            },
            NameTypePair {
                name: "status".to_string(),
                typ: AnalysedType::Enum(TypeEnum {
                    cases: vec![
                        "backlog".to_string(),
                        "in-progress".to_string(),
                        "done".to_string(),
                    ],
                }),
            },
            NameTypePair {
                name: "created-timestamp".to_string(),
                typ: AnalysedType::S64(TypeS64),
            },
            NameTypePair {
                name: "updated-timestamp".to_string(),
                typ: AnalysedType::S64(TypeS64),
            },
            NameTypePair {
                name: "deadline".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::S64(TypeS64)),
                }),
            },
        ],
    });

    let response_type = AnalysedType::Result(TypeResult {
        ok: Some(Box::new(todo_record)),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });

    let response_output = RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "body".to_string(),
                    typ: response_type,
                },
            ],
        }),
    };

    // Create worker binding
    let worker_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("user-worker"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: path_input,
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: request_input,
            worker_calls: None,
            rib_output: Some(response_output),
        },
        invocation_context_compiled: None,
    };

    // Create route
    let route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/add").unwrap(),
        binding: GatewayBindingCompiled::Worker(worker_binding),
        middlewares: None,
    };

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("todo-list".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "todo-list", "0.0.1");

    // Verify path parameter
    let post_op = &yaml_value["paths"]["/v0.0.1/{user}/add"]["post"];
    let parameters = &post_op["parameters"];
    assert_eq!(parameters[0]["name"], "user");
    assert_eq!(parameters[0]["in"], "path");

    // Verify request body with optional field
    let request_schema = &post_op["requestBody"]["content"]["application/json"]["schema"];
    let input_props = &request_schema["properties"]["input"]["properties"];

    // Required fields
    assert_eq!(input_props["title"]["type"], "string");
    assert_eq!(input_props["priority"]["type"], "string");

    // Optional field (nullable)
    assert_eq!(input_props["deadline"]["nullable"], true);
    assert_eq!(input_props["deadline"]["type"], "string");

    // Verify response with oneOf (Result type)
    let response_schema = &post_op["responses"]["201"]["content"]["application/json"]["schema"];
    assert!(response_schema["oneOf"].is_sequence());
    assert_eq!(response_schema["oneOf"].as_sequence().unwrap().len(), 2);

    // Verify success case
    let ok_case = &response_schema["oneOf"][0];
    assert!(ok_case["properties"]["ok"].is_mapping());
    let ok_props = &ok_case["properties"]["ok"]["properties"];
    assert_eq!(ok_props["id"]["type"], "string");
    assert_eq!(ok_props["title"]["type"], "string");
    assert_eq!(ok_props["deadline"]["nullable"], true);
    assert_eq!(ok_props["deadline"]["type"], "integer");

    // Verify error case
    let err_case = &response_schema["oneOf"][1];
    assert!(err_case["properties"]["err"].is_mapping());
    assert_eq!(err_case["properties"]["err"]["type"], "string");
}

// Test 8: Multiple path parameters (user and time)
#[test]
fn test_user_time_conversion() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr, TypeU32};
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create RIB input with both user and time path parameters
    let path_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "user".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "time".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                ],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "path".to_string(),
                    typ: path_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create worker binding
    let worker_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("worker-delay"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: path_input,
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: \"delayed response\"}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: RibInputTypeInfo {
                types: HashMap::new(),
            },
            worker_calls: None,
            rib_output: None,
        },
        invocation_context_compiled: None,
    };

    // Create route
    let route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/{time}/delay-echo").unwrap(),
        binding: GatewayBindingCompiled::Worker(worker_binding),
        middlewares: None,
    };

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("delay-echo".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "delay-echo", "0.0.1");

    // Verify POST operation exists
    let post_op = &yaml_value["paths"]["/v0.0.1/{user}/{time}/delay-echo"]["post"];
    assert!(post_op.is_mapping());

    // Verify both path parameters
    let parameters = &post_op["parameters"];
    assert!(parameters.is_sequence());
    assert_eq!(parameters.as_sequence().unwrap().len(), 2);

    // Verify user parameter
    assert_eq!(parameters[0]["name"], "user");
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(parameters[0]["schema"]["type"], "string");

    // Verify time parameter
    assert_eq!(parameters[1]["name"], "time");
    assert_eq!(parameters[1]["in"], "path");
    assert_eq!(parameters[1]["required"], true);
    assert_eq!(parameters[1]["schema"]["type"], "integer");
    assert_eq!(parameters[1]["schema"]["format"], "int32");

    // Verify binding information
    let binding = &post_op["x-golem-api-gateway-binding"];
    assert_eq!(binding["binding-type"], "default");
    assert_eq!(
        binding["component-name"],
        "550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(binding["component-version"], 0);
    assert_eq!(binding["worker-name"], "\"worker-delay\"");
}

// Test 9: Query parameter conversion
#[test]
fn test_query_parameter_conversion() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr, TypeU32};
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create RIB input with query parameter
    let query_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let query_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "echo".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "query".to_string(),
                    typ: query_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create response output
    let response_output = RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "body".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        }),
    };

    // Create worker binding
    let worker_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("worker-static"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: RibInputTypeInfo {
                types: HashMap::new(),
            },
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: query_input,
            worker_calls: None,
            rib_output: Some(response_output),
        },
        invocation_context_compiled: None,
    };

    // Create route
    let route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.3/echo-query").unwrap(),
        binding: GatewayBindingCompiled::Worker(worker_binding),
        middlewares: None,
    };

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("delay-echo".to_string()),
        version: ApiVersion("0.0.3".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "delay-echo", "0.0.3");

    // Verify POST operation exists
    let post_op = &yaml_value["paths"]["/v0.0.3/echo-query"]["post"];
    assert!(post_op.is_mapping());

    // Verify query parameter
    let parameters = &post_op["parameters"];
    assert!(parameters.is_sequence());
    assert_eq!(parameters.as_sequence().unwrap().len(), 1);

    // Verify echo query parameter
    assert_eq!(parameters[0]["name"], "echo");
    assert_eq!(parameters[0]["in"], "query");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(parameters[0]["schema"]["type"], "string");

    // Verify binding information
    let binding = &post_op["x-golem-api-gateway-binding"];
    assert_eq!(binding["binding-type"], "default");
    assert_eq!(binding["worker-name"], "\"worker-static\"");

    // Verify response schema
    let response_schema = &post_op["responses"]["201"]["content"]["application/json"]["schema"];
    assert_eq!(response_schema["type"], "string");
}

// Test 10: Security conversion
#[test]
fn test_security_conversion() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use golem_worker_service_base::gateway_middleware::{
        HttpAuthenticationMiddleware, HttpMiddleware, HttpMiddlewares,
    };
    use golem_worker_service_base::gateway_security::{
        Provider, SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata,
    };
    use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
    use rib::{Expr, RibByteCode, RibInputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create worker binding for secure route
    let create_secure_binding = |worker_name: &str| -> WorkerBindingCompiled {
        WorkerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000")
                    .unwrap(),
                version: 1,
            },
            worker_name_compiled: Some(WorkerNameCompiled {
                worker_name: Expr::literal(worker_name),
                compiled_worker_name: RibByteCode::default(),
                rib_input_type_info: RibInputTypeInfo {
                    types: HashMap::new(),
                },
            }),
            idempotency_key_compiled: None,
            response_compiled: ResponseMappingCompiled {
                response_mapping_expr: Expr::literal("{status: 200, body: \"secure data\"}"),
                response_mapping_compiled: RibByteCode::default(),
                rib_input: RibInputTypeInfo {
                    types: HashMap::new(),
                },
                worker_calls: None,
                rib_output: None,
            },
            invocation_context_compiled: None,
        }
    };

    // Create minimal security scheme for testing
    let create_security_scheme = |scheme_name: &str| -> SecurityScheme {
        SecurityScheme::new(
            Provider::Google,
            SecuritySchemeIdentifier::new(scheme_name.to_string()),
            ClientId::new("test-client-id".to_string()),
            ClientSecret::new("test-client-secret".to_string()),
            RedirectUrl::new("http://localhost:8080/auth/callback".to_string()).unwrap(),
            vec![Scope::new("openid".to_string())],
        )
    };

    // Create routes without middleware first, then add middleware using HttpMiddlewares
    let mut routes = Vec::new();

    // First secure route with api-key-auth
    let api_key_auth_scheme = create_security_scheme("api-key-auth");
    let api_key_middleware = HttpAuthenticationMiddleware {
        security_scheme_with_metadata: SecuritySchemeWithProviderMetadata {
            security_scheme: api_key_auth_scheme,
            provider_metadata: serde_json::from_str(
                r#"{
                "issuer": "https://accounts.google.com",
                "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
                "jwks_uri": "https://www.googleapis.com/oauth2/v3/certs",
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"]
            }"#,
            )
            .unwrap(),
        },
    };

    let route1 = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/v0.1.0/secure-resource").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_secure_binding("secure-worker")),
        middlewares: Some(HttpMiddlewares(vec![HttpMiddleware::authenticate_request(
            api_key_middleware.security_scheme_with_metadata,
        )])),
    };
    routes.push(route1);

    // Second secure route with jwt-auth
    let jwt_auth_scheme = create_security_scheme("jwt-auth");
    let jwt_middleware = HttpAuthenticationMiddleware {
        security_scheme_with_metadata: SecuritySchemeWithProviderMetadata {
            security_scheme: jwt_auth_scheme,
            provider_metadata: serde_json::from_str(
                r#"{
                "issuer": "https://accounts.google.com",
                "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
                "jwks_uri": "https://www.googleapis.com/oauth2/v3/certs",
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"]
            }"#,
            )
            .unwrap(),
        },
    };

    let route2 = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.1.0/another-secure-resource").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_secure_binding("secure-worker")),
        middlewares: Some(HttpMiddlewares(vec![HttpMiddleware::authenticate_request(
            jwt_middleware.security_scheme_with_metadata,
        )])),
    };
    routes.push(route2);

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("secure-api".to_string()),
        version: ApiVersion("0.1.0".to_string()),
        routes,
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "secure-api", "0.1.0");

    // Verify security schemes in components
    let security_schemes = &yaml_value["components"]["securitySchemes"];
    assert!(security_schemes.is_mapping());

    // Verify api-key-auth scheme
    assert_eq!(security_schemes["api-key-auth"]["type"], "apiKey");
    assert_eq!(security_schemes["api-key-auth"]["in"], "header");
    assert_eq!(security_schemes["api-key-auth"]["name"], "Authorization");

    // Verify jwt-auth scheme
    assert_eq!(security_schemes["jwt-auth"]["type"], "apiKey");
    assert_eq!(security_schemes["jwt-auth"]["in"], "header");
    assert_eq!(security_schemes["jwt-auth"]["name"], "Authorization");

    // Verify operation-level security
    let get_operation = &yaml_value["paths"]["/v0.1.0/secure-resource"]["get"];
    assert!(get_operation["security"].is_sequence());
    let get_security = get_operation["security"].as_sequence().unwrap();
    assert_eq!(get_security.len(), 1);
    assert!(get_security[0]["api-key-auth"].is_sequence());

    let post_operation = &yaml_value["paths"]["/v0.1.0/another-secure-resource"]["post"];
    assert!(post_operation["security"].is_sequence());
    let post_security = post_operation["security"].as_sequence().unwrap();
    assert_eq!(post_security.len(), 1);
    assert!(post_security[0]["jwt-auth"].is_sequence());

    // Verify global security
    let global_security = &yaml_value["security"];
    assert!(global_security.is_sequence());
    let global_security_array = global_security.as_sequence().unwrap();
    assert_eq!(global_security_array.len(), 2);
    assert!(global_security_array[0]["api-key-auth"].is_sequence());
    assert!(global_security_array[1]["jwt-auth"].is_sequence());
}

// Test 11: Variant output structure
#[test]
fn test_variant_output_structure() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{
        AnalysedType, NameOptionTypePair, NameTypePair, TypeRecord, TypeStr, TypeU64, TypeVariant,
    };
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create request body input
    let request_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let body_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "message".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "body".to_string(),
                    typ: body_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create variant response type
    let variant_type = AnalysedType::Variant(TypeVariant {
        cases: vec![
            NameOptionTypePair {
                name: "rand1".to_string(),
                typ: Some(AnalysedType::Str(TypeStr)),
            },
            NameOptionTypePair {
                name: "rand2".to_string(),
                typ: Some(AnalysedType::Str(TypeStr)),
            },
            NameOptionTypePair {
                name: "rand3".to_string(),
                typ: Some(AnalysedType::Str(TypeStr)),
            },
        ],
    });

    let response_output = RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "body".to_string(),
                    typ: variant_type,
                },
            ],
        }),
    };

    // Create worker binding
    let worker_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: None, // No worker name for this test
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("{status: 200, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: request_input,
            worker_calls: None,
            rib_output: Some(response_output),
        },
        invocation_context_compiled: None,
    };

    // Create route
    let route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.3/echo-firstclass").unwrap(),
        binding: GatewayBindingCompiled::Worker(worker_binding),
        middlewares: None,
    };

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("delay-echo".to_string()),
        version: ApiVersion("0.0.3".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_basic_openapi_properties(&yaml_value, "delay-echo", "0.0.3");

    // Verify request body
    let post_op = &yaml_value["paths"]["/v0.0.3/echo-firstclass"]["post"];
    let request_schema = &post_op["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(request_schema["properties"]["message"]["type"], "string");

    // Verify variant response structure
    let response_schema = &post_op["responses"]["201"]["content"]["application/json"]["schema"];
    assert!(response_schema["oneOf"].is_sequence());
    let one_of_cases = response_schema["oneOf"].as_sequence().unwrap();
    assert_eq!(one_of_cases.len(), 3);

    // Verify each variant case
    let rand1 = &one_of_cases[0];
    assert!(rand1["properties"]["rand1"].is_mapping());
    assert_eq!(rand1["properties"]["rand1"]["type"], "string");
    assert!(rand1["required"]
        .as_sequence()
        .unwrap()
        .contains(&serde_yaml::Value::String("rand1".to_string())));

    let rand2 = &one_of_cases[1];
    assert!(rand2["properties"]["rand2"].is_mapping());
    assert_eq!(rand2["properties"]["rand2"]["type"], "string");
    assert!(rand2["required"]
        .as_sequence()
        .unwrap()
        .contains(&serde_yaml::Value::String("rand2".to_string())));

    let rand3 = &one_of_cases[2];
    assert!(rand3["properties"]["rand3"].is_mapping());
    assert_eq!(rand3["properties"]["rand3"]["type"], "string");
    assert!(rand3["required"]
        .as_sequence()
        .unwrap()
        .contains(&serde_yaml::Value::String("rand3".to_string())));
}

// Test 12: Complete integration test with full YAML comparison
#[test]
fn test_oas_conversion_full_structure_shopping_cart() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{
        AnalysedType, NameTypePair, TypeF32, TypeList, TypeRecord, TypeStr, TypeU32, TypeU64,
    };
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    let mut routes = Vec::new();

    // 1. SwaggerUI route
    let swagger_route = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/v0.0.1/swagger-shopping-cart").unwrap(),
        binding: GatewayBindingCompiled::SwaggerUi(SwaggerUiBinding::default()),
        middlewares: None,
    };
    routes.push(swagger_route);

    // 2. Worker route with array response
    let path_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "path".to_string(),
                    typ: path_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create cart item record type
    let cart_item_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "product-id".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "name".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "price".to_string(),
                typ: AnalysedType::F32(TypeF32),
            },
            NameTypePair {
                name: "quantity".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
        ],
    });

    // Create list of cart items
    let cart_items_list = AnalysedType::List(TypeList {
        inner: Box::new(cart_item_type),
    });

    let response_output = RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "body".to_string(),
                    typ: cart_items_list,
                },
            ],
        }),
    };

    let worker_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
        worker_name_compiled: Some(WorkerNameCompiled {
            worker_name: Expr::literal("worker-${user}"),
            compiled_worker_name: RibByteCode::default(),
            rib_input_type_info: path_input,
        }),
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("let result = golem:shoppingcart/api.{get-cart-contents}();\n{status: 200: u64, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: RibInputTypeInfo { types: HashMap::new() },
            worker_calls: None,
            rib_output: Some(response_output),
        },
        invocation_context_compiled: None,
    };

    let worker_route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/get-cart-contents").unwrap(),
        binding: GatewayBindingCompiled::Worker(worker_binding),
        middlewares: None,
    };
    routes.push(worker_route);

    // 3. CORS preflight route
    let cors = HttpCors::new(
        "*",
        "GET, POST, PUT, DELETE, OPTIONS",
        "Content-Type, Authorization",
        None, // No expose headers
        None, // No allow credentials
        None, // No max age
    );

    let cors_route = CompiledRoute {
        method: MethodPattern::Options,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/get-cart-contents").unwrap(),
        binding: GatewayBindingCompiled::Static(StaticBinding::HttpCorsPreflight(Box::new(cors))),
        middlewares: None,
    };
    routes.push(cors_route);

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("shopping-cart".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes,
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
    )
    .unwrap();

    // Expected complete YAML structure (updated to match actual implementation)
    let expected_yaml = r#"
openapi: 3.0.0
info:
  title: shopping-cart
  version: 0.0.1
paths:
  /v0.0.1/swagger-shopping-cart:
    get:
      responses:
        '200':
          description: OK
        default:
          description: OK
      x-golem-api-gateway-binding:
        binding-type: swagger-ui
  /v0.0.1/{user}/get-cart-contents:
    post:
      parameters:
      - in: path
        name: user
        description: 'Path parameter: user'
        required: true
        schema:
          type: string
        explode: false
        style: simple
      responses:
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: object
                  properties:
                    product-id:
                      type: string
                    name:
                      type: string
                    price:
                      type: number
                      format: float
                    quantity:
                      type: integer
                      format: int32
                      minimum: 0
                  required:
                  - product-id
                  - name
                  - price
                  - quantity
        default:
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: object
                  properties:
                    product-id:
                      type: string
                    name:
                      type: string
                    price:
                      type: number
                      format: float
                    quantity:
                      type: integer
                      format: int32
                      minimum: 0
                  required:
                  - product-id
                  - name
                  - price
                  - quantity
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: 550e8400-e29b-41d4-a716-446655440000
        component-version: 0
        response: "\"let result = golem:shoppingcart/api.{get-cart-contents}();\n{status: 200: u64, body: result}\""
        worker-name: "\"worker-${user}\""
    options:
      responses:
        '200':
          description: OK
        default:
          description: OK
      x-golem-api-gateway-binding:
        binding-type: cors-preflight
        response: |-
          {
            Access-Control-Allow-Headers: "Content-Type, Authorization",
            Access-Control-Allow-Methods: "GET, POST, PUT, DELETE, OPTIONS",
            Access-Control-Allow-Origin: "*"
          }
components: {}
x-golem-api-definition-id: shopping-cart
x-golem-api-definition-version: 0.0.1
"#;

    // Parse both YAMLs for comparison
    let actual_yaml: serde_yaml::Value = serde_yaml::from_str(&openapi_response.openapi_yaml)
        .expect("Failed to parse actual OpenAPI YAML");
    let expected_yaml: serde_yaml::Value =
        serde_yaml::from_str(expected_yaml).expect("Failed to parse expected OpenAPI YAML");

    // Single assert comparing the complete structure
    assert_eq!(actual_yaml, expected_yaml);
}
