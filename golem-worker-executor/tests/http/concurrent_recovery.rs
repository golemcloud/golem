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

//! A streaming POST that is effectively idempotent (the default idempotence mode is on) recovers
//! call-locally after a crash: the recorded send `Start` is completed in place, the response body
//! continues after the chunks the guest already observed, and an independent remote operation
//! recorded next to it (a concurrent GET) keeps its recorded outcome instead of being rolled back
//! or refusing the recovery.

use super::*;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt as _;
use futures::stream;
use golem_common::model::AgentId;
use golem_common::model::oplog::PublicOplogEntryWithIndex;
use golem_worker_executor_test_utils::TestWorkerExecutor;
use pretty_assertions::assert_eq;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;
use tokio::sync::watch;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

const INFERENCE_BODY: &str = "chunk-0;chunk-1;chunk-2;chunk-3;chunk-4;chunk-5;chunk-6;chunk-7;";
/// The first inference response streams these bytes before it holds.
const DELIVERED_BEFORE_CRASH: usize = "chunk-0;chunk-1;chunk-2;".len();
const CHANGED_INFERENCE_BODY: &str =
    "CHUNK-0;CHUNK-1;CHUNK-2;CHUNK-3;CHUNK-4;CHUNK-5;CHUNK-6;CHUNK-7;";
const SIBLING_BODY: &str = "sibling-ok";

fn expected_request_body() -> Vec<u8> {
    (0..8)
        .map(|i| format!("request-chunk-{i};"))
        .collect::<String>()
        .into_bytes()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InferenceMode {
    /// The first POST holds before its response headers.
    BeforeHeaders,
    /// The first POST streams a prefix and holds; a POST carrying a `Range` header continues
    /// the prefix with a `206`.
    MidBodyThenRange,
    /// The first POST streams a prefix and holds; a POST carrying a `Range` header ignores it
    /// and sends the same full body again.
    MidBodyThenFullSame,
    /// The first POST streams a prefix and holds; a POST carrying a `Range` header ignores it
    /// and sends a different body.
    MidBodyThenFullChanged,
}

impl InferenceMode {
    fn holds_mid_body(self) -> bool {
        self != InferenceMode::BeforeHeaders
    }
}

/// An HTTP server with an `/inference` POST route whose first response is cut short by the
/// test (before headers or after a body prefix), and a `/sibling` GET route that answers only
/// after the first inference request body arrived, so the sibling's recorded outcome is durable
/// next to the incomplete inference send when the test crashes the executor.
struct InferenceServer {
    mode: InferenceMode,
    post_count: AtomicUsize,
    get_count: AtomicUsize,
    post_bodies: Mutex<Vec<Vec<u8>>>,
    post_range_headers: Mutex<Vec<Option<String>>>,
    first_post_arrived: watch::Sender<bool>,
}

impl InferenceServer {
    fn full_response(body: &'static str) -> Response {
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .body(Body::from(body))
            .unwrap()
    }

    async fn inference(
        State(server): State<Arc<InferenceServer>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let count = server.post_count.fetch_add(1, Ordering::SeqCst) + 1;
        server.post_bodies.lock().unwrap().push(body.to_vec());
        let range = headers
            .get("range")
            .map(|value| value.to_str().unwrap().to_string());
        server
            .post_range_headers
            .lock()
            .unwrap()
            .push(range.clone());
        if count == 1 {
            server.first_post_arrived.send_replace(true);
            if server.mode.holds_mid_body() {
                let prefix = stream::once(async {
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        &INFERENCE_BODY.as_bytes()[..DELIVERED_BEFORE_CRASH],
                    ))
                });
                let hold = stream::once(async {
                    std::future::pending::<()>().await;
                    Ok(Bytes::new())
                });
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/octet-stream")
                    .body(Body::from_stream(prefix.chain(hold)))
                    .unwrap();
            }
            std::future::pending::<()>().await;
        }
        match (server.mode, range) {
            (InferenceMode::MidBodyThenRange, Some(range)) => {
                assert_eq!(range, format!("bytes={DELIVERED_BEFORE_CRASH}-"));
                Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header("content-type", "application/octet-stream")
                    .header(
                        "content-range",
                        format!(
                            "bytes {DELIVERED_BEFORE_CRASH}-{}/{}",
                            INFERENCE_BODY.len() - 1,
                            INFERENCE_BODY.len()
                        ),
                    )
                    .body(Body::from(&INFERENCE_BODY[DELIVERED_BEFORE_CRASH..]))
                    .unwrap()
            }
            (InferenceMode::MidBodyThenFullChanged, Some(_)) => {
                Self::full_response(CHANGED_INFERENCE_BODY)
            }
            _ => Self::full_response(INFERENCE_BODY),
        }
    }

    async fn sibling(State(server): State<Arc<InferenceServer>>) -> impl IntoResponse {
        let mut arrived = server.first_post_arrived.subscribe();
        arrived
            .wait_for(|arrived| *arrived)
            .await
            .expect("the inference server state is alive");
        server.get_count.fetch_add(1, Ordering::SeqCst);
        SIBLING_BODY
    }
}

