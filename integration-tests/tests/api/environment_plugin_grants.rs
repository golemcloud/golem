// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
    RegistryServiceClient, RegistryServiceCreateEnvironmentPluginGrantError,
    RegistryServiceDeleteEnvironmentPluginGrantError, RegistryServiceGetComponentError,
    RegistryServiceGetEnvironmentPluginGrantError, RegistryServiceGetPluginByIdError,
    RegistryServiceListEnvironmentEnvironmentPluginGrantsError,
};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::base64::Base64;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::permission_share::{
    PermissionShare, PermissionShareCreation, PermissionShareData, PermissionShareName,
};
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

async fn create_permission_share(
    client: &impl RegistryServiceClient,
    owner_account_id: AccountId,
    target_account_email: AccountEmail,
    name: &str,
    lower_positive: Vec<String>,
) -> anyhow::Result<PermissionShare> {
    Ok(client
        .create_permission_share(
            &owner_account_id.0,
            &PermissionShareCreation {
                target_account_email,
                name: PermissionShareName(name.to_string()),
                data: PermissionShareData {
                    lower_positive,
                    lower_negative: Vec::new(),
                    upper_positive: Vec::new(),
                    upper_negative: Vec::new(),
                },
            },
        )
        .await?)
}

fn environment_view_grant(owner: &str, app_name: &str, env_name: &str, recipient: &str) -> String {
    format!("environment({owner}/{app_name}) @ {recipient} : view : {env_name}")
}

fn environment_plugin_grant_grant(
    owner: &str,
    app_name: &str,
    env_name: &str,
    recipient: &str,
    verb: &str,
) -> String {
    format!("environment.plugin-grant({owner}/{app_name}/{env_name}) @ {recipient} : {verb} : *")
}

