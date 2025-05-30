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

use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::ComponentId;
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::model::ComponentName;
use golem_worker_service_base::gateway_api_definition::http::api_oas_convert::OpenApiHttpApiDefinitionResponse;
use golem_worker_service_base::gateway_api_definition::http::{
    AllPathPatterns, CompiledHttpApiDefinition, CompiledRoute, MethodPattern,
};
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::gateway_binding::{
    gateway_binding_compiled::GatewayBindingCompiled, StaticBinding, SwaggerUiBinding,
};
use golem_worker_service_base::gateway_middleware::HttpCors;
use golem_worker_service_base::service::gateway::{ComponentView, ConversionContext};
use std::str::FromStr;

// Dummy conversion context for tests
struct DummyConversionContext;

#[async_trait]
impl ConversionContext for DummyConversionContext {
    async fn component_by_name(&self, _name: &ComponentName) -> Result<ComponentView, String> {
        unimplemented!()
    }

    async fn component_by_id(&self, component_id: &ComponentId) -> Result<ComponentView, String> {
        // Return component views based on the component IDs used in tests
        match component_id.to_string().as_str() {
            "550e8400-e29b-41d4-a716-446655440000" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("shopping-cart".to_string()),
                latest_version: 0,
            }),
            "550e8400-e29b-41d4-a716-446655440001" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("api-with-cors".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440002" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("swagger-api".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440003" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("test-worker-api".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440004" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("empty-api".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440005" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("simple-echo".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440006" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("todo-list".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440007" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("delay-echo".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440008" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("secure-api".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440009" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("parameter-test-api".to_string()),
                latest_version: 1,
            }),
            "550e8400-e29b-41d4-a716-446655440010" => Ok(ComponentView {
                id: component_id.clone(),
                name: ComponentName("comprehensive-types-api".to_string()),
                latest_version: 1,
            }),
            _ => Err(format!("Component not found: {}", component_id)),
        }
    }
}

// Helper function to assert basic OpenAPI properties
fn assert_basic_openapi_properties(yaml: &serde_yaml::Value, id: &str, version: &str) {
    assert_eq!(yaml["openapi"], "3.0.0");
    assert_eq!(yaml["info"]["title"], id);
    assert_eq!(yaml["info"]["version"], version);
    assert_eq!(yaml["x-golem-api-definition-id"], id);
    assert_eq!(yaml["x-golem-api-definition-version"], version);
}

