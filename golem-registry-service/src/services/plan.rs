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

use crate::config::PlansConfig;
use crate::repo::model::account_usage::UsageType;
use crate::repo::model::plan::PlanRecord;
use crate::repo::plan::PlanRepo;
use anyhow::anyhow;
use golem_common::model::PlanId;
use golem_common::model::account::Plan;
use golem_common::{SafeDisplay, error_forwarders};
use golem_service_base::repo::RepoError;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("Plan not found for id {0}")]
    PlanNotFound(PlanId),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for PlanError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::PlanNotFound(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarders!(PlanError, RepoError);

pub struct PlanService {
    plan_repo: Arc<dyn PlanRepo>,
    config: PlansConfig,
}

impl PlanService {
    pub fn new(plan_repo: Arc<dyn PlanRepo>, config: PlansConfig) -> Self {
        assert!(
            config.plans.contains_key("default"),
            "No default plan in precreated plans"
        );

        Self { plan_repo, config }
    }

    pub async fn create_initial_plans(&self) -> Result<(), PlanError> {
        for (name, plan) in &self.config.plans {
            let plan_id = PlanId(plan.plan_id);
            let existing_plan = self.get(&plan_id).await;

            let needs_update = match existing_plan {
                Ok(existing_plan) => {
                    let needs_update = existing_plan.app_limit != plan.app_limit
                        || existing_plan.env_limit != plan.env_limit
                        || existing_plan.component_limit != plan.component_limit
                        || existing_plan.storage_limit != plan.storage_limit
                        || existing_plan.worker_limit != plan.worker_limit
                        || existing_plan.monthly_gas_limit != plan.monthly_gas_limit
                        || existing_plan.monthly_upload_limit != plan.app_limit;

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
                let record: PlanRecord = PlanRecord {
                    name: name.clone(),
                    plan_id: plan.plan_id,
                    limits: BTreeMap::from_iter([
                        (UsageType::TotalAppCount, Some(plan.app_limit)),
                        (UsageType::TotalEnvCount, Some(plan.env_limit)),
                        (UsageType::TotalComponentCount, Some(plan.component_limit)),
                        (
                            UsageType::TotalComponentStorageBytes,
                            Some(plan.storage_limit),
                        ),
                        (UsageType::TotalWorkerCount, Some(plan.worker_limit)),
                        (UsageType::MonthlyGasLimit, Some(plan.monthly_gas_limit)),
                        (
                            UsageType::MonthlyComponentUploadLimitBytes,
                            Some(plan.monthly_upload_limit),
                        ),
                    ]),
                };

                self.plan_repo.create_or_update(record).await?;
            }
        }

        Ok(())
    }

    pub async fn get_default_plan(&self) -> Result<Plan, PlanError> {
        let plan_id = self.config.plans.get("default").unwrap().plan_id;

        debug!("Getting default plan {}", plan_id);

        let plan = self.plan_repo.get_by_id(&plan_id).await?;

        match plan {
            Some(plan) => Ok(plan.try_into()?),
            None => Err(anyhow!("Could not find default plan with id {plan_id}"))?,
        }
    }

    pub async fn get(&self, plan_id: &PlanId) -> Result<Plan, PlanError> {
        debug!("Getting plan {}", plan_id);

        let result = self
            .plan_repo
            .get_by_id(&plan_id.0)
            .await?
            .ok_or(PlanError::PlanNotFound(plan_id.clone()))?;

        Ok(result.try_into()?)
    }
}
