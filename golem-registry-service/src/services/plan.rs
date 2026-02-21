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

use crate::config::PrecreatedPlan;
use crate::repo::model::plan::PlanRecord;
use crate::repo::plan::PlanRepo;
use golem_common::model::plan::{Plan, PlanId};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::PlanAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("Plan not found for id {0}")]
    PlanNotFound(PlanId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for PlanError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::PlanNotFound(_) => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(PlanError, RepoError);

pub struct PlanService {
    plan_repo: Arc<dyn PlanRepo>,
}

impl PlanService {
    pub fn new(plan_repo: Arc<dyn PlanRepo>) -> Self {
        Self { plan_repo }
    }

    pub async fn create_initial_plans(
        &self,
        plans: &HashMap<String, PrecreatedPlan>,
    ) -> Result<(), PlanError> {
        for (name, plan) in plans {
            let existing_plan = self.get(&plan.plan_id, &AuthCtx::System).await;

            let needs_update = match existing_plan {
                Ok(existing_plan) => {
                    let needs_update = existing_plan.app_limit != plan.app_limit
                        || existing_plan.env_limit != plan.env_limit
                        || existing_plan.component_limit != plan.component_limit
                        || existing_plan.storage_limit != plan.storage_limit
                        || existing_plan.worker_limit != plan.worker_limit
                        || existing_plan.monthly_gas_limit != plan.monthly_gas_limit
                        || existing_plan.monthly_upload_limit != plan.app_limit
                        || existing_plan.max_memory_per_worker != plan.max_memory_per_worker;

                    if needs_update {
                        info!("Updating initial plan {}", plan.plan_id);
                    };

                    needs_update
                }
                Err(PlanError::PlanNotFound(_)) => {
                    info!("Creating initial plan {} with id {}", name, plan.plan_id);
                    true
                }
                Err(other) => Err(other)?,
            };

            if needs_update {
                self.create_or_update_plan(
                    Plan {
                        plan_id: plan.plan_id,
                        name: plan.plan_name.clone(),
                        app_limit: plan.app_limit,
                        env_limit: plan.env_limit,
                        component_limit: plan.component_limit,
                        worker_limit: plan.worker_limit,
                        worker_connection_limit: plan.worker_connection_limit,
                        storage_limit: plan.storage_limit,
                        monthly_gas_limit: plan.monthly_gas_limit,
                        monthly_upload_limit: plan.monthly_upload_limit,
                        max_memory_per_worker: plan.max_memory_per_worker,
                    },
                    &AuthCtx::System,
                )
                .await?;
            }
        }

        Ok(())
    }

    pub async fn get(&self, plan_id: &PlanId, auth: &AuthCtx) -> Result<Plan, PlanError> {
        auth.authorize_plan_action(plan_id, PlanAction::ViewPlan)
            .map_err(|_| PlanError::PlanNotFound(*plan_id))?;

        debug!("Getting plan {}", plan_id);

        let result = self
            .plan_repo
            .get_by_id(plan_id.0)
            .await?
            .ok_or(PlanError::PlanNotFound(*plan_id))?;

        Ok(result.into())
    }

    async fn create_or_update_plan(&self, plan: Plan, auth: &AuthCtx) -> Result<(), PlanError> {
        auth.authorize_plan_action(&plan.plan_id, PlanAction::CreateOrUpdatePlan)?;

        let record: PlanRecord = PlanRecord {
            name: plan.name.0,
            plan_id: plan.plan_id.0,
            max_memory_per_worker: plan.max_memory_per_worker.into(),
            total_app_count: plan.app_limit.into(),
            total_env_count: plan.env_limit.into(),
            total_component_count: plan.component_limit.into(),
            total_component_storage_bytes: plan.storage_limit.into(),
            total_worker_count: plan.worker_limit.into(),
            total_worker_connection_count: plan.worker_connection_limit.into(),
            monthly_component_upload_limit_bytes: plan.monthly_upload_limit.into(),
            monthly_gas_limit: plan.monthly_gas_limit.into(),
        };

        self.plan_repo.create_or_update(record).await?;

        Ok(())
    }
}
