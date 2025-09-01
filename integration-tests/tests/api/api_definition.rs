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

use crate::{Deps, Tracing};
use assert2::assert;
use golem_client::model::{
    GatewayBindingComponent, GatewayBindingData, GatewayBindingType, HttpApiDefinitionRequest,
    HttpApiDefinitionResponseData, HttpCors, MethodPattern, RouteRequestData,
};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(Deps);

#[test]
#[tracing::instrument]
async fn create_and_get_api_definition(deps: &Deps) {
    let admin = deps.admin().await;

    let (_, component_name) = admin
        .component("shopping-cart-resource")
        .unique()
        .store_and_get_name()
        .await;

    let request = HttpApiDefinitionRequest {
        id: Uuid::new_v4().to_string(),
        version: "1".to_string(),
        draft: true,
        security: None,
        routes: vec![RouteRequestData {
            method: MethodPattern::Post,
            path: "/{user-id}/test-path-1".to_string(),
            binding: GatewayBindingData {
                component: Some(GatewayBindingComponent {
                    name: component_name.0,
                    version: Some(0),
                }),
                worker_name: None,
                response: Some(
                    r#"
                        let user-id = request.path.user-id;
                        let worker = "shopping-cart-${user-id}";
                        let inst = instance(worker);
                        let res = inst.cart(user-id);
                        let contents = res.get-cart-contents();
                        {
                            headers: {ContentType: "json", userid: "foo"},
                            body: contents,
                            status: 201
                        }
                    "#
                    .to_string(),
                ),
                idempotency_key: None,
                binding_type: Some(GatewayBindingType::Default),
                invocation_context: None,
            },
            security: None,
        }],
    };

    let project = admin.default_project().await;

    let response = deps
        .worker_service()
        .create_api_definition(&admin.token, &project, &request)
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request, &response);

    let response = deps
        .worker_service()
        .get_api_definition(&admin.token, &project, &request.id, &response.version)
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request, &response);

    let response = deps
        .worker_service()
        .get_api_definition(&admin.token, &project, &request.id, "not-exists")
        .await;

    assert!(response.is_err());
    response.err().unwrap().to_string().contains("NotFound");
}

