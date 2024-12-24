// Integration test for Rib API → OpenAPI Spec → Swagger UI → Client Library
// Copyright 2024 Golem Cloud
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

use std::process::Command;
use golem_wasm_ast::analysis::{model, AnalysedType};
use golem_worker_service::api::definition::types::{ApiDefinition, Route, HttpMethod, BindingType};

#[test]
fn test_rib_to_openapi_to_client_flow() {
    // Step 1: Define a sample API
    let api_definition = ApiDefinition {
        id: "test_api".to_string(),
        name: "Test API".to_string(),
        version: "1.0.0".to_string(),
        description: "Integration Test API".to_string(),
        routes: vec![Route {
            path: "/test".to_string(),
            method: HttpMethod::Get,
            description: "Test route".to_string(),
            component_name: "TestComponent".to_string(),
            binding: BindingType::Default {
                input_type: AnalysedType::Str(model::TypeStr {}),
                output_type: AnalysedType::Bool(model::TypeBool {}),
                function_name: "test_function".to_string(),
            },
        }],
    };

    // Step 2: Generate the OpenAPI Spec
    let openapi_spec = super::OpenApiGenerator::generate(&api_definition);
    assert_eq!(openapi_spec.info.title, "Test API");

    // Step 3: Serve Swagger UI
    let swagger_config = super::SwaggerUiConfig {
        enabled: true,
        path: "/docs".to_string(),
        title: Some("Test API Documentation".to_string()),
        theme: None,
        api_id: "test_api".to_string(),
        version: "1.0.0".to_string(),
    };
    let swagger_ui = super::generate_swagger_ui(&swagger_config);
    assert!(swagger_ui.contains("Test API Documentation"));

    // Step 4: Generate and validate a client library
    let status = Command::new("openapi-generator-cli")
        .args(&[
            "generate",
            "-i",
            "/path/to/openapi.yaml",
            "-g",
            "python",
            "-o",
            "/path/to/generated/python-client",
        ])
        .status()
        .expect("Failed to generate client library");
    assert!(status.success());

    // Step 5: Validate the client library
    let client_test_status = Command::new("pytest")
        .arg("/path/to/generated/python-client/tests")
        .status()
        .expect("Failed to run client tests");
    assert!(client_test_status.success());
}
