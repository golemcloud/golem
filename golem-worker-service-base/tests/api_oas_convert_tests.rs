// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use chrono::Utc;
use golem_common::model::GatewayBindingType;
use golem_wasm_ast::analysis::{
    AnalysedType, NameTypePair, TypeF32, TypeRecord, TypeStr, TypeU32, TypeU64,
};
use golem_worker_service_base::api::api_definition::{
    GatewayBindingResponseData, HttpApiDefinitionResponseData, ResolvedGatewayBindingComponent,
    RouteResponseData,
};
use golem_worker_service_base::gateway_api_definition::http::{
    api_oas_convert::OpenApiHttpApiDefinitionResponse, MethodPattern,
};
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use rib::{RibInputTypeInfo, RibOutputTypeInfo};
use serde_json::from_str;
use std::collections::HashMap;

#[test]
fn test_oas_conversion_end_to_end() {
    // Create a sample API definition
    let api_definition = HttpApiDefinitionResponseData {
        id: ApiDefinitionId("test-api".to_string()),
        version: ApiVersion("0.1.0".to_string()),
        routes: vec![
            RouteResponseData {
                method: MethodPattern::Get,
                path: "/v0.1.0/products".to_string(),
                security: None,
                binding: GatewayBindingResponseData {
                    binding_type: Some(GatewayBindingType::Default),
                    component: Some(ResolvedGatewayBindingComponent {
                        name: "product-service".to_string(),
                        version: 0,
                    }),
                    worker_name: Some("\"product-worker\"".to_string()),
                    worker_name_input: None,
                    idempotency_key: None,
                    idempotency_key_input: None,
                    response: Some("let products = golem:product/api.{get-products}(); {status: 200u64, body: products}".to_string()),
                    response_mapping_input: None,
                    response_mapping_output: None,
                    cors_preflight: None,
                },
            },
            RouteResponseData {
                method: MethodPattern::Post,
                path: "/v0.1.0/products".to_string(),
                security: None,
                binding: GatewayBindingResponseData {
                    binding_type: Some(GatewayBindingType::Default),
                    component: Some(ResolvedGatewayBindingComponent {
                        name: "product-service".to_string(),
                        version: 0,
                    }),
                    worker_name: Some("\"product-worker\"".to_string()),
                    worker_name_input: None,
                    idempotency_key: None,
                    idempotency_key_input: None,
                    response: Some("let product = request.body; {status: 201u64, body: {success: true, id: product.id}}".to_string()),
                    response_mapping_input: None,
                    response_mapping_output: None,
                    cors_preflight: None,
                },
            }
        ],
        draft: false,
        created_at: None,
    };

    // Convert to OpenAPI
    let oas_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&api_definition)
            .expect("Failed to convert API definition to OpenAPI");

    // Verify basic properties
    assert_eq!(oas_response.id, ApiDefinitionId("test-api".to_string()));
    assert_eq!(oas_response.version, ApiVersion("0.1.0".to_string()));

    // Verify OpenAPI YAML content
    let yaml_str = oas_response.openapi_yaml;
    assert!(yaml_str.contains("openapi: 3.0.0"));
    assert!(yaml_str.contains("x-golem-api-definition-id: test-api"));
    assert!(yaml_str.contains("x-golem-api-definition-version: 0.1.0"));

    // Verify paths
    assert!(yaml_str.contains("/v0.1.0/products:"));
    assert!(yaml_str.contains("get:"));
    assert!(yaml_str.contains("post:"));

    // Verify component binding information
    assert!(yaml_str.contains("component-name: product-service"));
    assert!(yaml_str.contains("component-version: 0"));

    // Verify response mappings
    assert!(yaml_str.contains("let products = golem:product/api.{get-products}()"));
    assert!(yaml_str.contains("let product = request.body"));

    // Verify worker name
    assert!(yaml_str.contains("\"product-worker\""));
}

