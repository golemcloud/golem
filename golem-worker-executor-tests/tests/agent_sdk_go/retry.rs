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

//! The Go SDK's retry DSL against a real failing endpoint: a policy that matches
//! on status code makes the host re-issue the request transparently, so the guest
//! only ever sees the eventual success. Mirrors
//! `agent_sdk_ts::sdk_policy::ts_http_status_retry_policy_retries_matching_status`.

use crate::Tracing;
use axum::Router;
use axum::extract::Query;
use axum::http::StatusCode;
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

/// The endpoint answers 500 for the first two requests and 200 afterwards. Under
/// a `retry.StatusCode.OneOf(500)` policy the guest's single `http.Get` still
/// returns the success body, and the server saw exactly three live requests —
/// proving the retries were real and bounded.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_retry_policy_retries_matching_status(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    const FAIL_COUNT: u32 = 2;
    let hits = Arc::new(AtomicU32::new(0));
    let hits_in_server = hits.clone();
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
                get(move |query: Query<QueryParams>| {
                    let hits = hits_in_server.clone();
                    async move {
                        let n = hits.fetch_add(1, Ordering::AcqRel);
                        if n < FAIL_COUNT {
                            (StatusCode::INTERNAL_SERVER_ERROR, "boom".to_string())
                        } else {
                            (StatusCode::OK, format!("ok-{}", query.payload))
                        }
                    }
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
    let agent_id = agent_id!("HttpAgent", "go-retry-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let body = executor
        .invoke_and_await_agent(&component, &agent_id, "retry-callback", data_value!("x"))
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    server.abort();

    assert_eq!(body, "ok-x");
    assert_eq!(
        hits.load(Ordering::Acquire),
        FAIL_COUNT + 1,
        "the endpoint should have been hit once per attempt"
    );
    Ok(())
}

/// A `retry.Schedule` retries in the agent: the endpoint answers 500 twice,
/// so the agent makes three attempts with a durable sleep between them, and
/// no host policy is involved. After an executor restart the replay rebuilds
/// the agent from the oplog: it reaches the same attempt count without calling
/// the endpoint again.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_retry_schedule_retries_in_the_agent_and_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    const FAIL_COUNT: u32 = 2;
    let hits = Arc::new(AtomicU32::new(0));
    let hits_in_server = hits.clone();
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
                get(move |query: Query<QueryParams>| {
                    let hits = hits_in_server.clone();
                    async move {
                        let n = hits.fetch_add(1, Ordering::AcqRel);
                        if n < FAIL_COUNT {
                            (StatusCode::INTERNAL_SERVER_ERROR, "boom".to_string())
                        } else {
                            (StatusCode::OK, format!("ok-{}", query.payload))
                        }
                    }
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
    let agent_id = agent_id!("HttpAgent", "go-retry-schedule-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "schedule-callback", data_value!("x"))
        .await?
        .into_typed::<String>()?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(result, "3:ok-x");
    assert_eq!(hits.load(Ordering::Acquire), FAIL_COUNT + 1);

    drop(executor);
    let executor = start(deps, &context).await?;

    let attempts = executor
        .invoke_and_await_agent(&component, &agent_id, "schedule-attempts", data_value!())
        .await?
        .into_typed::<u32>()?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    server.abort();

    assert_eq!(attempts, 3, "the replay rebuilt a different attempt count");
    assert_eq!(
        hits.load(Ordering::Acquire),
        FAIL_COUNT + 1,
        "the replay called the endpoint again"
    );
    Ok(())
}
