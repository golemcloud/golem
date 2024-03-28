use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::model::{Plan, ResourceLimits, Role};
use crate::repo::account::AccountRepo;
use crate::repo::account_fuel::AccountFuelRepo;
use crate::repo::account_uploads::AccountUploadsRepo;
use crate::repo::account_workers::AccountWorkersRepo;
use crate::repo::plan::PlanRepo;
use crate::repo::project::ProjectRepo;
use crate::repo::template::TemplateRepo;
use crate::repo::RepoError;
use async_trait::async_trait;
use golem_common::model::ProjectId;
use golem_common::model::{AccountId, TemplateId};

#[derive(Debug, Clone)]
pub enum PlanLimitError {
    AccountIdNotFound(AccountId),
    ProjectIdNotFound(ProjectId),
    TemplateIdNotFound(TemplateId),
    Internal(String),
    Unauthorized(String),
}

impl PlanLimitError {
    pub fn internal<T: Display>(error: T) -> Self {
        PlanLimitError::Internal(error.to_string())
    }
}

impl From<RepoError> for PlanLimitError {
    fn from(error: RepoError) -> Self {
        PlanLimitError::internal(error)
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
        self.count < self.limit
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
    /// Get Limits.

    async fn get_account_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<LimitResult, PlanLimitError>;

    async fn get_project_limits(
        &self,
        project_id: &ProjectId,
    ) -> Result<LimitResult, PlanLimitError>;

    async fn get_template_limits(
        &self,
        template_id: &TemplateId,
    ) -> Result<LimitResult, PlanLimitError>;

    /// Check Limits.

    async fn check_project_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError>;

    async fn check_template_limit(
        &self,
        project_id: &ProjectId,
    ) -> Result<CheckLimitResult, PlanLimitError>;

    async fn check_worker_limit(
        &self,
        template_id: &TemplateId,
    ) -> Result<CheckLimitResult, PlanLimitError>;

    async fn check_storage_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError>;

    async fn check_upload_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError>;

    /// Fuel consumption.

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<ResourceLimits, PlanLimitError>;

    async fn record_fuel_consumption(
        &self,
        updates: HashMap<AccountId, i64>,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError>;
}

pub struct PlanLimitServiceDefault {
    plan_repo: Arc<dyn PlanRepo + Sync + Send>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
    account_workers_repo: Arc<dyn AccountWorkersRepo + Sync + Send>,
    account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
    project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    template_repo: Arc<dyn TemplateRepo + Sync + Send>,
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

    async fn get_template_limits(
        &self,
        template_id: &TemplateId,
    ) -> Result<LimitResult, PlanLimitError> {
        let project_id = self.get_project_id(template_id).await?;
        let account_id = self.get_account_id(&project_id).await?;
        let result = self.get_account_limits(&account_id).await?;
        Ok(result)
    }

    async fn check_project_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        let limits = self.get_account_limits(account_id).await?;
        let num_projects = self.project_repo.get_own_count(&account_id.value).await?;
        let count: i64 = num_projects
            .try_into()
            .map_err(|_| PlanLimitError::internal("Failed to convert projects count"))?;

        Ok(CheckLimitResult {
            account_id: account_id.clone(),
            count,
            limit: limits.plan.plan_data.project_limit.into(),
        })
    }

    async fn check_template_limit(
        &self,
        project_id: &ProjectId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        let account_id = self.get_account_id(project_id).await?;
        let limits = self.get_account_limits(&account_id).await?;
        let projects = self.project_repo.get_own(&account_id.value).await?;
        let project_ids = projects.into_iter().map(|p| p.project_id).collect();
        let num_templates = self
            .template_repo
            .get_count_by_projects(project_ids)
            .await?;

        let count = num_templates
            .try_into()
            .map_err(|_| PlanLimitError::internal("Failed to convert templates count"))?;

        Ok(CheckLimitResult {
            account_id,
            count,
            limit: limits.plan.plan_data.template_limit.into(),
        })
    }

    async fn check_worker_limit(
        &self,
        template_id: &TemplateId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        let project_id = self.get_project_id(template_id).await?;
        let account_id = self.get_account_id(&project_id).await?;
        let plan = self.get_plan(&account_id).await?;
        let num_workers = self.account_workers_repo.get(&account_id).await?;

        Ok(CheckLimitResult {
            account_id,
            count: num_workers.into(),
            limit: plan.plan_data.worker_limit.into(),
        })
    }

    async fn check_storage_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        let plan = self.get_plan(account_id).await?;
        let projects = self.project_repo.get_own(&account_id.value).await?;
        let project_ids = projects.into_iter().map(|p| p.project_id).collect();
        let count = self.template_repo.get_size_by_projects(project_ids).await?;

        let count = count
            .try_into()
            .map_err(|_| PlanLimitError::internal("Failed to convert storage count"))?;

        Ok(CheckLimitResult {
            account_id: account_id.clone(),
            count,
            limit: plan.plan_data.storage_limit.into(),
        })
    }

