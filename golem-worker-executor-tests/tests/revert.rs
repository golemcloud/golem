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
use crate::hot_update::{FlakyComponentService, Outage};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ResolveRevertLastInvocationsRequest, RevertWorkerRequest,
    resolve_revert_last_invocations_response, revert_worker_response,
};
use golem_common::model::agent::{RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget};
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::{AgentStatus, OplogIndex};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::{TestDsl, update_counts};
use golem_worker_executor_test_utils::callers;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use log::info;
use pretty_assertions::{assert_eq, assert_ne};
use std::sync::Arc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_update_v1")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[timeout("4m")]
async fn revert_validates_delivery_snapshots_only_in_its_retained_prefix(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use futures::StreamExt;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::{InstalledPlugin, PluginPriority};
    use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::plugin_registration::PluginRegistrationId;
    use golem_common::serialization::{decode, serialize};
    use golem_worker_executor::services::golem_config::OplogConfig;
    use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
    use golem_worker_executor::storage::indexed::{
        IndexedStorageLabelledApi, IndexedStorageNamespace,
    };
    use golem_worker_executor_test_utils::{sqlite_storage_config, start_with_oplog_config};

    let context = TestContext::new(last_unique_id);
    let executor = start_with_oplog_config(
        deps,
        &context,
        Some(OplogConfig {
            indexed_storage_layers: 1,
            blob_storage_layers: 0,
            ..Default::default()
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let indexed_config = golem_worker_executor::derive_disjoint_sqlite_config(
        &sqlite_storage_config(deps, &context),
        "indexed",
    );
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", indexed_config.database)).await?;

    futures::stream::iter([(true, false), (true, true), (false, true)])
        .then(|(corrupt, retain)| {
            let executor = &executor;
            let component = &component;
            let indexed = &indexed;
            let pool = &pool;
            async move {
                let agent = agent_key!("Counter", format!("delivery-cut-{corrupt}-{retain}"));
                let id = executor.start_agent(&component.id, agent.clone()).await?;
                executor
                    .invoke_and_await_agent(component, &agent, "increment", data_value!())
                    .await?;
                let resident = executor.active_agent(&id).await.unwrap().primary();
                let retained_index = resident.get_last_known_status().await.oplog_idx;
                let grant = EnvironmentPluginGrantId::new();
                let plugin = InstalledPlugin {
                    environment_plugin_grant_id: grant,
                    priority: PluginPriority(0),
                    parameters: Default::default(),
                    plugin_registration_id: PluginRegistrationId::new(),
                    plugin_name: "delivery-cut".into(),
                    plugin_version: "1".into(),
                    oplog_processor_component_id: Some(component.id),
                    oplog_processor_component_revision: Some(ComponentRevision::INITIAL),
                    installed_by: context.account_id,
                };
                // Seed the producer's pre-send and confirmation shapes through the status actor.
                let delivery = serialize(&(plugin, resident.get_latest_worker_metadata().await))
                    .map_err(anyhow::Error::msg)?;
                let checkpoint = |delivery: Option<Vec<u8>>| OplogEntry::OplogProcessorCheckpoint {
                    timestamp: golem_common::model::Timestamp::now_utc(),
                    plugin_grant_id: grant,
                    target_agent_id: id.agent_id,
                    confirmed_up_to: if delivery.is_some() { OplogIndex::NONE } else { retained_index },
                    sending_up_to: retained_index,
                    last_batch_start: OplogIndex::INITIAL,
                    source_component_revision: ComponentRevision::INITIAL,
                    delivery,
                };
                let delivery_index = executor
                    .add_and_commit_oplog_entry(&id.agent_id, checkpoint(Some(delivery)))
                    .await?;
                let confirmation = executor
                    .add_and_commit_oplog_entry(&id.agent_id, checkpoint(None))
                    .await?;
                assert!(resident.get_last_known_status().await.oplog_idx >= confirmation);
                assert!(Arc::ptr_eq(
                    &resident,
                    &executor.active_agent(&id).await.unwrap().primary()
                ));

                let namespace = IndexedStorageNamespace::OpLog {
                    owned_agent_id: id.clone(),
                    agent_mode: AgentMode::Durable,
                };
                let key = id.storage_key();
                let read = || async { indexed.with_entity("test", "revert_delivery", "entry")
                    .read_raw(namespace.clone(), &key, delivery_index.into(), delivery_index.into()).await };
                let original = read().await?.pop().unwrap().1;
                let stored = if corrupt {
                    let mut entry: OplogEntry = decode(&original)?;
                    let OplogEntry::OplogProcessorCheckpoint { delivery, .. } = &mut entry else {
                        panic!("expected delivery checkpoint");
                    };
                    *delivery = Some(vec![255]);
                    let bytes = serialize(&entry).map_err(anyhow::Error::msg)?;
                    assert_eq!(sqlx::query("UPDATE index_storage SET value = ? WHERE namespace = 'durable-worker-oplog' AND key = ? AND id = ?")
                        .bind(&bytes).bind(&key).bind(delivery_index.as_u64() as i64)
                        .execute(pool).await?.rows_affected(), 1);
                    bytes
                } else { original };
                let cut = if retain { delivery_index } else { retained_index };
                let result = executor.revert(&id, RevertWorkerTarget::RevertToOplogIndex(
                    RevertToOplogIndex { last_oplog_index: cut }
                )).await;
                assert_eq!(read().await?, vec![(delivery_index.into(), stored)]);
                let stored_entries = indexed.with_entity("test", "revert_delivery", "entry")
                    .read::<OplogEntry>(namespace.clone(), &key, 1, u64::MAX).await?;
                if corrupt && retain {
                    let message = format!("{:#}", result.expect_err("retained corruption must fail"));
                    assert!(message.contains(&format!("oplog index {delivery_index}")), "{message}");
                    assert!(message.contains("version: 255"), "{message}");
                    assert!(!stored_entries.iter()
                        .any(|(_, entry)| matches!(entry, OplogEntry::Revert { .. })));
                } else {
                    result?;
                    assert!(stored_entries.iter().any(|(_, entry)| matches!(entry,
                        OplogEntry::Revert { dropped_region, .. } if dropped_region.start == cut.next()
                            && dropped_region.end >= confirmation)));
                    assert_eq!(executor.invoke_and_await_agent(component, &agent, "increment", data_value!())
                        .await?.into_typed::<u32>()?, 2);
                }
                Ok::<_, anyhow::Error>(())
            }
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()?;
    pool.close().await;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn self_revert_is_rejected_without_stopping_the_agent(
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
    let agent = agent_key!("GolemHostApi", "self-revert");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    for (method, target) in [
        ("revert_agent_result", 1u64),
        ("revert_last_invocations_result", 1u64),
        ("revert_last_invocations_result", u64::MAX),
    ] {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            executor.invoke_and_await_agent(
                &component,
                &agent,
                method,
                data_value!(worker.agent_id, target),
            ),
        )
        .await??
        .into_typed::<Result<(), String>>()?;
        assert!(
            result.unwrap_err().contains("Self-revert is not supported"),
            "self rejection must precede count resolution"
        );
        let metadata = executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "get_self_metadata_result",
                data_value!(),
            )
            .await?
            .into_typed::<Result<String, String>>()?;
        assert_eq!(metadata, Ok(worker.agent_id.to_string()));
    }

    executor.simulated_crash(&worker).await?;
    let metadata = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(metadata, Ok(worker.agent_id.to_string()));
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Revert { .. }))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revert_successful_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id1 = agent_key!("RpcCounter", "counter1");
    let worker_id1 = executor
        .start_agent(&component.id, agent_id1.clone())
        .await?;
    executor.log_output(&worker_id1).await?;

    let agent_id2 = agent_key!("RpcCounter", "counter2");
    let worker_id2 = executor
        .start_agent(&component.id, agent_id2.clone())
        .await?;

    // counter1.inc_by(5)
    executor
        .invoke_and_await_agent(&component, &agent_id1, "inc_by", data_value!(5u64))
        .await?;

    // counter2.inc_by(1)
    executor
        .invoke_and_await_agent(&component, &agent_id2, "inc_by", data_value!(1u64))
        .await?;
    let retained_index = executor
        .get_oplog(&worker_id2, OplogIndex::INITIAL)
        .await?
        .last()
        .unwrap()
        .oplog_index;

    // counter2.inc_by(2)
    executor
        .invoke_and_await_agent(&component, &agent_id2, "inc_by", data_value!(2u64))
        .await?;

    // counter2.get_value() -> 3
    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id2, "get_value", data_value!())
        .await?;

    // Revert last 2 invocations on counter2 (undoes inc_by(2) and get_value)
    executor
        .revert(
            &worker_id2,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 2,
            }),
        )
        .await?;

    // Reverting twice to the same live tail is valid even though its successor is deleted.
    for _ in 0..2 {
        executor
            .revert(
                &worker_id2,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: retained_index,
                }),
            )
            .await?;
    }
    assert!(
        executor
            .revert(
                &worker_id2,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: retained_index.next(),
                }),
            )
            .await
            .is_err(),
        "a deleted entry cannot be the retained tail"
    );

    // counter1.get_value() -> 5 (unchanged)
    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id1, "get_value", data_value!())
        .await?;

    // counter2.get_value() -> 1 (inc_by(2) was reverted)
    let result2_after = executor
        .invoke_and_await_agent(&component, &agent_id2, "get_value", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id1).await?;
    executor.check_oplog_is_queryable(&worker_id2).await?;

    let result1_value = result1.into_typed::<u64>()?;
    let result2_value = result2.into_typed::<u64>()?;
    let result2_after_value = result2_after.into_typed::<u64>()?;

    assert_eq!(result1_value, 5);
    assert_eq!(result2_value, 3);
    assert_eq!(result2_after_value, 1);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn count_based_revert_rejects_a_stale_resolution_without_writing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_key!("RpcCounter", "stale-revert");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "inc_by", data_value!(1u64))
        .await?;

    let resolution = executor
        .client
        .clone()
        .resolve_revert_last_invocations(ResolveRevertLastInvocationsRequest {
            agent_id: Some(worker_id.agent_id.into()),
            number_of_invocations: 1,
            environment_id: Some(context.default_environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
        })
        .await?
        .into_inner();
    let resolved = match resolution.result {
        Some(resolve_revert_last_invocations_response::Result::Success(resolved)) => resolved,
        other => anyhow::bail!("failed to resolve count-based revert: {other:?}"),
    };

    executor
        .invoke_and_await_agent(&component, &agent_id, "get_value", data_value!())
        .await?;
    let before = crate::revert_replay_tail::settled_oplog(&executor, &worker_id).await?;

    let response = executor
        .client
        .clone()
        .revert_worker(RevertWorkerRequest {
            agent_id: Some(worker_id.agent_id.into()),
            target: Some(
                RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                    number_of_invocations: 1,
                })
                .into(),
            ),
            environment_id: Some(context.default_environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
            resolved_revert: Some(resolved),
            expected_fingerprint: None,
        })
        .await?
        .into_inner();
    match response.result {
        Some(revert_worker_response::Result::Failure(error)) => {
            assert!(
                format!("{error:?}").contains("Stale count-based revert resolution"),
                "unexpected stale revert error: {error:?}"
            );
        }
        other => anyhow::bail!("stale count-based revert unexpectedly succeeded: {other:?}"),
    }

    let after = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        before.last().map(|entry| entry.oplog_index),
        after.last().map(|entry| entry.oplog_index),
        "stale count-based revert wrote an oplog entry"
    );

    Ok(())
}

