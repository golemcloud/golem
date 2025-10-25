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
use golem_common::model::auth::Namespace;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::{AccountId, ComponentId, ProjectId};
use golem_service_base::model::ComponentName;
use golem_worker_service::gateway_api_definition::http::api_oas_convert::OpenApiHttpApiDefinitionResponse;
use golem_worker_service::gateway_api_definition::http::{
    AllPathPatterns, CompiledHttpApiDefinition, CompiledRoute, MethodPattern,
};
use golem_worker_service::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service::gateway_binding::FileServerBindingCompiled;
use golem_worker_service::gateway_binding::{
    gateway_binding_compiled::GatewayBindingCompiled, HttpHandlerBindingCompiled,
    ResponseMappingCompiled, StaticBinding, SwaggerUiBinding, WorkerNameCompiled,
};
use golem_worker_service::gateway_middleware::HttpCors;
use golem_worker_service::service::gateway::{ComponentView, ConversionContext};
use rib::{Expr, RibByteCode, RibInputTypeInfo};
use std::collections::HashMap;
use std::str::FromStr;
use test_r::test;

test_r::enable!();

// Helper function to create test namespace
fn test_namespace() -> Namespace {
    Namespace::new(
        ProjectId::from_str("44f28456-d0c2-45d2-aaad-6e85462b6f18").unwrap(),
        AccountId::from("a92803c1-186a-4367-bc00-23faffb5c932"),
    )
}

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
            _ => Err(format!("Component not found: {component_id}")),
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