#[test]
fn test_simple_conversion() {
    // Sample JSON input
    let json_data = r#"
    {
        "id": "shopping-cart",
        "version": "0.0.1",
        "routes": []
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Verify the ID and version
    assert_eq!(
        openapi_response.id,
        ApiDefinitionId("shopping-cart".to_string())
    );
    assert_eq!(openapi_response.version, ApiVersion("0.0.1".to_string()));

    // Verify the OpenAPI YAML contains the correct ID and version
    let yaml_str = openapi_response.openapi_yaml;
    assert!(yaml_str.contains("x-golem-api-definition-id: shopping-cart"));
    assert!(yaml_str.contains("x-golem-api-definition-version: 0.0.1"));
}

#[test]
fn test_route_conversion_basic_fields() {
    // Sample JSON input with a single route
    let json_data = r#"
    {
        "id": "shopping-cart",
        "version": "0.0.1",
        "routes": [
            {
                "method": "Post",
                "path": "/v0.0.1/{user}/add-item",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "shopping-cart",
                        "version": 0
                    },
                    "response": "let item = request.body.item; let result = golem:shoppingcart/api.{add-item}(item); {status: 200u64, body: \"Item added\"}",
                    "workerName": "let user: string = request.path.user; \"worker-${user}\""
                }
            }
        ]
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Verify the OpenAPI YAML contains the correct route information
    let yaml_str = openapi_response.openapi_yaml;
    assert!(yaml_str.contains("component-name: shopping-cart"));
    assert!(yaml_str.contains("component-version: 0"));
    assert!(yaml_str.contains("let item = request.body.item; let result = golem:shoppingcart/api.{add-item}(item); {status: 200u64, body: \"Item added\"}"));
    assert!(yaml_str.contains("let user: string = request.path.user; \"worker-${user}\""));
}

#[test]
fn test_cors_and_swagger_binding_types() {
    // Sample JSON input with CORS and Swagger routes
    let json_data = r#"
    {
        "id": "shopping-cart",
        "version": "0.0.1",
        "routes": [
            {
                "method": "Get",
                "path": "/v0.0.1/swagger-shopping-cart",
                "binding": {
                    "bindingType": "swagger-ui"
                }
            },
            {
                "method": "Options",
                "path": "/v0.0.1/{user}/add-item",
                "binding": {
                    "bindingType": "cors-preflight",
                    "corsPreflight": {
                        "allowHeaders": "Content-Type, Authorization",
                        "allowMethods": "GET, POST, PUT, DELETE, OPTIONS",
                        "allowOrigin": "*"
                    }
                }
            }
        ]
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Verify the OpenAPI YAML contains the correct binding types and CORS configuration
    let yaml_str = openapi_response.openapi_yaml;

    // Check Swagger binding
    assert!(yaml_str.contains("binding-type: swagger-ui"));

    // Check CORS binding and configuration
    assert!(yaml_str.contains("binding-type: cors-preflight"));
    assert!(yaml_str.contains("Access-Control-Allow-Headers: \"Content-Type, Authorization\""));
    assert!(yaml_str.contains("Access-Control-Allow-Methods: \"GET, POST, PUT, DELETE, OPTIONS\""));
    assert!(yaml_str.contains("Access-Control-Allow-Origin: \"*\""));
}

#[test]
fn test_request_body_schema_generation() {
    // Sample JSON input with a POST route that takes a request body
    let json_data = r#"
    {
        "id": "product-service",
        "version": "0.1.0",
        "routes": [
            {
                "method": "Post",
                "path": "/v0.1.0/products",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "product-service",
                        "version": 0
                    },
                    "response": "let product = request.body; {status: 201u64, body: {success: true, id: product.id}}",
                    "responseMappingInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "body",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "id",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "name",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "price",
                                                    "typ": {
                                                        "type": "F32"
                                                    }
                                                }
                                            ],
                                            "type": "Record"
                                        }
                                    }
                                ],
                                "type": "Record"
                            }
                        }
                    }
                }
            }
        ]
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Verify the OpenAPI YAML contains the correct request body schema
    let yaml_str = openapi_response.openapi_yaml;

    // Check request body schema
    assert!(yaml_str.contains("requestBody:"));
    assert!(yaml_str.contains("required: true"));
    assert!(yaml_str.contains("application/json:"));
    assert!(yaml_str.contains("type: object"));
    assert!(yaml_str.contains("properties:"));
    assert!(yaml_str.contains("id:"));
    assert!(yaml_str.contains("type: string"));
    assert!(yaml_str.contains("name:"));
    assert!(yaml_str.contains("type: string"));
    assert!(yaml_str.contains("price:"));
    assert!(yaml_str.contains("type: number"));
    assert!(yaml_str.contains("format: float"));
}

