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

use crate::{to_grpc_rib_expr, Tracing};
use assert2::{assert, check};
use golem_api_grpc::proto::golem::apidefinition::v1::{
    api_definition_request, create_api_definition_request, ApiDefinitionRequest,
    CreateApiDefinitionRequest,
};
use golem_api_grpc::proto::golem::apidefinition::{
    api_definition, ApiDefinition, ApiDefinitionId, GatewayBinding, GatewayBindingType,
    HttpApiDefinition, HttpMethod, HttpRoute,
};
use golem_api_grpc::proto::golem::component::VersionedComponentId;
use golem_client::model::{
    ApiDefinitionInfo, ApiDeployment, ApiDeploymentRequest, ApiSite, ComponentType,
};
use golem_common::model::{ComponentId, ProjectId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use std::collections::HashMap;
use std::panic;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_get_api_deployment(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let project_id = admin.default_project().await;

    let component_id = admin.component("shopping-cart").unique().store().await;

    fn new_api_definition_id(prefix: &str) -> String {
        format!("{}-{}", prefix, Uuid::new_v4())
    }

    let api_definition_1 = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("a"),
        "1".to_string(),
        "/path-1".to_string(),
    )
    .await;

    let api_definition_2 = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("b"),
        "2".to_string(),
        "/path-2".to_string(),
    )
    .await;

    let request = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![
            ApiDefinitionInfo {
                id: api_definition_1.id.as_ref().unwrap().value.clone(),
                version: api_definition_1.version.clone(),
            },
            ApiDefinitionInfo {
                id: api_definition_2.id.as_ref().unwrap().value.clone(),
                version: api_definition_2.version.clone(),
            },
        ],
        site: ApiSite {
            host: "localhost".to_string(),
            subdomain: Some("subdomain".to_string()),
        },
    };

    let response = deps
        .worker_service()
        .create_or_update_api_deployment(&admin.token, request.clone())
        .await
        .unwrap();
    check!(request.api_definitions == response.api_definitions);
    check!(request.site == response.site);

    let response = deps
        .worker_service()
        .get_api_deployment(&admin.token, &project_id, "subdomain.localhost")
        .await
        .unwrap();
    check!(request.api_definitions == response.api_definitions);
    check!(request.site == response.site);

    let api_definition_3 = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("c"),
        "1".to_string(),
        "/path-3".to_string(),
    )
    .await;

    let request = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![
            ApiDefinitionInfo {
                id: api_definition_2.id.as_ref().unwrap().value.clone(),
                version: api_definition_2.version.clone(),
            },
            ApiDefinitionInfo {
                id: api_definition_3.id.as_ref().unwrap().value.clone(),
                version: api_definition_3.version.clone(),
            },
        ],
        site: ApiSite {
            host: "localhost".to_string(),
            subdomain: Some("subdomain".to_string()),
        },
    };

    // NOTE: create_or_update does not delete previous defs
    let expected_merged = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![
            ApiDefinitionInfo {
                id: api_definition_1.id.as_ref().unwrap().value.clone(),
                version: api_definition_1.version.clone(),
            },
            ApiDefinitionInfo {
                id: api_definition_2.id.as_ref().unwrap().value.clone(),
                version: api_definition_2.version.clone(),
            },
            ApiDefinitionInfo {
                id: api_definition_3.id.as_ref().unwrap().value.clone(),
                version: api_definition_3.version.clone(),
            },
        ],
        site: ApiSite {
            host: "localhost".to_string(),
            subdomain: Some("subdomain".to_string()),
        },
    };

    let response = deps
        .worker_service()
        .create_or_update_api_deployment(&admin.token, request.clone())
        .await
        .unwrap();
    check!(expected_merged
        .api_definitions
        .iter()
        .all(|item| response.api_definitions.contains(item)));
    check!(request.site == response.site);

    let response = deps
        .worker_service()
        .get_api_deployment(&admin.token, &project_id, "subdomain.localhost")
        .await
        .unwrap();
    check!(expected_merged
        .api_definitions
        .iter()
        .all(|item| response.api_definitions.contains(item)));
    check!(request.site == response.site);

    deps.worker_service()
        .delete_api_deployment(&admin.token, &project_id, "subdomain.localhost")
        .await
        .unwrap();
    let response = deps
        .worker_service()
        .get_api_deployment(&admin.token, &project_id, "subdomain.localhost")
        .await;
    assert!(response.is_err());
    check!(response.err().unwrap().to_string().contains("not found"));
}