// Test 1: Simple conversion
#[test]
async fn test_simple_conversion() {
    // Create a simple CompiledHttpApiDefinition with no routes
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("shopping-cart".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![],
        draft: false,
        created_at: Utc::now(),
        namespace: test_namespace(),
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
    assert_eq!(yaml_value["info"]["title"], "shopping-cart");
    assert_eq!(yaml_value["info"]["version"], "0.0.1");

    // Verify Golem extensions
    assert_eq!(yaml_value["x-golem-api-definition-id"], "shopping-cart");
    assert_eq!(yaml_value["x-golem-api-definition-version"], "0.0.1");

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

// Test 2: CORS preflight route
// Test cors-preflight is converted to rib valid response string
#[test]
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
        namespace: test_namespace(),
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

// Test 3: SwaggerUI, FileServer, HttpHandler binding types
#[test]
async fn test_other_binding_types() {
    // Create a compiled route with SwaggerUI binding
    let swagger_ui_route = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/v0.1.0/swagger-ui").unwrap(),
        binding: GatewayBindingCompiled::SwaggerUi(SwaggerUiBinding::default()),
        middlewares: None,
    };

    let file_server_route = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/v0.1.0/fileserver").unwrap(),
        binding: GatewayBindingCompiled::FileServer(Box::new(FileServerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440002")
                    .unwrap(),
                version: 1,
            },
            worker_name_compiled: Some(WorkerNameCompiled {
                worker_name: Expr::literal("file-server-worker"),
                compiled_worker_name: RibByteCode::default(),
                rib_input_type_info: RibInputTypeInfo {
                    types: HashMap::new(),
                },
            }),
            idempotency_key_compiled: None,
            response_compiled: ResponseMappingCompiled {
                response_mapping_expr: Expr::literal("\"file-content\""),
                response_mapping_compiled: RibByteCode::default(),
                rib_input: RibInputTypeInfo {
                    types: HashMap::new(),
                },
                worker_calls: None,
                rib_output: None,
            },
            invocation_context_compiled: None,
        })),
        middlewares: None,
    };

    let http_handler_route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.1.0/http-handler").unwrap(),
        binding: GatewayBindingCompiled::HttpHandler(Box::new(HttpHandlerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440002")
                    .unwrap(),
                version: 1,
            },
            worker_name_compiled: Some(WorkerNameCompiled {
                worker_name: Expr::literal("http-handler-worker"),
                compiled_worker_name: RibByteCode::default(),
                rib_input_type_info: RibInputTypeInfo {
                    types: HashMap::new(),
                },
            }),
            idempotency_key_compiled: None,
        })),
        middlewares: None,
    };

    // Create a CompiledHttpApiDefinition with the SwaggerUI route
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("swagger-api".to_string()),
        version: ApiVersion("0.1.0".to_string()),
        routes: vec![swagger_ui_route, file_server_route, http_handler_route],
        draft: false,
        created_at: Utc::now(),
        namespace: test_namespace(),
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

    // Verify the response structure matches GET 200 and default
    let responses = &yaml_value["paths"]["/v0.1.0/swagger-ui"]["get"]["responses"];
    assert!(responses.is_mapping());
    assert!(responses["200"].is_mapping());
    assert_eq!(responses["200"]["description"], "OK");
    assert!(responses["default"].is_mapping());
    assert_eq!(responses["default"]["description"], "OK");

    // SwaggerUI responses should not have content since there's no response body schema
    assert!(!responses["200"]
        .as_mapping()
        .unwrap()
        .contains_key("content"));

    // Verify file server binding
    let file_server_binding =
        &yaml_value["paths"]["/v0.1.0/fileserver"]["get"]["x-golem-api-gateway-binding"];
    assert_eq!(file_server_binding["binding-type"], "file-server");
    assert_eq!(file_server_binding["component-name"], "swagger-api");
    assert_eq!(file_server_binding["component-version"], 1);
    assert_eq!(file_server_binding["response"], "\"\"file-content\"\"");
    assert_eq!(file_server_binding["worker-name"], "\"file-server-worker\"");

    // Verify file server responses
    let file_server_responses = &yaml_value["paths"]["/v0.1.0/fileserver"]["get"]["responses"];
    assert!(file_server_responses.is_mapping());
    assert!(file_server_responses["200"].is_mapping());
    assert_eq!(file_server_responses["200"]["description"], "OK");
    assert!(file_server_responses["default"].is_mapping());
    assert_eq!(file_server_responses["default"]["description"], "OK");

    // Verify http handler binding
    let http_handler_binding =
        &yaml_value["paths"]["/v0.1.0/http-handler"]["post"]["x-golem-api-gateway-binding"];
    assert_eq!(http_handler_binding["binding-type"], "http-handler");
    assert_eq!(http_handler_binding["component-name"], "swagger-api");
    assert_eq!(http_handler_binding["component-version"], 1);
    assert_eq!(
        http_handler_binding["worker-name"],
        "\"http-handler-worker\""
    );

    // Verify http handler responses
    let http_handler_responses = &yaml_value["paths"]["/v0.1.0/http-handler"]["post"]["responses"];
    assert!(http_handler_responses.is_mapping());
    assert!(http_handler_responses["201"].is_mapping());
    assert_eq!(http_handler_responses["201"]["description"], "Created");
    assert!(http_handler_responses["default"].is_mapping());
    assert_eq!(http_handler_responses["default"]["description"], "Created");
}

