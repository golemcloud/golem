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

//! Custom durability scopes of the MoonBit SDK across concurrent coroutines of one
//! invocation, driven through the `agent-sdk-moonbit` guest. Mirrors the custom
//! durability rows of `durability.rs`.

use crate::Tracing;
use axum::Router;
use axum::body::Bytes;
use axum::extract::Query;
use axum::routing::{any, post};
use golem_common::model::AgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::Notify;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_moonbit")]
    PrecompiledComponent
);

const GENERATOR: &str = "golem-it::test-generator";
const SEND: &str = "http::client::send";
const CONSUME_BODY: &str = "http::types::response::consume-body";

#[derive(Deserialize)]
struct Params {
    payload: String,
}

type History = Arc<Mutex<Vec<String>>>;

struct Fixture {
    executor: TestWorkerExecutor,
    component: ComponentDto,
    agent_id: golem_common::model::agent::ParsedAgentKey,
    worker_id: AgentId,
    server: tokio::task::JoinHandle<()>,
}

async fn fixture(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_sdk_moonbit: &PrecompiledComponent,
    name: &str,
    route: Router,
) -> anyhow::Result<Fixture> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let server =
        tokio::spawn(async move { axum::serve(listener, route).await.unwrap() }.in_current_span());
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_moonbit)
        .store()
        .await?;
    let agent_id = agent_id!("CustomDurability", name);
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "warmup", data_value!())
        .await?;
    Ok(Fixture {
        executor,
        component,
        agent_id,
        worker_id,
        server,
    })
}

fn recording_route(history: &History) -> Router {
    let history = history.clone();
    Router::new().route(
        "/callback",
        any(move |Query(params): Query<Params>| {
            let history = history.clone();
            async move {
                history.lock().unwrap().push(params.payload.clone());
                params.payload
            }
        }),
    )
}

fn anchor(oplog: &[PublicOplogEntryWithIndex], method: &str) -> OplogIndex {
    oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(&params.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(call) if call.method_name == method) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("agent invocation {method}"))
}

/// Custom `Start`s of `function_name` under `anchor`, ordered by ordinal, with
/// their parent `Start` index.
fn custom_starts(
    oplog: &[PublicOplogEntryWithIndex],
    function_name: &str,
    anchor: OplogIndex,
) -> Vec<(OplogIndex, u64, Option<OplogIndex>)> {
    let mut starts: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == function_name
                    && params.invocation_id.is_some_and(|id| id.anchor == anchor) =>
            {
                Some((
                    entry.oplog_index,
                    params.invocation_id.unwrap().ordinal,
                    params.parent_start_index,
                ))
            }
            _ => None,
        })
        .collect();
    starts.sort_by_key(|(_, ordinal, _)| *ordinal);
    starts
}

/// Effects of `function_name` that a custom scope owns as observations.
fn owned(oplog: &[PublicOplogEntryWithIndex], function_name: &str, owner: OplogIndex) -> usize {
    oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
            PublicOplogEntry::Start(params) if params.function_name == function_name
                && params.observational_owner == Some(owner)
                && params.invocation_id.is_none())
        })
        .count()
}

/// Effects of `function_name` that run at the root of the agent invocation.
fn root(
    oplog: &[PublicOplogEntryWithIndex],
    function_name: &str,
    anchor: OplogIndex,
) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == function_name
                    && params.invocation_id.is_some_and(|id| id.anchor == anchor) =>
            {
                assert_eq!(params.observational_owner, None);
                assert_eq!(params.parent_start_index, None);
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

fn ends(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
            PublicOplogEntry::End(params) if params.start_index == start)
        })
        .count()
}

async fn invoke_strings(fixture: &Fixture, method: &str) -> anyhow::Result<Vec<String>> {
    fixture
        .executor
        .invoke_and_await_agent(&fixture.component, &fixture.agent_id, method, data_value!())
        .await?
        .into_typed::<Vec<String>>()
}

