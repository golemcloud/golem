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
use golem_common::model::OwnedAgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

const TEST_TTL: Duration = Duration::from_millis(50);

async fn wait_until(message: &str, mut condition: impl AsyncFnMut() -> bool) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {message}"))
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn unloaded_workers_are_evicted_after_ttl_only_when_exclusively_cached(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.active_agents.ttl = TEST_TTL;
                config.durable_stream.renewal_interval = Duration::from_millis(5);
                config.durable_stream.reconciliation_interval = Duration::from_millis(5);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("Clock", "ttl-eviction-owner");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);

    tokio::time::sleep(TEST_TTL * 4).await;
    assert!(
        executor.worker_is_cached(&owned_agent_id).await,
        "a loaded worker must survive TTL eviction"
    );

    wait_until("worker to become idle", || async {
        matches!(
            executor.stop_worker_if_idle(&owned_agent_id).await,
            Ok(true)
        )
    })
    .await?;
    let active_agent = executor
        .active_agent(&owned_agent_id)
        .await
        .expect("the unloaded worker must initially remain cached");

    tokio::time::sleep(TEST_TTL * 4).await;
    assert!(
        executor.worker_is_cached(&owned_agent_id).await,
        "an external ActiveAgent reference must prevent eviction"
    );

    let worker = active_agent.primary();
    drop(active_agent);
    tokio::time::sleep(TEST_TTL * 4).await;
    assert!(
        executor.worker_is_cached(&owned_agent_id).await,
        "an external Worker reference must prevent eviction"
    );

    drop(worker);
    wait_until(
        "exclusively cached unloaded worker to be evicted",
        || async { !executor.worker_is_cached(&owned_agent_id).await },
    )
    .await?;

    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn ttl_eviction_removes_the_evicted_owners_card_interests(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| config.active_agents.ttl = TEST_TTL)),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("Clock", "ttl-card-interest-owner");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);

    assert!(
        !executor.tracked_card_ids().await.is_empty(),
        "the active owner must register its invocation card interests"
    );
    wait_until("worker to become idle", || async {
        matches!(
            executor.stop_worker_if_idle(&owned_agent_id).await,
            Ok(true)
        )
    })
    .await?;
    wait_until("unloaded worker to be evicted", || async {
        !executor.worker_is_cached(&owned_agent_id).await
    })
    .await?;

    assert!(
        executor.tracked_card_ids().await.is_empty(),
        "evicting the owner must unregister its card interests, as explicit ActiveAgents removal does"
    );
    Ok(())
}
