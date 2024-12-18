use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_worker_executor_base::services::golem_config::{GolemConfig, ShardManagerServiceConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugConfig {
    pub golem_config: GolemConfig,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            golem_config: GolemConfig {
                shard_manager_service: ShardManagerServiceConfig::Disabled,
                ..GolemConfig::default()
            },
        }
    }
}

impl HasConfigExamples<DebugConfig> for DebugConfig {
    fn examples() -> Vec<ConfigExample<DebugConfig>> {
        vec![("default-debug-config", DebugConfig::default())]
    }
}

pub fn load_or_dump_config() -> ConfigLoader<DebugConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/debug-worker-executor.toml"))
}
