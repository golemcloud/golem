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

use crate::Tracing;
use golem_common::agent_id;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The number of observations of the gRPC API histogram `family` for `api`. The executor of the
/// test runs in this process, so its default registry is the one its `/metrics` endpoint exports.
fn grpc_api_count(family: &str, api: &str) -> u64 {
    prometheus::default_registry()
        .gather()
        .into_iter()
        .filter(|f| f.name() == family)
        .flat_map(|f| f.get_metric().to_vec())
        .filter(|metric| {
            let has = |name: &str, value: &str| {
                metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == name && label.value() == value)
            };
            has("api", api) && has("api_type", "grpc")
        })
        .map(|metric| metric.get_histogram().get_sample_count())
        .sum()
}

#[test]
#[tracing::instrument]
async fn an_established_worker_connection_counts_as_a_success(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("BlobStore", "connect-metrics-test-1");
    let worker_id = executor.start_agent(&component.id, agent_id).await?;

    let succeeded_before = grpc_api_count("api_success_seconds", "connect_worker");
    let _events = executor.capture_output(&worker_id).await?;
    let succeeded_after = grpc_api_count("api_success_seconds", "connect_worker");

    drop(executor);

    // Other tests in this process may connect too, so only an increase is asserted. Before the
    // fix the recorder was dropped with the request and every connection counted as `Drop`.
    assert!(
        succeeded_after > succeeded_before,
        "an established connection must count as a success: before={succeeded_before}, after={succeeded_after}"
    );
    Ok(())
}