#[test]
#[tracing::instrument]
async fn get_api_definition_versions(deps: &Deps) {
    let admin = deps.admin().await;

    let (_, component_name) = admin
        .component("shopping-cart-resource")
        .unique()
        .store_and_get_name()
        .await;

    let request_1 = HttpApiDefinitionRequest {
        id: Uuid::new_v4().to_string(),
        version: "1".to_string(),
        draft: true,
        security: None,
        routes: vec![RouteRequestData {
            method: MethodPattern::Post,
            path: "/{user-id}/test-path-1".to_string(),
            binding: GatewayBindingData {
                component: Some(GatewayBindingComponent {
                    name: component_name.0.to_string(),
                    version: Some(0),
                }),
                worker_name: None,
                response: Some(
                    r#"
                        let user-id = request.path.user-id;
                        let worker = "shopping-cart-${user-id}";
                        let inst = instance(worker);
                        let res = inst.cart(user-id);
                        let contents = res.get-cart-contents();
                        {
                            headers: {ContentType: "json", userid: "foo"},
                            body: contents,
                            status: 201
                        }
                    "#
                    .to_string(),
                ),
                idempotency_key: None,
                binding_type: Some(GatewayBindingType::Default),
                invocation_context: None,
            },
            security: None,
        }],
    };

    let request_2 = HttpApiDefinitionRequest {
        id: request_1.id.clone(),
        version: "2".to_string(),
        draft: true,
        security: None,
        routes: vec![
            RouteRequestData {
                method: MethodPattern::Get,
                path: "/{user-id}/test-path-1".to_string(),
                binding: GatewayBindingData {
                    component: Some(GatewayBindingComponent {
                        name: component_name.0.to_string(),
                        version: Some(0),
                    }),
                    worker_name: None,
                    response: Some(
                        r#"
                            let user-id = request.path.user-id;
                            let worker = "shopping-cart-${user-id}";
                            let inst = instance(worker);
                            let res = inst.cart(user-id);
                            let contents = res.get-cart-contents();
                            {
                                headers: {ContentType: "json", userid: "foo"},
                                body: contents,
                                status: 201
                            }
                        "#
                        .to_string(),
                    ),
                    idempotency_key: None,
                    binding_type: Some(GatewayBindingType::Default),
                    invocation_context: None,
                },
                security: None,
            },
            RouteRequestData {
                method: MethodPattern::Patch,
                path: "/{user-id}/test-path-2".to_string(),
                binding: GatewayBindingData {
                    component: Some(GatewayBindingComponent {
                        name: component_name.0.to_string(),
                        version: Some(0),
                    }),
                    worker_name: None,
                    response: Some(
                        r#"
                            let user-id = request.path.user-id;
                            let worker = "shopping-cart-${user-id}";
                            let inst = instance(worker);
                            let res = inst.cart(user-id);
                            let contents = res.get-cart-contents();
                            {
                                headers: {ContentType: "json", userid: "foo"},
                                body: contents,
                                status: 201
                            }
                        "#
                        .to_string(),
                    ),
                    idempotency_key: None,
                    binding_type: Some(GatewayBindingType::Default),
                    invocation_context: None,
                },
                security: None,
            },
        ],
    };

    let project = admin.default_project().await;

    let response_1 = deps
        .worker_service()
        .create_api_definition(&admin.token, &project, &request_1)
        .await
        .unwrap();
    check_equal_api_definition_request_and_response(&request_1, &response_1);

    let versions = deps
        .worker_service()
        .get_api_definition_versions(&admin.token, &project, &request_1.id)
        .await
        .unwrap();
    assert!(versions.len() == 1);
    check_equal_api_definition_request_and_response(&request_1, &versions[0]);

    let request_1 = HttpApiDefinitionRequest {
        draft: false,
        ..request_1
    };

    let updated_1 = deps
        .worker_service()
        .update_api_definition(&admin.token, &project, &request_1)
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request_1, &updated_1);

    let versions = deps
        .worker_service()
        .get_api_definition_versions(&admin.token, &project, &request_1.id)
        .await
        .unwrap();
    assert!(versions.len() == 1);
    check_equal_api_definition_request_and_response(&request_1, &versions[0]);

    let response_2 = deps
        .worker_service()
        .create_api_definition(&admin.token, &project, &request_2)
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request_2, &response_2);

    let versions = deps
        .worker_service()
        .get_api_definition_versions(&admin.token, &project, &request_1.id)
        .await
        .unwrap();
    assert!(versions.len() == 2);
    check_equal_api_definition_request_and_response(&request_1, &versions[0]);
    check_equal_api_definition_request_and_response(&request_2, &versions[1]);

    deps.worker_service()
        .delete_api_definition(&admin.token, &project, &request_1.id, "1")
        .await
        .unwrap();

    let versions = deps
        .worker_service()
        .get_api_definition_versions(&admin.token, &project, &request_1.id)
        .await
        .unwrap();
    assert!(versions.len() == 1);
    check_equal_api_definition_request_and_response(&request_2, &versions[0]);
}

