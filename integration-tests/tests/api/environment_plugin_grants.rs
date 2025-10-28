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
    RegistryServiceClient, RegistryServiceCreateEnvironmentPluginGrantError,
    RegistryServiceDeleteEnvironmentPluginGrantError, RegistryServiceGetComponentError,
    RegistryServiceGetEnvironmentPluginGrantError, RegistryServiceGetPluginByIdError,
};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::base64::Base64;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn can_grant_plugin_to_shared_env(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let client_1 = user_1.registry_service_client().await;

    let user_2 = deps.user().await?;
    let client_2 = user_2.registry_service_client().await;

    let (_, plugin_env) = user_1.app_and_env().await?;
    let (_, shared_env) = user_2.app_and_env().await?;
    user_2
        .share_environment(
            &user_1.account_id,
            &shared_env.id,
            &[EnvironmentRole::Admin],
        )
        .await?;

    let plugin_component = user_1
        .component(&plugin_env.id, "oplog-processor")
        .store()
        .await?;

    let plugin = client_1
        .create_plugin(
            &user_1.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    // user not owning the plugin cannot share it
    {
        let result = client_2
            .create_environment_plugin_grant(
                &shared_env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id.clone(),
                },
            )
            .await;

        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceCreateEnvironmentPluginGrantError::Error400(_)
            )) = result
        );
    }

    let plugin_grant = client_1
        .create_environment_plugin_grant(
            &shared_env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id.clone(),
            },
        )
        .await?;

    // both users can see the plugin grant when listing
    {
        let environment_plugin_grants = client_1
            .list_environment_plugin_grants(&shared_env.id.0)
            .await?;
        assert!(environment_plugin_grants.values == vec![plugin_grant.clone()])
    }
    {
        let environment_plugin_grants = client_2
            .list_environment_plugin_grants(&shared_env.id.0)
            .await?;
        assert!(environment_plugin_grants.values == vec![plugin_grant.clone()])
    }

    // both users can see the plugin grant when getting by id
    {
        let fetched = client_1
            .get_environment_plugin_grant(&plugin_grant.id.0)
            .await?;
        assert!(fetched == plugin_grant)
    }
    {
        let fetched = client_2
            .get_environment_plugin_grant(&plugin_grant.id.0)
            .await?;
        assert!(fetched == plugin_grant)
    }

    client_1
        .delete_environment_plugin_grant(&plugin_grant.id.0)
        .await?;
    // second delete fails with 404
    {
        let result = client_1
            .delete_environment_plugin_grant(&plugin_grant.id.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceDeleteEnvironmentPluginGrantError::Error404(_)
            )) = result
        );
    }

    // both users do not see plugin grant anymore when listing
    {
        let environment_plugin_grants = client_1
            .list_environment_plugin_grants(&shared_env.id.0)
            .await?;
        assert!(environment_plugin_grants.values == vec![])
    }
    {
        let environment_plugin_grants = client_2
            .list_environment_plugin_grants(&shared_env.id.0)
            .await?;
        assert!(environment_plugin_grants.values == vec![])
    }

    // both users cannot get the plugin grant by id anymore
    {
        let result = client_1
            .get_environment_plugin_grant(&plugin_grant.id.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentPluginGrantError::Error404(_)
            )) = result
        );
    }
    {
        let result = client_2
            .get_environment_plugin_grant(&plugin_grant.id.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentPluginGrantError::Error404(_)
            )) = result
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fail_with_404_when_sharing_plugin_to_env_you_are_not_member_of(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let client_1 = user_1.registry_service_client().await;

    let user_2 = deps.user().await?;

    let (_, plugin_env) = user_1.app_and_env().await?;
    let (_, unrelated_env) = user_2.app_and_env().await?;

    let plugin_component = user_1
        .component(&plugin_env.id, "oplog-processor")
        .store()
        .await?;

    let plugin = client_1
        .create_plugin(
            &user_1.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    {
        let result = client_1
            .create_environment_plugin_grant(
                &unrelated_env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id.clone(),
                },
            )
            .await;

        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceCreateEnvironmentPluginGrantError::Error404(_)
            )) = result
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn member_of_env_cannot_see_plugin_or_plugin_component(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let client_1 = user_1.registry_service_client().await;

    let user_2 = deps.user().await?;
    let client_2 = user_2.registry_service_client().await;

    let (_, plugin_env) = user_1.app_and_env().await?;
    let (_, shared_env) = user_2.app_and_env().await?;
    user_2
        .share_environment(
            &user_1.account_id,
            &shared_env.id,
            &[EnvironmentRole::Admin],
        )
        .await?;

    let plugin_component = user_1
        .component(&plugin_env.id, "oplog-processor")
        .store()
        .await?;

    let plugin = client_1
        .create_plugin(
            &user_1.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id.clone(),
                    component_revision: plugin_component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let plugin_grant = client_1
        .create_environment_plugin_grant(
            &shared_env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id.clone(),
            },
        )
        .await?;

    // User 2 cannot directly see plugin in user 1's account
    {
        let result = client_2.get_plugin_by_id(&plugin_grant.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetPluginByIdError::Error404(_)
            )) = result
        );
    }

    // User 2 cannot directly see component that is part of the plugin
    {
        let result = client_2.get_component(&plugin_component.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetComponentError::Error404(_)
            )) = result
        );
    }

    Ok(())
}
