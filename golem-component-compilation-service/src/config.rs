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

use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::model::{Empty, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_common::SafeDisplay;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use golem_service_base::service::compiled_component::CompiledComponentServiceConfig;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::fmt::Write;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    // Tracing
    pub tracing: TracingConfig,

    // Services
    pub registry_service: RegistryServiceConfig,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub blob_storage: BlobStorageConfig,

    // Workers
    pub compile_worker: CompileWorkerConfig,

    pub grpc: GrpcApiConfig,

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
            self.registry_service.to_safe_string_indented()
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

        let _ = writeln!(&mut result, "grpc");
        let _ = writeln!(&mut result, "{}", self.grpc.to_safe_string_indented());

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

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("component-compilation-service"),
            registry_service: Default::default(),
            compiled_component_service: Default::default(),
            blob_storage: BlobStorageConfig::default_local_file_system(),
            compile_worker: Default::default(),
            grpc: GrpcApiConfig::default(),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    pub port: u16,
    pub tls: GrpcServerTlsConfig,
}

impl SafeDisplay for GrpcApiConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "port: {}", self.port);

        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());

        result
    }
}

impl Default for GrpcApiConfig {
    fn default() -> Self {
        Self {
            port: 9091,
            tls: GrpcServerTlsConfig::disabled(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum RegistryServiceConfig {
    Static(StaticRegistryServiceConfig),
    Dynamic(Empty),
}

impl RegistryServiceConfig {
    pub fn static_config(&self) -> Option<StaticRegistryServiceConfig> {
        match self {
            RegistryServiceConfig::Static(config) => Some(config.clone()),
            RegistryServiceConfig::Dynamic(_) => None,
        }
    }
}

impl SafeDisplay for RegistryServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            RegistryServiceConfig::Static(inner) => {
                let _ = writeln!(&mut result, "static:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            RegistryServiceConfig::Dynamic(_) => {
                let _ = writeln!(&mut result, "dynamic");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StaticRegistryServiceConfig {
    pub host: String,
    pub port: u16,
}

impl SafeDisplay for StaticRegistryServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompileWorkerConfig {
    pub max_message_size: usize,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl SafeDisplay for CompileWorkerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "max_message_size: {}", self.max_message_size);
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for StaticRegistryServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_owned(),
            port: 9090,
        }
    }
}

impl Default for RegistryServiceConfig {
    fn default() -> Self {
        Self::Static(Default::default())
    }
}

impl Default for CompileWorkerConfig {
    fn default() -> Self {
        Self {
            max_message_size: 1000000,
            client_config: GrpcClientConfig {
                retries_on_unavailable: RetryConfig::max_attempts_3(),
                connect_timeout: Duration::from_secs(10),
                ..Default::default()
            },
        }
    }
}

pub fn make_config_loader() -> ConfigLoader<ServerConfig> {
    ConfigLoader::new_with_examples(Path::new("config/component-compilation-service.toml"))
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }
}
