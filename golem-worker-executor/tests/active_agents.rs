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
use golem_api_grpc::proto::golem::shardmanager::{ShardEpochEntry, ShardId};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AssignShardsRequest, RevokeShardsRequest, assign_shards_response, revoke_shards_response,
};
use golem_common::model::OwnedAgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
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

/// `ShardId::from_agent_id` divides by the shard count, so a push carrying zero would abort this
/// executor on its next routing decision rather than fail the call. Refused at the door, and the
/// set it already holds is left alone.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn a_push_of_zero_shards_is_refused_rather_than_applied(
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
    let parsed_agent_id = agent_id!("Clock", "zero-shard-push");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);

    let mut client = executor.client.clone();
    let refused = client
        .assign_shards(AssignShardsRequest {
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch: 0,
            }],
            lease_ttl: Some(prost_types::Duration {
                seconds: 3600,
                nanos: 0,
            }),
            revision: 5,
            number_of_shards: 0,
        })
        .await?
        .into_inner();
    assert!(
        !matches!(
            refused.result,
            Some(assign_shards_response::Result::Success(_))
        ),
        "a push naming zero shards must be refused, not applied"
    );

    // The executor is still serving the set it had: the refusal happened before anything was
    // installed, so its routing hash never sees a zero to divide by.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(executor.worker_is_loaded(&owned_agent_id).await);
    executor
        .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
        .await?;

    drop(client);
    drop(executor);
    Ok(())
}

/// A revoke read from a state older than the last delivery this executor applied must be dropped
/// whole - not merely ignored for bookkeeping, but *without sweeping*. The sweep is the part with
/// teeth: it restarts every agent whose shard has gone, so a stale revoke that still swept would
/// interrupt agents on a shard this executor legitimately owns.
///
/// Driven over the wire because the guard lives in the gRPC handler, which cannot be constructed
/// in a unit test: its `WorkerCtx` and `HasAll` bounds are satisfied only by the production
/// bootstrap.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn a_revoke_older_than_the_last_delivery_does_not_sweep_agents(
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
    let parsed_agent_id = agent_id!("Clock", "stale-revoke-owner");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);
    assert!(executor.worker_is_loaded(&owned_agent_id).await);

    // The single-shard bootstrap owns shard 0, so this agent is on it.
    let shard = ShardId { value: 0 };
    let mut client = executor.client.clone();
    let assigned = client
        .assign_shards(AssignShardsRequest {
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(shard),
                epoch: 0,
            }],
            lease_ttl: Some(prost_types::Duration {
                seconds: 3600,
                nanos: 0,
            }),
            revision: 5,
            number_of_shards: 1,
        })
        .await?
        .into_inner();
    assert!(matches!(
        assigned.result,
        Some(assign_shards_response::Result::Success(_))
    ));

    // A revoke from before that push. It is answered `Success` - the shard is already where the
    // manager wants it - but it must change nothing.
    let stale = client
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![shard],
            revision: 3,
        })
        .await?
        .into_inner();
    assert!(matches!(
        stale.result,
        Some(revoke_shards_response::Result::Success(_))
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        executor.worker_is_loaded(&owned_agent_id).await,
        "a revoke older than the last applied delivery must not sweep the agents of a shard this \
         executor still owns"
    );

    // ...whereas one at the applied revision does take the shard, and the sweep runs.
    let current = client
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![shard],
            revision: 5,
        })
        .await?
        .into_inner();
    assert!(matches!(
        current.result,
        Some(revoke_shards_response::Result::Success(_))
    ));
    wait_until("the revoked shard's agent to be swept", || async {
        !executor.worker_is_loaded(&owned_agent_id).await
    })
    .await?;

    drop(client);
    drop(executor);
    Ok(())
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
