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

use crate::{to_grpc_rib_expr, Tracing};
use assert2::{assert, check};
use golem_api_grpc::proto::golem::apidefinition::v1::{
    api_definition_request, create_api_definition_request, update_api_definition_request,
    ApiDefinitionRequest, CreateApiDefinitionRequest, DeleteApiDefinitionRequest,
    GetApiDefinitionRequest, GetApiDefinitionVersionsRequest, UpdateApiDefinitionRequest,
};
use golem_api_grpc::proto::golem::apidefinition::{
    api_definition, ApiDefinition, ApiDefinitionId, GatewayBinding, GatewayBindingType,
    HttpApiDefinition, HttpMethod, HttpRoute,
};
use golem_api_grpc::proto::golem::component::VersionedComponentId;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;
use serde_yaml::Value;
use serde_json::Value as JsonValue;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_get_api_security_scheme(deps: &EnvBasedTestDependencies) {
    let component_id = deps.component("counters").unique().store().await;

    let request = ApiDefinitionRequest {
        id: Some(ApiDefinitionId {
            value: Uuid::new_v4().to_string(),
        }),
        version: "1".to_string(),
        draft: true,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![HttpRoute {
                    method: HttpMethod::Post as i32,
                    path: "/test-path-1".to_string(),
                    binding: Some(GatewayBinding {
                        component: Some(VersionedComponentId {
                            component_id: Some(component_id.clone().into()),
                            version: 0,
                        }),
                        worker_name: Some(to_grpc_rib_expr(r#""counter""#)),
                        response: Some(to_grpc_rib_expr(
                            r#"
                                let status: u64 = 200;
                                {
                                    headers: {ContentType: "json", userid: "foo"},
                                   body: "foo",
                                   status: status
                                }
                            "#,
                        )),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default as i32),
                        static_binding: None,
                        invocation_context: None,
                    }),
                    middleware: None,
                }],
            },
        )),
    };

    let response = deps
        .worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
            api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                request.clone(),
            )),
        })
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request, &response);

    let response = deps
        .worker_service()
        .get_api_definition(GetApiDefinitionRequest {
            api_definition_id: request.id.clone(),
            version: response.version.clone(),
        })
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request, &response);

    let response = deps
        .worker_service()
        .get_api_definition(GetApiDefinitionRequest {
            api_definition_id: request.id.clone(),
            version: "not-exists".to_string(),
        })
        .await;

    assert!(response.is_err());
    response.err().unwrap().to_string().contains("NotFound");
}

