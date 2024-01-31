use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::PlanId;
use tracing::info;

use crate::config::PlansConfig;
use crate::model::{Plan, PlanData};
use crate::repo::plan::{PlanRecord, PlanRepo};
use crate::repo::RepoError;

#[derive(Debug, Clone)]
pub enum PlanError {
    Internal(String),
}

impl Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl PlanError {
    pub fn internal<T: Display>(error: T) -> Self {
        PlanError::Internal(error.to_string())
    }
}

impl From<RepoError> for PlanError {
    fn from(error: RepoError) -> Self {
        PlanError::internal(error)
    }
}

#[async_trait]
pub trait PlanService {
    async fn create_initial_plan(&self) -> Result<Plan, PlanError>;

    async fn get_default_plan(&self) -> Result<Plan, PlanError>;

    async fn get(&self, plan_id: &PlanId) -> Result<Option<Plan>, PlanError>;
}

pub struct PlanServiceDefault {
    plan_repo: Arc<dyn PlanRepo + Sync + Send>,
    plans_config: PlansConfig,
}

impl PlanServiceDefault {
    pub fn new(plan_repo: Arc<dyn PlanRepo + Sync + Send>, plans_config: PlansConfig) -> Self {
        PlanServiceDefault {
            plan_repo,
            plans_config,
        }
    }
}

#[async_trait]
impl PlanService for PlanServiceDefault {
    async fn create_initial_plan(&self) -> Result<Plan, PlanError> {
        let default_plan: Plan = self.plans_config.default.clone().into();

        info!("Create initial plan {}", default_plan.plan_id);

        let record: PlanRecord = default_plan.clone().into();

        self.plan_repo.update(&record).await?;

        Ok(default_plan)
    }

    async fn get_default_plan(&self) -> Result<Plan, PlanError> {
        let plan_id = self.plans_config.default.plan_id;

        info!("Getting default plan {}", plan_id);

        let plan = self.plan_repo.get(&plan_id).await?;

        match plan {
            Some(plan) => Ok(plan.into()),
            None => Err(PlanError::Internal("Could not find default plan".into())),
        }
    }

    async fn get(&self, plan_id: &PlanId) -> Result<Option<Plan>, PlanError> {
        info!("Getting plan {}", plan_id);
        let result = self.plan_repo.get(&plan_id.0).await?;
        Ok(result.map(|p| p.into()))
    }
}

#[derive(Default)]
pub struct PlanServiceNoOp {}

#[async_trait]
impl PlanService for PlanServiceNoOp {
    async fn create_initial_plan(&self) -> Result<Plan, PlanError> {
        Ok(Plan::default())
    }

    async fn get_default_plan(&self) -> Result<Plan, PlanError> {
        Ok(Plan::default())
    }

    async fn get(&self, plan_id: &PlanId) -> Result<Option<Plan>, PlanError> {
        Ok(Some(Plan {
            plan_id: plan_id.clone(),
            plan_data: PlanData::default(),
        }))
    }
}
