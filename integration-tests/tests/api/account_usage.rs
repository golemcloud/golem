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

use chrono::{Duration, Utc};
use golem_client::api::{
    RegistryServiceClearAccountStorageOverrideError, RegistryServiceClient,
    RegistryServiceGetAccountStorageOverrideError, RegistryServiceSetAccountStorageOverrideError,
};
use golem_common::model::account::{AccountId, AccountRevision, AccountSetPlan};
use golem_common::model::account_usage::{
    BYTE_SECONDS_PER_GB_MONTH, SetStorageLimit, StorageLimit,
};
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService as _, ResourceUsageUpdate,
};
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn account_storage_usage_accumulates_grpc_updates(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let registry_service = deps.registry_service();
    let registry_client = GrpcRegistryService::new(&GrpcRegistryServiceConfig {
        host: registry_service.grpc_host(),
        port: registry_service.grpc_port(),
        client_config: GrpcClientConfig::default(),
        invalidation_event_subscriber: Default::default(),
    });

    for (durable_storage_byte_seconds_delta, ephemeral_storage_byte_seconds_delta) in
        [(133, 476), (7, 24)]
    {
        registry_client
            .batch_update_resource_usage(HashMap::from([(
                AccountId(user.account_id.0),
                ResourceUsageUpdate {
                    fuel_delta: 0,
                    http_call_count_delta: 0,
                    rpc_call_count_delta: 0,
                    durable_storage_byte_seconds_delta,
                    ephemeral_storage_byte_seconds_delta,
                },
            )]))
            .await?;
    }

    let usage = registry_service
        .client(&user.token)
        .await
        .get_account_storage_usage(&user.account_id.0, None)
        .await?;

    assert_eq!(usage.account_id, user.account_id);
    assert_eq!(usage.usage.compute_gcu, 0.0);
    assert_eq!(
        usage.usage.durable_storage_gb_month,
        140.0 / BYTE_SECONDS_PER_GB_MONTH
    );
    assert_eq!(
        usage.usage.ephemeral_storage_gb_month,
        500.0 / BYTE_SECONDS_PER_GB_MONTH
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn account_storage_usage_history_is_authenticated_and_empty_for_new_account(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;

    let history = deps
        .registry_service()
        .client(&user.token)
        .await
        .get_account_storage_usage_history(&user.account_id.0, Some(6))
        .await?;

    assert!(history.is_empty());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn account_storage_override_endpoints_hide_foreign_accounts(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let foreign_user = deps.user().await?;
    assert_ne!(user.account_id, foreign_user.account_id);
    let client = deps.registry_service().client(&user.token).await;

    let error = client
        .get_account_storage_override(&foreign_user.account_id.0)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        golem_client::Error::Item(RegistryServiceGetAccountStorageOverrideError::Error404(_))
    ));

    let error = client
        .set_account_storage_override(
            &foreign_user.account_id.0,
            &SetStorageLimit {
                value: 1,
                expires_at: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        golem_client::Error::Item(RegistryServiceSetAccountStorageOverrideError::Error404(_))
    ));

    let error = client
        .clear_account_storage_override(&foreign_user.account_id.0)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        golem_client::Error::Item(RegistryServiceClearAccountStorageOverrideError::Error404(_))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn account_storage_override_endpoints_resolve_set_expire_and_clear(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let admin = deps.admin().await;
    let admin_client = admin.registry_service_client().await;
    admin_client
        .set_account_plan(
            &user.account_id.0,
            &AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: deps.registry_service().low_disk_space_plan(),
            },
        )
        .await?;

    let client = deps.registry_service().client(&user.token).await;
    let expected_default = StorageLimit {
        effective_value: 5,
        plan_default: 5,
        override_value: None,
        ceiling: 20,
        user_configurable: true,
    };
    assert_eq!(
        client
            .get_account_storage_override(&user.account_id.0)
            .await?,
        expected_default
    );

    let expected_override = StorageLimit {
        effective_value: 12,
        override_value: Some(12),
        ..expected_default.clone()
    };
    assert_eq!(
        client
            .set_account_storage_override(
                &user.account_id.0,
                &SetStorageLimit {
                    value: 12,
                    expires_at: None,
                },
            )
            .await?,
        expected_override
    );
    assert_eq!(
        client
            .get_account_storage_override(&user.account_id.0)
            .await?,
        expected_override
    );

    assert_eq!(
        admin_client
            .set_account_storage_override(
                &user.account_id.0,
                &SetStorageLimit {
                    value: 15,
                    expires_at: Some(Utc::now() - Duration::seconds(1)),
                },
            )
            .await?,
        expected_default
    );

    client
        .set_account_storage_override(
            &user.account_id.0,
            &SetStorageLimit {
                value: 12,
                expires_at: None,
            },
        )
        .await?;
    assert_eq!(
        client
            .clear_account_storage_override(&user.account_id.0)
            .await?,
        expected_default
    );
    assert_eq!(
        client
            .get_account_storage_override(&user.account_id.0)
            .await?,
        expected_default
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn account_storage_override_endpoints_validate_plan_ceiling_and_expiry(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    let error = client
        .set_account_storage_override(
            &user.account_id.0,
            &SetStorageLimit {
                value: 1,
                expires_at: None,
            },
        )
        .await
        .unwrap_err();
    let golem_client::Error::Item(RegistryServiceSetAccountStorageOverrideError::Error400(body)) =
        error
    else {
        panic!("expected non-configurable plan error, got {error:?}")
    };
    assert_eq!(body.code, "RESOURCE_OVERRIDE_NOT_USER_CONFIGURABLE");
    assert_eq!(body.errors, ["Storage limit is not user configurable"]);

    let admin = deps.admin().await;
    admin
        .registry_service_client()
        .await
        .set_account_plan(
            &user.account_id.0,
            &AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: deps.registry_service().low_disk_space_plan(),
            },
        )
        .await?;

    let error = client
        .set_account_storage_override(
            &user.account_id.0,
            &SetStorageLimit {
                value: 21,
                expires_at: None,
            },
        )
        .await
        .unwrap_err();
    let golem_client::Error::Item(RegistryServiceSetAccountStorageOverrideError::Error422(body)) =
        error
    else {
        panic!("expected plan ceiling error, got {error:?}")
    };
    assert_eq!(body.code, "LIMIT_EXCEEDED");
    assert_eq!(body.error, "Storage limit exceeds plan ceiling 20");

    let error = client
        .set_account_storage_override(
            &user.account_id.0,
            &SetStorageLimit {
                value: 12,
                expires_at: Some(Utc::now() + Duration::hours(1)),
            },
        )
        .await
        .unwrap_err();
    let golem_client::Error::Item(RegistryServiceSetAccountStorageOverrideError::Error403(body)) =
        error
    else {
        panic!("expected admin-only expiry error, got {error:?}")
    };
    assert_eq!(body.code, "AUTH_FORBIDDEN");
    assert_eq!(body.error, "Only admins may set an override expiry");

    Ok(())
}
