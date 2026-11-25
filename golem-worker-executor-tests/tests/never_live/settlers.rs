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

//! E1c and E27: the settlers of a never-live fork, and the end of its generation.

use super::peers::TcpPeer;
use super::{settled_update_outcomes_after, tip};
use crate::Tracing;
use golem_common::model::agent::extraction::extract_component_metadata;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{AgentEvent, OplogIndex, OwnedAgentId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The `Start` entries of `oplog` that have no `End` and no `Cancelled`.
fn incomplete_starts(oplog: &[PublicOplogEntryWithIndex]) -> Vec<&PublicOplogEntryWithIndex> {
    let ended: Vec<OplogIndex> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => Some(end.start_index),
            PublicOplogEntry::Cancelled(cancelled) => Some(cancelled.start_index),
            _ => None,
        })
        .collect();
    oplog
        .iter()
        .filter(|entry| {
            matches!(entry.entry, PublicOplogEntry::Start(_)) && !ended.contains(&entry.oplog_index)
        })
        .collect()
}

/// One line per entry of `oplog`: its index, kind, and for a `Start` its function, parent and
/// observational owner. For failure messages.
fn describe(oplog: &[PublicOplogEntryWithIndex]) -> Vec<String> {
    oplog
        .iter()
        .map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) => format!(
                "{} Start {} parent={:?} owner={:?}",
                entry.oplog_index,
                start.function_name,
                start.parent_start_index,
                start.observational_owner
            ),
            PublicOplogEntry::End(end) => {
                format!("{} End of {}", entry.oplog_index, end.start_index)
            }
            PublicOplogEntry::Error(error) => format!("{} Error {error:?}", entry.oplog_index),
            PublicOplogEntry::Log(log) => format!("{} Log {:?}", entry.oplog_index, log.message),
            other => format!(
                "{} {}",
                entry.oplog_index,
                format!("{other:?}")
                    .split(['(', ' '])
                    .next()
                    .unwrap_or_default()
            ),
        })
        .collect()
}

/// The invocations of `wait_long` that reached the agent `peer`.
async fn peer_invocations(
    executor: &TestWorkerExecutor,
    peer: &OwnedAgentId,
) -> anyhow::Result<usize> {
    if executor.get_worker_metadata_opt(peer).await?.is_none() {
        return Ok(0);
    }
    Ok(executor
        .get_oplog(peer, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .count())
}

/// E1c: a source has three calls that do not end at the cut: an RPC call, an HTTP request and a
/// tool entity body that waits on a clock. A never-live fork of that cut, updated to its own
/// revision, settles all three at the end of its replay: it writes one `SuccessfulUpdate`, no
/// `End` or `Cancelled` for any incomplete `Start`, and no entry of the entity body. The HTTP
/// peer and the RPC peer get no further attempt.
#[test]
#[timeout("6m")]
async fn overlapping_incomplete_calls_of_a_never_live_fork_settle_with_one_outcome(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = crate::fork::start_with_local_resume_and(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        }
        .with_agent_sockets(),
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(crate::tool_streaming::deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let http_peer = TcpPeer::silent().await?;
    let peer_name = format!("e1c-peer-{}", uuid::Uuid::new_v4());
    let rpc_peer = executor.keyed_agent_id(
        component.environment_id,
        &agent_key!("ToolStreamingCaller", peer_name.clone()),
    )?;
    let name = format!("e1c-{}", uuid::Uuid::new_v4());
    let agent = agent_key!("ToolStreamingCaller", name.clone());
    let source_id = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_agent(
            &component,
            &agent,
            "overlapping_incomplete_calls",
            data_value!(http_peer.port, peer_name.clone()),
        )
        .await?;

    let cut = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let oplog = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
            let incomplete = incomplete_starts(&oplog);
            let body_waits = incomplete.iter().any(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start) if start.parent_start_index.is_some())
            });
            if body_waits
                && incomplete.len() >= 3
                && peer_invocations(&executor, &rpc_peer).await? >= 1
            {
                break anyhow::Ok(tip(&executor, &source_id).await?);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let cut = match cut {
        Ok(cut) => cut?,
        Err(_) => {
            let oplog = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
            return Err(anyhow::anyhow!(
                "the calls did not all start; peer invocations: {}; source: {:#?}",
                peer_invocations(&executor, &rpc_peer).await?,
                describe(&oplog)
            ));
        }
    };
    let source_oplog = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
    let held: Vec<OplogIndex> = incomplete_starts(&source_oplog)
        .into_iter()
        .filter(|entry| entry.oplog_index <= cut)
        .map(|entry| entry.oplog_index)
        .collect();
    assert!(held.len() >= 3, "{held:?}");
    let connections = http_peer.connections();
    let invocations = peer_invocations(&executor, &rpc_peer).await?;

    let fork = executor.fork_worker_never_live(&source_id, cut).await?;
    executor
        .auto_update_worker(&fork, component.revision, false)
        .await?;
    let outcomes = settled_update_outcomes_after(&executor, &fork, cut).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );

    let oplog = executor.get_oplog(&fork, OplogIndex::INITIAL).await?;
    let appended = super::entry_kinds(&oplog, cut);
    assert!(
        appended.iter().all(|kind| matches!(
            kind.as_str(),
            "StreamSession" | "PendingUpdate" | "SuccessfulUpdate"
        )),
        "{appended:?}"
    );
    let still_incomplete: Vec<OplogIndex> = incomplete_starts(&oplog)
        .into_iter()
        .map(|entry| entry.oplog_index)
        .collect();
    for start in &held {
        assert!(still_incomplete.contains(start), "{start}: {appended:?}");
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(http_peer.connections(), connections);
    assert_eq!(peer_invocations(&executor, &rpc_peer).await?, invocations);
    Ok(())
}

