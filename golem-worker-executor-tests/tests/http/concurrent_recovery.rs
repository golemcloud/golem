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

//! A streaming POST recovers
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
    /// The durable request-body payload is unavailable as corrupt history after the crash.
    MidBodyWithCorruptRecording,
    BeforeFirstChunkWithCorruptRecording,
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
    read_permission_count: AtomicUsize,
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

    async fn allow_body_read(State(server): State<Arc<InferenceServer>>) -> impl IntoResponse {
        if server.read_permission_count.fetch_add(1, Ordering::SeqCst) == 0 {
            std::future::pending::<()>().await;
        }
        "read-now"
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
        read_permission_count: AtomicUsize::new(0),
        post_bodies: Mutex::new(Vec::new()),
        post_range_headers: Mutex::new(Vec::new()),
        first_post_arrived: watch::Sender::new(false),
    });
    let router = Router::new()
        .route("/inference", post(InferenceServer::inference))
        .route("/sibling", get(InferenceServer::sibling))
        .route("/allow-body-read", get(InferenceServer::allow_body_read))
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
        // A pending chunk's Start can follow the sibling's last commit. Flush the resident
        // buffer so this checkpoint observes a durable prefix, not the commit race.
        executor.commit_oplog(worker_id).await?;
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
    inject_payload_outage: bool,
) -> anyhow::Result<RecoveredRun> {
    let context = TestContext::new(last_unique_id);
    let before_first_chunk = mode == InferenceMode::BeforeFirstChunkWithCorruptRecording;
    let corrupt_recording =
        mode == InferenceMode::MidBodyWithCorruptRecording || before_first_chunk;
    let overrides = || golem_worker_executor_test_utils::TestExecutorOverrides {
        configure: inject_payload_outage.then(|| {
            Arc::new(
                |config: &mut golem_worker_executor::services::golem_config::GolemConfig| {
                    config.oplog.max_payload_size = 0;
                },
            ) as _
        }),
        ..Default::default()
    };
    let executor =
        golem_worker_executor_test_utils::start_with_overrides(deps, &context, overrides()).await?;
    let (server, port, http_server) = spawn_inference_server(mode).await?;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    if before_first_chunk {
        env.insert("WAIT_BEFORE_RESPONSE_READ".to_string(), "true".to_string());
    }
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
    // Before the headers, the inference send itself is incomplete. After them, its body parent
    // is incomplete and the guest has observed a committed prefix under it. The next read's
    // Start may still be buffered, so it cannot synchronize a crash through the public oplog.
    // The sibling GET (send and body) is complete either way.
    let before_crash = wait_for_oplog(
        &executor,
        &worker_id,
        "a completed sibling send next to the incomplete inference request",
        |oplog| {
            let sends = partition_starts(oplog, "http::client::send");
            let bodies = partition_starts(oplog, "http::types::response::consume-body");
            let chunks = partition_starts(oplog, "http::types::response::consume-body-chunk");
            if !sends.ended.iter().all(|start| {
                oplog.iter().any(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivered)
                        if delivered.start_index == *start)
                })
            }) {
                return false;
            }
            if before_first_chunk {
                sends.counts() == (0, 2, 1) && bodies.counts() == (0, 1, 1)
                    && !oplog.iter().any(|entry| matches!(&entry.entry,
                        PublicOplogEntry::Start(start) if start.parent_start_index == bodies.incomplete.first().copied()))
            } else if mode.holds_mid_body() {
                sends.counts() == (0, 2, 0)
                    && bodies.counts() == (0, 1, 1)
                    && oplog.iter().any(|entry| {
                        let PublicOplogEntry::Start(start) = &entry.entry else {
                            return false;
                        };
                        start.parent_start_index == bodies.incomplete.first().copied()
                            && chunks.ended.contains(&entry.oplog_index)
                            && oplog.iter().any(|delivery| {
                                matches!(
                                    &delivery.entry,
                                    PublicOplogEntry::CompletionDelivered(delivered)
                                        if delivered.start_index == entry.oplog_index
                                )
                            })
                    })
            } else {
                sends.counts() == (0, 1, 1) && bodies.counts() == (0, 1, 0)
            }
        },
    )
    .await?;
    assert_eq!(server.post_count.load(Ordering::SeqCst), 1);
    assert_eq!(server.get_count.load(Ordering::SeqCst), 1);
    assert_eq!(jump_count(&before_crash), 0);

    if corrupt_recording && !before_first_chunk {
        timeout(Duration::from_secs(20), async {
            loop {
                executor.commit_oplog(&worker_id).await?;
                let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                if !partition_starts(&entries, "http::types::response::consume-body-chunk")
                    .incomplete
                    .is_empty()
                {
                    break anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
    }

    let outage_payload =
        if inject_payload_outage {
            use golem_common::model::oplog::payload::{
                HostRequestP3HttpClientRequestBodyFrame, OplogPayload,
            };
            use golem_common::model::oplog::types::SerializableP3HttpRequestBodyFrame;
            use golem_common::model::oplog::{HostStreamKind, OplogEntry};
            use golem_common::schema::FromSchema;
            let parent = before_crash
                .iter()
                .find_map(|entry| match &entry.entry {
                    PublicOplogEntry::HostStreamFrame(params)
                        if params.kind == HostStreamKind::P3HttpRequestBody =>
                    {
                        let frame = HostRequestP3HttpClientRequestBodyFrame::from_value(
                            params.payload.value(),
                        )
                        .expect("request-body frames must decode through their public schema");
                        matches!(frame.frame, SerializableP3HttpRequestBodyFrame::Data { .. })
                            .then_some(params.parent_start_index)
                    }
                    _ => None,
                })
                .expect("the POST must have recorded request-body data");
            Some(executor.stored_oplog(&worker_id).await.into_iter().find_map(|entry| match entry {
            OplogEntry::HostStreamFrame {
                parent_start_index,
                kind: HostStreamKind::P3HttpRequestBody,
                payload: OplogPayload::External { payload_id, .. },
                ..
            } if parent_start_index == parent => Some(payload_id),
            _ => None,
        }).expect("the request body must have an external payload to exercise the backend outage"))
        } else {
            None
        };
    drop(executor);
    let executor =
        golem_worker_executor_test_utils::start_with_overrides(deps, &context, overrides()).await?;
    if let Some(payload) = outage_payload {
        if corrupt_recording {
            executor.set_oplog_download_corruption(&worker_id, payload);
        } else {
            executor.set_oplog_download_outage_for_payload(&worker_id, payload);
        }
    }
    let result = {
        let invocation = executor.invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            method,
            data_value!(),
        );
        tokio::pin!(invocation);
        if inject_payload_outage && !corrupt_recording {
            let hit = async {
                while executor.oplog_download_outage_hits(&worker_id).is_empty() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            tokio::select! {
                hit = timeout(Duration::from_secs(20), hit) => hit?,
                result = &mut invocation => panic!("payload outage must recover without completing the invocation: {result:?}"),
            }
            assert_eq!(
                server.post_count.load(Ordering::SeqCst),
                1,
                "storage recovery must precede the resend"
            );
            executor.set_oplog_download_outage(&worker_id, false);
        }
        timeout(Duration::from_secs(60), &mut invocation)
            .await?
            .map_err(|error| error.to_string())
            .and_then(|value| {
                value
                    .into_typed::<String>()
                    .map_err(|error| error.to_string())
            })
    };
    let executor = if corrupt_recording {
        drop(executor);
        start(deps, &context).await?
    } else {
        executor
    };
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    if inject_payload_outage {
        assert!(oplog.iter().all(|entry| !matches!(&entry.entry,
            PublicOplogEntry::Error(params) if params.kind == golem_common::model::oplog::OplogErrorKind::Invocation
        )), "storage outages must not charge semantic retries");
    }
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
async fn post_writer_get_crash_before_headers_drains_before_send_resolution(
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
        "post_with_writer_get",
        true,
        false,
    )
    .await?;
    assert_eq!(run.result, Ok(full_success()));
    assert_call_local_recovery(&run);
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!((run.after_replay.gets, run.after_replay.posts), (2, 3));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn post_writer_get_crash_mid_body_reuses_the_drained_request(
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
        "post_with_writer_get",
        true,
        false,
    )
    .await?;
    assert_eq!(run.result, Ok(full_success()));
    assert_call_local_recovery(&run);
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!((run.after_replay.gets, run.after_replay.posts), (2, 3));
    Ok(())
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
        false,
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
        false,
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
        false,
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
        false,
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

#[test]
#[tracing::instrument]
async fn post_crash_body_payload_outage_uses_infrastructure_recovery(
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
        true,
    )
    .await?;
    assert_eq!(run.result, Ok(full_success()));
    assert_call_local_recovery(&run);
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!((run.after_replay.gets, run.after_replay.posts), (2, 3));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn post_crash_corrupt_recording_completes_original_scope_with_typed_loss(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = recover_concurrent_post(
        last_unique_id,
        deps,
        http_tests,
        InferenceMode::MidBodyWithCorruptRecording,
        "post_with_concurrent_get",
        true,
        true,
    )
    .await?;
    assert_eq!(
        run.result,
        Ok(format!(
            "post=200 {} incomplete(ErrorCode::HttpResponseIncomplete) | get=200 {SIBLING_BODY}",
            &INFERENCE_BODY[..DELIVERED_BEFORE_CRASH]
        ))
    );
    assert_eq!(
        (run.recovered.posts, run.recovered.gets),
        (1, 1),
        "owner loss must not repeat either effect"
    );
    assert_eq!(
        partition_starts(&run.oplog, "http::client::send").counts(),
        (0, 2, 0)
    );
    assert_eq!(
        partition_starts(&run.oplog, "http::types::response::consume-body").counts(),
        (0, 2, 0)
    );
    assert!(
        partition_starts(&run.oplog, "http::types::response::consume-body-chunk")
            .incomplete
            .is_empty()
    );
    assert_eq!(jump_count(&run.oplog), 0);
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!((run.after_replay.posts, run.after_replay.gets), (2, 2));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn post_crash_corrupt_recording_delivers_loss_through_a_new_first_child(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = recover_concurrent_post(
        last_unique_id,
        deps,
        http_tests,
        InferenceMode::BeforeFirstChunkWithCorruptRecording,
        "post_with_concurrent_get",
        true,
        true,
    )
    .await?;
    assert_eq!(
        run.result,
        Ok(format!(
            "post=200  incomplete(ErrorCode::HttpResponseIncomplete) | get=200 {SIBLING_BODY}"
        ))
    );
    assert_eq!((run.recovered.posts, run.recovered.gets), (1, 1));
    assert_eq!(
        partition_starts(&run.oplog, "http::client::send").counts(),
        (0, 3, 0)
    );
    assert!(
        partition_starts(&run.oplog, "http::types::response::consume-body")
            .incomplete
            .is_empty()
    );
    assert_eq!(jump_count(&run.oplog), 0);
    assert_eq!(run.result_after_replay, Ok(full_success()));
    assert_eq!((run.after_replay.posts, run.after_replay.gets), (2, 2));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn incomplete_send_resumes_a_body_kept_open_until_response_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let requests = Arc::new(AtomicUsize::new(0));
    let (arrived, mut received) = watch::channel(false);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn({
        let requests = requests.clone();
        async move {
            let route = Router::new().route(
                "/early-response",
                axum::routing::put(move |request: axum::extract::Request| {
                    let requests = requests.clone();
                    let arrived = arrived.clone();
                    async move {
                        let mut body = request.into_body();
                        let frame = http_body_util::BodyExt::frame(&mut body)
                            .await
                            .unwrap()
                            .unwrap();
                        assert_eq!(frame.into_data().unwrap(), Bytes::from_static(b"hello"));
                        if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            arrived.send_replace(true);
                            std::future::pending::<()>().await;
                        }
                        "early-body"
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
    });
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    let key = IdempotencyKey::fresh();
    let method = "put_with_p3_body_open_until_response_read";
    executor
        .invoke_agent_with_key(&component, &agent_id, &key, method, data_value!())
        .await?;
    timeout(
        Duration::from_secs(20),
        received.wait_for(|arrived| *arrived),
    )
    .await??;
    executor.simulated_crash(&worker_id).await?;
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
    .await??
    .into_typed::<String>()?;
    assert_eq!(result, "200 early-body transmit=Ok(())");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        partition_starts(&oplog, "http::client::send").counts(),
        (0, 1, 0)
    );
    assert_eq!(jump_count(&oplog), 0);
    drop(executor);
    server.abort();
    Ok(())
}

#[test]
#[test_r::timeout("2m")]
async fn parent_only_response_repair_reports_loss_after_delivered_eof(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::payload::OplogPayload;
    use golem_common::model::oplog::{HostStreamKind, OplogEntry};
    let context = TestContext::new(last_unique_id);
    let executor = golem_worker_executor_test_utils::start_with_overrides(
        deps,
        &context,
        golem_worker_executor_test_utils::TestExecutorOverrides {
            configure: Some(Arc::new(|config| config.oplog.max_payload_size = 0)),
            ..Default::default()
        },
    )
    .await?;
    let posts = Arc::new(AtomicUsize::new(0));
    let gets = Arc::new(AtomicUsize::new(0));
    let release_headers = Arc::new(tokio::sync::Semaphore::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let posts = posts.clone();
        let gets = gets.clone();
        let release_headers = release_headers.clone();
        async move {
            let router = Router::new()
                .route(
                    "/inference",
                    post(move |_: Bytes| {
                        let posts = posts.clone();
                        let release_headers = release_headers.clone();
                        async move {
                            if posts.fetch_add(1, Ordering::SeqCst) == 0 {
                                release_headers.acquire().await.unwrap().forget();
                            }
                            INFERENCE_BODY
                        }
                    }),
                )
                .route(
                    "/sibling",
                    get(move || {
                        gets.fetch_add(1, Ordering::SeqCst);
                        async { SIBLING_BODY }
                    }),
                );
            axum::serve(listener, router).await.unwrap();
        }
    }));
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent = agent_id!("HttpClient4");
    let worker = executor
        .start_agent_with(
            &component.id,
            agent.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    let key = IdempotencyKey::fresh();
    let method = "post_with_concurrent_get";
    executor
        .invoke_agent_with_key(&component, &agent, &key, method, data_value!())
        .await?;
    wait_for_oplog(&executor, &worker, "completed sibling body", |entries| {
        partition_starts(entries, "http::types::response::consume-body").counts() == (0, 1, 0)
    })
    .await?;
    let (entered, release_end) = executor.gate_next_oplog_function_end(
        &worker,
        HostFunctionName::P3HttpClientConsumeBody,
        true,
    );
    release_headers.add_permits(1);
    timeout(Duration::from_secs(20), entered).await??;
    executor.commit_oplog(&worker).await?;
    let before = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let parent = partition_starts(&before, "http::types::response::consume-body").incomplete[0];
    let last_child = before
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.parent_start_index == Some(parent) => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .next_back()
        .unwrap();
    assert!(before.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::CompletionDelivered(delivered) if delivered.start_index == last_child)));
    assert!(
        partition_starts(&before, "http::types::response::consume-body-chunk")
            .incomplete
            .is_empty()
    );
    let send_start = before
        .iter()
        .find_map(|entry| {
            use golem_common::model::oplog::payload::HostRequestP3HttpClientRequestBodyFrame;
            use golem_common::model::oplog::types::SerializableP3HttpRequestBodyFrame;
            use golem_common::schema::FromSchema;
            let PublicOplogEntry::HostStreamFrame(frame) = &entry.entry else {
                return None;
            };
            if frame.kind != HostStreamKind::P3HttpRequestBody {
                return None;
            }
            let payload =
                HostRequestP3HttpClientRequestBodyFrame::from_value(frame.payload.value()).unwrap();
            matches!(
                payload.frame,
                SerializableP3HttpRequestBodyFrame::Data { .. }
            )
            .then_some(frame.parent_start_index)
        })
        .unwrap();
    let payload = executor
        .stored_oplog(&worker)
        .await
        .into_iter()
        .find_map(|entry| match entry {
            OplogEntry::HostStreamFrame {
                parent_start_index,
                kind: HostStreamKind::P3HttpRequestBody,
                payload: OplogPayload::External { payload_id, .. },
                ..
            } if parent_start_index == send_start => Some(payload_id),
            _ => None,
        })
        .unwrap();
    executor.set_oplog_download_corruption(&worker, payload);
    // The Store cannot unwind while the consume-body task waits for the gated End receipt.
    // Signal the crash first, so the refused End is observed as process loss, not as a trap.
    let crashed = executor.signal_simulated_crash(&worker).await?;
    release_end.send(()).unwrap();
    timeout(Duration::from_secs(20), crashed).await??;
    let result = timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(&component, &agent, &key, method, data_value!()),
    )
    .await??
    .into_typed::<String>()?;
    assert_eq!(
        result,
        format!(
            "post=200 {INFERENCE_BODY} incomplete(ErrorCode::HttpResponseIncomplete) | get=200 {SIBLING_BODY}"
        )
    );
    assert_eq!(
        (posts.load(Ordering::SeqCst), gets.load(Ordering::SeqCst)),
        (1, 1)
    );
    drop(executor);
    let executor = start(deps, &context).await?;
    let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert!(
        partition_starts(&entries, "http::types::response::consume-body")
            .ended
            .contains(&parent)
    );
    let result = timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent(&component, &agent, method, data_value!()),
    )
    .await??
    .into_typed::<String>()?;
    assert_eq!(result, full_success());
    assert_eq!(
        (posts.load(Ordering::SeqCst), gets.load(Ordering::SeqCst)),
        (2, 2)
    );
    drop(executor);
    drop(server);
    Ok(())
}

#[test]
#[test_r::timeout("2m")]
async fn cancelling_response_reconstruction_does_not_wait_for_peer_headers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(AtomicUsize::new(0));
    let signals = Arc::new(AtomicUsize::new(0));
    let cancel = Arc::new(tokio::sync::Semaphore::new(0));
    let (arrived_tx, mut arrived) = mpsc::unbounded_channel();
    let (closed_tx, mut closed) = mpsc::unbounded_channel();
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let requests = requests.clone();
        let cancel = cancel.clone();
        async move {
            let mut connections = tokio::task::JoinSet::new();
            while let Ok((mut socket, _)) = listener.accept().await {
                let requests = requests.clone();
                let signals = signals.clone();
                let cancel = cancel.clone();
                let arrived_tx = arrived_tx.clone();
                let closed_tx = closed_tx.clone();
                connections.spawn(async move {
                    let headers = read_request_headers(&mut socket).await.unwrap();
                    if headers.starts_with(b"GET /gated-body ") {
                        let count = requests.fetch_add(1, Ordering::SeqCst) + 1;
                        if count == 1 {
                            socket
                                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n")
                                .await
                                .unwrap();
                            socket.flush().await.unwrap();
                        }
                        arrived_tx.send(count).unwrap();
                        let mut byte = [0];
                        while socket.read(&mut byte).await.is_ok_and(|len| len > 0) {}
                        let _ = closed_tx.send(count);
                    } else if headers.starts_with(b"GET /cancel-signal ") {
                        if signals.fetch_add(1, Ordering::SeqCst) == 0 {
                            std::future::pending::<()>().await;
                        }
                        cancel.acquire().await.unwrap().forget();
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                            .await
                            .unwrap();
                    } else {
                        assert!(headers.starts_with(b"GET /cancel-done "));
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                            .await
                            .unwrap();
                    }
                });
            }
        }
    }));
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent = agent_id!("HttpClient4");
    let worker = executor
        .start_agent_with(
            &component.id,
            agent.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    let method = "get_and_cancel_body_read_after_signal";
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(&component, &agent, &key, method, data_value!())
        .await?;
    assert_eq!(
        timeout(Duration::from_secs(20), arrived.recv()).await?,
        Some(1)
    );
    timeout(Duration::from_secs(20), async {
        loop {
            executor.commit_oplog(&worker).await?;
            let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
            if partition_starts(&entries, "http::types::response::consume-body-chunk")
                .incomplete
                .len()
                == 1
                && partition_starts(&entries, "http::client::send").counts() == (0, 1, 1)
            {
                break anyhow::Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    drop(executor);
    let executor = start(deps, &context).await?;
    let invocation =
        executor.invoke_and_await_agent_with_key(&component, &agent, &key, method, data_value!());
    let coordinate = async {
        assert_eq!(
            timeout(Duration::from_secs(20), arrived.recv()).await?,
            Some(2)
        );
        cancel.add_permits(1);
        anyhow::Ok(())
    };
    let (result, coordinated) = timeout(Duration::from_secs(30), async {
        tokio::join!(invocation, coordinate)
    })
    .await?;
    coordinated?;
    assert_eq!(
        result?.into_typed::<String>()?,
        "cancelled-after-signal(200, 200) done=200"
    );
    timeout(Duration::from_secs(10), async {
        while closed.recv().await != Some(2) {}
    })
    .await?;
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        partition_starts(&entries, "http::client::send").counts(),
        (0, 3, 0)
    );
    assert_eq!(
        partition_starts(&entries, "http::types::response::consume-body").counts(),
        (0, 1, 0)
    );
    let chunks = partition_starts(&entries, "http::types::response::consume-body-chunk");
    assert_eq!(chunks.counts(), (0, 1, 0));
    let expected: golem_common::model::oplog::HostResponse =
        golem_common::model::oplog::HostResponseP3HttpClientConsumeBodyChunk {
            chunk: golem_common::model::oplog::types::SerializableP3HttpBodyChunk::Cancelled,
        }
        .into();
    let expected = expected.into_typed_schema_value().unwrap();
    assert!(entries.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::End(end) if end.start_index == chunks.ended[0] && end.response.as_ref() == Some(&expected))));
    drop(executor);
    drop(server);
    Ok(())
}