#[test]
fn test_response_schema_type_strictness() {
    // Sample JSON input with a GET route that returns a specific response structure
    let json_data = r#"
    {
        "id": "cart-service",
        "version": "0.1.0",
        "routes": [
            {
                "method": "Get",
                "path": "/v0.1.0/cart-items",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "cart-service",
                        "version": 0
                    },
                    "response": "let cart_items = golem:shoppingcart/api.{get-cart-contents}(); {status: 200u64, body: cart_items}",
                    "responseMappingOutput": {
                        "analysed_type": {
                            "fields": [
                                {
                                    "name": "body",
                                    "typ": {
                                        "type": "List",
                                        "inner": {
                                            "fields": [
                                                {
                                                    "name": "product-id",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "name",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "price",
                                                    "typ": {
                                                        "type": "F32"
                                                    }
                                                },
                                                {
                                                    "name": "quantity",
                                                    "typ": {
                                                        "type": "U32"
                                                    }
                                                }
                                            ],
                                            "type": "Record"
                                        }
                                    }
                                },
                                {
                                    "name": "status",
                                    "typ": {
                                        "type": "U64"
                                    }
                                }
                            ],
                            "type": "Record"
                        }
                    }
                }
            }
        ]
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Verify the OpenAPI YAML contains the correct response schema
    let yaml_str = openapi_response.openapi_yaml;

    // Check response schema
    assert!(yaml_str.contains("responses:"), "Missing responses section");
    assert!(yaml_str.contains("'200':"), "Missing 200 status code");
    assert!(yaml_str.contains("type: object"));
    assert!(yaml_str.contains("properties:"));
    assert!(yaml_str.contains("body:"));
    assert!(yaml_str.contains("type: array"));
    assert!(yaml_str.contains("items:"));
    assert!(yaml_str.contains("type: object"));
    assert!(yaml_str.contains("product-id:"));
    assert!(yaml_str.contains("type: string"));
    assert!(yaml_str.contains("name:"));
    assert!(yaml_str.contains("type: string"));
    assert!(yaml_str.contains("price:"));
    assert!(yaml_str.contains("type: integer"));
    assert!(yaml_str.contains("format: float"));
    assert!(yaml_str.contains("quantity:"));
    assert!(yaml_str.contains("type: integer"));
    assert!(yaml_str.contains("format: int32"));
    assert!(yaml_str.contains("minimum: 0"));
    assert!(yaml_str.contains("status:"));
    assert!(yaml_str.contains("type: integer"));
    assert!(yaml_str.contains("minimum: 0"));
}

