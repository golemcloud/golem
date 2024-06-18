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

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_component_service_base::config::ComponentCompilationConfig;
use golem_service_base::config::{ComponentStoreConfig, DbConfig};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerExecutorClientCacheConfig {
    pub max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl Default for WorkerExecutorClientCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 1000,
            time_to_idle: Duration::from_secs(60 * 60 * 4),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentServiceConfig {
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub component_store: ComponentStoreConfig,
    pub compilation: ComponentCompilationConfig,
}

impl ComponentServiceConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/component-service.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self {
            enable_tracing_console: false,
            enable_json_log: false,
            http_port: 8081,
            grpc_port: 9091,
            db: DbConfig::default(),
            component_store: ComponentStoreConfig::default(),
            compilation: ComponentCompilationConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        let _ = super::ComponentServiceConfig::new();
    }

    #[test]
    pub fn compilation_can_be_disabled() {
        std::env::set_var("GOLEM__COMPILATION__TYPE", "Disabled");
        let cfg = super::ComponentServiceConfig::new();
        std::env::remove_var("GOLEM__COMPILATION__TYPE");

        assert!(matches!(
            cfg.compilation,
            super::ComponentCompilationConfig::Disabled(_)
        ));
    }
}
