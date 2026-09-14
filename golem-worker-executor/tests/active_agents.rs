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
use golem_api_grpc::proto::golem::shardmanager::ShardId;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AssignShardsRequest, RevokeShardsRequest, SetShardAssignmentRequest, assign_shards_response,
    revoke_shards_response, set_shard_assignment_response,
};
use golem_common::model::OwnedAgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::{HasActiveAgents, HasOplog};
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

#[test]
#[timeout("120s")]
async fn abandoned_deletion_finishes_after_the_invocation_loop_exits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed = agent_id!("Clock", "abandoned-deletion");
    let agent_id = executor.start_agent(&component.id, parsed.clone()).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &agent_id);
    let old = executor.active_agent(&owned).await.unwrap();
    let mut gate = executor.gate_next_agent_invocation_success(&agent_id);
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let parsed = parsed.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &parsed, "healthcheck", data_value!())
                .await
        }
    });
    gate.entered().await;
    assert!(old.primary().is_loaded().await);
    let deletion = tokio::spawn({
        let executor = executor.clone();
        let agent_id = agent_id.clone();
        async move { executor.delete_worker(&agent_id).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while old.primary().is_loaded().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("deletion did not start stopping the worker");
    assert!(executor.worker_is_cached(&owned).await);
    assert!(executor.get_worker_metadata(&agent_id).await.is_ok());
    assert!(!deletion.is_finished());
    deletion.abort();
    assert!(deletion.await.unwrap_err().is_cancelled());
    gate.release();
    let _ = invocation.await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_is_cached(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("abandoned deletion did not finish");
    assert!(executor.get_worker_metadata(&agent_id).await.is_err());
    executor
        .invoke_and_await_agent(&component, &parsed, "healthcheck", data_value!())
        .await?;
    let replacement = executor.active_agent(&owned).await.unwrap();
    assert!(!Arc::ptr_eq(&old, &replacement));
    assert!(
        golem_worker_executor::worker::Worker::start_if_needed(old.primary())
            .await
            .is_err()
    );
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn shard_retirement_removes_old_owner_without_removing_its_replacement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let mut client = executor.client.clone();
    let assignment = client
        .set_shard_assignment(SetShardAssignmentRequest {
            number_of_shards: 1,
            shard_ids: vec![ShardId { value: 0 }],
        })
        .await?
        .into_inner();
    assert!(matches!(
        assignment.result,
        Some(set_shard_assignment_response::Result::Success(_))
    ));
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("Clock", "shard-retirement-owner");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);
    for replace_assignment in [false, true] {
        executor
            .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
            .await?;
        let old = executor.active_agent(&owned_agent_id).await.unwrap();
        let blocked_completion = if replace_assignment {
            let mut gate = executor.gate_next_agent_invocation_success(&agent_id);
            let executor = executor.clone();
            let component = component.clone();
            let parsed_agent_id = parsed_agent_id.clone();
            let invocation = tokio::spawn(async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &parsed_agent_id,
                        "healthcheck",
                        data_value!(),
                    )
                    .await
            });
            gate.entered().await;
            Some((gate, invocation))
        } else {
            None
        };
        let mut retirement_client = client.clone();
        let retirement = async move {
            if replace_assignment {
                let response = retirement_client
                    .set_shard_assignment(SetShardAssignmentRequest {
                        number_of_shards: 1,
                        shard_ids: vec![],
                    })
                    .await?
                    .into_inner();
                assert!(matches!(
                    response.result,
                    Some(set_shard_assignment_response::Result::Success(_))
                ));
            } else {
                let response = retirement_client
                    .revoke_shards(RevokeShardsRequest {
                        shard_ids: vec![ShardId { value: 0 }],
                    })
                    .await?
                    .into_inner();
                assert!(matches!(
                    response.result,
                    Some(revoke_shards_response::Result::Success(_))
                ));
            }
            Ok::<(), anyhow::Error>(())
        };
        tokio::pin!(retirement);
        if let Some((gate, invocation)) = blocked_completion {
            assert!(
                tokio::time::timeout(Duration::from_millis(100), retirement.as_mut())
                    .await
                    .is_err()
            );
            assert!(executor.worker_is_cached(&owned_agent_id).await);
            gate.release();
            retirement.as_mut().await?;
            let _ = invocation.await?;
        } else {
            retirement.as_mut().await?;
        }
        assert!(!executor.worker_is_cached(&owned_agent_id).await);
        assert!(!old.primary().is_loaded().await);
        let reject_stale_invocation = || async {
            let oplog = old.primary().oplog();
            let before = oplog.current_oplog_index().await;
            assert!(
                old.primary()
                    .clone()
                    .invoke(golem_common::model::AgentInvocation::SaveSnapshot {
                        idempotency_key: golem_common::model::IdempotencyKey::fresh(),
                    })
                    .await
                    .is_err()
            );
            assert_eq!(oplog.current_oplog_index().await, before);
        };
        reject_stale_invocation().await;
        assert!(
            golem_worker_executor::worker::Worker::start_if_needed(old.primary())
                .await
                .is_err()
        );
        assert!(executor.tracked_card_ids().await.is_empty());
        let response = client
            .assign_shards(AssignShardsRequest {
                shard_ids: vec![ShardId { value: 0 }],
            })
            .await?
            .into_inner();
        assert!(matches!(
            response.result,
            Some(assign_shards_response::Result::Success(_))
        ));
        executor
            .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
            .await?;
        let replacement = executor.active_agent(&owned_agent_id).await.unwrap();
        assert!(!Arc::ptr_eq(&old, &replacement));
        reject_stale_invocation().await;
        let interests = executor.tracked_card_ids().await;
        assert!(!interests.is_empty());
        let old_worker = old.primary();
        assert!(replacement.entity_metadata().accepting_entities);
        old_worker
            .set_interrupting(
                golem_service_base::error::worker_executor::InterruptKind::Interrupt(
                    golem_common::model::Timestamp::now_utc(),
                ),
            )
            .await;
        assert!(replacement.entity_metadata().accepting_entities);
        old_worker.active_agents().remove(&old_worker).await;
        assert!(Arc::ptr_eq(
            &replacement,
            &executor.active_agent(&owned_agent_id).await.unwrap()
        ));
        assert_eq!(executor.tracked_card_ids().await, interests);
    }
    Ok(())
}

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