#[test]
fn test_cors_preflight_response_formatting() {
    // Sample JSON input with a CORS preflight route
    let json_data = r#"
    {
        "id": "api-with-cors",
        "version": "0.1.0",
        "routes": [
            {
                "method": "Options",
                "path": "/v0.1.0/api/resource",
                "binding": {
                    "bindingType": "cors-preflight",
                    "corsPreflight": {
                        "allowHeaders": "Content-Type, Authorization",
                        "allowMethods": "GET, POST, PUT, DELETE, OPTIONS",
                        "allowOrigin": "*",
                        "exposeHeaders": "X-Request-ID",
                        "allowCredentials": true,
                        "maxAge": 8400
                    }
                }
            }
        ]
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Verify the OpenAPI YAML contains the correct CORS configuration
    let yaml_str = openapi_response.openapi_yaml;

    // Check CORS configuration
    assert!(yaml_str.contains("binding-type: cors-preflight"));
    assert!(yaml_str.contains("Access-Control-Allow-Headers: \"Content-Type, Authorization\""));
    assert!(yaml_str.contains("Access-Control-Allow-Methods: \"GET, POST, PUT, DELETE, OPTIONS\""));
    assert!(yaml_str.contains("Access-Control-Allow-Origin: \"*\""));
    assert!(yaml_str.contains("Access-Control-Expose-Headers: \"X-Request-ID\""));
    assert!(yaml_str.contains("Access-Control-Allow-Credentials: true"));
    assert!(yaml_str.contains("Access-Control-Max-Age: 8400"));
}

#[test]
fn test_from_http_api_definition_response_data() {
    // Create a sample HttpApiDefinitionResponseData similar to shopping-cart example
    let response_data = create_sample_response_data();

    // Convert the response data to OpenAPI
    let result =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data);

    // Verify the conversion was successful
    assert!(result.is_ok());
    let openapi_response = result.unwrap();

    // Verify the basic properties were transferred correctly
    assert_eq!(openapi_response.id, response_data.id);
    assert_eq!(openapi_response.version, response_data.version);

    // The YAML should be a non-empty string
    assert!(!openapi_response.openapi_yaml.is_empty());

    // Parse the YAML to verify it's valid OpenAPI
    let openapi_yaml_result =
        serde_yaml::from_str::<serde_yaml::Value>(&openapi_response.openapi_yaml);
    assert!(openapi_yaml_result.is_ok());

    // Verify the OpenAPI version
    let yaml_value = openapi_yaml_result.unwrap();
    assert_eq!(yaml_value["openapi"], "3.0.0");

    // Verify the extension fields were added
    assert_eq!(yaml_value["x-golem-api-definition-id"], "test-api");
    assert_eq!(yaml_value["x-golem-api-definition-version"], "0.0.1");

    // Verify the paths section was created and contains our endpoint
    assert!(yaml_value["paths"]["/v0.0.1/{user}/get-items"].is_mapping());
    assert!(yaml_value["paths"]["/v0.0.1/{user}/get-items"]["get"].is_mapping());

    // Verify parameters for the path parameter
    let parameters = &yaml_value["paths"]["/v0.0.1/{user}/get-items"]["get"]["parameters"];
    assert!(parameters.is_sequence());
    assert_eq!(parameters.as_sequence().unwrap().len(), 1);
    assert_eq!(parameters[0]["name"], "user");
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);

    // Verify responses
    let responses = &yaml_value["paths"]["/v0.0.1/{user}/get-items"]["get"]["responses"];
    assert!(responses.is_mapping());
    assert!(responses["200"].is_mapping());
    assert_eq!(responses["200"]["description"], "OK");

    // Verify binding information in extensions
    let binding =
        &yaml_value["paths"]["/v0.0.1/{user}/get-items"]["get"]["x-golem-api-gateway-binding"];
    assert!(binding.is_mapping());
    assert_eq!(binding["binding-type"], "default");
    assert_eq!(binding["component-name"], "test-component");
    assert_eq!(binding["component-version"], 1);
}

