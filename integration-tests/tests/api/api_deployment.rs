use crate::{to_grpc_rib_expr, Tracing};
use assert2::{assert, check};
use golem_api_grpc::proto::golem::apidefinition::v1::{
    api_definition_request, create_api_definition_request, ApiDefinitionRequest,
    CreateApiDefinitionRequest,
};
use golem_api_grpc::proto::golem::apidefinition::{
    ApiDefinition, ApiDefinitionId, GatewayBinding, GatewayBindingType, HttpApiDefinition,
    HttpMethod, HttpRoute,
};
use golem_api_grpc::proto::golem::component::VersionedComponentId;
use golem_client::model::{ApiDefinitionInfo, ApiDeployment, ApiDeploymentRequest, ApiSite};
use golem_common::model::ComponentId;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_get_api_deployment(deps: &EnvBasedTestDependencies) {
    let component_id = deps.component("counters").unique().store().await;

    fn new_api_definition_id(prefix: &str) -> String {
        format!("{}-{}", prefix, Uuid::new_v4())
    }

    let api_definition_1 = create_api_definition(
        deps,
        &component_id,
        new_api_definition_id("a"),
        "1".to_string(),
        "/path-1".to_string(),
    )
    .await;
    let api_definition_2 = create_api_definition(
        deps,
        &component_id,
        new_api_definition_id("b"),
        "2".to_string(),
        "/path-2".to_string(),
    )
    .await;

    let request = ApiDeploymentRequest {
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
        .create_or_update_api_deployment(request.clone())
        .await
        .unwrap();
    check!(request.api_definitions == response.api_definitions);
    check!(request.site == response.site);

    let response = deps
        .worker_service()
        .get_api_deployment("subdomain.localhost")
        .await
        .unwrap();
    check!(request.api_definitions == response.api_definitions);
    check!(request.site == response.site);

    let api_definition_3 = create_api_definition(
        deps,
        &component_id,
        new_api_definition_id("c"),
        "1".to_string(),
        "/path-3".to_string(),
    )
    .await;

    let request = ApiDeploymentRequest {
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
        .create_or_update_api_deployment(request.clone())
        .await
        .unwrap();
    check!(expected_merged.api_definitions == response.api_definitions);
    check!(request.site == response.site);

    let response = deps
        .worker_service()
        .get_api_deployment("subdomain.localhost")
        .await
        .unwrap();
    check!(expected_merged.api_definitions == response.api_definitions);
    check!(request.site == response.site);

    deps.worker_service()
        .delete_api_deployment("subdomain.localhost")
        .await
        .unwrap();
    let response = deps
        .worker_service()
        .get_api_deployment("subdomain.localhost")
        .await;
    assert!(response.is_err());
    check!(response.err().unwrap().to_string().contains("not found"));
}

#[test]
#[tracing::instrument]
async fn get_all_api_deployments(deps: &EnvBasedTestDependencies) {
    let component_id = deps.component("counters").unique().store().await;

    let api_definition_1 = create_api_definition(
        deps,
        &component_id,
        Uuid::new_v4().to_string(),
        "1".to_string(),
        "/path-1".to_string(),
    )
    .await;
    let api_definition_2 = create_api_definition(
        deps,
        &component_id,
        Uuid::new_v4().to_string(),
        "2".to_string(),
        "/path-2".to_string(),
    )
    .await;

    deps.worker_service()
        .create_or_update_api_deployment(ApiDeploymentRequest {
            api_definitions: vec![ApiDefinitionInfo {
                id: api_definition_1.id.as_ref().unwrap().value.clone(),
                version: api_definition_1.version.clone(),
            }],
            site: ApiSite {
                host: "domain".to_string(),
                subdomain: None,
            },
        })
        .await
        .unwrap();
    deps.worker_service()
        .create_or_update_api_deployment(ApiDeploymentRequest {
            api_definitions: vec![ApiDefinitionInfo {
                id: api_definition_1.id.as_ref().unwrap().value.clone(),
                version: api_definition_1.version.clone(),
            }],
            site: ApiSite {
                host: "domain".to_string(),
                subdomain: Some("subdomain".to_string()),
            },
        })
        .await
        .unwrap();
    deps.worker_service()
        .create_or_update_api_deployment(ApiDeploymentRequest {
            api_definitions: vec![ApiDefinitionInfo {
                id: api_definition_2.id.as_ref().unwrap().value.clone(),
                version: api_definition_2.version.clone(),
            }],
            site: ApiSite {
                host: "other-domain".to_string(),
                subdomain: None,
            },
        })
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
            .list_api_deployments(None)
            .await
            .unwrap(),
    );
    check!(result.contains_key(".domain"));
    check!(result.contains_key("subdomain.domain"));
    check!(result.contains_key(".other-domain"));

    let result = by_domains(
        deps.worker_service()
            .list_api_deployments(Some(&api_definition_1.id.as_ref().unwrap().value))
            .await
            .unwrap(),
    );
    check!(result.contains_key(".domain"));
    check!(result.contains_key("subdomain.domain"));
    check!(!result.contains_key(".other-domain"));

    let result = by_domains(
        deps.worker_service()
            .list_api_deployments(Some(&api_definition_2.id.as_ref().unwrap().value))
            .await
            .unwrap(),
    );
    check!(!result.contains_key(".domain"));
    check!(!result.contains_key("subdomain.domain"));
    check!(result.contains_key(".other-domain"));
}

async fn create_api_definition(
    deps: &EnvBasedTestDependencies,
    component_id: &ComponentId,
    api_definition_id: String,
    version: String,
    path: String,
) -> ApiDefinition {
    deps.worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
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
                                    worker_name: Some(to_grpc_rib_expr(r#""counter""#)),
                                    response: Some(to_grpc_rib_expr(
                                        r#"
                                            let status: u64 = 200;
                                            {
                                              headers: {
                                                {ContentType: "json", userid: "foo"}
                                              },
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
        })
        .await
        .unwrap()
}
