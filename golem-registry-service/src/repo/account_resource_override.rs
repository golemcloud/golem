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
    AccountResourceOverrideDimension, AccountResourceOverrideReason, AccountResourceOverrideRecord,
};
use crate::repo::model::plan::PlanRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
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

    pub(crate) fn for_plan(plan: &PlanRecord) -> [Self; 2] {
        [
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
}

#[derive(sqlx::FromRow)]
struct OverridePoliciesRecord {
    max_memory_per_worker: NumericU64,
    max_memory_per_worker_ceiling: NumericU64,
    max_memory_per_worker_user_configurable: bool,
    max_disk_space_per_worker_enabled: bool,
    max_disk_space_per_worker: NumericU64,
    max_disk_space_per_worker_ceiling: NumericU64,
    max_disk_space_per_worker_user_configurable: bool,
}

impl OverridePoliciesRecord {
    fn into_policies(self) -> [OverridePolicy; 2] {
        [
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
    ) -> RepoResult<Option<[OverridePolicy; 2]>> {
        let policies: Option<OverridePoliciesRecord> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
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
    ) -> RepoResult<Option<[OverridePolicy; 2]>> {
        let policies: Option<OverridePoliciesRecord> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
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
    async fn upsert_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        record: AccountResourceOverrideRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO account_resource_overrides (
                    account_id, dimension, override_value, reason, expires_at, created_by, created_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (account_id, dimension) DO UPDATE SET
                    override_value = $3,
                    reason = $4,
                    expires_at = $5,
                    created_by = $6,
                    created_at = $7
            "# })
            .bind(record.account_id)
            .bind(record.dimension.as_str())
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
        policies: [OverridePolicy; 2],
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
                              AND (expires_at IS NULL OR expires_at > $6)
                              AND (override_value < $1 OR override_value > $2)
                        "# })
                        .bind(NumericU64::new(policy.default))
                        .bind(NumericU64::new(policy.ceiling))
                        .bind(AccountResourceOverrideReason::DowngradeClamp.as_str())
                        .bind(account_id)
                        .bind(policy.dimension.as_str())
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
                              AND (expires_at IS NULL OR expires_at > $6)
                              AND (override_value < $1 OR override_value > $2)
                        "# })
                        .bind(NumericU64::new(policy.default))
                        .bind(NumericU64::new(policy.ceiling))
                        .bind(AccountResourceOverrideReason::DowngradeClamp.as_str())
                        .bind(plan_id)
                        .bind(policy.dimension.as_str())
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
                              AND (expires_at IS NULL OR expires_at > $3)
                        "# })
                        .bind(account_id)
                        .bind(policy.dimension.as_str())
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
                              AND (expires_at IS NULL OR expires_at > $3)
                        "# })
                        .bind(plan_id)
                        .bind(policy.dimension.as_str())
                        .bind(&now),
                    )
                    .await?;
                }
            }
        }

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
                    "SELECT override_value FROM account_resource_overrides WHERE account_id = $1 AND dimension = $2 AND (expires_at IS NULL OR expires_at > $3)",
                )
                .bind(account_id)
                .bind(dimension.as_str())
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
                        .into_iter()
                        .find(|policy| policy.dimension == dimension)
                        .expect("all self-service override dimensions have a policy");
                    policy.validate_user_value(value)?;
                    Self::upsert_in_tx(
                        tx,
                        AccountResourceOverrideRecord {
                            account_id,
                            dimension,
                            override_value: value.into(),
                            reason: AccountResourceOverrideReason::UserSelfServe,
                            expires_at: None,
                            created_by,
                            created_at: SqlDateTime::now(),
                        },
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
                tx.execute(
                    sqlx::query("DELETE FROM account_resource_overrides WHERE account_id = $1 AND dimension = $2")
                        .bind(account_id)
                        .bind(dimension.as_str()),
                )
                .await?;
                Ok(())
            }
            .boxed()
        })
        .await
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
}