#[test]
fn test_request_body_mapping() {
    // Create a sample with request body mapping
    let response_data = create_sample_with_request_body();

    // Convert the response data to OpenAPI
    let result =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data);

    // Verify the conversion was successful
    assert!(result.is_ok());
    let openapi_response = result.unwrap();

    // Parse the YAML to verify it's valid OpenAPI
    let openapi_yaml_result =
        serde_yaml::from_str::<serde_yaml::Value>(&openapi_response.openapi_yaml);
    assert!(openapi_yaml_result.is_ok());
    let yaml_value = openapi_yaml_result.unwrap();

    // Verify the POST route was created
    assert!(yaml_value["paths"]["/v0.0.1/{user}/add-item"].is_mapping());
    assert!(yaml_value["paths"]["/v0.0.1/{user}/add-item"]["post"].is_mapping());

    // Verify request body exists
    let request_body = &yaml_value["paths"]["/v0.0.1/{user}/add-item"]["post"]["requestBody"];
    assert!(request_body.is_mapping());
    assert_eq!(request_body["required"], true);

    // Verify the content type is JSON
    assert!(request_body["content"]["application/json"].is_mapping());

    // Verify the schema exists and has the expected structure
    let schema = &request_body["content"]["application/json"]["schema"];
    assert!(schema.is_mapping());
    assert!(schema["type"] == "object");

    // Verify the item property exists in the schema
    let properties = &schema["properties"];
    assert!(properties.is_mapping());
    assert!(properties["item"].is_mapping());

    // Verify the item schema has the expected properties
    let item_properties = &properties["item"]["properties"];
    assert!(item_properties.is_mapping());
    assert!(item_properties["name"].is_mapping());
    assert!(
        item_properties["productId"].is_mapping() || item_properties["product-id"].is_mapping()
    );
    assert!(item_properties["price"].is_mapping());
    assert!(item_properties["quantity"].is_mapping());

    // Verify the required fields are listed
    assert!(schema["required"].is_sequence());
    let required = schema["required"].as_sequence().unwrap();
    assert!(required.contains(&serde_yaml::Value::String("item".to_string())));
}

#[test]
fn test_security_conversion() {
    // Sample JSON input with security on routes
    let json_data = r#"
    {
        "id": "secure-api",
        "version": "0.1.0",
        "routes": [
            {
                "method": "Get",
                "path": "/v0.1.0/secure-resource",
                "security": "api-key-auth",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "secure-component",
                        "version": 1
                    },
                    "response": "let result = golem:secure/api.{get-secure-data}(); {status: 200u64, body: result}",
                    "workerName": "\"secure-worker\""
                }
            },
            {
                "method": "Post",
                "path": "/v0.1.0/another-secure-resource",
                "security": "jwt-auth",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "secure-component",
                        "version": 1
                    },
                    "response": "let body = request.body; let result = golem:secure/api.{post-secure-data}(body); {status: 201u64, body: result}",
                    "workerName": "\"secure-worker\""
                }
            }
        ]
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Parse the YAML to verify security schemes and requirements
    let yaml_str = openapi_response.openapi_yaml;
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(&yaml_str).unwrap();

    // Verify security schemes in components
    assert!(yaml_value["components"].is_mapping());
    assert!(yaml_value["components"]["securitySchemes"].is_mapping());
    assert!(yaml_value["components"]["securitySchemes"]["api-key-auth"].is_mapping());
    assert!(yaml_value["components"]["securitySchemes"]["jwt-auth"].is_mapping());

    // Verify security schemes are of type apiKey
    assert_eq!(
        yaml_value["components"]["securitySchemes"]["api-key-auth"]["type"],
        "apiKey"
    );
    assert_eq!(
        yaml_value["components"]["securitySchemes"]["jwt-auth"]["type"],
        "apiKey"
    );

    // Verify security scheme locations
    assert_eq!(
        yaml_value["components"]["securitySchemes"]["api-key-auth"]["in"],
        "header"
    );
    assert_eq!(
        yaml_value["components"]["securitySchemes"]["jwt-auth"]["in"],
        "header"
    );

    // Verify security scheme names
    assert_eq!(
        yaml_value["components"]["securitySchemes"]["api-key-auth"]["name"],
        "Authorization"
    );
    assert_eq!(
        yaml_value["components"]["securitySchemes"]["jwt-auth"]["name"],
        "Authorization"
    );

    // Verify path operations have security requirements
    assert!(yaml_value["paths"]["/v0.1.0/secure-resource"]["get"]["security"].is_sequence());
    assert!(
        yaml_value["paths"]["/v0.1.0/secure-resource"]["get"]["security"][0]["api-key-auth"]
            .is_sequence()
    );
    assert!(
        yaml_value["paths"]["/v0.1.0/secure-resource"]["get"]["security"][0]["api-key-auth"]
            .as_sequence()
            .unwrap()
            .is_empty()
    );

    assert!(
        yaml_value["paths"]["/v0.1.0/another-secure-resource"]["post"]["security"].is_sequence()
    );
    assert!(
        yaml_value["paths"]["/v0.1.0/another-secure-resource"]["post"]["security"][0]["jwt-auth"]
            .is_sequence()
    );
    assert!(
        yaml_value["paths"]["/v0.1.0/another-secure-resource"]["post"]["security"][0]["jwt-auth"]
            .as_sequence()
            .unwrap()
            .is_empty()
    );

    // Verify global security is defined
    assert!(yaml_value["security"].is_sequence());
    assert_eq!(yaml_value["security"].as_sequence().unwrap().len(), 2);
    assert!(yaml_value["security"][0]["api-key-auth"].is_sequence());
    assert!(yaml_value["security"][1]["jwt-auth"].is_sequence());
}

