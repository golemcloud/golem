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
use golem_client::api::RegistryServiceClient;
use golem_client::model::AccountRole;
use golem_client::model::UpdatedAccountData;
use golem_common::model::account::AccountRevision;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn get_account(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let account = client.get_account(&user.account_id.0).await?;

    assert!(account.id == user.account_id);
    assert!(account.email == user.account_email);
    assert!(account.revision == AccountRevision::INITIAL);
    assert!(account.roles == Vec::new());
    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_account(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let new_name = Uuid::new_v4().to_string();
    let new_email = format!("{new_name}@golem.cloud");

    let account = client
        .update_account(
            &user.account_id.0,
            &UpdatedAccountData {
                name: new_name.clone(),
                email: new_email.clone(),
            },
        )
        .await?;

    assert!(account.id == user.account_id);
    assert!(account.name == new_name);
    assert!(account.email == new_email);
    assert!(account.revision == AccountRevision(1));

    {
        let account_from_get = client.get_account(&user.account_id.0).await?;
        assert!(account_from_get == account);
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn set_roles(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    {
        let account = client
            .set_account_roles(
                &user.account_id.0,
                &[AccountRole::Admin, AccountRole::MarketingAdmin],
            )
            .await?;

        assert!(account.roles == vec![AccountRole::Admin, AccountRole::MarketingAdmin]);
        assert!(account.revision == AccountRevision(1));
    }

    {
        let account = client
            .set_account_roles(&user.account_id.0, &[AccountRole::Admin])
            .await?;

        assert!(account.roles == vec![AccountRole::Admin]);
        assert!(account.revision == AccountRevision(2));
    }
    Ok(())
}