// Test 4: Security conversion
#[test]
async fn test_security_conversion() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled}; // WorkerNameCompiled
    use golem_worker_service::gateway_middleware::{
        HttpAuthenticationMiddleware, HttpMiddleware, HttpMiddlewares,
    };
    use golem_worker_service::gateway_security::{
        Provider, SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata,
    };
    use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
    use rib::{Expr, RibByteCode, RibInputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create worker binding for secure route
    let create_secure_binding = |_worker_name: &str| -> WorkerBindingCompiled {
        WorkerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000")
                    .unwrap(),
                version: 1,
            },
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
        binding: GatewayBindingCompiled::Worker(Box::new(create_secure_binding("secure-worker"))),
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
        binding: GatewayBindingCompiled::Worker(Box::new(create_secure_binding("secure-worker"))),
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
        namespace: test_namespace(),
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

    // Expected OpenAPI YAML
    let expected_yaml = r#"openapi: 3.0.0
info:
  title: secure-api
  version: 0.1.0
paths:
  /v0.1.0/another-secure-resource:
    post:
      responses:
        default:
          description: Created
        '201':
          description: Created
      security:
      - jwt-auth: []
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: shopping-cart
        component-version: 1
        response: '"{status: 200, body: "secure data"}"'
  /v0.1.0/secure-resource:
    get:
      responses:
        default:
          description: OK
        '200':
          description: OK
      security:
      - api-key-auth: []
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: shopping-cart
        component-version: 1
        response: '"{status: 200, body: "secure data"}"'
components:
  securitySchemes:
    api-key-auth:
      type: apiKey
      in: header
      name: Authorization
      description: API key security scheme for api-key-auth
    jwt-auth:
      type: apiKey
      in: header
      name: Authorization
      description: API key security scheme for jwt-auth
security:
- api-key-auth: []
- jwt-auth: []
x-golem-api-definition-id: secure-api
x-golem-api-definition-version: 0.1.0
"#;

    // Parse both YAMLs to serde_yaml::Value for structural comparison
    let expected: serde_yaml::Value =
        serde_yaml::from_str(expected_yaml).expect("Failed to parse expected YAML");
    let actual: serde_yaml::Value = serde_yaml::from_str(&openapi_response.openapi_yaml)
        .expect("Failed to parse actual OpenAPI YAML");

    assert_eq!(actual, expected);
}

