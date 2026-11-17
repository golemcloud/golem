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

//! An external server-sent event source whose events carry no `id:` field gives a reader no
//! position to resume from. Agent code reads such a source through outgoing HTTP. When the agent
//! crashes mid-read, the next read after reconstruction fails with an explicit
//! incomplete-response error after the events the guest already received. It never restarts the
//! source silently, which would deliver those events again or splice a different stream after
//! them.

use super::*;
use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use futures::StreamExt as _;
use futures::stream;
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

/// The events the first response streams before it holds.
const FIRST_FEED_PREFIX: &str = "data: a-1\n\ndata: a-2\n\ndata: a-3\n\n";
/// A live feed has moved on when it is requested again, so every later request receives these
/// events instead of the first ones.
const LATER_FEED: &str = "data: b-1\n\ndata: b-2\n\n";

#[derive(Debug, Clone, PartialEq, Eq)]
struct EventRequest {
    range: Option<String>,
    idempotency_key: Option<String>,
}

struct EventSource {
    requests: Mutex<Vec<EventRequest>>,
    count: AtomicUsize,
    first_request_arrived: watch::Sender<bool>,
}

impl EventSource {
    fn event_stream(body: Body) -> Response {
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .unwrap()
    }

    async fn events(State(source): State<Arc<EventSource>>, headers: HeaderMap) -> Response {
        let header = |name: &str| {
            headers
                .get(name)
                .map(|value| value.to_str().unwrap().to_string())
        };
        source.requests.lock().unwrap().push(EventRequest {
            range: header("range"),
            idempotency_key: header("idempotency-key"),
        });
        if source.count.fetch_add(1, Ordering::SeqCst) == 0 {
            source.first_request_arrived.send_replace(true);
            let prefix = stream::once(async {
                Ok::<_, std::convert::Infallible>(Bytes::from_static(FIRST_FEED_PREFIX.as_bytes()))
            });
            let hold = stream::once(async {
                std::future::pending::<()>().await;
                Ok(Bytes::new())
            });
            return Self::event_stream(Body::from_stream(prefix.chain(hold)));
        }
        Self::event_stream(Body::from(LATER_FEED))
    }
}

#[test]
#[tracing::instrument]
async fn sse_source_without_ids_fails_explicitly_after_a_crash_mid_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let source = Arc::new(EventSource {
        requests: Mutex::new(Vec::new()),
        count: AtomicUsize::new(0),
        first_request_arrived: watch::Sender::new(false),
    });
    let router = Router::new()
        .route("/events", get(EventSource::events))
        .with_state(source.clone());
    let http_server = spawn(
        async move {
            axum::serve(listener, router).await.unwrap();
        }
        .in_current_span(),
    );

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
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &key,
            "read_sse_without_ids",
            data_value!(),
        )
        .await?;
    timeout(
        Duration::from_secs(20),
        source
            .first_request_arrived
            .subscribe()
            .wait_for(|arrived| *arrived),
    )
    .await??;
    // Crash only once the guest received the first events: the send is complete, its body is
    // open, and a body chunk under it has its delivery marker.
    wait_for_oplog(
        &executor,
        &worker_id,
        "a delivered event chunk under the open response body",
        |oplog| {
            let sends = partition_starts(oplog, "http::client::send");
            let bodies = partition_starts(oplog, "http::types::response::consume-body");
            let chunks = partition_starts(oplog, "http::types::response::consume-body-chunk");
            sends.counts() == (0, 1, 0)
                && bodies.counts() == (0, 0, 1)
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
        },
    )
    .await?;
    assert_eq!(source.count.load(Ordering::SeqCst), 1);

    drop(executor);
    let executor = start(deps, &context).await?;
    let result = timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "read_sse_without_ids",
            data_value!(),
        ),
    )
    .await??
    .into_typed::<String>()?;
    assert_eq!(
        result, "events=[a-1,a-2,a-3] incomplete(ErrorCode::HttpResponseIncomplete) after 3 events",
        "the guest keeps the events it received and then sees an explicit failure; the later \
         feed is neither delivered from its start nor spliced after them"
    );

    let requests = source.requests.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        2,
        "recovery resends the recorded request once and opens no other session: {requests:?}"
    );
    assert_eq!(requests[0].range, None);
    assert_eq!(
        requests[1].range,
        Some(format!("bytes={}-", FIRST_FEED_PREFIX.len())),
        "the resend asks to continue after the delivered bytes"
    );
    assert!(requests[0].idempotency_key.is_some());
    assert_eq!(
        requests[1].idempotency_key, requests[0].idempotency_key,
        "the resend is the recorded request, not a new one"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Jump(_))),
        "recovery must not roll back any history"
    );
    assert_eq!(
        partition_starts(&oplog, "http::client::send").counts(),
        (0, 1, 0),
        "the send completes under its recorded Start; it is not recorded twice"
    );
    assert_eq!(
        partition_starts(&oplog, "http::types::response::consume-body").counts(),
        (0, 1, 0),
        "the open body completes under its recorded Start"
    );
    executor.check_oplog_is_queryable(&worker_id).await?;

    // A fresh invocation after one more reconstruction replays the repaired history, including
    // the explicit failure, without a request; only the new read reaches the source.
    drop(executor);
    let executor = start(deps, &context).await?;
    let next = timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &IdempotencyKey::fresh(),
            "read_sse_without_ids",
            data_value!(),
        ),
    )
    .await??
    .into_typed::<String>()?;
    assert_eq!(next, "events=[b-1,b-2] end");
    let requests = source.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests[2].range, None);

    drop(executor);
    http_server.abort();
    Ok(())
}
