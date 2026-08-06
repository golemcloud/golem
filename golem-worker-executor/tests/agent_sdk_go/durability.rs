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

//! The headline durability property for the Go SDK: a side effect recorded in one
//! invocation is REPLAYED from the oplog after an executor restart, not re-run.
//! Mirrors `durability::custom_durability_1`.

use crate::Tracing;
use axum::Router;
use axum::extract::Query;
use axum::routing::get;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// Pure-state replay: a durable counter's state is rebuilt by replaying its
/// prior invocations after an executor restart — no external I/O, isolating
/// whether basic Go replay is deterministic.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_counter_survives_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("CounterAgent", "go-restart-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5i64))
        .await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let value = executor
        .invoke_and_await_agent(&component, &agent_id, "value", data_value!())
        .await?
        .into_typed::<i64>()?;
    drop(executor);

    assert_eq!(value, 6);
    Ok(())
}

/// A durable outbound HTTP call made in the first invocation is served from the
/// oplog after a restart rather than re-fetched: the external counter advances
/// once per *live* call, so the second invocation sees "1-b", not "2-b".
///
/// IGNORED — root-caused, pending a core-team executor/runtime fix (G35/T48).
/// The first (live) call records correctly ("0-a"), but replay after a restart
/// fails with "Unexpected oplog entry during replay: expected Start {
/// monotonic_clock::now, ReadLocal, request }". Cause: while blocked on the P3
/// `wasi:http` send (which yields to the component event loop), the Go runtime
/// (scheduler/GC/netpoll) issues ~22 `monotonic_clock::now` reads, each journaled
/// as a strict durable op; their interleaving with the HTTP call's own durable
/// sub-ops differs between record and replay, so the replay cursor finds no
/// matching Start. HTTP itself is host-durablized and replays fine — only the
/// runtime clock chatter breaks it. Not fixable in the guest SDK: naive executor
/// clock-leniency turns the crash into an intermittent hang (breaks Go's monotonic
/// invariant), and dropping monotonic-clock journaling breaks the deliberate
/// `monotonic_clock_now_replay_parity` guarantee (durability.rs). The converged
/// fix (pending review) is clock classification: make the runtime's ambient
/// `monotonic-clock::now` non-durable while user `time.Now()` reads a separate
/// durable clock, so user-observed time still replays exactly. Pure-state replay
/// (go_counter_survives_restart) works, isolating this to the blocking-host-call path.
#[test]
#[ignore = "G35/T48: Go runtime monotonic-clock reads during blocking host calls corrupt positional replay"]
#[tracing::instrument]
#[timeout("2m")]
async fn go_durable_http_replays_not_reruns(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(AtomicU32::new(0));
    let response_clone = response.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    #[derive(Deserialize)]
    struct QueryParams {
        payload: String,
    }

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/callback",
                get(move |query: Query<QueryParams>| async move {
                    format!(
                        "{}-{}",
                        response_clone.fetch_add(1, Ordering::AcqRel),
                        query.payload
                    )
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("HttpAgent", "go-durability-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("a"))
        .await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart the executor: the first invocation must replay from the oplog.
    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("b"))
        .await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result1.into_typed::<String>()?, "0-a");
    assert_eq!(result2.into_typed::<String>()?, "1-b");
    Ok(())
}