// Test 5: Multiple component binding
#[test]
async fn test_multi_component_binding() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr, TypeU32};
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled};
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Route 1: Path parameter (user), component shopping-cart
    let worker_name_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "path".to_string(),
                    typ: path_record,
                }],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };
    let response_compiled_shopping_cart = ResponseMappingCompiled {
        response_mapping_expr: Expr::literal("{status: 200, body: \"success\"}"),
        response_mapping_compiled: RibByteCode::default(),
        rib_input: worker_name_input,
        worker_calls: None,
        rib_output: None,
    };
    let component_id_shopping_cart = VersionedComponentId {
        component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
        version: 0,
    };
    let worker_binding_shopping_cart = WorkerBindingCompiled {
        component_id: component_id_shopping_cart,
        idempotency_key_compiled: None,
        response_compiled: response_compiled_shopping_cart,
        invocation_context_compiled: None,
    };
    let route_shopping_cart = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.1.0/{user}/action").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(worker_binding_shopping_cart)),
        middlewares: None,
    };

    // Route 2: Path parameter (user) and query parameter (echo), component delay-echo
    let query_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let query_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "echo".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "path".to_string(),
                        typ: path_record,
                    },
                    NameTypePair {
                        name: "query".to_string(),
                        typ: query_record,
                    },
                ],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };
    let response_output = RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            name: None,
            owner: None,
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
    let response_compiled_delay_echo = ResponseMappingCompiled {
        response_mapping_expr: Expr::literal("{status: 200, body: result}"),
        response_mapping_compiled: RibByteCode::default(),
        rib_input: query_input,
        worker_calls: None,
        rib_output: Some(response_output),
    };
    let component_id_delay_echo = VersionedComponentId {
        component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440007").unwrap(),
        version: 1,
    };
    let worker_binding_delay_echo = WorkerBindingCompiled {
        component_id: component_id_delay_echo,
        idempotency_key_compiled: None,
        response_compiled: response_compiled_delay_echo,
        invocation_context_compiled: None,
    };
    let route_delay_echo = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.2.0/{user}/echo-query").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(worker_binding_delay_echo)),
        middlewares: None,
    };

    // Create a CompiledHttpApiDefinition with both routes
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("test-multi-component-api".to_string()),
        version: ApiVersion("0.2.0".to_string()),
        routes: vec![route_shopping_cart, route_delay_echo],
        draft: false,
        created_at: Utc::now(),
        namespace: test_namespace(),
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
    assert_basic_openapi_properties(&yaml_value, "test-multi-component-api", "0.2.0");

    // --- Route 1 assertions ---
    assert!(yaml_value["paths"]["/v0.1.0/{user}/action"].is_mapping());
    let post_op_shopping_cart = &yaml_value["paths"]["/v0.1.0/{user}/action"]["post"];
    assert!(post_op_shopping_cart.is_mapping());
    let parameters_shopping_cart = &post_op_shopping_cart["parameters"];
    assert!(parameters_shopping_cart.is_sequence());
    assert_eq!(parameters_shopping_cart.as_sequence().unwrap().len(), 1);
    assert_eq!(parameters_shopping_cart[0]["name"], "user");
    assert_eq!(parameters_shopping_cart[0]["in"], "path");
    assert_eq!(parameters_shopping_cart[0]["required"], true);
    assert_eq!(parameters_shopping_cart[0]["schema"]["type"], "string");
    let binding_shopping_cart = &post_op_shopping_cart["x-golem-api-gateway-binding"];
    assert_eq!(binding_shopping_cart["binding-type"], "default");
    assert_eq!(binding_shopping_cart["component-name"], "shopping-cart");
    assert_eq!(binding_shopping_cart["component-version"], 0);
    assert_eq!(
        binding_shopping_cart["response"],
        "\"{status: 200, body: \"success\"}\""
    );

    // --- Route 2 assertions ---
    assert!(yaml_value["paths"]["/v0.2.0/{user}/echo-query"].is_mapping());
    let post_op_delay_echo = &yaml_value["paths"]["/v0.2.0/{user}/echo-query"]["post"];
    assert!(post_op_delay_echo.is_mapping());
    let parameters_delay_echo = &post_op_delay_echo["parameters"];
    assert!(parameters_delay_echo.is_sequence());
    assert_eq!(parameters_delay_echo.as_sequence().unwrap().len(), 2);
    // user path param
    assert_eq!(parameters_delay_echo[0]["name"], "user");
    assert_eq!(parameters_delay_echo[0]["in"], "path");
    assert_eq!(parameters_delay_echo[0]["required"], true);
    assert_eq!(parameters_delay_echo[0]["schema"]["type"], "string");
    // echo query param
    assert_eq!(parameters_delay_echo[1]["name"], "echo");
    assert_eq!(parameters_delay_echo[1]["in"], "query");
    assert_eq!(parameters_delay_echo[1]["required"], true);
    assert_eq!(parameters_delay_echo[1]["schema"]["type"], "string");
    let binding_delay_echo = &post_op_delay_echo["x-golem-api-gateway-binding"];
    assert_eq!(binding_delay_echo["binding-type"], "default");
    assert_eq!(binding_delay_echo["component-name"], "delay-echo");
    assert_eq!(binding_delay_echo["component-version"], 1);
    // Response schema (should be string, as in test_query_parameter_conversion)
    let response_schema_delay_echo =
        &post_op_delay_echo["responses"]["201"]["content"]["application/json"]["schema"];
    assert_eq!(response_schema_delay_echo["type"], "string");
}

