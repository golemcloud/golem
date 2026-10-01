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

pub mod error;

use self::error::LimitExceededError;
use super::account::{AccountError, AccountService};
use crate::repo::account_usage::AccountUsageRepo;
use crate::repo::environment::EnvironmentRepo;
use crate::repo::model::account_usage::{
    AccountUsage as RepoAccountUsage, AccountUsageRecord, UsageType,
};
use crate::services::account_usage::error::AccountUsageError;
use chrono::{TimeZone, Utc};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::account_usage::{
    AccountUsage, AccountUsageMetering, AccountUsageMetrics, AccountUsagePeriod, MeteringStatus,
    byte_seconds_to_gb_month, fuel_to_gcu,
};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::{
    AccountUsageResourcePattern, AccountUsageVerb, ClassPermissionTarget, PermissionTarget,
};
use golem_service_base::clients::registry::ResourceUsageMetering;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::AuthorizationError;
use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
use golem_service_base::repo::SqlDateTime;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub const BLOB_STORAGE_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(300);
const MAX_CONCURRENT_BLOB_STORAGE_RECONCILIATIONS: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct ResourceUsageUpdate {
    pub fuel_delta: i64,
    pub http_call_count_delta: u64,
    pub rpc_call_count_delta: u64,
    pub durable_storage_byte_seconds_delta: i64,
    pub ephemeral_storage_byte_seconds_delta: i64,
    pub memory_gb_seconds_delta: i64,
    pub blob_storage_bytes_delta: i64,
    pub metering: ResourceUsageMetering,
}

pub struct AccountUsageService {
    account_usage_repo: Arc<dyn AccountUsageRepo>,
    account_service: Arc<AccountService>,
    environment_repo: Arc<dyn EnvironmentRepo>,
    blob_storage: Arc<dyn BlobStorage>,
    blob_storage_reconciliation_enabled: bool,
    blob_storage_reconciliation_interval: Duration,
    blob_storage_reconciliation_state:
        Arc<Mutex<HashMap<AccountId, BlobStorageReconciliationState>>>,
    blob_storage_reconciliation_tasks: Mutex<JoinSet<()>>,
    blob_storage_reconciliation_permits: Arc<Semaphore>,
}

#[derive(Debug, Default)]
struct BlobStorageReconciliationState {
    running: bool,
    last_success: Option<Instant>,
}

// TODO: do we want to add component max size limit?
//       if so, probably should be much bigger then the previous 50mb
impl AccountUsageService {
    pub fn new(
        account_usage_repo: Arc<dyn AccountUsageRepo>,
        account_service: Arc<AccountService>,
        environment_repo: Arc<dyn EnvironmentRepo>,
        blob_storage: Arc<dyn BlobStorage>,
        blob_storage_reconciliation_enabled: bool,
        blob_storage_reconciliation_interval: Duration,
    ) -> Self {
        Self {
            account_usage_repo,
            account_service,
            environment_repo,
            blob_storage,
            blob_storage_reconciliation_enabled,
            blob_storage_reconciliation_interval,
            blob_storage_reconciliation_state: Arc::new(Mutex::new(HashMap::new())),
            blob_storage_reconciliation_tasks: Mutex::new(JoinSet::new()),
            blob_storage_reconciliation_permits: Arc::new(Semaphore::new(
                MAX_CONCURRENT_BLOB_STORAGE_RECONCILIATIONS,
            )),
        }
    }

