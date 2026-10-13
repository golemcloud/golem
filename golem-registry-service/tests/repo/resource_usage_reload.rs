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

use super::Deps;
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AccountUsagePeriod, MonthlyUsageMode, MonthlyUsageModeTransitionSource,
};
use golem_registry_service::repo::account_usage::{
    AccountUsageRepo, ResolvedMonthlyPolicyRevision, SetMonthlyUsageModeError,
};
use golem_registry_service::repo::model::account_usage::{
    AccountMonthlyUsageMode, AccountUsage, AccountUsageRecord, PersistedMonthlyUsageModeTransition,
    UsageType,
};
use golem_registry_service::services::account_usage::{AccountUsageService, ResourceUsageUpdate};
use golem_service_base::clients::registry::ResourceUsageMetering;
use golem_service_base::model::{MonthlyResourcePolicyStatus, auth::AuthCtx};
use golem_service_base::repo::{RepoError, RepoResult, SqlDateTime};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use uuid::Uuid;

struct FailReloadAfterCommit {
    inner: Arc<dyn AccountUsageRepo>,
    unavailable: AtomicBool,
    commits: AtomicUsize,
    failed_reads: AtomicUsize,
}

#[async_trait]
impl AccountUsageRepo for FailReloadAfterCommit {
    async fn get_with_active_overrides_at(
        &self,
        account: Uuid,
        date: &SqlDateTime,
        active: &SqlDateTime,
    ) -> RepoResult<Option<AccountUsage>> {
        self.inner
            .get_with_active_overrides_at(account, date, active)
            .await
    }
    async fn get_for_resource_limits(
        &self,
        account: Uuid,
        date: &SqlDateTime,
        metering: ResourceUsageMetering,
    ) -> RepoResult<Option<AccountUsage>> {
        if self.unavailable.load(Ordering::Acquire) {
            self.failed_reads.fetch_add(1, Ordering::AcqRel);
            return Err(RepoError::InternalError(anyhow::anyhow!(
                "resource policy reload unavailable"
            )));
        }
        self.inner
            .get_for_resource_limits(account, date, metering)
            .await
    }
    async fn get_for_type(
        &self,
        account: Uuid,
        date: &SqlDateTime,
        dimension: UsageType,
    ) -> RepoResult<Option<AccountUsage>> {
        self.inner.get_for_type(account, date, dimension).await
    }
    async fn get_usage_report(
        &self,
        account: Uuid,
        period: AccountUsagePeriod,
    ) -> RepoResult<AccountUsageRecord> {
        self.inner.get_usage_report(account, period).await
    }
    async fn get_usage_history(
        &self,
        account: Uuid,
        period: AccountUsagePeriod,
        last: usize,
    ) -> RepoResult<Vec<AccountUsageRecord>> {
        self.inner.get_usage_history(account, period, last).await
    }
    async fn get_monthly_usage_mode(
        &self,
        account: Uuid,
    ) -> RepoResult<Option<AccountMonthlyUsageMode>> {
        self.inner.get_monthly_usage_mode(account).await
    }
    async fn get_monthly_usage_mode_transitions(
        &self,
        account: Uuid,
    ) -> RepoResult<Vec<PersistedMonthlyUsageModeTransition>> {
        self.inner.get_monthly_usage_mode_transitions(account).await
    }
    async fn set_monthly_usage_mode(
        &self,
        account: Uuid,
        mode: MonthlyUsageMode,
        actor: Uuid,
        source: MonthlyUsageModeTransitionSource,
    ) -> Result<PersistedMonthlyUsageModeTransition, SetMonthlyUsageModeError> {
        self.inner
            .set_monthly_usage_mode(account, mode, actor, source)
            .await
    }
    async fn get_or_create_monthly_policy_revision(
        &self,
        account: Uuid,
        period: AccountUsagePeriod,
        active: &SqlDateTime,
        metering: ResourceUsageMetering,
    ) -> RepoResult<ResolvedMonthlyPolicyRevision> {
        self.inner
            .get_or_create_monthly_policy_revision(account, period, active, metering)
            .await
    }
    async fn add(&self, usage: &AccountUsage) -> RepoResult<u64> {
        let revision = self.inner.add(usage).await?;
        self.commits.fetch_add(1, Ordering::AcqRel);
        self.unavailable.store(true, Ordering::Release);
        Ok(revision)
    }
    async fn set_total_usage(
        &self,
        account: Uuid,
        dimension: UsageType,
        value: u64,
    ) -> RepoResult<()> {
        self.inner.set_total_usage(account, dimension, value).await
    }
}

