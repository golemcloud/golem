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

/// Wall-clock durability: a `time.Now()` reading recorded in a live invocation is
/// reproduced from the oplog when that invocation is replayed after an executor
/// restart — not re-read as the current time. The agent stores each reading in
/// durable state; after a restart, `first-time` replays the original `record-time`
/// invocation, so the first reading must be unchanged.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_wall_clock_replayed_after_restart(
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
    let agent_id = agent_id!("ClockAgent", "go-clock-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "record-time", data_value!())
        .await?
        .into_typed::<i64>()?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart: reading first-time replays the record-time invocation, whose
    // time.Now() must reproduce the recorded value from the oplog.
    drop(executor);
    let executor = start(deps, &context).await?;

    let first = executor
        .invoke_and_await_agent(&component, &agent_id, "first-time", data_value!())
        .await?
        .into_typed::<i64>()?;
    drop(executor);

    assert_eq!(first, recorded);
    Ok(())
}

/// A durable outbound HTTP call made in the first invocation is served from the
/// oplog after a restart rather than re-fetched: the external counter advances
/// once per *live* call, so the second invocation sees "1-b", not "2-b".
///
/// This also covers the distinct host-completion and guest-delivery boundaries of
/// concurrent P3 calls: replay must not expose the recorded HTTP result before the
/// point where the live execution delivered it to the Go callback, because durable
/// clock and body operations may have occurred between those boundaries.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_outgoing_http_replayed_without_network(
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
