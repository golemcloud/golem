use std::time::Duration;

use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples, RetryConfig};
use golem_worker_executor_base::services::golem_config::{make_config_loader, GolemConfig};
use http::Uri;
use serde::{Deserialize, Serialize};
use url::Url;

use cloud_common::config::MergedConfigLoaderOrDumper;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct AdditionalGolemConfig {
    pub resource_limits: ResourceLimitsConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ResourceLimitsConfig {
    Grpc(ResourceLimitsGrpcConfig),
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLimitsGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub batch_update_interval: Duration,
}

impl ResourceLimitsGrpcConfig {
    #[allow(unused)]
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse resource limits service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build resource limits service URI")
    }
}

impl HasConfigExamples<AdditionalGolemConfig> for AdditionalGolemConfig {
    fn examples() -> Vec<ConfigExample<AdditionalGolemConfig>> {
        vec![(
            "with disabled resource limits",
            Self {
                resource_limits: ResourceLimitsConfig::Disabled,
            },
        )]
    }
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self::Grpc(ResourceLimitsGrpcConfig {
            host: "localhost".to_string(),
            port: 8080,
            access_token: "access_token".to_string(),
            retries: RetryConfig::default(),
            batch_update_interval: Duration::from_secs(60),
        })
    }
}

pub fn make_additional_config_loader() -> ConfigLoader<AdditionalGolemConfig> {
    ConfigLoader::new_with_examples("config/worker-executor.toml".to_string())
}

pub fn load_or_dump_config() -> Option<(GolemConfig, AdditionalGolemConfig)> {
    MergedConfigLoaderOrDumper::new("golem-config", make_config_loader())
        .add(
            "additional-golem-config",
            make_additional_config_loader(),
            |base, additional| (base, additional),
        )
        .finish()
}

#[cfg(test)]
mod tests {
    use golem_worker_executor_base::services::golem_config::make_config_loader;

    use crate::services::config::{load_or_dump_config, make_additional_config_loader};

    #[test]
    pub fn base_config_is_loadable() {
        make_config_loader()
            .load()
            .expect("Failed to load base config");
    }

    #[test]
    pub fn additional_config_is_loadable() {
        make_additional_config_loader()
            .load()
            .expect("Failed to load additional config");
    }

    #[test]
    pub fn merged_config_is_loadable() {
        load_or_dump_config().expect("Failed to load additional config");
    }
}
