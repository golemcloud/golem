use async_trait::async_trait;
use golem_common::model::{AccountId, ComponentId, WorkerId};
use golem_service_base::clients::limit::{LimitError, LimitService};
use golem_service_base::model::ResourceLimits;

pub struct StubLimitService;

#[async_trait]
impl LimitService for StubLimitService {
    async fn update_component_limit(
        &self,
        _account_id: &AccountId,
        _component_id: &ComponentId,
        _count: i32,
        _size: i64,
    ) -> Result<(), LimitError> {
        Ok(())
    }

    async fn update_worker_limit(
        &self,
        _account_id: &AccountId,
        _worker_id: &WorkerId,
        _value: i32,
    ) -> Result<(), LimitError> {
        Ok(())
    }

    async fn update_worker_connection_limit(
        &self,
        _account_id: &AccountId,
        _worker_id: &WorkerId,
        _value: i32,
    ) -> Result<(), LimitError> {
        Ok(())
    }

    async fn get_resource_limits(
        &self,
        _account_id: &AccountId,
    ) -> Result<ResourceLimits, LimitError> {
        Ok(ResourceLimits {
            available_fuel: 1000,
            max_memory_per_worker: 1024 * 1024 * 1024, // 1 GB
        })
    }
}