#[tokio::test]
async fn test_simple_conversion() {
    // Create a simple CompiledHttpApiDefinition with no routes
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("shopping-cart".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![],
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_cors_preflight_response_formatting() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
    let expected_response = r#"{access-control-allow-origin: "*", access-control-allow-methods: "GET, POST, PUT, DELETE, OPTIONS", access-control-allow-headers: "Content-Type, Authorization", access-control-allow-credentials: "true", access-control-expose-headers: "X-Request-ID", access-control-max-age: 8400: u64}"#;
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
#[tokio::test]
async fn test_swagger_ui_binding() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_basic_worker_binding_with_path_parameters() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
    assert_eq!(binding["component-name"], "shopping-cart");
    assert_eq!(binding["component-version"], 0);
    assert_eq!(binding["worker-name"], "\"worker-test\"");
    assert_eq!(binding["response"], "\"{status: 200, body: \"success\"}\"");
}

// Test for empty routes but with verification of all basic OpenAPI structure
#[tokio::test]
async fn test_empty_api_with_complete_structure_verification() {
    // Create a simple CompiledHttpApiDefinition with no routes
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("empty-api".to_string()),
        version: ApiVersion("1.0.0".to_string()),
        routes: vec![],
        draft: true, // Test with draft = true
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_basic_types_and_record_conversion() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_complete_todo_structure_with_optional_and_oneof() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_user_time_conversion() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
    assert_eq!(binding["component-name"], "shopping-cart");
    assert_eq!(binding["component-version"], 0);
    assert_eq!(binding["worker-name"], "\"worker-delay\"");
}

// Test 9: Query parameter conversion
#[tokio::test]
async fn test_query_parameter_conversion() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_security_conversion() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_variant_output_structure() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
#[tokio::test]
async fn test_oas_conversion_full_structure_shopping_cart() {
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

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
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
        component-name: shopping-cart
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
          {access-control-allow-origin: "*", access-control-allow-methods: "GET, POST, PUT, DELETE, OPTIONS", access-control-allow-headers: "Content-Type, Authorization"}
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

// Test 13: Path, Query, and Header Parameter Combinations Test
#[tokio::test]
async fn test_path_query_header_parameter_combinations() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{
        AnalysedType, NameTypePair, TypeRecord, TypeStr, TypeU32, TypeU64,
    };
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    let mut routes = Vec::new();

    // Helper function to create path input
    let create_path_input = |path_fields: Vec<(&str, AnalysedType)>| -> RibInputTypeInfo {
        let mut types = HashMap::new();
        let path_record = AnalysedType::Record(TypeRecord {
            fields: path_fields
                .into_iter()
                .map(|(name, typ)| NameTypePair {
                    name: name.to_string(),
                    typ,
                })
                .collect(),
        });
        let request_record = AnalysedType::Record(TypeRecord {
            fields: vec![NameTypePair {
                name: "path".to_string(),
                typ: path_record,
            }],
        });
        types.insert("request".to_string(), request_record);
        RibInputTypeInfo { types }
    };

    // Helper function to create query input
    let create_query_input = |query_fields: Vec<(&str, AnalysedType)>| -> RibInputTypeInfo {
        let mut types = HashMap::new();
        let query_record = AnalysedType::Record(TypeRecord {
            fields: query_fields
                .into_iter()
                .map(|(name, typ)| NameTypePair {
                    name: name.to_string(),
                    typ,
                })
                .collect(),
        });
        let request_record = AnalysedType::Record(TypeRecord {
            fields: vec![NameTypePair {
                name: "query".to_string(),
                typ: query_record,
            }],
        });
        types.insert("request".to_string(), request_record);
        RibInputTypeInfo { types }
    };

    // Helper function to create header input
    let create_header_input = |header_fields: Vec<(&str, AnalysedType)>| -> RibInputTypeInfo {
        let mut types = HashMap::new();
        let header_record = AnalysedType::Record(TypeRecord {
            fields: header_fields
                .into_iter()
                .map(|(name, typ)| NameTypePair {
                    name: name.to_string(),
                    typ,
                })
                .collect(),
        });
        let request_record = AnalysedType::Record(TypeRecord {
            fields: vec![NameTypePair {
                name: "headers".to_string(),
                typ: header_record,
            }],
        });
        types.insert("request".to_string(), request_record);
        RibInputTypeInfo { types }
    };

    // Route 1: Combination of path, query, and header parameters
    let path_input_worker = create_path_input(vec![("user", AnalysedType::Str(TypeStr))]);
    let path_input_response = create_path_input(vec![("user_id", AnalysedType::U32(TypeU32))]);
    let query_input_worker = create_query_input(vec![("limit", AnalysedType::U32(TypeU32))]);
    let query_input_response = create_query_input(vec![("offset", AnalysedType::U32(TypeU32))]);
    let header_input_worker = create_header_input(vec![("age", AnalysedType::Str(TypeStr))]);
    let header_input_response = create_header_input(vec![("country", AnalysedType::Str(TypeStr))]);

    // Combine all input types for worker
    let mut worker_input_types = HashMap::new();
    worker_input_types.extend(path_input_worker.types);
    worker_input_types.extend(query_input_worker.types);
    worker_input_types.extend(header_input_worker.types);

    // Combine all input types for response
    let mut response_input_types = HashMap::new();
    response_input_types.extend(path_input_response.types);
    response_input_types.extend(query_input_response.types);
    response_input_types.extend(header_input_response.types);

    let route1 = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/api/v1/{user}/profile/{user_id}").unwrap(),
        binding: GatewayBindingCompiled::Worker(WorkerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440001")
                    .unwrap(),
                version: 1,
            },
            worker_name_compiled: Some(WorkerNameCompiled {
                worker_name: Expr::literal("worker-${user}"),
                compiled_worker_name: RibByteCode::default(),
                rib_input_type_info: RibInputTypeInfo { types: worker_input_types },
            }),
            idempotency_key_compiled: None,
            response_compiled: ResponseMappingCompiled {
                response_mapping_expr: Expr::literal(
                    "{status: 200, body: \"User profile for ${user_id}\"}",
                ),
                response_mapping_compiled: RibByteCode::default(),
                rib_input: RibInputTypeInfo { types: response_input_types },
                worker_calls: None,
                rib_output: Some(RibOutputTypeInfo {
                    analysed_type: AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair {
                                name: "status".to_string(),
                                typ: AnalysedType::U64(TypeU64),
                            },
                            NameTypePair {
                                name: "body".to_string(),
                                typ: AnalysedType::Str(TypeStr),
                            },
                        ],
                    }),
                }),
            },
            invocation_context_compiled: None,
        }),
        middlewares: None,
    };
    routes.push(route1);

    // Route 2: No parameters (for completeness)
    let route2 = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/api/v1/health").unwrap(),
        binding: GatewayBindingCompiled::Worker(WorkerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440004")
                    .unwrap(),
                version: 1,
            },
            worker_name_compiled: Some(WorkerNameCompiled {
                worker_name: Expr::literal("health-worker"),
                compiled_worker_name: RibByteCode::default(),
                rib_input_type_info: RibInputTypeInfo {
                    types: HashMap::new(),
                },
            }),
            idempotency_key_compiled: None,
            response_compiled: ResponseMappingCompiled {
                response_mapping_expr: Expr::literal("{status: 200, body: \"OK\"}"),
                response_mapping_compiled: RibByteCode::default(),
                rib_input: RibInputTypeInfo {
                    types: HashMap::new(),
                },
                worker_calls: None,
                rib_output: Some(RibOutputTypeInfo {
                    analysed_type: AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair {
                                name: "status".to_string(),
                                typ: AnalysedType::U64(TypeU64),
                            },
                            NameTypePair {
                                name: "body".to_string(),
                                typ: AnalysedType::Str(TypeStr),
                            },
                        ],
                    }),
                }),
            },
            invocation_context_compiled: None,
        }),
        middlewares: None,
    };
    routes.push(route2);

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("parameter-test-api".to_string()),
        version: ApiVersion("1.0.0".to_string()),
        routes,
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
    .unwrap();

    let expected_yaml = r#"
    openapi: 3.0.0
    info:
      title: parameter-test-api
      version: 1.0.0
    paths:
      /api/v1/health:
        get:
          responses:
            default:
              description: OK
              content:
                application/json:
                  schema:
                    type: string
            '200':
              description: OK
              content:
                application/json:
                  schema:
                    type: string
          x-golem-api-gateway-binding:
            binding-type: default
            component-name: empty-api
            component-version: 1
            response: '"{status: 200, body: "OK"}"'
            worker-name: '"health-worker"'
      /api/v1/{user}/profile/{user_id}:
        get:
          parameters:
          - in: header
            name: age
            description: 'Header parameter: age'
            required: true
            schema:
              type: string
            explode: false
            style: simple
          - in: header
            name: country
            description: 'Header parameter: country'
            required: true
            schema:
              type: string
            explode: false
            style: simple
          responses:
            default:
              description: OK
              content:
                application/json:
                  schema:
                    type: string
            '200':
              description: OK
              content:
                application/json:
                  schema:
                    type: string
          x-golem-api-gateway-binding:
            binding-type: default
            component-name: api-with-cors
            component-version: 1
            response: '"{status: 200, body: "User profile for ${user_id}"}"'
            worker-name: '"worker-${user}"'
    components: {}
    x-golem-api-definition-id: parameter-test-api
    x-golem-api-definition-version: 1.0.0
    "#;

    // Parse both YAMLs for comparison
    let actual_yaml: serde_yaml::Value = serde_yaml::from_str(&openapi_response.openapi_yaml)
        .expect("Failed to parse actual OpenAPI YAML");
    let expected_yaml: serde_yaml::Value =
        serde_yaml::from_str(expected_yaml).expect("Failed to parse expected OpenAPI YAML");

    // Single assert comparing the complete structure
    assert_eq!(actual_yaml, expected_yaml);
}


