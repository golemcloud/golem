use std::sync::Arc;

use golem_worker_executor_base::services::HasExtraDeps;

use crate::context::Context;
use crate::services::config::AdditionalGolemConfig;

pub mod config;

pub trait HasAdditionalGolemConfig {
    fn additional_golem_config(&self) -> Arc<config::AdditionalGolemConfig>;
}

#[derive(Clone)]
pub struct AdditionalDeps {
    additional_golem_config: Arc<AdditionalGolemConfig>,
}

impl AdditionalDeps {
    pub fn new(additional_golem_config: Arc<AdditionalGolemConfig>) -> Self {
        Self {
            additional_golem_config,
        }
    }

    #[cfg(test)]
    #[allow(unused)]
    pub async fn mocked() -> Self {
        Self {
            additional_golem_config: Arc::new(AdditionalGolemConfig::new()),
        }
    }
}

impl HasAdditionalGolemConfig for AdditionalDeps {
    fn additional_golem_config(&self) -> Arc<AdditionalGolemConfig> {
        self.additional_golem_config.clone()
    }
}

impl<T: HasExtraDeps<Context>> HasAdditionalGolemConfig for T {
    fn additional_golem_config(&self) -> Arc<AdditionalGolemConfig> {
        self.extra_deps().additional_golem_config.clone()
    }
}
