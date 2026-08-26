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

/// IGNORED — this currently fails: the executor refuses to end the region with
/// "Cannot end atomic region N: non-re-executable durable calls initiated in it
/// are still in flight". The region holds a lease per durable call it started
/// (durable_host/mod.rs `ActiveAtomicRegion.members`, weak refs kept alive by the
/// call's handle), and the outbound HTTP call's lease is still surviving at close.
///
/// Attempts that did NOT fix it (don't repeat blindly):
///   - waiting for the "handled ok" future's writer goroutine to finish before
///     returning from Body.Close (no effect);
///   - dropping the response trailers future in Body.Close (no effect);
///   - dropping the done/trailers future WRITERS after their write — this DOES
///     clear the atomic-region error, but the invocation then hangs to the test
///     timeout, so it breaks the protocol somewhere else.
/// Checked and not the cause: `Send` consumes the request handle and
/// `consume-body` consumes the response handle, so neither leaks.
///
/// Rust's `transactions.rs` performs HTTP inside atomic regions successfully, so
/// this is a Go-transport gap, not a platform limitation.
#[test]
#[ignore = "GOL-486: outbound HTTP inside an atomic region keeps a non-repairable durable call in flight, so the region cannot close"]
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
        .invoke_and_await_agent(&component, &agent_id, "atomic-callback", data_value!("inside"))
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
        .invoke_and_await_agent(&component, &agent_id, "atomic-call", data_value!("eu", 5i64))
        .await?
        .into_typed::<i64>()?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    assert_eq!(total, 5);
    Ok(())
}
