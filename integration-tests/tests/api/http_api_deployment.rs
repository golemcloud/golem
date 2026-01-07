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

use assert2::assert;
use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateHttpApiDeploymentLegacyError,
    RegistryServiceGetHttpApiDeploymentInEnvironmentLegacyError,
    RegistryServiceGetHttpApiDeploymentLegacyError,
    RegistryServiceUpdateHttpApiDeploymentLegacyError,
};
use golem_common::model::component::ComponentName;
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_definition::{
    GatewayBinding, HttpApiDefinitionCreation, HttpApiDefinitionName, HttpApiDefinitionVersion,
    HttpApiRoute, RouteMethod, WorkerGatewayBinding,
};
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentCreation, HttpApiDeploymentUpdate,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn create_http_api_deployment_for_nonexitant_domain(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: Domain("testdomain.com".to_string()),
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    let result = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDeploymentLegacyError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn create_http_api_deployment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain,
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    let http_api_deployment = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    assert!(http_api_deployment_creation.domain == http_api_deployment_creation.domain);

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_legacy(&http_api_deployment.id.0)
            .await?;
        assert!(fetched_http_api_deployment == http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_environment_legacy(&env.id.0, &http_api_deployment.domain.0)
            .await?;
        assert!(fetched_http_api_deployment == http_api_deployment);
    }

    {
        let result = client
            .list_http_api_deployments_in_environment_legacy(&env.id.0)
            .await?;
        assert!(result.values == vec![http_api_deployment]);
    }

    Ok(())
}

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn update_http_api_deployment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain,
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    let http_api_deployment = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    let http_api_deployment_update = HttpApiDeploymentUpdate {
        current_revision: http_api_deployment.revision,
        api_definitions: Some(vec![
            HttpApiDefinitionName("test-api".to_string()),
            HttpApiDefinitionName("test-api-2".to_string()),
        ]),
    };

    let updated_http_api_deployment = client
        .update_http_api_deployment_legacy(&http_api_deployment.id.0, &http_api_deployment_update)
        .await?;

    assert!(updated_http_api_deployment.id == http_api_deployment.id);
    assert!(updated_http_api_deployment.revision == http_api_deployment.revision.next()?);
    assert!(
        updated_http_api_deployment.api_definitions
            == http_api_deployment_update.api_definitions.unwrap()
    );

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_revision_legacy(
                &http_api_deployment.id.0,
                http_api_deployment.revision.into(),
            )
            .await?;
        assert!(fetched_http_api_deployment == http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_revision_legacy(
                &http_api_deployment.id.0,
                updated_http_api_deployment.revision.into(),
            )
            .await?;
        assert!(fetched_http_api_deployment == updated_http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_legacy(&http_api_deployment.id.0)
            .await?;
        assert!(fetched_http_api_deployment == updated_http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_environment_legacy(&env.id.0, &http_api_deployment.domain.0)
            .await?;
        assert!(fetched_http_api_deployment == updated_http_api_deployment);
    }

    {
        let result = client
            .list_http_api_deployments_in_environment_legacy(&env.id.0)
            .await?;
        assert!(result.values == vec![updated_http_api_deployment]);
    }

    Ok(())
}

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn delete_http_api_deployment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain,
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    let http_api_deployment = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    client
        .delete_http_api_deployment_legacy(
            &http_api_deployment.id.0,
            http_api_deployment.revision.into(),
        )
        .await?;

    {
        let result = client
            .get_http_api_deployment_legacy(&http_api_deployment.id.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDeploymentLegacyError::Error404(_)
            )) = result
        );
    }

    {
        let result = client
            .get_http_api_deployment_in_environment_legacy(&env.id.0, &http_api_deployment.domain.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDeploymentInEnvironmentLegacyError::Error404(_)
            )) = result
        );
    }

    {
        let result = client
            .list_http_api_deployments_in_environment_legacy(&env.id.0)
            .await?;
        assert!(result.values == vec![]);
    }

    Ok(())
}

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn cannot_create_two_http_api_deployments_for_same_domain(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain,
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    let result = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDeploymentLegacyError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn updates_with_wrong_revision_number_are_rejected(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain,
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    let http_api_deployment = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    let http_api_deployment_update = HttpApiDeploymentUpdate {
        current_revision: http_api_deployment.revision.next()?,
        api_definitions: Some(vec![
            HttpApiDefinitionName("test-api".to_string()),
            HttpApiDefinitionName("test-api-2".to_string()),
        ]),
    };

    let result = client
        .update_http_api_deployment_legacy(&http_api_deployment.id.0, &http_api_deployment_update)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceUpdateHttpApiDeploymentLegacyError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn http_api_deployment_recreation(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain,
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    let http_api_deployment_1 = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    client
        .delete_http_api_deployment_legacy(
            &http_api_deployment_1.id.0,
            http_api_deployment_1.revision.into(),
        )
        .await?;

    let http_api_deployment_2 = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    assert!(http_api_deployment_2.id == http_api_deployment_1.id);
    assert!(http_api_deployment_2.revision == http_api_deployment_1.revision.next()?.next()?);

    client
        .delete_http_api_deployment_legacy(
            &http_api_deployment_2.id.0,
            http_api_deployment_2.revision.into(),
        )
        .await?;

    Ok(())
}

#[test]
#[ignore = "disabled until code-first routes"]
#[tracing::instrument]
async fn fetch_in_deployment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    user.component(&env.id, "golem_it_constructor_parameter_echo")
        .name("golem-it:constructor-parameter-echo")
        .store()
        .await?;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("echo-api".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![HttpApiRoute {
            method: RouteMethod::Post,
            path: "/echo/{param}".to_string(),
            binding: GatewayBinding::Worker(WorkerGatewayBinding {
                component_name: ComponentName("golem-it:constructor-parameter-echo".to_string()),
                idempotency_key: None,
                invocation_context: None,
                response: r#"
                    let param = request.path.param;
                    let agent = ephemeral-echo-agent("${param}");
                    let result = agent.change-and-get();
                    {
                        body: {
                            result: result
                        },
                        status: 200
                    }
                "#
                .to_string(),
            }),
            security: None,
        }],
    };

    let http_api_definition = client
        .create_http_api_definition_legacy(&env.id.0, &http_api_definition_creation)
        .await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        api_definitions: vec![http_api_definition.name],
    };

    let http_api_deployment = client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    let deployment = user.deploy_environment(&env.id).await?;

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_deployment_legacy(
                &env.id.0,
                deployment.revision.into(),
                &domain.0,
            )
            .await?;
        assert!(fetched_http_api_deployment == http_api_deployment);
    }

    {
        let fetched_http_api_deployments = client
            .list_http_api_deployments_in_deployment_legacy(&env.id.0, deployment.revision.into())
            .await?;
        assert!(fetched_http_api_deployments.values == vec![http_api_deployment]);
    }

    Ok(())
}
