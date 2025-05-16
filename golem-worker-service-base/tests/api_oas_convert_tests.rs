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

use golem_worker_service_base::api::api_definition::HttpApiDefinitionResponseData;
use golem_worker_service_base::gateway_api_definition::http::api_oas_convert::OpenApiHttpApiDefinitionResponse;
use serde_json::from_str;

// Test that the conversion works for a simple API definition with no routes,
// Test yaml values to confirm the structure is correct
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

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_eq!(yaml_value["openapi"], "3.0.0");
    assert_eq!(yaml_value["info"]["title"], "shopping-cart");
    assert_eq!(yaml_value["info"]["version"], "0.0.1");
    assert_eq!(yaml_value["x-golem-api-definition-id"], "shopping-cart");
    assert_eq!(yaml_value["x-golem-api-definition-version"], "0.0.1");

    // Verify empty paths
    assert!(yaml_value["paths"].is_mapping());
    assert!(yaml_value["paths"].as_mapping().unwrap().is_empty());
}

// Test that the conversion works for a CORS preflight route
// Test cors-preflight is converted to rib valid response string
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

    // Verify the content type
    let content = &responses["200"]["content"]["application/json"];
    assert!(content.is_mapping());

    // Verify the schema
    let schema = &content["schema"];
    assert_eq!(schema["type"], "object");
}

// Test that the conversion works for a simple route without response mapping
// Test dynamic path parameters, binding, workername, and response
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
                    "workerName": "let user: string = request.path.user; \"worker-${user}\"",
                    "workerNameInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "path",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "user",
                                                    "typ": {
                                                        "type": "Str"
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

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_eq!(yaml_value["openapi"], "3.0.0");
    assert_eq!(yaml_value["info"]["title"], "shopping-cart");
    assert_eq!(yaml_value["info"]["version"], "0.0.1");
    assert_eq!(yaml_value["x-golem-api-definition-id"], "shopping-cart");
    assert_eq!(yaml_value["x-golem-api-definition-version"], "0.0.1");

    // Verify path exists
    assert!(yaml_value["paths"]["/v0.0.1/{user}/add-item"].is_mapping());

    // Verify POST operation
    let post_op = &yaml_value["paths"]["/v0.0.1/{user}/add-item"]["post"];
    assert!(post_op.is_mapping());

    // Verify parameters
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
    assert_eq!(
        binding["worker-name"],
        "let user: string = request.path.user; \"worker-${user}\""
    );
    assert_eq!(binding["response"], "let item = request.body.item; let result = golem:shoppingcart/api.{add-item}(item); {status: 200u64, body: \"Item added\"}");
}

