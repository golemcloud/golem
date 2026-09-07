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

use crate::repo::model::account_resource_override::{
    AccountResourceOverrideDbRecord, AccountResourceOverrideDimension,
    AccountResourceOverrideEventRecord, AccountResourceOverrideReason,
    AccountResourceOverrideRecord, AccountResourceOverrideSource, event_type_str,
    persisted_admin_reason, persisted_dimension,
};
use crate::repo::model::plan::PlanRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AdminResourceGrant, AdminResourceGrantChange, AdminResourceGrantEventType,
    AdminResourceGrantReason, EFFECTIVELY_UNLIMITED_STORAGE_LIMIT, MonthlyPlanAmounts,
};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{
    NumericU64, PoolLabelledTransaction, RepoError, RepoResult, SqlDateTime,
};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub struct OverridePolicy {
    pub dimension: AccountResourceOverrideDimension,
    pub enabled: bool,
    pub default: u64,
    pub ceiling: u64,
    pub user_configurable: bool,
}

impl OverridePolicy {
    fn storage(enabled: bool, default: u64, ceiling: u64, user_configurable: bool) -> Self {
        Self {
            dimension: AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            enabled,
            default,
            ceiling,
            user_configurable,
        }
    }

    fn memory(default: u64, ceiling: u64, user_configurable: bool) -> Self {
        Self {
            dimension: AccountResourceOverrideDimension::MaxMemoryPerWorker,
            enabled: true,
            default,
            ceiling,
            user_configurable,
        }
    }

    fn monthly(dimension: AccountResourceOverrideDimension, default: u64) -> Self {
        Self {
            dimension,
            enabled: true,
            default,
            ceiling: u64::MAX,
            user_configurable: false,
        }
    }

    pub(crate) fn for_plan(plan: &PlanRecord) -> [Self; 6] {
        [
            Self::monthly(
                AccountResourceOverrideDimension::MonthlyComputeGcu,
                plan.monthly_compute_gcu.get(),
            ),
            Self::monthly(
                AccountResourceOverrideDimension::MonthlyMemoryGbSeconds,
                plan.monthly_memory_gb_seconds.get(),
            ),
            Self::monthly(
                AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth,
                plan.monthly_durable_storage_gb_month.get(),
            ),
            Self::monthly(
                AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth,
                plan.monthly_ephemeral_storage_gb_month.get(),
            ),
            Self::storage(
                plan.max_disk_space_per_worker_enabled,
                plan.max_disk_space_per_worker.get(),
                plan.max_disk_space_per_worker_ceiling.get(),
                plan.max_disk_space_per_worker_user_configurable,
            ),
            Self::memory(
                plan.max_memory_per_worker.get(),
                plan.max_memory_per_worker_ceiling.get(),
                plan.max_memory_per_worker_user_configurable,
            ),
        ]
    }

    fn validate_user_value(&self, value: u64) -> Result<(), OverridePolicyViolation> {
        if !self.enabled {
            return Err(OverridePolicyViolation::FeatureDisabled);
        }
        if !self.user_configurable {
            return Err(OverridePolicyViolation::NotUserConfigurable);
        }
        if value < self.default {
            return Err(OverridePolicyViolation::BelowPlanDefault(self.default));
        }
        if value > self.ceiling {
            return Err(OverridePolicyViolation::ExceedsPlanCeiling(self.ceiling));
        }
        Ok(())
    }

    fn fallback_value(&self, user_value: Option<u64>) -> u64 {
        if !self.enabled
            && self.dimension == AccountResourceOverrideDimension::MaxDiskSpacePerWorker
        {
            return EFFECTIVELY_UNLIMITED_STORAGE_LIMIT;
        }
        user_value
            .filter(|_| self.enabled && self.user_configurable)
            .filter(|value| (self.default..=self.ceiling).contains(value))
            .unwrap_or(self.default)
    }
}

#[derive(sqlx::FromRow)]
struct OverridePoliciesRecord {
    monthly_compute_gcu: NumericU64,
    monthly_memory_gb_seconds: NumericU64,
    monthly_durable_storage_gb_month: NumericU64,
    monthly_ephemeral_storage_gb_month: NumericU64,
    max_memory_per_worker: NumericU64,
    max_memory_per_worker_ceiling: NumericU64,
    max_memory_per_worker_user_configurable: bool,
    max_disk_space_per_worker_enabled: bool,
    max_disk_space_per_worker: NumericU64,
    max_disk_space_per_worker_ceiling: NumericU64,
    max_disk_space_per_worker_user_configurable: bool,
}

impl OverridePoliciesRecord {
    fn into_policies(self) -> [OverridePolicy; 6] {
        [
            OverridePolicy::monthly(
                AccountResourceOverrideDimension::MonthlyComputeGcu,
                self.monthly_compute_gcu.get(),
            ),
            OverridePolicy::monthly(
                AccountResourceOverrideDimension::MonthlyMemoryGbSeconds,
                self.monthly_memory_gb_seconds.get(),
            ),
            OverridePolicy::monthly(
                AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth,
                self.monthly_durable_storage_gb_month.get(),
            ),
            OverridePolicy::monthly(
                AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth,
                self.monthly_ephemeral_storage_gb_month.get(),
            ),
            OverridePolicy::storage(
                self.max_disk_space_per_worker_enabled,
                self.max_disk_space_per_worker.get(),
                self.max_disk_space_per_worker_ceiling.get(),
                self.max_disk_space_per_worker_user_configurable,
            ),
            OverridePolicy::memory(
                self.max_memory_per_worker.get(),
                self.max_memory_per_worker_ceiling.get(),
                self.max_memory_per_worker_user_configurable,
            ),
        ]
    }