    fn trigger_blob_storage_reconciliation(&self, account_id: AccountId) {
        // Incremental executor deltas and the S3 snapshot are intentionally not coordinated.
        // Deltas delayed across a sweep can temporarily over- or under-count usage. A later
        // sweep restores the physical total. Sweeps are activity-triggered by limit reads and
        // usage updates, so an idle account is corrected on its next such request after the
        // interval has elapsed.
        if !self.blob_storage_reconciliation_enabled {
            return;
        }

        {
            let mut state = self.blob_storage_reconciliation_state.lock().unwrap();
            let account = state.entry(account_id).or_default();
            if account.running
                || account
                    .last_success
                    .is_some_and(|last| last.elapsed() < self.blob_storage_reconciliation_interval)
            {
                return;
            }
            account.running = true;
        }

        let account_usage_repo = self.account_usage_repo.clone();
        let environment_repo = self.environment_repo.clone();
        let blob_storage = self.blob_storage.clone();
        let state = self.blob_storage_reconciliation_state.clone();
        let permits = self.blob_storage_reconciliation_permits.clone();
        let mut tasks = self.blob_storage_reconciliation_tasks.lock().unwrap();
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let result = async {
                let _permit = permits.acquire_owned().await.map_err(anyhow::Error::new)?;
                reconcile_blob_storage_usage(
                    account_id,
                    account_usage_repo,
                    environment_repo,
                    blob_storage,
                )
                .await
            }
            .await;

            let mut state = state.lock().unwrap();
            let account = state.entry(account_id).or_default();
            account.running = false;
            match result {
                Ok(()) => account.last_success = Some(Instant::now()),
                Err(error) => tracing::warn!(
                    %account_id,
                    %error,
                    "Failed to reconcile blob storage usage; retaining the incrementally tracked value"
                ),
            }
        });
    }

    async fn get_account_usage_for_resource_limits(
        &self,
        account_id: AccountId,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        let period = AccountUsagePeriod::current();
        let date = SqlDateTime::new(
            Utc.with_ymd_and_hms(period.year, period.month, 1, 0, 0, 0)
                .single()
                .expect("validated account usage period"),
        );
        match self
            .account_usage_repo
            .get_for_resource_limits(account_id.0, &date)
            .await?
        {
            Some(usage) => Ok(usage),
            None => Err(AccountUsageError::AccountNotfound(account_id)),
        }
    }

    pub async fn ensure_application_within_limits(
        &self,
        account_id: AccountId,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalAppCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalAppCount, 1)?;

        Ok(())
    }

    pub async fn ensure_environment_within_limits(
        &self,
        account_id: AccountId,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalEnvCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalEnvCount, 1)?;

        Ok(())
    }

    pub async fn ensure_new_component_within_limits(
        &self,
        account_id: AccountId,
        component_size_bytes: u64,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self.get_account_usage(account_id, None).await?;

        self.add_checked(&mut account_usage, UsageType::TotalComponentCount, 1)?;

        if component_size_bytes > i64::MAX as u64 {
            return Err(AccountUsageError::ComponentTooLarge(component_size_bytes));
        }

        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            component_size_bytes as i64,
        )?;

        Ok(())
    }

    pub async fn ensure_updated_component_within_limits(
        &self,
        account_id: AccountId,
        new_component_size_bytes: u64,
        old_component_size_bytes: u64,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalComponentStorageBytes))
            .await?;

        if new_component_size_bytes > i64::MAX as u64 {
            return Err(AccountUsageError::ComponentTooLarge(
                new_component_size_bytes,
            ));
        }

        // Use the net delta so that replacing a large version with a smaller one
        // does not incorrectly count against the quota.
        let delta = new_component_size_bytes as i64 - old_component_size_bytes as i64;

        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            delta,
        )?;

        Ok(())
    }

    pub async fn add_worker_connection(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        auth.authorize_system_only("update account usage")?;

        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalWorkerConnectionCount))
            .await?;
        self.add_checked(&mut account_usage, UsageType::TotalWorkerConnectionCount, 1)?;
        self.account_usage_repo.add(&account_usage).await?;
        Ok(())
    }

    pub async fn remove_worker_connection(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        auth.authorize_system_only("update account usage")?;

        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalWorkerConnectionCount))
            .await?;
        self.add_checked(
            &mut account_usage,
            UsageType::TotalWorkerConnectionCount,
            -1,
        )?;
        self.account_usage_repo.add(&account_usage).await?;
        Ok(())
    }

    /// Admit a registry-dispatched HTTP attempt against the owner's monthly budget.
    /// Like connection admission, the limit check is approximate under concurrency;
    /// the repository atomically adds the charge without losing concurrent deltas.
    pub async fn record_http_call(&self, account_id: AccountId) -> Result<(), AccountUsageError> {
        let mut usage = self
            .get_account_usage(account_id, Some(UsageType::MonthlyHttpCalls))
            .await?;
        self.add_checked(&mut usage, UsageType::MonthlyHttpCalls, 1)?;
        self.account_usage_repo.add(&usage).await?;
        Ok(())
    }

    pub async fn update_resource_usage(
        &self,
        updates: HashMap<AccountId, ResourceUsageUpdate>,
        auth: &AuthCtx,
    ) -> Result<AccountResourceLimits, AccountUsageError> {
        auth.authorize_system_only("update account usage")?;

        let mut limits_of_updated_accounts = HashMap::new();
        for (account_id, update) in updates {
            match self.get_account_usage_for_resource_limits(account_id).await {
                Ok(mut account_usage) => {
                    // Usage can slightly exceed the monthly limit. The worker executor
                    // will suspend the worker at the next opportunity.
                    account_usage.add_change(UsageType::MonthlyGasLimit, update.fuel_delta);
                    account_usage.add_change(
                        UsageType::MonthlyHttpCalls,
                        i64::try_from(update.http_call_count_delta).unwrap_or(i64::MAX),
                    );
                    account_usage.add_change(
                        UsageType::MonthlyRpcCalls,
                        i64::try_from(update.rpc_call_count_delta).unwrap_or(i64::MAX),
                    );
                    account_usage.add_change(
                        UsageType::MonthlyDurableAgentStorageByteSeconds,
                        update.durable_storage_byte_seconds_delta,
                    );
                    account_usage.add_change(
                        UsageType::MonthlyEphemeralStorageByteSeconds,
                        update.ephemeral_storage_byte_seconds_delta,
                    );
                    account_usage.add_change(
                        UsageType::MonthlyMemoryGbSeconds,
                        update.memory_gb_seconds_delta,
                    );
                    account_usage.add_change(
                        UsageType::TotalBlobStorageBytes,
                        update.blob_storage_bytes_delta,
                    );
                    account_usage.metering = Some(update.metering);

                    tracing::debug!(
                        %account_id,
                        fuel_delta = update.fuel_delta,
                        memory_gb_seconds_delta = update.memory_gb_seconds_delta,
                        durable_storage_byte_seconds_delta = update.durable_storage_byte_seconds_delta,
                        ephemeral_storage_byte_seconds_delta = update.ephemeral_storage_byte_seconds_delta,
                        http_call_count_delta = update.http_call_count_delta,
                        rpc_call_count_delta = update.rpc_call_count_delta,
                        blob_storage_bytes_delta = update.blob_storage_bytes_delta,
                        "Updating account resource usage"
                    );

                    match self.account_usage_repo.add(&account_usage).await {
                        Ok(()) => {
                            self.trigger_blob_storage_reconciliation(account_id);
                            match self.get_account_usage_for_resource_limits(account_id).await {
                                Ok(reconciled_usage) => {
                                    limits_of_updated_accounts
                                        .insert(account_id, reconciled_usage.resource_limits());
                                }
                                Err(error) => {
                                    tracing::error!(
                                        %account_id,
                                        %error,
                                        "Failed to reload account usage after update"
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            tracing::error!(
                                %account_id,
                                %error,
                                "Failed to apply resource usage update"
                            );
                        }
                    }
                }
                Err(AccountUsageError::AccountNotfound(_)) => {
                    // We received an update for a deleted account. Return an empty
                    // set of limits to fence the executor more quickly.
                    limits_of_updated_accounts.insert(
                        account_id,
                        ResourceLimits {
                            available_fuel: 0,
                            max_memory_per_worker: 0,
                            max_table_elements_per_worker: 0,
                            max_disk_space_per_worker: 0,
                            per_invocation_http_call_limit: 0,
                            per_invocation_rpc_call_limit: 0,
                            available_http_calls: 0,
                            available_rpc_calls: 0,
                            available_blob_storage_bytes: 0,
                            max_concurrent_agents_per_executor: 0,
                            oplog_writes_per_second: 0,
                            usage_update_applied: false,
                        },
                    );
                }
                Err(error) => {
                    tracing::error!(
                        %account_id,
                        %error,
                        "Failed to load account usage for resource update"
                    );
                }
            }
        }

        Ok(AccountResourceLimits(limits_of_updated_accounts))
    }

    pub async fn get_resouce_limits(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<ResourceLimits, AccountUsageError> {
        let account = self
            .account_service
            .get(account_id, auth)
            .await
            .map_err(map_account_error(account_id))?;

        authorize_account_usage_permission(auth, &account.email, AccountUsageVerb::View)?;

        self.trigger_blob_storage_reconciliation(account_id);
        let account_usage = self
            .get_account_usage_for_resource_limits(account_id)
            .await?;

        Ok(account_usage.resource_limits())
    }

    pub async fn get_usage(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<AccountUsage, AccountUsageError> {
        self.get_usage_for_period(account_id, AccountUsagePeriod::current(), auth)
            .await
    }

    pub async fn get_usage_for_period(
        &self,
        account_id: AccountId,
        period: AccountUsagePeriod,
        auth: &AuthCtx,
    ) -> Result<AccountUsage, AccountUsageError> {
        self.authorize_usage(account_id, auth).await?;
        let report = self
            .account_usage_repo
            .get_usage_report(account_id.0, period)
            .await?;
        Ok(Self::usage(account_id, report))
    }

    pub async fn get_usage_history(
        &self,
        account_id: AccountId,
        last: usize,
        auth: &AuthCtx,
    ) -> Result<Vec<AccountUsage>, AccountUsageError> {
        let current_period = AccountUsagePeriod::current();
        self.authorize_usage(account_id, auth).await?;
        let history = self
            .account_usage_repo
            .get_usage_history(account_id.0, current_period, last)
            .await?;

        Ok(history
            .into_iter()
            .map(|report| Self::usage(account_id, report))
            .collect())
    }

    async fn authorize_usage(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        let account = self
            .account_service
            .get(account_id, auth)
            .await
            .map_err(map_account_error(account_id))?;
        authorize_account_usage_permission(auth, &account.email, AccountUsageVerb::View)?;
        Ok(())
    }

    fn usage(account_id: AccountId, report: AccountUsageRecord) -> AccountUsage {
        let metering = report.metering.map_or_else(
            || AccountUsageMetering {
                compute: MeteringStatus::Unknown,
                memory: MeteringStatus::Unknown,
                durable_storage: MeteringStatus::Unknown,
                ephemeral_storage: MeteringStatus::Unknown,
            },
            |metering| {
                let filesystem = metering_status(metering.filesystem);
                AccountUsageMetering {
                    compute: metering_status(metering.compute),
                    memory: metering_status(metering.memory),
                    durable_storage: filesystem,
                    ephemeral_storage: filesystem,
                }
            },
        );
        AccountUsage {
            account_id,
            usage: AccountUsageMetrics {
                period: report.period,
                as_of: report.as_of.unwrap_or_else(Utc::now),
                compute_gcu: fuel_to_gcu(report.compute_fuel),
                memory_gb_seconds: report.memory_gb_seconds,
                durable_storage_gb_month: byte_seconds_to_gb_month(
                    report.durable_storage_byte_seconds,
                ),
                ephemeral_storage_gb_month: byte_seconds_to_gb_month(
                    report.ephemeral_storage_byte_seconds,
                ),
                metering,
            },
        }
    }

    async fn get_account_usage(
        &self,
        account_id: AccountId,
        usage_type: Option<UsageType>,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        self.get_account_usage_at(account_id, usage_type, AccountUsagePeriod::current())
            .await
    }

    async fn get_account_usage_at(
        &self,
        account_id: AccountId,
        usage_type: Option<UsageType>,
        period: AccountUsagePeriod,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        let date = SqlDateTime::new(
            Utc.with_ymd_and_hms(period.year, period.month, 1, 0, 0, 0)
                .single()
                .expect("validated account usage period"),
        );
        let usage = match usage_type {
            Some(usage_type) => {
                self.account_usage_repo
                    .get_for_type(account_id.0, &date, usage_type)
                    .await?
            }
            None => self.account_usage_repo.get(account_id.0, &date).await?,
        };

        match usage {
            Some(usage) => Ok(usage),
            None => Err(AccountUsageError::AccountNotfound(account_id)),
        }
    }

    fn add_checked(
        &self,
        account_usage: &mut RepoAccountUsage,
        usage_type: UsageType,
        value: i64,
    ) -> Result<(), AccountUsageError> {
        if !account_usage.add_change(usage_type, value) {
            return Err(AccountUsageError::LimitExceeded(LimitExceededError {
                limit_name: format!("{usage_type:?}"),
                limit_value: account_usage.plan.limit(usage_type),
                current_value: account_usage.usage(usage_type),
            }));
        }

        Ok(())
    }
}

async fn reconcile_blob_storage_usage(
    account_id: AccountId,
    account_usage_repo: Arc<dyn AccountUsageRepo>,
    environment_repo: Arc<dyn EnvironmentRepo>,
    blob_storage: Arc<dyn BlobStorage>,
) -> Result<(), AccountUsageError> {
    let environments = environment_repo
        .list_ids_by_account_including_deleted(account_id.0)
        .await
        .map_err(anyhow::Error::new)?;
    let mut total = 0u64;
    for environment_id in environments {
        let blobs = blob_storage
            .list_blobs_below(
                "account_usage",
                "reconcile_blob_storage_usage",
                BlobStorageNamespace::CustomStorage {
                    environment_id: environment_id.into(),
                },
                Path::new(""),
            )
            .await?;
        total = blobs.iter().fold(total, |sum, (_, metadata)| {
            sum.saturating_add(metadata.size)
        });
    }
    account_usage_repo
        .set_total_usage(account_id.0, UsageType::TotalBlobStorageBytes, total)
        .await?;
    Ok(())
}

fn metering_status(enabled: bool) -> MeteringStatus {
    if enabled {
        MeteringStatus::Enabled
    } else {
        MeteringStatus::Disabled
    }
}

pub(crate) fn map_account_error(
    account_id: AccountId,
) -> impl FnOnce(AccountError) -> AccountUsageError {
    move |err| match err {
        AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
            AccountUsageError::AccountNotfound(account_id)
        }
        other => AccountUsageError::InternalError(other.into()),
    }
}

pub(crate) fn authorize_account_usage_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    verb: AccountUsageVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&account_usage_permission_target(account_email, verb))
}

fn account_usage_permission_target(
    account_email: &AccountEmail,
    verb: AccountUsageVerb,
) -> PermissionTarget {
    PermissionTarget::AccountUsage(ClassPermissionTarget {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: account_email.clone(),
        },
        resource: AccountUsageResourcePattern,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::model::account_usage::{AccountUsage, UsageType};
    use crate::repo::model::plan::PlanRecord;
    use golem_common::model::account_usage::StorageLimit;
    use golem_service_base::repo::NumericU64;
    use std::collections::BTreeMap;
    use test_r::test;
    use uuid::Uuid;

    /// Build a minimal `AccountUsage` with a given storage quota and current usage.
    fn make_usage(storage_limit: u64, current_storage_bytes: u64) -> AccountUsage {
        let plan = PlanRecord {
            plan_id: Uuid::new_v4(),
            name: "test".to_string(),
            max_memory_per_worker: NumericU64::new(u64::MAX),
            max_memory_per_worker_ceiling: NumericU64::new(u64::MAX),
            max_memory_per_worker_user_configurable: false,
            monthly_memory_gb_seconds: NumericU64::new(u64::MAX),
            monthly_memory_gb_seconds_ceiling: NumericU64::new(u64::MAX),
            monthly_memory_gb_seconds_user_configurable: false,
            max_table_elements_per_worker: NumericU64::new(u64::MAX),
            max_disk_space_per_worker: NumericU64::new(u64::MAX),
            max_disk_space_per_worker_ceiling: NumericU64::new(u64::MAX),
            max_disk_space_per_worker_user_configurable: false,
            max_concurrent_agents_per_executor: NumericU64::new(u64::MAX),
            total_app_count: NumericU64::new(u64::MAX),
            total_env_count: NumericU64::new(u64::MAX),
            total_component_count: NumericU64::new(u64::MAX),
            total_worker_connection_count: NumericU64::new(u64::MAX),
            total_component_storage_bytes: NumericU64::new(storage_limit),
            total_blob_storage_bytes: NumericU64::new(u64::MAX),
            monthly_gas_limit: NumericU64::new(u64::MAX),
            monthly_component_upload_limit_bytes: NumericU64::new(u64::MAX),
            per_invocation_http_call_limit: NumericU64::new(u64::MAX),
            per_invocation_rpc_call_limit: NumericU64::new(u64::MAX),
            monthly_http_call_limit: NumericU64::new(u64::MAX),
            monthly_rpc_call_limit: NumericU64::new(u64::MAX),
            oplog_writes_per_second: NumericU64::new(u64::MAX),
        };
        let mut usage = BTreeMap::new();
        usage.insert(UsageType::TotalComponentStorageBytes, current_storage_bytes);
        AccountUsage {
            account_id: Uuid::new_v4(),
            year: 2026,
            month: 1,
            usage,
            plan,
            storage_limit: StorageLimit {
                effective_value: u64::MAX,
                plan_default: u64::MAX,
                override_value: None,
                ceiling: u64::MAX,
                user_configurable: false,
            },
            max_memory_per_worker: golem_common::model::account_usage::MemoryLimit {
                effective_value: u64::MAX,
                plan_default: u64::MAX,
                override_value: None,
                ceiling: u64::MAX,
                user_configurable: false,
            },
            monthly_memory_gb_seconds: golem_common::model::account_usage::MemoryLimit {
                effective_value: u64::MAX,
                plan_default: u64::MAX,
                override_value: None,
                ceiling: u64::MAX,
                user_configurable: false,
            },
            metering: None,
            changes: BTreeMap::new(),
        }
    }

    /// Simulates `ensure_updated_component_within_limits` inline so we can test
    /// the delta logic without needing a database-backed `AccountUsageRepo`.
    fn check_update(
        usage: &mut AccountUsage,
        new_bytes: u64,
        old_bytes: u64,
    ) -> Result<(), AccountUsageError> {
        if new_bytes > i64::MAX as u64 {
            return Err(AccountUsageError::ComponentTooLarge(new_bytes));
        }
        let delta = new_bytes as i64 - old_bytes as i64;
        if !usage.add_change(UsageType::TotalComponentStorageBytes, delta) {
            return Err(AccountUsageError::LimitExceeded(LimitExceededError {
                limit_name: "TotalComponentStorageBytes".to_string(),
                limit_value: usage.plan.limit(UsageType::TotalComponentStorageBytes),
                current_value: usage.usage(UsageType::TotalComponentStorageBytes),
            }));
        }
        Ok(())
    }

    #[test]
    fn update_with_smaller_version_is_allowed_near_quota() {
        // Quota: 1000 bytes. Current usage: 900 bytes (from existing component of 900 bytes).
        // Updating to a 500-byte version → net delta = 500 - 900 = -400 → should be allowed.
        let mut usage = make_usage(1000, 900);
        let result = check_update(&mut usage, 500, 900);
        assert!(
            result.is_ok(),
            "replacing 900-byte component with 500-byte version should be allowed near quota"
        );
        // Final projected usage = 900 + (-400) = 500
        assert_eq!(
            usage.final_value(UsageType::TotalComponentStorageBytes),
            500
        );
    }

    #[test]
    fn update_that_exceeds_quota_is_rejected() {
        // Quota: 1000 bytes. Current usage: 900 bytes (from existing 400-byte component).
        // Updating to an 800-byte version → net delta = 800 - 400 = +400 → 900 + 400 = 1300 > 1000.
        let mut usage = make_usage(1000, 900);
        let result = check_update(&mut usage, 800, 400);
        assert!(
            result.is_err(),
            "update that pushes total over quota must be rejected"
        );
    }

    #[test]
    fn update_to_same_size_is_allowed() {
        // Quota: 1000 bytes. Current usage: 900 bytes.
        // Updating to a component of the same size → net delta = 0 → always allowed.
        let mut usage = make_usage(1000, 900);
        let result = check_update(&mut usage, 500, 500);
        assert!(result.is_ok(), "same-size update must always be allowed");
        assert_eq!(
            usage.final_value(UsageType::TotalComponentStorageBytes),
            900
        );
    }

    #[test]
    fn update_within_quota_is_allowed() {
        // Quota: 1000 bytes. Current usage: 400 bytes (from existing 200-byte component).
        // Updating to a 400-byte version → net delta = 400 - 200 = +200 → 400 + 200 = 600 ≤ 1000.
        let mut usage = make_usage(1000, 400);
        let result = check_update(&mut usage, 400, 200);
        assert!(result.is_ok(), "update within quota must be allowed");
        assert_eq!(
            usage.final_value(UsageType::TotalComponentStorageBytes),
            600
        );
    }
}