// Test that the conversion works for a basic types with a echo function
// Test u64, bool, record, and enum, Does not test path parameters
// Test that the input and output types are converted to the correct types
#[test]
fn test_basic_types_and_record_conversion() {
    // Sample JSON input with routes for basic types, record, and enum
    let json_data = r#"
    {
        "id": "simple-echo",
        "version": "0.0.1",
        "routes": [
            {
                "method": "Post",
                "path": "/v0.0.1/{user}/u64",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "simple-echo",
                        "version": 0
                    },
                    "response": "let input: u64 = request.body.input; let result = golem:simpleecho/api.{echo-u64}(input); {status: 200u64, body: result}",
                    "responseMappingInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "body",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "input",
                                                    "typ": {
                                                        "type": "U64"
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
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "body",
                                    "typ": {
                                        "type": "U64"
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
                    },
                    "workerName": "let user: string = request.path.user; \"worker-${user}\""
                }
            },
            {
                "method": "Post",
                "path": "/v0.0.1/{user}/bool",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "simple-echo",
                        "version": 0
                    },
                    "response": "let input: bool = request.body.input; let result = golem:simpleecho/api.{echo-bool}(input); {status: 200u64, body: result}",
                    "responseMappingInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "body",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "input",
                                                    "typ": {
                                                        "type": "Bool"
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
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "body",
                                    "typ": {
                                        "type": "Bool"
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
                    },
                    "workerName": "let user: string = request.path.user; \"worker-${user}\""
                }
            },
            {
                "method": "Post",
                "path": "/v0.0.1/{user}/record",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "simple-echo",
                        "version": 0
                    },
                    "response": "let input = {name: \"testbot\", id: 30: u32, active: true}; let result = golem:simpleecho/api.{echo-record}(input); {status: 200u64, body: result}",
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
                                                        "type": "U32"
                                                    }
                                                },
                                                {
                                                    "name": "name",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "active",
                                                    "typ": {
                                                        "type": "Bool"
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
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "body",
                                    "typ": {
                                        "fields": [
                                            {
                                                "name": "id",
                                                "typ": {
                                                    "type": "U32"
                                                }
                                            },
                                            {
                                                "name": "name",
                                                "typ": {
                                                    "type": "Str"
                                                }
                                            },
                                            {
                                                "name": "active",
                                                "typ": {
                                                    "type": "Bool"
                                                }
                                            }
                                        ],
                                        "type": "Record"
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
                    },
                    "workerName": "let user: string = request.path.user; \"worker-${user}\""
                }
            },
            {
                "method": "Post",
                "path": "/v0.0.1/{user}/priority",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "simple-echo",
                        "version": 0
                    },
                    "response": "let input: string = request.body.input; let result = golem:simpleecho/api.{echo-priority}(input); {status: 200u64, body: result}",
                    "responseMappingInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "body",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "input",
                                                    "typ": {
                                                        "type": "Enum",
                                                        "cases": ["low", "medium", "high"]
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
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "body",
                                    "typ": {
                                        "type": "Enum",
                                        "cases": ["low", "medium", "high"]
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
                    },
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

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_eq!(yaml_value["openapi"], "3.0.0");
    assert_eq!(yaml_value["info"]["title"], "simple-echo");
    assert_eq!(yaml_value["info"]["version"], "0.0.1");

    // Verify u64 route, request body, and response
    let u64_route = &yaml_value["paths"]["/v0.0.1/{user}/u64"]["post"];

    let u64_schema = &u64_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(u64_schema["type"], "object");
    assert_eq!(u64_schema["properties"]["input"]["type"], "integer");
    assert_eq!(u64_schema["properties"]["input"]["format"], "int64");
    assert_eq!(
        u64_schema["required"],
        serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("input".to_string())])
    );

    let u64_response = &u64_route["responses"]["201"]["content"]["application/json"]["schema"];
    assert_eq!(u64_response["type"], "integer");
    assert_eq!(u64_response["format"], "int64");

    // Verify bool route, request body, and response
    let bool_route = &yaml_value["paths"]["/v0.0.1/{user}/bool"]["post"];

    let bool_schema = &bool_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(bool_schema["type"], "object");
    assert_eq!(bool_schema["properties"]["input"]["type"], "boolean");
    assert_eq!(
        bool_schema["required"],
        serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("input".to_string())])
    );

    let bool_response = &bool_route["responses"]["201"]["content"]["application/json"]["schema"];
    assert_eq!(bool_response["type"], "boolean");

    // Verify record route, request body, and response
    let record_route = &yaml_value["paths"]["/v0.0.1/{user}/record"]["post"];

    let record_schema = &record_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(record_schema["type"], "object");
    assert_eq!(record_schema["properties"]["id"]["type"], "integer");
    assert_eq!(record_schema["properties"]["id"]["format"], "int32");
    assert_eq!(record_schema["properties"]["name"]["type"], "string");
    assert_eq!(record_schema["properties"]["active"]["type"], "boolean");

    // Verify priority enum route, request body, and response
    let priority_route = &yaml_value["paths"]["/v0.0.1/{user}/priority"]["post"];

    let priority_schema = &priority_route["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(priority_schema["type"], "object");
    assert_eq!(priority_schema["properties"]["input"]["type"], "string");
    assert_eq!(
        priority_schema["properties"]["input"]["enum"],
        serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("low".to_string()),
            serde_yaml::Value::String("medium".to_string()),
            serde_yaml::Value::String("high".to_string())
        ])
    );
    assert_eq!(
        priority_schema["required"],
        serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("input".to_string())])
    );

    let priority_response =
        &priority_route["responses"]["201"]["content"]["application/json"]["schema"];
    assert_eq!(priority_response["type"], "string");
    assert_eq!(
        priority_response["enum"],
        serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("low".to_string()),
            serde_yaml::Value::String("medium".to_string()),
            serde_yaml::Value::String("high".to_string())
        ])
    );
}

