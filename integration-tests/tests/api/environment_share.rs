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
use golem_client::api::{RegistryServiceClient, RegistryServiceGetEnvironmentShareError};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::environment_share::{EnvironmentShareCreation, EnvironmentShareUpdate};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn share_environment_with_other_user(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;

    let share = client_1
        .create_environment_share(
            &env.id.0,
            &EnvironmentShareCreation {
                grantee_account_id: user_2.account_id.clone(),
                roles: vec![EnvironmentRole::Admin],
            },
        )
        .await?;

    assert!(share.grantee_account_id == user_2.account_id);
    assert!(share.roles == vec![EnvironmentRole::Admin]);

    {
        let fetched_share = client_1.get_environment_share(&share.id.0).await?;
        assert!(fetched_share == share);
    }

    {
        let all_environment_shares = client_1.get_environment_shares(&env.id.0).await?;
        assert!(all_environment_shares.values.contains(&share));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_environment_shares(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;

    let share = client_1
        .create_environment_share(
            &env.id.0,
            &EnvironmentShareCreation {
                grantee_account_id: user_2.account_id.clone(),
                roles: vec![EnvironmentRole::Admin],
            },
        )
        .await?;

    client_1.delete_environment_share(&share.id.0).await?;

    {
        let result = client_1.get_environment_share(&share.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentShareError::Error404(_)
            )) = result
        );
    }

    {
        let all_environment_shares = client_1.get_environment_shares(&env.id.0).await?;
        assert!(all_environment_shares.values == Vec::new());
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_environment_shares(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;

    let share = client_1
        .create_environment_share(
            &env.id.0,
            &EnvironmentShareCreation {
                grantee_account_id: user_2.account_id.clone(),
                roles: vec![EnvironmentRole::Admin],
            },
        )
        .await?;

    let updated_share = client_1
        .update_environment_share(
            &share.id.0,
            &EnvironmentShareUpdate {
                new_roles: vec![EnvironmentRole::Viewer],
            },
        )
        .await?;

    assert!(updated_share.roles == vec![EnvironmentRole::Viewer]);

    {
        let fetched_share = client_1.get_environment_share(&share.id.0).await?;
        assert!(fetched_share == updated_share);
    }

    Ok(())
}
