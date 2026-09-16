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

use crate::repo::account_resource_override::{
    DbAccountResourceOverrideRepo, OverridePolicy, OverrideReconciliationScope,
};
use crate::repo::account_usage::DbAccountUsageRepo;
use crate::repo::model::plan::PlanRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::model::account_usage::MonthlyUsageModeTransitionSource;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait PlanRepo: Send + Sync {
    /// Upserts the Plan Row and reconciles active resource overrides and grants for assigned
    /// accounts in the same transaction.
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()>;

    async fn get_by_id(&self, plan_id: Uuid) -> RepoResult<Option<PlanRecord>>;

    async fn list(&self) -> RepoResult<Vec<PlanRecord>>;
}

pub struct LoggedPlanRepo<Repo: PlanRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "plan repository";

impl<Repo: PlanRepo> LoggedPlanRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    pub fn span_id(plan_id: Uuid) -> Span {
        info_span!(SPAN_NAME, plan_id=%plan_id)
    }
}

#[async_trait]
impl<Repo: PlanRepo> PlanRepo for LoggedPlanRepo<Repo> {
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()> {
        let span = Self::span_id(plan.plan_id);
        self.repo.create_or_update(plan).instrument(span).await
    }

    async fn get_by_id(&self, plan_id: Uuid) -> RepoResult<Option<PlanRecord>> {
        self.repo
            .get_by_id(plan_id)
            .instrument(Self::span_id(plan_id))
            .await
    }

    async fn list(&self) -> RepoResult<Vec<PlanRecord>> {
        self.repo.list().await
    }
}

pub struct DbPlanRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "plan";
const MAX_PLAN_UPDATE_ATTEMPTS: usize = 8;

#[derive(Debug)]
enum PlanUpdateAttemptError {
    MembershipChanged,
    Repo(RepoError),
}

impl From<RepoError> for PlanUpdateAttemptError {
    fn from(error: RepoError) -> Self {
        Self::Repo(error)
    }
}