async fn spawn_inference_server(
    mode: InferenceMode,
) -> anyhow::Result<(Arc<InferenceServer>, u16, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let server = Arc::new(InferenceServer {
        mode,
        post_count: AtomicUsize::new(0),
        get_count: AtomicUsize::new(0),
        post_bodies: Mutex::new(Vec::new()),
        post_range_headers: Mutex::new(Vec::new()),
        first_post_arrived: watch::Sender::new(false),
    });
    let router = Router::new()
        .route("/inference", post(InferenceServer::inference))
        .route("/sibling", get(InferenceServer::sibling))
        .with_state(server.clone());
    let handle = spawn(
        async move {
            axum::serve(listener, router).await.unwrap();
        }
        .in_current_span(),
    );
    Ok((server, port, handle))
}

async fn wait_for_oplog(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
    description: &str,
    predicate: impl Fn(&[PublicOplogEntryWithIndex]) -> bool,
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        if predicate(&oplog) {
            return Ok(oplog);
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the oplog to show {description}: {oplog:#?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn jump_count(oplog: &[PublicOplogEntryWithIndex]) -> usize {
    oplog
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::Jump(_)))
        .count()
}

/// What the inference server saw at one point of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerCounts {
    posts: usize,
    gets: usize,
    post_bodies: Vec<Vec<u8>>,
    post_range_headers: Vec<Option<String>>,
}

impl ServerCounts {
    fn snapshot(server: &InferenceServer) -> Self {
        Self {
            posts: server.post_count.load(Ordering::SeqCst),
            gets: server.get_count.load(Ordering::SeqCst),
            post_bodies: server.post_bodies.lock().unwrap().clone(),
            post_range_headers: server.post_range_headers.lock().unwrap().clone(),
        }
    }
}

struct RecoveredRun {
    /// The result of the invocation that was interrupted by the crash.
    result: Result<String, String>,
    /// The oplog after that invocation settled.
    oplog: Vec<PublicOplogEntryWithIndex>,
    /// What the server saw once that invocation settled.
    recovered: ServerCounts,
    /// The result of a fresh invocation issued after one more reconstruction, which replays the
    /// repaired history before it runs. An error when the run did not ask for it.
    result_after_replay: Result<String, String>,
    /// What the server saw after that fresh invocation.
    after_replay: ServerCounts,
}