#[test]
#[tracing::instrument]
async fn get_api_definition_versions(deps: &EnvBasedTestDependencies) {
    let component_id = deps.component("counters").unique().store().await;

    let request_1 = ApiDefinitionRequest {
        id: Some(ApiDefinitionId {
            value: Uuid::new_v4().to_string(),
        }),
        version: "1".to_string(),
        draft: true,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![HttpRoute {
                    method: HttpMethod::Post as i32,
                    path: "/test-path-1".to_string(),
                    binding: Some(GatewayBinding {
                        component: Some(VersionedComponentId {
                            component_id: Some(component_id.clone().into()),
                            version: 0,
                        }),
                        worker_name: Some(to_grpc_rib_expr(r#""counter""#)),
                        response: Some(to_grpc_rib_expr(
                            r#"
                                let status: u64 = 201;
                                {
                                    headers: {ContentType: "json", userid: "foo"},
                                   body: "bar",
                                   status: status
                                }
                            "#,
                        )),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default as i32),
                        static_binding: None,
                        invocation_context: None,
                    }),
                    middleware: None,
                }],
            },
        )),
    };

    let request_2 = ApiDefinitionRequest {
        id: request_1.id.clone(),
        version: "2".to_string(),
        draft: true,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![
                    HttpRoute {
                        method: HttpMethod::Get as i32,
                        path: "/test-path-1".to_string(),
                        binding: Some(GatewayBinding {
                            component: Some(VersionedComponentId {
                                component_id: Some(component_id.clone().into()),
                                version: 0,
                            }),
                            worker_name: Some(to_grpc_rib_expr(r#""counter""#)),
                            response: Some(to_grpc_rib_expr(
                                r#"
                                let status: u64 = 200;
                                {
                                    headers: {ContentType: "json", userid: "foo"},
                                   body: "foo",
                                   status: status
                                }
                            "#,
                            )),
                            idempotency_key: None,
                            binding_type: Some(GatewayBindingType::Default as i32),
                            static_binding: None,
                            invocation_context: None,
                        }),
                        middleware: None,
                    },
                    HttpRoute {
                        method: HttpMethod::Patch as i32,
                        path: "/test-path-2".to_string(),
                        binding: Some(GatewayBinding {
                            component: Some(VersionedComponentId {
                                component_id: Some(component_id.clone().into()),
                                version: 0,
                            }),
                            worker_name: Some(to_grpc_rib_expr(r#""counter""#)),
                            response: Some(to_grpc_rib_expr(
                                r#"
                                let status: u64 = 200;
                                {
                                    headers: {ContentType: "json", userid: "foo"},
                                   body: "foo",
                                   status: status
                                }
                            "#,
                            )),
                            idempotency_key: None,
                            binding_type: Some(GatewayBindingType::Default as i32),
                            static_binding: None,
                            invocation_context: None,
                        }),
                        middleware: None,
                    },
                ],
            },
        )),
    };

    let response_1 = deps
        .worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
            api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                request_1.clone(),
            )),
        })
        .await
        .unwrap();
    check_equal_api_definition_request_and_response(&request_1, &response_1);

    let versions = deps
        .worker_service()
        .get_api_definition_versions(GetApiDefinitionVersionsRequest {
            api_definition_id: request_1.id.clone(),
        })
        .await
        .unwrap();
    assert!(versions.len() == 1);
    check_equal_api_definition_request_and_response(&request_1, &versions[0]);

    let request_1 = ApiDefinitionRequest {
        draft: false,
        ..request_1
    };

    let updated_1 = deps
        .worker_service()
        .update_api_definition(UpdateApiDefinitionRequest {
            api_definition: Some(update_api_definition_request::ApiDefinition::Definition(
                request_1.clone(),
            )),
        })
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request_1, &updated_1);

    let versions = deps
        .worker_service()
        .get_api_definition_versions(GetApiDefinitionVersionsRequest {
            api_definition_id: request_1.id.clone(),
        })
        .await
        .unwrap();
    assert!(versions.len() == 1);
    check_equal_api_definition_request_and_response(&request_1, &versions[0]);

    let response_2 = deps
        .worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
            api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                request_2.clone(),
            )),
        })
        .await
        .unwrap();
    check_equal_api_definition_request_and_response(&request_2, &response_2);

    let versions = deps
        .worker_service()
        .get_api_definition_versions(GetApiDefinitionVersionsRequest {
            api_definition_id: request_1.id.clone(),
        })
        .await
        .unwrap();
    assert!(versions.len() == 2);
    check_equal_api_definition_request_and_response(&request_1, &versions[0]);
    check_equal_api_definition_request_and_response(&request_2, &versions[1]);

    deps.worker_service()
        .delete_api_definition(DeleteApiDefinitionRequest {
            api_definition_id: request_1.id.clone(),
            version: "1".to_string(),
        })
        .await
        .unwrap();

    let versions = deps
        .worker_service()
        .get_api_definition_versions(GetApiDefinitionVersionsRequest {
            api_definition_id: request_1.id.clone(),
        })
        .await
        .unwrap();
    assert!(versions.len() == 1);
    check_equal_api_definition_request_and_response(&request_2, &versions[0]);
}

