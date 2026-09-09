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

use crate::repo::account_resource_override::DbAccountResourceOverrideRepo;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::account_resource_override::AccountResourceOverrideDimension;
use crate::repo::model::account_usage::{
    AccountMonthlyUsageMode, AccountUsage, AccountUsagePlan, AccountUsageRecord,
    MonthlyUsageModeStateRecord, MonthlyUsageModeTransitionRecord, MonthlyUsageTransitionBaseline,
    PersistedMonthlyUsageModeTransition, UsageGrouping, UsageTracking, UsageType,
};
use async_trait::async_trait;
use chrono::Datelike;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::model::account_usage::{
    AccountUsagePeriod, BYTE_NANOSECONDS_PER_GB_SECOND, MemoryLimit, MonthlyUsageMode,
    MonthlyUsageModeTransitionSource, StorageLimit,
};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::NumericU64;
use golem_service_base::repo::{PoolLabelledTransaction, RepoError, RepoResult, SqlDateTime};
use indoc::indoc;
use sqlx::{Database, FromRow, QueryBuilder, Row};
use std::collections::BTreeMap;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct AccountUsageReportRow {
    usage_type: Option<UsageType>,
    value: Option<NumericU64>,
    updated_at: SqlDateTime,
    compute_enabled: Option<bool>,
    memory_enabled: Option<bool>,
    filesystem_enabled: Option<bool>,
}

#[derive(sqlx::FromRow)]
struct MonthlyUsageBaselineRow {
    compute_fuel: NumericU64,
    memory_gb_seconds: NumericU64,
    durable_storage_byte_seconds: NumericU64,
    ephemeral_storage_byte_seconds: NumericU64,
}

#[derive(Debug, thiserror::Error)]
pub enum SetMonthlyUsageModeError {
    #[error("Account {0} not found")]
    AccountNotFound(Uuid),
    #[error("The account's current plan does not permit paid overage")]
    OverageNotEligible,
    #[error("Monthly usage mode is already {0}")]
    ModeUnchanged(MonthlyUsageMode),
    #[error("Only owner transitions may enable paid overage")]
    InvalidConsentSource,
    #[error("Paid-overage consent must be recorded by the account owner")]
    InvalidConsentActor,
    #[error(transparent)]
    Repo(#[from] RepoError),
}

impl AccountUsageReportRow {
    fn apply_to(self, report: &mut AccountUsageRecord) {
        let updated_at = self.updated_at.into_utc();
        match (
            self.usage_type,
            self.value,
            self.compute_enabled,
            self.memory_enabled,
            self.filesystem_enabled,
        ) {
            (Some(usage_type), Some(value), _, _, _) => {
                report.apply(usage_type, value.get(), updated_at)
            }
            (None, None, Some(compute), Some(memory), Some(filesystem)) => {
                report.apply_metering(compute, memory, filesystem, updated_at)
            }
            _ => {}
        }
    }
}

#[async_trait]
pub trait AccountUsageRepo: Send + Sync {
    async fn get(&self, account_id: Uuid, date: &SqlDateTime) -> RepoResult<Option<AccountUsage>> {
        self.get_with_active_overrides_at(account_id, date, &SqlDateTime::now())
            .await
    }

    async fn get_with_active_overrides_at(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        active_at: &SqlDateTime,
    ) -> RepoResult<Option<AccountUsage>>;

    async fn get_for_type(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        usage_type: UsageType,
    ) -> RepoResult<Option<AccountUsage>>;

    async fn get_usage_report(
        &self,
        account_id: Uuid,
        period: AccountUsagePeriod,
    ) -> RepoResult<AccountUsageRecord>;

    async fn get_usage_history(
        &self,
        account_id: Uuid,
        before: AccountUsagePeriod,
        last: usize,
    ) -> RepoResult<Vec<AccountUsageRecord>>;