#[test]
#[tracing::instrument]
async fn get_api_definition_all_versions(deps: &Deps) {
    let admin = deps.admin().await;

    let (_, component_name_1) = admin
        .component("shopping-cart-resource")
        .unique()
        .store_and_get_name()
        .await;

    let (_, component_name_2) = admin
        .component("shopping-cart-resource")
        .unique()
        .store_and_get_name()
        .await;

    let request_1_1 = HttpApiDefinitionRequest {
        id: Uuid::new_v4().to_string(),
        version: "1".to_string(),
        draft: true,
        security: None,
        routes: vec![RouteRequestData {
            method: MethodPattern::Post,
            path: "/{user-id}/test-path-1".to_string(),
            binding: GatewayBindingData {
                component: Some(GatewayBindingComponent {
                    name: component_name_1.0,
                    version: Some(0),
                }),
                worker_name: None,
                response: Some(
                    r#"
                        let user-id = request.path.user-id;
                        let worker = "shopping-cart-${user-id}";
                        let inst = instance(worker);
                        let res = inst.cart(user-id);
                        let contents = res.get-cart-contents();
                        {
                            headers: {ContentType: "json", userid: "foo"},
                            body: contents,
                            status: 201
                        }
                    "#
                    .to_string(),
                ),
                idempotency_key: None,
                binding_type: Some(GatewayBindingType::Default),
                invocation_context: None,
            },
            security: None,
        }],
    };

    let request_1_2 = HttpApiDefinitionRequest {
        version: "2".to_string(),
        ..request_1_1.clone()
    };

    let request_2_1 = HttpApiDefinitionRequest {
        id: Uuid::new_v4().to_string(),
        version: "1".to_string(),
        draft: true,
        security: None,
        routes: vec![RouteRequestData {
            method: MethodPattern::Post,
            path: "/{user-id}/test-path-2".to_string(),
            binding: GatewayBindingData {
                component: Some(GatewayBindingComponent {
                    name: component_name_2.0,
                    version: Some(0),
                }),
                worker_name: None,
                response: Some(
                    r#"
                        let user-id = request.path.user-id;
                        let worker = "shopping-cart-${user-id}";
                        let inst = instance(worker);
                        let res = inst.cart(user-id);
                        let contents = res.get-cart-contents();
                        {
                            headers: {ContentType: "json", userid: "foo"},
                            body: contents,
                            status: 201
                        }
                    "#
                    .to_string(),
                ),
                idempotency_key: None,
                binding_type: Some(GatewayBindingType::Default),
                invocation_context: None,
            },
            security: None,
        }],
    };

    let request_2_2 = HttpApiDefinitionRequest {
        version: "2".to_string(),
        ..request_2_1.clone()
    };

    let request_2_3 = HttpApiDefinitionRequest {
        version: "3".to_string(),
        ..request_2_1.clone()
    };

    let project = admin.default_project().await;

    for request in [
        &request_1_1,
        &request_1_2,
        &request_2_1,
        &request_2_2,
        &request_2_3,
    ] {
        deps.worker_service()
            .create_api_definition(&admin.token, &project, request)
            .await
            .unwrap();
    }

    let result = deps
        .worker_service()
        .get_all_api_definitions(&admin.token, &project)
        .await
        .unwrap();
    assert!(result.len() >= 5);

    let result = result
        .into_iter()
        .map(|api_definition| {
            (
                format!("{}@{}", &api_definition.id, api_definition.version),
                api_definition,
            )
        })
        .collect::<HashMap<_, _>>();

    fn check_contains(
        result: &HashMap<String, HttpApiDefinitionResponseData>,
        api_definition_request: &HttpApiDefinitionRequest,
    ) {
        check_equal_api_definition_request_and_response(
            api_definition_request,
            result
                .get(&format!(
                    "{}@{}",
                    &api_definition_request.id, api_definition_request.version
                ))
                .unwrap(),
        );
    }

    check_contains(&result, &request_1_1);
    check_contains(&result, &request_1_2);
    check_contains(&result, &request_2_1);
    check_contains(&result, &request_2_2);
    check_contains(&result, &request_2_3);
}

fn check_equal_api_definition_request_and_response(
    request: &HttpApiDefinitionRequest,
    response: &HttpApiDefinitionResponseData,
) {
    assert_eq!(request.id, response.id);
    assert_eq!(request.version, response.version);
    assert_eq!(request.draft, response.draft);

    assert_eq!(request.routes.len(), response.routes.len());

    for i in 0..request.routes.len() {
        let request_route = &request.routes[i];
        let response_route = &response.routes[i];

        assert_eq!(request_route.method, response_route.method);
        assert_eq!(request_route.path, response_route.path);
        assert_eq!(request_route.security, response_route.security);

        assert_eq!(
            request_route.binding.binding_type,
            response_route.binding.binding_type
        );
        check_optional_rib_code(
            request_route.binding.idempotency_key.as_deref(),
            response_route.binding.idempotency_key.as_deref(),
        );
        check_optional_rib_code(
            request_route.binding.invocation_context.as_deref(),
            response_route.binding.invocation_context.as_deref(),
        );
        check_optional_rib_code(
            request_route.binding.response.as_deref(),
            response_route.binding.response.as_deref(),
        );
        assert_eq!(
            request_route.binding.component.clone().map(|c| c.name),
            response_route.binding.component.clone().map(|c| c.name)
        );

        {
            let component_version = request_route
                .binding
                .component
                .clone()
                .and_then(|c| c.version);
            if let Some(component_version) = component_version {
                assert_eq!(
                    Some(component_version),
                    response_route.binding.component.clone().map(|c| c.version)
                );
            }
        }
    }
}

