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

use crate::Tracing;
use axum::Router;
use axum::extract::Query;
use axum::routing::any;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::{Notify, Semaphore};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

const GENERATOR: &str = "golem-it::test-generator";
const GENERATOR_PARENT: &str = "golem-it::test-generator-parent";
const HTTP_SEND: &str = "http::client::send";
const WAIT_FOR: &str = "clocks::monotonic-clock::wait-for";

/// Records each callback in arrival order and answers `<counter>-<payload>`, where the counter is
/// taken when the response is produced. `slow` answers only after `fast` has answered, and
/// `scoped` answers only after `outside` has arrived, so the custom body is still waiting while
/// its sibling, or the outside-scope call, runs and completes first.
async fn start_callback_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }

    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let counter = Arc::new(AtomicU32::new(0));
    let fast_answered = Arc::new(Notify::new());
    let outside_arrived = Arc::new(Notify::new());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let route_history = history.clone();
    let route = Router::new().route(
        "/callback",
        any(move |Query(params): Query<Params>| {
            let history = route_history.clone();
            let counter = counter.clone();
            let fast_answered = fast_answered.clone();
            let outside_arrived = outside_arrived.clone();
            async move {
                history.lock().unwrap().push(params.payload.clone());
                match params.payload.as_str() {
                    "slow" => fast_answered.notified().await,
                    "scoped" => outside_arrived.notified().await,
                    "outside" => outside_arrived.notify_one(),
                    _ => {}
                }
                let result = format!(
                    "{}-{}",
                    counter.fetch_add(1, Ordering::AcqRel),
                    params.payload
                );
                if params.payload == "fast" {
                    fast_answered.notify_one();
                }
                result
            }
        }),
    );
    tokio::spawn(
        async move {
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );
    (port, history)
}

/// The position of the `AgentInvocationStarted` of `method_name`. Its index is the anchor of the
/// invocation's custom Starts.
fn invocation_start(oplog: &[PublicOplogEntryWithIndex], method_name: &str) -> usize {
    oplog
        .iter()
        .position(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(params)
                    if matches!(
                        &params.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(invocation)
                            if invocation.method_name == method_name
                    )
            )
        })
        .unwrap_or_else(|| panic!("no invocation of {method_name}"))
}

/// The oplog entries of the single agent invocation of `method_name`, from its
/// `AgentInvocationStarted` (whose index is the invocation anchor) up to its
/// `AgentInvocationFinished`.
fn invocation_entries<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    method_name: &str,
) -> (OplogIndex, &'a [PublicOplogEntryWithIndex]) {
    let start = invocation_start(oplog, method_name);
    let end = oplog[start..]
        .iter()
        .position(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("{method_name} did not finish"));
    (oplog[start].oplog_index, &oplog[start..=end])
}

struct CustomStart {
    index: OplogIndex,
    function_name: String,
    parent: Option<OplogIndex>,
    ordinal: u64,
}

/// Custom durable `Start`s of one agent invocation, ordered by their invocation ordinal.
fn custom_starts(entries: &[PublicOplogEntryWithIndex], anchor: OplogIndex) -> Vec<CustomStart> {
    let mut starts: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == GENERATOR
                    || params.function_name == GENERATOR_PARENT =>
            {
                let id = params
                    .invocation_id
                    .expect("custom Start carries an invocation ID");
                // A root operation is anchored at its agent invocation, a nested one at the Start
                // of its parent.
                assert_eq!(
                    id.anchor,
                    params.parent_start_index.unwrap_or(anchor),
                    "custom Start belongs to its invocation"
                );
                Some(CustomStart {
                    index: entry.oplog_index,
                    function_name: params.function_name.clone(),
                    parent: params.parent_start_index,
                    ordinal: id.ordinal,
                })
            }
            _ => None,
        })
        .collect();
    starts.sort_by_key(|start| (start.ordinal, start.index));
    starts
}

