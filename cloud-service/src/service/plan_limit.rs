use crate::auth::AccountAuthorisation;
use crate::model::{AccountAction, Plan, ResourceLimits};
use crate::repo::account::AccountRepo;
use crate::repo::account_components::AccountComponentsRepo;
use crate::repo::account_connections::AccountConnectionsRepo;
use crate::repo::account_fuel::AccountFuelRepo;
use crate::repo::account_uploads::AccountUploadsRepo;
use crate::repo::account_used_storage::AccountUsedStorageRepo;
use crate::repo::account_workers::AccountWorkersRepo;
use crate::repo::plan::PlanRepo;
use crate::repo::project::ProjectRepo;
use async_trait::async_trait;
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
use std::fmt::Debug;
use std::num::TryFromIntError;
use std::sync::Arc;

use super::auth::{AuthService, AuthServiceError};

#[derive(Debug, thiserror::Error)]
pub enum PlanLimitError {
    #[error("Limit Exceeded: {0}")]
    LimitExceeded(String),
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Project Not Found: {0}")]
    ProjectNotFound(ProjectId),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Internal repository error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
}

impl PlanLimitError {
    fn limit_exceeded(error: impl AsRef<str>) -> Self {
        Self::LimitExceeded(error.as_ref().to_string())
    }
}

impl SafeDisplay for PlanLimitError {
    fn to_safe_string(&self) -> String {
        match self {
            PlanLimitError::LimitExceeded(_) => self.to_string(),
            PlanLimitError::AccountNotFound(_) => self.to_string(),
            PlanLimitError::ProjectNotFound(_) => self.to_string(),
            PlanLimitError::Internal(_) => self.to_string(),
            PlanLimitError::InternalRepoError(inner) => inner.to_safe_string(),
            PlanLimitError::AuthError(inner) => inner.to_safe_string(),
        }
    }
}

#[derive(Clone)]
pub struct LimitResult {
    pub account_id: AccountId,
    pub plan: Plan,
}

#[derive(Clone)]
pub struct CheckLimitResult {
    pub account_id: AccountId,
    pub count: i64,
    pub limit: i64,
}

impl CheckLimitResult {
    pub fn in_limit(&self) -> bool {
        self.count <= self.limit
    }

    pub fn not_in_limit(&self) -> bool {
        !self.in_limit()
    }

    pub fn available(&self) -> i64 {
        self.limit - self.count
    }

    pub fn add(&self, count: i64) -> Self {
        Self {
            count: self.count + count,
            limit: self.limit,
            account_id: self.account_id.clone(),
        }
    }
}

#[async_trait]
pub trait PlanLimitService {
    /// Get Account Limits.
    async fn get_account_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<LimitResult, PlanLimitError>;

    /// Get Project Limits.
    async fn get_project_limits(
        &self,
        project_id: &ProjectId,
    ) -> Result<LimitResult, PlanLimitError>;

    /// Check Limits.
    async fn check_project_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError>;

    /// Get memory and CPU limits
    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<ResourceLimits, PlanLimitError>;

    /// Record fuel consumption - internal API for executors
    async fn record_fuel_consumption(
        &self,
        updates: HashMap<AccountId, i64>,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError>;

    /// Update component limit.
    async fn update_component_limit(
        &self,
        account_id: &AccountId,
        count: i32,
        size: i64,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError>;

    /// Update worker limit.
    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        value: i32,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError>;

    /// Update worker connection limit.
    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        value: i32,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError>;
}

pub struct PlanLimitServiceDefault {
    auth_service: Arc<dyn AuthService>,
    plan_repo: Arc<dyn PlanRepo + Sync + Send>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
    account_workers_repo: Arc<dyn AccountWorkersRepo + Sync + Send>,
    account_connections_repo: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    account_components_repo: Arc<dyn AccountComponentsRepo + Sync + Send>,
    account_used_storage_repo: Arc<dyn AccountUsedStorageRepo + Sync + Send>,
    account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
    project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    account_fuel_repo: Arc<dyn AccountFuelRepo + Sync + Send>,
}

#[async_trait]
impl PlanLimitService for PlanLimitServiceDefault {
    async fn get_account_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<LimitResult, PlanLimitError> {
        let plan = self.get_plan(account_id).await?;
        Ok(LimitResult {
            account_id: account_id.clone(),
            plan,
        })
    }

    async fn get_project_limits(
        &self,
        project_id: &ProjectId,
    ) -> Result<LimitResult, PlanLimitError> {
        let account_id = self.get_account_id(project_id).await?;
        let result = self.get_account_limits(&account_id).await?;
        Ok(result)
    }

    async fn check_project_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        let limits = self.get_account_limits(account_id).await?;
        let num_projects = self.project_repo.get_owned_count(&account_id.value).await?;
        let count: i64 = num_projects.try_into().map_err(|e: TryFromIntError| {
            PlanLimitError::Internal(format!("Failed to convert projects count: {e}"))
        })?;

        Ok(CheckLimitResult {
            account_id: account_id.clone(),
            count,
            limit: limits.plan.plan_data.project_limit.into(),
        })
    }

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<ResourceLimits, PlanLimitError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::ViewLimits)
            .await?;

