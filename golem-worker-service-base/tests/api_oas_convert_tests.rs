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
use golem_worker_service_base::gateway_api_definition::http::CompiledHttpApiDefinition;
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};

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