/// The observational owners of the HTTP sends of one invocation, in oplog order. `None` is an
/// ordinary host call outside every custom scope.
fn http_send_owners(entries: &[PublicOplogEntryWithIndex]) -> Vec<Option<OplogIndex>> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == HTTP_SEND => {
                Some(params.observational_owner)
            }
            _ => None,
        })
        .collect()
}

fn owned_sends(owners: &[Option<OplogIndex>], owner: OplogIndex) -> usize {
    owners.iter().filter(|o| **o == Some(owner)).count()
}

/// The indices of the Starts of `function_name` whose observational owner is `owner`.
fn starts_owned_by(
    entries: &[PublicOplogEntryWithIndex],
    function_name: &str,
    owner: Option<OplogIndex>,
) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == function_name && params.observational_owner == owner =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

/// The indices of the Ends of the Start at `start`.
fn ends_of(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == start => Some(entry.oplog_index),
            _ => None,
        })
        .collect()
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_custom_durability_scopes_stay_apart_under_concurrency(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (port, history) = start_callback_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_key!("CustomDurabilityTs", "scopes");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let nested = executor
        .invoke_and_await_agent(&component, &agent_id, "nested", data_value!(port as f64))
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(nested, vec!["0-nested", "1-root-sibling"]);

    let concurrent = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "concurrentSiblings",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        concurrent,
        vec!["3-slow", "2-fast"],
        "the first sibling completes after the second"
    );

    let outside = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "outsideScopeWhileWaiting",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        outside,
        vec!["5-scoped", "4-outside"],
        "the outside call completes while the custom body waits"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let (anchor, entries) = invocation_entries(&oplog, "nested");
    let starts = custom_starts(entries, anchor);
    let names: Vec<_> = starts.iter().map(|s| s.function_name.as_str()).collect();
    assert_eq!(names, vec![GENERATOR_PARENT, GENERATOR, GENERATOR]);
    assert_eq!(starts[0].parent, None, "the outer operation is at the root");
    assert_eq!(
        starts[1].parent,
        Some(starts[0].index),
        "the nested operation is a child of the outer one"
    );
    assert_eq!(
        starts[2].parent, None,
        "the later operation is a root sibling"
    );
    let owners = http_send_owners(entries);
    assert_eq!(owners.len(), 2);
    assert_eq!(owned_sends(&owners, starts[1].index), 1);
    assert_eq!(owned_sends(&owners, starts[2].index), 1);

    let (anchor, entries) = invocation_entries(&oplog, "concurrentSiblings");
    let starts = custom_starts(entries, anchor);
    assert_eq!(starts.len(), 2);
    let owners = http_send_owners(entries);
    assert_eq!(owners.len(), 2);
    for (ordinal, start) in starts.iter().enumerate() {
        assert_eq!(start.function_name, GENERATOR);
        assert_eq!(start.ordinal, ordinal as u64);
        assert_eq!(start.parent, None, "concurrent operations are siblings");
        assert_eq!(
            owned_sends(&owners, start.index),
            1,
            "each sibling owns exactly its own HTTP send"
        );
    }

    let (anchor, entries) = invocation_entries(&oplog, "outsideScopeWhileWaiting");
    let starts = custom_starts(entries, anchor);
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].parent, None);
    let owners = http_send_owners(entries);
    assert_eq!(owners.len(), 2);
    assert_eq!(
        owned_sends(&owners, starts[0].index),
        1,
        "the custom operation owns only its own HTTP send"
    );
    assert_eq!(
        owners.iter().filter(|owner| owner.is_none()).count(),
        1,
        "the call outside the scope stays an ordinary host call"
    );

    let before_crash = history.lock().unwrap().clone();
    let mut sorted = before_crash.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "fast",
            "nested",
            "outside",
            "root-sibling",
            "scoped",
            "slow"
        ]
    );

    executor.simulated_crash(&worker_id).await?;
    let after_recovery = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "callback",
            data_value!(port as f64, "after"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(after_recovery, "6-after");
    let mut expected = before_crash;
    expected.push("after".to_string());
    assert_eq!(
        *history.lock().unwrap(),
        expected,
        "replay must not repeat completed effects"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_custom_durability_parents_wait_and_escaped_work_leaves_the_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (port, _history) = start_callback_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_key!("CustomDurabilityTs", "lifecycle");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let parent = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "unawaitedChild",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(parent, "parent");
    let failure = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "failingBody",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(failure, "body failed");
    let escaped = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "escapedWork",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(escaped.len(), 2);
    assert!(escaped[0].ends_with("-escape-scoped"), "{escaped:?}");
    assert!(escaped[1].ends_with("-escaped"), "{escaped:?}");

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    // The parent does not await its child, and still finishes only after it.
    let (anchor, entries) = invocation_entries(&oplog, "unawaitedChild");
    let starts = custom_starts(entries, anchor);
    let names: Vec<_> = starts.iter().map(|s| s.function_name.as_str()).collect();
    assert_eq!(names, vec![GENERATOR_PARENT, GENERATOR]);
    let (parent, child) = (&starts[0], &starts[1]);
    assert_eq!(parent.parent, None);
    assert_eq!(
        child.parent,
        Some(parent.index),
        "the unawaited child nests"
    );
    let parent_end = ends_of(entries, parent.index);
    let child_end = ends_of(entries, child.index);
    assert_eq!(parent_end.len(), 1);
    assert_eq!(child_end.len(), 1);
    assert!(
        child_end[0] < parent_end[0],
        "the parent finishes after its child"
    );
    let owners = http_send_owners(entries);
    assert_eq!(owned_sends(&owners, child.index), 1);
    assert_eq!(owned_sends(&owners, parent.index), 0);

    // A failing body leaves its operation unfinished; later work is back at the root.
    let (anchor, entries) = invocation_entries(&oplog, "failingBody");
    let starts = custom_starts(entries, anchor);
    assert_eq!(starts.len(), 1);
    assert_eq!(ends_of(entries, starts[0].index), Vec::<OplogIndex>::new());
    let owners = http_send_owners(entries);
    assert_eq!(owners.len(), 2);
    assert_eq!(owned_sends(&owners, starts[0].index), 1);
    assert_eq!(owners.iter().filter(|owner| owner.is_none()).count(), 1);

    // A reaction that the body registered and that runs after the operation finished runs in the
    // nearest open scope, the root.
    let (anchor, entries) = invocation_entries(&oplog, "escapedWork");
    let starts = custom_starts(entries, anchor);
    assert_eq!(starts.len(), 1);
    let end = ends_of(entries, starts[0].index);
    assert_eq!(end.len(), 1);
    assert_eq!(
        starts_owned_by(entries, HTTP_SEND, Some(starts[0].index)).len(),
        1
    );
    let root_sends = starts_owned_by(entries, HTTP_SEND, None);
    assert_eq!(
        root_sends.len(),
        1,
        "the escaped call is an ordinary host call"
    );
    assert!(
        root_sends[0] > end[0],
        "the escaped call runs after the End"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_custom_durability_recovers_one_completed_and_one_incomplete_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let second_arrived = Arc::new(Notify::new());
    let checkpoint = Arc::new(Notify::new());
    let resume = Arc::new(Semaphore::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }
    let route_history = history.clone();
    let callback_resume = resume.clone();
    let release_resume = resume.clone();
    let route_checkpoint = checkpoint.clone();
    let route = Router::new()
        .route(
            "/callback",
            any(move |Query(params): Query<Params>| {
                let history = route_history.clone();
                let arrived = second_arrived.clone();
                let resume = callback_resume.clone();
                async move {
                    history.lock().unwrap().push(params.payload.clone());
                    match params.payload.as_str() {
                        "first" => arrived.notified().await,
                        "second" => {
                            arrived.notify_one();
                            let _ = resume.acquire().await;
                        }
                        _ => {}
                    }
                    params.payload
                }
            }),
        )
        .route(
            "/release",
            any(move || {
                let checkpoint = route_checkpoint.clone();
                let resume = release_resume.clone();
                async move {
                    // The guest sends this only after the first operation finished.
                    checkpoint.notify_one();
                    let _ = resume.acquire().await;
                    "released"
                }
            }),
        );
    let server =
        tokio::spawn(async move { axum::serve(listener, route).await.unwrap() }.in_current_span());

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_key!("CustomDurabilityTs", "partial-recovery");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "concurrentFirstFinishes",
                    data_value!(port as f64),
                )
                .await
        }
        .in_current_span()
    });
    tokio::time::timeout(Duration::from_secs(20), checkpoint.notified()).await?;
    executor.commit_oplog(&worker_id.agent_id).await?;

    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let start = invocation_start(&before, "concurrentFirstFinishes");
    let anchor = before[start].oplog_index;
    let entries = &before[start..];
    let siblings: Vec<_> = custom_starts(entries, anchor)
        .into_iter()
        .map(|start| {
            assert_eq!(start.function_name, GENERATOR);
            assert_eq!(start.parent, None);
            (start.index, start.ordinal)
        })
        .collect();
    assert_eq!(siblings.len(), 2);
    assert_eq!((siblings[0].1, siblings[1].1), (0, 1));
    let (first, second) = (siblings[0].0, siblings[1].0);
    assert_eq!(
        ends_of(entries, first).len(),
        1,
        "the first sibling completed"
    );
    assert_eq!(
        ends_of(entries, second).len(),
        0,
        "the second is incomplete"
    );
    // The timer of the completed body sleeps in its scope, so replay skips it with the body.
    let first_waits = starts_owned_by(entries, WAIT_FOR, Some(first));
    assert!(
        !first_waits.is_empty(),
        "the timer sleep belongs to its operation"
    );
    assert_eq!(
        starts_owned_by(entries, WAIT_FOR, None),
        Vec::<OplogIndex>::new(),
        "no timer sleep is a root call"
    );
    let release = starts_owned_by(entries, HTTP_SEND, None);
    assert_eq!(
        release.len(),
        1,
        "the release request is the only root send"
    );
    assert_eq!(ends_of(entries, release[0]).len(), 0);

    executor.simulated_crash(&worker_id).await?;
    // Closing the gate also releases requests from the discarded Store, if still connected.
    resume.close();
    assert_eq!(
        invocation.await??.into_typed::<Vec<String>>()?,
        vec!["first", "second"]
    );
    let effects = history.lock().unwrap().clone();
    assert_eq!(effects.iter().filter(|p| *p == "first").count(), 1);
    assert_eq!(effects.iter().filter(|p| *p == "second").count(), 2);

    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entries = &recovered[start..];
    let recovered_siblings: Vec<_> = custom_starts(entries, anchor)
        .into_iter()
        .map(|start| (start.index, start.ordinal))
        .collect();
    assert_eq!(
        recovered_siblings, siblings,
        "recovery keeps both logical identities"
    );
    for sibling in [first, second] {
        assert_eq!(ends_of(entries, sibling).len(), 1);
        assert!(!starts_owned_by(entries, HTTP_SEND, Some(sibling)).is_empty());
    }
    assert_eq!(
        starts_owned_by(entries, WAIT_FOR, Some(first)),
        first_waits,
        "completed replay records no new sleep"
    );
    assert_eq!(
        starts_owned_by(entries, WAIT_FOR, None),
        Vec::<OplogIndex>::new()
    );
    assert_eq!(
        starts_owned_by(entries, HTTP_SEND, None),
        release,
        "the release request completes under its original Start"
    );
    assert_eq!(ends_of(entries, release[0]).len(), 1);
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(&entry.entry, PublicOplogEntry::Jump(_))),
        "recovery must not roll back sibling history"
    );
    server.abort();
    Ok(())
}