impl<DBP: Pool> DbPlanRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedPlanRepo<Self>
    where
        Self: PlanRepo,
    {
        LoggedPlanRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl PlanRepo for DbPlanRepo<PostgresPool> {
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()> {
        for attempt in 1..=MAX_PLAN_UPDATE_ATTEMPTS {
            let plan = plan.clone();
            let result = self
                .db_pool
                .with_tx_err(METRICS_SVC_NAME, "create_or_update", |tx| {
                    async move {
                let plan_id = plan.plan_id;
                // Existing members are locked before the Plan row to match account-side writes.
                let locked_account_ids =
                    DbAccountResourceOverrideRepo::<PostgresPool>::lock_accounts_for_plan_in_tx(
                        tx, plan_id,
                    )
                    .await?;
                let override_policies = OverridePolicy::for_plan(&plan);
                let overage_eligible = plan.overage_eligible;
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO plans (
                            plan_id, name, max_memory_per_worker,
                            max_memory_per_worker_ceiling, max_memory_per_worker_user_configurable,
                            monthly_compute_gcu, monthly_memory_gb_seconds,
                            monthly_durable_storage_gb_month, monthly_ephemeral_storage_gb_month,
                            overage_eligible,
                            max_table_elements_per_worker, max_disk_space_per_worker_enabled, max_disk_space_per_worker,
                            max_disk_space_per_worker_ceiling, max_disk_space_per_worker_user_configurable,
                            max_concurrent_agents_per_executor,
                            total_app_count, total_env_count, total_component_count,
                            total_worker_connection_count, total_component_storage_bytes,
                            monthly_gas_limit, monthly_component_upload_limit_bytes,
                            per_invocation_http_call_limit, per_invocation_rpc_call_limit,
                            monthly_http_call_limit, monthly_rpc_call_limit,
                            oplog_writes_per_second
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)
                        ON CONFLICT (plan_id) DO UPDATE SET
                            name = $2,
                            max_memory_per_worker = $3,
                            max_memory_per_worker_ceiling = $4,
                            max_memory_per_worker_user_configurable = $5,
                            monthly_compute_gcu = $6,
                            monthly_memory_gb_seconds = $7,
                            monthly_durable_storage_gb_month = $8,
                            monthly_ephemeral_storage_gb_month = $9,
                            overage_eligible = $10,
                            max_table_elements_per_worker = $11,
                            max_disk_space_per_worker_enabled = $12,
                            max_disk_space_per_worker = $13,
                            max_disk_space_per_worker_ceiling = $14,
                            max_disk_space_per_worker_user_configurable = $15,
                            max_concurrent_agents_per_executor = $16,
                            total_app_count = $17,
                            total_env_count = $18,
                            total_component_count = $19,
                            total_worker_connection_count = $20,
                            total_component_storage_bytes = $21,
                            monthly_gas_limit = $22,
                            monthly_component_upload_limit_bytes = $23,
                            per_invocation_http_call_limit = $24,
                            per_invocation_rpc_call_limit = $25,
                            monthly_http_call_limit = $26,
                            monthly_rpc_call_limit = $27,
                            oplog_writes_per_second = $28
                    "#})
                    .bind(plan.plan_id)
                    .bind(plan.name)
                    .bind(plan.max_memory_per_worker)
                    .bind(plan.max_memory_per_worker_ceiling)
                    .bind(plan.max_memory_per_worker_user_configurable)
                    .bind(plan.monthly_compute_gcu)
                    .bind(plan.monthly_memory_gb_seconds)
                    .bind(plan.monthly_durable_storage_gb_month)
                    .bind(plan.monthly_ephemeral_storage_gb_month)
                    .bind(plan.overage_eligible)
                    .bind(plan.max_table_elements_per_worker)
                    .bind(plan.max_disk_space_per_worker_enabled)
                    .bind(plan.max_disk_space_per_worker)
                    .bind(plan.max_disk_space_per_worker_ceiling)
                    .bind(plan.max_disk_space_per_worker_user_configurable)
                    .bind(plan.max_concurrent_agents_per_executor)
                    .bind(plan.total_app_count)
                    .bind(plan.total_env_count)
                    .bind(plan.total_component_count)
                    .bind(plan.total_worker_connection_count)
                    .bind(plan.total_component_storage_bytes)
                    .bind(plan.monthly_gas_limit)
                    .bind(plan.monthly_component_upload_limit_bytes)
                    .bind(plan.per_invocation_http_call_limit)
                    .bind(plan.per_invocation_rpc_call_limit)
                    .bind(plan.monthly_http_call_limit)
                    .bind(plan.monthly_rpc_call_limit)
                    .bind(plan.oplog_writes_per_second)
                )
                .await?;

                // Retry if an assignment committed while this transaction waited for the Plan row.
                // Reconciliation must not touch an account that was not locked before the Plan.
                let current_account_ids =
                    DbAccountResourceOverrideRepo::<PostgresPool>::account_ids_for_plan_in_tx(
                        tx, plan_id,
                    )
                    .await?;
                if current_account_ids != locked_account_ids {
                    return Err(PlanUpdateAttemptError::MembershipChanged);
                }

                if !overage_eligible {
                    for account_id in locked_account_ids {
                        DbAccountUsageRepo::<PostgresPool>::force_hard_limit_in_tx(
                            tx,
                            account_id,
                            golem_common::model::account::AccountId::SYSTEM.0,
                            MonthlyUsageModeTransitionSource::PlanEligibilityRemoved,
                        )
                        .await?;
                    }
                }

                DbAccountResourceOverrideRepo::<PostgresPool>::reconcile_in_tx(
                    tx,
                    OverrideReconciliationScope::Plan(plan_id),
                    override_policies,
                )
                .await?;

                Ok(())
            }
            .boxed()
                })
                .await;

            match result {
                Ok(()) => return Ok(()),
                Err(PlanUpdateAttemptError::MembershipChanged)
                    if attempt < MAX_PLAN_UPDATE_ATTEMPTS =>
                {
                    continue;
                }
                Err(PlanUpdateAttemptError::MembershipChanged) => {
                    return Err(RepoError::InternalError(anyhow::anyhow!(
                        "Plan membership kept changing during {MAX_PLAN_UPDATE_ATTEMPTS} update attempts"
                    )));
                }
                Err(PlanUpdateAttemptError::Repo(error)) => return Err(error),
            }
        }
        unreachable!("the bounded Plan update loop always returns")
    }

    async fn get_by_id(&self, plan_id: Uuid) -> RepoResult<Option<PlanRecord>> {
        let plan: Option<PlanRecord> = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        plan_id, name, max_memory_per_worker,
                        max_memory_per_worker_ceiling, max_memory_per_worker_user_configurable,
                        monthly_compute_gcu, monthly_memory_gb_seconds,
                        monthly_durable_storage_gb_month, monthly_ephemeral_storage_gb_month,
                        overage_eligible,
                        max_table_elements_per_worker, max_disk_space_per_worker_enabled, max_disk_space_per_worker,
                        max_disk_space_per_worker_ceiling, max_disk_space_per_worker_user_configurable,
                        max_concurrent_agents_per_executor,
                        total_app_count, total_env_count, total_component_count,
                        total_worker_connection_count, total_component_storage_bytes,
                        monthly_gas_limit, monthly_component_upload_limit_bytes,
                        per_invocation_http_call_limit, per_invocation_rpc_call_limit,
                        monthly_http_call_limit, monthly_rpc_call_limit,
                        oplog_writes_per_second
                    FROM plans
                    WHERE plan_id = $1
                "# })
                .bind(plan_id),
            )
            .await?;

        match plan {
            Some(plan) => Ok(Some(plan)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> RepoResult<Vec<PlanRecord>> {
        let plans = self
            .with_ro("list")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                SELECT
                    plan_id, name, max_memory_per_worker,
                    max_memory_per_worker_ceiling, max_memory_per_worker_user_configurable,
                    monthly_compute_gcu, monthly_memory_gb_seconds,
                    monthly_durable_storage_gb_month, monthly_ephemeral_storage_gb_month,
                    overage_eligible,
                    max_table_elements_per_worker, max_disk_space_per_worker_enabled, max_disk_space_per_worker,
                    max_disk_space_per_worker_ceiling, max_disk_space_per_worker_user_configurable,
                    max_concurrent_agents_per_executor,
                    total_app_count, total_env_count, total_component_count,
                    total_worker_connection_count, total_component_storage_bytes,
                    monthly_gas_limit, monthly_component_upload_limit_bytes,
                    per_invocation_http_call_limit, per_invocation_rpc_call_limit,
                    monthly_http_call_limit, monthly_rpc_call_limit,
                    oplog_writes_per_second
                FROM plans
            "# }))
            .await?;

        Ok(plans)
    }
}
