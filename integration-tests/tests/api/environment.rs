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
    RegistryServiceClient, RegistryServiceCreateEnvironmentError,
    RegistryServiceGetApplicationEnvironmentError, RegistryServiceListApplicationEnvironmentsError,
    RegistryServiceUpdateEnvironmentError,
};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentUpdate};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use std::collections::HashSet;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
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
        assert!(result == env_1);
    }

    {
        let envs = client
            .list_application_environments(&app.id.0)
            .await?
            .values;

        assert!(envs.len() == 2);

        let env_ids = envs.into_iter().map(|a| a.id).collect::<HashSet<_>>();

        assert!(env_ids == HashSet::from_iter([env_1.id.clone(), env_2.id.clone()]));
    }

    client
        .delete_environment(&env_2.id.0, env_2.revision.0)
        .await?;

    {
        let result = client
            .get_application_environment(&app.id.0, &env_2.name.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetApplicationEnvironmentError::Error404(_)
            )) = result
        );
    }

    {
        let envs = client
            .list_application_environments(&app.id.0)
            .await?
            .values;

        assert!(envs.len() == 1);

        let env_ids = envs.into_iter().map(|a| a.id).collect::<Vec<_>>();

        assert!(env_ids == vec![env_1.id.clone()]);
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
        assert!(let Err(golem_client::Error::Item(RegistryServiceGetApplicationEnvironmentError::Error404(_))) = result);
    }

    {
        let result = client.list_application_environments(&app.id.0).await;
        assert!(let Err(golem_client::Error::Item(RegistryServiceListApplicationEnvironmentsError::Error404(_))) = result);
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

    user_client.delete_account(&user.account_id.0).await?;

    {
        let result = admin_client.list_application_environments(&app.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceListApplicationEnvironmentsError::Error404(_)
            )) = result
        );
    }

    {
        let result = admin_client
            .get_application_environment(&app.id.0, &env.name.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetApplicationEnvironmentError::Error404(_)
            )) = result
        );
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

    client.delete_application(&app.id.0, app.revision.0).await?;

    {
        let result = client.list_application_environments(&app.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceListApplicationEnvironmentsError::Error404(_)
            )) = result
        );
    }

    {
        let result = client
            .get_application_environment(&app.id.0, &env.name.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetApplicationEnvironmentError::Error404(_)
            )) = result
        );
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
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceCreateEnvironmentError::Error409(_)
            )) = result
        );
    }

    // try to rename environment to conflicting name
    {
        let result = client
            .update_environment(
                &env_2.id.0,
                &EnvironmentUpdate {
                    current_revision: env_2.revision,
                    new_name: Some(env_1.name.clone()),
                },
            )
            .await;

        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceUpdateEnvironmentError::Error409(_)
            )) = result
        );
    }

    // delete the environment, now creating a new one will succeed
    client
        .delete_environment(&env_1.id.0, env_1.revision.0)
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
            .delete_environment(&env_3.id.0, env_3.revision.0)
            .await?;
    }

    // update environment to reused name
    client
        .update_environment(
            &env_2.id.0,
            &EnvironmentUpdate {
                current_revision: env_2.revision,
                new_name: Some(env_1.name.clone()),
            },
        )
        .await?;

    Ok(())
}
