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
