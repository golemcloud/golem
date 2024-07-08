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

use std::ops::Deref;

use ctor::{ctor, dtor};
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};

mod worker;

struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(&TracingConfig::local_dev("integration-tests"));
        Self
    }
}

#[ctor]
pub static DEPS: EnvBasedTestDependencies = {
    let deps = EnvBasedTestDependencies::blocking_new(3);

    deps.redis_monitor().assert_valid();
    println!(
        "Started a cluster of {} worker executors",
        deps.worker_executor_cluster().size()
    );

    deps
};

#[dtor]
unsafe fn drop_deps() {
    let base_deps_ptr = DEPS.deref() as *const EnvBasedTestDependencies;
    let base_deps_ptr = base_deps_ptr as *mut EnvBasedTestDependencies;
    (*base_deps_ptr).kill_all()
}

// Note: by having it after DEPS, we don't see logs during setting up the env. But moving it before
// it crashes the dropping of the tracing subscriber with
// cannot access a Thread Local Storage value during or after destruction: AccessError
// This will be solved by our new test library in the future
#[ctor]
pub static TRACING: Tracing = Tracing::init();