    fn grant_policy(&self, dimension: AccountResourceOverrideDimension) -> OverridePolicy {
        match dimension {
            AccountResourceOverrideDimension::MonthlyComputeGcu => {
                OverridePolicy::monthly(dimension, self.monthly_compute_gcu.get())
            }
            AccountResourceOverrideDimension::MonthlyMemoryGbSeconds => {
                OverridePolicy::monthly(dimension, self.monthly_memory_gb_seconds.get())
            }
            AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth => {
                OverridePolicy::monthly(dimension, self.monthly_durable_storage_gb_month.get())
            }
            AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth => {
                OverridePolicy::monthly(dimension, self.monthly_ephemeral_storage_gb_month.get())
            }
            AccountResourceOverrideDimension::MaxMemoryPerWorker => OverridePolicy::memory(
                self.max_memory_per_worker.get(),
                self.max_memory_per_worker_ceiling.get(),
                self.max_memory_per_worker_user_configurable,
            ),
            AccountResourceOverrideDimension::MaxDiskSpacePerWorker => OverridePolicy::storage(
                self.max_disk_space_per_worker_enabled,
                self.max_disk_space_per_worker.get(),
                self.max_disk_space_per_worker_ceiling.get(),
                self.max_disk_space_per_worker_user_configurable,
            ),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OverridePolicyViolation {
    #[error("feature is disabled")]
    FeatureDisabled,
    #[error("resource limit is not user configurable")]
    NotUserConfigurable,
    #[error("resource limit is below plan default {0}")]
    BelowPlanDefault(u64),
    #[error("resource limit exceeds plan ceiling {0}")]
    ExceedsPlanCeiling(u64),
}

#[derive(Debug, thiserror::Error)]
pub enum SetAccountResourceOverrideError {
    #[error("Account {0} not found")]
    AccountNotFound(Uuid),
    #[error(transparent)]
    Policy(#[from] OverridePolicyViolation),
    #[error(transparent)]
    Internal(#[from] RepoError),
}

#[derive(Debug, thiserror::Error)]
pub enum AdminResourceGrantRepoError {
    #[error("Account {0} not found")]
    AccountNotFound(Uuid),
    #[error(
        "Maximum storage per agent is disabled because managed filesystem quotas are unavailable"
    )]
    FeatureDisabled,
    #[error("Grant value {value} does not exceed the currently resolved value {current}")]
    DoesNotIncreaseResolvedValue { value: u64, current: u64 },
    #[error("Promotional grants require a future expiry")]
    PromotionalExpiryRequired,
    #[error("The grant value exceeds the supported internal range")]
    ValueOverflow,
    #[error("No active grant exists for this resource dimension")]
    GrantNotFound,
    #[error(transparent)]
    Internal(#[from] RepoError),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum OverrideReconciliationScope {
    Account(Uuid),
    Plan(Uuid),
}

#[async_trait]
pub trait AccountResourceOverrideRepo: Send + Sync {
    async fn get_active_value(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        now: &SqlDateTime,
    ) -> RepoResult<Option<NumericU64>>;

    async fn upsert(&self, record: AccountResourceOverrideRecord) -> RepoResult<()>;

    async fn set_user_override(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        value: u64,
        created_by: Uuid,
    ) -> Result<OverridePolicy, SetAccountResourceOverrideError>;

    async fn delete(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<()>;

    async fn set_admin_grant(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        value: u64,
        reason: AdminResourceGrantReason,
        expires_at: Option<SqlDateTime>,
        actor_account_id: Uuid,
    ) -> Result<AdminResourceGrantChange, AdminResourceGrantRepoError>;

    async fn clear_admin_grant(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        actor_account_id: Uuid,
    ) -> Result<AdminResourceGrantChange, AdminResourceGrantRepoError>;

    async fn get_active_admin_grants(
        &self,
        account_id: Uuid,
        now: &SqlDateTime,
    ) -> RepoResult<Vec<AdminResourceGrant>>;

    async fn get_admin_grant_events(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<AdminResourceGrantChange>>;

    async fn cleanup_expired_admin_grants(&self, now: SqlDateTime) -> RepoResult<u64>;

    async fn run_admin_grant_cleanup_loop(
        &self,
        interval: std::time::Duration,
    ) -> Result<(), anyhow::Error> {
        anyhow::ensure!(
            !interval.is_zero(),
            "Admin resource grant cleanup interval must be greater than zero"
        );
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            match self.cleanup_expired_admin_grants(SqlDateTime::now()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "Cleaned up expired admin resource grants")
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "Failed to clean up expired admin resource grants")
                }
            }
        }
    }
}

pub struct LoggedAccountResourceOverrideRepo<Repo: AccountResourceOverrideRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "account resource override repository";

impl<Repo: AccountResourceOverrideRepo> LoggedAccountResourceOverrideRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span(account_id: Uuid, dimension: AccountResourceOverrideDimension) -> Span {
        info_span!(SPAN_NAME, account_id = %account_id, dimension = dimension.as_str())
    }

    fn span_account(account_id: Uuid) -> Span {
        info_span!(SPAN_NAME, account_id = %account_id)
    }
}

#[async_trait]
impl<Repo: AccountResourceOverrideRepo> AccountResourceOverrideRepo
    for LoggedAccountResourceOverrideRepo<Repo>
{
    async fn get_active_value(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        now: &SqlDateTime,
    ) -> RepoResult<Option<NumericU64>> {
        self.repo
            .get_active_value(account_id, dimension, now)
            .instrument(Self::span(account_id, dimension))
            .await
    }

    async fn upsert(&self, record: AccountResourceOverrideRecord) -> RepoResult<()> {
        let span = Self::span(record.account_id, record.dimension);
        self.repo.upsert(record).instrument(span).await
    }

    async fn set_user_override(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        value: u64,
        created_by: Uuid,
    ) -> Result<OverridePolicy, SetAccountResourceOverrideError> {
        self.repo
            .set_user_override(account_id, dimension, value, created_by)
            .instrument(Self::span(account_id, dimension))
            .await
    }

    async fn delete(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<()> {
        self.repo
            .delete(account_id, dimension)
            .instrument(Self::span(account_id, dimension))
            .await
    }

    async fn set_admin_grant(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        value: u64,
        reason: AdminResourceGrantReason,
        expires_at: Option<SqlDateTime>,
        actor_account_id: Uuid,
    ) -> Result<AdminResourceGrantChange, AdminResourceGrantRepoError> {
        self.repo
            .set_admin_grant(
                account_id,
                dimension,
                value,
                reason,
                expires_at,
                actor_account_id,
            )
            .instrument(Self::span(account_id, dimension))
            .await
    }

    async fn clear_admin_grant(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        actor_account_id: Uuid,
    ) -> Result<AdminResourceGrantChange, AdminResourceGrantRepoError> {
        self.repo
            .clear_admin_grant(account_id, dimension, actor_account_id)
            .instrument(Self::span(account_id, dimension))
            .await
    }

    async fn get_active_admin_grants(
        &self,
        account_id: Uuid,
        now: &SqlDateTime,
    ) -> RepoResult<Vec<AdminResourceGrant>> {
        self.repo
            .get_active_admin_grants(account_id, now)
            .instrument(Self::span_account(account_id))
            .await
    }

    async fn get_admin_grant_events(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<AdminResourceGrantChange>> {
        self.repo
            .get_admin_grant_events(account_id)
            .instrument(Self::span_account(account_id))
            .await
    }

    async fn cleanup_expired_admin_grants(&self, now: SqlDateTime) -> RepoResult<u64> {
        self.repo.cleanup_expired_admin_grants(now).await
    }
}

pub struct DbAccountResourceOverrideRepo<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> DbAccountResourceOverrideRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedAccountResourceOverrideRepo<Self>
    where
        Self: AccountResourceOverrideRepo,
    {
        LoggedAccountResourceOverrideRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro("account_resource_override", api_name)
    }

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> RepoResult<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, RepoResult<R>>
            + Send,
    {
        self.db_pool
            .with_tx("account_resource_override", api_name, f)
            .await
    }
}

impl DbAccountResourceOverrideRepo<PostgresPool> {
    // Transactions that lock both record types acquire account rows before Plan rows.
    pub(crate) async fn lock_account_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
    ) -> RepoResult<Option<Uuid>> {
        let account: Option<(Uuid,)> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id
                    FROM accounts
                    WHERE account_id = $1
                      AND deleted_at IS NULL
                    FOR UPDATE
                "# })
                .bind(account_id),
            )
            .await?;
        if account.is_none() {
            return Ok(None);
        }
        let (plan_id,): (Uuid,) = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_revisions.plan_id
                    FROM accounts
                    JOIN account_revisions
                      ON account_revisions.account_id = accounts.account_id
                     AND account_revisions.revision_id = accounts.current_revision_id
                    WHERE accounts.account_id = $1
                "# })
                .bind(account_id),
            )
            .await?;
        Ok(Some(plan_id))
    }

    async fn lock_account_including_deleted_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
    ) -> RepoResult<Option<Uuid>> {
        let plan_id: Option<(Uuid,)> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_revisions.plan_id
                    FROM accounts
                    JOIN account_revisions
                      ON account_revisions.account_id = accounts.account_id
                     AND account_revisions.revision_id = accounts.current_revision_id
                    WHERE accounts.account_id = $1
                    FOR UPDATE OF accounts
                "# })
                .bind(account_id),
            )
            .await?;
        Ok(plan_id.map(|(plan_id,)| plan_id))
    }

    pub(crate) async fn lock_accounts_for_plan_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        plan_id: Uuid,
    ) -> RepoResult<Vec<Uuid>> {
        let accounts: Vec<(Uuid,)> = tx
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                SELECT accounts.account_id
                FROM accounts
                JOIN account_revisions
                  ON account_revisions.account_id = accounts.account_id
                 AND account_revisions.revision_id = accounts.current_revision_id
                WHERE account_revisions.plan_id = $1
                  AND accounts.deleted_at IS NULL
                ORDER BY accounts.account_id
                FOR UPDATE OF accounts
            "# })
                .bind(plan_id),
            )
            .await?;
        Ok(accounts
            .into_iter()
            .map(|(account_id,)| account_id)
            .collect())
    }

    pub(crate) async fn lock_plan_policies_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        plan_id: Uuid,
    ) -> RepoResult<Option<[OverridePolicy; 6]>> {
        let policies: Option<OverridePoliciesRecord> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        monthly_compute_gcu, monthly_memory_gb_seconds,
                        monthly_durable_storage_gb_month, monthly_ephemeral_storage_gb_month,
                        max_memory_per_worker,
                        max_memory_per_worker_ceiling, max_memory_per_worker_user_configurable,
                        max_disk_space_per_worker_enabled, max_disk_space_per_worker,
                        max_disk_space_per_worker_ceiling, max_disk_space_per_worker_user_configurable
                    FROM plans
                    WHERE plan_id = $1
                    FOR UPDATE
                "# })
                .bind(plan_id),
            )
            .await?;
        Ok(policies.map(OverridePoliciesRecord::into_policies))
    }
}

