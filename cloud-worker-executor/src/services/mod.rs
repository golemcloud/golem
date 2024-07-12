use std::sync::Arc;

use crate::context::Context;
use golem_worker_executor_base::services::HasExtraDeps;

use crate::services::config::AdditionalGolemConfig;

pub mod config;
pub mod resource_limits;

pub trait HasAdditionalGolemConfig {
    fn additional_golem_config(&self) -> Arc<config::AdditionalGolemConfig>;
}

pub trait HasResourceLimits {
    fn resource_limits(&self) -> Arc<dyn resource_limits::ResourceLimits + Send + Sync>;
}

#[derive(Clone)]
pub struct AdditionalDeps {
    additional_golem_config: Arc<AdditionalGolemConfig>,
    resource_limits: Arc<dyn resource_limits::ResourceLimits + Send + Sync>,
}

impl AdditionalDeps {
    pub fn new(
        additional_golem_config: Arc<AdditionalGolemConfig>,
        resource_limits: Arc<dyn resource_limits::ResourceLimits + Send + Sync>,
    ) -> Self {
        Self {
            additional_golem_config,
            resource_limits,
        }
    }

    #[cfg(test)]
    #[allow(unused)]
    pub async fn mocked() -> Self {
        let resource_limits = Arc::new(resource_limits::ResourceLimitsMock::new());
        Self {
            additional_golem_config: Arc::new(AdditionalGolemConfig::default()),
            resource_limits,
        }
    }
}

impl HasAdditionalGolemConfig for AdditionalDeps {
    fn additional_golem_config(&self) -> Arc<AdditionalGolemConfig> {
        self.additional_golem_config.clone()
    }
}

impl HasResourceLimits for AdditionalDeps {
    fn resource_limits(&self) -> Arc<dyn resource_limits::ResourceLimits + Send + Sync> {
        self.resource_limits.clone()
    }
}

impl<T: HasExtraDeps<Context>> HasAdditionalGolemConfig for T {
    fn additional_golem_config(&self) -> Arc<AdditionalGolemConfig> {
        self.extra_deps().additional_golem_config.clone()
    }
}

impl<T: HasExtraDeps<Context>> HasResourceLimits for T {
    fn resource_limits(&self) -> Arc<dyn resource_limits::ResourceLimits + Send + Sync> {
        self.extra_deps().resource_limits.clone()
    }
}
