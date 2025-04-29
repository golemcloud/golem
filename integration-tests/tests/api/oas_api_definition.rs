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

use crate::Tracing;
use assert2::assert;

use golem_api_grpc::proto::golem::apidefinition::v1::{
    create_api_definition_request,
    CreateApiDefinitionRequest,
};
use golem_api_grpc::proto::golem::apidefinition::{
    api_definition, static_binding,
};

use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_get_openapi_definition(deps: &EnvBasedTestDependencies) {
    let (_component_id, component_name) = deps
        .component("shopping-cart")
        .unique()
        .store_and_get_name()
        .await;

    let unique_component_name = component_name.0;

    let openapi_yaml = format!(r#"
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
              {{ContentType: "json", userid: "foo"}}
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
"#, unique_component_name = unique_component_name);

    let result = deps
        .worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
            api_definition: Some(create_api_definition_request::ApiDefinition::Openapi(
                openapi_yaml,
            )),
        })
        .await;

    assert!(result.is_ok(), "Failed to create API definition: {:?}", result.as_ref().err());

    let response = result.unwrap();
    let definition = response.definition.unwrap();

    match definition {
        api_definition::Definition::Http(http) => {
            // Verify the number of routes
            assert_eq!(http.routes.len(), 2);

            // Verify the POST route
            let post_route = &http.routes[0];
            assert_eq!(post_route.method, 2); // 2 = POST
            assert_eq!(post_route.path, "/v0.0.1/{user}/add-item");

            // Verify POST route binding
            let post_binding = post_route.binding.as_ref().unwrap();
            assert_eq!(post_binding.binding_type, Some(0)); // 0 = Default

            // Verify POST route component
            let component = post_binding.component.as_ref().unwrap();
            assert_eq!(component.version, 0);
            assert!(component.component_id.is_some());

            // Verify POST route response
            let response_binding = post_binding.response.as_ref().unwrap();
            assert!(response_binding.expr.is_some());

            // Verify OPTIONS route
            let options_route = &http.routes[1];
            assert_eq!(options_route.method, 6); // 6 = OPTIONS
            assert_eq!(options_route.path, "/v0.0.1/{user}/add-item");

            // Verify OPTIONS route binding
            let options_binding = options_route.binding.as_ref().unwrap();
            assert_eq!(options_binding.binding_type, Some(2)); // 2 = CorsPreflight

            // Verify OPTIONS route static binding for CORS
            let static_binding_wrapper = options_binding.static_binding.as_ref().unwrap();
            let static_binding_value = static_binding_wrapper.static_binding.as_ref().unwrap();

            assert!(matches!(static_binding_value, static_binding::StaticBinding::HttpCorsPreflight(_)));
            
            if let static_binding::StaticBinding::HttpCorsPreflight(cors) = static_binding_value {
                assert_eq!(cors.allow_origin.as_deref(), Some("*"));
                assert_eq!(cors.allow_methods.as_deref(), Some("GET, POST, PUT, DELETE, OPTIONS"));
                assert_eq!(cors.allow_headers.as_deref(), Some("Content-Type, Authorization"));
            }
        }
    }
}