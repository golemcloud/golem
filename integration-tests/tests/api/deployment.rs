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
use assert2::{assert, let_assert};
use golem_client::api::{RegistryServiceClient, RegistryServiceDeployEnvironmentError};
use golem_client::model::DeploymentCreation;
use golem_common::model::component::ComponentUpdate;
use golem_common::model::diff::Hash;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::collections::BTreeMap;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
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
                current_deployment_revision: None,
                expected_deployment_hash:
                    "d14c6cc958f2dbb6c77400744c809deb08caceb2f9d1e8e39265387a4dbfe8de".parse()?,
                version: "0.0.1".to_string(),
            },
        )
        .await?;

    // plan hash and actual hash are the same
    assert!(deployment.deployment_hash == plan.deployment_hash);

    // Can get hash and current revision from environment
    {
        let fetched_environment = client.get_environment(&env.id.0).await?;
        let_assert!(Some(current_deployment) = fetched_environment.current_deployment);
        assert!(current_deployment.revision == deployment.revision);
        assert!(current_deployment.hash == deployment.deployment_hash);
    }

    // Plan of the deployed deployment is the same as the original plan
    {
        let fetched_deployment = client
            .get_environment_deployed_deployment_plan(&env.id.0, deployment.revision.0)
            .await?;
        assert!(fetched_deployment.deployment_hash == plan.deployment_hash);
        assert!(fetched_deployment.components == plan.components);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fail_with_400_on_hash_mismatch(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "shopping-cart").store().await?;

    {
        let result = client
            .deploy_environment(
                &env.id.0,
                &DeploymentCreation {
                    current_deployment_revision: None,
                    expected_deployment_hash: Hash::empty(),
                    version: "0.0.1".to_string(),
                },
            )
            .await;

        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceDeployEnvironmentError::Error400(_)
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
                current_deployment_revision: None,
                expected_deployment_hash:
                    "d14c6cc958f2dbb6c77400744c809deb08caceb2f9d1e8e39265387a4dbfe8de".parse()?,
                version: "0.0.1".to_string(),
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
                current_deployment_revision: Some(deployment_1.revision),
                expected_deployment_hash:
                    "1513f16898ef099114f5148e388f47a254d91fc788eac2d234dea0ad0ed7425d".parse()?,
                version: "0.0.2".to_string(),
            },
        )
        .await?;

    {
        let fetched_component = client
            .get_deployment_component(
                &env.id.0,
                deployment_1.revision.0,
                &component.component_name.0,
            )
            .await?;
        assert!(fetched_component == component);
    }

    {
        let fetched_component = client
            .get_deployment_component(
                &env.id.0,
                deployment_2.revision.0,
                &component.component_name.0,
            )
            .await?;
        assert!(fetched_component == updated_component);
    }

    Ok(())
}
