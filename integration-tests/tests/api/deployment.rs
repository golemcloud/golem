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

use assert2::{assert, let_assert};
use golem_client::api::{
    RegistryServiceClient, RegistryServiceDeployEnvironmentError,
    RegistryServiceRollbackEnvironmentError,
};
use golem_client::model::DeploymentCreation;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentName, ComponentUpdate};
use golem_common::model::deployment::{
    DeploymentPlan, DeploymentPlanComponentEntry, DeploymentPlanHttpApiDeploymentEntry,
    DeploymentRollback, DeploymentVersion,
};
use golem_common::model::diff::Hash;
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentCurrentDeploymentView;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::collections::BTreeMap;
use std::str::FromStr;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn deploy_environment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "shopping-cart").store().await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let deployment = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash:
                    "ac2f64cca38293baa0e971ce6dd1678e95c688557f42a6d6463bfe9983a1c5d2".parse()?,
                version: DeploymentVersion("0.0.1".to_string()),
            },
        )
        .await?;

    // plan hash and actual hash are the same
    assert!(deployment.deployment_hash == plan.deployment_hash);

    // Can get hash and current revision from environment
    {
        let fetched_environment = client.get_environment(&env.id.0).await?;
        let_assert!(Some(current_deployment) = fetched_environment.current_deployment);
        assert!(current_deployment.deployment_revision == deployment.revision);
        assert!(current_deployment.deployment_hash == deployment.deployment_hash);
    }

    // Summary of the deployed deployment is the same as the original plan
    {
        let fetched_deployment = client
            .get_deployment_summary(&env.id.0, deployment.revision.into())
            .await?;
        assert!(fetched_deployment.deployment_hash == plan.deployment_hash);
        assert!(fetched_deployment.components == plan.components);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fail_with_409_on_hash_mismatch(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "shopping-cart").store().await?;

    {
        let result = client
            .deploy_environment(
                &env.id.0,
                &DeploymentCreation {
                    current_revision: None,
                    expected_deployment_hash: Hash::empty(),
                    version: DeploymentVersion("0.0.1".to_string()),
                },
            )
            .await;

        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceDeployEnvironmentError::Error409(_)
            )) = result
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_component_version_from_previous_deployment(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "shopping-cart").store().await?;

    let deployment_1 = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash:
                    "ac2f64cca38293baa0e971ce6dd1678e95c688557f42a6d6463bfe9983a1c5d2".parse()?,
                version: DeploymentVersion("0.0.1".to_string()),
            },
        )
        .await?;

    let updated_component = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                new_file_options: BTreeMap::new(),
                removed_files: Vec::new(),
                dynamic_linking: None,
                env: Some(BTreeMap::from_iter(vec![(
                    "ENV_VAR".to_string(),
                    "ENV_VAR_VALUE".to_string(),
                )])),
                agent_types: None,
                plugin_updates: Vec::new(),
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    let deployment_2 = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: Some(deployment_1.current_revision),
                expected_deployment_hash:
                    "d6a1f6264c4eb8c61f9e5b9360e55d24d862b10ec0abcfb51998365561c94063".parse()?,
                version: DeploymentVersion("0.0.2".to_string()),
            },
        )
        .await?;

    {
        let fetched_component = client
            .get_deployment_component(
                &env.id.0,
                deployment_1.revision.into(),
                &component.component_name.0,
            )
            .await?;
        assert!(fetched_component == component);
    }

    {
        let fetched_component = client
            .get_deployment_component(
                &env.id.0,
                deployment_2.revision.into(),
                &component.component_name.0,
            )
            .await?;
        assert!(fetched_component == updated_component);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn full_deployment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    // needs to be static as it's used for hash calculation
    let domain = Domain("full_deployment_test.golem.cloud".to_string());

    client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    let component = user
        .component(&env.id, "golem_it_agent_http_routes_ts")
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

    let expected_hash =
        Hash::from_str("a578db91bcc1305e81cf305ba68009b06e556235a74c65cc7bbf3715c0cd5c76")?;

    let expected_plan = DeploymentPlan {
        current_revision: None,
        deployment_hash: expected_hash,
        components: vec![DeploymentPlanComponentEntry {
            id: component.id,
            revision: component.revision,
            name: ComponentName("golem-it:agent-http-routes-ts".to_string()),
            hash: Hash::from_str(
                "23d4dda294f6394b762d20542797d68f890243315b149cdff1cd34beffca0b71",
            )?,
        }],
        http_api_deployments: vec![DeploymentPlanHttpApiDeploymentEntry {
            id: http_api_deployment.id,
            revision: http_api_deployment.revision,
            domain: domain.clone(),
            hash: Hash::from_str(
                "e56e7b639dd6373669c2f5a8c3f51ba13a15b3aa08dd737067a547fd52144f09",
            )?,
        }],
    };

    {
        let plan = client.get_environment_deployment_plan(&env.id.0).await?;
        assert!(plan == expected_plan);
    }

    let deployment = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: expected_hash,
                version: DeploymentVersion("0.0.1".to_string()),
            },
        )
        .await?;
    assert!(deployment.deployment_hash == expected_hash);

    {
        let deployment = client
            .get_deployment_summary(&env.id.0, deployment.revision.into())
            .await?;

        assert!(deployment.deployment_hash == expected_hash);
        assert!(deployment.components == expected_plan.components);
        assert!(deployment.http_api_deployments == expected_plan.http_api_deployments);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rollback(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "shopping-cart").store().await?;

    let deployment_1 = user.deploy_environment(&env.id).await?;

    user.component(&env.id, "it_agent_counters_release")
        .store()
        .await?;

    let deployment_2 = user.deploy_environment(&env.id).await?;

    assert!(deployment_2.revision != deployment_1.revision);
    assert!(deployment_2.deployment_hash != deployment_1.deployment_hash);

    // noop rollback
    {
        let result = client
            .rollback_environment(
                &env.id.0,
                &DeploymentRollback {
                    current_revision: deployment_2.current_revision,
                    deployment_revision: deployment_2.revision,
                },
            )
            .await;

        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceRollbackEnvironmentError::Error409(_)
            )) = result
        );
    }

    {
        let env = client.get_environment(&env.id.0).await?;
        assert!(
            env.current_deployment
                == Some(EnvironmentCurrentDeploymentView {
                    revision: deployment_2.current_revision,
                    deployment_revision: deployment_2.revision,
                    deployment_version: deployment_2.version,
                    deployment_hash: deployment_2.deployment_hash
                })
        )
    };

    // actual rollback
    let rollback_result = client
        .rollback_environment(
            &env.id.0,
            &DeploymentRollback {
                current_revision: deployment_2.current_revision,
                deployment_revision: deployment_1.revision,
            },
        )
        .await?;

    let expected_revision = deployment_2.current_revision.next()?;

    assert!(rollback_result.current_revision == expected_revision);
    assert!(rollback_result.revision == deployment_1.revision);
    assert!(rollback_result.deployment_hash == deployment_1.deployment_hash);
    assert!(rollback_result.version == deployment_1.version);

    {
        let env = client.get_environment(&env.id.0).await?;
        assert!(
            env.current_deployment
                == Some(EnvironmentCurrentDeploymentView {
                    revision: expected_revision,
                    deployment_revision: deployment_1.revision,
                    deployment_version: deployment_1.version,
                    deployment_hash: deployment_1.deployment_hash
                })
        )
    };

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filter_deployments_by_version(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "shopping-cart").store().await?;

    let deployment_1 = user.deploy_environment(&env.id).await?;

    client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                new_file_options: BTreeMap::new(),
                removed_files: Vec::new(),
                dynamic_linking: None,
                env: Some(BTreeMap::from_iter(vec![(
                    "ENV_VAR".to_string(),
                    "ENV_VAR_VALUE".to_string(),
                )])),
                agent_types: None,
                plugin_updates: Vec::new(),
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    let deployment_2 = user.deploy_environment(&env.id).await?;

    {
        let deployments = client.list_deployments(&env.id.0, None).await?;
        assert!(
            deployments.values == vec![deployment_1.clone().into(), deployment_2.clone().into()]
        )
    }

    {
        let deployments = client
            .list_deployments(&env.id.0, Some(&deployment_2.version.0))
            .await?;
        assert!(deployments.values == vec![deployment_2.clone().into()])
    }

    Ok(())
}