/// Crashes the agent, recovers it through the next invocation, and checks that replay
/// repeats no recorded effect.
async fn assert_replay_repeats_nothing(fixture: &Fixture, history: &History) -> anyhow::Result<()> {
    let before = history.lock().unwrap().clone();
    fixture.executor.simulated_crash(&fixture.worker_id).await?;
    let after = fixture
        .executor
        .invoke_and_await_agent(
            &fixture.component,
            &fixture.agent_id,
            "callback",
            data_value!("after"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(after, "after");
    let mut expected = before;
    expected.push("after".to_string());
    assert_eq!(*history.lock().unwrap(), expected);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_custom_scopes_stay_with_their_coroutines(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let history = History::default();
    let slow_recorded = Arc::new(Notify::new());
    let fast_recorded = Arc::new(Notify::new());
    let root_recorded = Arc::new(Notify::new());
    let route_history = history.clone();
    // Initiation order is slow, fast, root. Completion order is fast, root, slow: the root
    // call runs while the slow scope waits.
    let route = Router::new().route(
        "/callback",
        any(move |Query(params): Query<Params>| {
            let history = route_history.clone();
            let slow_recorded = slow_recorded.clone();
            let fast_recorded = fast_recorded.clone();
            let root_recorded = root_recorded.clone();
            async move {
                match params.payload.as_str() {
                    "slow" => {
                        history.lock().unwrap().push(params.payload.clone());
                        slow_recorded.notify_one();
                        root_recorded.notified().await;
                    }
                    "fast" => {
                        slow_recorded.notified().await;
                        history.lock().unwrap().push(params.payload.clone());
                        fast_recorded.notify_one();
                    }
                    "root" => {
                        fast_recorded.notified().await;
                        history.lock().unwrap().push(params.payload.clone());
                        root_recorded.notify_one();
                    }
                    _ => history.lock().unwrap().push(params.payload.clone()),
                }
                params.payload
            }
        }),
    );
    let fixture = fixture(
        last_unique_id,
        deps,
        agent_sdk_moonbit,
        "concurrent-siblings",
        route,
    )
    .await?;

    assert_eq!(
        invoke_strings(&fixture, "concurrent").await?,
        vec!["slow", "fast", "root"]
    );
    assert_eq!(*history.lock().unwrap(), vec!["slow", "fast", "root"]);

    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let anchor = anchor(&oplog, "concurrent");
    let siblings = custom_starts(&oplog, GENERATOR, anchor);
    assert_eq!(
        siblings
            .iter()
            .map(|(_, ordinal, _)| *ordinal)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    for (start, _, parent) in &siblings {
        assert_eq!(*parent, None, "identical siblings are not nested");
        assert_eq!(ends(&oplog, *start), 1);
        assert_eq!(
            owned(&oplog, SEND, *start),
            1,
            "each sibling owns its own send"
        );
        assert_eq!(owned(&oplog, CONSUME_BODY, *start), 1);
    }
    assert_eq!(
        root(&oplog, SEND, anchor).len(),
        1,
        "the root call made while a sibling scope waits belongs to no scope"
    );
    assert_eq!(root(&oplog, CONSUME_BODY, anchor).len(), 1);

    assert_replay_repeats_nothing(&fixture, &history).await?;
    fixture.server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_nested_custom_scopes_keep_their_parent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let history = History::default();
    let fixture = fixture(
        last_unique_id,
        deps,
        agent_sdk_moonbit,
        "nested",
        recording_route(&history),
    )
    .await?;

    assert_eq!(
        invoke_strings(&fixture, "nested").await?,
        vec!["nested+parent-own", "root-sibling"]
    );

    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let anchor = anchor(&oplog, "nested");
    let outer = custom_starts(&oplog, "golem-it::test-generator-parent", anchor);
    assert_eq!(outer.len(), 1);
    let (outer_start, _, outer_parent) = outer[0];
    assert_eq!(outer_parent, None);
    assert!(custom_starts(&oplog, GENERATOR, anchor).is_empty());
    // A nested custom invocation is identified relative to its parent's Start.
    let inner = custom_starts(&oplog, GENERATOR, outer_start);
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].1, 0);
    assert_eq!(inner[0].2, Some(outer_start), "the inner scope is nested");
    assert_eq!(owned(&oplog, SEND, inner[0].0), 1);
    assert_eq!(owned(&oplog, CONSUME_BODY, inner[0].0), 1);
    assert_eq!(
        owned(&oplog, SEND, outer_start),
        1,
        "the parent owns its own effect"
    );
    assert_eq!(root(&oplog, SEND, anchor).len(), 1);

    assert_replay_repeats_nothing(&fixture, &history).await?;
    fixture.server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_custom_scopes_recover_with_one_completed_and_one_suspended_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let history = History::default();
    let second_arrived = Arc::new(Notify::new());
    let checkpoint = Arc::new(Notify::new());
    let resume = Arc::new(tokio::sync::Semaphore::new(0));
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
                    // The guest sends this only after the first scope finished.
                    checkpoint.notify_one();
                    let _ = resume.acquire().await;
                    "released"
                }
            }),
        );
    let fixture = fixture(
        last_unique_id,
        deps,
        agent_sdk_moonbit,
        "partial-recovery",
        route,
    )
    .await?;
    let invocation = tokio::spawn({
        let executor = fixture.executor.clone();
        let component = fixture.component.clone();
        let agent_id = fixture.agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &agent_id, "first_finishes", data_value!())
                .await
        }
        .in_current_span()
    });
    tokio::time::timeout(Duration::from_secs(20), checkpoint.notified()).await?;
    fixture.executor.commit_oplog(&fixture.worker_id).await?;
    let before = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let anchor = anchor(&before, "first_finishes");
    let siblings: Vec<_> = custom_starts(&before, GENERATOR, anchor)
        .into_iter()
        .map(|(start, ordinal, parent)| {
            assert_eq!(parent, None);
            (start, ordinal)
        })
        .collect();
    assert_eq!(
        siblings
            .iter()
            .map(|(_, ordinal)| *ordinal)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(ends(&before, siblings[0].0), 1);
    assert_eq!(ends(&before, siblings[1].0), 0);
    let release = root(&before, SEND, anchor);
    assert_eq!(release.len(), 1, "the release request started at the root");
    assert_eq!(ends(&before, release[0]), 0);

    fixture.executor.simulated_crash(&fixture.worker_id).await?;
    resume.close();
    assert_eq!(
        invocation.await??.into_typed::<Vec<String>>()?,
        vec!["first", "second"]
    );
    let effects = history.lock().unwrap().clone();
    assert_eq!(effects.iter().filter(|p| *p == "first").count(), 1);
    assert_eq!(
        effects.iter().filter(|p| *p == "second").count(),
        2,
        "only the incomplete sibling runs again"
    );
    let recovered = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let recovered_siblings: Vec<_> = custom_starts(&recovered, GENERATOR, anchor)
        .into_iter()
        .map(|(start, ordinal, _)| (start, ordinal))
        .collect();
    assert_eq!(
        recovered_siblings, siblings,
        "recovery keeps both identities"
    );
    for (start, _) in &siblings {
        assert_eq!(ends(&recovered, *start), 1);
        assert!(owned(&recovered, SEND, *start) >= 1);
    }
    assert!(
        recovered
            .iter()
            .all(|entry| !matches!(&entry.entry, PublicOplogEntry::Jump(_))),
        "call-local recovery must not roll back sibling history"
    );
    assert_eq!(
        root(&recovered, SEND, anchor),
        release,
        "the release request completes under its original Start"
    );
    assert_eq!(ends(&recovered, release[0]), 1);
    fixture.server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_custom_scopes_clean_up_after_failure_and_cancellation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let history = History::default();
    let hang_recorded = Arc::new(Notify::new());
    let route_history = history.clone();
    let route = Router::new().route(
        "/callback",
        any(move |Query(params): Query<Params>| {
            let history = route_history.clone();
            let hang_recorded = hang_recorded.clone();
            async move {
                match params.payload.as_str() {
                    "hang" => {
                        history.lock().unwrap().push(params.payload.clone());
                        hang_recorded.notify_one();
                        std::future::pending::<()>().await;
                    }
                    "trigger" => {
                        hang_recorded.notified().await;
                        history.lock().unwrap().push(params.payload.clone());
                    }
                    _ => history.lock().unwrap().push(params.payload.clone()),
                }
                params.payload
            }
        }),
    );
    let fixture = fixture(last_unique_id, deps, agent_sdk_moonbit, "failures", route).await?;

    assert_eq!(
        invoke_strings(&fixture, "failures").await?,
        vec!["caught", "cancelled", "after-failures"]
    );
    assert_eq!(
        *history.lock().unwrap(),
        vec!["fail-body", "hang", "trigger", "after-failures"]
    );

    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let anchor = anchor(&oplog, "failures");
    let failing = custom_starts(&oplog, "golem-it::test-failing", anchor);
    assert_eq!(failing.len(), 1);
    assert_eq!(
        ends(&oplog, failing[0].0),
        0,
        "a failed body is not finished"
    );
    assert_eq!(owned(&oplog, SEND, failing[0].0), 1);
    let cancelled = custom_starts(&oplog, GENERATOR, anchor);
    assert_eq!(cancelled.len(), 1);
    assert_eq!(
        ends(&oplog, cancelled[0].0),
        0,
        "a cancelled body is not finished"
    );
    assert_eq!(owned(&oplog, SEND, cancelled[0].0), 1);
    assert_eq!(
        root(&oplog, SEND, anchor).len(),
        2,
        "the failing sibling and the call after both scopes belong to no scope"
    );
    fixture.server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_custom_scope_rejects_escaped_tasks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let history = History::default();
    let fixture = fixture(
        last_unique_id,
        deps,
        agent_sdk_moonbit,
        "escaped",
        recording_route(&history),
    )
    .await?;

    let result = fixture
        .executor
        .invoke_and_await_agent(
            &fixture.component,
            &fixture.agent_id,
            "escaped",
            data_value!(),
        )
        .await;
    // Release MoonBit builds do not carry abort messages, so the abort shows as a trap.
    let error = format!(
        "{:?}",
        result.expect_err("an escaped task fails the invocation")
    );
    assert!(
        error.contains("DeterministicTrap") && error.contains("unreachable"),
        "unexpected error: {error}"
    );
    assert!(
        history.lock().unwrap().is_empty(),
        "the body exit aborts before the escaped task runs"
    );
    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let escaped = custom_starts(&oplog, "golem-it::test-escaped", anchor(&oplog, "escaped"));
    assert_eq!(escaped.len(), 1);
    assert_eq!(
        ends(&oplog, escaped[0].0),
        0,
        "the escaping scope is not finished"
    );
    fixture.server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_custom_scope_owns_its_stream_producer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let route = Router::new().route(
        "/echo",
        post(|body: Bytes| async move { String::from_utf8(body.to_vec()).unwrap() }),
    );
    let fixture = fixture(last_unique_id, deps, agent_sdk_moonbit, "producer", route).await?;

    let echoed = fixture
        .executor
        .invoke_and_await_agent(
            &fixture.component,
            &fixture.agent_id,
            "producer",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(echoed, "streamed");

    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let anchor = anchor(&oplog, "producer");
    let upload = custom_starts(&oplog, "golem-it::test-upload", anchor);
    assert_eq!(upload.len(), 1);
    assert_eq!(ends(&oplog, upload[0].0), 1);
    assert_eq!(owned(&oplog, SEND, upload[0].0), 1);
    let root_effects: BTreeSet<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.invocation_id.is_some_and(|id| id.anchor == anchor)
                    && params.function_name != "golem-it::test-upload" =>
            {
                Some(params.function_name.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        root_effects,
        BTreeSet::new(),
        "the producer's effects stay in the scope"
    );
    fixture.server.abort();
    Ok(())
}

fn end_index(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Option<OplogIndex> {
    oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::End(params) if params.start_index == start => Some(entry.oplog_index),
        _ => None,
    })
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_custom_scope_waits_for_its_future_producer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let history = History::default();
    let early_history = history.clone();
    let route = recording_route(&history).route(
        "/early",
        post(move || {
            let history = early_history.clone();
            async move {
                history.lock().unwrap().push("early".to_string());
                "early".to_string()
            }
        }),
    );
    let fixture = fixture(
        last_unique_id,
        deps,
        agent_sdk_moonbit,
        "future-producer",
        route,
    )
    .await?;

    assert_eq!(
        invoke_strings(&fixture, "future_producer").await?,
        vec!["early", "after-producer"]
    );

    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let anchor = anchor(&oplog, "future_producer");
    let upload = custom_starts(&oplog, "golem-it::test-trailers", anchor);
    assert_eq!(upload.len(), 1);
    let scope = upload[0].0;
    let scope_end = end_index(&oplog, scope).expect("the scope is finished");
    let owned_sends: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == SEND
                    && params.observational_owner == Some(scope)
                    && params.invocation_id.is_none() =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        owned_sends.len(),
        4,
        "the scope owns the request, its own call and both producer calls"
    );
    assert!(
        owned_sends.iter().all(|send| *send < scope_end),
        "the scope finishes after the producer that outlived its body"
    );
    let root_sends = root(&oplog, SEND, anchor);
    assert_eq!(root_sends.len(), 1);
    assert!(scope_end < root_sends[0]);
    let calls = history.lock().unwrap().clone();
    // The request and the producer's first call race; the rest is ordered.
    assert_eq!(
        calls[..2].iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([&"early".to_string(), &"trailer".to_string()])
    );
    assert_eq!(
        calls[2..],
        ["body-returned", "producer-after-body", "after-producer"]
    );

    assert_replay_repeats_nothing(&fixture, &history).await?;
    fixture.server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn moonbit_custom_scope_cancelled_while_finishing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_moonbit")] agent_sdk_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let history = History::default();
    let fixture = fixture(
        last_unique_id,
        deps,
        agent_sdk_moonbit,
        "cancel-finish",
        recording_route(&history),
    )
    .await?;

    assert_eq!(
        invoke_strings(&fixture, "cancel_finish").await?,
        vec!["during-finish", "after-finish-cancel"],
        "the sibling fails while the scope waits in finish-async"
    );
    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    let anchor = anchor(&oplog, "cancel_finish");
    let scope = custom_starts(&oplog, "golem-it::test-finish-cancelled", anchor);
    assert_eq!(scope.len(), 1);
    // The cancellation can arrive before or after the host appends the End.
    assert!(ends(&oplog, scope[0].0) <= 1);
    assert_eq!(owned(&oplog, SEND, scope[0].0), 0);
    assert_eq!(root(&oplog, SEND, anchor).len(), 1);

    assert_replay_repeats_nothing(&fixture, &history).await?;
    let oplog = fixture
        .executor
        .get_oplog(&fixture.worker_id, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        custom_starts(&oplog, "golem-it::test-finish-cancelled", anchor).len(),
        1,
        "recovery does not start the cancelled scope again"
    );
    assert_eq!(
        ends(&oplog, scope[0].0),
        1,
        "recovery leaves the scope finished"
    );
    fixture.server.abort();
    Ok(())
}
