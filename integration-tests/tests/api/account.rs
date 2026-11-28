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
    RegistryServiceClient, RegistryServiceCreateAccountError,
    RegistryServiceCreatePermissionShareError, RegistryServiceDeleteAccountError,
    RegistryServiceGetAccountByEmailError, RegistryServicePlaceAccountDeletionHoldError,
    RegistryServiceRemoveAccountDeletionHoldError, RegistryServiceSetAccountPlanError,
};
use golem_client::model::AccountUpdate;
use golem_common::model::account::{
    AccountCreation, AccountDeletionHoldCreation, AccountEmail, AccountRevision, AccountSetPlan,
};
use golem_common::model::account_usage::StorageResourceLimitValue;
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareName,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use pretty_assertions::{assert_eq, assert_matches};
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

        let account_by_email = client
            .get_account_by_email(user.account_email.as_str())
            .await?;
        assert_eq!(account_by_email, account);

        let other_user = deps.user().await?;
        let other_client = deps.registry_service().client(&other_user.token).await;
        let result = other_client
            .get_account_by_email(user.account_email.as_str())
            .await;
        assert_matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetAccountByEmailError::Error404(_)
            ))
        );
    }

    // get account plan
    {
        let plan = client.get_account_plan(&user.account_id.0).await?;
        assert_matches!(
            plan.max_storage_per_agent,
            StorageResourceLimitValue::Disabled(_)
        );
        assert_matches!(
            plan.max_storage_per_agent_ceiling,
            StorageResourceLimitValue::Disabled(_)
        );
        assert!(!plan.max_storage_per_agent_user_configurable);
    }

    // get account tokens
    {
        let tokens = client.list_account_tokens(&user.account_id.0).await?;
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

    let updated_account = client
        .update_account(
            &user.account_id.0,
            &AccountUpdate {
                current_revision: AccountRevision::INITIAL,
                name: Some(new_name.clone()),
            },
        )
        .await?;

    assert_eq!(updated_account.id, user.account_id);
    assert_eq!(updated_account.name, new_name);
    assert_eq!(updated_account.email, user.account_email);
    assert_eq!(updated_account.revision, AccountRevision::new(1)?);

    {
        let account_from_get = client.get_account(&user.account_id.0).await?;
        assert_eq!(account_from_get, updated_account);
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

    let email = AccountEmail::new(format!("{}@golem.cloud", Uuid::new_v4()));

    {
        let account = client
            .create_account(&AccountCreation {
                name: Uuid::new_v4().to_string(),
                email: email.clone(),
                roles: Vec::new(),
            })
            .await?;

        assert_eq!(account.email, email);
    }

    {
        let failed_account_creation = client
            .create_account(&AccountCreation {
                name: Uuid::new_v4().to_string(),
                email: email.clone(),
                roles: Vec::new(),
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

fn account_share(
    account: &AccountEmail,
    recipient: &AccountEmail,
    verbs: &[&str],
) -> PermissionShareData {
    PermissionShareData {
        lower_positive: verbs
            .iter()
            .map(|verb| {
                format!(
                    "account({}) @ {} : {verb} :",
                    account.as_str(),
                    recipient.as_str()
                )
            })
            .collect(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
    }
}

#[test]
#[tracing::instrument]
async fn owner_cannot_change_the_plan_of_the_own_account(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let result = client
        .set_account_plan(
            &user.account_id.0,
            &AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: deps.registry_service().low_fuel_plan(),
            },
        )
        .await;
    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceSetAccountPlanError::Error403(_)
        ))
    );

    let account = client.get_account(&user.account_id.0).await?;
    assert_eq!(account.plan_id, deps.registry_service().default_plan());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn admin_can_change_the_plan_of_an_account(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let admin = deps.admin().await;
    let admin_client = deps.registry_service().client(&admin.token).await;

    let account = admin_client
        .set_account_plan(
            &user.account_id.0,
            &AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: deps.registry_service().low_fuel_plan(),
            },
        )
        .await?;
    assert_eq!(account.plan_id, deps.registry_service().low_fuel_plan());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn set_plan_permission_share_lets_the_recipient_change_a_plan(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let customer = deps.user().await?;
    let billing = deps.user().await?;
    let admin = deps.admin().await;
    let admin_client = deps.registry_service().client(&admin.token).await;
    let billing_client = deps.registry_service().client(&billing.token).await;

    let set_plan = AccountSetPlan {
        current_revision: AccountRevision::INITIAL,
        plan: deps.registry_service().low_fuel_plan(),
    };
    // Without the share the customer account is not visible to the billing account.
    let before_share = billing_client
        .set_account_plan(&customer.account_id.0, &set_plan)
        .await;
    assert_matches!(
        before_share,
        Err(golem_client::Error::Item(
            RegistryServiceSetAccountPlanError::Error404(_)
        ))
    );

    admin_client
        .create_permission_share(
            &admin.account_id.0,
            &PermissionShareCreation {
                target_account_email: billing.account_email.clone(),
                name: PermissionShareName(format!("set-plan-{}", customer.account_id.0)),
                data: account_share(
                    &customer.account_email,
                    &billing.account_email,
                    &["view", "set-plan"],
                ),
            },
        )
        .await?;

    let account = billing_client
        .set_account_plan(&customer.account_id.0, &set_plan)
        .await?;
    assert_eq!(account.plan_id, deps.registry_service().low_fuel_plan());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn owner_cannot_share_set_plan_on_the_own_account(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let other = deps.user().await?;
    let client = deps.registry_service().client(&owner.token).await;

    let result = client
        .create_permission_share(
            &owner.account_id.0,
            &PermissionShareCreation {
                target_account_email: other.account_email.clone(),
                name: PermissionShareName("set-plan".to_string()),
                data: account_share(&owner.account_email, &other.account_email, &["set-plan"]),
            },
        )
        .await;
    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreatePermissionShareError::Error403(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deletion_hold_refuses_account_delete_until_removed(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let owner = deps.user().await?;
    let holder = deps.user().await?;

    let admin_client = deps.registry_service().client(&admin.token).await;
    let owner_client = deps.registry_service().client(&owner.token).await;
    let holder_client = deps.registry_service().client(&holder.token).await;

    admin_client
        .create_permission_share(
            &admin.account_id.0,
            &PermissionShareCreation {
                target_account_email: holder.account_email.clone(),
                name: PermissionShareName(format!("deletion-hold-{}", Uuid::new_v4())),
                data: account_share(
                    &owner.account_email,
                    &holder.account_email,
                    &["view", "hold-deletion"],
                ),
            },
        )
        .await?;

    let reason = "The account has an active subscription.".to_string();
    let hold = holder_client
        .place_account_deletion_hold(
            &owner.account_id.0,
            &holder.account_id.0,
            &AccountDeletionHoldCreation {
                reason: reason.clone(),
            },
        )
        .await?;
    assert_eq!(hold.account_id, owner.account_id);
    assert_eq!(hold.holder_account_id, holder.account_id);
    assert_eq!(hold.reason, reason);

    {
        let holds = owner_client
            .list_account_deletion_holds(&owner.account_id.0)
            .await?;
        assert_eq!(holds.values, vec![hold.clone()]);
    }

    let account = owner_client.get_account(&owner.account_id.0).await?;
    {
        let result = owner_client
            .delete_account(&owner.account_id.0, account.revision.into())
            .await;
        let_assert_conflict_with_reason(result, &reason);
    }

    {
        let result = owner_client
            .remove_account_deletion_hold(&owner.account_id.0, &holder.account_id.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceRemoveAccountDeletionHoldError::Error403(_)
            ))
        ));
    }

    {
        let result = owner_client
            .place_account_deletion_hold(
                &owner.account_id.0,
                &holder.account_id.0,
                &AccountDeletionHoldCreation {
                    reason: "replaced by the owner".to_string(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServicePlaceAccountDeletionHoldError::Error403(_)
            ))
        ));
    }

    admin_client
        .remove_account_deletion_hold(&owner.account_id.0, &holder.account_id.0)
        .await?;
    holder_client
        .remove_account_deletion_hold(&owner.account_id.0, &holder.account_id.0)
        .await?;

    owner_client
        .delete_account(&owner.account_id.0, account.revision.into())
        .await?;

    {
        let result = holder_client
            .place_account_deletion_hold(
                &owner.account_id.0,
                &holder.account_id.0,
                &AccountDeletionHoldCreation { reason },
            )
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServicePlaceAccountDeletionHoldError::Error404(_)
            ))
        ));
    }

    Ok(())
}

fn let_assert_conflict_with_reason(
    result: Result<
        golem_client::model::Empty,
        golem_client::Error<RegistryServiceDeleteAccountError>,
    >,
    reason: &str,
) {
    match result {
        Err(golem_client::Error::Item(RegistryServiceDeleteAccountError::Error409(body))) => {
            assert_eq!(body.code, "ACCOUNT_DELETION_HELD");
            assert!(
                body.error.contains(reason),
                "the error must name the reason of the hold: {}",
                body.error
            );
        }
        other => panic!("expected 409 ACCOUNT_DELETION_HELD, got {other:?}"),
    }
}
