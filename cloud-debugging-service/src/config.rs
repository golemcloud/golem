use cloud_common::config::{MergedConfigLoaderOrDumper, RemoteCloudServiceConfig};
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_worker_executor_base::services::golem_config::{GolemConfig, ShardManagerServiceConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// A wrapper over golem config with a few custom behaviour
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugConfig {
    #[serde(flatten)]
    pub golem_config: GolemConfig,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            golem_config: GolemConfig {
                shard_manager_service: ShardManagerServiceConfig::SingleShard,
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

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct AdditionalDebugConfig {
    pub cloud_service: RemoteCloudServiceConfig,
}
impl HasConfigExamples<AdditionalDebugConfig> for AdditionalDebugConfig {
    fn examples() -> Vec<ConfigExample<AdditionalDebugConfig>> {
        vec![(
            "default-additional-config",
            AdditionalDebugConfig {
                cloud_service: RemoteCloudServiceConfig::default(),
            },
        )]
    }
}

pub fn load_or_dump_config() -> Option<(DebugConfig, AdditionalDebugConfig)> {
    MergedConfigLoaderOrDumper::new("golem-config", make_base_debug_config_loader())
        .add(
            "additional-golem-config",
            make_additional_config_loader(),
            |base, additional| (base, additional),
        )
        .finish()
}

fn make_base_debug_config_loader() -> ConfigLoader<DebugConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/debug-worker-executor.toml"))
}

fn make_additional_config_loader() -> ConfigLoader<AdditionalDebugConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/debug-worker-executor.toml"))
}