/// Two count-based reverts of the last invocation carry the same resolution, as a repaired guest
/// call or a retried request sends them. The first drops the last invocation. The second finds
/// that revert already applied and succeeds without writing. A new resolution of the last
/// invocation skips the dropped one. After a new invocation starts, the same resolution is stale.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn a_repeated_count_based_revert_of_the_same_resolution_writes_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_key!("RpcCounter", "double-revert");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "inc_by", data_value!(1u64))
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "inc_by", data_value!(10u64))
        .await?;

    let resolve = || {
        let mut client = executor.client.clone();
        let request = ResolveRevertLastInvocationsRequest {
            agent_id: Some(worker_id.agent_id.into()),
            number_of_invocations: 1,
            environment_id: Some(context.default_environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
        };
        async move {
            match client
                .resolve_revert_last_invocations(request)
                .await?
                .into_inner()
                .result
            {
                Some(resolve_revert_last_invocations_response::Result::Success(resolved)) => {
                    Ok(resolved)
                }
                other => Err(anyhow::anyhow!("failed to resolve the revert: {other:?}")),
            }
        }
    };
    let revert = |resolved| {
        let mut client = executor.client.clone();
        let request = RevertWorkerRequest {
            agent_id: Some(worker_id.agent_id.into()),
            target: Some(
                RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                    number_of_invocations: 1,
                })
                .into(),
            ),
            environment_id: Some(context.default_environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
            resolved_revert: Some(resolved),
            expected_fingerprint: None,
        };
        async move { anyhow::Ok(client.revert_worker(request).await?.into_inner().result) }
    };

    let resolved = resolve().await?;
    match revert(resolved).await? {
        Some(revert_worker_response::Result::Success(_)) => {}
        other => anyhow::bail!("the first revert failed: {other:?}"),
    }
    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let resolved_again = resolve().await?;
    assert!(
        resolved_again.last_oplog_index < resolved.last_oplog_index,
        "the resolution after the revert counted the dropped invocation: {resolved_again:?}, \
         first resolution {resolved:?}"
    );

    match revert(resolved).await? {
        Some(revert_worker_response::Result::Success(_)) => {}
        other => anyhow::bail!("the repeated revert of an applied resolution failed: {other:?}"),
    }
    let after = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        before.last().map(|entry| entry.oplog_index),
        after.last().map(|entry| entry.oplog_index),
        "the repeated revert wrote an oplog entry"
    );

    let value = executor
        .invoke_and_await_agent(&component, &agent_id, "get_value", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(value, 1, "the first revert drops only the last invocation");

    let before = crate::revert_replay_tail::settled_oplog(&executor, &worker_id).await?;
    match revert(resolved).await? {
        Some(revert_worker_response::Result::Failure(error)) => {
            assert!(
                format!("{error:?}").contains("Stale count-based revert resolution"),
                "unexpected stale revert error: {error:?}"
            );
        }
        other => anyhow::bail!("the resolution applied again after a new invocation: {other:?}"),
    }
    let after = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        before.last().map(|entry| entry.oplog_index),
        after.last().map(|entry| entry.oplog_index),
        "the stale revert wrote an oplog entry"
    );

    Ok(())
}