// Deploy API that uses shopping-cart's get-cart-contents function.
// Update the component to a different wasm file, and it should fail.
// Delete the API deployment, and the update should succeed.
#[test]
#[tracing::instrument]
async fn create_api_deployment_and_update_component(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let project_id = admin.default_project().await;

    let component_id = admin.component("shopping-cart").unique().store().await;

    fn new_api_definition_id(prefix: &str) -> String {
        format!("{}-{}", prefix, Uuid::new_v4())
    }

    let api_definition_1 = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("a"),
        "1".to_string(),
        "/path-4".to_string(),
    )
    .await;

    let request = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition_1.id.as_ref().unwrap().value.clone(),
            version: api_definition_1.version.clone(),
        }],
        site: ApiSite {
            host: "localhost".to_string(),
            subdomain: Some("subdomain-2".to_string()),
        },
    };

    deps.worker_service()
        .create_or_update_api_deployment(&admin.token, request.clone())
        .await
        .unwrap();

    // Trying to update the component (with a completely different wasm)
    // which was already used in an API definition
    // where function get-cart-contents is being used.
    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await
        .unwrap_err()
        .to_string();

    check!(update_component.contains("Component Constraint Error"));
    check!(update_component.contains("Missing Functions"));
    check!(update_component.contains("get-cart-contents"));

    // Delete the API deployment and see if component can be updated
    // as constraints should be removed after deleting the API deployment
    deps.worker_service()
        .delete_api_deployment(&admin.token, &project_id, "subdomain-2.localhost")
        .await
        .unwrap();

    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await;

    check!(update_component.is_ok());
}

// Deploy 2 API definitions, both making use of shopping-cart's get-cart-contents function.
// Update the component to a different wasm file, and it should fail.
// Delete the first API deployment, and the update should still fail.
// Delete the second API deployment, and the update should succeed.
#[test]
#[tracing::instrument]
async fn create_multiple_api_deployments_and_update_component_1(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let project_id = admin.default_project().await;

    let component_id = admin.component("shopping-cart").unique().store().await;

    fn new_api_definition_id(prefix: &str) -> String {
        format!("{}-{}", prefix, Uuid::new_v4())
    }

    let api_definition = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("a"),
        "1".to_string(),
        "/path-5".to_string(),
    )
    .await;

    // Same API definition but different subdomain
    let request1 = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition.id.as_ref().unwrap().value.clone(),
            version: api_definition.version.clone(),
        }],
        site: ApiSite {
            host: "domain1".to_string(),
            subdomain: Some("subdomain1".to_string()),
        },
    };

    let request2 = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition.id.as_ref().unwrap().value.clone(),
            version: api_definition.version.clone(),
        }],
        site: ApiSite {
            host: "domain2".to_string(),
            subdomain: Some("subdomain2".to_string()),
        },
    };

    deps.worker_service()
        .create_or_update_api_deployment(&admin.token, request1.clone())
        .await
        .unwrap();

    deps.worker_service()
        .create_or_update_api_deployment(&admin.token, request2.clone())
        .await
        .unwrap();

    // Trying to update the component (with a completely different wasm)
    // which was already used in an API definition
    // where function get-cart-contents is being used.
    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await
        .unwrap_err()
        .to_string();

    check!(update_component.contains("Component Constraint Error"));
    check!(update_component.contains("Missing Functions"));
    check!(update_component.contains("get-cart-contents"));

    // Delete one of the API deployments and see if component can be updated, and it
    // should fail as the component is still being used in subdomain2
    deps.worker_service()
        .delete_api_deployment(&admin.token, &project_id, "subdomain1.domain1")
        .await
        .unwrap();

    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await
        .unwrap_err()
        .to_string();

    check!(update_component.contains("Component Constraint Error"));
    check!(update_component.contains("Missing Functions"));
    check!(update_component.contains("get-cart-contents"));

    // Delete the final API deployment and see if component can be updated, and it should succeed
    deps.worker_service()
        .delete_api_deployment(&admin.token, &project_id, "subdomain2.domain2")
        .await
        .unwrap();

    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await;

    check!(update_component.is_ok());
}

