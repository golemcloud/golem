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

test_r::enable!();

use golem_api_grpc::proto::golem::rib::Expr;
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use test_r::{tag_suite, test_dep};

mod api_definition;
mod api_deployment;
mod api_security;
mod component;
mod invocation_context;
mod plugins;
mod worker;

tag_suite!(api_security, http_only);
tag_suite!(api_deployment, http_only);
tag_suite!(invocation_context, http_only);

pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test("sharding-tests").with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
pub async fn create_deps(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(
        EnvBasedTestDependenciesConfig {
            number_of_shards_override: Some(3),
            ..EnvBasedTestDependenciesConfig::new()
        }
        .with_env_overrides(),
    )
    .await;

    deps.redis_monitor().assert_valid();

    deps
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}

pub fn to_grpc_rib_expr(expr: &str) -> Expr {
    rib::Expr::from_text(expr).unwrap().into()
}
