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
    RegistryServiceClient, RegistryServiceCreateApplicationError,
    RegistryServiceGetAccountApplicationError, RegistryServiceListAccountApplicationsError,
    RegistryServiceUpdateApplicationError,
};
use golem_common::model::application::{ApplicationCreation, ApplicationUpdate};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use test_r::{inherit_test_dep, test};

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
        assert_eq!(result, app_1);
    }

    {
        let apps = client
            .list_account_applications(&user.account_id.0)
            .await?
            .values;

        assert_eq!(apps.len(), 2);

        let app_ids = apps.into_iter().map(|a| a.id).collect::<HashSet<_>>();

        assert_eq!(app_ids, HashSet::from_iter([app_1.id, app_2.id]));
    }

    client
        .delete_application(&app_2.id.0, app_2.revision.into())
        .await?;

    {
        let result = client
            .get_account_application(&user.account_id.0, &app_2.name.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetAccountApplicationError::Error404(_)
            ))
        ));
    }

    {
        let apps = client
            .list_account_applications(&user.account_id.0)
            .await?
            .values;

        assert_eq!(apps.len(), 1);

        let app_ids = apps.into_iter().map(|a| a.id).collect::<Vec<_>>();

        assert_eq!(app_ids, vec![app_1.id]);
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
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetAccountApplicationError::Error404(_)
            ))
        ));
    }

    {
        let result = client.list_account_applications(&user_1.account_id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceListAccountApplicationsError::Error403(_)
            ))
        ));
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

    let account = user_client.get_account(&user.account_id.0).await?;
    user_client
        .delete_account(&account.id.0, account.revision.into())
        .await?;

    {
        let result = admin_client
            .list_account_applications(&user.account_id.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceListAccountApplicationsError::Error403(_)
            ))
        ));
    }

    {
        let result = admin_client
            .get_account_application(&user.account_id.0, &app.name.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetAccountApplicationError::Error404(_)
            ))
        ));
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
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceCreateApplicationError::Error409(_)
            ))
        ));
    }

    // try to rename environment to conflicting name
    {
        let result = client
            .update_application(
                &app_2.id.0,
                &ApplicationUpdate {
                    current_revision: app_2.revision,
                    name: Some(app_1.name.clone()),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceUpdateApplicationError::Error409(_)
            ))
        ));
    }

    // delete the environment, now creating a new one will succeed
    client
        .delete_application(&app_1.id.0, app_1.revision.into())
        .await?;

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

        client
            .delete_application(&app_3.id.0, app_3.revision.into())
            .await?;
    }

    // update environment to reused name
    client
        .update_application(
            &app_2.id.0,
            &ApplicationUpdate {
                current_revision: app_2.revision,
                name: Some(app_1.name.clone()),
            },
        )
        .await?;

    Ok(())
}