pub async fn committed_usage_with_failed_reload_reports_unavailable_and_recovers(deps: &Deps) {
    let account = deps.create_account().await.revision.account_id;
    let mut plan = deps
        .plan_repo
        .get_by_id(deps.test_plan_id())
        .await
        .unwrap()
        .unwrap();
    let original_plan = plan.clone();
    plan.overage_eligible = true;
    deps.plan_repo.create_or_update(plan).await.unwrap();
    deps.account_usage_repo
        .set_monthly_usage_mode(
            account,
            MonthlyUsageMode::AllowOverage,
            account,
            MonthlyUsageModeTransitionSource::Owner,
        )
        .await
        .unwrap();
    let repo = Arc::new(FailReloadAfterCommit {
        inner: deps.account_usage_repo.clone(),
        unavailable: AtomicBool::new(false),
        commits: AtomicUsize::new(0),
        failed_reads: AtomicUsize::new(0),
    });
    let service = AccountUsageService::new(
        repo.clone(),
        deps.account_service(),
        deps.environment_repo.clone(),
        deps.blob_storage.clone(),
        false,
        std::time::Duration::from_secs(300),
    );
    let metering = ResourceUsageMetering {
        compute: false,
        memory: true,
        filesystem: false,
    };
    let initial = service
        .get_resouce_limits(AccountId(account), metering, &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(initial.monthly_policy.mode, MonthlyUsageMode::AllowOverage);
    assert!(initial.monthly_policy.available_memory_gb_seconds > 1);
    let updates = HashMap::from([(
        AccountId(account),
        ResourceUsageUpdate {
            period: initial.monthly_policy.period,
            monthly_usage_mode_revision: initial.monthly_usage_mode_revision,
            monthly_policy_revision: initial.monthly_policy_revision,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta: 0,
            http_call_count_delta: 0,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta: 1,
            blob_storage_bytes_delta: 0,
            metering,
        },
    )]);
    let result = service
        .update_resource_usage(updates, &AuthCtx::System)
        .await
        .unwrap();
    let unavailable = result.0.get(&AccountId(account)).unwrap();
    assert_eq!(repo.commits.load(Ordering::Acquire), 1);
    assert_eq!(repo.failed_reads.load(Ordering::Acquire), 1);
    assert!(unavailable.usage_update_applied);
    assert_eq!(
        unavailable.monthly_policy_status,
        MonthlyResourcePolicyStatus::Unavailable
    );
    assert_eq!(unavailable.monthly_policy.mode, initial.monthly_policy.mode);
    assert_eq!(
        unavailable.monthly_policy_revision,
        initial.monthly_policy_revision
    );
    assert_eq!(
        unavailable.max_memory_per_worker,
        initial.max_memory_per_worker
    );
    let persisted = deps
        .account_usage_repo
        .get(account, &SqlDateTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.usage(UsageType::MonthlyMemoryGbSeconds), 1);
    repo.unavailable.store(false, Ordering::Release);
    let restored = service
        .get_resouce_limits(AccountId(account), metering, &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(
        restored.monthly_policy_status,
        MonthlyResourcePolicyStatus::Available
    );
    assert_eq!(restored.monthly_policy.mode, MonthlyUsageMode::AllowOverage);
    assert_eq!(
        restored.monthly_policy.available_memory_gb_seconds,
        initial.monthly_policy.available_memory_gb_seconds - 1
    );
    assert_eq!(
        repo.commits.load(Ordering::Acquire),
        1,
        "recovery must not replay committed usage"
    );
    deps.plan_repo
        .create_or_update(original_plan)
        .await
        .unwrap();
}