// Deploy 2 API definitions (same component-id),
// of which only one makes use of a worker function (get-cart-contents)
// Update the component to a different wasm file, and it should fail.
// Delete the API deployment that uses worker function, and the update should succeed.
#[test]
#[tracing::instrument]
async fn create_multiple_api_deployments_and_update_component_2(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").unique().store().await;
    let project_id = admin.default_project().await;

    fn new_api_definition_id(prefix: &str) -> String {
        format!("{}-{}", prefix, Uuid::new_v4())
    }

    let api_definition1 = create_api_definition_without_worker_calls(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("a"),
        "1".to_string(),
        "/path-6".to_string(),
    )
    .await;

    let api_definition2 = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("a"),
        "1".to_string(),
        "/path-7".to_string(),
    )
    .await;

    //
    let request1 = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition1.id.as_ref().unwrap().value.clone(),
            version: api_definition1.version.clone(),
        }],
        site: ApiSite {
            host: "domain3".to_string(),
            subdomain: Some("subdomain3".to_string()),
        },
    };

    let request2 = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition2.id.as_ref().unwrap().value.clone(),
            version: api_definition2.version.clone(),
        }],
        site: ApiSite {
            host: "domain4".to_string(),
            subdomain: Some("subdomain4".to_string()),
        },
    };

    deps.worker_service()
        .create_or_update_api_deployment(&admin.token, request1.clone())
        .await
        .unwrap();

    deps.worker_service()
        .create_or_update_api_deployment(&admin.token, request2.clone())
        .await
        .unwrap();

    // Trying to update the component (with a completely different wasm)
    // which was already used in an API definition
    // where function get-cart-contents is being used.
    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await
        .unwrap_err()
        .to_string();

    check!(update_component.contains("Component Constraint Error"));
    check!(update_component.contains("Missing Functions"));
    check!(update_component.contains("get-cart-contents"));

    // Delete API deployment that was using the worker function
    deps.worker_service()
        .delete_api_deployment(&admin.token, &project_id, "subdomain4.domain4")
        .await
        .unwrap();

    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await;

    check!(update_component.is_ok());

    // Delete the final API deployment and cleanup
    deps.worker_service()
        .delete_api_deployment(&admin.token, &project_id, "subdomain3.domain3")
        .await
        .unwrap();
}