fn create_sample_response_data() -> HttpApiDefinitionResponseData {
    // Create a sample mapping output that represents a list of items
    let response_mapping_output = Some(RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "body".to_string(),
                    typ: AnalysedType::List(golem_wasm_ast::analysis::TypeList {
                        inner: Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "id".to_string(),
                                    typ: AnalysedType::Str(TypeStr {}),
                                },
                                NameTypePair {
                                    name: "name".to_string(),
                                    typ: AnalysedType::Str(TypeStr {}),
                                },
                                NameTypePair {
                                    name: "price".to_string(),
                                    typ: AnalysedType::F32(TypeF32 {}),
                                },
                            ],
                        })),
                    }),
                },
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::U64(TypeU64 {}),
                },
            ],
        }),
    });

    // Create a sample worker name input with path parameter type info
    let worker_name_input = Some(RibInputTypeInfo {
        types: HashMap::from([
            (
                "request".to_string(),
                AnalysedType::Record(TypeRecord {
                    fields: vec![NameTypePair {
                        name: "path".to_string(),
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![NameTypePair {
                                name: "user".to_string(),
                                typ: AnalysedType::Str(TypeStr {}),
                            }],
                        }),
                    }],
                }),
            ),
            (
                "request.path.user".to_string(),
                AnalysedType::Str(TypeStr {}),
            ),
        ]),
    });

    // Create a route similar to the shopping cart example
    let route = RouteResponseData {
        method: MethodPattern::Get,
        path: "/v0.0.1/{user}/get-items".to_string(),
        security: None,
        binding: GatewayBindingResponseData {
            binding_type: Some(GatewayBindingType::Default),
            component: Some(ResolvedGatewayBindingComponent {
                name: "test-component".to_string(),
                version: 1,
            }),
            worker_name: Some("let user = request.path.user; `worker-${user}`".to_string()),
            worker_name_input,
            idempotency_key: None,
            idempotency_key_input: None,
            response: Some(
                "let items = golem:testapi/api.{get-items}(); {status: 200u64, body: items}"
                    .to_string(),
            ),
            response_mapping_input: None,
            response_mapping_output,
            cors_preflight: None,
        },
    };

    // Create the HttpApiDefinitionResponseData
    HttpApiDefinitionResponseData {
        id: ApiDefinitionId("test-api".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Some(Utc::now()),
    }
}

