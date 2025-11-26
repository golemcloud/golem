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

use super::Tracing;
use assert2::assert;
use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateHttpApiDefinitionError,
    RegistryServiceGetHttpApiDefinitionError,
    RegistryServiceGetHttpApiDefinitionInEnvironmentError,
    RegistryServiceUpdateHttpApiDefinitionError,
};
use golem_common::model::component::ComponentName;
use golem_common::model::http_api_definition::{
    GatewayBinding, HttpApiDefinitionCreation, HttpApiDefinitionName, HttpApiDefinitionUpdate,
    HttpApiDefinitionVersion, HttpApiRoute, RouteMethod, WorkerGatewayBinding,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{EnvironmentOptions, TestDslExtended};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_http_api_definition(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-definition".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![HttpApiRoute {
            method: RouteMethod::Post,
            path: "/{user-id}/test-path-1".to_string(),
            binding: GatewayBinding::Worker(WorkerGatewayBinding {
                component_name: ComponentName("test-component".to_string()),
                idempotency_key: None,
                invocation_context: None,
                response: r#"
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
            }),
            security: None,
        }],
    };

    let http_api_definition = client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation)
        .await?;

    assert!(http_api_definition.name == http_api_definition_creation.name);
    assert!(http_api_definition.version == http_api_definition_creation.version);
    assert!(http_api_definition.routes == http_api_definition_creation.routes);

    {
        let fetched_http_api_definition = client
            .get_http_api_definition(&http_api_definition.id.0)
            .await?;
        assert!(fetched_http_api_definition == http_api_definition);
    }

    {
        let fetched_http_api_definition = client
            .get_http_api_definition_in_environment(&env.id.0, &http_api_definition.name.0)
            .await?;
        assert!(fetched_http_api_definition == http_api_definition);
    }

    {
        let result = client
            .list_environment_http_api_definitions(&env.id.0)
            .await?;
        assert!(result.values == vec![http_api_definition]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_http_api_definition(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-definition".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![HttpApiRoute {
            method: RouteMethod::Post,
            path: "/{user-id}/test-path-1".to_string(),
            binding: GatewayBinding::Worker(WorkerGatewayBinding {
                component_name: ComponentName("test-component".to_string()),
                idempotency_key: None,
                invocation_context: None,
                response: r#"
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
            }),
            security: None,
        }],
    };

    let http_api_definition = client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation)
        .await?;

    let http_api_definition_update = HttpApiDefinitionUpdate {
        current_revision: http_api_definition.revision,
        version: Some(HttpApiDefinitionVersion("2".to_string())),
        routes: Some(Vec::new()),
    };

    let updated_http_api_definition = client
        .update_http_api_definition(&http_api_definition.id.0, &http_api_definition_update)
        .await?;

    assert!(updated_http_api_definition.id == http_api_definition.id);
    assert!(updated_http_api_definition.revision == http_api_definition.revision.next()?);
    assert!(updated_http_api_definition.version == http_api_definition_update.version.unwrap());
    assert!(updated_http_api_definition.routes == http_api_definition_update.routes.unwrap());

    {
        let fetched_http_api_definition = client
            .get_http_api_definition(&http_api_definition.id.0)
            .await?;
        assert!(fetched_http_api_definition == updated_http_api_definition);
    }

    {
        let fetched_http_api_definition = client
            .get_http_api_definition_in_environment(&env.id.0, &http_api_definition.name.0)
            .await?;
        assert!(fetched_http_api_definition == updated_http_api_definition);
    }

    {
        let result = client
            .list_environment_http_api_definitions(&env.id.0)
            .await?;
        assert!(result.values == vec![updated_http_api_definition]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_http_api_definition(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-definition".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![HttpApiRoute {
            method: RouteMethod::Post,
            path: "/{user-id}/test-path-1".to_string(),
            binding: GatewayBinding::Worker(WorkerGatewayBinding {
                component_name: ComponentName("test-component".to_string()),
                idempotency_key: None,
                invocation_context: None,
                response: r#"
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
            }),
            security: None,
        }],
    };

    let http_api_definition = client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation)
        .await?;

    client
        .delete_http_api_definition(&http_api_definition.id.0, http_api_definition.revision.0)
        .await?;

    {
        let result = client
            .get_http_api_definition(&http_api_definition.id.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDefinitionError::Error404(_)
            )) = result
        );
    }

    {
        let result = client
            .get_http_api_definition_in_environment(&env.id.0, &http_api_definition.name.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDefinitionInEnvironmentError::Error404(_)
            )) = result
        );
    }

    {
        let result = client
            .list_environment_http_api_definitions(&env.id.0)
            .await?;
        assert!(result.values == vec![]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_create_two_http_api_definitions_with_same_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-definition".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![HttpApiRoute {
            method: RouteMethod::Post,
            path: "/{user-id}/test-path-1".to_string(),
            binding: GatewayBinding::Worker(WorkerGatewayBinding {
                component_name: ComponentName("test-component".to_string()),
                idempotency_key: None,
                invocation_context: None,
                response: r#"
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
            }),
            security: None,
        }],
    };

    client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation)
        .await?;

    let result = client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDefinitionError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
