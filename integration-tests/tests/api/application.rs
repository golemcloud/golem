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
    RegistryServiceClient, RegistryServiceCreateApplicationError,
    RegistryServiceGetAccountApplicationError, RegistryServiceListAccountApplicationsError,
    RegistryServiceUpdateApplicationError,
};
use golem_common::model::application::{ApplicationCreation, ApplicationUpdate};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use std::collections::HashSet;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn can_get_and_list_applications(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let app_1 = user.app().await?;
    let app_2 = user.app().await?;
    let client = deps.registry_service().client(&user.token).await;

    {
        let result = client
            .get_account_application(&user.account_id.0, &app_1.name.0)
            .await?;
        assert!(result == app_1);
    }

    {
        let apps = client
            .list_account_applications(&user.account_id.0)
            .await?
            .values;

        assert!(apps.len() == 2);

        let app_ids = apps.into_iter().map(|a| a.id).collect::<HashSet<_>>();

        assert!(app_ids == HashSet::from_iter([app_1.id.clone(), app_2.id.clone()]));
    }

    client.delete_application(&app_2.id.0).await?;

    {
        let result = client
            .get_account_application(&user.account_id.0, &app_2.name.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetAccountApplicationError::Error404(_)
            )) = result
        );
    }

    {
        let apps = client
            .list_account_applications(&user.account_id.0)
            .await?
            .values;

        assert!(apps.len() == 1);

        let app_ids = apps.into_iter().map(|a| a.id).collect::<Vec<_>>();

        assert!(app_ids == vec![app_1.id.clone()]);
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
    let app = user_1.app().await?;
    let client = deps.registry_service().client(&user_2.token).await;

    {
        let result = client
            .get_account_application(&user_1.account_id.0, &app.name.0)
            .await;
        assert!(let Err(golem_client::Error::Item(RegistryServiceGetAccountApplicationError::Error404(_))) = result);
    }

    {
        let result = client.list_account_applications(&user_1.account_id.0).await;
        assert!(let Err(golem_client::Error::Item(RegistryServiceListAccountApplicationsError::Error403(_))) = result);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deleting_account_hides_applications(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let user = deps.user().await?;

    let user_client = deps.registry_service().client(&user.token).await;
    let admin_client = deps.registry_service().client(&admin.token).await;

    let app = user.app().await?;

    user_client.delete_account(&user.account_id.0).await?;

    {
        let result = admin_client
            .list_account_applications(&user.account_id.0)
            .await;
        assert!(let Err(golem_client::Error::Item(RegistryServiceListAccountApplicationsError::Error403(_))) = result);
    }

    {
        let result = admin_client
            .get_account_application(&user.account_id.0, &app.name.0)
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetAccountApplicationError::Error404(_)
            )) = result
        );
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_create_two_applications_with_same_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let app_1 = user.app().await?;
    let app_2 = user.app().await?;

    // try to create a second environment with the same
    {
        let result = client
            .create_application(
                &user.account_id.0,
                &ApplicationCreation {
                    name: app_1.name.clone(),
                },
            )
            .await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceCreateApplicationError::Error409(_)
            )) = result
        );
    }

    // try to rename environment to conflicting name
    {
        let result = client
            .update_application(
                &app_2.id.0,
                &ApplicationUpdate {
                    new_name: Some(app_1.name.clone()),
                },
            )
            .await;

        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceUpdateApplicationError::Error409(_)
            )) = result
        );
    }

    // delete the environment, now creating a new one will succeed
    client.delete_application(&app_1.id.0).await?;

    // create environment with reused name
    {
        let app_3 = client
            .create_application(
                &user.account_id.0,
                &ApplicationCreation {
                    name: app_1.name.clone(),
                },
            )
            .await?;

        client.delete_application(&app_3.id.0).await?;
    }

    // update environment to reused name
    client
        .update_application(
            &app_2.id.0,
            &ApplicationUpdate {
                new_name: Some(app_1.name.clone()),
            },
        )
        .await?;

    Ok(())
}
