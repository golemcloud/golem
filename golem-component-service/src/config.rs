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

use golem_common::config::{ConfigExample, ConfigLoader, DbConfig, HasConfigExamples};
use golem_common::model::{Empty, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_common::SafeDisplay;
use golem_service_base::clients::RemoteServiceConfig;
use golem_service_base::config::BlobStorageConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceConfig {
    pub tracing: TracingConfig,
    pub environment: String,
    pub workspace: String,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub compilation: ComponentCompilationConfig,
    pub cloud_service: RemoteServiceConfig,
    pub blob_storage: BlobStorageConfig,
    pub plugin_transformations: PluginTransformationsConfig,
    pub cors_origin_regex: String,
}

impl SafeDisplay for ComponentServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "environment: {}", self.environment);
        let _ = writeln!(&mut result, "workspace: {}", self.workspace);
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);
        let _ = writeln!(&mut result, "gRPC port: {}", self.grpc_port);
        let _ = writeln!(&mut result, "DB:");
        let _ = writeln!(&mut result, "{}", self.db.to_safe_string_indented());
        let _ = writeln!(&mut result, "compilation:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.compilation.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "cloud service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.cloud_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "blob storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.blob_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "plugin transformations:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.plugin_transformations.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "CORS origin regex: {}", self.cors_origin_regex);

        result
    }
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
            cloud_service: RemoteServiceConfig::default(),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentCompilationConfig {
    Enabled(ComponentCompilationEnabledConfig),
    Disabled(Empty),
}

impl SafeDisplay for ComponentCompilationConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            ComponentCompilationConfig::Enabled(inner) => {
                let _ = writeln!(&mut result, "enabled:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            ComponentCompilationConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

impl Default for ComponentCompilationConfig {
    fn default() -> Self {
        Self::Enabled(ComponentCompilationEnabledConfig {
            host: "localhost".to_string(),
            port: 9091,
            retries: RetryConfig::default(),
            connect_timeout: Duration::from_secs(10),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
}

impl SafeDisplay for ComponentCompilationEnabledConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "connect timeout: {:?}", self.connect_timeout);
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
}

impl ComponentCompilationEnabledConfig {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build ComponentCompilationService URI")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PluginTransformationsConfig {
    pub retries: RetryConfig,
}

impl SafeDisplay for PluginTransformationsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
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