fn check_optional_rib_code(actual: Option<&str>, expected: Option<&str>) {
    assert_eq!(
        actual.map(|v| rib::from_string(v).unwrap()),
        expected.map(|v| rib::from_string(v).unwrap())
    );
}

// Create API definition from OpenAPI YAML
#[test]
#[tracing::instrument]
async fn create_openapi_yaml_definition(deps: &Deps) {
    let admin = deps.admin().await;

    let (_component_id, component_name) = admin
        .component("shopping-cart")
        .unique()
        .store_and_get_name()
        .await;

    let unique_component_name = component_name.0;

    let openapi_yaml = format!(
        r#"
openapi: 3.0.0
info:
  title: {unique_component_name}
  version: 0.0.1
paths:
  /v0.0.1/{{user}}/add-item:
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
        '200':
          description: Success
      x-golem-api-gateway-binding:
        binding-type: default
        component-name: {unique_component_name}
        component-version: 0
        response: |-
          let worker = instance("{unique_component_name}");
          let result = worker.get-cart-contents();
          let status: u64 = 200;
          {{
            headers: {{
              ContentType: "json",
              userid: "foo"
            }},
            body: "Item added",
            status: status
          }}
    options:
      responses:
        '200':
          description: Success
      x-golem-api-gateway-binding:
        binding-type: cors-preflight
        response: |-
          {{
            Access-Control-Allow-Headers: "Content-Type, Authorization",
            Access-Control-Allow-Methods: "GET, POST, PUT, DELETE, OPTIONS",
            Access-Control-Allow-Origin: "*"
          }}
x-golem-api-definition-id: shopping-cart
x-golem-api-definition-version: 0.0.1
"#
    );

    let project = admin.default_project().await;

    let result = deps
        .worker_service()
        .create_api_definition_from_yaml(&admin.token, &project, &openapi_yaml)
        .await;

    let response = result.unwrap();

    // Verify top-level fields
    assert_eq!(response.id, "shopping-cart");
    assert_eq!(response.version, "0.0.1");
    assert!(response.draft);

    assert_eq!(response.routes.len(), 2);

    let post_route = &response.routes[0];
    assert_eq!(post_route.method, MethodPattern::Post);
    assert_eq!(post_route.path, "/v0.0.1/{user}/add-item");

    let post_binding = &post_route.binding;
    assert_eq!(
        post_route.binding.binding_type,
        Some(GatewayBindingType::Default)
    );

    let component = post_binding.component.as_ref().unwrap();
    assert_eq!(component.version, 0);
    assert_eq!(component.name, unique_component_name);

    assert!(post_binding.response.is_some());

    let options_route = &response.routes[1];
    assert_eq!(options_route.method, MethodPattern::Options);
    assert_eq!(options_route.path, "/v0.0.1/{user}/add-item");

    let options_binding = &options_route.binding;
    assert_eq!(
        options_binding.binding_type,
        Some(GatewayBindingType::CorsPreflight)
    );
    assert_eq!(
        options_binding.cors_preflight,
        Some(HttpCors {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST, PUT, DELETE, OPTIONS".to_string(),
            allow_headers: "Content-Type, Authorization".to_string(),
            expose_headers: None,
            allow_credentials: None,
            max_age: None
        })
    );
}

