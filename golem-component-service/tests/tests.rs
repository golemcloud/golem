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

pub mod all;

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_service_base::clients::limit::LimitService;
use std::sync::Arc;
use test_r::{sequential_suite, test_dep};

test_r::enable!();

sequential_suite!(all);

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("component-service-base-tests")
                .with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
fn stub_limit_service() -> Arc<dyn LimitService> {
    Arc::new(all::stubs::StubLimitService)
}