// Test 14: Comprehensive AnalysedType Coverage Test (10 Routes)
#[tokio::test]
async fn test_comprehensive_analysed_type_coverage() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm_ast::analysis::{
        AnalysedType, NameOptionTypePair, NameTypePair, TypeBool, TypeChr, TypeEnum, TypeF32,
        TypeF64, TypeFlags, TypeList, TypeOption, TypeRecord, TypeResult, TypeS32, TypeS64,
        TypeStr, TypeTuple, TypeU32, TypeU64, TypeVariant,
    };
    use golem_worker_service_base::gateway_binding::{
        ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
    };
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    let mut routes = Vec::new();

    // Helper function to create a worker binding with given types
    let create_worker_binding = |component_id: &str,
                                 version: u64,
                                 request_type: Option<AnalysedType>,
                                 response_type: AnalysedType|
     -> WorkerBindingCompiled {
        let request_input = if let Some(req_type) = request_type {
            let mut types = HashMap::new();
            let body_record = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "input".to_string(),
                    typ: req_type,
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
        } else {
            RibInputTypeInfo {
                types: HashMap::new(),
            }
        };

        WorkerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str(component_id).unwrap(),
                version,
            },
            worker_name_compiled: Some(WorkerNameCompiled {
                worker_name: Expr::literal("type-test-worker"),
                compiled_worker_name: RibByteCode::default(),
                rib_input_type_info: RibInputTypeInfo {
                    types: HashMap::new(),
                },
            }),
            idempotency_key_compiled: None,
            response_compiled: ResponseMappingCompiled {
                response_mapping_expr: Expr::literal("{status: 201, body: result}"),
                response_mapping_compiled: RibByteCode::default(),
                rib_input: request_input,
                worker_calls: None,
                rib_output: Some(RibOutputTypeInfo {
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
                }),
            },
            invocation_context_compiled: None,
        }
    };

    // Route 1: Primitive types (boolean, integers, floats, string, char)
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/primitives").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440001",
            1,
            Some(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "boolean_val".to_string(),
                        typ: AnalysedType::Bool(TypeBool),
                    },
                    NameTypePair {
                        name: "u32_val".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                    NameTypePair {
                        name: "s64_val".to_string(),
                        typ: AnalysedType::S64(TypeS64),
                    },
                    NameTypePair {
                        name: "f64_val".to_string(),
                        typ: AnalysedType::F64(TypeF64),
                    },
                    NameTypePair {
                        name: "string_val".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "char_val".to_string(),
                        typ: AnalysedType::Chr(TypeChr),
                    },
                ],
            })),
            AnalysedType::Bool(TypeBool),
        )),
        middlewares: None,
    });

    // Route 2: Collections (list, tuple, option)
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/collections").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440002",
            1,
            Some(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "list_val".to_string(),
                        typ: AnalysedType::List(TypeList {
                            inner: Box::new(AnalysedType::Str(TypeStr)),
                        }),
                    },
                    NameTypePair {
                        name: "tuple_val".to_string(),
                        typ: AnalysedType::Tuple(TypeTuple {
                            items: vec![AnalysedType::Str(TypeStr), AnalysedType::U32(TypeU32)],
                        }),
                    },
                    NameTypePair {
                        name: "optional_val".to_string(),
                        typ: AnalysedType::Option(TypeOption {
                            inner: Box::new(AnalysedType::Str(TypeStr)),
                        }),
                    },
                ],
            })),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::U32(TypeU32)),
            }),
        )),
        middlewares: None,
    });

    // Route 3: Record type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/record").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440003",
            1,
            Some(AnalysedType::Record(TypeRecord {
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
            })),
            AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "result_id".to_string(),
                        typ: AnalysedType::U64(TypeU64),
                    },
                    NameTypePair {
                        name: "message".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
            }),
        )),
        middlewares: None,
    });

    // Route 4: Enum and Flags types
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/enum-flags").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440004",
            1,
            Some(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "color".to_string(),
                        typ: AnalysedType::Enum(TypeEnum {
                            cases: vec!["red".to_string(), "green".to_string(), "blue".to_string()],
                        }),
                    },
                    NameTypePair {
                        name: "permissions".to_string(),
                        typ: AnalysedType::Flags(TypeFlags {
                            names: vec![
                                "read".to_string(),
                                "write".to_string(),
                                "execute".to_string(),
                            ],
                        }),
                    },
                ],
            })),
            AnalysedType::Enum(TypeEnum {
                cases: vec![
                    "success".to_string(),
                    "warning".to_string(),
                    "error".to_string(),
                ],
            }),
        )),
        middlewares: None,
    });

    // Route 5: Result type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/result").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440005",
            1,
            Some(AnalysedType::Result(TypeResult {
                ok: Some(Box::new(AnalysedType::Str(TypeStr))),
                err: Some(Box::new(AnalysedType::Str(TypeStr))),
            })),
            AnalysedType::Result(TypeResult {
                ok: Some(Box::new(AnalysedType::U64(TypeU64))),
                err: Some(Box::new(AnalysedType::Str(TypeStr))),
            }),
        )),
        middlewares: None,
    });

    // Route 6: Variant type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/variant").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440006",
            1,
            Some(AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "text".to_string(),
                        typ: Some(AnalysedType::Str(TypeStr)),
                    },
                    NameOptionTypePair {
                        name: "number".to_string(),
                        typ: Some(AnalysedType::U32(TypeU32)),
                    },
                    NameOptionTypePair {
                        name: "flag".to_string(),
                        typ: None,
                    },
                ],
            })),
            AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "success".to_string(),
                        typ: Some(AnalysedType::Str(TypeStr)),
                    },
                    NameOptionTypePair {
                        name: "error".to_string(),
                        typ: Some(AnalysedType::Str(TypeStr)),
                    },
                ],
            }),
        )),
        middlewares: None,
    });

    // Route 7: Complex nested type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/complex").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440007",
            1,
            Some(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "optional_list".to_string(),
                        typ: AnalysedType::Option(TypeOption {
                            inner: Box::new(AnalysedType::List(TypeList {
                                inner: Box::new(AnalysedType::Str(TypeStr)),
                            })),
                        }),
                    },
                    NameTypePair {
                        name: "result_record".to_string(),
                        typ: AnalysedType::Result(TypeResult {
                            ok: Some(Box::new(AnalysedType::Record(TypeRecord {
                                fields: vec![NameTypePair {
                                    name: "value".to_string(),
                                    typ: AnalysedType::U32(TypeU32),
                                }],
                            }))),
                            err: Some(Box::new(AnalysedType::Str(TypeStr))),
                        }),
                    },
                ],
            })),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "id".to_string(),
                            typ: AnalysedType::U64(TypeU64),
                        },
                        NameTypePair {
                            name: "status".to_string(),
                            typ: AnalysedType::Enum(TypeEnum {
                                cases: vec!["pending".to_string(), "completed".to_string()],
                            }),
                        },
                    ],
                })),
            }),
        )),
        middlewares: None,
    });

    // Route 8: Different number formats
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/numbers").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440008",
            1,
            Some(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "s32_val".to_string(),
                        typ: AnalysedType::S32(TypeS32),
                    },
                    NameTypePair {
                        name: "f32_val".to_string(),
                        typ: AnalysedType::F32(TypeF32),
                    },
                ],
            })),
            AnalysedType::F32(TypeF32),
        )),
        middlewares: None,
    });

    // Route 9: Array response
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/array-response").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440009",
            1,
            Some(AnalysedType::Str(TypeStr)),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "name".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        NameTypePair {
                            name: "count".to_string(),
                            typ: AnalysedType::U32(TypeU32),
                        },
                    ],
                })),
            }),
        )),
        middlewares: None,
    });

    // Route 10: GET without request body
    routes.push(CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/types/info").unwrap(),
        binding: GatewayBindingCompiled::Worker(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440010",
            1,
            None, // No request body
            AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "api_version".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "supported_types".to_string(),
                        typ: AnalysedType::List(TypeList {
                            inner: Box::new(AnalysedType::Str(TypeStr)),
                        }),
                    },
                ],
            }),
        )),
        middlewares: None,
    });

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("comprehensive-types-api".to_string()),
        version: ApiVersion("2.0.0".to_string()),
        routes,
        draft: false,
        created_at: Utc::now(),
        namespace: DefaultNamespace(),
    };

    // Create dummy conversion context
    let conversion_ctx = DummyConversionContext.boxed();

    // Convert to OpenAPI
    let openapi_response = OpenApiHttpApiDefinitionResponse::from_compiled_http_api_definition(
        &compiled_api_definition,
        &conversion_ctx,
    )
    .await
    .unwrap();

    // Expected complete YAML structure
    let expected_yaml = r#"
