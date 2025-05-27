use crate::config::PlansConfig;
use crate::model::Plan;
use crate::repo::plan::{PlanRecord, PlanRepo};
use async_trait::async_trait;
use cloud_common::model::PlanId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("Could not find default plan with id: {0}")]
    CouldNotFindDefaultPlan(Uuid),
    #[error("Internal error: {0}")]
    InternalRepoError(#[from] RepoError),
}

impl SafeDisplay for PlanError {
    fn to_safe_string(&self) -> String {
        match self {
            PlanError::CouldNotFindDefaultPlan(_) => self.to_string(),
            PlanError::InternalRepoError(inner) => inner.to_safe_string(),
        }
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
            None => Err(PlanError::CouldNotFindDefaultPlan(plan_id)),
        }
    }

    async fn get(&self, plan_id: &PlanId) -> Result<Option<Plan>, PlanError> {
        info!("Getting plan {}", plan_id);
        let result = self.plan_repo.get(&plan_id.0).await?;
        Ok(result.map(|p| p.into()))
    }
}
