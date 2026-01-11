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

pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::account_usage::{AccountUsage, UsageGrouping, UsageTracking, UsageType};
use crate::repo::model::datetime::SqlDateTime;
use crate::repo::model::plan::PlanRecord;
use async_trait::async_trait;
use chrono::Datelike;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool};
use golem_service_base::repo::RepoResult;
use golem_service_base::repo::numeric::NumericU64;
use indoc::indoc;
use sqlx::{Database, Row};
use std::collections::BTreeMap;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait AccountUsageRepo: Send + Sync {
    async fn get(&self, account_id: Uuid, date: &SqlDateTime) -> RepoResult<Option<AccountUsage>>;

    async fn get_for_type(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        usage_type: UsageType,
    ) -> RepoResult<Option<AccountUsage>>;

    async fn add(&self, account_usage: &AccountUsage) -> RepoResult<()>;
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

    async fn add(&self, account_usage: &AccountUsage) -> RepoResult<()> {
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
#[async_trait]
impl AccountUsageRepo for DbAccountUsageRepo<PostgresPool> {
    async fn get(&self, account_id: Uuid, date: &SqlDateTime) -> RepoResult<Option<AccountUsage>> {
        let Some(plan) = self.get_plan(account_id).await? else {
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
                        value
                    FROM
                        account_usage_stats
                    WHERE
                        account_id = $1
                        AND usage_key IN ($2, $3)
                    UNION ALL SELECT $4 AS usage_type, total_apps AS value FROM counts
                    UNION ALL SELECT $5 AS usage_type, total_envs AS value FROM counts
                    UNION ALL SELECT $6 AS usage_type, total_components AS value FROM counts
                    UNION ALL SELECT $7 AS usage_type, total_component_size AS value FROM counts;
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
        for row in usage_rows {
            usage.insert(
                row.try_get("usage_type")?,
                row.try_get::<NumericU64, _>("value")?.get(),
            );
        }

        Ok(Some(AccountUsage {
            account_id,
            year: date.as_utc().year(),
            month: date.as_utc().month(),
            usage,
            plan,
            changes: Default::default(),
        }))
    }

    async fn get_for_type(
        &self,
        account_id: Uuid,
        date: &SqlDateTime,
        usage_type: UsageType,
    ) -> RepoResult<Option<AccountUsage>> {
        let Some(plan) = self.get_plan(account_id).await? else {
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

        Ok(Some(AccountUsage {
            account_id,
            year: date.as_utc().year(),
            month: date.as_utc().month(),
            usage,
            plan,
            changes: Default::default(),
        }))
    }

    async fn add(&self, account_usage: &AccountUsage) -> RepoResult<()> {
        let account_id = account_usage.account_id;
        let changes = account_usage.changes.clone();
        let date_usage_key = year_and_month_to_usage_key(account_usage.year, account_usage.month);

        self.with_tx("change_usage", |tx| {
            async move {
                for (usage_type, change) in changes {
                    if usage_type.tracking() != UsageTracking::Stats || change == 0 {
                        continue;
                    }

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO account_usage_stats (
                                account_id,
                                usage_type,
                                usage_key,
                                value,
                                updated_at
                            )
                            VALUES ($1, $2, $3, $4, $5)
                            ON CONFLICT (account_id, usage_type, usage_key) DO UPDATE
                            SET
                                value = CASE
                                    WHEN account_usage_stats.value + $6 < 0
                                        THEN 0
                                    WHEN account_usage_stats.value + $6 > 18446744073709551615
                                        THEN 18446744073709551615
                                    ELSE account_usage_stats.value + $6
                                END,
                                updated_at = $5;
                        "#})
                        .bind(account_id)
                        .bind(usage_type)
                        .bind(match usage_type.grouping() {
                            UsageGrouping::Total => USAGE_KEY_TOTAL,
                            UsageGrouping::Monthly => &date_usage_key,
                        })
                        .bind(if change < 0 {
                            0.into()
                        } else {
                            NumericU64::new(change as u64)
                        })
                        .bind(SqlDateTime::now())
                        .bind(change),
                    )
                    .await?;
                }

                Ok(())
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

    async fn get_plan(&self, account_id: Uuid) -> RepoResult<Option<PlanRecord>>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountUsageRepoInternal for DbAccountUsageRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn get_plan(&self, account_id: Uuid) -> RepoResult<Option<PlanRecord>> {
        let plan: Option<PlanRecord> = self
            .with_ro("get_plan - plan")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                SELECT
                    p.plan_id, p.name, p.max_memory_per_worker, p.total_app_count,
                    p.total_env_count, p.total_component_count, p.total_worker_count, p.total_worker_connection_count,
                    p.total_component_storage_bytes, p.monthly_gas_limit, p.monthly_component_upload_limit_bytes
                FROM accounts a
                JOIN account_revisions ar ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
                JOIN plans p ON p.plan_id = ar.plan_id
                WHERE a.account_id = $1 AND a.deleted_at IS NULL
            "#})
                .bind(account_id),
            )
            .await?;

        Ok(plan)
    }
}

static USAGE_KEY_TOTAL: &str = "total";

fn date_to_usage_key(date: &SqlDateTime) -> String {
    year_and_month_to_usage_key(date.as_utc().year(), date.as_utc().month())
}

fn year_and_month_to_usage_key(year: i32, month: u32) -> String {
    format!("{:04}-{:02}", year, month)
}
