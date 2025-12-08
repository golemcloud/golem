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
    RegistryServiceClient, RegistryServiceCreateHttpApiDeploymentError,
    RegistryServiceGetHttpApiDeploymentError,
    RegistryServiceGetHttpApiDeploymentInEnvironmentError,
    RegistryServiceUpdateHttpApiDeploymentError,
};
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentCreation, HttpApiDeploymentUpdate,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
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
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDeploymentError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
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
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    assert!(http_api_deployment_creation.domain == http_api_deployment_creation.domain);

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment(&http_api_deployment.id.0)
            .await?;
        assert!(fetched_http_api_deployment == http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_environment(&env.id.0, &http_api_deployment.domain.0)
            .await?;
        assert!(fetched_http_api_deployment == http_api_deployment);
    }

    {
        let result = client
            .list_http_api_deployments_in_environment(&env.id.0)
            .await?;
        assert!(result.values == vec![http_api_deployment]);
    }

    Ok(())
}

#[test]
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
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let http_api_deployment_update = HttpApiDeploymentUpdate {
        current_revision: http_api_deployment.revision,
        api_definitions: Some(vec![
            HttpApiDefinitionName("test-api".to_string()),
            HttpApiDefinitionName("test-api-2".to_string()),
        ]),
    };

    let updated_http_api_deployment = client
        .update_http_api_deployment(&http_api_deployment.id.0, &http_api_deployment_update)
        .await?;

    assert!(updated_http_api_deployment.id == http_api_deployment.id);
    assert!(updated_http_api_deployment.revision == http_api_deployment.revision.next()?);
    assert!(
        updated_http_api_deployment.api_definitions
            == http_api_deployment_update.api_definitions.unwrap()
    );

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_revision(
                &http_api_deployment.id.0,
                http_api_deployment.revision.into(),
            )
            .await?;
        assert!(fetched_http_api_deployment == http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_revision(
                &http_api_deployment.id.0,
                updated_http_api_deployment.revision.into(),
            )
            .await?;
        assert!(fetched_http_api_deployment == updated_http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment(&http_api_deployment.id.0)
            .await?;
        assert!(fetched_http_api_deployment == updated_http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_environment(&env.id.0, &http_api_deployment.domain.0)
            .await?;
        assert!(fetched_http_api_deployment == updated_http_api_deployment);
    }

    {
        let result = client
            .list_http_api_deployments_in_environment(&env.id.0)
            .await?;
        assert!(result.values == vec![updated_http_api_deployment]);
    }

    Ok(())
}

#[test]
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
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    client
        .delete_http_api_deployment(
            &http_api_deployment.id.0,
            http_api_deployment.revision.into(),
        )
        .await?;

    {
        let result = client
            .get_http_api_deployment(&http_api_deployment.id.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDeploymentError::Error404(_)
            )) = result
        );
    }

    {
        let result = client
            .get_http_api_deployment_in_environment(&env.id.0, &http_api_deployment.domain.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDeploymentInEnvironmentError::Error404(_)
            )) = result
        );
    }

    {
        let result = client
            .list_http_api_deployments_in_environment(&env.id.0)
            .await?;
        assert!(result.values == vec![]);
    }

    Ok(())
}

#[test]
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
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let result = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDeploymentError::Error409(_)
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
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain,
        api_definitions: vec![HttpApiDefinitionName("test-api".to_string())],
    };

    let http_api_deployment = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let http_api_deployment_update = HttpApiDeploymentUpdate {
        current_revision: http_api_deployment.revision.next()?,
        api_definitions: Some(vec![
            HttpApiDefinitionName("test-api".to_string()),
            HttpApiDefinitionName("test-api-2".to_string()),
        ]),
    };

    let result = client
        .update_http_api_deployment(&http_api_deployment.id.0, &http_api_deployment_update)
        .await;

    assert!(
        let Err(golem_client::Error::Item(
            RegistryServiceUpdateHttpApiDeploymentError::Error409(_)
        )) = result
    );

    Ok(())
}

#[test]
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
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    client
        .delete_http_api_deployment(
            &http_api_deployment_1.id.0,
            http_api_deployment_1.revision.into(),
        )
        .await?;

    let http_api_deployment_2 = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    assert!(http_api_deployment_2.id == http_api_deployment_1.id);
    assert!(http_api_deployment_2.revision == http_api_deployment_1.revision.next()?.next()?);

    client
        .delete_http_api_deployment(
            &http_api_deployment_2.id.0,
            http_api_deployment_2.revision.into(),
        )
        .await?;

    Ok(())
}
