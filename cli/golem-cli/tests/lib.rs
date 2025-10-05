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

use std::fmt::Debug;
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use test_r::{tag_suite, test_dep};
use golem_test_framework::config::{EnvBasedTestDependencies, EnvBasedTestDependenciesConfig};

test_r::enable!();

mod app;
mod stubgen;

tag_suite!(app, app);

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(&TracingConfig::test(
            "golem-cli-integration-tests",
        ));
        Self
    }
}

#[test_dep]
fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
pub async fn test_dependencies(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let env_config = EnvBasedTestDependenciesConfig::new();
    let env = EnvBasedTestDependencies::new(env_config).await;

    env
}
