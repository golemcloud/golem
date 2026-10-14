// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

#[path = "custom_api/http_test_context.rs"]
mod http_test_context;
#[path = "custom_api/raw_http_process_loss.rs"]
mod raw_http_process_loss;

use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
#[allow(unused_imports)]
use golem_test_framework::config::WorkerExecutorClusterControl;
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, WorkerExecutorClusterControlDispatch,
    WorkerExecutorClusterControlStub,
};
use test_r::test_dep;

test_r::enable!();
test_r::sequential_suite!(raw_http_process_loss);

// Process-loss tests own one cluster, separate from the ordinary per-database suites.
#[test_dep(scope = Hosted, worker = both(WorkerExecutorClusterControl))]
pub async fn create_deps() -> EnvBasedTestDependencies {
    init_tracing_with_default_debug_env_filter(
        &TracingConfig::test_pretty_without_time("http-handler-lifecycle").with_env_overrides(),
    );
    EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing HTTP lifecycle test dependencies")
}