// Test 6: Basic types and record conversion
#[test]
async fn test_basic_types_and_record_conversion() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm::analysis::{
        AnalysedType, NameTypePair, TypeBool, TypeEnum, TypeRecord, TypeStr, TypeU32, TypeU64,
    };
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled};
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Helper function to create RIB input with body field
    let create_body_input = |body_type: AnalysedType| -> RibInputTypeInfo {
        let mut types = HashMap::new();
        let body_record = AnalysedType::Record(TypeRecord {
            name: None,
            owner: None,
            fields: vec![NameTypePair {
                name: "input".to_string(),
                typ: body_type,
            }],
        });
        let request_record = AnalysedType::Record(TypeRecord {
            name: None,
            owner: None,
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
                name: None,
                owner: None,
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
    let _path_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let path_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
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
        binding: GatewayBindingCompiled::Worker(Box::new(u64_binding)),
        middlewares: None,
    });

    // Bool route
    let bool_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
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
        binding: GatewayBindingCompiled::Worker(Box::new(bool_binding)),
        middlewares: None,
    });

    // Record route
    let record_type = AnalysedType::Record(TypeRecord {
        name: None,
        owner: None,
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
        binding: GatewayBindingCompiled::Worker(Box::new(record_binding)),
        middlewares: None,
    });

    // Enum route
    let enum_type = AnalysedType::Enum(TypeEnum {
        name: None,
        owner: None,
        cases: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
    });

    let enum_binding = WorkerBindingCompiled {
        component_id: VersionedComponentId {
            component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            version: 0,
        },
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
        binding: GatewayBindingCompiled::Worker(Box::new(enum_binding)),
        middlewares: None,
    });

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("simple-echo".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes,
        draft: false,
        created_at: Utc::now(),
        namespace: test_namespace(),
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
#[test]
async fn test_complete_todo_structure_with_optional_and_oneof() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm::analysis::{
        AnalysedType, NameTypePair, TypeEnum, TypeOption, TypeRecord, TypeResult, TypeS64, TypeStr,
        TypeU64,
    };
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled};
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create request body with optional field
    let request_body_type = AnalysedType::Record(TypeRecord {
        name: None,
        owner: None,
        fields: vec![
            NameTypePair {
                name: "title".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "priority".to_string(),
                typ: AnalysedType::Enum(TypeEnum {
                    name: None,
                    owner: None,
                    cases: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
                }),
            },
            NameTypePair {
                name: "deadline".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    name: None,
                    owner: None,
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    });

    let request_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let body_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "input".to_string(),
                    typ: request_body_type,
                }],
            });
            let path_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "body".to_string(),
                        typ: body_record,
                    },
                    NameTypePair {
                        name: "path".to_string(),
                        typ: path_record,
                    },
                ],
            });
            types.insert("request".to_string(), request_record);
            types
        },
    };

    // Create response with Result type (oneOf)
    let todo_record = AnalysedType::Record(TypeRecord {
        name: None,
        owner: None,
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
                    name: None,
                    owner: None,
                    cases: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
                }),
            },
            NameTypePair {
                name: "status".to_string(),
                typ: AnalysedType::Enum(TypeEnum {
                    name: None,
                    owner: None,
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
                    name: None,
                    owner: None,
                    inner: Box::new(AnalysedType::S64(TypeS64)),
                }),
            },
        ],
    });

    let response_type = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: Some(Box::new(todo_record)),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });

    let response_output = RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            name: None,
            owner: None,
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
        binding: GatewayBindingCompiled::Worker(Box::new(worker_binding)),
        middlewares: None,
    };

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("todo-list".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: test_namespace(),
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

// Test 8: Variant output structure
#[test]
async fn test_variant_output_structure() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm::analysis::{
        AnalysedType, NameOptionTypePair, NameTypePair, TypeRecord, TypeStr, TypeU64, TypeVariant,
    };
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled};
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    // Create request body input
    let request_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();
            let body_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "message".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
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
        name: None,
        owner: None,
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
            name: None,
            owner: None,
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
        binding: GatewayBindingCompiled::Worker(Box::new(worker_binding)),
        middlewares: None,
    };

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("delay-echo".to_string()),
        version: ApiVersion("0.0.3".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Utc::now(),
        namespace: test_namespace(),
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

// Test 9: Complete integration test with full YAML comparison
#[test]
async fn test_oas_conversion_full_structure_shopping_cart() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm::analysis::{
        AnalysedType, NameTypePair, TypeF32, TypeList, TypeRecord, TypeStr, TypeU32, TypeU64,
    };
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled}; //WorkerNameCompiled
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
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "user".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
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
        name: None,
        owner: None,
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
        name: None,
        owner: None,
        inner: Box::new(cart_item_type),
    });

    let response_output = RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            name: None,
            owner: None,
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
            version: 0
        },
        idempotency_key_compiled: None,
        response_compiled: ResponseMappingCompiled {
            response_mapping_expr: Expr::literal("let result = golem:shoppingcart/api.{get-cart-contents}();\n{status: 200: u64, body: result}"),
            response_mapping_compiled: RibByteCode::default(),
            rib_input: path_input,
            worker_calls: None,
            rib_output: Some(response_output)
        },
        invocation_context_compiled: None
    };

    let worker_route = CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/v0.0.1/{user}/get-cart-contents").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(worker_binding)),
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
        namespace: test_namespace(),
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: shopping-cart
        component-version: 0
        response: "\"let result = golem:shoppingcart/api.{get-cart-contents}();\n{status: 200: u64, body: result}\""
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

