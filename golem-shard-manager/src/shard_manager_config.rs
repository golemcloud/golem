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

use crate::model::Empty;
use crate::shard_manager_config::HealthCheckMode::K8s;
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples, RedisConfig};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_common::SafeDisplay;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardManagerConfig {
    pub tracing: TracingConfig,
    pub persistence: PersistenceConfig,
    pub worker_executors: WorkerExecutorServiceConfig,
    pub health_check: HealthCheckConfig,
    pub http_port: u16,
    pub grpc_port: u16,
    pub number_of_shards: usize,
    pub rebalance_threshold: f64,
}

impl SafeDisplay for ShardManagerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "persistence:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.persistence.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "worker executors:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.worker_executors.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "healthcheck:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.health_check.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);
        let _ = writeln!(&mut result, "gRPC port: {}", self.grpc_port);
        let _ = writeln!(&mut result, "number of shards: {}", self.number_of_shards);
        let _ = writeln!(
            &mut result,
            "rebalance threshold: {}",
            self.rebalance_threshold
        );
        result
    }
}

impl Default for ShardManagerConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("shard-manager"),
            persistence: PersistenceConfig::default(),
            worker_executors: WorkerExecutorServiceConfig::default(),
            health_check: HealthCheckConfig::default(),
            http_port: 8081,
            grpc_port: 9002,
            number_of_shards: 1024,
            rebalance_threshold: 0.1,
        }
    }
}

impl HasConfigExamples<ShardManagerConfig> for ShardManagerConfig {
    fn examples() -> Vec<ConfigExample<ShardManagerConfig>> {
        vec![(
            "with k8s healthcheck",
            Self {
                health_check: HealthCheckConfig {
                    delay: Duration::from_secs(1),
                    mode: K8s(HealthCheckK8sConfig {
                        namespace: "namespace".to_string(),
                    }),
                    silent: false,
                },
                ..Self::default()
            },
        )]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerExecutorServiceConfig {
    #[serde(with = "humantime_serde")]
    pub assign_shards_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub health_check_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub revoke_shards_timeout: Duration,
    pub retries: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
}

impl SafeDisplay for WorkerExecutorServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "assign shards timeout: {:?}",
            self.assign_shards_timeout
        );
        let _ = writeln!(
            &mut result,
            "health check timeout: {:?}",
            self.health_check_timeout
        );
        let _ = writeln!(
            &mut result,
            "revoke shards timeout: {:?}",
            self.revoke_shards_timeout
        );
        let _ = writeln!(&mut result, "connect timeout: {:?}", self.connect_timeout);
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        result
    }
}

impl Default for WorkerExecutorServiceConfig {
    fn default() -> Self {
        Self {
            assign_shards_timeout: Duration::from_secs(5),
            health_check_timeout: Duration::from_secs(2),
            revoke_shards_timeout: Duration::from_secs(5),
            retries: RetryConfig::max_attempts_5(),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(with = "humantime_serde")]
    pub delay: Duration,
    pub mode: HealthCheckMode,
    pub silent: bool,
}

impl SafeDisplay for HealthCheckConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "delay: {:?}", self.delay);
        let _ = writeln!(&mut result, "mode:");
        let _ = writeln!(&mut result, "{}", self.mode.to_safe_string_indented());
        let _ = writeln!(&mut result, "silent: {}", self.silent);
        result
    }
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(10),
            mode: HealthCheckMode::default(),
            silent: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum HealthCheckMode {
    Grpc(Empty),
    #[cfg(feature = "kubernetes")]
    K8s(HealthCheckK8sConfig),
}

impl SafeDisplay for HealthCheckMode {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            HealthCheckMode::Grpc(_) => {
                let _ = writeln!(&mut result, "gRPC");
            }
            #[cfg(feature = "kubernetes")]
            HealthCheckMode::K8s(inner) => {
                let _ = writeln!(&mut result, "k8s:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

impl Default for HealthCheckMode {
    fn default() -> Self {
        Self::Grpc(Empty {})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckK8sConfig {
    pub namespace: String,
}

impl SafeDisplay for HealthCheckK8sConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "namespace: {}", self.namespace);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum PersistenceConfig {
    Redis(RedisConfig),
    FileSystem(FileSystemPersistenceConfig),
}

impl SafeDisplay for PersistenceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            PersistenceConfig::Redis(inner) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            PersistenceConfig::FileSystem(inner) => {
                let _ = writeln!(&mut result, "filesystem:");
                let _ = writeln!(&mut result, "path: {:?}", inner.path);
            }
        }
        result
    }
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self::Redis(RedisConfig::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSystemPersistenceConfig {
    pub path: PathBuf,
}

pub fn make_config_loader() -> ConfigLoader<ShardManagerConfig> {
    ConfigLoader::new_with_examples(Path::new("config/shard-manager.toml"))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;
    use test_r::test;

    use crate::shard_manager_config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        env::set_current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
            .expect("Failed to set current directory");

        let _ = make_config_loader().load().expect("Failed to load config");
    }
}