fn create_sample_with_request_body() -> HttpApiDefinitionResponseData {
    // Create a sample request body type info
    let response_mapping_input = Some(RibInputTypeInfo {
        types: HashMap::from([
            (
                "request".to_string(),
                AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "body".to_string(),
                            typ: AnalysedType::Record(TypeRecord {
                                fields: vec![NameTypePair {
                                    name: "item".to_string(),
                                    typ: AnalysedType::Record(TypeRecord {
                                        fields: vec![
                                            NameTypePair {
                                                name: "product-id".to_string(),
                                                typ: AnalysedType::Str(TypeStr {}),
                                            },
                                            NameTypePair {
                                                name: "name".to_string(),
                                                typ: AnalysedType::Str(TypeStr {}),
                                            },
                                            NameTypePair {
                                                name: "price".to_string(),
                                                typ: AnalysedType::F32(TypeF32 {}),
                                            },
                                            NameTypePair {
                                                name: "quantity".to_string(),
                                                typ: AnalysedType::U32(TypeU32 {}),
                                            },
                                        ],
                                    }),
                                }],
                            }),
                        },
                        NameTypePair {
                            name: "path".to_string(),
                            typ: AnalysedType::Record(TypeRecord {
                                fields: vec![NameTypePair {
                                    name: "user".to_string(),
                                    typ: AnalysedType::Str(TypeStr {}),
                                }],
                            }),
                        },
                    ],
                }),
            ),
            (
                "request.body".to_string(),
                AnalysedType::Record(TypeRecord {
                    fields: vec![NameTypePair {
                        name: "item".to_string(),
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "product-id".to_string(),
                                    typ: AnalysedType::Str(TypeStr {}),
                                },
                                NameTypePair {
                                    name: "name".to_string(),
                                    typ: AnalysedType::Str(TypeStr {}),
                                },
                                NameTypePair {
                                    name: "price".to_string(),
                                    typ: AnalysedType::F32(TypeF32 {}),
                                },
                                NameTypePair {
                                    name: "quantity".to_string(),
                                    typ: AnalysedType::U32(TypeU32 {}),
                                },
                            ],
                        }),
                    }],
                }),
            ),
            (
                "request.path.user".to_string(),
                AnalysedType::Str(TypeStr {}),
            ),
        ]),
    });

    // Create response output type info
    let response_mapping_output = Some(RibOutputTypeInfo {
        analysed_type: AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "body".to_string(),
                    typ: AnalysedType::Str(TypeStr {}),
                },
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::U64(TypeU64 {}),
                },
            ],
        }),
    });

    // Create worker name input with path parameter type info
    let worker_name_input = Some(RibInputTypeInfo {
        types: HashMap::from([
            (
                "request".to_string(),
                AnalysedType::Record(TypeRecord {
                    fields: vec![NameTypePair {
                        name: "path".to_string(),
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![NameTypePair {
                                name: "user".to_string(),
                                typ: AnalysedType::Str(TypeStr {}),
                            }],
                        }),
                    }],
                }),
            ),
            (
                "request.path.user".to_string(),
                AnalysedType::Str(TypeStr {}),
            ),
        ]),
    });

    // Create a route similar to the shopping cart add-item example
    let route = RouteResponseData {
        method: MethodPattern::Post,
        path: "/v0.0.1/{user}/add-item".to_string(),
        security: None,
        binding: GatewayBindingResponseData {
            binding_type: Some(GatewayBindingType::Default),
            component: Some(ResolvedGatewayBindingComponent {
                name: "test-component".to_string(),
                version: 1,
            }),
            worker_name: Some("let user = request.path.user; `worker-${user}`".to_string()),
            worker_name_input,
            idempotency_key: None,
            idempotency_key_input: None,
            response: Some("let item = request.body.item; let result = golem:testapi/api.{add-item}(item); {status: 200u64, body: \"Item added\"}".to_string()),
            response_mapping_input,
            response_mapping_output,
            cors_preflight: None,
        },
    };

    // Create the HttpApiDefinitionResponseData
    HttpApiDefinitionResponseData {
        id: ApiDefinitionId("test-api".to_string()),
        version: ApiVersion("0.0.1".to_string()),
        routes: vec![route],
        draft: false,
        created_at: Some(Utc::now()),
    }
}