#[test]
#[tracing::instrument]
async fn get_all_api_deployments(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let project_id = admin.default_project().await;
    let component_id = admin.component("shopping-cart").unique().store().await;

    let api_definition_1 = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        Uuid::new_v4().to_string(),
        "1".to_string(),
        "/path-1".to_string(),
    )
    .await;
    let api_definition_2 = create_api_definition(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        Uuid::new_v4().to_string(),
        "2".to_string(),
        "/path-2".to_string(),
    )
    .await;

    deps.worker_service()
        .create_or_update_api_deployment(
            &admin.token,
            ApiDeploymentRequest {
                project_id: project_id.0,
                api_definitions: vec![ApiDefinitionInfo {
                    id: api_definition_1.id.as_ref().unwrap().value.clone(),
                    version: api_definition_1.version.clone(),
                }],
                site: ApiSite {
                    host: "domain".to_string(),
                    subdomain: None,
                },
            },
        )
        .await
        .unwrap();

    deps.worker_service()
        .create_or_update_api_deployment(
            &admin.token,
            ApiDeploymentRequest {
                project_id: project_id.0,
                api_definitions: vec![ApiDefinitionInfo {
                    id: api_definition_1.id.as_ref().unwrap().value.clone(),
                    version: api_definition_1.version.clone(),
                }],
                site: ApiSite {
                    host: "domain".to_string(),
                    subdomain: Some("subdomain".to_string()),
                },
            },
        )
        .await
        .unwrap();

    deps.worker_service()
        .create_or_update_api_deployment(
            &admin.token,
            ApiDeploymentRequest {
                project_id: project_id.0,
                api_definitions: vec![ApiDefinitionInfo {
                    id: api_definition_2.id.as_ref().unwrap().value.clone(),
                    version: api_definition_2.version.clone(),
                }],
                site: ApiSite {
                    host: "other-domain".to_string(),
                    subdomain: None,
                },
            },
        )
        .await
        .unwrap();

    fn by_domains(result: Vec<ApiDeployment>) -> HashMap<String, Vec<ApiDefinitionInfo>> {
        result
            .into_iter()
            .map(|api_deployment| {
                (
                    format!(
                        "{}.{}",
                        api_deployment.site.subdomain.unwrap_or_default(),
                        api_deployment.site.host
                    ),
                    api_deployment.api_definitions,
                )
            })
            .collect::<HashMap<_, _>>()
    }

    let result = by_domains(
        deps.worker_service()
            .list_api_deployments(&admin.token, &project_id, None)
            .await
            .unwrap(),
    );
    check!(result.contains_key(".domain"));
    check!(result.contains_key("subdomain.domain"));
    check!(result.contains_key(".other-domain"));

    let result = by_domains(
        deps.worker_service()
            .list_api_deployments(
                &admin.token,
                &project_id,
                Some(&api_definition_1.id.as_ref().unwrap().value),
            )
            .await
            .unwrap(),
    );
    check!(result.contains_key(".domain"));
    check!(result.contains_key("subdomain.domain"));
    check!(!result.contains_key(".other-domain"));

    let result = by_domains(
        deps.worker_service()
            .list_api_deployments(
                &admin.token,
                &project_id,
                Some(&api_definition_2.id.as_ref().unwrap().value),
            )
            .await
            .unwrap(),
    );
    check!(!result.contains_key(".domain"));
    check!(!result.contains_key("subdomain.domain"));
    check!(result.contains_key(".other-domain"));
}

