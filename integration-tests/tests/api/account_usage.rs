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

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use golem_client::api::{
    RegistryServiceClearAccountStorageOverrideError, RegistryServiceClient,
    RegistryServiceGetAccountStorageOverrideError, RegistryServiceSetAccountStorageOverrideError,
};
use golem_common::model::account::{AccountId, AccountRevision, AccountSetPlan};
use golem_common::model::account_usage::{
    AccountUsagePeriod, MemoryLimit, MeteringStatus, SetMemoryLimit, SetStorageLimit, StorageLimit,
};
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService as _, ResourceUsageMetering,
    ResourceUsageUpdate,
};
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_test_framework::components::rdb::DbInfo;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

fn previous_period(period: AccountUsagePeriod) -> AccountUsagePeriod {
    if period.month == 1 {
        AccountUsagePeriod {
            year: period.year - 1,
            month: 12,
        }
    } else {
        AccountUsagePeriod {
            year: period.year,
            month: period.month - 1,
        }
    }
}

fn registry_client(deps: &EnvBasedTestDependencies) -> GrpcRegistryService {
    let registry_service = deps.registry_service();
    GrpcRegistryService::new(&GrpcRegistryServiceConfig {
        host: registry_service.grpc_host(),
        port: registry_service.grpc_port(),
        client_config: GrpcClientConfig::default(),
        invalidation_event_subscriber: Default::default(),
    })
}