// Create API definition from OpenAPI JSON
#[test]
#[tracing::instrument]
async fn create_openapi_json_definition(deps: &Deps) {
    let admin = deps.admin().await;

    let (_component_id, component_name) = admin
        .component("shopping-cart")
        .unique()
        .store_and_get_name()
        .await;

    let unique_component_name = component_name.0;

    let openapi_json = format!(
        r#"
{{
  "openapi": "3.0.0",
  "info": {{
    "title": "{unique_component_name}",
    "version": "0.0.1"
  }},
  "paths": {{
    "/v0.0.1/{{user}}/add-item": {{
      "post": {{
        "parameters": [
          {{
            "in": "path",
            "name": "user",
            "description": "Path parameter: user",
            "required": true,
            "schema": {{
              "type": "string"
            }},
            "explode": false,
            "style": "simple"
          }}
        ],
        "responses": {{
          "200": {{
            "description": "Success"
          }}
        }},
        "x-golem-api-gateway-binding": {{
          "binding-type": "default",
          "component-name": "{unique_component_name}",
          "component-version": 0,
          "response": "let worker = instance(\"{unique_component_name}\");\nlet result = worker.get-cart-contents();\nlet status: u64 = 200;\n{{\n  headers: {{\n    ContentType: \"json\",\n    userid: \"foo\"\n  }},\n  body: \"Item added\",\n  status: status\n}}"
        }}
      }},
      "options": {{
        "responses": {{
          "200": {{
            "description": "Success"
          }}
        }},
        "x-golem-api-gateway-binding": {{
          "binding-type": "cors-preflight",
          "response": "{{\n  Access-Control-Allow-Headers: \"Content-Type, Authorization\",\n  Access-Control-Allow-Methods: \"GET, POST, PUT, DELETE, OPTIONS\",\n  Access-Control-Allow-Origin: \"*\"\n}}"
        }}
      }}
    }}
  }},
  "x-golem-api-definition-id": "shopping-cart-openapi-json",
  "x-golem-api-definition-version": "0.0.1"
}}
"#
    );

    let project = admin.default_project().await;

    let result = deps
        .worker_service()
        .create_api_definition_from_json(&admin.token, &project, &openapi_json)
        .await;

    let response = result.unwrap();

    // Verify top-level fields
    assert_eq!(response.id, "shopping-cart-openapi-json");
    assert_eq!(response.version, "0.0.1");
    assert!(response.draft);

    assert_eq!(response.routes.len(), 2);

    let post_route = &response.routes[0];
    assert_eq!(post_route.method, MethodPattern::Post);
    assert_eq!(post_route.path, "/v0.0.1/{user}/add-item");

    let post_binding = &post_route.binding;
    assert_eq!(
        post_route.binding.binding_type,
        Some(GatewayBindingType::Default)
    );

    let component = post_binding.component.as_ref().unwrap();
    assert_eq!(component.version, 0);
    assert_eq!(component.name, unique_component_name);

    assert!(post_binding.response.is_some());

    let options_route = &response.routes[1];
    assert_eq!(options_route.method, MethodPattern::Options);
    assert_eq!(options_route.path, "/v0.0.1/{user}/add-item");

    let options_binding = &options_route.binding;
    assert_eq!(
        options_binding.binding_type,
        Some(GatewayBindingType::CorsPreflight)
    );
    assert_eq!(
        options_binding.cors_preflight,
        Some(HttpCors {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST, PUT, DELETE, OPTIONS".to_string(),
            allow_headers: "Content-Type, Authorization".to_string(),
            expose_headers: None,
            allow_credentials: None,
            max_age: None
        })
    );
}

#[test]
#[tracing::instrument]
async fn test_export_openapi_spec_simple(deps: &Deps) {
    let admin = deps.admin().await;
    let project = admin.default_project().await;

    let (_component_id, component_name) = admin
        .component("counters")
        .unique()
        .store_and_get_name()
        .await;

    // Create an API definition with a specific route
    let api_id = Uuid::new_v4().to_string();
    let request = HttpApiDefinitionRequest {
        id: api_id.clone(),
        version: "1.0".to_string(),
        draft: true,
        security: None,
        routes: vec![RouteRequestData {
            method: MethodPattern::Get,
            path: "/test-simple-export".to_string(),
            binding: GatewayBindingData {
                component: Some(GatewayBindingComponent {
                    name: component_name.0,
                    version: Some(0),
                }),
                worker_name: None,
                response: Some(
                    r#"
                        {
                            headers: {ContentType: "application/json"},
                            body: "Simple export test response",
                            status: 200
                        }
                    "#
                    .to_string(),
                ),
                idempotency_key: None,
                binding_type: Some(GatewayBindingType::Default),
                invocation_context: None,
            },
            security: None,
        }],
    };

    // Create the API definition
    let response = deps
        .worker_service()
        .create_api_definition(&admin.token, &project, &request)
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request, &response);

    // Export the API definition
    let export_data = deps
        .worker_service()
        .export_openapi_spec(&admin.token, &project, &api_id, "1.0")
        .await
        .unwrap();

    // Validate that there is YAML content
    assert!(
        !export_data.openapi_yaml.is_empty(),
        "OpenAPI YAML should not be empty"
    );

    // Basic validation of the YAML structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&export_data.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic OpenAPI structure
    assert_eq!(yaml_value["openapi"].as_str().unwrap(), "3.0.0");
    assert_eq!(yaml_value["info"]["title"].as_str().unwrap(), api_id);
    assert_eq!(yaml_value["info"]["version"].as_str().unwrap(), "1.0");
    assert_eq!(
        yaml_value["x-golem-api-definition-id"].as_str().unwrap(),
        api_id
    );
    assert_eq!(
        yaml_value["x-golem-api-definition-version"]
            .as_str()
            .unwrap(),
        "1.0"
    );

    // Verify path exists
    assert!(yaml_value["paths"]["/test-simple-export"].is_mapping());
}

