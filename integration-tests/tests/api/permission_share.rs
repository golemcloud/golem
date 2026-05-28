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
    RegistryServiceClient, RegistryServiceCreatePermissionShareError,
    RegistryServiceGetPermissionShareError,
};
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareName, PermissionShareUpdate,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

fn share_data(permission: &str) -> PermissionShareData {
    PermissionShareData {
        lower_positive: vec![permission.to_string()],
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
    }
}

#[test]
#[tracing::instrument]
async fn create_list_update_and_delete_permission_share(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let target = deps.user().await?;

    let client = deps.registry_service().client(&owner.token).await;

    let creation = PermissionShareCreation {
        target_account_id: target.account_id,
        name: PermissionShareName("team-access".to_string()),
        data: share_data(&format!(
            "application(owner) @ {} : view : shop",
            target.account_email.as_str()
        )),
    };

    let share = client
        .create_permission_share(&owner.account_id.0, &creation)
        .await?;

    assert_eq!(share.owner_account_id, owner.account_id);
    assert_eq!(share.target_account_id, target.account_id);
    assert_eq!(share.name, creation.name);
    assert!(share.current_card_id.is_some());
    assert_eq!(share.data, creation.data);

    {
        let fetched = client.get_permission_share(&share.id.0).await?;
        assert_eq!(fetched, share);
    }

    {
        let fetched = client
            .get_permission_share_by_name(&owner.account_id.0, &share.name.0)
            .await?;
        assert_eq!(fetched, share);
    }

    {
        let owned = client
            .list_owned_permission_shares(&owner.account_id.0)
            .await?;
        assert_eq!(owned.values, vec![share.clone()]);
    }

    {
        let target_client = deps.registry_service().client(&target.token).await;
        let received = target_client
            .list_received_permission_shares(&target.account_id.0)
            .await?;
        assert_eq!(received.values, vec![share.clone()]);
    }

    let update = PermissionShareUpdate {
        current_revision: share.revision,
        name: PermissionShareName("team-access-renamed".to_string()),
        data: share_data("application(owner) @ * : create : *"),
    };

    let updated = client.update_permission_share(&share.id.0, &update).await?;
    assert_eq!(updated.revision, share.revision.next()?);
    assert_eq!(updated.name, update.name);
    assert_eq!(updated.data, update.data);
    assert_eq!(updated.current_card_id, share.current_card_id);

    client
        .delete_permission_share(&updated.id.0, updated.revision.into())
        .await?;

    let deleted_get = client.get_permission_share(&updated.id.0).await;
    assert!(matches!(
        deleted_get,
        Err(golem_client::Error::Item(
            RegistryServiceGetPermissionShareError::Error404(_)
        ))
    ));

    let owned = client
        .list_owned_permission_shares(&owner.account_id.0)
        .await?;
    assert_eq!(owned.values, Vec::new());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn permission_share_names_are_unique_per_owner(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let target_1 = deps.user().await?;
    let target_2 = deps.user().await?;

    let client = deps.registry_service().client(&owner.token).await;

    let creation = PermissionShareCreation {
        target_account_id: target_1.account_id,
        name: PermissionShareName("shared-name".to_string()),
        data: share_data("application(owner) @ * : view : shop"),
    };

    client
        .create_permission_share(&owner.account_id.0, &creation)
        .await?;

    let duplicate = PermissionShareCreation {
        target_account_id: target_2.account_id,
        name: creation.name.clone(),
        data: share_data("application(owner) @ * : create : *"),
    };

    let result = client
        .create_permission_share(&owner.account_id.0, &duplicate)
        .await;
    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreatePermissionShareError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn permission_share_rejects_third_party_recipient(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let target = deps.user().await?;
    let third_party = deps.user().await?;

    let client = deps.registry_service().client(&owner.token).await;

    let creation = PermissionShareCreation {
        target_account_id: target.account_id,
        name: PermissionShareName("bad-recipient".to_string()),
        data: share_data(&format!(
            "application(owner) @ {} : view : shop",
            third_party.account_email.as_str()
        )),
    };

    let result = client
        .create_permission_share(&owner.account_id.0, &creation)
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreatePermissionShareError::Error400(_)
        ))
    ));

    Ok(())
}