// Test 10: Path, Query, and Header Parameter Combinations Test
#[test]
async fn test_path_query_header_parameter_combinations() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr, TypeU32, TypeU64};
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled};
    use rib::{Expr, RibByteCode, RibInputTypeInfo, RibOutputTypeInfo};
    use std::collections::HashMap;
    use std::str::FromStr;

    let mut routes = Vec::new();

    // Route 1: Combination of path, query, and header parameters
    // Create a unified RibInputTypeInfo that contains all parameter types
    let combined_input = RibInputTypeInfo {
        types: {
            let mut types = HashMap::new();

            // Create path record with both user and user_id
            let path_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "user".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "user_id".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                ],
            });

            // Create query record with limit and offset
            let query_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "limit".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                    NameTypePair {
                        name: "offset".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                ],
            });

            // Create headers record with age and country
            let headers_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "age".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "country".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
            });

            // Combine all into a single request record
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "path".to_string(),
                        typ: path_record,
                    },
                    NameTypePair {
                        name: "query".to_string(),
                        typ: query_record,
                    },
                    NameTypePair {
                        name: "headers".to_string(),
                        typ: headers_record,
                    },
                ],
            });

            types.insert("request".to_string(), request_record);
            types
        },
    };

    let route1 = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/api/v1/{user}/profile/{user_id}").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(WorkerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440001")
                    .unwrap(),
                version: 1,
            },
            idempotency_key_compiled: None,
            response_compiled: ResponseMappingCompiled {
                response_mapping_expr: Expr::literal(
                    "{status: 200, body: \"User profile for ${user_id}\"}",
                ),
                response_mapping_compiled: RibByteCode::default(),
                rib_input: combined_input,
                worker_calls: None,
                rib_output: Some(RibOutputTypeInfo {
                    analysed_type: AnalysedType::Record(TypeRecord {
                        name: None,
                        owner: None,
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
        })),
        middlewares: None,
    };
    routes.push(route1);

    // Route 2: No parameters (for completeness)
    let route2 = CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/api/v1/health").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(WorkerBindingCompiled {
            component_id: VersionedComponentId {
                component_id: ComponentId::from_str("550e8400-e29b-41d4-a716-446655440004")
                    .unwrap(),
                version: 1,
            },
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
                        name: None,
                        owner: None,
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
        })),
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
        namespace: test_namespace(),
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
                '*/*':
                  schema:
                    type: string
            '200':
              description: OK
              content:
                application/json:
                  schema:
                    type: string
                '*/*':
                  schema:
                    type: string
          x-golem-api-gateway-binding:
            binding-type: default
            component-name: empty-api
            component-version: 1
            response: '"{status: 200, body: "OK"}"'
      /api/v1/{user}/profile/{user_id}:
        get:
          parameters:
          - in: path
            name: user
            description: 'Path parameter: user'
            required: true
            schema:
              type: string
            explode: false
            style: simple
          - in: path
            name: user_id
            description: 'Path parameter: user_id'
            required: true
            schema:
              type: integer
              format: int32
              minimum: 0
            explode: false
            style: simple
          - in: query
            name: limit
            description: 'Query parameter: limit'
            required: true
            schema:
              type: integer
              format: int32
              minimum: 0
            explode: false
            style: form
            allowEmptyValue: false
          - in: query
            name: offset
            description: 'Query parameter: offset'
            required: true
            schema:
              type: integer
              format: int32
              minimum: 0
            explode: false
            style: form
            allowEmptyValue: false
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
                '*/*':
                  schema:
                    type: string
            '200':
              description: OK
              content:
                application/json:
                  schema:
                    type: string
                '*/*':
                  schema:
                    type: string
          x-golem-api-gateway-binding:
            binding-type: default
            component-name: api-with-cors
            component-version: 1
            response: '"{status: 200, body: "User profile for ${user_id}"}"'
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