/// Runs `add_after_release` on a `BackgroundWork` agent, whose result arrives while its
/// background work waits for a promise, and resolves a count-based revert of that invocation.
/// With `settle_first`, the test completes the promise and waits for the invocation's
/// `AgentInvocationSettled` before it sends the revert, so the oplog tail moved after the
/// resolution without a new invocation. Otherwise it sends the revert while the background work
/// still waits. Either way the revert applies the resolved cut and drops the invocation.
async fn count_based_revert_of_an_invocation_with_background_work(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_sdk_rust: &PrecompiledComponent,
    case: &str,
    settle_first: bool,
) -> anyhow::Result<()> {
    use golem_common::model::PromiseId;
    use golem_common::model::oplog::PublicAgentInvocation;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.suspend.suspend_after = std::time::Duration::from_secs(600);
                config.suspend.rpc_suspend_after = std::time::Duration::from_secs(600);
                config.suspend.wait_suspend_grace = std::time::Duration::from_secs(600);
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let tally_name = format!("{case}-tally-{}", context.redis_prefix());
    let tally = agent_key!("Tally", tally_name.clone());
    let agent = agent_key!(
        "BackgroundWork",
        format!("{case}-{}", context.redis_prefix())
    );
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    let release = executor
        .invoke_and_await_agent(&component, &agent, "create_release", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "add_after_release",
            data_value!(release.clone(), tally_name, None::<PromiseId>, 5u64),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "returned");

    let resolution = executor
        .client
        .clone()
        .resolve_revert_last_invocations(ResolveRevertLastInvocationsRequest {
            agent_id: Some(worker_id.agent_id.into()),
            number_of_invocations: 1,
            environment_id: Some(context.default_environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
        })
        .await?
        .into_inner();
    let resolved = match resolution.result {
        Some(resolve_revert_last_invocations_response::Result::Success(resolved)) => resolved,
        other => anyhow::bail!("failed to resolve count-based revert: {other:?}"),
    };
    let resolved_tail = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .last()
        .map(|entry| entry.oplog_index);

    if settle_first {
        executor
            .complete_promise(&worker_id.environment_id, &release, vec![1])
            .await?;
        let invocation = executor
            .get_oplog(&worker_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::AgentInvocationStarted(started) => match &started.invocation {
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "add_after_release" =>
                    {
                        Some(method.invocation_id)
                    }
                    _ => None,
                },
                _ => None,
            })
            .expect("add_after_release started");
        let settled = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                if oplog.iter().any(|entry| {
                    matches!(&entry.entry,
                        PublicOplogEntry::AgentInvocationSettled(settled)
                            if settled.invocation_id == invocation)
                }) {
                    return Ok::<_, anyhow::Error>(oplog);
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await??;
        assert_ne!(
            settled.last().map(|entry| entry.oplog_index),
            resolved_tail,
            "the oplog tail moved after the resolution"
        );
        assert_eq!(
            settled
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
                .count(),
            3,
            "no invocation started after add_after_release"
        );
    }

    let response = executor
        .client
        .clone()
        .revert_worker(RevertWorkerRequest {
            agent_id: Some(worker_id.agent_id.into()),
            target: Some(
                RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                    number_of_invocations: 1,
                })
                .into(),
            ),
            environment_id: Some(context.default_environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
            resolved_revert: Some(resolved),
            expected_fingerprint: None,
        })
        .await?
        .into_inner();
    match response.result {
        Some(revert_worker_response::Result::Success(_)) => {}
        other => anyhow::bail!("count-based revert failed: {other:?}"),
    }

    let reverted = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let region = reverted
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Revert(revert) => Some(revert.dropped_region.clone()),
            _ => None,
        })
        .expect("the revert is recorded");
    assert_eq!(
        region.start,
        OplogIndex::from_u64(resolved.last_oplog_index).next(),
        "the revert drops the oplog from the resolved cut"
    );

    if !settle_first {
        executor
            .complete_promise(&worker_id.environment_id, &release, vec![1])
            .await?;
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "completions", data_value!())
            .await?
            .into_typed::<u64>()?,
        0,
        "the reverted invocation's background run is not part of the agent's state"
    );
    if !settle_first {
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &tally, "total", data_value!())
                .await?
                .into_typed::<u64>()?,
            0,
            "the reverted invocation's background work never adds to the tally"
        );
    }

    drop(executor);
    Ok(())
}

