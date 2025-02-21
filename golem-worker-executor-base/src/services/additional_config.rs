// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::PathBuf;
use std::time::Duration;

use super::golem_config::{make_config_loader, GolemConfig};
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::model::RetryConfig;
use golem_service_base::config::MergedConfigLoaderOrDumper;
use http::Uri;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DefaultAdditionalGolemConfig {
    pub component_service: ComponentServiceConfig,
    pub component_cache: ComponentCacheConfig,
}

impl HasConfigExamples<DefaultAdditionalGolemConfig> for DefaultAdditionalGolemConfig {
    fn examples() -> Vec<ConfigExample<DefaultAdditionalGolemConfig>> {
        vec![(
            "with disabled resource limits",
            Self {
                component_cache: ComponentCacheConfig::default(),
                component_service: ComponentServiceConfig::default(),
            },
        )]
    }
}

pub fn make_additional_config_loader() -> ConfigLoader<DefaultAdditionalGolemConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/worker-executor.toml"))
}

pub fn load_or_dump_config() -> Option<(GolemConfig, DefaultAdditionalGolemConfig)> {
    MergedConfigLoaderOrDumper::new("golem-config", make_config_loader())
        .add(
            "additional-golem-config",
            make_additional_config_loader(),
            |base, additional| (base, additional),
        )
        .finish()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCacheConfig {
    pub max_capacity: usize,
    pub max_metadata_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl Default for ComponentCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 32,
            max_metadata_capacity: 16384,
            time_to_idle: Duration::from_secs(12 * 60 * 60),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentServiceConfig {
    Local(ComponentServiceLocalConfig),
    Grpc(ComponentServiceGrpcConfig),
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self::Grpc(ComponentServiceGrpcConfig::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceLocalConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
    pub max_component_size: usize,
}

impl ComponentServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse component service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build component service URI")
    }
}

impl Default for ComponentServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9090,
            access_token: "2a354594-7a63-4091-a46b-cc58d379f677".to_string(),
            retries: RetryConfig::max_attempts_3(),
            max_component_size: 50 * 1024 * 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::services::additional_config::{load_or_dump_config, make_additional_config_loader};
    use crate::services::golem_config::make_config_loader;

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
