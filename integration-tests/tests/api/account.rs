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
    RegistryServiceClient, RegistryServiceCreateAccountError, RegistryServiceUpdateAccountError,
};
use pretty_assertions::{assert_eq, assert_ne};
use golem_client::model::AccountUpdate;
use golem_common::model::account::{
    AccountCreation, AccountEmail, AccountRevision, AccountSetRoles,
};
use golem_common::model::auth::AccountRole;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn get_account(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    {
        let account = client.get_account(&user.account_id.0).await?;

        assert_eq!(account.id, user.account_id);
        assert_eq!(account.email, user.account_email);
        assert_eq!(account.revision, AccountRevision::INITIAL);
        assert_eq!(account.roles, Vec::new());
    }

    // get account plan
    {
        let result = client.get_account_plan(&user.account_id.0).await;
        assert!(result.is_ok())
    }

    // get account tokens
    {
        let tokens = client.get_account_tokens(&user.account_id.0).await?;
        assert_eq!(tokens.values.len(), 1)
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_account(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let new_name = Uuid::new_v4().to_string();
    let new_email = AccountEmail(format!("{new_name}@golem.cloud"));

    let updated_account = client
        .update_account(
            &user.account_id.0,
            &AccountUpdate {
                current_revision: AccountRevision::INITIAL,
                name: Some(new_name.clone()),
                email: Some(new_email.clone()),
            },
        )
        .await?;

    assert_eq!(updated_account.id, user.account_id);
    assert_eq!(updated_account.name, new_name);
    assert_eq!(updated_account.email, new_email);
    assert_eq!(updated_account.revision, AccountRevision::new(1)?);

    {
        let account_from_get = client.get_account(&user.account_id.0).await?;
        assert_eq!(account_from_get, updated_account);
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn set_roles(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let admin_client = deps.admin().await.registry_service_client().await;

    {
        let account = admin_client
            .set_account_roles(
                &user.account_id.0,
                &AccountSetRoles {
                    current_revision: AccountRevision::INITIAL,
                    roles: vec![AccountRole::MarketingAdmin, AccountRole::Admin],
                },
            )
            .await?;

        // We always reorder the roles so they are consistent
        assert_eq!(account.roles, vec![AccountRole::Admin, AccountRole::MarketingAdmin]);
        assert_eq!(account.revision, AccountRevision::new(1)?);
    }

    {
        let account = admin_client
            .set_account_roles(
                &user.account_id.0,
                &AccountSetRoles {
                    current_revision: AccountRevision::new(1)?,
                    roles: vec![AccountRole::Admin],
                },
            )
            .await?;

        assert_eq!(account.roles, vec![AccountRole::Admin]);
        assert_eq!(account.revision, AccountRevision::new(2)?);
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

    let email = AccountEmail(format!("{}@golem.cloud", Uuid::new_v4()));

    {
        let account = client
            .create_account(&AccountCreation {
                name: Uuid::new_v4().to_string(),
                email: email.clone(),
            })
            .await?;

        assert_eq!(account.email, email);
    }

    {
        let failed_account_creation = client
            .create_account(&AccountCreation {
                name: Uuid::new_v4().to_string(),
                email: email.clone(),
            })
            .await;

        assert!(matches!(
            failed_account_creation,
            Err(golem_client::Error::Item(
                RegistryServiceCreateAccountError::Error409(_)
            ))
        ));
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

    let conflicting_email = AccountEmail(format!("{}@golem.cloud", Uuid::new_v4()));

    {
        let account = client
            .create_account(&AccountCreation {
                name: Uuid::new_v4().to_string(),
                email: conflicting_email.clone(),
            })
            .await?;

        assert_eq!(account.email, conflicting_email);
    }

    {
        let account = client
            .create_account(&AccountCreation {
                name: Uuid::new_v4().to_string(),
                email: AccountEmail(format!("{}@golem.cloud", Uuid::new_v4())),
            })
            .await?;

        let failed_account_update = client
            .update_account(
                &account.id.0,
                &AccountUpdate {
                    current_revision: account.revision,
                    name: None,
                    email: Some(conflicting_email.clone()),
                },
            )
            .await;

        assert!(matches!(
            failed_account_update,
            Err(golem_client::Error::Item(
                RegistryServiceUpdateAccountError::Error409(_)
            ))
        ));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn emails_can_be_reused(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let client = deps.registry_service().client(&admin.token).await;

    let conflicting_email = AccountEmail(format!("{}@golem.cloud", Uuid::new_v4()));

    let account_1 = client
        .create_account(&AccountCreation {
            name: Uuid::new_v4().to_string(),
            email: conflicting_email.clone(),
        })
        .await?;

    let account_2 = client
        .create_account(&AccountCreation {
            name: Uuid::new_v4().to_string(),
            email: AccountEmail(format!("{}@golem.cloud", Uuid::new_v4())),
        })
        .await?;

    let account_1 = client
        .update_account(
            &account_1.id.0,
            &AccountUpdate {
                current_revision: account_1.revision,
                name: None,
                email: Some(AccountEmail(format!("{}@golem.cloud", Uuid::new_v4()))),
            },
        )
        .await?;

    let account_2 = client
        .update_account(
            &account_2.id.0,
            &AccountUpdate {
                current_revision: account_2.revision,
                name: None,
                email: Some(conflicting_email.clone()),
            },
        )
        .await?;

    assert_ne!(account_1.email, conflicting_email);
    assert_eq!(account_2.email, conflicting_email);

    Ok(())
}