#[test]
#[tracing::instrument]
async fn can_grant_plugin_to_shared_env(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let client_1 = user_1.registry_service_client().await;

    let user_2 = deps.user().await?;
    let client_2 = user_2.registry_service_client().await;

    let (_, plugin_env) = user_1.app_and_env().await?;
    let (_, shared_env) = user_2.app_and_env().await?;
    create_permission_share(
        &client_2,
        user_2.account_id,
        user_1.account_email.clone(),
        "grant-plugin-to-shared-env",
        vec![
            environment_view_grant(
                user_2.account_email.as_str(),
                &shared_env.application_name.0,
                &shared_env.name.0,
                user_1.account_email.as_str(),
            ),
            environment_plugin_grant_grant(
                user_2.account_email.as_str(),
                &shared_env.application_name.0,
                &shared_env.name.0,
                user_1.account_email.as_str(),
                "create",
            ),
            environment_plugin_grant_grant(
                user_2.account_email.as_str(),
                &shared_env.application_name.0,
                &shared_env.name.0,
                user_1.account_email.as_str(),
                "view",
            ),
            environment_plugin_grant_grant(
                user_2.account_email.as_str(),
                &shared_env.application_name.0,
                &shared_env.name.0,
                user_1.account_email.as_str(),
                "delete",
            ),
        ],
    )
    .await?;

    let plugin_component = user_1
        .component(&plugin_env.id, "oplog_processor_release")
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
        )
        .await?;

    // user not owning the plugin cannot share it
    {
        let result = client_2
            .create_environment_plugin_grant(
                &shared_env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceCreateEnvironmentPluginGrantError::Error400(_)
            ))
        ));
    }

    let plugin_grant = client_1
        .create_environment_plugin_grant(
            &shared_env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await?;

    // both users can see the plugin grant when listing
    for client in [&client_1, &client_2] {
        let grants = client
            .list_environment_environment_plugin_grants(&shared_env.id.0)
            .await?
            .values
            .into_iter()
            .filter(|v| {
                v.plugin_account.id != deps.registry_service().builtin_plugin_owner_account_id()
            })
            .collect::<Vec<_>>();

        assert_eq!(grants.len(), 1);
        let grant = &grants[0];

        assert_eq!(grant.id, plugin_grant.id);
        assert_eq!(grant.environment_id, shared_env.id);
        assert_eq!(grant.plugin.id, plugin.id);
        assert_eq!(grant.plugin_account.id, user_1.account_id);
    }

    // both users can see the plugin grant when getting by id
    for client in [&client_1, &client_2] {
        let fetched = client
            .get_environment_plugin_grant(&plugin_grant.id.0)
            .await?;

        assert_eq!(fetched.id, plugin_grant.id);
        assert_eq!(fetched.environment_id, shared_env.id);
        assert_eq!(fetched.plugin.id, plugin.id);
        assert_eq!(fetched.plugin_account.id, user_1.account_id);
    }

    client_1
        .delete_environment_plugin_grant(&plugin_grant.id.0)
        .await?;

    // second delete fails with 404
    {
        let result = client_1
            .delete_environment_plugin_grant(&plugin_grant.id.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceDeleteEnvironmentPluginGrantError::Error404(_)
            ))
        ));
    }

    // both users do not see plugin grant anymore when listing
    for client in [&client_1, &client_2] {
        let grants = client
            .list_environment_environment_plugin_grants(&shared_env.id.0)
            .await?
            .values
            .into_iter()
            .filter(|v| {
                v.plugin_account.id != deps.registry_service().builtin_plugin_owner_account_id()
            })
            .collect::<Vec<_>>();

        assert!(grants.is_empty());
    }

    // both users cannot get the plugin grant by id anymore
    for client in [&client_1, &client_2] {
        let result = client
            .get_environment_plugin_grant(&plugin_grant.id.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentPluginGrantError::Error404(_)
            ))
        ));
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
        .component(&plugin_env.id, "oplog_processor_release")
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
        )
        .await?;

    {
        let result = client_1
            .create_environment_plugin_grant(
                &unrelated_env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceCreateEnvironmentPluginGrantError::Error404(_)
            ))
        ));
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
    create_permission_share(
        &client_2,
        user_2.account_id,
        user_1.account_email.clone(),
        "plugin-member-view-env",
        vec![
            environment_view_grant(
                user_2.account_email.as_str(),
                &shared_env.application_name.0,
                &shared_env.name.0,
                user_1.account_email.as_str(),
            ),
            environment_plugin_grant_grant(
                user_2.account_email.as_str(),
                &shared_env.application_name.0,
                &shared_env.name.0,
                user_1.account_email.as_str(),
                "create",
            ),
        ],
    )
    .await?;

    let plugin_component = user_1
        .component(&plugin_env.id, "oplog_processor_release")
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
        )
        .await?;

    let plugin_grant = client_1
        .create_environment_plugin_grant(
            &shared_env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await?;

    // User 2 cannot directly see plugin
    {
        let result = client_2.get_plugin_by_id(&plugin.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetPluginByIdError::Error404(_)
            ))
        ));
    }

    // But can see it via the grant
    {
        let fetched = client_2
            .get_environment_plugin_grant(&plugin_grant.id.0)
            .await?;

        assert_eq!(fetched.plugin.id, plugin.id);
        assert_eq!(fetched.plugin_account.id, user_1.account_id);
    }

    // User 2 cannot see the underlying component
    {
        let result = client_2.get_component(&plugin_component.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetComponentError::Error404(_)
            ))
        ));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_grant_deleted_plugin(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;

    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;

    let plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin".into(),
                version: "1.0.0".into(),
                description: "desc".into(),
                icon: Base64(vec![]),
                homepage: "https://golem.cloud".into(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
        )
        .await?;

    client.delete_plugin(&plugin.id.0).await?;

    let result = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateEnvironmentPluginGrantError::Error400(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn environment_owner_cannot_grant_foreign_plugin(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_env_owner = deps.user().await?;
    let client_env_owner = user_env_owner.registry_service_client().await;

    let user_plugin_owner = deps.user().await?;
    let client_plugin_owner = user_plugin_owner.registry_service_client().await;

    let (_, env) = user_env_owner.app_and_env().await?;
    let (_, plugin_env) = user_plugin_owner.app_and_env().await?;

    let component = user_plugin_owner
        .component(&plugin_env.id, "oplog_processor_release")
        .store()
        .await?;

    let plugin = client_plugin_owner
        .create_plugin(
            &user_plugin_owner.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin".into(),
                version: "1.0.0".into(),
                description: "desc".into(),
                icon: Base64(vec![]),
                homepage: "https://golem.cloud".into(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
        )
        .await?;

    let result = client_env_owner
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateEnvironmentPluginGrantError::Error400(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn shared_user_with_readonly_role_cannot_grant_plugin(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_owner = deps.user().await?;
    let client_owner = user_owner.registry_service_client().await;

    let user_shared = deps.user().await?;

    let (_, plugin_env) = user_owner.app_and_env().await?;
    let (_, shared_env) = user_shared.app_and_env().await?;

    create_permission_share(
        &user_shared.registry_service_client().await,
        user_shared.account_id,
        user_owner.account_email.clone(),
        "readonly-plugin-grant-access",
        vec![environment_view_grant(
            user_shared.account_email.as_str(),
            &shared_env.application_name.0,
            &shared_env.name.0,
            user_owner.account_email.as_str(),
        )],
    )
    .await?;

    let component = user_owner
        .component(&plugin_env.id, "oplog_processor_release")
        .store()
        .await?;
    let plugin = client_owner
        .create_plugin(
            &user_owner.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin".into(),
                version: "1.0.0".into(),
                description: "desc".into(),
                icon: Base64(vec![]),
                homepage: "https://golem.cloud".into(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
        )
        .await?;

    let result = client_owner
        .create_environment_plugin_grant(
            &shared_env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateEnvironmentPluginGrantError::Error403(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn shared_user_cannot_list_grants_after_share_revoked(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let shared = deps.user().await?;

    let client_owner = owner.registry_service_client().await;
    let client_shared = shared.registry_service_client().await;

    let (_, env) = owner.app_and_env().await?;
    let (_, plugin_env) = owner.app_and_env().await?;

    let permission_share = create_permission_share(
        &client_owner,
        owner.account_id,
        shared.account_email.clone(),
        "list-grants-revoked-access",
        vec![
            environment_view_grant(
                owner.account_email.as_str(),
                &env.application_name.0,
                &env.name.0,
                shared.account_email.as_str(),
            ),
            environment_plugin_grant_grant(
                owner.account_email.as_str(),
                &env.application_name.0,
                &env.name.0,
                shared.account_email.as_str(),
                "view",
            ),
        ],
    )
    .await?;

    let comp = owner
        .component(&plugin_env.id, "oplog_processor_release")
        .store()
        .await?;
    let plugin = client_owner
        .create_plugin(
            &owner.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin".into(),
                version: "1.0.0".into(),
                description: "desc".into(),
                icon: Base64(vec![]),
                homepage: "https://golem.cloud".into(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: comp.id,
                    component_revision: comp.revision,
                }),
            },
        )
        .await?;

    let grant = client_owner
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await?;

    client_owner
        .delete_permission_share(&permission_share.id.0, permission_share.revision.into())
        .await?;

    let result_shared = client_shared
        .list_environment_environment_plugin_grants(&env.id.0)
        .await;
    assert!(matches!(
        result_shared,
        Err(golem_client::Error::Item(
            RegistryServiceListEnvironmentEnvironmentPluginGrantsError::Error404(_)
        ))
    ));

    // Environment owner can still list plugin grants
    let result_owner = client_owner
        .list_environment_environment_plugin_grants(&env.id.0)
        .await?;
    assert!(
        result_owner
            .values
            .iter()
            .map(|epg| epg.id)
            .collect::<Vec<_>>()
            .contains(&grant.id)
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn revoked_user_cannot_fetch_grant(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let revoked_user = deps.user().await?;

    let client_owner = owner.registry_service_client().await;
    let client_revoked = revoked_user.registry_service_client().await;

    let (_, env) = owner.app_and_env().await?;
    let permission_share = create_permission_share(
        &client_owner,
        owner.account_id,
        revoked_user.account_email.clone(),
        "revoked-fetch-grant-access",
        vec![
            environment_view_grant(
                owner.account_email.as_str(),
                &env.application_name.0,
                &env.name.0,
                revoked_user.account_email.as_str(),
            ),
            environment_plugin_grant_grant(
                owner.account_email.as_str(),
                &env.application_name.0,
                &env.name.0,
                revoked_user.account_email.as_str(),
                "view",
            ),
        ],
    )
    .await?;

    let component = owner
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let plugin = client_owner
        .create_plugin(
            &owner.account_id.0,
            &PluginRegistrationCreation {
                name: "plugin".into(),
                version: "1.0.0".into(),
                description: "desc".into(),
                icon: Base64(vec![]),
                homepage: "https://golem.cloud".into(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: component.id,
                    component_revision: component.revision,
                }),
            },
        )
        .await?;

    let grant = client_owner
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin.id,
            },
        )
        .await?;

    client_owner
        .delete_permission_share(&permission_share.id.0, permission_share.revision.into())
        .await?;

    let result = client_revoked
        .get_environment_plugin_grant(&grant.id.0)
        .await;
    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceGetEnvironmentPluginGrantError::Error404(_)
        ))
    ));

    Ok(())
}
