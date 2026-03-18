// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

mod agent_config;
mod api;
mod custom_api;
mod fork;
mod otlp_plugin;
mod plugins;
mod worker;

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use test_r::{tag_suite, test_dep};

test_r::enable!();

tag_suite!(worker, group1);
tag_suite!(fork, group1);

tag_suite!(worker_local_agent_config, group2);
tag_suite!(api, group2);
tag_suite!(custom_api, group2);

test_r::sequential_suite!(otlp_plugin);
test_r::sequential_suite!(plugins);

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        #[cfg(unix)]
        unsafe {
            backtrace_on_stack_overflow::enable()
        };
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("integration-tests").with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
pub async fn create_deps(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}