// Test 11: Comprehensive AnalysedType Coverage Test (10 Routes)
#[test]
async fn test_comprehensive_analysed_type_coverage() {
    use golem_common::base_model::ComponentId;
    use golem_common::model::component::VersionedComponentId;
    use golem_wasm::analysis::{
        AnalysedType, NameOptionTypePair, NameTypePair, TypeBool, TypeChr, TypeEnum, TypeF32,
        TypeF64, TypeFlags, TypeList, TypeOption, TypeRecord, TypeResult, TypeS32, TypeS64,
        TypeStr, TypeTuple, TypeU32, TypeU64, TypeVariant,
    };
    use golem_worker_service::gateway_binding::{ResponseMappingCompiled, WorkerBindingCompiled};
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
                name: None,
                owner: None,
                fields: vec![NameTypePair {
                    name: "input".to_string(),
                    typ: req_type,
                }],
            });
            let request_record = AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
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
            idempotency_key_compiled: None,
            response_compiled: ResponseMappingCompiled {
                response_mapping_expr: Expr::literal("{status: 201, body: result}"),
                response_mapping_compiled: RibByteCode::default(),
                rib_input: request_input,
                worker_calls: None,
                rib_output: Some(RibOutputTypeInfo {
                    analysed_type: AnalysedType::Record(TypeRecord {
                        name: None,
                        owner: None,
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
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440001",
            1,
            Some(AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
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
        ))),
        middlewares: None,
    });

    // Route 2: Collections (list, tuple, option)
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/collections").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440002",
            1,
            Some(AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "list_val".to_string(),
                        typ: AnalysedType::List(TypeList {
                            name: None,
                            owner: None,
                            inner: Box::new(AnalysedType::Str(TypeStr)),
                        }),
                    },
                    NameTypePair {
                        name: "tuple_val".to_string(),
                        typ: AnalysedType::Tuple(TypeTuple {
                            name: None,
                            owner: None,
                            items: vec![AnalysedType::Str(TypeStr), AnalysedType::U32(TypeU32)],
                        }),
                    },
                    NameTypePair {
                        name: "optional_val".to_string(),
                        typ: AnalysedType::Option(TypeOption {
                            name: None,
                            owner: None,
                            inner: Box::new(AnalysedType::Str(TypeStr)),
                        }),
                    },
                ],
            })),
            AnalysedType::List(TypeList {
                name: None,
                owner: None,
                inner: Box::new(AnalysedType::U32(TypeU32)),
            }),
        ))),
        middlewares: None,
    });

    // Route 3: Record type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/record").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440003",
            1,
            Some(AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
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
                name: None,
                owner: None,
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
        ))),
        middlewares: None,
    });

    // Route 4: Enum and Flags types
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/enum-flags").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440004",
            1,
            Some(AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "color".to_string(),
                        typ: AnalysedType::Enum(TypeEnum {
                            name: None,
                            owner: None,
                            cases: vec!["red".to_string(), "green".to_string(), "blue".to_string()],
                        }),
                    },
                    NameTypePair {
                        name: "permissions".to_string(),
                        typ: AnalysedType::Flags(TypeFlags {
                            name: None,
                            owner: None,
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
                name: None,
                owner: None,
                cases: vec![
                    "success".to_string(),
                    "warning".to_string(),
                    "error".to_string(),
                ],
            }),
        ))),
        middlewares: None,
    });

    // Route 5: Result type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/result").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440005",
            1,
            Some(AnalysedType::Result(TypeResult {
                name: None,
                owner: None,
                ok: Some(Box::new(AnalysedType::Str(TypeStr))),
                err: Some(Box::new(AnalysedType::Str(TypeStr))),
            })),
            AnalysedType::Result(TypeResult {
                name: None,
                owner: None,
                ok: Some(Box::new(AnalysedType::U64(TypeU64))),
                err: Some(Box::new(AnalysedType::Str(TypeStr))),
            }),
        ))),
        middlewares: None,
    });

    // Route 6: Variant type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/variant").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440006",
            1,
            Some(AnalysedType::Variant(TypeVariant {
                name: None,
                owner: None,
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
                name: None,
                owner: None,
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
        ))),
        middlewares: None,
    });

    // Route 7: Complex nested type
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/complex").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440007",
            1,
            Some(AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "optional_list".to_string(),
                        typ: AnalysedType::Option(TypeOption {
                            name: None,
                            owner: None,
                            inner: Box::new(AnalysedType::List(TypeList {
                                name: None,
                                owner: None,
                                inner: Box::new(AnalysedType::Str(TypeStr)),
                            })),
                        }),
                    },
                    NameTypePair {
                        name: "result_record".to_string(),
                        typ: AnalysedType::Result(TypeResult {
                            name: None,
                            owner: None,
                            ok: Some(Box::new(AnalysedType::Record(TypeRecord {
                                name: None,
                                owner: None,
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
                name: None,
                owner: None,
                inner: Box::new(AnalysedType::Record(TypeRecord {
                    name: None,
                    owner: None,
                    fields: vec![
                        NameTypePair {
                            name: "id".to_string(),
                            typ: AnalysedType::U64(TypeU64),
                        },
                        NameTypePair {
                            name: "status".to_string(),
                            typ: AnalysedType::Enum(TypeEnum {
                                name: None,
                                owner: None,
                                cases: vec!["pending".to_string(), "completed".to_string()],
                            }),
                        },
                    ],
                })),
            }),
        ))),
        middlewares: None,
    });

    // Route 8: Different number formats
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/numbers").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440008",
            1,
            Some(AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
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
        ))),
        middlewares: None,
    });

    // Route 9: Array response
    routes.push(CompiledRoute {
        method: MethodPattern::Post,
        path: AllPathPatterns::from_str("/types/array-response").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440009",
            1,
            Some(AnalysedType::Str(TypeStr)),
            AnalysedType::List(TypeList {
                name: None,
                owner: None,
                inner: Box::new(AnalysedType::Record(TypeRecord {
                    name: None,
                    owner: None,
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
        ))),
        middlewares: None,
    });

    // Route 10: GET without request body
    routes.push(CompiledRoute {
        method: MethodPattern::Get,
        path: AllPathPatterns::from_str("/types/info").unwrap(),
        binding: GatewayBindingCompiled::Worker(Box::new(create_worker_binding(
            "550e8400-e29b-41d4-a716-446655440010",
            1,
            None, // No request body
            AnalysedType::Record(TypeRecord {
                name: None,
                owner: None,
                fields: vec![
                    NameTypePair {
                        name: "api_version".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "supported_types".to_string(),
                        typ: AnalysedType::List(TypeList {
                            name: None,
                            owner: None,
                            inner: Box::new(AnalysedType::Str(TypeStr)),
                        }),
                    },
                ],
            }),
        ))),
        middlewares: None,
    });

    // Create API definition
    let compiled_api_definition = CompiledHttpApiDefinition {
        id: ApiDefinitionId("comprehensive-types-api".to_string()),
        version: ApiVersion("2.0.0".to_string()),
        routes,
        draft: false,
        created_at: Utc::now(),
        namespace: test_namespace(),
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: parameter-test-api
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: swagger-api
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: delay-echo
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: empty-api
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: comprehensive-types-api
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: number
                format: float
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: secure-api
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
        '201':
          description: Created
          content:
            application/json:
              schema:
                type: boolean
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: api-with-cors
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: test-worker-api
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: simple-echo
        component-version: 1
        response: '"{status: 201, body: result}"'
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
            '*/*':
              schema:
                type: string
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
            '*/*':
              schema:
                type: string
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: todo-list
        component-version: 1
        response: '"{status: 201, body: result}"'
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
