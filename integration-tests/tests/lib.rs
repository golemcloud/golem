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

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies, TestDependenciesDsl};
use test_r::test_dep;
use golem_common::model::AccountId;

test_r::enable!();

mod fork;

mod rib;
mod rib_repl;
mod worker;

#[derive(Debug)]
pub struct Tracing;

pub type Deps = TestDependenciesDsl<EnvBasedTestDependencies>;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test("integration-tests").with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
pub async fn create_deps(_tracing: &Tracing) -> Deps {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await;

    deps.redis_monitor().assert_valid();

    let deps2 = TestDependenciesDsl {
        deps,
        account_id: AccountId { value: "".to_string() },
        account_email: "".to_string(),
        token: Default::default(),
    };

    deps2
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}
