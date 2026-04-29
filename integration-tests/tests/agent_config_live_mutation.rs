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

#[path = "agent_config/shared_agent_config_live_mutation.rs"]
mod shared_agent_config_live_mutation;

use convert_case::ccase;
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use std::sync::Arc;
use test_r::{tag_suite, test_dep};

test_r::enable!();

tag_suite!(shared_agent_config_live_mutation, group8);

trait TestContext: std::fmt::Debug + Send + Sync {
    fn test_component_file(&self) -> &'static str;
    fn test_component_name(&self) -> &'static str;
    fn agent_method_name(&self) -> &'static str;
    fn case_config_path_segment(&self, segment: &str) -> String;
}

#[test_dep(tagged_as = "ts")]
fn test_context_ts() -> Arc<dyn TestContext> {
    #[derive(Debug)]
    struct TsTestContext;

    impl TestContext for TsTestContext {
        fn test_component_file(&self) -> &'static str {
            "golem_it_agent_sdk_ts"
        }
        fn test_component_name(&self) -> &'static str {
            "golem-it:agent-sdk-ts"
        }
        fn agent_method_name(&self) -> &'static str {
            "echoLocalConfig"
        }
        fn case_config_path_segment(&self, segment: &str) -> String {
            ccase!(kebab -> camel, segment)
        }
    }

    Arc::new(TsTestContext)
}

#[test_dep(tagged_as = "rust")]
fn test_context_rust() -> Arc<dyn TestContext> {
    #[derive(Debug)]
    struct RustTestContext;

    impl TestContext for RustTestContext {
        fn test_component_file(&self) -> &'static str {
            "golem_it_agent_sdk_rust_release"
        }
        fn test_component_name(&self) -> &'static str {
            "golem-it:agent-sdk-rust"
        }
        fn agent_method_name(&self) -> &'static str {
            "echo_local_config"
        }
        fn case_config_path_segment(&self, segment: &str) -> String {
            ccase!(kebab -> snake, segment)
        }
    }

    Arc::new(RustTestContext)
}

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        #[cfg(unix)]
        unsafe {
            backtrace_on_stack_overflow::enable()
        };
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("agent-config-live-mutation")
                .with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
pub async fn create_deps(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        environment_state_cache_capacity: Some(0),
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
