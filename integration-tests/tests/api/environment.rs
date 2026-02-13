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
    RegistryServiceClient, RegistryServiceCreateEnvironmentError,
    RegistryServiceGetApplicationEnvironmentError, RegistryServiceListApplicationEnvironmentsError,
    RegistryServiceUpdateEnvironmentError,
};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::environment::{EnvironmentCreation, EnvironmentUpdate};
use pretty_assertions::assert_eq;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use std::collections::HashSet;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_get_environments(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;
    let (app, env_1) = user.app_and_env().await?;
    let env_2 = user.env(&app.id).await?;

    {
        let result = client
            .get_application_environment(&app.id.0, &env_1.name.0)
            .await?;
        assert_eq!(result, env_1);
    }

    {
        let envs = client
            .list_application_environments(&app.id.0)
            .await?
            .values;

        assert_eq!(envs.len(), 2);

        let env_ids = envs.into_iter().map(|a| a.id).collect::<HashSet<_>>();

        assert_eq!(env_ids, HashSet::from_iter([env_1.id, env_2.id]));
    }

    client
        .delete_environment(&env_2.id.0, env_2.revision.into())
        .await?;

    {
        let result = client
            .get_application_environment(&app.id.0, &env_2.name.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetApplicationEnvironmentError::Error404(_)
            ))
        ));
    }

    {
        let envs = client
            .list_application_environments(&app.id.0)
            .await?
            .values;

        assert_eq!(envs.len(), 1);

        let env_ids = envs.into_iter().map(|a| a.id).collect::<Vec<_>>();

        assert_eq!(env_ids, vec![env_1.id]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn other_users_cannot_get_applications(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (app, env) = user_1.app_and_env().await?;
    let client = deps.registry_service().client(&user_2.token).await;

    {
        let result = client
            .get_application_environment(&app.id.0, &env.name.0)
            .await;
        assert!(matches!(result, Err(golem_client::Error::Item(RegistryServiceGetApplicationEnvironmentError::Error404(_)))));
    }

    {
        let result = client.list_application_environments(&app.id.0).await;
        assert!(matches!(result, Err(golem_client::Error::Item(RegistryServiceListApplicationEnvironmentsError::Error404(_)))));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deleting_account_hides_environments(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let user = deps.user().await?;

    let user_client = deps.registry_service().client(&user.token).await;
    let admin_client = deps.registry_service().client(&admin.token).await;

    let (app, env) = user.app_and_env().await?;

    let account = user_client.get_account(&user.account_id.0).await?;
    user_client
        .delete_account(&account.id.0, account.revision.into())
        .await?;

    {
        let result = admin_client.list_application_environments(&app.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceListApplicationEnvironmentsError::Error404(_)
            ))
        ));
    }

    {
        let result = admin_client
            .get_application_environment(&app.id.0, &env.name.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetApplicationEnvironmentError::Error404(_)
            ))
        ));
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn deleting_application_hides_environments(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;

    let client = deps.registry_service().client(&user.token).await;

    let (app, env) = user.app_and_env().await?;

    client
        .delete_application(&app.id.0, app.revision.into())
        .await?;

    {
        let result = client.list_application_environments(&app.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceListApplicationEnvironmentsError::Error404(_)
            ))
        ));
    }

    {
        let result = client
            .get_application_environment(&app.id.0, &env.name.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetApplicationEnvironmentError::Error404(_)
            ))
        ));
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_create_two_environments_with_same_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let (app, env_1) = user.app_and_env().await?;
    let env_2 = user.env(&app.id).await?;

    // try to create a second environment with the same
    {
        let result = client
            .create_environment(
                &app.id.0,
                &EnvironmentCreation {
                    name: env_1.name.clone(),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceCreateEnvironmentError::Error409(_)
            ))
        ));
    }

    // try to rename environment to conflicting name
    {
        let result = client
            .update_environment(
                &env_2.id.0,
                &EnvironmentUpdate {
                    current_revision: env_2.revision,
                    name: Some(env_1.name.clone()),
                    compatibility_check: None,
                    version_check: None,
                    security_overrides: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceUpdateEnvironmentError::Error409(_)
            ))
        ));
    }

    // delete the environment, now creating a new one will succeed
    client
        .delete_environment(&env_1.id.0, env_1.revision.into())
        .await?;

    // create environment with reused name
    {
        let env_3 = client
            .create_environment(
                &app.id.0,
                &EnvironmentCreation {
                    name: env_1.name.clone(),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                },
            )
            .await?;

        client
            .delete_environment(&env_3.id.0, env_3.revision.into())
            .await?;
    }

    // update environment to reused name
    client
        .update_environment(
            &env_2.id.0,
            &EnvironmentUpdate {
                current_revision: env_2.revision,
                name: Some(env_1.name.clone()),
                compatibility_check: None,
                version_check: None,
                security_overrides: None,
            },
        )
        .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_visible_environments_shows_owned_and_shared(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let grantee = deps.user().await?;
    let client_owner = deps.registry_service().client(&owner.token).await;
    let client_grantee = deps.registry_service().client(&grantee.token).await;

    // Create two applications + environments for owner
    let (app_1, env_1a) = owner.app_and_env().await?;
    let env_1b = owner.env(&app_1.id).await?;
    let (_, env_2a) = owner.app_and_env().await?;

    // Share one environment with grantee
    owner
        .share_environment(&env_1b.id, &grantee.account_id, &[EnvironmentRole::Admin])
        .await?;

    // Owner sees all environments
    let visible_owner = client_owner
        .list_visible_environments(None, None, None)
        .await?;
    let owner_env_ids: HashSet<_> = visible_owner
        .values
        .into_iter()
        .map(|e| e.environment.id)
        .collect();
    assert!(owner_env_ids.contains(&env_1a.id));
    assert!(owner_env_ids.contains(&env_1b.id));
    assert!(owner_env_ids.contains(&env_2a.id));

    // Grantee sees only shared environment
    let visible_grantee = client_grantee
        .list_visible_environments(None, None, None)
        .await?
        .values;
    let grantee_env_ids: HashSet<_> = visible_grantee
        .into_iter()
        .map(|e| e.environment.id)
        .collect();
    assert!(!grantee_env_ids.contains(&env_1a.id));
    assert!(grantee_env_ids.contains(&env_1b.id));
    assert!(!grantee_env_ids.contains(&env_2a.id));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_visible_environments_excludes_deleted_entities(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let (app, env) = user.app_and_env().await?;

    // Delete environment
    client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;

    // Deleted env should not appear
    let visible = client
        .list_visible_environments(None, None, None)
        .await?
        .values;
    assert!(!visible.iter().any(|e| e.environment.id == env.id));

    // Delete application
    client
        .delete_application(&app.id.0, app.revision.into())
        .await?;
    let visible_after_app_delete = client
        .list_visible_environments(None, None, None)
        .await?
        .values;
    assert!(visible_after_app_delete.is_empty());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_visible_environments_multiple_accounts_isolated(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;
    let client_2 = deps.registry_service().client(&user_2.token).await;

    let (_app1, env1) = user_1.app_and_env().await?;
    let (_app2, env2) = user_2.app_and_env().await?;

    // Each user sees only their own environments
    let visible_1 = client_1
        .list_visible_environments(None, None, None)
        .await?
        .values;
    assert!(visible_1.iter().any(|e| e.environment.id == env1.id));
    assert!(!visible_1.iter().any(|e| e.environment.id == env2.id));

    let visible_2 = client_2
        .list_visible_environments(None, None, None)
        .await?
        .values;
    assert!(visible_2.iter().any(|e| e.environment.id == env2.id));
    assert!(!visible_2.iter().any(|e| e.environment.id == env1.id));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deleted_account_hides_shared_environments_from_grantee(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let grantee = deps.user().await?;

    let owner_client = deps.registry_service().client(&owner.token).await;
    let grantee_client = deps.registry_service().client(&grantee.token).await;

    // Owner creates an application and an environment
    let (_, env) = owner.app_and_env().await?;

    // Owner shares the environment with the grantee
    owner
        .share_environment(&env.id, &grantee.account_id, &[EnvironmentRole::Admin])
        .await?;

    // Grantee can see the shared environment
    let visible_before_delete = grantee_client
        .list_visible_environments(None, None, None)
        .await?
        .values;
    assert!(
        visible_before_delete
            .iter()
            .any(|e| e.environment.id == env.id),
        "Shared environment should be visible before deletion"
    );

    // Owner deletes their account
    let owner_account_info = owner_client.get_account(&owner.account_id.0).await?;
    owner_client
        .delete_account(&owner_account_info.id.0, owner_account_info.revision.into())
        .await?;

    // Grantee should no longer see the environment
    let visible_after_delete = grantee_client
        .list_visible_environments(None, None, None)
        .await?
        .values;
    assert!(
        !visible_after_delete
            .iter()
            .any(|e| e.environment.id == env.id),
        "Environment from deleted account should no longer be visible to grantee"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_visible_environments_filters_by_account_email(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let client_1 = deps.registry_service().client(&user_1.token).await;
    let client_2 = deps.registry_service().client(&user_2.token).await;

    let (_app1, env1) = user_1.app_and_env().await?;
    let (_app2, env2) = user_2.app_and_env().await?;

    // Filter by user_1 email
    let filtered_1 = client_1
        .list_visible_environments(Some(&user_1.account_email.0), None, None)
        .await?
        .values;
    assert!(filtered_1
        .iter()
        .all(|e| e.account.email == user_1.account_email));
    assert!(filtered_1.iter().any(|e| e.environment.id == env1.id));
    assert!(filtered_1.iter().all(|e| e.environment.id != env2.id));

    // Filter by user_2 email
    let filtered_2 = client_2
        .list_visible_environments(Some(&user_2.account_email.0), None, None)
        .await?
        .values;
    assert!(filtered_2
        .iter()
        .all(|e| e.account.email == user_2.account_email));
    assert!(filtered_2.iter().any(|e| e.environment.id == env2.id));
    assert!(filtered_2.iter().all(|e| e.environment.id != env1.id));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_visible_environments_filters_by_app_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let (app_1, env_1a) = user.app_and_env().await?;
    let env_1b = user.env(&app_1.id).await?;
    let (_, env_2a) = user.app_and_env().await?;

    // Filter by app_1 name
    let filtered = client
        .list_visible_environments(None, Some(&app_1.name.0), None)
        .await?
        .values;
    let env_ids: HashSet<_> = filtered.iter().map(|e| e.environment.id).collect();
    assert!(env_ids.contains(&env_1a.id));
    assert!(env_ids.contains(&env_1b.id));
    assert!(!env_ids.contains(&env_2a.id));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_visible_environments_filters_by_environment_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let (_app, env_1) = user.app_and_env().await?;
    let env_2 = user.env(&_app.id).await?;

    // Filter by env_1 name
    let filtered = client
        .list_visible_environments(None, None, Some(&env_1.name.0))
        .await?
        .values;
    let env_ids: HashSet<_> = filtered.iter().map(|e| e.environment.id).collect();
    assert!(env_ids.contains(&env_1.id));
    assert!(!env_ids.contains(&env_2.id));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_visible_environments_combined_filters(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let (app_1, env_1) = user.app_and_env().await?;
    let (_, env_2) = user.app_and_env().await?;

    // Apply all filters together (account_email + app_name + environment_name)
    let filtered = client
        .list_visible_environments(
            Some(&user.account_email.0),
            Some(&app_1.name.0),
            Some(&env_1.name.0),
        )
        .await?
        .values;

    let env_ids: HashSet<_> = filtered.iter().map(|e| e.environment.id).collect();
    assert!(env_ids.contains(&env_1.id));
    assert!(!env_ids.contains(&env_2.id));

    Ok(())
}