openapi: 3.0.0
info:
  title: comprehensive-types-api
  version: 2.0.0
paths:
  /types/array-response:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  type: string
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: object
                  properties:
                    name:
                      type: string
                    count:
                      type: integer
                      format: int32
                      minimum: 0
                  required:
                  - name
                  - count
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: object
                  properties:
                    name:
                      type: string
                    count:
                      type: integer
                      format: int32
                      minimum: 0
                  required:
                  - name
                  - count
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: parameter-test-api
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/collections:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  type: object
                  properties:
                    list_val:
                      type: array
                      items:
                        type: string
                    tuple_val:
                      type: array
                      description: Tuple type
                      items:
                        type: object
                      minItems: 2
                      maxItems: 2
                    optional_val:
                      type: string
                      nullable: true
                  required:
                  - list_val
                  - tuple_val
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: integer
                  format: int32
                  minimum: 0
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: integer
                  format: int32
                  minimum: 0
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: swagger-api
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/complex:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  type: object
                  properties:
                    optional_list:
                      type: array
                      nullable: true
                      items:
                        type: string
                    result_record:
                      oneOf:
                      - type: object
                        properties:
                          ok:
                            type: object
                            properties:
                              value:
                                type: integer
                                format: int32
                                minimum: 0
                            required:
                            - value
                        required:
                        - ok
                      - type: object
                        properties:
                          err:
                            type: string
                        required:
                        - err
                  required:
                  - result_record
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: object
                  properties:
                    id:
                      type: integer
                      format: int64
                      minimum: 0
                    status:
                      type: string
                      enum:
                      - pending
                      - completed
                  required:
                  - id
                  - status
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: array
                items:
                  type: object
                  properties:
                    id:
                      type: integer
                      format: int64
                      minimum: 0
                    status:
                      type: string
                      enum:
                      - pending
                      - completed
                  required:
                  - id
                  - status
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: delay-echo
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/enum-flags:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  type: object
                  properties:
                    color:
                      type: string
                      enum:
                      - red
                      - green
                      - blue
                    permissions:
                      description: Flags type - array of flag names
                      type: array
                      items:
                        type: string
                        enum:
                        - read
                        - write
                        - execute
                      minItems: 0
                      maxItems: 3
                      uniqueItems: true
                  required:
                  - color
                  - permissions
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                type: string
                enum:
                - success
                - warning
                - error
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: string
                enum:
                - success
                - warning
                - error
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: empty-api
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/info:
    get:
      responses:
        default:
          description: OK
          content:
            application/json:
              schema:
                type: object
                properties:
                  api_version:
                    type: string
                  supported_types:
                    type: array
                    items:
                      type: string
                required:
                - api_version
                - supported_types
        '200':
          description: OK
          content:
            application/json:
              schema:
                type: object
                properties:
                  api_version:
                    type: string
                  supported_types:
                    type: array
                    items:
                      type: string
                required:
                - api_version
                - supported_types
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: comprehensive-types-api
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/numbers:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  type: object
                  properties:
                    s32_val:
                      type: integer
                      format: int32
                    f32_val:
                      type: number
                      format: float
                  required:
                  - s32_val
                  - f32_val
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                type: number
                format: float
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: number
                format: float
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: secure-api
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/primitives:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  type: object
                  properties:
                    boolean_val:
                      type: boolean
                    u32_val:
                      type: integer
                      format: int32
                      minimum: 0
                    s64_val:
                      type: integer
                      format: int64
                    f64_val:
                      type: number
                      format: double
                    string_val:
                      type: string
                    char_val:
                      description: Unicode character
                      type: string
                      pattern: ^.{1}$
                      minLength: 1
                      maxLength: 1
                  required:
                  - boolean_val
                  - u32_val
                  - s64_val
                  - f64_val
                  - string_val
                  - char_val
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                type: boolean
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: boolean
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: api-with-cors
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/record:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  type: object
                  properties:
                    id:
                      type: integer
                      format: int32
                      minimum: 0
                    name:
                      type: string
                    active:
                      type: boolean
                  required:
                  - id
                  - name
                  - active
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                type: object
                properties:
                  result_id:
                    type: integer
                    format: int64
                    minimum: 0
                  message:
                    type: string
                required:
                - result_id
                - message
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: object
                properties:
                  result_id:
                    type: integer
                    format: int64
                    minimum: 0
                  message:
                    type: string
                required:
                - result_id
                - message
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: test-worker-api
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/result:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  oneOf:
                  - type: object
                    properties:
                      ok:
                        type: string
                    required:
                    - ok
                  - type: object
                    properties:
                      err:
                        type: string
                    required:
                    - err
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                oneOf:
                - type: object
                  properties:
                    ok:
                      type: integer
                      format: int64
                      minimum: 0
                  required:
                  - ok
                - type: object
                  properties:
                    err:
                      type: string
                  required:
                  - err
        '201':
          description: Created
          content:
            application/json:
              schema:
                oneOf:
                - type: object
                  properties:
                    ok:
                      type: integer
                      format: int64
                      minimum: 0
                  required:
                  - ok
                - type: object
                  properties:
                    err:
                      type: string
                  required:
                  - err
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: simple-echo
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
  /types/variant:
    post:
      requestBody:
        description: Request payload
        content:
          application/json:
            schema:
              type: object
              properties:
                input:
                  oneOf:
                  - type: object
                    properties:
                      text:
                        type: string
                    required:
                    - text
                  - type: object
                    properties:
                      number:
                        type: integer
                        format: int32
                        minimum: 0
                    required:
                    - number
                  - type: string
              required:
              - input
        required: true
      responses:
        default:
          description: Created
          content:
            application/json:
              schema:
                oneOf:
                - type: object
                  properties:
                    success:
                      type: string
                  required:
                  - success
                - type: object
                  properties:
                    error:
                      type: string
                  required:
                  - error
        '201':
          description: Created
          content:
            application/json:
              schema:
                oneOf:
                - type: object
                  properties:
                    success:
                      type: string
                  required:
                  - success
                - type: object
                  properties:
                    error:
                      type: string
                  required:
                  - error
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: todo-list
        component-version: 1
        response: '"{status: 201, body: result}"'
        worker-name: '"type-test-worker"'
components: {}
x-golem-api-definition-id: comprehensive-types-api
x-golem-api-definition-version: 2.0.0
"#;

    // Parse both YAMLs for comparison
    let actual_yaml: serde_yaml::Value = serde_yaml::from_str(&openapi_response.openapi_yaml)
        .expect("Failed to parse actual OpenAPI YAML");
    let expected_yaml: serde_yaml::Value =
        serde_yaml::from_str(expected_yaml).expect("Failed to parse expected OpenAPI YAML");

    // Single assert comparing the complete structure
    assert_eq!(actual_yaml, expected_yaml);
}