/// The function name of the custom durable call of `CustomDurability::cancel_pending_result`.
const CANCEL_RESULT: &str = "golem-it::cancel-result";

/// The entries appended to `oplog` after `cut` that are not update entries.
fn non_update_entries(oplog: &[PublicOplogEntryWithIndex], cut: OplogIndex) -> Vec<String> {
    super::entry_kinds(oplog, cut)
        .into_iter()
        .filter(|kind| {
            !matches!(
                kind.as_str(),
                "StreamSession" | "PendingUpdate" | "SuccessfulUpdate"
            )
        })
        .collect()
}

/// E27: the first update of a never-live fork ends its generation while work of that generation
/// still waits, and a second update queued with it runs in the next generation.
///
/// `CustomDurability::cancel_pending_result` races a custom durable call against an HTTP request
/// to `/cancel`. The custom call ends first, and the cancel then drops its undelivered result,
/// whose destructor makes an HTTP request to `/callback`.
///
/// - Cut at the `End` of the custom call, before its delivery: the replay holds a completion
///   without a delivery marker, which waits for the replay tail.
/// - Cut right after the `Start` of the destructor's HTTP request, which has no `End`: the
///   destructor waits for the replay tail.
///
/// Each fork gets two automatic updates queued together: to its own revision, then to a newer
/// revision with the same code. Each update writes one `SuccessfulUpdate`, the second in a later
/// generation, so the waiting work of the first generation neither keeps it from ending nor
/// reaches the refusal state of the second. The forks append no other entry, and their replays
/// reach no route of the server.
#[test]
#[timeout("6m")]
async fn the_waiting_work_of_a_never_live_generation_does_not_reach_the_next_one(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use axum::Router;
    use axum::routing::any;

    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume_and(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let cancels = Arc::new(AtomicUsize::new(0));
    let callbacks = Arc::new(AtomicUsize::new(0));
    let cancel = Arc::new(tokio::sync::Semaphore::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let routes = Router::new()
        .route(
            "/cancel",
            any({
                let cancels = cancels.clone();
                let cancel = cancel.clone();
                move || {
                    let cancels = cancels.clone();
                    let cancel = cancel.clone();
                    async move {
                        cancels.fetch_add(1, Ordering::SeqCst);
                        cancel.acquire().await.unwrap().forget();
                        "cancelled"
                    }
                }
            }),
        )
        .route(
            "/callback",
            any({
                let callbacks = callbacks.clone();
                move || {
                    let callbacks = callbacks.clone();
                    async move {
                        callbacks.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<&'static str>().await
                    }
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });

    let name = format!("e27-{}", uuid::Uuid::new_v4());
    let agent = agent_key!("CustomDurability", name.clone());
    let source_id = executor
        .start_agent_with(
            &component.id,
            agent.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent, "warmup_environment", data_value!())
        .await?;
    let mut events = executor.capture_output(&source_id).await?;
    let (accepted, release) = executor.gate_next_oplog_function_end(
        &source_id.agent_id,
        HostFunctionName::Custom(CANCEL_RESULT.to_string()),
        false,
    );
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent = agent.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent,
                    "cancel_pending_result",
                    data_value!(false),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), accepted).await??;

    // The `End` of the custom call, without a delivery marker after it.
    let (custom_start, markerless) = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
            let start = oplog.iter().find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start) if start.function_name == CANCEL_RESULT => {
                    Some(entry.oplog_index)
                }
                _ => None,
            });
            let end = start.and_then(|start| {
                oplog.iter().find_map(|entry| match &entry.entry {
                    PublicOplogEntry::End(end) if end.start_index == start => {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                })
            });
            if let (Some(start), Some(end)) = (start, end) {
                break anyhow::Ok((start, end));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let markerless_fork = executor
        .fork_worker_never_live(&source_id, markerless)
        .await?;

    // The cancel request is at the server. It answers while the custom call's `End` receipt is
    // still held, so the guest's race ends with the cancel and drops the undelivered result. The
    // destructor of that result then makes its HTTP request to `/callback`, whose `Start` waits
    // for the held receipt; the source is held right after that `Start` is committed, so the
    // fork's cut leaves it without an `End`.
    tokio::time::timeout(Duration::from_secs(30), async {
        while cancels.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let mut held = executor
        .gate_after_next_append(&source_id.agent_id, |entry| {
            matches!(entry, golem_common::model::oplog::OplogEntry::Start { function_name, .. }
                if function_name.to_string() == "http::client::send")
        })
        .await;
    cancel.add_permits(1);
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.recv().await {
            if matches!(AgentEvent::try_from(event), Ok(AgentEvent::Log { message, .. })
                if message.ends_with("dropping undelivered custom result"))
            {
                return;
            }
        }
        panic!("event stream ended before result destruction");
    })
    .await?;
    release
        .send(())
        .map_err(|_| anyhow::anyhow!("the End gate is gone"))?;
    tokio::time::timeout(Duration::from_secs(30), held.entered()).await?;
    let destructor_cut = tip(&executor, &source_id).await?;
    let oplog = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
    // The destructor's request is the HTTP `Start` right after the log line of `Drop`.
    let dropped = oplog
        .iter()
        .find(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Log(log)
                if log.message.contains("dropping undelivered custom result"))
        })
        .map(|entry| entry.oplog_index);
    let destructor = incomplete_starts(&oplog)
        .into_iter()
        .rfind(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start)
                if start.function_name == "http::client::send")
                && dropped.is_some_and(|dropped| entry.oplog_index > dropped)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the destructor's request has no incomplete Start: {:#?}",
                describe(&oplog)
            )
        })?
        .oplog_index;
    assert!(
        !oplog.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::Error(error) if error.error != "in-function retry")),
        "the source failed: {:#?}",
        describe(&oplog)
    );
    assert!(destructor > custom_start);
    assert_eq!(destructor, destructor_cut);
    let destructor_fork = executor
        .fork_worker_never_live(&source_id, destructor_cut)
        .await?;
    // The released source sends the destructor's request; only the forks are counted after it.
    drop(held);
    tokio::time::timeout(Duration::from_secs(30), async {
        while callbacks.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let cancels_before = cancels.load(Ordering::SeqCst);
    let callbacks_before = callbacks.load(Ordering::SeqCst);

    // The second update targets a newer revision with the same code: the executor refuses a
    // second request for an update that is already pending.
    let newer = executor
        .update_component(&component.id, "golem_it_host_api_tests_release")
        .await?;
    for (case, fork, cut) in [
        ("markerless completion", &markerless_fork, markerless),
        ("waiting destructor", &destructor_fork, destructor_cut),
    ] {
        executor
            .auto_update_worker(fork, component.revision, true)
            .await?;
        executor
            .auto_update_worker(fork, newer.revision, false)
            .await?;
        let outcomes = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let outcomes: Vec<_> = super::update_outcomes(&executor, fork)
                    .await?
                    .into_iter()
                    .filter(|outcome| outcome.oplog_index > cut)
                    .collect();
                if outcomes.len() >= 2 && !executor.worker_is_loaded(fork).await {
                    break anyhow::Ok(outcomes);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let Ok(outcomes) = outcomes else {
            let oplog = executor.get_oplog(fork, OplogIndex::INITIAL).await?;
            return Err(anyhow::anyhow!(
                "{case}: the second update did not settle; loaded: {}; appended: {:?}",
                executor.worker_is_loaded(fork).await,
                super::entry_kinds(&oplog, cut)
            ));
        };
        let outcomes = outcomes?;
        assert_eq!(outcomes.len(), 2, "{case}: {outcomes:?}");
        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome.entry, PublicOplogEntry::SuccessfulUpdate(_))),
            "{case}: {outcomes:?}"
        );
        assert!(
            executor.instance_load_count(&fork.agent_id) >= 2,
            "{case}: the second update did not run in a later generation"
        );
        let oplog = executor.get_oplog(fork, OplogIndex::INITIAL).await?;
        assert_eq!(
            non_update_entries(&oplog, cut),
            Vec::<String>::new(),
            "{case}"
        );
    }

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(cancels.load(Ordering::SeqCst), cancels_before);
    assert_eq!(callbacks.load(Ordering::SeqCst), callbacks_before);
    invocation.abort();
    server.abort();
    Ok(())
}