/// The indices of the Cancelled entries of the Start at `start`.
fn cancellations_of(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Cancelled(params) if params.start_index == start => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_custom_durability_recovers_a_request_cancelled_inside_an_open_operation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let hang_arrived = Arc::new(Notify::new());
    let checkpoint = Arc::new(Notify::new());
    let resume = Arc::new(Semaphore::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }
    let route_history = history.clone();
    let route_checkpoint = checkpoint.clone();
    let route_resume = resume.clone();
    let route = Router::new().route(
        "/callback",
        any(move |Query(params): Query<Params>| {
            let history = route_history.clone();
            let hang_arrived = hang_arrived.clone();
            let checkpoint = route_checkpoint.clone();
            let resume = route_resume.clone();
            async move {
                history.lock().unwrap().push(params.payload.clone());
                match params.payload.as_str() {
                    // Never answers; the guest aborts it.
                    "hang" => {
                        hang_arrived.notify_one();
                        std::future::pending::<()>().await;
                    }
                    "hang-ready" => hang_arrived.notified().await,
                    // The operation is still open, after its request was cancelled.
                    "after-abort" => {
                        checkpoint.notify_one();
                        let _ = resume.acquire().await;
                    }
                    _ => {}
                }
                params.payload
            }
        }),
    );
    let server =
        tokio::spawn(async move { axum::serve(listener, route).await.unwrap() }.in_current_span());

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_key!("CustomDurabilityTs", "cancelled-call");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "cancelledCall",
                    data_value!(port as f64),
                )
                .await
        }
        .in_current_span()
    });
    tokio::time::timeout(Duration::from_secs(20), checkpoint.notified()).await?;
    executor.commit_oplog(&worker_id.agent_id).await?;

    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let start = invocation_start(&before, "cancelledCall");
    let anchor = before[start].oplog_index;
    let entries = &before[start..];
    let operations = custom_starts(entries, anchor);
    assert_eq!(operations.len(), 1);
    let operation = operations[0].index;
    assert_eq!(operations[0].parent, None);
    assert_eq!(
        ends_of(entries, operation).len(),
        0,
        "the operation is open"
    );
    let sends = starts_owned_by(entries, HTTP_SEND, Some(operation));
    assert_eq!(sends.len(), 3, "every request of the body belongs to it");
    let cancelled: Vec<_> = sends
        .iter()
        .copied()
        .filter(|send| !cancellations_of(entries, *send).is_empty())
        .collect();
    let ended: Vec<_> = sends
        .iter()
        .copied()
        .filter(|send| !ends_of(entries, *send).is_empty())
        .collect();
    assert_eq!(cancelled.len(), 1, "the aborted request is cancelled");
    assert_eq!(ended.len(), 1, "the ready request completed");
    assert!(
        cancellations_of(entries, cancelled[0])[0] > ends_of(entries, ended[0])[0],
        "the abort follows the ready answer"
    );
    assert_eq!(
        starts_owned_by(entries, HTTP_SEND, None),
        Vec::<OplogIndex>::new()
    );

    executor.simulated_crash(&worker_id).await?;
    resume.close();
    assert_eq!(
        invocation.await??.into_typed::<Vec<String>>()?,
        vec!["aborted-AbortError", "after-cancel"]
    );
    // Recovery of an incomplete custom operation runs its whole body again.
    let effects = history.lock().unwrap().clone();
    let count = |payload: &str| effects.iter().filter(|p| *p == payload).count();
    assert_eq!(
        count("hang"),
        2,
        "each attempt sends and aborts its own request"
    );
    assert_eq!(count("hang-ready"), 2);
    assert_eq!(count("after-abort"), 2);
    assert_eq!(count("after-cancel"), 1);

    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entries = &recovered[start..];
    let recovered_operations: Vec<_> = custom_starts(entries, anchor)
        .into_iter()
        .map(|start| start.index)
        .collect();
    assert_eq!(
        recovered_operations,
        vec![operation],
        "recovery keeps the identity of the operation"
    );
    assert_eq!(ends_of(entries, operation).len(), 1);
    let terminals = |send: OplogIndex| {
        (
            ends_of(entries, send).len(),
            cancellations_of(entries, send).len(),
        )
    };
    // The first attempt keeps its terminals; its request open at the crash stays incomplete.
    assert_eq!(terminals(cancelled[0]), (0, 1));
    assert_eq!(terminals(ended[0]), (1, 0));
    let interrupted: Vec<_> = sends
        .iter()
        .copied()
        .filter(|send| *send != cancelled[0] && *send != ended[0])
        .collect();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(terminals(interrupted[0]), (0, 0));
    // The second attempt runs in the same scope, and its abort is again a cancellation there.
    let rerun: Vec<_> = starts_owned_by(entries, HTTP_SEND, Some(operation))
        .into_iter()
        .filter(|send| !sends.contains(send))
        .collect();
    assert_eq!(rerun.len(), 3);
    let mut rerun_terminals: Vec<_> = rerun.iter().map(|send| terminals(*send)).collect();
    rerun_terminals.sort();
    assert_eq!(rerun_terminals, vec![(0, 1), (1, 0), (1, 0)]);
    assert!(rerun.iter().all(|send| *send > interrupted[0]));
    let root_sends = starts_owned_by(entries, HTTP_SEND, None);
    assert_eq!(root_sends.len(), 1);
    assert!(root_sends[0] > ends_of(entries, operation)[0]);
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(&entry.entry, PublicOplogEntry::Jump(_))),
        "recovery must not roll back history"
    );
    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_custom_durability_parent_admits_children_until_it_stops_waiting(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (port, history) = start_callback_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_key!("CustomDurabilityTs", "late-children");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "lateChildren",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], "parent");
    assert!(result[1].ends_with("-late"), "{result:?}");

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (anchor, entries) = invocation_entries(&oplog, "lateChildren");
    let starts = custom_starts(entries, anchor);
    let parent = starts
        .iter()
        .find(|start| start.function_name == GENERATOR_PARENT)
        .expect("parent Start");
    assert_eq!(parent.parent, None);
    let children: Vec<_> = starts
        .iter()
        .filter(|start| start.parent == Some(parent.index))
        .map(|start| start.index)
        .collect();
    assert_eq!(
        children.len(),
        2,
        "early and ready are children of the parent"
    );
    let roots: Vec<_> = starts
        .iter()
        .filter(|start| start.function_name == GENERATOR && start.parent.is_none())
        .collect();
    assert_eq!(roots.len(), 1, "late starts outside the parent");
    assert!(roots[0].index > children[1]);
    let parent_end = ends_of(entries, parent.index);
    assert_eq!(parent_end.len(), 1);
    for child in &children {
        let end = ends_of(entries, *child);
        assert_eq!(end.len(), 1);
        assert!(end[0] < parent_end[0], "the parent ends after its children");
    }
    assert_eq!(ends_of(entries, roots[0].index).len(), 1);
    let owners = http_send_owners(entries);
    for owner in children.iter().chain([&roots[0].index]) {
        assert_eq!(owned_sends(&owners, *owner), 1);
    }
    assert_eq!(owned_sends(&owners, parent.index), 0);

    let before_crash = history.lock().unwrap().clone();
    let mut sorted = before_crash.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["early", "late", "ready"]);

    executor.simulated_crash(&worker_id).await?;
    let after_recovery = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "callback",
            data_value!(port as f64, "after"),
        )
        .await?
        .into_typed::<String>()?;
    assert!(after_recovery.ends_with("-after"));
    let mut expected = before_crash;
    expected.push("after".to_string());
    assert_eq!(
        *history.lock().unwrap(),
        expected,
        "replay must not repeat completed effects"
    );
    Ok(())
}
