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
    RegistryServiceClient, RegistryServiceCreatePluginError, RegistryServiceGetPluginByIdError,
};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::base64::Base64;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn can_create_and_fetch_plugins(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "oplog-processor").store().await?;

    let plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    // check user can fetch plugin
    {
        let fetched_plugin = client.get_plugin_by_id(&plugin.id.0).await?;
        assert_eq!(fetched_plugin, plugin);
    }

    // check other user cannot fetch plugin
    {
        let user_2 = deps.user().await?;
        let client_2 = user_2.registry_service_client().await;
        let result = client_2.get_plugin_by_id(&plugin.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetPluginByIdError::Error404(_)
            ))
        ));
    }

    // delete plugin
    client.delete_plugin(&plugin.id.0).await?;

    // fetching plugin after deletion fails with 404
    {
        let result = client.get_plugin_by_id(&plugin.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetPluginByIdError::Error404(_)
            ))
        ));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn can_list_plugins(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "oplog-processor").store().await?;

    let plugin1 = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor-1".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let plugin2 = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor-2".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    {
        let all_plugins = client.get_account_plugins(&user.account_id.0).await?;
        let plugin_ids = all_plugins
            .values
            .into_iter()
            .map(|p| p.id)
            .collect::<HashSet<_>>();
        assert_eq!(plugin_ids, HashSet::from_iter(vec![plugin1.id, plugin2.id]));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fails_with_bad_request_if_user_creates_oplog_processor_from_invalid_component(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let result = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreatePluginError::Error400(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fails_with_conflict_when_creating_two_plugins_with_same_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user.component(&env.id, "oplog-processor").store().await?;

    client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let result = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreatePluginError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fails_with_bad_request_when_creating_plugin_if_component_user_does_not_have_access_to(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;

    let (_, env) = user_1.app_and_env().await?;
    let component = user_1.component(&env.id, "oplog-processor").store().await?;

    let client = deps.registry_service().client(&user_2.token).await;

    let result = client
        .create_plugin(
            &user_2.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreatePluginError::Error400(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn should_allow_creating_plugin_with_component_in_share_environment(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;

    let (_, env) = user_1.app_and_env().await?;
    let component = user_1.component(&env.id, "oplog-processor").store().await?;
    user_1
        .share_environment(&env.id, &user_2.account_id, &[EnvironmentRole::Viewer])
        .await?;

    let client = deps.registry_service().client(&user_2.token).await;

    client
        .create_plugin(
            &user_2.account_id.0,
            &PluginRegistrationCreation {
                name: "test-oplog-processor".to_string(),
                version: "1.0.0".to_string(),
                description: "description".to_string(),
                icon: Base64(Vec::new()),
                homepage: "https://golem.cloud".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    Ok(())
}
