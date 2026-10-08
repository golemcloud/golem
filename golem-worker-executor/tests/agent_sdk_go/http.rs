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

//! Concurrent outgoing HTTP from goroutines. Mirrors
//! `http::http_client_using_reqwest_async_parallel_replay`: sixteen goroutines
//! post at once, the server holds every response and releases them in reverse
//! order, so the sends overlap and complete out of initiation order in the
//! oplog. A restart then replays those interleaved records before a fresh
//! invocation runs live.

use crate::Tracing;
use axum::Router;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::routing::post;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::spawn;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// The Start indexes of the `function_name` durable calls that ended.
fn ended_starts(oplog: &[PublicOplogEntryWithIndex], function_name: &str) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == function_name => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .filter(|start| {
            oplog
                .iter()
                .any(|e| matches!(&e.entry, PublicOplogEntry::End(p) if p.start_index == *start))
        })
        .collect()
}

#[test]
#[ignore]
// More than ten concurrent outgoing requests from goroutines never start sending; the invocation hangs.
#[tracing::instrument]
#[timeout("5m")]
async fn go_parallel_http_from_goroutines_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let captured_body: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_body_clone = captured_body.clone();

    // The first 16 requests (the first invocation) are held: each handler
    // registers a release channel keyed by the guest-assigned `X-Test` request
    // id (0..16) and answers only when the test releases it, so the release
    // order is tied to request identity rather than network arrival order.
    // Later requests (the post-restart invocation) respond immediately, as
    // does everything once `holding` is cleared (the releaser gave up), so a
    // straggler cannot block forever after a release timeout.
    let held: Arc<Mutex<std::collections::HashMap<u16, oneshot::Sender<()>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let held_in_server = held.clone();
    let arrivals = Arc::new(AtomicUsize::new(0));
    let arrivals_in_server = arrivals.clone();
    let holding = Arc::new(AtomicBool::new(true));
    let holding_in_server = holding.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(move |headers: HeaderMap, body: Bytes| {
                    let held = held_in_server.clone();
                    let arrivals = arrivals_in_server.clone();
                    let holding = holding_in_server.clone();
                    let captured_body = captured_body_clone.clone();
                    async move {
                        let header = headers
                            .get("X-Test")
                            .map(|h| h.to_str().unwrap().to_string())
                            .unwrap_or("no X-Test header".to_string());
                        let body = String::from_utf8(body.to_vec()).unwrap();
                        {
                            let mut capture = captured_body.lock().unwrap();
                            capture.push(body.clone());
                        }
                        if arrivals.fetch_add(1, Ordering::SeqCst) < 16
                            && holding.load(Ordering::SeqCst)
                        {
                            let id: u16 = header
                                .parse()
                                .expect("the first invocation's requests carry numeric X-Test ids");
                            let (release_tx, release_rx) = oneshot::channel();
                            let previous = held.lock().unwrap().insert(id, release_tx);
                            assert!(
                                previous.is_none(),
                                "request id {id} must arrive exactly once while responses are held"
                            );
                            let _ = release_rx.await;
                        }
                        format!(
                            "{{ \"percentage\" : 0.25, \"message\": \"response message {header}\" }}"
                        )
                    }
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    // Releases the held responses once all 16 concurrent requests (ids 0..16)
    // are in flight, in reverse request-id order, pacing the releases so the
    // completions land in the oplog out of initiation order. On timeout it
    // first stops the holding and drops every held channel so the blocked
    // handlers answer and the invocation cannot hang the test.
    let held_in_releaser = held.clone();
    let arrivals_in_releaser = arrivals.clone();
    let holding_in_releaser = holding.clone();
    let response_releaser = spawn(
        async move {
            let all_arrived = timeout(Duration::from_secs(30), async {
                loop {
                    if (0..16u16).all(|id| held_in_releaser.lock().unwrap().contains_key(&id)) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            if all_arrived.is_err() {
                holding_in_releaser.store(false, Ordering::SeqCst);
                held_in_releaser.lock().unwrap().clear();
                panic!(
                    "all 16 concurrent requests (distinct X-Test ids 0..16) must arrive while \
                     every response is held; {} arrived",
                    arrivals_in_releaser.load(Ordering::SeqCst)
                );
            }
            for id in (0..16u16).rev() {
                let release_tx = held_in_releaser
                    .lock()
                    .unwrap()
                    .remove(&id)
                    .expect("a held release channel exists for every request id");
                let _ = release_tx.send(());
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpAgent", "go-parallel-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = timeout(
        Duration::from_secs(120),
        executor.invoke_and_await_agent(&component, &agent_id, "run-parallel", data_value!(16u16)),
    )
    .await
    .expect("the first parallel invocation must complete once the held responses are released")?;
    let return_value = result.into_return_value().expect("Expected a return value");
    let SchemaValue::List { elements: lst } = &return_value else {
        panic!("Expected List, got {return_value:?}")
    };
    assert_eq!(lst.len(), 16);
    assert_eq!(captured_body.lock().unwrap().len(), 16);
    response_releaser.await?;

    // The durable record must prove both the overlap and the out-of-order
    // completion the server enforced: all 16 send `Start` entries precede
    // every send `End` entry, and the `End` order is not the `Start` order.
    {
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let ended = ended_starts(&oplog, "http::client::send");
        assert_eq!(
            ended.len(),
            16,
            "all 16 concurrent sends must complete with an End"
        );
        let send_starts: std::collections::HashSet<_> = ended.iter().copied().collect();
        let max_start_index = *ended.iter().max().unwrap();
        let ends_in_oplog_order: Vec<_> = oplog
            .iter()
            .filter_map(|e| match &e.entry {
                PublicOplogEntry::End(p) if send_starts.contains(&p.start_index) => {
                    Some((e.oplog_index, p.start_index))
                }
                _ => None,
            })
            .collect();
        let min_end_entry_index = ends_in_oplog_order
            .iter()
            .map(|(idx, _)| *idx)
            .min()
            .unwrap();
        assert!(
            min_end_entry_index > max_start_index,
            "all 16 sends must overlap: every send Start (last at {max_start_index}) must \
             precede every send End (first at {min_end_entry_index})"
        );
        let ends_by_start: Vec<_> = ends_in_oplog_order
            .iter()
            .map(|(_, start_index)| *start_index)
            .collect();
        assert!(
            !ends_by_start.is_sorted(),
            "the reverse-order releases must complete the sends out of initiation order, but \
             the End entries follow the Start order exactly: {ends_by_start:?}"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    // The fresh invocation first forces a full replay of the previous
    // invocation's interleaved concurrent-send records, then runs live.
    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "run-parallel", data_value!(16u16))
        .await?;
    let return_value2 = result2
        .into_return_value()
        .expect("Expected a return value");
    let SchemaValue::List { elements: lst2 } = &return_value2 else {
        panic!("Expected List, got {return_value2:?}")
    };
    assert_eq!(lst2.len(), 16);
    // The replayed sends must be served from the oplog, not re-issued: only
    // the fresh invocation's 16 requests reach the server.
    assert_eq!(captured_body.lock().unwrap().len(), 32);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}