/// Starts `method` on a fresh agent, crashes the executor once the sibling GET has its
/// recorded outcome next to the incomplete inference send (and, for a mid-body hold, once the
/// guest observed the streamed prefix), restarts, and settles the invocation.
async fn recover_concurrent_post(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
    mode: InferenceMode,
    method: &str,
    verify_replay: bool,
) -> anyhow::Result<RecoveredRun> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (server, port, http_server) = spawn_inference_server(mode).await?;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let key = IdempotencyKey::fresh();

    executor
        .invoke_agent_with_key(&component, &agent_id, &key, method, data_value!())
        .await?;
    timeout(
        Duration::from_secs(20),
        server
            .first_post_arrived
            .subscribe()
            .wait_for(|arrived| *arrived),
    )
    .await??;
    // Before the headers, the inference send itself is the incomplete call. After them, the
    // send is complete and its consume-body scope is the incomplete one: at least one chunk is
    // recorded under it, and the guest's next read, which the held server never answers, is the
    // one incomplete chunk child. The sibling GET (send and body) is complete either way.
    let before_crash = wait_for_oplog(
        &executor,
        &worker_id,
        "a completed sibling send next to the incomplete inference request",
        |oplog| {
            let sends = partition_starts(oplog, "http::client::send");
            let bodies = partition_starts(oplog, "http::types::response::consume-body");
            let chunks = partition_starts(oplog, "http::types::response::consume-body-chunk");
            if mode.holds_mid_body() {
                sends.counts() == (0, 2, 0)
                    && bodies.counts() == (0, 1, 1)
                    && !chunks.ended.is_empty()
                    && chunks.incomplete.len() == 1
            } else {
                sends.counts() == (0, 1, 1) && bodies.counts() == (0, 1, 0)
            }
        },
    )
    .await?;
    assert_eq!(server.post_count.load(Ordering::SeqCst), 1);
    assert_eq!(server.get_count.load(Ordering::SeqCst), 1);
    assert_eq!(jump_count(&before_crash), 0);

    drop(executor);
    let executor = start(deps, &context).await?;
    let result = timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            method,
            data_value!(),
        ),
    )
    .await?
    .map_err(|error| error.to_string())
    .and_then(|value| {
        value
            .into_typed::<String>()
            .map_err(|error| error.to_string())
    });
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    let recovered = ServerCounts::snapshot(&server);

    let result_after_replay = if verify_replay {
        drop(executor);
        let executor = start(deps, &context).await?;
        let next_key = IdempotencyKey::fresh();
        let result = timeout(
            Duration::from_secs(60),
            executor.invoke_and_await_agent_with_key(
                &component,
                &agent_id,
                &next_key,
                method,
                data_value!(),
            ),
        )
        .await?
        .map_err(|error| error.to_string())
        .and_then(|value| {
            value
                .into_typed::<String>()
                .map_err(|error| error.to_string())
        });
        drop(executor);
        result
    } else {
        drop(executor);
        Err("replay verification was not requested".to_string())
    };
    http_server.abort();
    Ok(RecoveredRun {
        result,
        oplog,
        recovered,
        result_after_replay,
        after_replay: ServerCounts::snapshot(&server),
    })
}

fn full_success() -> String {
    format!("post=200 {INFERENCE_BODY} | get=200 {SIBLING_BODY}")
}

fn assert_call_local_recovery(run: &RecoveredRun) {
    assert_eq!(
        jump_count(&run.oplog),
        0,
        "call-local recovery must not roll back any history"
    );
    assert_eq!(
        partition_starts(&run.oplog, "http::client::send").counts(),
        (0, 2, 0),
        "both sends complete under their recorded Starts; no send is recorded twice"
    );
    assert_eq!(
        partition_starts(&run.oplog, "http::types::response::consume-body").counts(),
        (0, 2, 0),
        "each of the two response bodies is consumed under one recorded scope"
    );
    assert_eq!(
        run.recovered.gets, 1,
        "the sibling GET keeps its recorded outcome and is not re-issued"
    );
    assert_eq!(
        run.recovered.posts, 2,
        "the inference POST is re-issued exactly once"
    );
    assert_eq!(
        run.recovered.post_bodies,
        vec![expected_request_body(), expected_request_body()],
        "the re-issued POST carries the request body byte-identically"
    );
}