async fn create_api_definition_without_worker_calls(
    deps: &EnvBasedTestDependencies,
    token: &Uuid,
    project: &ProjectId,
    component_id: &ComponentId,
    api_definition_id: String,
    version: String,
    path: String,
) -> ApiDefinition {
    deps.worker_service()
        .create_api_definition(
            token,
            project,
            CreateApiDefinitionRequest {
                api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                    ApiDefinitionRequest {
                        id: Some(ApiDefinitionId {
                            value: api_definition_id,
                        }),
                        version,
                        draft: false,
                        definition: Some(api_definition_request::Definition::Http(
                            HttpApiDefinition {
                                routes: vec![HttpRoute {
                                    method: HttpMethod::Post as i32,
                                    path,
                                    binding: Some(GatewayBinding {
                                        component: Some(VersionedComponentId {
                                            component_id: Some(component_id.clone().into()),
                                            version: 0,
                                        }),
                                        worker_name: None,
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
                    },
                )),
            },
        )
        .await
        .unwrap()
}

async fn create_api_definition(
    deps: &EnvBasedTestDependencies,
    token: &Uuid,
    project: &ProjectId,
    component_id: &ComponentId,
    api_definition_id: String,
    version: String,
    path: String,
) -> ApiDefinition {
    deps.worker_service()
        .create_api_definition(
            token,
            project,
            CreateApiDefinitionRequest {
                api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                    ApiDefinitionRequest {
                        id: Some(ApiDefinitionId {
                            value: api_definition_id,
                        }),
                        version,
                        draft: false,
                        definition: Some(api_definition_request::Definition::Http(
                            HttpApiDefinition {
                                routes: vec![HttpRoute {
                                    method: HttpMethod::Post as i32,
                                    path,
                                    binding: Some(GatewayBinding {
                                        component: Some(VersionedComponentId {
                                            component_id: Some(component_id.clone().into()),
                                            version: 0,
                                        }),
                                        worker_name: None,
                                        response: Some(to_grpc_rib_expr(
                                            r#"
                                                let worker = instance("shopping-cart");
                                                let result = worker.get-cart-contents();
                                                let status: u64 = 200;
                                                {
                                                headers: { ContentType: "json", userid: "foo" },
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
                    },
                )),
            },
        )
        .await
        .unwrap()
}

#[test]
#[tracing::instrument]
async fn undeploy_api_test(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").unique().store().await;
    let project = admin.default_project().await;

    let api_definition_1 = create_api_definition(
        deps,
        &admin.token,
        &project,
        &component_id,
        Uuid::new_v4().to_string(),
        "1".to_string(),
        "/api/v1/path-1".to_string(),
    )
    .await;

    let api_definition_2 = create_api_definition(
        deps,
        &admin.token,
        &project,
        &component_id,
        Uuid::new_v4().to_string(),
        "2".to_string(),
        "/api/v2/path-2".to_string(),
    )
    .await;

    // Deploy both APIs to the same site
    deps.worker_service()
        .create_or_update_api_deployment(
            &admin.token,
            ApiDeploymentRequest {
                project_id: project.0,
                api_definitions: vec![
                    ApiDefinitionInfo {
                        id: api_definition_1.id.as_ref().unwrap().value.clone(),
                        version: api_definition_1.version.clone(),
                    },
                    ApiDefinitionInfo {
                        id: api_definition_2.id.as_ref().unwrap().value.clone(),
                        version: api_definition_2.version.clone(),
                    },
                ],
                site: ApiSite {
                    host: "localhost".to_string(),
                    subdomain: Some("undeploy-test".to_string()),
                },
            },
        )
        .await
        .unwrap();

    // List deployments and check both are present
    let deployments = deps
        .worker_service()
        .list_api_deployments(&admin.token, &project, None)
        .await
        .unwrap();
    check!(deployments
        .iter()
        .any(|d| d.api_definitions.contains(&ApiDefinitionInfo {
            id: api_definition_1.id.as_ref().unwrap().value.clone(),
            version: api_definition_1.version.clone(),
        })));
    check!(deployments
        .iter()
        .any(|d| d.api_definitions.contains(&ApiDefinitionInfo {
            id: api_definition_2.id.as_ref().unwrap().value.clone(),
            version: api_definition_2.version.clone(),
        })));

    // Undeploy API 1
    deps.worker_service()
        .undeploy_api(
            &admin.token,
            &project,
            "undeploy-test.localhost",
            &api_definition_1.id.as_ref().unwrap().value,
            &api_definition_1.version,
        )
        .await
        .unwrap();

    // Verify that API 1 is no longer in the deployments
    let deployments = deps
        .worker_service()
        .list_api_deployments(&admin.token, &project, None)
        .await
        .unwrap();
    check!(!deployments
        .iter()
        .any(|d| d.api_definitions.contains(&ApiDefinitionInfo {
            id: api_definition_1.id.as_ref().unwrap().value.clone(),
            version: api_definition_1.version.clone(),
        })));

    // Verify that API 2 is still in the deployments
    check!(deployments
        .iter()
        .any(|d| d.api_definitions.contains(&ApiDefinitionInfo {
            id: api_definition_2.id.as_ref().unwrap().value.clone(),
            version: api_definition_2.version.clone(),
        })));

    // Test undeploying from a non-existent API
    let result = deps
        .worker_service()
        .undeploy_api(
            &admin.token,
            &project,
            "subdomain.localhost",
            "non-existent-id",
            "1",
        )
        .await;
    assert!(result.is_err());

    // Test undeploying from a non-existent site
    let result = deps
        .worker_service()
        .undeploy_api(
            &admin.token,
            &project,
            "non-existent.localhost",
            &api_definition_2.id.as_ref().unwrap().value,
            &api_definition_2.version,
        )
        .await;
    assert!(result.is_err());
}

#[test]
#[tracing::instrument]
async fn undeploy_component_constraint_test(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let project = admin.default_project().await;

    let component_id = admin.component("shopping-cart").unique().store().await;

    fn new_api_definition_id(prefix: &str) -> String {
        format!("{}-{}", prefix, Uuid::new_v4())
    }

    let api_definition_1 = create_api_definition(
        deps,
        &admin.token,
        &project,
        &component_id,
        new_api_definition_id("a"),
        "1".to_string(),
        "/path-undeploy".to_string(),
    )
    .await;

    let request = ApiDeploymentRequest {
        project_id: project.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition_1.id.as_ref().unwrap().value.clone(),
            version: api_definition_1.version.clone(),
        }],
        site: ApiSite {
            host: "localhost".to_string(),
            subdomain: Some("undeploy-test".to_string()),
        },
    };

    deps.worker_service()
        .create_or_update_api_deployment(&admin.token, request.clone())
        .await
        .unwrap();

    // Trying to update the component (with a completely different wasm)
    // which was already used in an API definition
    // where function get-cart-contents is being used.
    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await
        .unwrap_err()
        .to_string();

    check!(update_component.contains("Component Constraint Error"));
    check!(update_component.contains("Missing Functions"));
    check!(update_component.contains("get-cart-contents"));

    // Undeploy the API and see if component can be updated
    // as constraints should be removed after undeploying the API
    deps.worker_service()
        .undeploy_api(
            &admin.token,
            &project,
            "undeploy-test.localhost",
            &api_definition_1.id.as_ref().unwrap().value,
            &api_definition_1.version,
        )
        .await
        .unwrap();

    let update_component = deps
        .component_service()
        .update_component(
            &admin.token,
            &component_id,
            &deps.component_directory().join("counters.wasm"),
            ComponentType::Durable,
            None,
            None,
            &HashMap::new(),
        )
        .await;

    check!(update_component.is_ok());
}

async fn create_shopping_cart_api_definition_with_two_routes(
    deps: &EnvBasedTestDependencies,
    token: &Uuid,
    project: &ProjectId,
    component_id: &ComponentId,
    api_definition_id: String,
    version: String,
) -> ApiDefinition {
    deps.worker_service()
        .create_api_definition(
            token,
            project,
            CreateApiDefinitionRequest {
                api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                    ApiDefinitionRequest {
                        id: Some(ApiDefinitionId {
                            value: api_definition_id,
                        }),
                        version,
                        draft: false,
                        definition: Some(api_definition_request::Definition::Http(
                            HttpApiDefinition {
                                routes: vec![
                                    // Route for adding content to cart
                                    HttpRoute {
                                        method: HttpMethod::Post as i32,
                                        path: "/v1/static-worker/add-to-cart".to_string(),
                                        binding: Some(GatewayBinding {
                                            component: Some(VersionedComponentId {
                                                component_id: Some(component_id.clone().into()),
                                                version: 0,
                                            }),
                                            worker_name: None,
                                            response: Some(to_grpc_rib_expr(
                                                r#"
                                                    let worker = instance("static-worker");
                                                    worker.add-item(request.body);
                                                    {
                                                        headers: { ContentType: "json" },
                                                        body: "successfully added item to the cart",
                                                        status: 201
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
                                    // Route for getting cart contents
                                    HttpRoute {
                                        method: HttpMethod::Get as i32,
                                        path: "/v1/static-worker/get-cart-contents".to_string(),
                                        binding: Some(GatewayBinding {
                                            component: Some(VersionedComponentId {
                                                component_id: Some(component_id.clone().into()),
                                                version: 0,
                                            }),
                                            worker_name: None,
                                            response: Some(to_grpc_rib_expr(
                                                r#"
                                                    let worker = instance("static-worker");
                                                    let result = worker.get-cart-contents();
                                                    {
                                                        headers: { ContentType: "json" },
                                                        body: result,
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
                                    },
                                ],
                            },
                        )),
                    },
                )),
            },
        )
        .await
        .unwrap()
}

#[test]
#[tracing::instrument]
async fn deploy_api_and_make_worker_calls(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;
    let project_id = admin.default_project().await;
    let component_id = admin.component("shopping-cart").unique().store().await;

    fn new_api_definition_id(prefix: &str) -> String {
        format!("{}-{}", prefix, Uuid::new_v4())
    }

    // Create API definition with two routes: add-content and cart-contents
    let api_definition = create_shopping_cart_api_definition_with_two_routes(
        deps,
        &admin.token,
        &project_id,
        &component_id,
        new_api_definition_id("shopping-cart-api"),
        "1".to_string(),
    )
    .await;

    // Deploy the API to localhost:9093
    // http service gateway runs on port 9093 in test-framework
    let request = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition.id.as_ref().unwrap().value.clone(),
            version: api_definition.version.clone(),
        }],
        site: ApiSite {
            host: "localhost:9093".to_string(),
            subdomain: None,
        },
    };

    let response = deps
        .worker_service()
        .create_or_update_api_deployment(&admin.token, request.clone())
        .await
        .unwrap();

    // Verify the deployment was successful
    check!(request.api_definitions == response.api_definitions);
    check!(request.site == response.site);

    // Verify we can retrieve the deployment
    let retrieved_deployment = deps
        .worker_service()
        .get_api_deployment(&admin.token, &project_id, "localhost:9093")
        .await
        .unwrap();
    check!(request.api_definitions == retrieved_deployment.api_definitions);
    check!(request.site == retrieved_deployment.site);

    // Verify the API definition has both main routes
    let definition = api_definition.definition.unwrap();
    match definition {
        api_definition::Definition::Http(http_def) => {
            check!(http_def.routes.len() == 2);

            // Check that we have both expected main routes
            let has_add_to_cart = http_def.routes.iter().any(|route| {
                route.method == HttpMethod::Post as i32
                    && route.path == "/v1/static-worker/add-to-cart"
            });
            let has_get_cart_contents = http_def.routes.iter().any(|route| {
                route.method == HttpMethod::Get as i32
                    && route.path == "/v1/static-worker/get-cart-contents"
            });

            check!(has_add_to_cart);
            check!(has_get_cart_contents);
        }
    }

    // Wait a moment for the deployment to be ready
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Create HTTP client to test the deployed API
    let http_client = reqwest::Client::new();

    // Test data for the products
    let product1 = serde_json::json!({
        "product-id": "foo",
        "name": "foo",
        "price": 42,
        "quantity": 42
    });

    let product2 = serde_json::json!({
        "product-id": "bar",
        "name": "bar",
        "price": 100,
        "quantity": 1
    });

    // Add first product to cart
    let add_product1 = http_client
        .post("http://localhost:9093/v1/static-worker/add-to-cart")
        .json(&product1)
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap();

    check!(add_product1.status().is_success());
    let add_product1_text = add_product1.text().await.unwrap();
    check!(add_product1_text.contains("successfully added item to the cart"));

    // Add second product to cart
    let add_product2 = http_client
        .post("http://localhost:9093/v1/static-worker/add-to-cart")
        .json(&product2)
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap();

    check!(add_product2.status().is_success());
    let add_product2_text = add_product2.text().await.unwrap();
    check!(add_product2_text.contains("successfully added item to the cart"));

    // Get cart contents and verify both products are there
    let get_cart_contents = http_client
        .get("http://localhost:9093/v1/static-worker/get-cart-contents")
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap();

    check!(get_cart_contents.status().is_success());
    let cart_contents: Vec<serde_json::Value> = get_cart_contents.json().await.unwrap();
    check!(cart_contents.len() == 2);

    // Verify the exact contents of the cart
    let expected_product1 = serde_json::json!({
        "name": "foo",
        "price": 42.0,
        "product-id": "foo",
        "quantity": 42
    });
    let expected_product2 = serde_json::json!({
        "name": "bar",
        "price": 100.0,
        "product-id": "bar",
        "quantity": 1
    });

    // Check that both products are in the cart with exact values
    check!(cart_contents.contains(&expected_product1));
    check!(cart_contents.contains(&expected_product2));

    // Clean up - delete the deployment
    deps.worker_service()
        .delete_api_deployment(&admin.token, &project_id, "localhost:9093")
        .await
        .unwrap();

    // Verify the deployment was deleted
    let response = deps
        .worker_service()
        .get_api_deployment(&admin.token, &project_id, "localhost:9093")
        .await;
    assert!(response.is_err());
    check!(response.err().unwrap().to_string().contains("not found"));
}