impl DbAccountResourceOverrideRepo<SqlitePool> {
    pub(crate) async fn lock_account_in_tx(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        account_id: Uuid,
    ) -> RepoResult<Option<Uuid>> {
        let account: Option<(Uuid,)> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id
                    FROM accounts
                    WHERE account_id = $1
                      AND deleted_at IS NULL
                "# })
                .bind(account_id),
            )
            .await?;
        if account.is_none() {
            return Ok(None);
        }

        let (plan_id,): (Uuid,) = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_revisions.plan_id
                    FROM accounts
                    JOIN account_revisions
                      ON account_revisions.account_id = accounts.account_id
                     AND account_revisions.revision_id = accounts.current_revision_id
                    WHERE accounts.account_id = $1
                "# })
                .bind(account_id),
            )
            .await?;
        Ok(Some(plan_id))
    }

    async fn lock_account_including_deleted_in_tx(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        account_id: Uuid,
    ) -> RepoResult<Option<Uuid>> {
        let plan_id: Option<(Uuid,)> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_revisions.plan_id
                    FROM accounts
                    JOIN account_revisions
                      ON account_revisions.account_id = accounts.account_id
                     AND account_revisions.revision_id = accounts.current_revision_id
                    WHERE accounts.account_id = $1
                "# })
                .bind(account_id),
            )
            .await?;
        Ok(plan_id.map(|(plan_id,)| plan_id))
    }

    pub(crate) async fn lock_accounts_for_plan_in_tx(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        plan_id: Uuid,
    ) -> RepoResult<Vec<Uuid>> {
        let accounts: Vec<(Uuid,)> = tx
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                SELECT accounts.account_id
                FROM accounts
                JOIN account_revisions
                  ON account_revisions.account_id = accounts.account_id
                 AND account_revisions.revision_id = accounts.current_revision_id
                WHERE account_revisions.plan_id = $1
                  AND accounts.deleted_at IS NULL
                ORDER BY accounts.account_id
            "# })
                .bind(plan_id),
            )
            .await?;
        Ok(accounts
            .into_iter()
            .map(|(account_id,)| account_id)
            .collect())
    }

    pub(crate) async fn lock_plan_policies_in_tx(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        plan_id: Uuid,
    ) -> RepoResult<Option<[OverridePolicy; 6]>> {
        let policies: Option<OverridePoliciesRecord> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        monthly_compute_gcu, monthly_memory_gb_seconds,
                        monthly_durable_storage_gb_month, monthly_ephemeral_storage_gb_month,
                        max_memory_per_worker,
                        max_memory_per_worker_ceiling, max_memory_per_worker_user_configurable,
                        max_disk_space_per_worker_enabled, max_disk_space_per_worker,
                        max_disk_space_per_worker_ceiling, max_disk_space_per_worker_user_configurable
                    FROM plans
                    WHERE plan_id = $1
                "# })
                .bind(plan_id),
            )
            .await?;
        Ok(policies.map(OverridePoliciesRecord::into_policies))
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbAccountResourceOverrideRepo<PostgresPool> {
    pub(crate) async fn account_ids_for_plan_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        plan_id: Uuid,
    ) -> RepoResult<Vec<Uuid>> {
        let accounts: Vec<(Uuid,)> = tx
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT accounts.account_id
                    FROM accounts
                    JOIN account_revisions
                      ON account_revisions.account_id = accounts.account_id
                     AND account_revisions.revision_id = accounts.current_revision_id
                    WHERE account_revisions.plan_id = $1
                      AND accounts.deleted_at IS NULL
                    ORDER BY accounts.account_id
                "# })
                .bind(plan_id),
            )
            .await?;
        Ok(accounts
            .into_iter()
            .map(|(account_id,)| account_id)
            .collect())
    }

    async fn upsert_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        record: AccountResourceOverrideRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO account_resource_overrides (
                    account_id, dimension, source, override_value, reason, expires_at, created_by, created_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (account_id, dimension, source) DO UPDATE SET
                    override_value = $4,
                    reason = $5,
                    expires_at = $6,
                    created_by = $7,
                    created_at = $8
            "# })
            .bind(record.account_id)
            .bind(record.dimension.as_str())
            .bind(record.source.as_str())
            .bind(record.override_value)
            .bind(record.reason.as_str())
            .bind(record.expires_at)
            .bind(record.created_by)
            .bind(record.created_at),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn reconcile_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        scope: OverrideReconciliationScope,
        policies: [OverridePolicy; 6],
    ) -> RepoResult<()> {
        let now = SqlDateTime::now();

        for policy in policies {
            match (scope, policy.enabled && policy.user_configurable) {
                (OverrideReconciliationScope::Account(account_id), true) => {
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE account_resource_overrides
                            SET override_value = CASE
                                  WHEN override_value < $1 THEN $1
                                  ELSE $2
                                END,
                                reason = $3
                            WHERE account_id = $4
                              AND dimension = $5
                              AND source = $6
                              AND (expires_at IS NULL OR expires_at > $7)
                              AND (override_value < $1 OR override_value > $2)
                        "# })
                        .bind(NumericU64::new(policy.default))
                        .bind(NumericU64::new(policy.ceiling))
                        .bind(AccountResourceOverrideReason::DowngradeClamp.as_str())
                        .bind(account_id)
                        .bind(policy.dimension.as_str())
                        .bind(AccountResourceOverrideSource::SelfService.as_str())
                        .bind(&now),
                    )
                    .await?;
                }
                (OverrideReconciliationScope::Plan(plan_id), true) => {
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE account_resource_overrides
                            SET override_value = CASE
                                  WHEN override_value < $1 THEN $1
                                  ELSE $2
                                END,
                                reason = $3
                            WHERE account_id IN (
                                SELECT accounts.account_id
                                FROM accounts
                                JOIN account_revisions
                                  ON account_revisions.account_id = accounts.account_id
                                 AND account_revisions.revision_id = accounts.current_revision_id
                                WHERE account_revisions.plan_id = $4
                                  AND accounts.deleted_at IS NULL
                            )
                              AND dimension = $5
                              AND source = $6
                              AND (expires_at IS NULL OR expires_at > $7)
                              AND (override_value < $1 OR override_value > $2)
                        "# })
                        .bind(NumericU64::new(policy.default))
                        .bind(NumericU64::new(policy.ceiling))
                        .bind(AccountResourceOverrideReason::DowngradeClamp.as_str())
                        .bind(plan_id)
                        .bind(policy.dimension.as_str())
                        .bind(AccountResourceOverrideSource::SelfService.as_str())
                        .bind(&now),
                    )
                    .await?;
                }
                (OverrideReconciliationScope::Account(account_id), false) => {
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            DELETE FROM account_resource_overrides
                            WHERE account_id = $1
                              AND dimension = $2
                              AND source = $3
                              AND (expires_at IS NULL OR expires_at > $4)
                        "# })
                        .bind(account_id)
                        .bind(policy.dimension.as_str())
                        .bind(AccountResourceOverrideSource::SelfService.as_str())
                        .bind(&now),
                    )
                    .await?;
                }
                (OverrideReconciliationScope::Plan(plan_id), false) => {
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            DELETE FROM account_resource_overrides
                            WHERE account_id IN (
                                SELECT accounts.account_id
                                FROM accounts
                                JOIN account_revisions
                                  ON account_revisions.account_id = accounts.account_id
                                 AND account_revisions.revision_id = accounts.current_revision_id
                                WHERE account_revisions.plan_id = $1
                                  AND accounts.deleted_at IS NULL
                            )
                              AND dimension = $2
                              AND source = $3
                              AND (expires_at IS NULL OR expires_at > $4)
                        "# })
                        .bind(plan_id)
                        .bind(policy.dimension.as_str())
                        .bind(AccountResourceOverrideSource::SelfService.as_str())
                        .bind(&now),
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }

    async fn grant_policy_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        plan_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<OverridePolicy> {
        let record: OverridePoliciesRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        monthly_compute_gcu, monthly_memory_gb_seconds,
                        monthly_durable_storage_gb_month, monthly_ephemeral_storage_gb_month,
                        max_memory_per_worker,
                        max_memory_per_worker_ceiling, max_memory_per_worker_user_configurable,
                        max_disk_space_per_worker_enabled, max_disk_space_per_worker,
                        max_disk_space_per_worker_ceiling, max_disk_space_per_worker_user_configurable
                    FROM plans
                    WHERE plan_id = $1
                "# })
                .bind(plan_id),
            )
            .await?;
        Ok(record.grant_policy(dimension))
    }

    async fn active_value_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        source: AccountResourceOverrideSource,
        now: &SqlDateTime,
    ) -> RepoResult<Option<u64>> {
        let value: Option<(NumericU64,)> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT override_value
                    FROM account_resource_overrides
                    WHERE account_id = $1 AND dimension = $2 AND source = $3
                      AND (expires_at IS NULL OR expires_at > $4)
                "# })
                .bind(account_id)
                .bind(dimension.as_str())
                .bind(source.as_str())
                .bind(now),
            )
            .await?;
        Ok(value.map(|(value,)| value.get()))
    }

    async fn active_admin_grant_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        now: &SqlDateTime,
    ) -> RepoResult<Option<AccountResourceOverrideDbRecord>> {
        tx.fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT account_id, dimension, override_value, reason, expires_at, created_by, created_at
                FROM account_resource_overrides
                WHERE account_id = $1 AND dimension = $2 AND source = $3
                  AND (expires_at IS NULL OR expires_at > $4)
            "# })
            .bind(account_id)
            .bind(dimension.as_str())
            .bind(AccountResourceOverrideSource::AdminGrant.as_str())
            .bind(now),
        )
        .await
    }

    async fn admin_grant_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<Option<AccountResourceOverrideDbRecord>> {
        tx.fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT account_id, dimension, override_value, reason, expires_at, created_by, created_at
                FROM account_resource_overrides
                WHERE account_id = $1 AND dimension = $2 AND source = $3
            "# })
            .bind(account_id)
            .bind(dimension.as_str())
            .bind(AccountResourceOverrideSource::AdminGrant.as_str()),
        )
        .await
    }

    async fn fallback_value_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        account_id: Uuid,
        policy: OverridePolicy,
        now: &SqlDateTime,
    ) -> RepoResult<u64> {
        let user_value = Self::active_value_in_tx(
            tx,
            account_id,
            policy.dimension,
            AccountResourceOverrideSource::SelfService,
            now,
        )
        .await?;
        Ok(policy.fallback_value(user_value))
    }

    async fn insert_admin_event_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        change: &AdminResourceGrantChange,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO account_resource_override_events (
                    event_id, account_id, dimension, event_type, reason, actor_account_id,
                    changed_at, old_value, new_value, expires_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "# })
            .bind(Uuid::new_v4())
            .bind(change.account_id.0)
            .bind(AccountResourceOverrideDimension::from(change.dimension).as_str())
            .bind(event_type_str(change.event_type))
            .bind(AccountResourceOverrideReason::from(change.reason).as_str())
            .bind(change.actor_account_id.0)
            .bind(SqlDateTime::new(change.changed_at))
            .bind(NumericU64::new(change.old_value))
            .bind(NumericU64::new(change.new_value))
            .bind(change.expires_at.map(SqlDateTime::new)),
        )
        .await?;
        Ok(())
    }

    fn expired_admin_grant_change(
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        grant: &AccountResourceOverrideDbRecord,
        fallback: u64,
        processed_at: &SqlDateTime,
    ) -> RepoResult<AdminResourceGrantChange> {
        let expires_at = grant.expires_at.clone().ok_or_else(|| {
            RepoError::InternalError(anyhow::anyhow!(
                "Expired admin resource grant has no expiry"
            ))
        })?;
        Ok(AdminResourceGrantChange {
            account_id: AccountId(account_id),
            dimension: dimension.into(),
            event_type: AdminResourceGrantEventType::OverrideExpired,
            reason: persisted_admin_reason(&grant.reason)?,
            actor_account_id: AccountId::SYSTEM,
            changed_at: processed_at.clone().into_utc(),
            old_value: grant.override_value.get(),
            new_value: fallback,
            expires_at: Some(expires_at.into_utc()),
        })
    }

    fn cleared_admin_grant_change(
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        grant: AccountResourceOverrideDbRecord,
        fallback: u64,
        actor_account_id: AccountId,
        changed_at: &SqlDateTime,
    ) -> RepoResult<AdminResourceGrantChange> {
        Ok(AdminResourceGrantChange {
            account_id: AccountId(account_id),
            dimension: dimension.into(),
            event_type: AdminResourceGrantEventType::OverrideCleared,
            reason: persisted_admin_reason(&grant.reason)?,
            actor_account_id,
            changed_at: changed_at.clone().into_utc(),
            old_value: grant.override_value.get(),
            new_value: fallback,
            expires_at: grant.expires_at.map(SqlDateTime::into_utc),
        })
    }

    fn validate_admin_grant(
        policy: OverridePolicy,
        value: u64,
        current: u64,
        reason: AdminResourceGrantReason,
        expires_at: Option<&SqlDateTime>,
        now: &SqlDateTime,
    ) -> Result<(), AdminResourceGrantRepoError> {
        if !policy.enabled {
            return Err(AdminResourceGrantRepoError::FeatureDisabled);
        }
        if policy.dimension == AccountResourceOverrideDimension::MaxDiskSpacePerWorker
            && value >= EFFECTIVELY_UNLIMITED_STORAGE_LIMIT
        {
            return Err(AdminResourceGrantRepoError::ValueOverflow);
        }
        if value <= current {
            return Err(AdminResourceGrantRepoError::DoesNotIncreaseResolvedValue {
                value,
                current,
            });
        }
        if reason == AdminResourceGrantReason::Promotional
            && expires_at.is_none_or(|expires_at| expires_at <= now)
        {
            return Err(AdminResourceGrantRepoError::PromotionalExpiryRequired);
        }
        let mut amounts = MonthlyPlanAmounts {
            compute_gcu: 0,
            memory_gb_seconds: 0,
            durable_storage_gb_month: 0,
            ephemeral_storage_gb_month: 0,
        };
        match policy.dimension {
            AccountResourceOverrideDimension::MonthlyComputeGcu => amounts.compute_gcu = value,
            AccountResourceOverrideDimension::MonthlyMemoryGbSeconds => {
                amounts.memory_gb_seconds = value
            }
            AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth => {
                amounts.durable_storage_gb_month = value
            }
            AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth => {
                amounts.ephemeral_storage_gb_month = value
            }
            AccountResourceOverrideDimension::MaxDiskSpacePerWorker
            | AccountResourceOverrideDimension::MaxMemoryPerWorker => return Ok(()),
        }
        amounts
            .resolve()
            .map_err(|_| AdminResourceGrantRepoError::ValueOverflow)?;
        Ok(())
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountResourceOverrideRepo for DbAccountResourceOverrideRepo<PostgresPool> {
    async fn get_active_value(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        now: &SqlDateTime,
    ) -> RepoResult<Option<NumericU64>> {
        let value: Option<(NumericU64,)> = self
            .with_ro("get_active_value")
            .fetch_optional_as(
                sqlx::query_as(
                    "SELECT override_value FROM account_resource_overrides WHERE account_id = $1 AND dimension = $2 AND source = $3 AND (expires_at IS NULL OR expires_at > $4)",
                )
                .bind(account_id)
                .bind(dimension.as_str())
                .bind(AccountResourceOverrideSource::SelfService.as_str())
                .bind(now),
            )
            .await?;
        Ok(value.map(|(value,)| value))
    }

    async fn upsert(&self, record: AccountResourceOverrideRecord) -> RepoResult<()> {
        self.with_tx("upsert", |tx| {
            async move { Self::upsert_in_tx(tx, record).await }.boxed()
        })
        .await
    }

    async fn set_user_override(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        value: u64,
        created_by: Uuid,
    ) -> Result<OverridePolicy, SetAccountResourceOverrideError> {
        self.db_pool
            .with_tx_err("account_resource_override", "set_user_override", |tx| {
                async move {
                    let plan_id = Self::lock_account_in_tx(tx, account_id)
                        .await?
                        .ok_or(SetAccountResourceOverrideError::AccountNotFound(account_id))?;
                    let policies = Self::lock_plan_policies_in_tx(tx, plan_id)
                        .await?
                        .ok_or_else(|| {
                            RepoError::InternalError(anyhow::anyhow!(
                                "Account {account_id} references missing plan {plan_id}"
                            ))
                        })?;
                    let policy = policies
                        .iter()
                        .copied()
                        .find(|policy| policy.dimension == dimension)
                        .expect("all self-service override dimensions have a policy");
                    policy.validate_user_value(value)?;
                    Self::upsert_in_tx(
                        tx,
                        AccountResourceOverrideRecord {
                            account_id,
                            dimension,
                            source: AccountResourceOverrideSource::SelfService,
                            override_value: value.into(),
                            reason: AccountResourceOverrideReason::UserSelfServe,
                            expires_at: None,
                            created_by,
                            created_at: SqlDateTime::now(),
                        },
                    )
                    .await?;
                    Self::reconcile_in_tx(
                        tx,
                        OverrideReconciliationScope::Account(account_id),
                        policies,
                    )
                    .await?;
                    Ok(policy)
                }
                .boxed()
            })
            .await
    }

    async fn delete(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<()> {
        self.with_tx("delete", |tx| {
            async move {
                let _ = Self::lock_account_including_deleted_in_tx(tx, account_id).await?;
                tx.execute(
                    sqlx::query("DELETE FROM account_resource_overrides WHERE account_id = $1 AND dimension = $2 AND source = $3")
                        .bind(account_id)
                        .bind(dimension.as_str())
                        .bind(AccountResourceOverrideSource::SelfService.as_str()),
                )
                .await?;
                Ok(())
            }
            .boxed()
        })
        .await
    }

    async fn set_admin_grant(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        value: u64,
        reason: AdminResourceGrantReason,
        expires_at: Option<SqlDateTime>,
        actor_account_id: Uuid,
    ) -> Result<AdminResourceGrantChange, AdminResourceGrantRepoError> {
        self.db_pool
            .with_tx_err("account_resource_override", "set_admin_grant", |tx| {
                async move {
                    let plan_id = Self::lock_account_in_tx(tx, account_id)
                        .await?
                        .ok_or(AdminResourceGrantRepoError::AccountNotFound(account_id))?;
                    Self::lock_plan_policies_in_tx(tx, plan_id)
                        .await?
                        .ok_or_else(|| {
                            RepoError::InternalError(anyhow::anyhow!(
                                "Account {account_id} references missing plan {plan_id}"
                            ))
                        })?;
                    let now = SqlDateTime::now();
                    let policy = Self::grant_policy_in_tx(tx, plan_id, dimension).await?;
                    let fallback = Self::fallback_value_in_tx(tx, account_id, policy, &now).await?;
                    let existing_grant = Self::admin_grant_in_tx(tx, account_id, dimension).await?;
                    let (old_value, expired_grant) = match existing_grant.as_ref() {
                        Some(grant)
                            if grant
                                .expires_at
                                .as_ref()
                                .is_some_and(|expires_at| expires_at <= &now) =>
                        {
                            (fallback, Some(grant))
                        }
                        Some(grant) => (grant.override_value.get(), None),
                        None => (fallback, None),
                    };
                    Self::validate_admin_grant(
                        policy,
                        value,
                        old_value,
                        reason,
                        expires_at.as_ref(),
                        &now,
                    )?;
                    if let Some(grant) = expired_grant {
                        let expired = Self::expired_admin_grant_change(
                            account_id, dimension, grant, fallback, &now,
                        )?;
                        Self::insert_admin_event_in_tx(tx, &expired).await?;
                    }
                    Self::upsert_in_tx(
                        tx,
                        AccountResourceOverrideRecord {
                            account_id,
                            dimension,
                            source: AccountResourceOverrideSource::AdminGrant,
                            override_value: value.into(),
                            reason: reason.into(),
                            expires_at: expires_at.clone(),
                            created_by: actor_account_id,
                            created_at: now.clone(),
                        },
                    )
                    .await?;
                    let change = AdminResourceGrantChange {
                        account_id: AccountId(account_id),
                        dimension: dimension.into(),
                        event_type: AdminResourceGrantEventType::OverrideGranted,
                        reason,
                        actor_account_id: AccountId(actor_account_id),
                        changed_at: now.into_utc(),
                        old_value,
                        new_value: value,
                        expires_at: expires_at.map(SqlDateTime::into_utc),
                    };
                    Self::insert_admin_event_in_tx(tx, &change).await?;
                    Ok(change)
                }
                .boxed()
            })
            .await
    }

    async fn clear_admin_grant(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        actor_account_id: Uuid,
    ) -> Result<AdminResourceGrantChange, AdminResourceGrantRepoError> {
        self.db_pool
            .with_tx_err("account_resource_override", "clear_admin_grant", |tx| {
                async move {
                    let plan_id = Self::lock_account_in_tx(tx, account_id)
                        .await?
                        .ok_or(AdminResourceGrantRepoError::AccountNotFound(account_id))?;
                    Self::lock_plan_policies_in_tx(tx, plan_id)
                        .await?
                        .ok_or_else(|| {
                            RepoError::InternalError(anyhow::anyhow!(
                                "Account {account_id} references missing plan {plan_id}"
                            ))
                        })?;
                    let now = SqlDateTime::now();
                    let policy = Self::grant_policy_in_tx(tx, plan_id, dimension).await?;
                    let fallback = Self::fallback_value_in_tx(tx, account_id, policy, &now).await?;
                    let grant = Self::active_admin_grant_in_tx(tx, account_id, dimension, &now)
                        .await?
                        .ok_or(AdminResourceGrantRepoError::GrantNotFound)?;
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            DELETE FROM account_resource_overrides
                            WHERE account_id = $1 AND dimension = $2 AND source = $3
                        "# })
                        .bind(account_id)
                        .bind(dimension.as_str())
                        .bind(AccountResourceOverrideSource::AdminGrant.as_str()),
                    )
                    .await?;
                    let change = Self::cleared_admin_grant_change(
                        account_id,
                        dimension,
                        grant,
                        fallback,
                        AccountId(actor_account_id),
                        &now,
                    )?;
                    Self::insert_admin_event_in_tx(tx, &change).await?;
                    Ok(change)
                }
                .boxed()
            })
            .await
    }

    async fn get_active_admin_grants(
        &self,
        account_id: Uuid,
        now: &SqlDateTime,
    ) -> RepoResult<Vec<AdminResourceGrant>> {
        let records: Vec<AccountResourceOverrideDbRecord> = self
            .with_ro("get_active_admin_grants")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id, dimension, override_value, reason, expires_at, created_by, created_at
                    FROM account_resource_overrides
                    WHERE account_id = $1 AND source = $2
                      AND (expires_at IS NULL OR expires_at > $3)
                    ORDER BY dimension
                "# })
                .bind(account_id)
                .bind(AccountResourceOverrideSource::AdminGrant.as_str())
                .bind(now),
            )
            .await?;
        records
            .into_iter()
            .map(AccountResourceOverrideDbRecord::into_admin_grant)
            .collect()
    }

    async fn get_admin_grant_events(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<AdminResourceGrantChange>> {
        let records: Vec<AccountResourceOverrideEventRecord> = self
            .with_ro("get_admin_grant_events")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id, dimension, event_type, reason, actor_account_id,
                           changed_at, old_value, new_value, expires_at
                    FROM account_resource_override_events
                    WHERE account_id = $1
                    ORDER BY changed_at, event_sequence
                "# })
                .bind(account_id),
            )
            .await?;
        records
            .into_iter()
            .map(AccountResourceOverrideEventRecord::into_public)
            .collect()
    }

    async fn cleanup_expired_admin_grants(&self, now: SqlDateTime) -> RepoResult<u64> {
        let candidates: Vec<(Uuid, String)> = self
            .with_ro("list_expired_admin_grants")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id, dimension
                    FROM account_resource_overrides
                    WHERE source = $1 AND expires_at IS NOT NULL AND expires_at <= $2
                    ORDER BY account_id, dimension
                "# })
                .bind(AccountResourceOverrideSource::AdminGrant.as_str())
                .bind(&now),
            )
            .await?;
        let mut cleaned = 0;
        for (account_id, persisted_dimension_value) in candidates {
            let now = now.clone();
            let removed = self
                .with_tx("cleanup_expired_admin_grant", |tx| {
                    async move {
                        let dimension = persisted_dimension(&persisted_dimension_value)?;
                        let Some(plan_id) =
                            Self::lock_account_including_deleted_in_tx(tx, account_id).await?
                        else {
                            return Ok(false);
                        };
                        Self::lock_plan_policies_in_tx(tx, plan_id).await?;
                        let grant: Option<AccountResourceOverrideDbRecord> = tx
                            .fetch_optional_as(
                                sqlx::query_as(indoc! { r#"
                                SELECT account_id, dimension, override_value, reason, expires_at,
                                       created_by, created_at
                                FROM account_resource_overrides
                                WHERE account_id = $1 AND dimension = $2 AND source = $3
                                  AND expires_at IS NOT NULL AND expires_at <= $4
                            "# })
                                .bind(account_id)
                                .bind(dimension.as_str())
                                .bind(AccountResourceOverrideSource::AdminGrant.as_str())
                                .bind(&now),
                            )
                            .await?;
                        let Some(grant) = grant else {
                            return Ok(false);
                        };
                        let policy = Self::grant_policy_in_tx(tx, plan_id, dimension).await?;
                        let fallback =
                            Self::fallback_value_in_tx(tx, account_id, policy, &now).await?;
                        tx.execute(
                            sqlx::query(indoc! { r#"
                            DELETE FROM account_resource_overrides
                            WHERE account_id = $1 AND dimension = $2 AND source = $3
                              AND expires_at IS NOT NULL AND expires_at <= $4
                        "# })
                            .bind(account_id)
                            .bind(dimension.as_str())
                            .bind(AccountResourceOverrideSource::AdminGrant.as_str())
                            .bind(&now),
                        )
                        .await?;
                        let change = Self::expired_admin_grant_change(
                            account_id, dimension, &grant, fallback, &now,
                        )?;
                        Self::insert_admin_event_in_tx(tx, &change).await?;
                        Ok(true)
                    }
                    .boxed()
                })
                .await?;
            cleaned += u64::from(removed);
        }
        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn policy(enabled: bool, user_configurable: bool) -> OverridePolicy {
        OverridePolicy {
            dimension: AccountResourceOverrideDimension::MaxMemoryPerWorker,
            enabled,
            default: 10,
            ceiling: 20,
            user_configurable,
        }
    }

    #[test]
    fn user_override_policy_accepts_inclusive_bounds() {
        assert!(policy(true, true).validate_user_value(10).is_ok());
        assert!(policy(true, true).validate_user_value(20).is_ok());
    }

    #[test]
    fn user_override_policy_rejects_disabled_nonconfigurable_and_out_of_range_values() {
        assert!(matches!(
            policy(false, true).validate_user_value(10),
            Err(OverridePolicyViolation::FeatureDisabled)
        ));
        assert!(matches!(
            policy(true, false).validate_user_value(10),
            Err(OverridePolicyViolation::NotUserConfigurable)
        ));
        assert!(matches!(
            policy(true, true).validate_user_value(9),
            Err(OverridePolicyViolation::BelowPlanDefault(10))
        ));
        assert!(matches!(
            policy(true, true).validate_user_value(21),
            Err(OverridePolicyViolation::ExceedsPlanCeiling(20))
        ));
    }

    #[test]
    fn disabled_storage_fallback_is_effectively_unlimited() {
        assert_eq!(
            OverridePolicy::storage(false, 10, 20, true).fallback_value(Some(15)),
            EFFECTIVELY_UNLIMITED_STORAGE_LIMIT
        );
    }

    #[test]
    fn admin_grant_policy_rejects_disabled_non_increasing_and_invalid_promotion() {
        let now = SqlDateTime::new(chrono::Utc::now());
        assert!(matches!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(false, true),
                20,
                10,
                AdminResourceGrantReason::Support,
                None,
                &now,
            ),
            Err(AdminResourceGrantRepoError::FeatureDisabled)
        ));
        assert!(matches!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(true, true),
                9,
                10,
                AdminResourceGrantReason::Support,
                None,
                &now,
            ),
            Err(AdminResourceGrantRepoError::DoesNotIncreaseResolvedValue {
                value: 9,
                current: 10
            })
        ));
        assert!(matches!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(true, true),
                10,
                10,
                AdminResourceGrantReason::Support,
                None,
                &now,
            ),
            Err(AdminResourceGrantRepoError::DoesNotIncreaseResolvedValue {
                value: 10,
                current: 10
            })
        ));
        assert!(matches!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(true, true),
                20,
                10,
                AdminResourceGrantReason::Promotional,
                None,
                &now,
            ),
            Err(AdminResourceGrantRepoError::PromotionalExpiryRequired)
        ));
        assert!(matches!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(true, true),
                20,
                10,
                AdminResourceGrantReason::Promotional,
                Some(&now),
                &now,
            ),
            Err(AdminResourceGrantRepoError::PromotionalExpiryRequired)
        ));
    }

    #[test]
    fn admin_grant_policy_accepts_valid_grants_and_rejects_conversion_overflow() {
        let now = SqlDateTime::new(chrono::Utc::now());
        let future = SqlDateTime::new(chrono::Utc::now() + chrono::Duration::hours(1));
        assert!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(true, true),
                20,
                10,
                AdminResourceGrantReason::Support,
                None,
                &now,
            )
            .is_ok()
        );
        assert!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(true, true),
                20,
                10,
                AdminResourceGrantReason::Promotional,
                Some(&future),
                &now,
            )
            .is_ok()
        );
        assert!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                policy(true, true),
                EFFECTIVELY_UNLIMITED_STORAGE_LIMIT,
                10,
                AdminResourceGrantReason::Support,
                None,
                &now,
            )
            .is_ok()
        );
        assert!(matches!(
            DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                OverridePolicy::monthly(AccountResourceOverrideDimension::MonthlyComputeGcu, 10,),
                u64::MAX,
                10,
                AdminResourceGrantReason::Support,
                None,
                &now,
            ),
            Err(AdminResourceGrantRepoError::ValueOverflow)
        ));
    }

    #[test]
    fn admin_grant_policy_rejects_unlimited_storage_sentinel_and_above() {
        let now = SqlDateTime::new(chrono::Utc::now());
        let storage_policy = OverridePolicy {
            dimension: AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            enabled: true,
            default: 10,
            ceiling: u64::MAX,
            user_configurable: true,
        };

        for value in [
            EFFECTIVELY_UNLIMITED_STORAGE_LIMIT,
            EFFECTIVELY_UNLIMITED_STORAGE_LIMIT + 1,
        ] {
            assert!(matches!(
                DbAccountResourceOverrideRepo::<SqlitePool>::validate_admin_grant(
                    storage_policy,
                    value,
                    EFFECTIVELY_UNLIMITED_STORAGE_LIMIT + 2,
                    AdminResourceGrantReason::Support,
                    None,
                    &now,
                ),
                Err(AdminResourceGrantRepoError::ValueOverflow)
            ));
        }
    }
}