#[test]
#[tracing::instrument]
async fn post_crash_before_headers_repairs_the_send_and_keeps_the_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = recover_concurrent_post(
        last_unique_id,
        deps,
        http_tests,
        InferenceMode::BeforeHeaders,
        "post_with_concurrent_get",
        true,
    )
    .await?;
    assert_eq!(run.result, Ok(full_success()));
    assert_call_local_recovery(&run);
    assert_eq!(
        run.recovered.post_range_headers[1], None,
        "nothing was delivered before the crash, so the re-issue is a fresh request"
    );
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!(
        (run.after_replay.gets, run.after_replay.posts),
        (2, 3),
        "only the fresh invocation issues further requests after reconstruction"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn post_crash_mid_body_resumes_with_a_range_continuation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = recover_concurrent_post(
        last_unique_id,
        deps,
        http_tests,
        InferenceMode::MidBodyThenRange,
        "post_with_concurrent_get",
        true,
    )
    .await?;
    assert_eq!(run.result, Ok(full_success()));
    assert_call_local_recovery(&run);
    assert_eq!(
        run.recovered.post_range_headers[1],
        Some(format!("bytes={DELIVERED_BEFORE_CRASH}-")),
        "the re-issue asks for the bytes after the delivered prefix"
    );
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!((run.after_replay.gets, run.after_replay.posts), (2, 3));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn post_crash_mid_body_continues_a_verified_full_response(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = recover_concurrent_post(
        last_unique_id,
        deps,
        http_tests,
        InferenceMode::MidBodyThenFullSame,
        "post_with_concurrent_get",
        true,
    )
    .await?;
    assert_eq!(run.result, Ok(full_success()));
    assert_call_local_recovery(&run);
    assert_eq!(
        run.recovered.post_range_headers[1],
        Some(format!("bytes={DELIVERED_BEFORE_CRASH}-"))
    );
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!((run.after_replay.gets, run.after_replay.posts), (2, 3));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn post_crash_mid_body_reports_an_incomplete_response_when_the_body_changed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = recover_concurrent_post(
        last_unique_id,
        deps,
        http_tests,
        InferenceMode::MidBodyThenFullChanged,
        "post_with_concurrent_get",
        true,
    )
    .await?;
    assert_eq!(
        run.result,
        Ok(format!(
            "post=200 {} incomplete(ErrorCode::HttpResponseIncomplete) | get=200 {SIBLING_BODY}",
            &INFERENCE_BODY[..DELIVERED_BEFORE_CRASH]
        )),
        "the guest keeps the prefix it observed and sees an explicit incomplete response; \
         no bytes of the different response are spliced after it"
    );
    assert_call_local_recovery(&run);
    assert_eq!(
        run.recovered.post_range_headers[1],
        Some(format!("bytes={DELIVERED_BEFORE_CRASH}-"))
    );
    assert_eq!(
        run.result_after_replay,
        Ok(full_success()),
        "the fresh invocation sends no Range and receives the full body"
    );
    assert_eq!(run.after_replay.post_range_headers[2], None);
    Ok(())
}

/// With the idempotence mode off, the POST is not re-executable: its batched scope can only be
/// rolled back, which the completed sibling GET recorded next to it forbids, so the invocation
/// fails instead of re-issuing the POST or discarding the sibling.
#[test]
#[tracing::instrument]
async fn non_idempotent_post_crash_before_headers_refuses_recovery_next_to_a_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = recover_concurrent_post(
        last_unique_id,
        deps,
        http_tests,
        InferenceMode::BeforeHeaders,
        "post_non_idempotent_with_concurrent_get",
        false,
    )
    .await?;
    let error = run
        .result
        .expect_err("a non-idempotent POST must not be re-issued");
    assert!(
        error.contains("cannot retry"),
        "expected the non-idempotent recovery refusal, got: {error}"
    );
    assert_eq!(
        (run.recovered.posts, run.recovered.gets),
        (1, 1),
        "neither request is re-issued and the sibling is not rolled back"
    );
    Ok(())
}
