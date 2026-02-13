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

use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateHttpApiDeploymentError,
    RegistryServiceDeleteHttpApiDeploymentError, RegistryServiceGetHttpApiDeploymentError,
    RegistryServiceGetHttpApiDeploymentInEnvironmentError,
    RegistryServiceUpdateHttpApiDeploymentError,
};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation, HttpApiDeploymentUpdate,
};
use pretty_assertions::assert_eq;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::collections::BTreeMap;
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
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let result = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDeploymentError::Error409(_)
        ))
    ));

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
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let http_api_deployment = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    assert_eq!(http_api_deployment.domain, http_api_deployment_creation.domain);

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment(&http_api_deployment.id.0)
            .await?;
        assert_eq!(fetched_http_api_deployment, http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_environment(&env.id.0, &http_api_deployment.domain.0)
            .await?;
        assert_eq!(fetched_http_api_deployment, http_api_deployment);
    }

    {
        let result = client
            .list_http_api_deployments_in_environment(&env.id.0)
            .await?;
        assert_eq!(result.values, vec![http_api_deployment]);
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
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let http_api_deployment = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let http_api_deployment_update = HttpApiDeploymentUpdate {
        current_revision: http_api_deployment.revision,
        agents: Some(BTreeMap::from_iter([
            (
                AgentTypeName("test-api".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("test-api-2".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
        ])),
        webhook_url: Some("/webhooks2/".to_string()),
    };

    let updated_http_api_deployment = client
        .update_http_api_deployment(&http_api_deployment.id.0, &http_api_deployment_update)
        .await?;

    assert_eq!(updated_http_api_deployment.id, http_api_deployment.id);
    assert_eq!(updated_http_api_deployment.revision, http_api_deployment.revision.next()?);
    assert_eq!(
        updated_http_api_deployment.webhooks_url,
        http_api_deployment_update.webhook_url.unwrap()
    );
    assert_eq!(updated_http_api_deployment.agents, http_api_deployment_update.agents.unwrap());

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_revision(
                &http_api_deployment.id.0,
                http_api_deployment.revision.into(),
            )
            .await?;
        assert_eq!(fetched_http_api_deployment, http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_revision(
                &http_api_deployment.id.0,
                updated_http_api_deployment.revision.into(),
            )
            .await?;
        assert_eq!(fetched_http_api_deployment, updated_http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment(&http_api_deployment.id.0)
            .await?;
        assert_eq!(fetched_http_api_deployment, updated_http_api_deployment);
    }

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_environment(&env.id.0, &http_api_deployment.domain.0)
            .await?;
        assert_eq!(fetched_http_api_deployment, updated_http_api_deployment);
    }

    {
        let result = client
            .list_http_api_deployments_in_environment(&env.id.0)
            .await?;
        assert_eq!(result.values, vec![updated_http_api_deployment]);
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
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
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
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDeploymentError::Error404(_)
            ))
        ));
    }

    {
        let result = client
            .get_http_api_deployment_in_environment(&env.id.0, &http_api_deployment.domain.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetHttpApiDeploymentInEnvironmentError::Error404(_)
            ))
        ));
    }

    {
        let result = client
            .list_http_api_deployments_in_environment(&env.id.0)
            .await?;
        assert_eq!(result.values, vec![]);
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
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let result = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateHttpApiDeploymentError::Error409(_)
        ))
    ));

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
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let http_api_deployment = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let http_api_deployment_update = HttpApiDeploymentUpdate {
        current_revision: http_api_deployment.revision.next()?,
        webhook_url: None,
        agents: Some(BTreeMap::from_iter([
            (
                AgentTypeName("test-api".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("test-api-2".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
        ])),
    };

    let result = client
        .update_http_api_deployment(&http_api_deployment.id.0, &http_api_deployment_update)
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceUpdateHttpApiDeploymentError::Error409(_)
        ))
    ));

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
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
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

    assert_eq!(http_api_deployment_2.id, http_api_deployment_1.id);
    assert_eq!(http_api_deployment_2.revision, http_api_deployment_1.revision.next()?.next()?);

    client
        .delete_http_api_deployment(
            &http_api_deployment_2.id.0,
            http_api_deployment_2.revision.into(),
        )
        .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fetch_in_deployment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    user.component(&env.id, "golem_it_agent_http_routes_ts")
        .name("golem-it:agent-http-routes-ts")
        .store()
        .await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        agents: BTreeMap::from_iter([(
            AgentTypeName("http-agent".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let http_api_deployment = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let deployment = user.deploy_environment(&env.id).await?;

    {
        let fetched_http_api_deployment = client
            .get_http_api_deployment_in_deployment(&env.id.0, deployment.revision.into(), &domain.0)
            .await?;
        assert_eq!(fetched_http_api_deployment, http_api_deployment);
    }

    {
        let fetched_http_api_deployments = client
            .list_http_api_deployments_in_deployment(&env.id.0, deployment.revision.into())
            .await?;
        assert_eq!(fetched_http_api_deployments.values, vec![http_api_deployment]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_access_http_api_deployment_from_another_user(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_a = deps.user().await?;
    let (_, env_a) = user_a.app_and_env().await?;
    let domain = user_a.register_domain(&env_a.id).await?;

    let client_a = deps.registry_service().client(&user_a.token).await;

    let creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let deployment = client_a
        .create_http_api_deployment(&env_a.id.0, &creation)
        .await?;

    // separate user
    let user_b = deps.user().await?;
    let client_b = deps.registry_service().client(&user_b.token).await;

    let result = client_b.get_http_api_deployment(&deployment.id.0).await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceGetHttpApiDeploymentError::Error404(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_delete_http_api_deployment_from_another_user(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_a = deps.user().await?;
    let (_, env_a) = user_a.app_and_env().await?;
    let domain = user_a.register_domain(&env_a.id).await?;

    let client_a = deps.registry_service().client(&user_a.token).await;

    let creation = HttpApiDeploymentCreation {
        domain,
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let deployment = client_a
        .create_http_api_deployment(&env_a.id.0, &creation)
        .await?;

    let user_b = deps.user().await?;
    let client_b = deps.registry_service().client(&user_b.token).await;

    let result = client_b
        .delete_http_api_deployment(&deployment.id.0, deployment.revision.into())
        .await;

    assert!(result.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_with_wrong_revision_is_rejected(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = HttpApiDeploymentCreation {
        domain,
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let deployment = client
        .create_http_api_deployment(&env.id.0, &creation)
        .await?;

    let wrong_revision = deployment.revision.next()?;

    let result = client
        .delete_http_api_deployment(&deployment.id.0, wrong_revision.into())
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceDeleteHttpApiDeploymentError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deleting_twice_returns_404(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = HttpApiDeploymentCreation {
        domain,
        agents: BTreeMap::from_iter([(
            AgentTypeName("test-api".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    let deployment = client
        .create_http_api_deployment(&env.id.0, &creation)
        .await?;

    client
        .delete_http_api_deployment(&deployment.id.0, deployment.revision.into())
        .await?;

    let result = client
        .delete_http_api_deployment(&deployment.id.0, deployment.revision.into())
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceDeleteHttpApiDeploymentError::Error404(_)
        ))
    ));

    Ok(())
}