async fn insert_usage_for_period(
    deps: &EnvBasedTestDependencies,
    account_id: AccountId,
    period: AccountUsagePeriod,
) -> anyhow::Result<DateTime<Utc>> {
    let usage_key = period.to_string();
    let updated_at = DateTime::<Utc>::from_timestamp_micros(Utc::now().timestamp_micros())
        .expect("current timestamp is representable at database precision");
    const VALUES: [(i32, i64); 4] = [(2, 1_500_000), (10, 140), (11, 500), (12, 14)];
    match deps.rdb().info() {
        DbInfo::Sqlite(root) => {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    SqliteConnectOptions::new()
                        .filename(root.join("golem_registry.db"))
                        .journal_mode(SqliteJournalMode::Wal)
                        .foreign_keys(true),
                )
                .await?;
            for (usage_type, value) in VALUES {
                sqlx::query(
                    "INSERT INTO account_usage_stats \
                     (account_id, usage_type, usage_key, value, updated_at) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(account_id.0)
                .bind(usage_type)
                .bind(&usage_key)
                .bind(value)
                .bind(updated_at)
                .execute(&pool)
                .await
                .with_context(|| format!("failed to seed SQLite usage type {usage_type}"))?;
            }
            sqlx::query(
                "INSERT INTO account_usage_metering_state \
                 (account_id, usage_key, compute_enabled, memory_enabled, \
                  filesystem_enabled, updated_at) \
                 VALUES ($1, $2, TRUE, TRUE, TRUE, $3)",
            )
            .bind(account_id.0)
            .bind(&usage_key)
            .bind(updated_at)
            .execute(&pool)
            .await
            .context("failed to seed SQLite metering state")?;
        }
        DbInfo::Postgres(postgres) => {
            let pool = PgPoolOptions::new()
                .max_connections(1)
                .connect_with(postgres.to_connect_options())
                .await?;
            for (usage_type, value) in VALUES {
                sqlx::query(
                    "INSERT INTO golem_registry.account_usage_stats \
                     (account_id, usage_type, usage_key, value, updated_at) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(account_id.0)
                .bind(usage_type)
                .bind(&usage_key)
                .bind(value)
                .bind(updated_at)
                .execute(&pool)
                .await
                .with_context(|| format!("failed to seed PostgreSQL usage type {usage_type}"))?;
            }
            sqlx::query(
                "INSERT INTO golem_registry.account_usage_metering_state \
                 (account_id, usage_key, compute_enabled, memory_enabled, \
                  filesystem_enabled, updated_at) \
                 VALUES ($1, $2, TRUE, TRUE, TRUE, $3)",
            )
            .bind(account_id.0)
            .bind(&usage_key)
            .bind(updated_at)
            .execute(&pool)
            .await
            .context("failed to seed PostgreSQL metering state")?;
        }
        DbInfo::Mysql(_) => anyhow::bail!("registry service does not support MySQL"),
    }

    Ok(updated_at)
}

#[test]
#[tracing::instrument]
async fn account_usage_reports_all_customer_dimensions(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let registry_service = deps.registry_service();
    let registry_client = registry_client(deps);
    let byte_seconds_per_gb_month = (1024_u64.pow(3) * 730 * 3600) as f64;

    let updates_started_at = Utc::now();
    for (fuel_delta, durable_storage_byte_seconds_delta, ephemeral_storage_byte_seconds_delta) in
        [(1_000_000, 133, 476), (500_000, 7, 24)]
    {
        registry_client
            .batch_update_resource_usage(HashMap::from([(
                AccountId(user.account_id.0),
                ResourceUsageUpdate {
                    fuel_delta,
                    http_call_count_delta: 0,
                    rpc_call_count_delta: 0,
                    durable_storage_byte_seconds_delta,
                    ephemeral_storage_byte_seconds_delta,
                    memory_gb_seconds_delta: 7,
                    metering: ResourceUsageMetering::all_enabled(),
                },
            )]))
            .await?;
    }
    let updates_finished_at = Utc::now();

    let usage = registry_service
        .client(&user.token)
        .await
        .get_account_usage(&user.account_id.0, None)
        .await?;

    assert_eq!(usage.account_id, user.account_id);
    assert_eq!(usage.usage.compute_gcu, 1.5);
    assert_eq!(usage.usage.memory_gb_seconds, 14);
    assert_eq!(
        usage.usage.durable_storage_gb_month,
        140.0 / byte_seconds_per_gb_month
    );
    assert_eq!(
        usage.usage.ephemeral_storage_gb_month,
        500.0 / byte_seconds_per_gb_month
    );
    assert_eq!(usage.usage.metering.compute, MeteringStatus::Enabled);
    assert_eq!(usage.usage.metering.memory, MeteringStatus::Enabled);
    assert_eq!(
        usage.usage.metering.durable_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(
        usage.usage.metering.ephemeral_storage,
        MeteringStatus::Enabled
    );
    assert!(usage.usage.as_of >= updates_started_at);
    assert!(usage.usage.as_of <= updates_finished_at);

    let historical_period = previous_period(usage.usage.period);
    let historical_as_of =
        insert_usage_for_period(deps, user.account_id, historical_period).await?;
    let history = registry_service
        .client(&user.token)
        .await
        .get_account_usage_history(&user.account_id.0, Some(6))
        .await?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].account_id, user.account_id);
    assert_eq!(history[0].usage.period, historical_period);
    assert_eq!(history[0].usage.compute_gcu, 1.5);
    assert_eq!(history[0].usage.memory_gb_seconds, 14);
    assert_eq!(
        history[0].usage.durable_storage_gb_month,
        140.0 / byte_seconds_per_gb_month
    );
    assert_eq!(
        history[0].usage.ephemeral_storage_gb_month,
        500.0 / byte_seconds_per_gb_month
    );
    assert_eq!(history[0].usage.metering.compute, MeteringStatus::Enabled);
    assert_eq!(history[0].usage.metering.memory, MeteringStatus::Enabled);
    assert_eq!(
        history[0].usage.metering.durable_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(
        history[0].usage.metering.ephemeral_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(history[0].usage.as_of, historical_as_of);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn account_usage_history_is_authenticated_and_empty_for_new_account(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let request_started_at = Utc::now();

    let current = deps
        .registry_service()
        .client(&user.token)
        .await
        .get_account_usage(&user.account_id.0, None)
        .await?;
    let request_finished_at = Utc::now();

    assert_eq!(current.usage.compute_gcu, 0.0);
    assert_eq!(current.usage.memory_gb_seconds, 0);
    assert_eq!(current.usage.durable_storage_gb_month, 0.0);
    assert_eq!(current.usage.ephemeral_storage_gb_month, 0.0);
    assert_eq!(current.usage.metering.compute, MeteringStatus::Unknown);
    assert_eq!(current.usage.metering.memory, MeteringStatus::Unknown);
    assert_eq!(
        current.usage.metering.durable_storage,
        MeteringStatus::Unknown
    );
    assert_eq!(
        current.usage.metering.ephemeral_storage,
        MeteringStatus::Unknown
    );
    assert!(current.usage.as_of >= request_started_at);
    assert!(current.usage.as_of <= request_finished_at);

    let registry_client = registry_client(deps);
    registry_client
        .batch_update_resource_usage(HashMap::from([(
            AccountId(user.account_id.0),
            ResourceUsageUpdate {
                fuel_delta: 0,
                http_call_count_delta: 0,
                rpc_call_count_delta: 0,
                durable_storage_byte_seconds_delta: 0,
                ephemeral_storage_byte_seconds_delta: 0,
                memory_gb_seconds_delta: 0,
                metering: ResourceUsageMetering::default(),
            },
        )]))
        .await?;

    let disabled = deps
        .registry_service()
        .client(&user.token)
        .await
        .get_account_usage(&user.account_id.0, None)
        .await?;
    assert_eq!(disabled.usage.compute_gcu, 0.0);
    assert_eq!(disabled.usage.memory_gb_seconds, 0);
    assert_eq!(disabled.usage.durable_storage_gb_month, 0.0);
    assert_eq!(disabled.usage.ephemeral_storage_gb_month, 0.0);
    assert_eq!(disabled.usage.metering.compute, MeteringStatus::Disabled);
    assert_eq!(disabled.usage.metering.memory, MeteringStatus::Disabled);
    assert_eq!(
        disabled.usage.metering.durable_storage,
        MeteringStatus::Disabled
    );
    assert_eq!(
        disabled.usage.metering.ephemeral_storage,
        MeteringStatus::Disabled
    );

    registry_client
        .batch_update_resource_usage(HashMap::from([(
            AccountId(user.account_id.0),
            ResourceUsageUpdate {
                fuel_delta: 0,
                http_call_count_delta: 0,
                rpc_call_count_delta: 0,
                durable_storage_byte_seconds_delta: 0,
                ephemeral_storage_byte_seconds_delta: 0,
                memory_gb_seconds_delta: 0,
                metering: ResourceUsageMetering::all_enabled(),
            },
        )]))
        .await?;

    let enabled = deps
        .registry_service()
        .client(&user.token)
        .await
        .get_account_usage(&user.account_id.0, None)
        .await?;
    assert_eq!(enabled.usage.compute_gcu, 0.0);
    assert_eq!(enabled.usage.memory_gb_seconds, 0);
    assert_eq!(enabled.usage.durable_storage_gb_month, 0.0);
    assert_eq!(enabled.usage.ephemeral_storage_gb_month, 0.0);
    assert_eq!(enabled.usage.metering.compute, MeteringStatus::Enabled);
    assert_eq!(enabled.usage.metering.memory, MeteringStatus::Enabled);
    assert_eq!(
        enabled.usage.metering.durable_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(
        enabled.usage.metering.ephemeral_storage,
        MeteringStatus::Enabled
    );

    let history = deps
        .registry_service()
        .client(&user.token)
        .await
        .get_account_usage_history(&user.account_id.0, Some(6))
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

    let expected_max_memory = MemoryLimit {
        effective_value: 10_000_000_000_000_000,
        plan_default: 10_000_000_000_000_000,
        override_value: None,
        ceiling: 20_000_000_000_000_000,
        user_configurable: true,
    };
    assert_eq!(
        client
            .get_account_max_memory_override(&user.account_id.0)
            .await?,
        expected_max_memory
    );
    let max_memory_override = client
        .set_account_max_memory_override(
            &user.account_id.0,
            &SetMemoryLimit {
                value: 12_000_000_000_000_000,
                expires_at: None,
            },
        )
        .await?;
    assert_eq!(
        max_memory_override.override_value,
        Some(12_000_000_000_000_000)
    );
    assert_eq!(
        client
            .clear_account_max_memory_override(&user.account_id.0)
            .await?,
        expected_max_memory
    );

    let expected_monthly_memory = MemoryLimit {
        effective_value: 30,
        plan_default: 30,
        override_value: None,
        ceiling: 60,
        user_configurable: true,
    };
    assert_eq!(
        client
            .get_account_monthly_memory_override(&user.account_id.0)
            .await?,
        expected_monthly_memory
    );
    let monthly_override = client
        .set_account_monthly_memory_override(
            &user.account_id.0,
            &SetMemoryLimit {
                value: 45,
                expires_at: None,
            },
        )
        .await?;
    assert_eq!(monthly_override.override_value, Some(45));
    assert_eq!(
        client
            .clear_account_monthly_memory_override(&user.account_id.0)
            .await?,
        expected_monthly_memory
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
