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

mod local_agent_config;
mod shared_agent_config;

use crate::Tracing;
use convert_case::ccase;
use golem_test_framework::config::EnvBasedTestDependencies;
use std::sync::Arc;
use test_r::{inherit_test_dep, test_dep};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

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
    struct TsTestContext;

    impl TestContext for TsTestContext {
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

    Arc::new(TsTestContext)
}
