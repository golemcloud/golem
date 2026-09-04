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

use crate::config::PrecreatedPlan;
use crate::repo::model::plan::PlanRecord;
use crate::repo::plan::PlanRepo;
use golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT;
use golem_common::model::card::owner::EmptyOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, PermissionTarget, PlanIdPattern, PlanResourcePattern, PlanVerb,
};
use golem_common::model::plan::{Plan, PlanId};
use golem_common::{SafeDisplay, error_forwarding};
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
    #[error("Invalid plan policy: {0}")]
    InvalidPolicy(String),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for PlanError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::PlanNotFound(_) | Self::InvalidPolicy(_) => self.to_string(),
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
            validate_plan_policy(plan)?;
            let desired_plan = Plan {
                plan_id: plan.plan_id,
                name: plan.plan_name.clone(),
                app_limit: plan.app_limit,
                env_limit: plan.env_limit,
                component_limit: plan.component_limit,
                worker_connection_limit: plan.worker_connection_limit,
                storage_limit: plan.storage_limit,
                monthly_gas_limit: plan.monthly_gas_limit,
                monthly_upload_limit: plan.monthly_upload_limit,
                max_memory_per_agent: plan.max_memory_per_agent,
                max_memory_per_agent_ceiling: plan.max_memory_per_agent_ceiling,
                max_memory_per_agent_user_configurable: plan.max_memory_per_agent_user_configurable,
                monthly_memory_gb_seconds: plan.monthly_memory_gb_seconds,
                monthly_memory_gb_seconds_ceiling: plan.monthly_memory_gb_seconds_ceiling,
                monthly_memory_gb_seconds_user_configurable: plan
                    .monthly_memory_gb_seconds_user_configurable,
                max_table_elements_per_worker: plan.max_table_elements_per_worker,
                max_storage_per_agent_enabled: plan.max_storage_per_agent_enabled,
                max_storage_per_agent: plan.max_storage_per_agent,
                max_storage_per_agent_ceiling: plan.resolved_max_storage_per_agent_ceiling(),
                max_storage_per_agent_user_configurable: plan
                    .max_storage_per_agent_user_configurable,
                per_invocation_http_call_limit: plan.per_invocation_http_call_limit,
                per_invocation_rpc_call_limit: plan.per_invocation_rpc_call_limit,
                monthly_http_call_limit: plan.monthly_http_call_limit,
                monthly_rpc_call_limit: plan.monthly_rpc_call_limit,
                max_concurrent_agents_per_executor: plan.max_concurrent_agents_per_executor,
                oplog_writes_per_second: plan.oplog_writes_per_second,
            };

            let needs_update = match self.get(&plan.plan_id, &AuthCtx::System).await {
                Ok(existing_plan) => {
                    // Comparing the whole record keeps reconciliation exhaustive: every
                    // field written below is also a field that can trigger an update, so
                    // a newly added plan limit is covered without touching this line.
                    let needs_update = existing_plan != desired_plan;

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
                self.create_or_update_plan(desired_plan, &AuthCtx::System)
                    .await?;
            }
        }

        Ok(())
    }

    pub async fn get(&self, plan_id: &PlanId, auth: &AuthCtx) -> Result<Plan, PlanError> {
        authorize_plan_permission(auth, PlanVerb::View, plan_resource(*plan_id))
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
        authorize_plan_permission(auth, PlanVerb::Update, plan_resource(plan.plan_id))?;

        let record: PlanRecord = PlanRecord {
            name: plan.name.0,
            plan_id: plan.plan_id.0,
            max_memory_per_worker: plan.max_memory_per_agent.into(),
            max_memory_per_worker_ceiling: plan.max_memory_per_agent_ceiling.into(),
            max_memory_per_worker_user_configurable: plan.max_memory_per_agent_user_configurable,
            monthly_memory_gb_seconds: plan.monthly_memory_gb_seconds.into(),
            monthly_memory_gb_seconds_ceiling: plan.monthly_memory_gb_seconds_ceiling.into(),
            monthly_memory_gb_seconds_user_configurable: plan
                .monthly_memory_gb_seconds_user_configurable,
            max_table_elements_per_worker: plan.max_table_elements_per_worker.into(),
            max_disk_space_per_worker_enabled: plan.max_storage_per_agent_enabled,
            max_disk_space_per_worker: plan.max_storage_per_agent.into(),
            max_disk_space_per_worker_ceiling: plan.max_storage_per_agent_ceiling.into(),
            max_disk_space_per_worker_user_configurable: plan
                .max_storage_per_agent_user_configurable,
            max_concurrent_agents_per_executor: plan.max_concurrent_agents_per_executor.into(),
            total_app_count: plan.app_limit.into(),
            total_env_count: plan.env_limit.into(),
            total_component_count: plan.component_limit.into(),
            total_component_storage_bytes: plan.storage_limit.into(),
            total_worker_connection_count: plan.worker_connection_limit.into(),
            monthly_component_upload_limit_bytes: plan.monthly_upload_limit.into(),
            monthly_gas_limit: plan.monthly_gas_limit.into(),
            per_invocation_http_call_limit: plan.per_invocation_http_call_limit.into(),
            per_invocation_rpc_call_limit: plan.per_invocation_rpc_call_limit.into(),
            monthly_http_call_limit: plan.monthly_http_call_limit.into(),
            monthly_rpc_call_limit: plan.monthly_rpc_call_limit.into(),
            oplog_writes_per_second: plan.oplog_writes_per_second.into(),
        };

        self.plan_repo.create_or_update(record).await?;

        Ok(())
    }
}

