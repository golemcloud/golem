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

use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::fmt::Write;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_common::SafeDisplay;
use golem_service_base::config::BlobStorageConfig;
use golem_worker_executor::services::golem_config::CompiledComponentServiceConfig;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    // Tracing
    pub tracing: TracingConfig,

    // Services
    pub component_service: ComponentServiceConfig,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub blob_storage: BlobStorageConfig,

    // Workers
    pub compile_worker: CompileWorkerConfig,

    // GRPC
    pub grpc_host: String,
    pub grpc_port: u16,

    // Metrics and healthcheck
    pub http_host: String,
    pub http_port: u16,
}

impl SafeDisplay for ServerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "component service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "compiled component service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.compiled_component_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "blob storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.blob_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "compile worker:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.compile_worker.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "gRPC host: {}", self.grpc_host);
        let _ = writeln!(&mut result, "gRPC port: {}", self.grpc_port);
        let _ = writeln!(&mut result, "HTTP host: {}", self.http_host);
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);
        result
    }
}

impl ServerConfig {
    pub fn http_addr(&self) -> Option<SocketAddrV4> {
        let address = self.http_host.parse::<Ipv4Addr>().ok()?;
        Some(SocketAddrV4::new(address, self.http_port))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentServiceConfig {
    Static(StaticComponentServiceConfig),
    Dynamic(DynamicComponentServiceConfig),
}

impl ComponentServiceConfig {
    pub fn static_config(&self) -> Option<StaticComponentServiceConfig> {
        match self {
            ComponentServiceConfig::Static(config) => Some(config.clone()),
            ComponentServiceConfig::Dynamic(_) => None,
        }
    }
}

impl SafeDisplay for ComponentServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            ComponentServiceConfig::Static(inner) => {
                let _ = writeln!(&mut result, "static:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            ComponentServiceConfig::Dynamic(inner) => {
                let _ = writeln!(&mut result, "dynamic:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StaticComponentServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
}

impl StaticComponentServiceConfig {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build ComponentService URI")
    }
}

impl SafeDisplay for StaticComponentServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "access_token: ****");
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DynamicComponentServiceConfig {
    pub access_token: Uuid,
}

impl SafeDisplay for DynamicComponentServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "access_token: ****");
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompileWorkerConfig {
    pub retries: RetryConfig,
    pub max_component_size: usize,
    pub connect_timeout: Duration,
}

impl SafeDisplay for CompileWorkerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "max component size: {}",
            self.max_component_size
        );
        let _ = writeln!(&mut result, "connect timeout: {:?}", self.connect_timeout);
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("component-compilation-service"),
            component_service: Default::default(),
            compiled_component_service: Default::default(),
            blob_storage: BlobStorageConfig::default_local_file_system(),
            compile_worker: Default::default(),
            grpc_host: "0.0.0.0".to_string(),
            grpc_port: 9091,
            http_host: "0.0.0.0".to_string(),
            http_port: 8084,
        }
    }
}

impl HasConfigExamples<ServerConfig> for ServerConfig {
    fn examples() -> Vec<ConfigExample<Self>> {
        vec![(
            "with s3 blob storage and disabled compiled component service",
            Self {
                blob_storage: BlobStorageConfig::default_s3(),
                compiled_component_service: CompiledComponentServiceConfig::disabled(),
                ..Self::default()
            },
        )]
    }
}

impl Default for StaticComponentServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_owned(),
            port: 9090,
            access_token: Uuid::parse_str("5c832d93-ff85-4a8f-9803-513950fdfdb1")
                .expect("invalid UUID"),
        }
    }
}

impl Default for DynamicComponentServiceConfig {
    fn default() -> Self {
        Self {
            access_token: Uuid::parse_str("5c832d93-ff85-4a8f-9803-513950fdfdb1")
                .expect("invalid UUID"),
        }
    }
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self::Static(Default::default())
    }
}

impl Default for CompileWorkerConfig {
    fn default() -> Self {
        Self {
            retries: RetryConfig::max_attempts_3(),
            max_component_size: 1000000,
            connect_timeout: Duration::from_secs(10),
        }
    }
}

pub fn make_config_loader() -> ConfigLoader<ServerConfig> {
    ConfigLoader::new_with_examples(Path::new("config/component-compilation-service.toml"))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;
    use test_r::test;

    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        env::set_current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("Failed to set current directory");

        make_config_loader().load().expect("Failed to load config");
    }
}
