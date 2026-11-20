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

use anyhow::anyhow;
use axum::{Router, routing::get};
use golem_api_grpc::proto::golem::shardmanager::{ShardEpochEntry, ShardId};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    AssignShardsRequest, assign_shards_response,
};
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{IdempotencyKey, OwnedAgentId, ShardEpoch};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::HasOplog;
use golem_worker_executor::services::oplog::OplogError;
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides, take_agent_oplog_over_at_epoch,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

#[test]
#[timeout("3m")]
async fn stale_writer_new_executor_reconstructs_accepted_work_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    stale_writer_takeover(last_unique_id, deps, http_tests, false).await
}

#[test]
#[timeout("3m")]
async fn stale_writer_same_executor_reacquires_at_a_higher_epoch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    stale_writer_takeover(last_unique_id, deps, http_tests, true).await
}

async fn stale_writer_takeover(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
    same_executor: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let storage_calls = KeyValueStorageFaults::default();
    let old = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                // Keep the accepted work dirty until retirement's forced flush. No timer should
                // consume that obligation before the shard moves.
                config.agent_status_flush.interval = Duration::from_secs(3600);
            })),
            wrap_key_value_storage: Some(Arc::new({
                let storage_calls = storage_calls.clone();
                move |inner| {
                    Arc::new(FaultInjectingKeyValueStorage::new(
                        inner,
                        storage_calls.clone(),
                    ))
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let effects = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let route = Router::new().route(
        "/poll",
        get({
            let effects = effects.clone();
            move || {
                effects.fetch_add(1, Ordering::SeqCst);
                async { "done" }
            }
        }),
    );
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, route).await
    }));

    let component = old
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent = agent_id!("HttpClient2");
    let worker_id = old
        .start_agent_with(
            &component.id,
            agent.clone(),
            HashMap::from([("PORT".into(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    // Finish construction before arming the method's success gate.
    old.invoke_and_await_agent(&component, &agent, "get_count", data_value!())
        .await?;
    let worker = old
        .active_agent(&owned)
        .await
        .ok_or_else(|| anyhow!("old worker missing"))?
        .primary();
    let loads = old.instance_load_count(&worker_id);
    let mut success = old.gate_next_agent_invocation_success(&worker_id);
    let key = IdempotencyKey::fresh();
    let mut waiter = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let old = old.clone();
        let component = component.clone();
        let agent = agent.clone();
        let key = key.clone();
        async move {
            old.invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "start_polling",
                data_value!("done"),
            )
            .await
        }
    }));
    tokio::time::timeout(Duration::from_secs(30), success.entered()).await?;
    assert_eq!(effects.load(Ordering::SeqCst), 1);

    // Acceptance returns only after the queued mutation is committed. It has not run yet.
    let queued_key = IdempotencyKey::fresh();
    old.invoke_agent_with_key(&component, &agent, &queued_key, "increment", data_value!())
        .await?;
    let before = old.stored_oplog(&worker_id).await;
    let queued_id = before
        .iter()
        .rev()
        .find_map(|entry| match entry {
            OplogEntry::PendingAgentInvocation { invocation_id, .. } => Some(*invocation_id),
            _ => None,
        })
        .ok_or_else(|| anyhow!("accepted increment has no pending entry"))?;
    assert!(
        !before.iter().any(|entry| {
            matches!(entry, OplogEntry::AgentInvocationStarted { invocation_id, .. }
                if *invocation_id == queued_id)
        }),
        "the queued increment must not run on the old owner"
    );
    let status_calls = storage_calls.calls("update_status");
    let epoch = ShardEpoch(1);
    take_agent_oplog_over_at_epoch(deps, &context, &owned, epoch.0).await?;
    success.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), &mut waiter).await??;
    let error = answer.expect_err("a fenced completion must never publish success to its waiter");
    let error = format!("{error:#}");
    assert!(
        error.contains("ShardingNotReady") || error.contains("Sharding not ready"),
        "{error}"
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while old.worker_is_cached(&owned).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    worker.join_interrupt_establishment().await?;
    tokio::time::timeout(Duration::from_secs(30), worker.wait_for_unload_cleanup()).await??;
    assert!(
        !old.worker_is_cached(&owned).await,
        "the stale generation must retire"
    );
    assert_eq!(
        old.instance_load_count(&worker_id),
        loads,
        "shard loss must not restart the old generation"
    );
    assert_eq!(
        old.stored_oplog(&worker_id).await,
        before,
        "the fence must keep the stored oplog unchanged through retirement"
    );
    assert_eq!(
        storage_calls.calls("update_status"),
        status_calls,
        "the stale generation must not publish its dirty status during retirement"
    );
    assert!(
        matches!(
            worker.oplog().add(OplogEntry::suspend()).await,
            Err(OplogError::Fenced(_))
        ),
        "the retained stale handle must refuse later writes too"
    );

    let new = if same_executor {
        old.clone()
    } else {
        start(deps, &context).await?
    };
    let assigned = new
        .client
        .clone()
        .assign_shards(AssignShardsRequest {
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch: epoch.0,
            }],
            number_of_shards: 1,
            revision: 1,
            incarnation_id: "5eed0000-0000-4000-8000-000000000001".into(),
        })
        .await?
        .into_inner();
    assert!(
        matches!(
            assigned.result,
            Some(assign_shards_response::Result::Success(_))
        ),
        "{assigned:?}"
    );

    // Read storage without activating the worker. Assignment recovery itself must discover
    // and finish the accepted queue; a fresh invocation must not be what rescues it.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if new.stored_oplog(&worker_id).await.iter().any(|entry| {
                matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, .. }
                    if *invocation_id == queued_id)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("the new epoch owner did not recover the accepted queue"))?;

    // Observe accepted work before retrying its keys: retries must not hide a lost queue.
    let count = new
        .invoke_and_await_agent(&component, &agent, "get_count", data_value!())
        .await?;
    assert_eq!(
        count.into_typed::<u64>()?,
        1,
        "takeover must recover the accepted increment without a resend"
    );

    // Retry both accepted keys. Neither replay of the completed HTTP call nor retry of the
    // queued mutation may execute the effect again.
    new.invoke_and_await_agent_with_key(
        &component,
        &agent,
        &key,
        "start_polling",
        data_value!("done"),
    )
    .await?;
    new.invoke_and_await_agent_with_key(
        &component,
        &agent,
        &queued_key,
        "increment",
        data_value!(),
    )
    .await?;
    let count = new
        .invoke_and_await_agent(&component, &agent, "get_count", data_value!())
        .await?;
    assert_eq!(count.into_typed::<u64>()?, 1);
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "completed HTTP effects must replay without contacting the peer"
    );
    new.invoke_and_await_agent(&component, &agent, "increment", data_value!())
        .await?;
    let count = new
        .invoke_and_await_agent(&component, &agent, "get_count", data_value!())
        .await?;
    assert_eq!(
        count.into_typed::<u64>()?,
        2,
        "fresh work must continue from the reconstructed count"
    );
    let replacement = new
        .active_agent(&owned)
        .await
        .ok_or_else(|| anyhow!("replacement missing"))?
        .primary();
    assert!(!Arc::ptr_eq(&worker, &replacement));
    let after = new.stored_oplog(&worker_id).await;
    let last_finished = after
        .iter()
        .rposition(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
        .expect("settled history must have a finish");
    assert!(
        after[last_finished + 1..].iter().all(OplogEntry::is_hint),
        "no positional entry may follow the last settled invocation"
    );
    let mut started = HashMap::new();
    let mut finished = HashMap::new();
    let mut open_calls = HashSet::new();
    for (offset, entry) in after.into_iter().enumerate() {
        match entry {
            OplogEntry::Start { .. } => {
                open_calls.insert(OplogIndex::INITIAL.as_u64() + offset as u64);
            }
            OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
                assert!(
                    open_calls.remove(&start_index.as_u64()),
                    "a host call terminal must match one open Start"
                );
            }
            OplogEntry::AgentInvocationStarted { invocation_id, .. } => {
                *started.entry(invocation_id).or_insert(0) += 1;
            }
            OplogEntry::AgentInvocationFinished { invocation_id, .. } => {
                *finished.entry(invocation_id).or_insert(0) += 1;
            }
            _ => {}
        }
    }
    assert!(
        open_calls.is_empty(),
        "settled history has incomplete host calls: {open_calls:?}"
    );
    assert_eq!(
        started.get(&queued_id),
        Some(&1),
        "the accepted queued mutation must start exactly once"
    );
    assert!(
        started.values().all(|count| *count == 1),
        "each accepted invocation starts once: {started:?}"
    );
    assert_eq!(
        started, finished,
        "every accepted invocation finishes exactly once"
    );
    Ok(())
}
