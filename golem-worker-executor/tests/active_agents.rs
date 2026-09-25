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
use golem_worker_executor::services::{HasActiveAgents, HasOplog};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

/// The shard manager process every shard push in these tests names.
const TEST_SHARD_MANAGER: &str = "5eed0000-0000-4000-8000-000000000001";

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
    let mut client = executor.client.clone();
    let assignment = client
        .assign_shards(AssignShardsRequest {
            number_of_shards: 1,
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch: 1,
            }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        assignment.result,
        Some(assign_shards_response::Result::Success(_))
    ));
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
    let revoked = client
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![ShardId { value: 0 }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        revoked.result,
        Some(revoke_shards_response::Result::Success(_))
    ));
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
    let assignment = client
        .assign_shards(AssignShardsRequest {
            number_of_shards: 1,
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch: 2,
            }],
            revision: 2,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        assignment.result,
        Some(assign_shards_response::Result::Success(_))
    ));
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
        .assign_shards(AssignShardsRequest {
            number_of_shards: 1,
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch: 1,
            }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        assignment.result,
        Some(assign_shards_response::Result::Success(_))
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
    for (cycle, replace_assignment) in [false, true].into_iter().enumerate() {
        let revision = cycle as u64 + 1;
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
                    .assign_shards(AssignShardsRequest {
                        number_of_shards: 1,
                        shard_epochs: vec![],
                        revision,
                        incarnation_id: TEST_SHARD_MANAGER.to_string(),
                    })
                    .await?
                    .into_inner();
                assert!(matches!(
                    response.result,
                    Some(assign_shards_response::Result::Success(_))
                ));
            } else {
                let response = retirement_client
                    .revoke_shards(RevokeShardsRequest {
                        shard_ids: vec![ShardId { value: 0 }],
                        revision,
                        incarnation_id: TEST_SHARD_MANAGER.to_string(),
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
                number_of_shards: 1,
                shard_epochs: vec![ShardEpochEntry {
                    shard_id: Some(ShardId { value: 0 }),
                    epoch: cycle as u64 + 2,
                }],
                revision: revision + 1,
                incarnation_id: TEST_SHARD_MANAGER.to_string(),
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
        old_worker
            .active_agents()
            .remove_worker(&old_worker, false)
            .await;
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
            revision: 5,
            number_of_shards: 0,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
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
            revision: 5,
            number_of_shards: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
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
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
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
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
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

/// A delivery that keeps a shard but raises its epoch means the shard left this executor and
/// came back, so another executor may have written to its agents in between. An agent still
/// holding the older epoch's oplog must be given up and reopened at the new epoch; a delivery at
/// the epoch it already holds must leave it alone.
///
/// Driven over the wire for the same reason as the stale-revoke test above: the sweep lives in
/// the gRPC handler.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn a_delivery_that_raises_a_kept_shards_epoch_gives_its_agents_up(
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
    let parsed_agent_id = agent_id!("Clock", "epoch-raise-owner");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);
    assert!(executor.worker_is_loaded(&owned_agent_id).await);

    // The single-shard bootstrap holds shard 0 at epoch 0, so the agent's oplog asserts epoch 0.
    let shard = ShardId { value: 0 };
    let mut client = executor.client.clone();
    let push = |epoch: u64, revision: u64| AssignShardsRequest {
        shard_epochs: vec![ShardEpochEntry {
            shard_id: Some(shard),
            epoch,
        }],
        revision,
        number_of_shards: 1,
        incarnation_id: TEST_SHARD_MANAGER.to_string(),
    };

    // The handler sweeps on every applied push, changed or not, so this does run the sweep.
    let same_epoch = client.assign_shards(push(0, 5)).await?.into_inner();
    assert!(matches!(
        same_epoch.result,
        Some(assign_shards_response::Result::Success(_))
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        executor.worker_is_loaded(&owned_agent_id).await,
        "a push at the epoch the agent already holds must not give it up"
    );

    let raised = client.assign_shards(push(1, 6)).await?.into_inner();
    assert!(matches!(
        raised.result,
        Some(assign_shards_response::Result::Success(_))
    ));
    // The agent is idle, so assignment recovery does not track it and cannot reopen it while
    // this waits.
    wait_until("the superseded agent to be given up", || async {
        !executor.worker_is_loaded(&owned_agent_id).await
    })
    .await?;

    executor
        .invoke_and_await_agent(&component, &parsed_agent_id, "healthcheck", data_value!())
        .await?;
    assert!(executor.worker_is_loaded(&owned_agent_id).await);

    // The reopen asserts epoch 1. Had it been handed the epoch-0 handle back, this push would
    // sweep it as superseded.
    let kept = client.assign_shards(push(1, 7)).await?.into_inner();
    assert!(matches!(
        kept.result,
        Some(assign_shards_response::Result::Success(_))
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        executor.worker_is_loaded(&owned_agent_id).await,
        "the agent reopened after the raise must hold the new epoch and survive a push of it"
    );

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

/// A revoke's sweep selects from the resolved agents, so one still being created when its shard
/// leaves is not in it. It read the assignment before the revoke and opens its oplog at the epoch
/// it was granted, so it must be given up once it is published - not left cached here, unfenced,
/// until the shard's new owner claims the oplog. Prepared rather than started: a started agent's
/// invocation loop checks ownership on its own, and this is the agent that has no loop to.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn an_agent_whose_shard_leaves_while_it_is_being_created_is_given_up_once_published(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = golem_common::model::AgentId {
        component_id: component.id,
        agent_id: agent_id!("Clock", "created-while-the-shard-leaves").to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);
    // The harness learns the executor's agent registry from the first agent that runs, and the
    // agent under test never does.
    let warm_up = agent_id!("Clock", "created-while-the-shard-leaves-warm-up");
    executor.start_agent(&component.id, warm_up.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &warm_up, "healthcheck", data_value!())
        .await?;

    // Creation pauses with the oplog open at the epoch the assignment held, before the agent is
    // published.
    let mut gate = executor
        .gate_next_agent_initialization_enqueue(&agent_id)
        .await;
    let creation = tokio::spawn({
        let mut client = executor.client.clone();
        let request = CreateWorkerRequest {
            agent_id: Some(agent_id.clone().into()),
            component_owner_account_id: Some(context.account_id.into()),
            environment_id: Some(context.default_environment_id.into()),
            env: std::collections::HashMap::new(),
            config: Vec::new(),
            ignore_already_existing: false,
            auth_ctx: Some(executor.auth_ctx().into()),
            principal: None,
            invocation_context: None,
        };
        async move { client.prepare_worker(request).await }
    });
    tokio::time::timeout(Duration::from_secs(20), gate.entered())
        .await
        .map_err(|_| anyhow::anyhow!("creation never reached the initialization enqueue"))?;

    let mut client = executor.client.clone();
    let revoked = client
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![ShardId { value: 0 }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        revoked.result,
        Some(revoke_shards_response::Result::Success(_))
    ));

    drop(gate);
    let created = tokio::time::timeout(Duration::from_secs(20), creation).await??;
    assert!(
        matches!(
            created?.into_inner().result,
            Some(create_worker_response::Result::Success(_))
        ),
        "the creation had started before the shard left, and completes"
    );

    // Nothing else touches the agent: no invocation, no second delivery.
    wait_until(
        "the agent created on a shard that left to be given up",
        || async { !executor.worker_is_cached(&owned_agent_id).await },
    )
    .await?;
    Ok(())
}