        let plan = self.get_plan(account_id).await?;
        let fuel = self.account_fuel_repo.get(account_id).await?;
        let available_fuel = plan.plan_data.monthly_gas_limit - fuel;
        Ok(ResourceLimits {
            available_fuel,
            max_memory_per_worker: 100 * 1024 * 1024,
        })
    }

    async fn record_fuel_consumption(
        &self,
        updates: HashMap<AccountId, i64>,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError> {
        // TODO: Should we do this in parallel?
        for (account_id, update) in updates {
            self.auth_service
                .authorize_account_action(auth, &account_id, &AccountAction::UpdateLimits)
                .await?;
            self.get_plan(&account_id).await?;
            self.account_fuel_repo.update(&account_id, update).await?;
        }
        Ok(())
    }

    async fn update_component_limit(
        &self,
        account_id: &AccountId,
        count: i32,
        size: i64,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::UpdateLimits)
            .await?;

        if size > 50000000 {
            return Err(PlanLimitError::limit_exceeded(
                "Component size limit exceeded (limit: 50MB)",
            ));
        }

        let plan = self.get_plan(account_id).await?;

        let num_components = self.account_components_repo.get(account_id).await?;

        let component_limit = CheckLimitResult {
            account_id: account_id.clone(),
            count: num_components as i64,
            limit: plan.plan_data.component_limit.into(),
        };

        if !component_limit.add(count as i64).in_limit() {
            return Err(PlanLimitError::limit_exceeded(format!(
                "Component limit exceeded (limit: {})",
                component_limit.limit
            )));
        }

        let num_uploads = self.account_uploads_repo.get(account_id).await?;

        let upload_limit = CheckLimitResult {
            account_id: account_id.clone(),
            count: num_uploads as i64,
            limit: plan.plan_data.monthly_upload_limit.into(),
        };

        if !upload_limit.add(size).in_limit() {
            return Err(PlanLimitError::limit_exceeded(format!(
                "Upload limit exceeded for account: {} (limit: {} MB)",
                upload_limit.account_id.value,
                upload_limit.limit / 1000000
            )));
        }

        let used_storage = self.account_used_storage_repo.get(account_id).await?;

        let storage_limit = CheckLimitResult {
            account_id: account_id.clone(),
            count: used_storage,
            limit: plan.plan_data.storage_limit.into(),
        };

        if !storage_limit.add(size).in_limit() {
            Err(PlanLimitError::limit_exceeded(format!(
                "Storage limit exceeded for account: {} (limit: {} MB)",
                storage_limit.account_id.value,
                storage_limit.limit / 1000000
            )))
        } else {
            self.account_components_repo
                .update(account_id, count)
                .await?;
            self.account_used_storage_repo
                .update(account_id, size)
                .await?;
            self.account_uploads_repo
                .update(account_id, size as i32)
                .await?;
            Ok(())
        }
    }

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        value: i32,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::UpdateLimits)
            .await?;

        let plan = self.get_plan(account_id).await?;
        let num_workers = self.account_workers_repo.get(account_id).await?;

        if value > 0 {
            let check_limit = CheckLimitResult {
                account_id: account_id.clone(),
                count: (num_workers + value).into(),
                limit: plan.plan_data.worker_limit.into(),
            };

            if check_limit.in_limit() {
                self.account_workers_repo.update(account_id, value).await?;
            } else {
                return Err(PlanLimitError::limit_exceeded(format!(
                    "Worker limit exceeded (limit: {})",
                    check_limit.limit
                )));
            }
        } else {
            self.account_workers_repo.update(account_id, value).await?;
        }

        Ok(())
    }

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        value: i32,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::UpdateLimits)
            .await?;

        let connections = self.account_connections_repo.get(account_id).await?;

        if value > 0 {
            let check_limit = CheckLimitResult {
                account_id: account_id.clone(),
                count: (connections + value).into(),
                limit: 10,
            };

            if check_limit.in_limit() {
                self.account_connections_repo
                    .update(account_id, value)
                    .await?;
            } else {
                return Err(PlanLimitError::limit_exceeded(format!(
                    "Worker connection limit exceeded (limit: {})",
                    check_limit.limit
                )));
            }
        } else {
            self.account_connections_repo
                .update(account_id, value)
                .await?;
        }

        Ok(())
    }
}

// Helper functions.
impl PlanLimitServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService>,
        plan_repo: Arc<dyn PlanRepo + Sync + Send>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
        account_workers_repo: Arc<dyn AccountWorkersRepo + Sync + Send>,
        account_connections_repo: Arc<dyn AccountConnectionsRepo + Send + Sync>,
        account_components_repo: Arc<dyn AccountComponentsRepo + Sync + Send>,
        account_used_storage_repo: Arc<dyn AccountUsedStorageRepo + Sync + Send>,
        account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
        project_repo: Arc<dyn ProjectRepo + Sync + Send>,
        account_fuel_repo: Arc<dyn AccountFuelRepo + Sync + Send>,
    ) -> Self {
        PlanLimitServiceDefault {
            auth_service,
            plan_repo,
            account_repo,
            account_workers_repo,
            account_connections_repo,
            account_components_repo,
            account_used_storage_repo,
            account_uploads_repo,
            project_repo,
            account_fuel_repo,
        }
    }

    async fn get_plan(&self, account_id: &AccountId) -> Result<Plan, PlanLimitError> {
        if let Some(account) = self.account_repo.get(&account_id.value).await? {
            if let Some(plan) = self.plan_repo.get(&account.plan_id).await? {
                Ok(plan.into())
            } else {
                Err(PlanLimitError::AccountNotFound(account_id.clone()))
            }
        } else {
            Err(PlanLimitError::AccountNotFound(account_id.clone()))
        }
    }

    async fn get_account_id(&self, project_id: &ProjectId) -> Result<AccountId, PlanLimitError> {
        if let Some(project) = self.project_repo.get(&project_id.0).await? {
            Ok(AccountId {
                value: project.owner_account_id,
            })
        } else {
            Err(PlanLimitError::ProjectNotFound(project_id.clone()))
        }
    }
}