fn validate_plan_policy(plan: &PrecreatedPlan) -> Result<(), PlanError> {
    if plan.max_memory_per_agent > plan.max_memory_per_agent_ceiling {
        return Err(PlanError::InvalidPolicy(
            "maximum memory per agent default exceeds its ceiling".to_string(),
        ));
    }

    if plan.max_storage_per_agent_enabled {
        if plan.max_storage_per_agent == 0 {
            return Err(PlanError::InvalidPolicy(
                "maximum storage per agent default must be greater than zero".to_string(),
            ));
        }
        if plan.max_storage_per_agent >= EFFECTIVELY_UNLIMITED_STORAGE_LIMIT {
            return Err(PlanError::InvalidPolicy(format!(
                "maximum storage per agent default must be below {EFFECTIVELY_UNLIMITED_STORAGE_LIMIT}"
            )));
        }
        if plan.resolved_max_storage_per_agent_ceiling() >= EFFECTIVELY_UNLIMITED_STORAGE_LIMIT {
            return Err(PlanError::InvalidPolicy(format!(
                "maximum storage per agent ceiling must be below {EFFECTIVELY_UNLIMITED_STORAGE_LIMIT}"
            )));
        }
        if plan.max_storage_per_agent > plan.resolved_max_storage_per_agent_ceiling() {
            return Err(PlanError::InvalidPolicy(
                "maximum storage per agent default exceeds its ceiling".to_string(),
            ));
        }
    }

    Ok(())
}

fn authorize_plan_permission(
    auth: &AuthCtx,
    verb: PlanVerb,
    resource: PlanResourcePattern,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::Plan(ClassPermissionTarget {
        verb: Some(verb),
        owner: EmptyOwnerPattern,
        resource,
    }))
}