/// A count-based revert sent after the reverted invocation settled applies: the settlement and
/// the background work moved the oplog tail after the resolution, but no invocation started, so
/// the stopped oplog still cuts at the resolved index.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn count_based_revert_applies_after_the_reverted_invocation_settled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    count_based_revert_of_an_invocation_with_background_work(
        last_unique_id,
        deps,
        agent_sdk_rust,
        "revert-after-settled",
        true,
    )
    .await
}

/// A count-based revert sent while the reverted invocation's background work still waits stops
/// the agent and applies.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn count_based_revert_applies_while_the_background_work_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    count_based_revert_of_an_invocation_with_background_work(
        last_unique_id,
        deps,
        agent_sdk_rust,
        "revert-during-drain",
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revert_failed_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_key!("FailingCounter", "revert_failed_worker");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(50u64))
        .await;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await;

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 1,
            }),
        )
        .await?;

    let result4 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result2.is_err());
    assert!(result3.is_err());
    assert!(
        result4.is_ok(),
        "Expected get after revert to succeed: {:?}",
        result4
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revert_failed_worker_to_invoke_of_failed_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_key!("FailingCounter", "revert_failed_worker");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(50u64))
        .await;

    let revert_target = {
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        oplog
            .iter()
            .rfind(|op| matches!(op.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .cloned()
            .unwrap()
    };

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: revert_target.oplog_index,
            }),
        )
        .await?;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result2.is_err());
    {
        let err3 = format!(
            "{}",
            result3.expect_err("Expected get after revert to fail")
        );
        assert!(
            err3.contains("value is too large"),
            "Expected 'value is too large' in error: {err3}"
        );
        assert!(
            !err3.contains("Oplog"),
            "Unexpected 'Oplog' in error: {err3}"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn revert_auto_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_key!("UpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor.log_output(&worker_id).await?;

    // Wait for the worker to finish initialization before reading the oplog
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Idle,
            std::time::Duration::from_secs(10),
        )
        .await?;

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    // Get the oplog index right after initialization (before the update)
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let after_init_index = oplog.last().unwrap().oplog_index;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    // Revert to just after initialization (not INITIAL), because agents cannot
    // be invoked if their initialization is reverted.
    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: after_init_index,
            }),
        )
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    info!("result: {result1:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0. After the revert to just after init, calling f2 again
    // returns a random number. The traces of the update should be gone.
    assert_eq!(result1.into_typed::<u64>()?, 0);
    assert_ne!(result2.into_typed::<u64>()?, 0);
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 0));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn revert_across_two_successful_manual_updates(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_key!("SnapshotUpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let initial = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let revert_target = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .last()
        .unwrap()
        .oplog_index;

    let revision_two = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, revision_two.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            revision_two.revision,
            std::time::Duration::from_secs(30),
        )
        .await?;
    let revision_two_target = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .last()
        .unwrap()
        .oplog_index;

    let revision_three = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, revision_three.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            revision_three.revision,
            std::time::Duration::from_secs(30),
        )
        .await?;

    let before_revert = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let first_pending_update = before_revert
        .iter()
        .find_map(|entry| {
            matches!(entry.entry, PublicOplogEntry::PendingUpdate { .. })
                .then_some(entry.oplog_index)
        })
        .expect("manual update should write PendingUpdate");
    let before_failed_cut = before_revert.last().unwrap().oplog_index;
    let split_result = executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: first_pending_update,
            }),
        )
        .await;
    assert!(
        split_result
            .expect_err("cut between PendingUpdate and SuccessfulUpdate must fail")
            .to_string()
            .contains("retains a snapshot-based PendingUpdate")
    );
    assert_eq!(
        executor
            .get_oplog(&worker_id, OplogIndex::INITIAL)
            .await?
            .last()
            .unwrap()
            .oplog_index,
        before_failed_cut,
        "an invalid update-boundary cut must not append Revert"
    );

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: revision_two_target,
            }),
        )
        .await?;

    let after_middle_revert = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(after_middle_revert.into_typed::<u32>()?, 1);
    assert_eq!(metadata.component_revision, revision_two.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));

    executor.simulated_crash(&worker_id).await?;
    let middle_after_restart = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(middle_after_restart.into_typed::<u32>()?, 1);

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: revert_target,
            }),
        )
        .await?;

    let after_revert = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(initial.into_typed::<u32>()?, 0);
    assert_eq!(after_revert.into_typed::<u32>()?, 0);
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 0));

    executor.simulated_crash(&worker_id).await?;
    let after_restart = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(after_restart.into_typed::<u32>()?, 0);

    let later_revision = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, later_revision.revision, false)
        .await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            later_revision.revision,
            std::time::Duration::from_secs(30),
        )
        .await?;
    let after_later_update = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    assert_eq!(after_later_update.into_typed::<u32>()?, 1);

    // The last two method invocations straddle the later manual update, so resolving this count
    // produces a cut in the surviving v1 history and crosses the update.
    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 2,
            }),
        )
        .await?;
    let after_count_revert = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "loaded_snapshot_revision",
            data_value!(),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(after_count_revert.into_typed::<u32>()?, 0);
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 0));
    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

