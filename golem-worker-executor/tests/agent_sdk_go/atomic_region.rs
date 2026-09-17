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

//! What may be done inside an atomic region. A durable call started in the region
//! must settle before the region closes — the executor refuses to end a region
//! while a non-re-executable durable call it started is still in flight. Today a
//! cross-agent RPC settles and an outbound HTTP call does not, so these two tests
//! pin down exactly where the boundary is.

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
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// An outbound HTTP call inside `golem.Atomically` settles before the region
/// closes. The region may only end once every durable call it started has its
/// terminal recorded; for a p3 HTTP body that terminal is written when the host
/// finalizes the consume-body scope, which it signals through the trailers
/// future — so the transport must await that future after EOF, as the reference
/// wasip3 client does. Without the await the guest reaches `MarkEndOperation`
/// first and the executor refuses to close over the still-open scope.
///
/// IGNORED until CI builds Go components with golem's Go toolchain fork
/// (`tmp/go-runtime-sampler-wasip1.patch`, see `snapshot.rs` for the mechanism):
/// with the stock fork ~1 run in 6 hung before `send` on a clock read issued by
/// the Go runtime's goroutine-tracking sampler from inside the scheduler. A
/// user-level clock read in the same place is covered by
/// `go_atomic_region_with_clock_read_and_outgoing_http`.
#[test]
#[ignore = "needs golem's go toolchain fork (tmp/go-runtime-sampler-wasip1.patch); hangs ~1/6 on the stock fork CI still uses"]
#[tracing::instrument]
#[timeout("2m")]
async fn go_atomic_region_with_outgoing_http(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    #[derive(Deserialize)]
    struct QueryParams {
        payload: String,
    }

    let server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/callback",
                get(move |query: Query<QueryParams>| async move { query.payload.clone() }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("HttpAgent", "go-atomic-http-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let body = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "atomic-callback",
            data_value!("inside"),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    server.abort();

    assert_eq!(body, "inside");
    Ok(())
}

/// A cross-agent RPC inside an atomic region settles, so the region closes
/// normally. This is the pattern the `golem-atomic-block-go` skill documents, and
/// it is what distinguishes GOL-486 from a general atomic-region problem.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_atomic_region_with_rpc(
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
    let agent_id = agent_id!("RpcAgent", "go-atomic-rpc-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let total = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "atomic-call",
            data_value!("eu", 5i64),
        )
        .await?
        .into_typed::<i64>()?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    assert_eq!(total, 5);
    Ok(())
}

/// A durable clock read (`time.Now()` = wall + monotonic host calls) inside an
/// atomic region, immediately followed by an outbound HTTP call: both settle and
/// the region closes. Pins down that positional clock calls inside a region do
/// not interfere with the HTTP scope's completion (measured 24/24 while
/// diagnosing GOL-486).
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_atomic_region_with_clock_read_and_outgoing_http(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    #[derive(Deserialize)]
    struct QueryParams {
        payload: String,
    }

    let server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/callback",
                get(move |query: Query<QueryParams>| async move { query.payload.clone() }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("HttpAgent", "go-atomic-timed-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let body = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "atomic-timed-callback",
            data_value!("timed"),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    server.abort();

    assert_eq!(body, "timed");
    Ok(())
}