    async fn check_upload_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        let plan = self.get_plan(account_id).await?;
        let num_uploads = self.account_uploads_repo.get(account_id).await?;

        Ok(CheckLimitResult {
            account_id: account_id.clone(),
            count: num_uploads.into(),
            limit: plan.plan_data.monthly_upload_limit.into(),
        })
    }

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<ResourceLimits, PlanLimitError> {
        self.check_authorization(account_id, auth)?;
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
            self.check_authorization(&account_id, auth)?;
            self.get_plan(&account_id).await?;
            self.account_fuel_repo.update(&account_id, update).await?;
        }
        Ok(())
    }
}

// Helper functions.
impl PlanLimitServiceDefault {
    pub fn new(
        plan_repo: Arc<dyn PlanRepo + Sync + Send>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
        account_workers_repo: Arc<dyn AccountWorkersRepo + Sync + Send>,
        account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
        project_repo: Arc<dyn ProjectRepo + Sync + Send>,
        template_repo: Arc<dyn TemplateRepo + Sync + Send>,
        account_fuel_repo: Arc<dyn AccountFuelRepo + Sync + Send>,
    ) -> Self {
        PlanLimitServiceDefault {
            plan_repo,
            account_repo,
            account_workers_repo,
            account_uploads_repo,
            project_repo,
            template_repo,
            account_fuel_repo,
        }
    }

    async fn get_plan(&self, account_id: &AccountId) -> Result<Plan, PlanLimitError> {
        if let Some(account) = self.account_repo.get(&account_id.value).await? {
            if let Some(plan) = self.plan_repo.get(&account.plan_id).await? {
                Ok(plan.into())
            } else {
                Err(PlanLimitError::AccountIdNotFound(account_id.clone()))
            }
        } else {
            Err(PlanLimitError::AccountIdNotFound(account_id.clone()))
        }
    }

    async fn get_account_id(&self, project_id: &ProjectId) -> Result<AccountId, PlanLimitError> {
        if let Some(project) = self.project_repo.get(&project_id.0).await? {
            Ok(AccountId {
                value: project.owner_account_id,
            })
        } else {
            Err(PlanLimitError::ProjectIdNotFound(project_id.clone()))
        }
    }

    async fn get_project_id(&self, template_id: &TemplateId) -> Result<ProjectId, PlanLimitError> {
        if let Some(template) = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?
        {
            Ok(ProjectId(template.project_id))
        } else {
            Err(PlanLimitError::TemplateIdNotFound(template_id.clone()))
        }
    }

    fn check_authorization(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError> {
        if auth.has_account_or_role(account_id, &Role::Admin) {
            Ok(())
        } else {
            Err(PlanLimitError::Unauthorized(
                "Insufficient privilege.".to_string(),
            ))
        }
    }
}

#[derive(Default)]
pub struct PlanLimitServiceNoOp {}

#[async_trait]
impl PlanLimitService for PlanLimitServiceNoOp {
    async fn get_account_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<LimitResult, PlanLimitError> {
        Ok(LimitResult {
            account_id: account_id.clone(),
            plan: Plan::default(),
        })
    }

    async fn get_project_limits(
        &self,
        _project_id: &ProjectId,
    ) -> Result<LimitResult, PlanLimitError> {
        Ok(LimitResult {
            account_id: AccountId::from(""),
            plan: Plan::default(),
        })
    }

    async fn get_template_limits(
        &self,
        _template_id: &TemplateId,
    ) -> Result<LimitResult, PlanLimitError> {
        Ok(LimitResult {
            account_id: AccountId::from(""),
            plan: Plan::default(),
        })
    }

    async fn check_project_limit(
        &self,
        _account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        Ok(CheckLimitResult {
            account_id: AccountId::from(""),
            count: 0,
            limit: 0,
        })
    }

    async fn check_template_limit(
        &self,
        _project_id: &ProjectId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        Ok(CheckLimitResult {
            account_id: AccountId::from(""),
            count: 0,
            limit: 0,
        })
    }

    async fn check_worker_limit(
        &self,
        _template_id: &TemplateId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        Ok(CheckLimitResult {
            account_id: AccountId::from(""),
            count: 0,
            limit: 0,
        })
    }

    async fn check_upload_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        Ok(CheckLimitResult {
            account_id: account_id.clone(),
            count: 0,
            limit: 0,
        })
    }

    async fn check_storage_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<CheckLimitResult, PlanLimitError> {
        Ok(CheckLimitResult {
            account_id: account_id.clone(),
            count: 0,
            limit: 0,
        })
    }

    async fn get_resource_limits(
        &self,
        _account_id: &AccountId,
        _auth: &AccountAuthorisation,
    ) -> Result<ResourceLimits, PlanLimitError> {
        Ok(ResourceLimits {
            available_fuel: 0,
            max_memory_per_worker: 0,
        })
    }

    async fn record_fuel_consumption(
        &self,
        _updates: HashMap<AccountId, i64>,
        _auth: &AccountAuthorisation,
    ) -> Result<(), PlanLimitError> {
        Ok(())
    }
}