#[test]
#[tracing::instrument]
async fn get_api_definition_all_versions(deps: &EnvBasedTestDependencies) {
    let component_id_1 = deps.component("counters").unique().store().await;
    let component_id_2 = deps.component("counters").unique().store().await;

    let request_1_1 = ApiDefinitionRequest {
        id: Some(ApiDefinitionId {
            value: Uuid::new_v4().to_string(),
        }),
        version: "1".to_string(),
        draft: true,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![HttpRoute {
                    method: HttpMethod::Post as i32,
                    path: "/test-path-1".to_string(),
                    binding: Some(GatewayBinding {
                        component: Some(VersionedComponentId {
                            component_id: Some(component_id_1.clone().into()),
                            version: 0,
                        }),
                        worker_name: Some(to_grpc_rib_expr(r#""counter""#)),
                        response: Some(to_grpc_rib_expr(
                            r#"
                                let status: u64 = 201;
                                {
                                    headers: { ContentType: "json", userid: "foo" },
                                   body: "bar",
                                   status: status
                                }
                            "#,
                        )),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default as i32),
                        static_binding: None,
                        invocation_context: None,
                    }),
                    middleware: None,
                }],
            },
        )),
    };

    let request_1_2 = ApiDefinitionRequest {
        version: "2".to_string(),
        ..request_1_1.clone()
    };

    let request_2_1 = ApiDefinitionRequest {
        id: Some(ApiDefinitionId {
            value: Uuid::new_v4().to_string(),
        }),
        version: "1".to_string(),
        draft: true,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![HttpRoute {
                    method: HttpMethod::Post as i32,
                    path: "/test-path-2".to_string(),
                    binding: Some(GatewayBinding {
                        component: Some(VersionedComponentId {
                            component_id: Some(component_id_2.clone().into()),
                            version: 0,
                        }),
                        worker_name: Some(to_grpc_rib_expr(r#""counter-2""#)),
                        response: Some(to_grpc_rib_expr(
                            r#"
                                let status: u64 = 404;
                                {
                                    headers: {ContentType: "json", userid: "foo"},
                                   body: "bar",
                                   status: status
                                }
                            "#,
                        )),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default as i32),
                        static_binding: None,
                        invocation_context: None,
                    }),
                    middleware: None,
                }],
            },
        )),
    };

    let request_2_2 = ApiDefinitionRequest {
        version: "2".to_string(),
        ..request_2_1.clone()
    };

    let request_2_3 = ApiDefinitionRequest {
        version: "3".to_string(),
        ..request_2_1.clone()
    };

    for request in [
        &request_1_1,
        &request_1_2,
        &request_2_1,
        &request_2_2,
        &request_2_3,
    ] {
        deps.worker_service()
            .create_api_definition(CreateApiDefinitionRequest {
                api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                    request.clone(),
                )),
            })
            .await
            .unwrap();
    }

    let result = deps
        .worker_service()
        .get_all_api_definitions()
        .await
        .unwrap();
    assert!(result.len() >= 5);

    let result = result
        .into_iter()
        .map(|api_definition| {
            (
                format!(
                    "{}@{}",
                    &api_definition.id.as_ref().unwrap().value,
                    api_definition.version
                ),
                api_definition,
            )
        })
        .collect::<HashMap<_, _>>();

    fn check_contains(
        result: &HashMap<String, ApiDefinition>,
        api_definition_request: &ApiDefinitionRequest,
    ) {
        check_equal_api_definition_request_and_response(
            api_definition_request,
            result
                .get(&format!(
                    "{}@{}",
                    &api_definition_request.id.as_ref().unwrap().value,
                    api_definition_request.version
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

#[test]
#[tracing::instrument]
// This is a full round trip test for API definition
// Which is the converted to OpenAPI YAML, delete the original API and 
// Upload the OpenAPI YAML as a new API definition
// Then verify that the new API definition is the same as the original

async fn test_export_import_api_definition(deps: &EnvBasedTestDependencies) {
    // Create a component to use in the API definition
    let component_id = deps.component("counters").unique().store().await;

    // Create an API definition with a specific route
    let api_id = Uuid::new_v4().to_string();
    let request = ApiDefinitionRequest {
        id: Some(ApiDefinitionId {
            value: api_id.clone(),
        }),
        version: "1.0".to_string(),
        draft: false,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![HttpRoute {
                    method: HttpMethod::Get as i32,
                    path: "/test-export-path".to_string(),
                    binding: Some(GatewayBinding {
                        component: Some(VersionedComponentId {
                            component_id: Some(component_id.clone().into()),
                            version: 0,
                        }),
                        worker_name: Some(to_grpc_rib_expr(r#""counter-export-test""#)),
                        response: Some(to_grpc_rib_expr(
                            r#"
                                {
                                    headers: {
                                        {ContentType: "application/json"}
                                    },
                                    body: "Export test response",
                                    status: 200
                                }
                            "#,
                        )),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default as i32),
                        static_binding: None,
                        invocation_context: None,
                    }),
                    middleware: None,
                }],
            },
        )),
    };

    // Create the API definition
    let response = deps
        .worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
            api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                request.clone(),
            )),
        })
        .await
        .unwrap();

    check_equal_api_definition_request_and_response(&request, &response);

    // Export the API definition
    let export_response = deps
        .worker_service()
        .export_api_definition(golem_api_grpc::proto::golem::apidefinition::v1::ExportApiDefinitionRequest {
            api_definition_id: Some(golem_api_grpc::proto::golem::apidefinition::ApiDefinitionId {
                value: api_id.clone(),
            }),
            version: "1.0".to_string(),
        })
        .await
        .unwrap();

    // Verify the export response
    let export_data = match export_response.result.unwrap() {
        golem_api_grpc::proto::golem::apidefinition::v1::export_api_definition_response::Result::Success(data) => data,
        golem_api_grpc::proto::golem::apidefinition::v1::export_api_definition_response::Result::Error(e) => {
            panic!("Export API definition failed: {:?}", e);
        }
    };

    // Parse the YAML to extract API definition details
    let parsed_yaml: Value = serde_yaml::from_str(&export_data.openapi_yaml)
        .expect("Failed to parse OpenAPI YAML");

    // Extract API definition details from YAML
    let _info = parsed_yaml["info"].as_mapping().expect("Missing info section");
    let paths = parsed_yaml["paths"].as_mapping().expect("Missing paths section");

    // Get the API ID and version from the root level extensions
    let yaml_api_id = parsed_yaml.get("x-golem-api-definition-id")
        .and_then(|v| v.as_str())
        .expect("API ID missing in YAML");
    assert_eq!(yaml_api_id, api_id, "API ID in YAML doesn't match original");

    let yaml_version = parsed_yaml.get("x-golem-api-definition-version")
        .and_then(|v| v.as_str())
        .expect("Version missing in YAML");
    assert_eq!(yaml_version, "1.0", "Version in YAML doesn't match original");

    // Get the first route (we know there's only one in this test)
    let (path, methods) = paths.iter().next().expect("No paths found");
    let method = methods["get"].as_mapping().expect("Missing GET method");
    let binding = method["x-golem-api-gateway-binding"]
        .as_mapping()
        .expect("Missing gateway binding");

    // Verify worker name matches
    let yaml_worker_name = binding.get("worker-name")
        .and_then(|v| v.as_str())
        .expect("Worker name missing in YAML");
    assert_eq!(yaml_worker_name, r#""counter-export-test""#, "Worker name in YAML doesn't match original");

    // Verify response data matches
    let yaml_response = binding.get("response")
        .and_then(|v| v.as_str())
        .expect("Response missing in YAML");
    assert!(yaml_response.contains("Export test response"), "Response in YAML doesn't match original");

    // Verify binding type is default
    let yaml_binding_type = binding.get("binding-type")
        .and_then(|v| v.as_str())
        .expect("Binding type missing in YAML");
    assert_eq!(yaml_binding_type, "default", "Binding type in YAML is not default");

    // Delete the original API definition
    deps.worker_service()
        .delete_api_definition(DeleteApiDefinitionRequest {
            api_definition_id: request.id.clone(),
            version: request.version.clone(),
        })
        .await
        .unwrap();

    // Create new API definition from parsed YAML
    let imported_request = ApiDefinitionRequest {
        id: Some(ApiDefinitionId {
            value: parsed_yaml.get("x-golem-api-definition-id")
                .and_then(|v| v.as_str())
                .map(|value| value.to_string())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
        }),
        version: parsed_yaml.get("x-golem-api-definition-version")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "1.0".to_string()),
        draft: false,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![HttpRoute {
                    method: HttpMethod::Get as i32,
                    path: path.as_str()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "/default-path".to_string()),
                    binding: Some(GatewayBinding {
                        component: Some(VersionedComponentId {
                            component_id: Some(component_id.clone().into()),
                            version: 0,
                        }),
                        worker_name: Some(to_grpc_rib_expr(yaml_worker_name)),
                        response: Some(to_grpc_rib_expr(yaml_response)),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default as i32),
                        static_binding: None,
                        invocation_context: None,
                    }),
                    middleware: None,
                }],
            },
        )),
    };

    // Create the imported API definition
    let imported_response = deps
        .worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
            api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                imported_request.clone(),
            )),
        })
        .await
        .unwrap();

    // Verify the imported API matches the original
    check_equal_api_definition_request_and_response(&request, &imported_response);
}

fn check_equal_api_definition_request_and_response(
    request: &ApiDefinitionRequest,
    response: &ApiDefinition,
) {
    check!(request.id == response.id);
    check!(request.version == response.version);
    check!(request.draft == response.draft);

    let api_definition_request::Definition::Http(request_api_def) =
        request.definition.as_ref().unwrap();
    let api_definition::Definition::Http(response_api_def) = response.definition.as_ref().unwrap();
    check!(request_api_def == response_api_def);
}

