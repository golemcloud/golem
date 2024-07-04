// Copyright 2024 Golem Cloud
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

use std::time::Duration;

use crate::model::Empty;
use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::{RedisConfig, RetryConfig};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct ShardManagerConfig {
    pub redis: RedisConfig,
    pub worker_executors: WorkerExecutorServiceConfig,
    pub health_check: HealthCheckConfig,
    pub enable_json_log: bool,
    pub http_port: u16,
    pub number_of_shards: usize,
    pub rebalance_threshold: f64,
}

impl ShardManagerConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/shard-manager.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerExecutorServiceConfig {
    #[serde(with = "humantime_serde")]
    pub assign_shards_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub health_check_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub revoke_shards_timeout: Duration,
    pub retries: RetryConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(with = "humantime_serde")]
    pub delay: Duration,
    pub mode: HealthCheckMode,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum HealthCheckMode {
    Grpc(Empty),
    #[cfg(feature = "kubernetes")]
    K8s(HealthCheckK8sConfig),
}

#[derive(Clone, Debug, Deserialize)]
pub struct HealthCheckK8sConfig {
    pub namespace: String,
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        let _ = super::ShardManagerConfig::new();
    }
}