#[ignore]
#[tracing::instrument]
async fn cannot_create_two_revisions_with_same_version(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user
        .app_and_env_custom(&EnvironmentOptions {
            compatibility_check: false,
            version_check: true,
            security_overrides: false,
        })
        .await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_revision = client
        .create_http_api_definition(
            &env.id.0,
            &HttpApiDefinitionCreation {
                name: HttpApiDefinitionName("test-definition".to_string()),
                version: HttpApiDefinitionVersion("1".to_string()),
                routes: Vec::new(),
            },
        )
        .await?;

    // TODO: deploy revision here as only deployed revisions are considered for version checks

    let result = client
        .update_http_api_definition(
            &http_api_revision.id.0,
            &HttpApiDefinitionUpdate {
                current_revision: http_api_revision.revision,
                version: Some(HttpApiDefinitionVersion("1".to_string())),
                routes: None,
            },
        )
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceUpdateHttpApiDefinitionError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn updates_with_wrong_revision_number_are_rejected(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_revision = client
        .create_http_api_definition(
            &env.id.0,
            &HttpApiDefinitionCreation {
                name: HttpApiDefinitionName("test-definition".to_string()),
                version: HttpApiDefinitionVersion("1".to_string()),
                routes: Vec::new(),
            },
        )
        .await?;

    let result = client
        .update_http_api_definition(
            &http_api_revision.id.0,
            &HttpApiDefinitionUpdate {
                current_revision: http_api_revision.revision.next()?,
                version: Some(HttpApiDefinitionVersion("2".to_string())),
                routes: None,
            },
        )
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceUpdateHttpApiDefinitionError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_create_http_api_definition_with_empty_version(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-definition".to_string()),
        version: HttpApiDefinitionVersion("".to_string()),
        routes: vec![],
    };

    let result = client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDefinitionError::Error400(_)
        )) = result
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_api_definition_recreation(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user
        .app_and_env_custom(&EnvironmentOptions {
            compatibility_check: false,
            version_check: true,
            security_overrides: false,
        })
        .await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_definition_creation_1 = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-definition".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![],
    };

    let http_api_definition_1 = client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation_1)
        .await?;

    client
        .delete_http_api_definition(
            &http_api_definition_1.id.0,
            http_api_definition_1.revision.0,
        )
        .await?;

    let http_api_definition_creation_2 = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("test-definition".to_string()),
        version: HttpApiDefinitionVersion("2".to_string()),
        routes: vec![],
    };

    let http_api_definition_2 = client
        .create_http_api_definition(&env.id.0, &http_api_definition_creation_2)
        .await?;

    assert!(http_api_definition_2.id == http_api_definition_1.id);
    assert!(http_api_definition_2.revision == http_api_definition_1.revision.next()?.next()?);

    client
        .delete_http_api_definition(
            &http_api_definition_2.id.0,
            http_api_definition_2.revision.0,
        )
        .await?;

    Ok(())
}
