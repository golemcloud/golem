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
use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateAccountError, RegistryServiceUpdateAccountError,
};
use golem_client::model::AccountRole;
use golem_client::model::UpdatedAccountData;
use golem_common::model::account::{AccountRevision, NewAccountData};
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

#[test]
#[tracing::instrument]
async fn create_account_with_duplicate_email_fails(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let client = deps.registry_service().client(&admin.token).await;

    let email = format!("{}@golem.cloud", Uuid::new_v4());

    {
        let account = client
            .create_account(&NewAccountData {
                name: Uuid::new_v4().to_string(),
                email: email.clone(),
            })
            .await?;

        assert!(account.email == email);
    }

    {
        let failed_account_creation = client
            .create_account(&NewAccountData {
                name: Uuid::new_v4().to_string(),
                email: email.clone(),
            })
            .await;

        let_assert!(
            Err(golem_client::Error::Item(
                RegistryServiceCreateAccountError::Error409(_)
            )) = failed_account_creation
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_account_with_duplicate_email_fails(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let client = deps.registry_service().client(&admin.token).await;

    let conflicting_email = format!("{}@golem.cloud", Uuid::new_v4());

    {
        let account = client
            .create_account(&NewAccountData {
                name: Uuid::new_v4().to_string(),
                email: conflicting_email.clone(),
            })
            .await?;

        assert!(account.email == conflicting_email);
    }

    {
        let account = client
            .create_account(&NewAccountData {
                name: Uuid::new_v4().to_string(),
                email: format!("{}@golem.cloud", Uuid::new_v4()),
            })
            .await?;

        let failed_account_update = client
            .update_account(
                &account.id.0,
                &UpdatedAccountData {
                    name: account.name,
                    email: conflicting_email.clone(),
                },
            )
            .await;

        let_assert!(
            Err(golem_client::Error::Item(
                RegistryServiceUpdateAccountError::Error409(_)
            )) = failed_account_update
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn emails_can_be_reused(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let client = deps.registry_service().client(&admin.token).await;

    let conflicting_email = format!("{}@golem.cloud", Uuid::new_v4());

    let account_1 = client
        .create_account(&NewAccountData {
            name: Uuid::new_v4().to_string(),
            email: conflicting_email.clone(),
        })
        .await?;

    let account_2 = client
        .create_account(&NewAccountData {
            name: Uuid::new_v4().to_string(),
            email: format!("{}@golem.cloud", Uuid::new_v4()),
        })
        .await?;

    let account_1 = client
        .update_account(
            &account_1.id.0,
            &UpdatedAccountData {
                name: account_1.name,
                email: format!("{}@golem.cloud", Uuid::new_v4()),
            },
        )
        .await?;

    let account_2 = client
        .update_account(
            &account_2.id.0,
            &UpdatedAccountData {
                name: account_2.name,
                email: conflicting_email.clone(),
            },
        )
        .await?;

    assert!(account_1.email != conflicting_email);
    assert!(account_2.email == conflicting_email);

    Ok(())
}
