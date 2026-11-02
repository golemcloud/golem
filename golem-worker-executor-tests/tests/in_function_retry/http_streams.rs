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
use golem_common::model::RetryConfig;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{HttpClientConfig, HttpClientEnabledConfig};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

use super::count_oplog_errors_containing;
use super::http_servers::start_gated_partial_response_http_server;

async fn run_response_body_pool_limit_control(
    max_connections: usize,
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(move |config| {
            config.retry = RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
            config.http_client = HttpClientConfig::Enabled(HttpClientEnabledConfig {
                connect_timeout: Duration::from_secs(30),
                max_connections_per_host: max_connections,
                max_total_connections: max_connections,
                ..Default::default()
            });
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let (port, mut requests, connections, ready, release) =
        start_gated_partial_response_http_server(1, b"hi".to_vec(), 206).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!("HttpClient4");
    let worker = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &component,
        &agent_id,
        "get_and_read_body_chunked",
        data_value!(),
    );
    let gate = async {
        assert_eq!(requests.recv().await, Some(None));
        assert_eq!(requests.recv().await, Some(Some(1)));
        ready.notified().await;
        release.notify_one();
    };
    let ((), result) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(gate, invocation)
    })
    .await?;
    assert_eq!(result?.into_typed::<String>()?, "200 hi");
    assert_eq!(connections.load(Ordering::SeqCst), 2);
    assert_eq!(
        count_oplog_errors_containing(&executor, &worker, "in-function retry").await?,
        1
    );

    drop(executor);
    let executor = start(deps, &context).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "stored_full_response", data_value!())
        .await?;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        2,
        "completed body replay must not dispatch"
    );
    executor.check_oplog_is_queryable(&worker).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_releases_single_pool_slot_before_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_response_body_pool_limit_control(1, last_unique_id, deps, http_tests).await
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_with_two_pool_slots(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_response_body_pool_limit_control(2, last_unique_id, deps, http_tests).await
}
