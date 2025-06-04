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

use cloud_common::config::RemoteCloudServiceConfig;
use golem_common::config::{ConfigExample, ConfigLoader, DbConfig, HasConfigExamples};
use golem_common::tracing::TracingConfig;
use golem_component_service_base::config::{
    ComponentCompilationConfig, PluginTransformationsConfig,
};
use golem_service_base::config::BlobStorageConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceConfig {
    pub tracing: TracingConfig,
    pub environment: String,
    pub workspace: String,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub compilation: ComponentCompilationConfig,
    pub cloud_service: RemoteCloudServiceConfig,
    pub blob_storage: BlobStorageConfig,
    pub plugin_transformations: PluginTransformationsConfig,
    pub cors_origin_regex: String,
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("component-service"),
            environment: "dev".to_string(),
            workspace: "release".to_string(),
            http_port: 8081,
            grpc_port: 9091,
            db: DbConfig::default(),
            compilation: ComponentCompilationConfig::default(),
            cloud_service: RemoteCloudServiceConfig::default(),
            blob_storage: BlobStorageConfig::default(),
            plugin_transformations: PluginTransformationsConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
        }
    }
}

impl HasConfigExamples<ComponentServiceConfig> for ComponentServiceConfig {
    fn examples() -> Vec<ConfigExample<ComponentServiceConfig>> {
        vec![]
    }
}

pub fn make_config_loader() -> ConfigLoader<ComponentServiceConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/component-service.toml"))
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }

    #[test]
    pub fn compilation_can_be_disabled() {
        std::env::set_var("GOLEM__COMPILATION__TYPE", "Disabled");
        let cfg = make_config_loader().load().expect("Failed to load config");
        std::env::remove_var("GOLEM__COMPILATION__TYPE");

        assert!(matches!(
            cfg.compilation,
            super::ComponentCompilationConfig::Disabled(_)
        ));
    }
}
