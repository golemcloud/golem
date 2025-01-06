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

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples, RedisConfig};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;

use crate::model::Empty;
use crate::shard_manager_config::HealthCheckMode::K8s;

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
}

impl Default for WorkerExecutorServiceConfig {
    fn default() -> Self {
        Self {
            assign_shards_timeout: Duration::from_secs(5),
            health_check_timeout: Duration::from_secs(2),
            revoke_shards_timeout: Duration::from_secs(5),
            retries: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(with = "humantime_serde")]
    pub delay: Duration,
    pub mode: HealthCheckMode,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(10),
            mode: HealthCheckMode::default(),
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

impl Default for HealthCheckMode {
    fn default() -> Self {
        Self::Grpc(Empty {})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckK8sConfig {
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum PersistenceConfig {
    Redis(RedisConfig),
    FileSystem(FileSystemPersistenceConfig),
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
    use test_r::test;

    use crate::shard_manager_config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        let _ = make_config_loader().load().expect("Failed to load config");
    }
}