// Test that the conversion works for a swagger-ui binding type
// Test also for default response
// Todo: Add test for file-server binding type, and any other new binding types
#[test]
fn test_swagger_ui_binding() {
    // Sample JSON input with Swagger UI route
    let json_data = r#"
    {
        "id": "swagger-api",
        "version": "0.1.0",
        "routes": [
            {
                "method": "Get",
                "path": "/v0.1.0/swagger-ui",
                "binding": {
                    "bindingType": "swagger-ui"
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

    // Define expected YAML structure
    let expected_yaml = r#"
openapi: 3.0.0
info:
  title: swagger-api
  version: 0.1.0
paths:
  /v0.1.0/swagger-ui:
    get:
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                type: object
        default:
          description: OK
          content:
            application/json:
              schema:
                type: object
      x-golem-api-gateway-binding:
        binding-type: swagger-ui
components: {}
x-golem-api-definition-id: swagger-api
x-golem-api-definition-version: 0.1.0
"#;

    // Parse both YAMLs for comparison
    let actual_yaml: serde_yaml::Value = serde_yaml::from_str(&openapi_response.openapi_yaml)
        .expect("Failed to parse actual OpenAPI YAML");
    let expected_yaml: serde_yaml::Value =
        serde_yaml::from_str(expected_yaml).expect("Failed to parse expected OpenAPI YAML");

    // Single assert comparing the complete structure
    assert_eq!(actual_yaml, expected_yaml);
}

// Test for if security is converted properly
// Only test the security part of the conversion
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

    // Expected OpenAPI YAML structure
    let expected_yaml = r#"
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
paths:
  /v0.1.0/secure-resource:
    get:
      security:
        - api-key-auth: []
  /v0.1.0/another-secure-resource:
    post:
      security:
        - jwt-auth: []
security:
  - api-key-auth: []
  - jwt-auth: []
"#;

    // Parse both YAMLs for comparison
    let actual_yaml: serde_yaml::Value = serde_yaml::from_str(&openapi_response.openapi_yaml)
        .expect("Failed to parse actual OpenAPI YAML");
    let expected_yaml: serde_yaml::Value =
        serde_yaml::from_str(expected_yaml).expect("Failed to parse expected OpenAPI YAML");

    // Compare the relevant sections
    assert_eq!(
        actual_yaml["components"]["securitySchemes"],
        expected_yaml["components"]["securitySchemes"]
    );
    assert_eq!(
        actual_yaml["paths"]["/v0.1.0/secure-resource"]["get"]["security"],
        expected_yaml["paths"]["/v0.1.0/secure-resource"]["get"]["security"]
    );
    assert_eq!(
        actual_yaml["paths"]["/v0.1.0/another-secure-resource"]["post"]["security"],
        expected_yaml["paths"]["/v0.1.0/another-secure-resource"]["post"]["security"]
    );
    assert_eq!(actual_yaml["security"], expected_yaml["security"]);
}

// Test for if optional and oneOf are converted properly
// Verify required fields are properly converted
// Complete todo-list structure, which test for record, enum, optional, and oneOf
#[test]
fn test_complete_todo_structure_with_optional_and_oneof() {
    // Sample JSON input with complete todo-list structure
    let json_data = r#"
    {
        "id": "todo-list",
        "version": "0.0.1",
        "routes": [
            {
                "method": "Post",
                "path": "/v0.0.1/{user}/add",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "todo-list",
                        "version": 0
                    },
                    "response": "let input = request.body.input;\nlet result = golem:todolist/api.{add}(input);\n{status: 200: u64, body: result}",
                    "responseMappingInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "body",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "input",
                                                    "typ": {
                                                        "fields": [
                                                            {
                                                                "name": "title",
                                                                "typ": {
                                                                    "type": "Str"
                                                                }
                                                            },
                                                            {
                                                                "name": "priority",
                                                                "typ": {
                                                                    "cases": [
                                                                        "low",
                                                                        "medium",
                                                                        "high"
                                                                    ],
                                                                    "type": "Enum"
                                                                }
                                                            },
                                                            {
                                                                "name": "deadline",
                                                                "typ": {
                                                                    "inner": {
                                                                        "type": "Str"
                                                                    },
                                                                    "type": "Option"
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
                                ],
                                "type": "Record"
                            }
                        }
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "body",
                                    "typ": {
                                        "err": {
                                            "type": "Str"
                                        },
                                        "ok": {
                                            "fields": [
                                                {
                                                    "name": "id",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "title",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "priority",
                                                    "typ": {
                                                        "cases": [
                                                            "low",
                                                            "medium",
                                                            "high"
                                                        ],
                                                        "type": "Enum"
                                                    }
                                                },
                                                {
                                                    "name": "status",
                                                    "typ": {
                                                        "cases": [
                                                            "backlog",
                                                            "in-progress",
                                                            "done"
                                                        ],
                                                        "type": "Enum"
                                                    }
                                                },
                                                {
                                                    "name": "created-timestamp",
                                                    "typ": {
                                                        "type": "S64"
                                                    }
                                                },
                                                {
                                                    "name": "updated-timestamp",
                                                    "typ": {
                                                        "type": "S64"
                                                    }
                                                },
                                                {
                                                    "name": "deadline",
                                                    "typ": {
                                                        "inner": {
                                                            "type": "S64"
                                                        },
                                                        "type": "Option"
                                                    }
                                                }
                                            ],
                                            "type": "Record"
                                        },
                                        "type": "Result"
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
                    },
                    "workerName": "let user: string = request.path.user;\n\"${user}\"",
                    "workerNameInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "path",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "user",
                                                    "typ": {
                                                        "type": "Str"
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

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic OpenAPI structure
    assert_eq!(yaml_value["openapi"], "3.0.0");
    assert_eq!(yaml_value["info"]["title"], "todo-list");
    assert_eq!(yaml_value["info"]["version"], "0.0.1");

    // Verify path exists
    let path = &yaml_value["paths"]["/v0.0.1/{user}/add"];
    assert!(path.is_mapping());

    // Verify POST operation
    let post_op = &path["post"];
    assert!(post_op.is_mapping());

    // Verify parameters
    let parameters = &post_op["parameters"];
    assert!(parameters.is_sequence());
    assert_eq!(parameters[0]["name"], "user");
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(parameters[0]["schema"]["type"], "string");

    // Verify request body
    let request_body = &post_op["requestBody"];
    assert!(request_body["required"].as_bool().unwrap());

    // Verify request body schema
    let request_schema = &request_body["content"]["application/json"]["schema"];
    assert_eq!(request_schema["type"], "object");

    // Verify required input field
    assert_eq!(
        request_schema["required"],
        serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("input".to_string())])
    );

    // Verify input properties
    let input_props = &request_schema["properties"]["input"]["properties"];

    // Required fields, no deadline
    assert_eq!(input_props["title"]["type"], "string");
    assert_eq!(input_props["priority"]["type"], "string");
    assert_eq!(
        input_props["priority"]["enum"],
        serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("low".to_string()),
            serde_yaml::Value::String("medium".to_string()),
            serde_yaml::Value::String("high".to_string())
        ])
    );

    // Optional field (deadline)
    assert_eq!(input_props["deadline"]["nullable"], true);
    assert_eq!(input_props["deadline"]["type"], "string");

    // Verify responses
    let responses = &post_op["responses"];
    assert!(responses.is_mapping());
    assert!(responses["201"].is_mapping());

    // Verify response schema
    let response_schema = &responses["201"]["content"]["application/json"]["schema"];

    // Verify OneOf structure
    assert!(response_schema["oneOf"].is_sequence());
    assert_eq!(response_schema["oneOf"].as_sequence().unwrap().len(), 2);

    // Verify success case (ok)
    let ok_schema = &response_schema["oneOf"][0];
    assert!(ok_schema["properties"]["ok"].is_mapping());

    // Verify required fields in success case
    let ok_props = &ok_schema["properties"]["ok"]["properties"];
    assert_eq!(ok_props["id"]["type"], "string");
    assert_eq!(ok_props["title"]["type"], "string");
    assert_eq!(ok_props["priority"]["type"], "string");
    assert_eq!(ok_props["status"]["type"], "string");
    assert_eq!(ok_props["created-timestamp"]["type"], "integer");
    assert_eq!(ok_props["updated-timestamp"]["type"], "integer");

    // Verify optional field (deadline) in success case
    assert_eq!(ok_props["deadline"]["nullable"], true);
    assert_eq!(ok_props["deadline"]["type"], "integer");
    assert_eq!(ok_props["deadline"]["format"], "int64");

    // Verify error case (err)
    let err_schema = &response_schema["oneOf"][1];
    assert!(err_schema["properties"]["err"].is_mapping());
    assert_eq!(err_schema["properties"]["err"]["type"], "string");
}

// Test for delay-echo, path parameter has {user} and {time}
// {user} is from workerNameInput, {time} is from responseMappingInput
#[test]
fn test_user_time_conversion() {
    // Sample JSON input with a single route
    let json_data = r#"
    {
        "id": "delay-echo",
        "version": "0.0.1",
        "routes": [
            {
                "method": "Post",
                "path": "/v0.0.1/{user}/{time}/delay-echo",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "delay-echo",
                        "version": 0
                    },
                    "response": "let user: string = request.path.user; let time: u32 = request.path.time; let result = golem:delayecho/api.{echo}(\"worker-${user}\", time); {status: 200u64, body: result}",
                    "workerName": "let user: string = request.path.user; \"worker-${user}\"",
                    "workerNameInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "path",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "user",
                                                    "typ": {
                                                        "type": "Str"
                                                    }
                                                },
                                                {
                                                    "name": "time",
                                                    "typ": {
                                                        "type": "U32"
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

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic properties
    assert_eq!(yaml_value["openapi"], "3.0.0");
    assert_eq!(yaml_value["info"]["title"], "delay-echo");
    assert_eq!(yaml_value["info"]["version"], "0.0.1");
    assert_eq!(yaml_value["x-golem-api-definition-id"], "delay-echo");
    assert_eq!(yaml_value["x-golem-api-definition-version"], "0.0.1");

    // Verify path exists
    assert!(yaml_value["paths"]["/v0.0.1/{user}/{time}/delay-echo"].is_mapping());

    // Verify POST operation
    let post_op = &yaml_value["paths"]["/v0.0.1/{user}/{time}/delay-echo"]["post"];
    assert!(post_op.is_mapping());

    // Verify parameters
    let parameters = &post_op["parameters"];
    assert!(parameters.is_sequence());
    assert_eq!(parameters[0]["name"], "user");
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(parameters[1]["name"], "time");
    assert_eq!(parameters[1]["in"], "path");
    assert_eq!(parameters[1]["required"], true);

    // Verify binding information
    let binding = &post_op["x-golem-api-gateway-binding"];
    assert_eq!(binding["binding-type"], "default");
    assert_eq!(binding["component-name"], "delay-echo");
    assert_eq!(binding["component-version"], 0);
    assert_eq!(
        binding["worker-name"],
        "let user: string = request.path.user; \"worker-${user}\""
    );
    assert_eq!(binding["response"], "let user: string = request.path.user; let time: u32 = request.path.time; let result = golem:delayecho/api.{echo}(\"worker-${user}\", time); {status: 200u64, body: result}");
}

// Test full structure of shopping-cart, tested as a single output openapi schema
// Test for cors-preflight, swagger-ui, default bindings
// Test for no request body, and array object as response
// Test also for default response
#[test]
fn test_oas_conversion_full_structure_shopping_cart() {
    // Sample JSON input with complete shopping-cart structure
    let json_data = r#"
    {
        "id": "shopping-cart",
        "version": "0.0.1",
        "routes": [
            {
                "binding": {
                    "bindingType": "swagger-ui",
                    "component": null,
                    "corsPreflight": null,
                    "idempotencyKey": null,
                    "idempotencyKeyInput": null,
                    "response": null,
                    "responseMappingInput": null,
                    "responseMappingOutput": null,
                    "workerName": null,
                    "workerNameInput": null
                },
                "method": "Get",
                "path": "/v0.0.1/swagger-shopping-cart",
                "security": null
            },
            {
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "shopping-cart",
                        "version": 0
                    },
                    "corsPreflight": null,
                    "idempotencyKey": null,
                    "idempotencyKeyInput": null,
                    "response": "let result = golem:shoppingcart/api.{get-cart-contents}();\n{status: 200: u64, body: result}",
                    "responseMappingInput": {
                        "types": {}
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "body",
                                    "typ": {
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
                                        },
                                        "type": "List"
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
                    },
                    "workerName": "let user: string = request.path.user;\n\"worker-${user}\"",
                    "workerNameInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "path",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "user",
                                                    "typ": {
                                                        "type": "Str"
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
                },
                "method": "Post",
                "path": "/v0.0.1/{user}/get-cart-contents",
                "security": null
            },
            {
                "binding": {
                    "bindingType": "cors-preflight",
                    "component": null,
                    "corsPreflight": {
                        "allowCredentials": null,
                        "allowHeaders": "Content-Type, Authorization",
                        "allowMethods": "GET, POST, PUT, DELETE, OPTIONS",
                        "allowOrigin": "*",
                        "exposeHeaders": null,
                        "maxAge": null
                    },
                    "idempotencyKey": null,
                    "idempotencyKeyInput": null,
                    "response": null,
                    "responseMappingInput": null,
                    "responseMappingOutput": null,
                    "workerName": null,
                    "workerNameInput": null
                },
                "method": "Options",
                "path": "/v0.0.1/{user}/get-cart-contents",
                "security": null
            }
        ]
    }"#;

    // Parse the JSON
    let response_data: HttpApiDefinitionResponseData = from_str(json_data).unwrap();

    // Convert to OpenAPI
    let openapi_response =
        OpenApiHttpApiDefinitionResponse::from_http_api_definition_response_data(&response_data)
            .unwrap();

    // Define expected YAML structure
    // The response should only contains the body part, from <status, body>
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
          content:
            application/json:
              schema:
                type: object
        default:
          description: OK
          content:
            application/json:
              schema:
                type: object
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
        response: |-
          let result = golem:shoppingcart/api.{get-cart-contents}();
          {status: 200: u64, body: result}
        worker-name: |-
          let user: string = request.path.user;
          "worker-${user}"
    options:
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                type: object
        default:
          description: OK
          content:
            application/json:
              schema:
                type: object
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

// Test for query parameter
// Test for first-class worker
#[test]
fn test_query_parameter_conversion() {
    // Sample JSON input with a query parameter
    let json_data = r#"
    {
        "id": "delay-echo",
        "version": "0.0.3",
        "routes": [
            {
                "method": "Post",
                "path": "/v0.0.3/echo-query",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "delay-echo",
                        "version": 0
                    },
                    "response": "let worker = instance(\"worker-static-2\");\nlet result = worker.echo-variant(request.query.echo);\nlet body = match result {  rand1(msg) => msg, rand2(msg) => msg, rand3(msg) => msg } ;\nlet status = match result {  rand1(_) => 200: u32, rand2(_) => 200: u32, rand3(_) => 400: u32 } ;\n{status: status, body: body}",
                    "responseMappingInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "query",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "echo",
                                                    "typ": {
                                                        "type": "Str"
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
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "status",
                                    "typ": {
                                        "type": "U32"
                                    }
                                },
                                {
                                    "name": "body",
                                    "typ": {
                                        "type": "Str"
                                    }
                                }
                            ],
                            "type": "Record"
                        }
                    },
                    "workerName": "\"worker-static\"",
                    "workerNameInput": {
                        "types": {}
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

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify query parameter
    let parameters = &yaml_value["paths"]["/v0.0.3/echo-query"]["post"]["parameters"];
    assert!(parameters.is_sequence());
    assert_eq!(parameters[0]["name"], "echo");
    assert_eq!(parameters[0]["in"], "query");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(parameters[0]["schema"]["type"], "string");
}

// Test for variant output
// Test for first-class worker
#[test]
fn test_variant_output_structure() {
    // Sample JSON input with variant output
    let json_data = r#"
    {
        "id": "delay-echo",
        "version": "0.0.3",
        "routes": [
            {
                "method": "Post",
                "path": "/v0.0.3/echo-firstclass",
                "binding": {
                    "bindingType": "default",
                    "component": {
                        "name": "delay-echo",
                        "version": 0
                    },
                    "response": "let worker = instance(\"worker-static\");\nlet result = worker.echo-variant(request.body.message);\n{status: 200: u64, body: result}",
                    "responseMappingInput": {
                        "types": {
                            "request": {
                                "fields": [
                                    {
                                        "name": "body",
                                        "typ": {
                                            "fields": [
                                                {
                                                    "name": "message",
                                                    "typ": {
                                                        "type": "Str"
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
                    },
                    "responseMappingOutput": {
                        "analysedType": {
                            "fields": [
                                {
                                    "name": "status",
                                    "typ": {
                                        "type": "U64"
                                    }
                                },
                                {
                                    "name": "body",
                                    "typ": {
                                        "cases": [
                                            {
                                                "name": "rand1",
                                                "typ": {
                                                    "type": "Str"
                                                    }
                                            },
                                            {
                                                "name": "rand2",
                                                "typ": {
                                                    "type": "Str"
                                                    }
                                            },
                                            {
                                                "name": "rand3",
                                                "typ": {
                                                    "type": "Str"
                                                    }
                                            }
                                        ],
                                        "type": "Variant"
                                    }
                                }
                            ],
                            "type": "Record"
                        }
                    },
                    "workerName": null,
                    "workerNameInput": null
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

    // Parse the YAML to verify the structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&openapi_response.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify variant output structure
    let response_schema = &yaml_value["paths"]["/v0.0.3/echo-firstclass"]["post"]["responses"]
        ["201"]["content"]["application/json"]["schema"];
    assert!(response_schema["oneOf"].is_sequence());
    assert_eq!(response_schema["oneOf"].as_sequence().unwrap().len(), 3);

    // Verify each variant case
    let rand1 = &response_schema["oneOf"][0];
    assert_eq!(rand1["properties"]["rand1"]["type"], "string");
    assert!(rand1["required"]
        .as_sequence()
        .unwrap()
        .contains(&serde_yaml::Value::String("rand1".to_string())));

    let rand2 = &response_schema["oneOf"][1];
    assert_eq!(rand2["properties"]["rand2"]["type"], "string");
    assert!(rand2["required"]
        .as_sequence()
        .unwrap()
        .contains(&serde_yaml::Value::String("rand2".to_string())));

    let rand3 = &response_schema["oneOf"][2];
    assert_eq!(rand3["properties"]["rand3"]["type"], "string");
    assert!(rand3["required"]
        .as_sequence()
        .unwrap()
        .contains(&serde_yaml::Value::String("rand3".to_string())));
}