    async fn get_monthly_usage_mode(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Option<AccountMonthlyUsageMode>>;

    async fn get_monthly_usage_mode_transitions(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<PersistedMonthlyUsageModeTransition>>;

    async fn set_monthly_usage_mode(
        &self,
        account_id: Uuid,
        target: MonthlyUsageMode,
        actor_account_id: Uuid,
        source: MonthlyUsageModeTransitionSource,
    ) -> Result<PersistedMonthlyUsageModeTransition, SetMonthlyUsageModeError>;

    async fn add(&self, account_usage: &AccountUsage) -> RepoResult<u64>;
}

pub struct LoggedAccountUsageRepo<Repo: AccountUsageRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "account usage repository";

impl<Repo: AccountUsageRepo> LoggedAccountUsageRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_account_id(account_id: Uuid) -> Span {
        info_span!(SPAN_NAME, account_id=%account_id)
    }
}

#[async_trait]
impl<Repo: AccountUsageRepo> AccountUsageRepo for LoggedAccountUsageRepo<Repo> {
    async fn get(&self, account_id: Uuid, date: &SqlDateTime) -> RepoResult<Option<AccountUsage>> {
        self.repo
            .get(account_id, date)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_with_active_overrides_at(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        active_at: &SqlDateTime,
    ) -> RepoResult<Option<AccountUsage>> {
        self.repo
            .get_with_active_overrides_at(account_id, date, active_at)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_for_type(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        usage_type: UsageType,
    ) -> RepoResult<Option<AccountUsage>> {
        self.repo
            .get_for_type(account_id, date, usage_type)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_usage_report(
        &self,
        account_id: Uuid,
        period: AccountUsagePeriod,
    ) -> RepoResult<AccountUsageRecord> {
        self.repo
            .get_usage_report(account_id, period)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_usage_history(
        &self,
        account_id: Uuid,
        before: AccountUsagePeriod,
        last: usize,
    ) -> RepoResult<Vec<AccountUsageRecord>> {
        self.repo
            .get_usage_history(account_id, before, last)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_monthly_usage_mode(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Option<AccountMonthlyUsageMode>> {
        self.repo
            .get_monthly_usage_mode(account_id)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_monthly_usage_mode_transitions(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<PersistedMonthlyUsageModeTransition>> {
        self.repo
            .get_monthly_usage_mode_transitions(account_id)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn set_monthly_usage_mode(
        &self,
        account_id: Uuid,
        target: MonthlyUsageMode,
        actor_account_id: Uuid,
        source: MonthlyUsageModeTransitionSource,
    ) -> Result<PersistedMonthlyUsageModeTransition, SetMonthlyUsageModeError> {
        self.repo
            .set_monthly_usage_mode(account_id, target, actor_account_id, source)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn add(&self, account_usage: &AccountUsage) -> RepoResult<u64> {
        self.repo
            .add(account_usage)
            .instrument(Self::span_account_id(account_usage.account_id))
            .await
    }
}

pub struct DbAccountUsageRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "account_usage";

impl<DBP: Pool> DbAccountUsageRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedAccountUsageRepo<Self>
    where
        Self: AccountUsageRepo,
    {
        LoggedAccountUsageRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> RepoResult<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, RepoResult<R>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbAccountUsageRepo<PostgresPool> {
    async fn monthly_memory_remainder_byte_nanoseconds(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
    ) -> RepoResult<u128> {
        let value: Option<(NumericU64,)> = self
            .with_ro("get_monthly_memory_remainders")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT byte_nanoseconds
                    FROM account_monthly_memory_remainders
                    WHERE account_id = $1 AND usage_key = $2
                "#})
                .bind(account_id)
                .bind(date_to_usage_key(date)),
            )
            .await?;
        Ok(value.map_or(0, |(value,)| value.get() as u128))
    }

    async fn usage_baseline_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
        period: AccountUsagePeriod,
    ) -> RepoResult<MonthlyUsageBaselineRow> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                SELECT
                    COALESCE(MAX(CASE WHEN usage_type = $3 THEN value END), 0) AS compute_fuel,
                    COALESCE(MAX(CASE WHEN usage_type = $4 THEN value END), 0) AS memory_gb_seconds,
                    COALESCE(MAX(CASE WHEN usage_type = $5 THEN value END), 0) AS durable_storage_byte_seconds,
                    COALESCE(MAX(CASE WHEN usage_type = $6 THEN value END), 0) AS ephemeral_storage_byte_seconds
                FROM account_usage_stats
                WHERE account_id = $1 AND usage_key = $2
            "#})
            .bind(account_id)
            .bind(year_and_month_to_usage_key(period.year, period.month))
            .bind(UsageType::MonthlyGasLimit)
            .bind(UsageType::MonthlyMemoryGbSeconds)
            .bind(UsageType::MonthlyDurableAgentStorageByteSeconds)
            .bind(UsageType::MonthlyEphemeralStorageByteSeconds),
        )
        .await
    }

    async fn current_mode_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
    ) -> RepoResult<(MonthlyUsageMode, u64)> {
        let mode: Option<(String, NumericU64)> = tx
            .fetch_optional_as(
                sqlx::query_as(
                    "SELECT mode, revision FROM account_monthly_usage_modes WHERE account_id = $1",
                )
                .bind(account_id),
            )
            .await?;
        match mode {
            Some((mode, revision)) => Ok((
                crate::repo::model::account_usage::monthly_usage_mode(&mode)?,
                revision.get(),
            )),
            None => Ok((MonthlyUsageMode::HardLimit, 0)),
        }
    }

    async fn write_transition_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
        actor_account_id: Uuid,
        source: MonthlyUsageModeTransitionSource,
        revision: u64,
        previous_mode: MonthlyUsageMode,
        new_mode: MonthlyUsageMode,
    ) -> RepoResult<PersistedMonthlyUsageModeTransition> {
        let now = chrono::Utc::now();
        let changed_at = SqlDateTime::new(
            chrono::DateTime::from_timestamp_micros(now.timestamp_micros())
                .expect("current timestamp is representable at microsecond precision"),
        );
        let period = AccountUsagePeriod {
            year: changed_at.as_utc().year(),
            month: changed_at.as_utc().month(),
        };
        let baseline = Self::usage_baseline_in_tx(tx, account_id, period).await?;
        let previous_mode_value =
            crate::repo::model::account_usage::monthly_usage_mode_str(previous_mode);
        let new_mode_value = crate::repo::model::account_usage::monthly_usage_mode_str(new_mode);

        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO account_monthly_usage_mode_transitions (
                    transition_id, account_id, revision, actor_account_id, source, changed_at,
                    previous_mode, new_mode, period_year, period_month,
                    compute_fuel, memory_gb_seconds,
                    durable_storage_byte_seconds, ephemeral_storage_byte_seconds
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            "#})
            .bind(Uuid::new_v4())
            .bind(account_id)
            .bind(NumericU64::new(revision))
            .bind(actor_account_id)
            .bind(
                crate::repo::model::account_usage::monthly_usage_mode_transition_source_str(source),
            )
            .bind(&changed_at)
            .bind(previous_mode_value)
            .bind(new_mode_value)
            .bind(period.year)
            .bind(i32::try_from(period.month).expect("month fits in i32"))
            .bind(baseline.compute_fuel)
            .bind(baseline.memory_gb_seconds)
            .bind(baseline.durable_storage_byte_seconds)
            .bind(baseline.ephemeral_storage_byte_seconds),
        )
        .await?;

        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO account_monthly_usage_modes (account_id, mode, revision, changed_by, changed_at)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (account_id) DO UPDATE SET
                    mode = $2,
                    revision = $3,
                    changed_by = $4,
                    changed_at = $5
            "#})
            .bind(account_id)
            .bind(new_mode_value)
            .bind(NumericU64::new(revision))
            .bind(actor_account_id)
            .bind(&changed_at),
        )
        .await?;

        Ok(PersistedMonthlyUsageModeTransition {
            revision,
            actor_account_id: golem_common::model::account::AccountId(actor_account_id),
            changed_at: changed_at.into_utc(),
            source,
            previous_mode,
            new_mode,
            usage_baseline: MonthlyUsageTransitionBaseline {
                period,
                compute_fuel: baseline.compute_fuel.get(),
                memory_gb_seconds: baseline.memory_gb_seconds.get(),
                durable_storage_byte_seconds: baseline.durable_storage_byte_seconds.get(),
                ephemeral_storage_byte_seconds: baseline.ephemeral_storage_byte_seconds.get(),
            },
        })
    }

    pub(crate) async fn force_hard_limit_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
        actor_account_id: Uuid,
        source: MonthlyUsageModeTransitionSource,
    ) -> RepoResult<Option<PersistedMonthlyUsageModeTransition>> {
        let (current, revision) = Self::current_mode_in_tx(tx, account_id).await?;
        if current == MonthlyUsageMode::HardLimit {
            return Ok(None);
        }
        let revision = revision.checked_add(1).ok_or_else(|| {
            RepoError::InternalError(anyhow::anyhow!(
                "Monthly usage mode revision overflow for account {account_id}"
            ))
        })?;
        Self::write_transition_in_tx(
            tx,
            account_id,
            actor_account_id,
            source,
            revision,
            current,
            MonthlyUsageMode::HardLimit,
        )
        .await
        .map(Some)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountUsageRepo for DbAccountUsageRepo<PostgresPool> {
    async fn get_with_active_overrides_at(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        active_at: &SqlDateTime,
    ) -> RepoResult<Option<AccountUsage>> {
        let Some(account_plan) = self.get_plan(account_id, active_at).await? else {
            return Ok(None);
        };

        let usage_rows = self
            .with_ro("get")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    WITH counts AS (
                        SELECT
                            CAST(COUNT(DISTINCT a.application_id) AS NUMERIC) AS total_apps,
                            CAST(COUNT(DISTINCT e.environment_id) AS NUMERIC) AS total_envs,
                            CAST(COUNT(DISTINCT c.component_id) AS NUMERIC) AS total_components,
                            CASE
                                WHEN SUM(cr.size) > 18446744073709551615
                                THEN 18446744073709551615
                                ELSE COALESCE(SUM(cr.size), 0)
                            END AS total_component_size
                        FROM applications a
                        LEFT JOIN environments e
                            ON e.application_id = a.application_id
                            AND e.deleted_at IS NULL
                        LEFT JOIN components c
                            ON c.environment_id = e.environment_id
                            AND c.deleted_at IS NULL
                        LEFT JOIN component_revisions cr
                            ON c.component_id = cr.component_id
                        WHERE
                            a.account_id = $1
                            AND a.deleted_at IS NULL
                    )
                    SELECT
                        usage_type,
                        value,
                        CAST(NULL AS NUMERIC) AS memory_byte_nanoseconds_remainder
                    FROM
                        account_usage_stats
                    WHERE
                        account_id = $1
                        AND usage_key IN ($2, $3)
                    UNION ALL SELECT $4 AS usage_type, total_apps AS value, CAST(NULL AS NUMERIC) FROM counts
                    UNION ALL SELECT $5 AS usage_type, total_envs AS value, CAST(NULL AS NUMERIC) FROM counts
                    UNION ALL SELECT $6 AS usage_type, total_components AS value, CAST(NULL AS NUMERIC) FROM counts
                    UNION ALL SELECT $7 AS usage_type, total_component_size AS value, CAST(NULL AS NUMERIC) FROM counts
                    UNION ALL
                    SELECT CAST(NULL AS INTEGER), CAST(NULL AS NUMERIC), byte_nanoseconds
                    FROM account_monthly_memory_remainders
                    WHERE account_id = $1 AND usage_key = $2;
                "#})
                .bind(account_id)
                .bind(date_to_usage_key(date))
                .bind(USAGE_KEY_TOTAL)
                .bind(UsageType::TotalAppCount)
                .bind(UsageType::TotalEnvCount)
                .bind(UsageType::TotalComponentCount)
                .bind(UsageType::TotalComponentStorageBytes),
            )
            .await?;

        let mut usage = BTreeMap::new();
        let mut monthly_memory_byte_nanoseconds_remainder = 0u128;
        for row in usage_rows {
            let usage_type = row.try_get::<Option<UsageType>, _>("usage_type")?;
            let value = row.try_get::<Option<NumericU64>, _>("value")?;
            if let (Some(usage_type), Some(value)) = (usage_type, value) {
                usage.insert(usage_type, value.get());
            }
            if let Some(remainder) =
                row.try_get::<Option<NumericU64>, _>("memory_byte_nanoseconds_remainder")?
            {
                monthly_memory_byte_nanoseconds_remainder =
                    monthly_memory_byte_nanoseconds_remainder
                        .saturating_add(remainder.get() as u128);
            }
        }

        let admin_grant_values = account_plan.admin_grant_values();
        let admin_grants = account_plan.admin_grants()?;
        let monthly_usage_mode = account_plan.monthly_usage_mode()?;
        Ok(Some(AccountUsage {
            account_id,
            year: date.as_utc().year(),
            month: date.as_utc().month(),
            usage,
            storage_limit: storage_limit(&account_plan),
            max_memory_per_worker: max_memory_per_worker(&account_plan),
            admin_grant_values,
            admin_grants,
            metering: None,
            monthly_usage_mode,
            monthly_usage_mode_revision: account_plan.monthly_usage_mode_revision.get(),
            monthly_memory_byte_nanoseconds_remainder,
            monthly_usage_attribution: None,
            plan: account_plan.plan,
            changes: Default::default(),
        }))
    }

    async fn get_for_type(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        usage_type: UsageType,
    ) -> RepoResult<Option<AccountUsage>> {
        let Some(account_plan) = self.get_plan(account_id, &SqlDateTime::now()).await? else {
            return Ok(None);
        };

        let usage_rows = {
            match usage_type.tracking() {
                UsageTracking::Stats => {
                    self.with_ro("get_for_type - stats")
                        .fetch_all(
                            sqlx::query(indoc! { r#"
                                SELECT usage_type, value FROM account_usage_stats
                                WHERE account_id = $1 AND usage_key = $2
                            "#})
                                .bind(account_id)
                                .bind(match usage_type.grouping() {
                                    UsageGrouping::Total => USAGE_KEY_TOTAL.to_string(),
                                    UsageGrouping::Monthly => date_to_usage_key(date),
                                }),
                        )
                        .await?
                }
                UsageTracking::SelectTotalAppCount => {
                    self.with_ro("get_for_type - total apps")
                        .fetch_all(
                            sqlx::query(indoc! { r#"
                                SELECT $1 as usage_type, (
                                    SELECT CAST(COUNT(*) AS NUMERIC)
                                    FROM applications
                                    WHERE account_id = $2 AND deleted_at IS NULL
                                ) as value
                            "#})
                                .bind(UsageType::TotalAppCount)
                                .bind(account_id),
                        )
                        .await?
                }
                UsageTracking::SelectTotalEnvCount => {
                    self.with_ro("get_for_type - total envs")
                        .fetch_all(
                            sqlx::query(indoc! { r#"
                                SELECT $1 as usage_type, (
                                    SELECT CAST(COUNT(*) AS NUMERIC)
                                    FROM applications a
                                    JOIN environments e ON e.application_id = a.application_id
                                    WHERE a.account_id = $2 AND a.deleted_at IS NULL AND e.deleted_at IS NULL
                                ) as value
                            "#})
                                .bind(UsageType::TotalEnvCount)
                                .bind(account_id),
                        )
                        .await?
                }
                UsageTracking::SelectTotalComponentCount => {
                    self.with_ro("get_for_type - total components")
                        .fetch_all(
                            sqlx::query(indoc! { r#"
                                SELECT $1 as usage_type, (
                                    SELECT CAST(COUNT(*) AS NUMERIC)
                                    FROM applications a
                                    JOIN environments e ON e.application_id = a.application_id
                                    JOIN components c ON c.environment_id = e.environment_id
                                    WHERE a.account_id = $2 AND a.deleted_at IS NULL AND e.deleted_at IS NULL AND c.deleted_at IS NULL
                                ) as value
                            "#})
                                .bind(UsageType::TotalAppCount)
                                .bind(account_id),
                        )
                        .await?
                }
                UsageTracking::SelectTotalComponentSize => {
                    self.with_ro("get_for_type - total component size")
                        .fetch_all(
                            sqlx::query(indoc! { r#"
                                SELECT
                                    $1 AS usage_type,
                                    (
                                        SELECT
                                            CASE
                                                WHEN total > 18446744073709551615
                                                THEN 18446744073709551615
                                                ELSE total
                                            END AS value
                                        FROM (
                                            SELECT COALESCE(SUM(cr.size), 0) AS total
                                            FROM applications a
                                            JOIN environments e
                                                ON e.application_id = a.application_id
                                            JOIN components c
                                                ON c.environment_id = e.environment_id
                                            JOIN component_revisions cr
                                                ON c.component_id = cr.component_id
                                            WHERE
                                                a.account_id = $2
                                                AND a.deleted_at IS NULL
                                                AND e.deleted_at IS NULL
                                                AND c.deleted_at IS NULL
                                        ) AS t
                                    ) AS value;
                            "#})
                                .bind(UsageType::TotalComponentStorageBytes)
                                .bind(account_id),
                        )
                        .await?
                }
            }
        };
        let mut usage = BTreeMap::new();
        for row in usage_rows {
            usage.insert(
                row.try_get("usage_type")?,
                row.try_get::<NumericU64, _>("value")?.get(),
            );
        }

        let admin_grant_values = account_plan.admin_grant_values();
        let admin_grants = account_plan.admin_grants()?;
        let monthly_usage_mode = account_plan.monthly_usage_mode()?;
        let monthly_memory_byte_nanoseconds_remainder = self
            .monthly_memory_remainder_byte_nanoseconds(account_id, date)
            .await?;
        Ok(Some(AccountUsage {
            account_id,
            year: date.as_utc().year(),
            month: date.as_utc().month(),
            usage,
            storage_limit: storage_limit(&account_plan),
            max_memory_per_worker: max_memory_per_worker(&account_plan),
            admin_grant_values,
            admin_grants,
            metering: None,
            monthly_usage_mode,
            monthly_usage_mode_revision: account_plan.monthly_usage_mode_revision.get(),
            monthly_memory_byte_nanoseconds_remainder,
            monthly_usage_attribution: None,
            plan: account_plan.plan,
            changes: Default::default(),
        }))
    }

    async fn get_usage_report(
        &self,
        account_id: Uuid,
        period: AccountUsagePeriod,
    ) -> RepoResult<AccountUsageRecord> {
        let rows = self
            .with_ro("get_usage_report")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    SELECT
                        usage_type,
                        value,
                        updated_at,
                        NULL AS compute_enabled,
                        NULL AS memory_enabled,
                        NULL AS filesystem_enabled
                    FROM account_usage_stats
                    WHERE account_id = $1
                        AND usage_key = $2
                        AND usage_type IN ($3, $4, $5, $6)
                    UNION ALL
                    SELECT
                        NULL AS usage_type,
                        NULL AS value,
                        updated_at,
                        compute_enabled,
                        memory_enabled,
                        filesystem_enabled
                    FROM account_usage_metering_state
                    WHERE account_id = $1 AND usage_key = $2
                "#})
                .bind(account_id)
                .bind(year_and_month_to_usage_key(period.year, period.month))
                .bind(UsageType::MonthlyDurableAgentStorageByteSeconds)
                .bind(UsageType::MonthlyEphemeralStorageByteSeconds)
                .bind(UsageType::MonthlyGasLimit)
                .bind(UsageType::MonthlyMemoryGbSeconds),
            )
            .await?;

        let mut report = AccountUsageRecord::new(period);
        for row in rows {
            AccountUsageReportRow::from_row(&row)?.apply_to(&mut report);
        }
        Ok(report)
    }

    async fn get_usage_history(
        &self,
        account_id: Uuid,
        before: AccountUsagePeriod,
        last: usize,
    ) -> RepoResult<Vec<AccountUsageRecord>> {
        let rows = self
            .with_ro("get_usage_history")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    WITH periods AS (
                        SELECT usage_key
                        FROM account_usage_stats
                        WHERE account_id = $1
                            AND usage_key < $2
                            AND usage_type IN ($3, $4, $5, $6)
                        UNION
                        SELECT usage_key
                        FROM account_usage_metering_state
                        WHERE account_id = $1 AND usage_key < $2
                        ORDER BY 1 DESC
                        LIMIT $7
                    )
                    SELECT
                        stats.usage_key,
                        stats.usage_type,
                        stats.value,
                        stats.updated_at,
                        NULL AS compute_enabled,
                        NULL AS memory_enabled,
                        NULL AS filesystem_enabled
                    FROM account_usage_stats stats
                    JOIN periods ON periods.usage_key = stats.usage_key
                    WHERE stats.account_id = $1
                        AND stats.usage_type IN ($3, $4, $5, $6)
                    UNION ALL
                    SELECT
                        state.usage_key,
                        NULL AS usage_type,
                        NULL AS value,
                        state.updated_at,
                        state.compute_enabled,
                        state.memory_enabled,
                        state.filesystem_enabled
                    FROM account_usage_metering_state state
                    JOIN periods ON periods.usage_key = state.usage_key
                    WHERE state.account_id = $1
                    ORDER BY 1 DESC
                "#})
                .bind(account_id)
                .bind(year_and_month_to_usage_key(before.year, before.month))
                .bind(UsageType::MonthlyDurableAgentStorageByteSeconds)
                .bind(UsageType::MonthlyEphemeralStorageByteSeconds)
                .bind(UsageType::MonthlyGasLimit)
                .bind(UsageType::MonthlyMemoryGbSeconds)
                .bind(i64::try_from(last).unwrap_or(i64::MAX)),
            )
            .await?;

        let mut periods = BTreeMap::<AccountUsagePeriod, AccountUsageRecord>::new();
        for row in rows {
            let usage_key: String = row.try_get("usage_key")?;
            let Some((year, month)) = usage_key.split_once('-') else {
                continue;
            };
            let (Ok(year), Ok(month)) = (year.parse(), month.parse()) else {
                continue;
            };
            let period = AccountUsagePeriod { year, month };
            let report = periods
                .entry(period)
                .or_insert_with(|| AccountUsageRecord::new(period));
            AccountUsageReportRow::from_row(&row)?.apply_to(report);
        }

        Ok(periods
            .into_iter()
            .rev()
            .map(|(_, report)| report)
            .collect())
    }

    async fn get_monthly_usage_mode(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Option<AccountMonthlyUsageMode>> {
        self.with_tx("get_monthly_usage_mode", |tx| {
            async move {
                if DbAccountResourceOverrideRepo::<PostgresPool>::lock_account_in_tx(
                    tx, account_id,
                )
                .await?
                .is_none()
                {
                    return Ok(None);
                }
                let state: MonthlyUsageModeStateRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            SELECT COALESCE(mode.mode, 'hard_limit') AS mode,
                                   COALESCE(mode.revision, 0) AS revision,
                                   plan.overage_eligible
                            FROM accounts account
                            JOIN account_revisions revision
                              ON revision.account_id = account.account_id
                             AND revision.revision_id = account.current_revision_id
                            JOIN plans plan ON plan.plan_id = revision.plan_id
                            LEFT JOIN account_monthly_usage_modes mode
                              ON mode.account_id = account.account_id
                            WHERE account.account_id = $1
                        "#})
                        .bind(account_id),
                    )
                    .await?;
                let latest_owner_transition: Option<MonthlyUsageModeTransitionRecord> = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! { r#"
                            SELECT revision, actor_account_id, changed_at, source, previous_mode, new_mode,
                                   period_year, period_month, compute_fuel, memory_gb_seconds,
                                   durable_storage_byte_seconds, ephemeral_storage_byte_seconds
                            FROM account_monthly_usage_mode_transitions
                            WHERE account_id = $1 AND source = 'owner'
                            ORDER BY revision DESC
                            LIMIT 1
                        "#})
                        .bind(account_id),
                    )
                    .await?;
                let latest_owner_transition = latest_owner_transition
                    .map(MonthlyUsageModeTransitionRecord::into_model)
                    .transpose()?
                    .map(PersistedMonthlyUsageModeTransition::into_public);
                Ok(Some(AccountMonthlyUsageMode {
                    mode: state.mode()?,
                    revision: state.revision.get(),
                    overage_eligible: state.overage_eligible,
                    latest_owner_transition,
                }))
            }
            .boxed()
        })
        .await
    }

    async fn get_monthly_usage_mode_transitions(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<PersistedMonthlyUsageModeTransition>> {
        let records: Vec<MonthlyUsageModeTransitionRecord> = self
            .with_ro("get_monthly_usage_mode_transitions")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT revision, actor_account_id, changed_at, source, previous_mode, new_mode,
                           period_year, period_month, compute_fuel, memory_gb_seconds,
                           durable_storage_byte_seconds, ephemeral_storage_byte_seconds
                    FROM account_monthly_usage_mode_transitions
                    WHERE account_id = $1
                    ORDER BY revision
                "#})
                .bind(account_id),
            )
            .await?;
        records
            .into_iter()
            .map(MonthlyUsageModeTransitionRecord::into_model)
            .collect()
    }

    async fn set_monthly_usage_mode(
        &self,
        account_id: Uuid,
        target: MonthlyUsageMode,
        actor_account_id: Uuid,
        source: MonthlyUsageModeTransitionSource,
    ) -> Result<PersistedMonthlyUsageModeTransition, SetMonthlyUsageModeError> {
        if target == MonthlyUsageMode::AllowOverage
            && source != MonthlyUsageModeTransitionSource::Owner
        {
            return Err(SetMonthlyUsageModeError::InvalidConsentSource);
        }
        if target == MonthlyUsageMode::AllowOverage && actor_account_id != account_id {
            return Err(SetMonthlyUsageModeError::InvalidConsentActor);
        }
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "set_monthly_usage_mode", |tx| {
                async move {
                    let plan_id =
                        DbAccountResourceOverrideRepo::<PostgresPool>::lock_account_in_tx(
                            tx, account_id,
                        )
                        .await?
                        .ok_or(SetMonthlyUsageModeError::AccountNotFound(account_id))?;
                    let (overage_eligible,): (bool,) = tx
                        .fetch_one_as(
                            sqlx::query_as("SELECT overage_eligible FROM plans WHERE plan_id = $1")
                                .bind(plan_id),
                        )
                        .await?;
                    if target == MonthlyUsageMode::AllowOverage && !overage_eligible {
                        return Err(SetMonthlyUsageModeError::OverageNotEligible);
                    }
                    let (current, revision) = Self::current_mode_in_tx(tx, account_id).await?;
                    if current == target {
                        return Err(SetMonthlyUsageModeError::ModeUnchanged(target));
                    }
                    let revision = revision.checked_add(1).ok_or_else(|| {
                        RepoError::InternalError(anyhow::anyhow!(
                            "Monthly usage mode revision overflow for account {account_id}"
                        ))
                    })?;
                    Ok(Self::write_transition_in_tx(
                        tx,
                        account_id,
                        actor_account_id,
                        source,
                        revision,
                        current,
                        target,
                    )
                    .await?)
                }
                .boxed()
            })
            .await
    }

    async fn add(&self, account_usage: &AccountUsage) -> RepoResult<u64> {
        let account_id = account_usage.account_id;
        let date_usage_key = year_and_month_to_usage_key(account_usage.year, account_usage.month);
        let changes = account_usage
            .changes
            .iter()
            .filter(|(usage_type, change)| {
                usage_type.tracking() == UsageTracking::Stats && **change != 0
            })
            .map(|(usage_type, change)| {
                let usage_key = match usage_type.grouping() {
                    UsageGrouping::Total => USAGE_KEY_TOTAL.to_string(),
                    UsageGrouping::Monthly => date_usage_key.clone(),
                };
                (*usage_type, usage_key, *change)
            })
            .collect::<Vec<_>>();
        let attributed_usage = (
            account_usage.change(UsageType::MonthlyGasLimit),
            account_usage.change(UsageType::MonthlyMemoryGbSeconds),
            account_usage.change(UsageType::MonthlyDurableAgentStorageByteSeconds),
            account_usage.change(UsageType::MonthlyEphemeralStorageByteSeconds),
        );
        let attribution = account_usage.monthly_usage_attribution;
        let attribution_revision = attribution
            .map(|attribution| attribution.revision)
            .unwrap_or(account_usage.monthly_usage_mode_revision);
        let attributed_remainders = attribution.map_or((0, 0, 0), |attribution| {
            (
                attribution.memory_byte_nanoseconds_remainder,
                attribution.durable_storage_byte_nanoseconds_remainder,
                attribution.ephemeral_storage_byte_nanoseconds_remainder,
            )
        });
        let metering = account_usage.metering;

        self.with_tx("change_usage", |tx| {
            async move {
                DbAccountResourceOverrideRepo::<PostgresPool>::lock_account_in_tx(tx, account_id)
                    .await?;
                let (_, current_revision) = Self::current_mode_in_tx(tx, account_id).await?;
                if attribution_revision > current_revision {
                    return Err(RepoError::InternalError(anyhow::anyhow!(
                        "Resource usage references future monthly usage mode revision {attribution_revision} for account {account_id}; current revision is {current_revision}"
                    )));
                }
                let updated_at = SqlDateTime::now();
                let mut changes = changes;
                if attributed_remainders.0 != 0 {
                    let previous: Option<(NumericU64,)> = tx
                        .fetch_optional_as(
                            sqlx::query_as(indoc! { r#"
                                SELECT byte_nanoseconds
                                FROM account_monthly_memory_remainders
                                WHERE account_id = $1 AND usage_key = $2
                            "#})
                            .bind(account_id)
                            .bind(&date_usage_key),
                        )
                        .await?;
                    let total = previous.map_or(0, |(value,)| value.get() as u128)
                        + attributed_remainders.0 as u128;
                    let carry = total / BYTE_NANOSECONDS_PER_GB_SECOND;
                    let remainder = total % BYTE_NANOSECONDS_PER_GB_SECOND;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO account_monthly_memory_remainders (
                                account_id, usage_key, byte_nanoseconds, updated_at
                            ) VALUES ($1, $2, $3, $4)
                            ON CONFLICT (account_id, usage_key) DO UPDATE SET
                                byte_nanoseconds = excluded.byte_nanoseconds,
                                updated_at = excluded.updated_at
                        "#})
                        .bind(account_id)
                        .bind(&date_usage_key)
                        .bind(NumericU64::new(remainder as u64))
                        .bind(&updated_at),
                    )
                    .await?;

                    if carry != 0 {
                        let carry = i64::try_from(carry).unwrap_or(i64::MAX);
                        if let Some((_, _, change)) = changes.iter_mut().find(
                            |(usage_type, _, _)| *usage_type == UsageType::MonthlyMemoryGbSeconds,
                        ) {
                            *change = change.saturating_add(carry);
                        } else {
                            changes.push((
                                UsageType::MonthlyMemoryGbSeconds,
                                date_usage_key.clone(),
                                carry,
                            ));
                        }
                    }
                }
                if !changes.is_empty() {
                    let mut query = QueryBuilder::<<PostgresPool as Pool>::Db>::new(indoc! { r#"
                        WITH changes (account_id, usage_type, usage_key, delta, updated_at) AS (
                    "#});
                    query.push_values(changes, |mut row, (usage_type, usage_key, change)| {
                        row.push_bind(account_id)
                            .push_bind(usage_type)
                            .push_bind(usage_key)
                            .push_bind(change)
                            .push_bind(updated_at.clone());
                    });
                    query.push(indoc! { r#"
                    )
                    INSERT INTO account_usage_stats (
                        account_id,
                        usage_type,
                        usage_key,
                        value,
                        updated_at
                    )
                    SELECT
                        account_id,
                        usage_type,
                        usage_key,
                        CASE WHEN delta < 0 THEN 0 ELSE delta END,
                        updated_at
                    FROM changes
                    WHERE true
                    ON CONFLICT (account_id, usage_type, usage_key) DO UPDATE
                    SET
                        value = CASE
                            WHEN account_usage_stats.value + (
                                SELECT delta
                                FROM changes
                                WHERE changes.account_id = account_usage_stats.account_id
                                  AND changes.usage_type = account_usage_stats.usage_type
                                  AND changes.usage_key = account_usage_stats.usage_key
                            ) < 0
                                THEN 0
                            WHEN account_usage_stats.value + (
                                SELECT delta
                                FROM changes
                                WHERE changes.account_id = account_usage_stats.account_id
                                  AND changes.usage_type = account_usage_stats.usage_type
                                  AND changes.usage_key = account_usage_stats.usage_key
                            ) > 18446744073709551615
                                THEN 18446744073709551615
                            ELSE account_usage_stats.value + (
                                SELECT delta
                                FROM changes
                                WHERE changes.account_id = account_usage_stats.account_id
                                  AND changes.usage_type = account_usage_stats.usage_type
                                  AND changes.usage_key = account_usage_stats.usage_key
                            )
                        END,
                        updated_at = excluded.updated_at;
                    "#});

                    tx.execute(query.build()).await?;
                }

                if attributed_usage != (0, 0, 0, 0) || attributed_remainders != (0, 0, 0) {
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO account_monthly_usage_mode_attribution (
                                usage_update_id, account_id, revision, usage_key,
                                compute_fuel_delta, memory_gb_seconds_delta,
                                durable_storage_byte_seconds_delta,
                                ephemeral_storage_byte_seconds_delta,
                                memory_byte_nanoseconds_remainder,
                                durable_storage_byte_nanoseconds_remainder,
                                ephemeral_storage_byte_nanoseconds_remainder,
                                recorded_at
                            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                        "#})
                        .bind(Uuid::new_v4())
                        .bind(account_id)
                        .bind(NumericU64::new(attribution_revision))
                        .bind(&date_usage_key)
                        .bind(attributed_usage.0)
                        .bind(attributed_usage.1)
                        .bind(attributed_usage.2)
                        .bind(attributed_usage.3)
                        .bind(NumericU64::new(attributed_remainders.0))
                        .bind(NumericU64::new(attributed_remainders.1))
                        .bind(NumericU64::new(attributed_remainders.2))
                        .bind(&updated_at),
                    )
                    .await?;
                }

                if let Some(metering) = metering {
                    let usage_key = date_usage_key;
                    let mut query = QueryBuilder::<<PostgresPool as Pool>::Db>::new(indoc! { r#"
                        INSERT INTO account_usage_metering_state (
                            account_id,
                            usage_key,
                            compute_enabled,
                            memory_enabled,
                            filesystem_enabled,
                            updated_at
                        )
                    "#});
                    query.push_values([metering], |mut row, metering| {
                        row.push_bind(account_id)
                            .push_bind(&usage_key)
                            .push_bind(metering.compute)
                            .push_bind(metering.memory)
                            .push_bind(metering.filesystem)
                            .push_bind(updated_at.clone());
                    });
                    query.push(indoc! { r#"
                        ON CONFLICT (account_id, usage_key) DO UPDATE
                        SET
                            compute_enabled = excluded.compute_enabled,
                            memory_enabled = excluded.memory_enabled,
                            filesystem_enabled = excluded.filesystem_enabled,
                            updated_at = excluded.updated_at
                    "#});

                    tx.execute(query.build()).await?;
                }

                Ok(current_revision)
            }
            .boxed()
        })
        .await
    }
}

#[async_trait]
trait AccountUsageRepoInternal: AccountUsageRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn get_plan(
        &self,
        account_id: Uuid,
        active_at: &SqlDateTime,
    ) -> RepoResult<Option<AccountUsagePlan>>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountUsageRepoInternal for DbAccountUsageRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn get_plan(
        &self,
        account_id: Uuid,
        active_at: &SqlDateTime,
    ) -> RepoResult<Option<AccountUsagePlan>> {
        let plan: Option<AccountUsagePlan> = self
            .with_ro("get_plan - plan")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                SELECT
                    p.plan_id, p.name, p.max_memory_per_worker,
                    p.max_memory_per_worker_ceiling, p.max_memory_per_worker_user_configurable,
                    p.monthly_compute_gcu, p.monthly_memory_gb_seconds,
                    p.monthly_durable_storage_gb_month, p.monthly_ephemeral_storage_gb_month,
                    p.overage_eligible,
                    COALESCE(mode.mode, 'hard_limit') AS monthly_usage_mode,
                    COALESCE(mode.revision, 0) AS monthly_usage_mode_revision,
                    p.max_table_elements_per_worker,
                    p.max_disk_space_per_worker_enabled,
                    p.max_disk_space_per_worker,
                    storage_override.override_value AS storage_override_value,
                    memory_override.override_value AS max_memory_override_value,
                    compute_grant.override_value AS monthly_compute_grant_value,
                    compute_grant.reason AS monthly_compute_grant_reason,
                    compute_grant.expires_at AS monthly_compute_grant_expires_at,
                    compute_grant.created_by AS monthly_compute_grant_created_by,
                    compute_grant.created_at AS monthly_compute_grant_created_at,
                    monthly_memory_grant.override_value AS monthly_memory_grant_value,
                    monthly_memory_grant.reason AS monthly_memory_grant_reason,
                    monthly_memory_grant.expires_at AS monthly_memory_grant_expires_at,
                    monthly_memory_grant.created_by AS monthly_memory_grant_created_by,
                    monthly_memory_grant.created_at AS monthly_memory_grant_created_at,
                    durable_storage_grant.override_value AS monthly_durable_storage_grant_value,
                    durable_storage_grant.reason AS monthly_durable_storage_grant_reason,
                    durable_storage_grant.expires_at AS monthly_durable_storage_grant_expires_at,
                    durable_storage_grant.created_by AS monthly_durable_storage_grant_created_by,
                    durable_storage_grant.created_at AS monthly_durable_storage_grant_created_at,
                    ephemeral_storage_grant.override_value AS monthly_ephemeral_storage_grant_value,
                    ephemeral_storage_grant.reason AS monthly_ephemeral_storage_grant_reason,
                    ephemeral_storage_grant.expires_at AS monthly_ephemeral_storage_grant_expires_at,
                    ephemeral_storage_grant.created_by AS monthly_ephemeral_storage_grant_created_by,
                    ephemeral_storage_grant.created_at AS monthly_ephemeral_storage_grant_created_at,
                    memory_grant.override_value AS max_memory_grant_value,
                    memory_grant.reason AS max_memory_grant_reason,
                    memory_grant.expires_at AS max_memory_grant_expires_at,
                    memory_grant.created_by AS max_memory_grant_created_by,
                    memory_grant.created_at AS max_memory_grant_created_at,
                    storage_grant.override_value AS storage_grant_value,
                    storage_grant.reason AS storage_grant_reason,
                    storage_grant.expires_at AS storage_grant_expires_at,
                    storage_grant.created_by AS storage_grant_created_by,
                    storage_grant.created_at AS storage_grant_created_at,
                    p.max_disk_space_per_worker_ceiling, p.max_disk_space_per_worker_user_configurable,
                    p.max_concurrent_agents_per_executor,
                    p.total_app_count,
                    p.total_env_count, p.total_component_count, p.total_worker_connection_count,
                    p.total_component_storage_bytes, p.monthly_gas_limit, p.monthly_component_upload_limit_bytes,
                    p.per_invocation_http_call_limit, p.per_invocation_rpc_call_limit,
                    p.monthly_http_call_limit, p.monthly_rpc_call_limit,
                    p.oplog_writes_per_second
                FROM accounts a
                JOIN account_revisions ar ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
                JOIN plans p ON p.plan_id = ar.plan_id
                LEFT JOIN account_resource_overrides storage_override
                    ON storage_override.account_id = a.account_id
                    AND storage_override.dimension = $2
                    AND storage_override.source = $5
                    AND (storage_override.expires_at IS NULL OR storage_override.expires_at > $4)
                LEFT JOIN account_resource_overrides memory_override
                    ON memory_override.account_id = a.account_id
                    AND memory_override.dimension = $3
                    AND memory_override.source = $5
                    AND (memory_override.expires_at IS NULL OR memory_override.expires_at > $4)
                LEFT JOIN account_resource_overrides compute_grant
                    ON compute_grant.account_id = a.account_id
                    AND compute_grant.dimension = $7 AND compute_grant.source = $6
                    AND (compute_grant.expires_at IS NULL OR compute_grant.expires_at > $4)
                LEFT JOIN account_resource_overrides monthly_memory_grant
                    ON monthly_memory_grant.account_id = a.account_id
                    AND monthly_memory_grant.dimension = $8 AND monthly_memory_grant.source = $6
                    AND (monthly_memory_grant.expires_at IS NULL OR monthly_memory_grant.expires_at > $4)
                LEFT JOIN account_resource_overrides durable_storage_grant
                    ON durable_storage_grant.account_id = a.account_id
                    AND durable_storage_grant.dimension = $9 AND durable_storage_grant.source = $6
                    AND (durable_storage_grant.expires_at IS NULL OR durable_storage_grant.expires_at > $4)
                LEFT JOIN account_resource_overrides ephemeral_storage_grant
                    ON ephemeral_storage_grant.account_id = a.account_id
                    AND ephemeral_storage_grant.dimension = $10 AND ephemeral_storage_grant.source = $6
                    AND (ephemeral_storage_grant.expires_at IS NULL OR ephemeral_storage_grant.expires_at > $4)
                LEFT JOIN account_resource_overrides memory_grant
                    ON memory_grant.account_id = a.account_id
                    AND memory_grant.dimension = $11 AND memory_grant.source = $6
                    AND (memory_grant.expires_at IS NULL OR memory_grant.expires_at > $4)
                LEFT JOIN account_resource_overrides storage_grant
                    ON storage_grant.account_id = a.account_id
                    AND storage_grant.dimension = $12 AND storage_grant.source = $6
                    AND (storage_grant.expires_at IS NULL OR storage_grant.expires_at > $4)
                LEFT JOIN account_monthly_usage_modes mode
                    ON mode.account_id = a.account_id
                WHERE a.account_id = $1 AND a.deleted_at IS NULL
            "#})
                .bind(account_id)
                .bind(AccountResourceOverrideDimension::MaxDiskSpacePerWorker.as_str())
                .bind(AccountResourceOverrideDimension::MaxMemoryPerWorker.as_str())
                .bind(active_at)
                .bind(crate::repo::model::account_resource_override::AccountResourceOverrideSource::SelfService.as_str())
                .bind(crate::repo::model::account_resource_override::AccountResourceOverrideSource::AdminGrant.as_str())
                .bind(AccountResourceOverrideDimension::MonthlyComputeGcu.as_str())
                .bind(AccountResourceOverrideDimension::MonthlyMemoryGbSeconds.as_str())
                .bind(AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth.as_str())
                .bind(AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth.as_str())
                .bind(AccountResourceOverrideDimension::MaxMemoryPerWorker.as_str())
                .bind(AccountResourceOverrideDimension::MaxDiskSpacePerWorker.as_str()),
            )
            .await?;

        Ok(plan)
    }
}

static USAGE_KEY_TOTAL: &str = "total";

fn storage_limit(account_plan: &AccountUsagePlan) -> StorageLimit {
    let plan_default = account_plan.plan.max_disk_space_per_worker.get();
    let ceiling = account_plan.plan.max_disk_space_per_worker_ceiling.get();
    let override_value = account_plan
        .storage_override_value
        .as_ref()
        .map(NumericU64::get);
    let mut limit = StorageLimit::resolve(
        account_plan.plan.max_disk_space_per_worker_enabled,
        plan_default,
        override_value,
        ceiling,
        account_plan
            .plan
            .max_disk_space_per_worker_user_configurable,
    );
    if limit.enabled
        && let Some(value) = account_plan
            .storage_grant_value
            .as_ref()
            .map(NumericU64::get)
    {
        limit.effective_value = Some(value);
    }
    limit
}

fn max_memory_per_worker(account_plan: &AccountUsagePlan) -> MemoryLimit {
    let mut limit = MemoryLimit::resolve(
        account_plan.plan.max_memory_per_worker.get(),
        account_plan
            .max_memory_override_value
            .as_ref()
            .map(NumericU64::get),
        account_plan.plan.max_memory_per_worker_ceiling.get(),
        account_plan.plan.max_memory_per_worker_user_configurable,
    );
    if let Some(value) = account_plan
        .max_memory_grant_value
        .as_ref()
        .map(NumericU64::get)
    {
        limit.effective_value = value;
    }
    limit
}

fn date_to_usage_key(date: &SqlDateTime) -> String {
    year_and_month_to_usage_key(date.as_utc().year(), date.as_utc().month())
}

fn year_and_month_to_usage_key(year: i32, month: u32) -> String {
    format!("{:04}-{:02}", year, month)
}