/// A revert does not check the target of an automatic update whose strategy is not chosen, so a
/// component service that cannot give the target does not refuse the revert. The start after the
/// revert chooses the strategy and applies the update once the service gives the target.
#[test]
#[tracing::instrument]
#[timeout("120s")]
async fn revert_to_an_automatic_update_without_a_strategy_ignores_its_unavailable_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let outage = Arc::new(Outage::default());
    let wrapper_outage = outage.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new(move |inner| {
                Arc::new(FlakyComponentService {
                    inner,
                    outage: wrapper_outage.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent_id = agent_key!("UpdateTest");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Idle,
            std::time::Duration::from_secs(10),
        )
        .await?;
    let before_update = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .last()
        .unwrap()
        .oplog_index;

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    outage.begin(updated_component.revision);
    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    // The start writes the strategy entry after the admission entry, then fails to get the
    // target.
    let pending_updates = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let pending_updates = executor
                .get_oplog(&worker_id, before_update.next())
                .await?
                .into_iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::PendingUpdate(_)))
                .map(|entry| entry.oplog_index)
                .collect::<Vec<_>>();
            if pending_updates.len() >= 2 && outage.refused() > 0 {
                break anyhow::Ok(pending_updates);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await??;
    let admission = pending_updates[0];

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: admission,
            }),
        )
        .await?;

    outage.end();
    executor.resume(&worker_id, true).await?;
    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            std::time::Duration::from_secs(30),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}