fn plan_resource(plan_id: PlanId) -> PlanResourcePattern {
    PlanResourcePattern::Plan(PlanIdPattern::PlanId(plan_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RegistryServiceConfig;
    use crate::repo::model::plan::PlanRecord;
    use async_trait::async_trait;
    use golem_service_base::repo::RepoResult;
    use std::sync::Mutex;
    use test_r::test;
    use uuid::Uuid;

    #[derive(Default)]
    struct InMemoryPlanRepo {
        plans: Mutex<HashMap<Uuid, PlanRecord>>,
    }

    #[async_trait]
    impl PlanRepo for InMemoryPlanRepo {
        async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()> {
            self.plans.lock().unwrap().insert(plan.plan_id, plan);
            Ok(())
        }

        async fn get_by_id(&self, plan_id: Uuid) -> RepoResult<Option<PlanRecord>> {
            Ok(self.plans.lock().unwrap().get(&plan_id).cloned())
        }

        async fn list(&self) -> RepoResult<Vec<PlanRecord>> {
            Ok(self.plans.lock().unwrap().values().cloned().collect())
        }
    }

    #[test]
    async fn initial_plan_preserves_storage_capability() {
        let repo = Arc::new(InMemoryPlanRepo::default());
        let service = PlanService::new(repo.clone());
        let mut plan = RegistryServiceConfig::default()
            .initial_plans
            .remove("default")
            .unwrap();
        let plan_id = plan.plan_id;

        plan.max_storage_per_agent_enabled = true;
        plan.max_storage_per_agent = 5;
        plan.max_storage_per_agent_ceiling = Some(20);
        service
            .create_initial_plans(&HashMap::from([("test".to_string(), plan.clone())]))
            .await
            .unwrap();
        assert!(
            repo.get_by_id(plan_id.0)
                .await
                .unwrap()
                .unwrap()
                .max_disk_space_per_worker_enabled
        );

        plan.max_storage_per_agent_enabled = false;
        service
            .create_initial_plans(&HashMap::from([("test".to_string(), plan)]))
            .await
            .unwrap();
        assert!(
            !repo
                .get_by_id(plan_id.0)
                .await
                .unwrap()
                .unwrap()
                .max_disk_space_per_worker_enabled
        );
    }

    #[test]
    fn plan_policy_requires_coherent_memory_range() {
        let mut plan = RegistryServiceConfig::default()
            .initial_plans
            .remove("default")
            .unwrap();
        plan.max_memory_per_agent = 11;
        plan.max_memory_per_agent_ceiling = 10;

        assert!(matches!(
            validate_plan_policy(&plan),
            Err(PlanError::InvalidPolicy(message))
                if message == "maximum memory per agent default exceeds its ceiling"
        ));

        plan.max_memory_per_agent_ceiling = 11;
        assert!(validate_plan_policy(&plan).is_ok());
    }

    #[test]
    fn enabled_storage_policy_must_be_finite_and_coherent() {
        let mut plan = RegistryServiceConfig::default()
            .initial_plans
            .remove("default")
            .unwrap();
        plan.max_storage_per_agent_enabled = true;
        plan.max_storage_per_agent = 1;
        plan.max_storage_per_agent_ceiling = Some(1);
        assert!(validate_plan_policy(&plan).is_ok());

        plan.max_storage_per_agent = 0;
        assert!(validate_plan_policy(&plan).is_err());

        plan.max_storage_per_agent = 3;
        plan.max_storage_per_agent_ceiling = Some(2);
        assert!(validate_plan_policy(&plan).is_err());

        plan.max_storage_per_agent = 1;
        plan.max_storage_per_agent_ceiling = Some(EFFECTIVELY_UNLIMITED_STORAGE_LIMIT);
        assert!(validate_plan_policy(&plan).is_err());

        plan.max_storage_per_agent = EFFECTIVELY_UNLIMITED_STORAGE_LIMIT;
        plan.max_storage_per_agent_ceiling = Some(EFFECTIVELY_UNLIMITED_STORAGE_LIMIT - 1);
        assert!(validate_plan_policy(&plan).is_err());
    }

    #[test]
    fn disabled_storage_policy_allows_internal_unlimited_values() {
        let plan = RegistryServiceConfig::default()
            .initial_plans
            .remove("default")
            .unwrap();
        assert!(!plan.max_storage_per_agent_enabled);
        assert!(validate_plan_policy(&plan).is_ok());
    }
}