#[test]
#[tracing::instrument]
// This is the full round trip test for API definition: API -> OpenAPI -> API
async fn test_roundtrip_api_definition(deps: &Deps) {
    let admin = deps.admin().await;
    let project = admin.default_project().await;

    let (_component_id, component_name) = admin
        .component("counters")
        .unique()
        .store_and_get_name()
        .await;

    // 1. Create an API definition request with a specific route
    let api_id = Uuid::new_v4().to_string();
    let request = HttpApiDefinitionRequest {
        id: api_id.clone(),
        version: "1.0".to_string(),
        draft: true,
        security: None,
        routes: vec![RouteRequestData {
            method: MethodPattern::Get,
            path: "/test-fixed-export-path".to_string(),
            binding: GatewayBindingData {
                component: Some(GatewayBindingComponent {
                    name: component_name.0,
                    version: Some(0),
                }),
                worker_name: None,
                response: Some(
                    r#"
                        {
                            headers: {
                                ContentType: "application/json"
                            },
                            body: "Fixed export test response",
                            status: 200
                        }
                    "#
                    .to_string(),
                ),
                idempotency_key: None,
                binding_type: Some(GatewayBindingType::Default),
                invocation_context: None,
            },
            security: None,
        }],
    };

    // 2. Create the API definition
    let original_api_definition = deps
        .worker_service()
        .create_api_definition(&admin.token, &project, &request)
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request, &original_api_definition);

    // 3. Export the API definition to OpenAPI format
    let export_data = deps
        .worker_service()
        .export_openapi_spec(&admin.token, &project, &api_id, "1.0")
        .await
        .unwrap();

    // Validate that there is YAML content
    assert!(
        !export_data.openapi_yaml.is_empty(),
        "OpenAPI YAML should not be empty"
    );

    // Basic validation of the YAML structure
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&export_data.openapi_yaml).expect("Failed to parse OpenAPI YAML");

    // Verify basic OpenAPI structure
    assert_eq!(yaml_value["openapi"].as_str().unwrap(), "3.0.0");
    assert_eq!(yaml_value["info"]["title"].as_str().unwrap(), api_id);
    assert_eq!(yaml_value["info"]["version"].as_str().unwrap(), "1.0");
    assert_eq!(
        yaml_value["x-golem-api-definition-id"].as_str().unwrap(),
        api_id
    );
    assert_eq!(
        yaml_value["x-golem-api-definition-version"]
            .as_str()
            .unwrap(),
        "1.0"
    );

    // Verify path exists
    assert!(yaml_value["paths"]["/test-fixed-export-path"].is_mapping());

    // 4. Delete the original API definition
    deps.worker_service()
        .delete_api_definition(&admin.token, &project, &api_id, "1.0")
        .await
        .unwrap();

    // 5. Create new API definition from the exported YAML
    let imported_api_definition = deps
        .worker_service()
        .create_api_definition_from_yaml(&admin.token, &project, &export_data.openapi_yaml)
        .await
        .unwrap();

    // 6. Compare both API definitions
    check_equal_api_definition_request_and_response(&request, &imported_api_definition);
}
