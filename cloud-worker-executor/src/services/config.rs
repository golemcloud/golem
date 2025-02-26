use std::path::PathBuf;
use std::time::Duration;

use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::model::RetryConfig;
use golem_service_base::config::MergedConfigLoaderOrDumper;
use golem_worker_executor_base::services::golem_config::{make_config_loader, GolemConfig};
use http::Uri;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct AdditionalGolemConfig {
    pub resource_limits: ResourceLimitsConfig,
    pub component_service: CloudComponentServiceConfig,
    pub component_cache: CloudComponentCacheConfig,
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
    pub fn url(&self) -> Url {
        build_url("resource limits", &self.host, self.port)
    }

    pub fn uri(&self) -> Uri {
        build_uri("resource limits", &self.host, self.port)
    }
}

impl HasConfigExamples<AdditionalGolemConfig> for AdditionalGolemConfig {
    fn examples() -> Vec<ConfigExample<AdditionalGolemConfig>> {
        vec![(
            "with disabled resource limits",
            Self {
                resource_limits: ResourceLimitsConfig::Disabled,
                component_cache: CloudComponentCacheConfig::default(),
                component_service: CloudComponentServiceConfig::default(),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudComponentServiceConfig {
    pub host: String,
    pub port: u16,
    pub project_host: String,
    pub project_port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
    pub max_component_size: usize,
}

impl CloudComponentServiceConfig {
    pub fn component_url(&self) -> Url {
        build_url("component", &self.project_host, self.project_port)
    }

    pub fn component_uri(&self) -> Uri {
        build_uri("component", &self.project_host, self.project_port)
    }

    pub fn project_url(&self) -> Url {
        build_url("project", &self.project_host, self.project_port)
    }

    pub fn project_uri(&self) -> Uri {
        build_uri("project", &self.project_host, self.project_port)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudComponentCacheConfig {
    pub max_capacity: usize,
    pub max_metadata_capacity: usize,
    pub max_resolved_component_capacity: usize,
    pub max_resolved_project_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl Default for CloudComponentCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 32,
            max_metadata_capacity: 16384,
            max_resolved_component_capacity: 1024,
            max_resolved_project_capacity: 1024,
            time_to_idle: Duration::from_secs(12 * 60 * 60),
        }
    }
}

impl Default for CloudComponentServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9090,
            project_host: "localhost".to_string(),
            project_port: 9091,
            access_token: "2a354594-7a63-4091-a46b-cc58d379f677".to_string(),
            retries: RetryConfig::max_attempts_3(),
            max_component_size: 50 * 1024 * 1024,
        }
    }
}

pub fn make_additional_config_loader() -> ConfigLoader<AdditionalGolemConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/worker-executor.toml"))
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

fn build_url(name: &str, host: &str, port: u16) -> Url {
    Url::parse(&format!("http://{}:{}", host, port))
        .unwrap_or_else(|_| panic!("Failed to parse {name} service URL"))
}

fn build_uri(name: &str, host: &str, port: u16) -> Uri {
    Uri::builder()
        .scheme("http")
        .authority(format!("{}:{}", host, port).as_str())
        .path_and_query("/")
        .build()
        .unwrap_or_else(|_| panic!("Failed to build {name} service URI"))
}

#[cfg(test)]
mod tests {
    use test_r::test;

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
