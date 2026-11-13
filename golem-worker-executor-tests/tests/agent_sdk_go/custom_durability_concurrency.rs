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

//! Custom durable operations (`durability.Run`) on concurrent goroutines of one
//! Go invocation (`CdConcAgent`): which scope every host call is attributed to
//! live, and whether a real restart replays the invocation without divergence
//! or repeated effects.

use crate::Tracing;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::schema::TypedSchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::{Notify, Semaphore};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

struct ServerState {
    history: Mutex<Vec<String>>,
    gates: Mutex<HashMap<String, Arc<Semaphore>>>,
    second_arrived: Notify,
    checkpoint: Notify,
    resume: Semaphore,
}

impl ServerState {
    fn new() -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            gates: Mutex::new(HashMap::new()),
            second_arrived: Notify::new(),
            checkpoint: Notify::new(),
            resume: Semaphore::new(0),
        }
    }
    fn gate(&self, name: &str) -> Arc<Semaphore> {
        self.gates
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(0)))
            .clone()
    }
    fn record(&self, entry: String) {
        self.history.lock().unwrap().push(entry);
    }
    fn history(&self) -> Vec<String> {
        self.history.lock().unwrap().clone()
    }
}

#[derive(Deserialize)]
struct Params {
    v: Option<String>,
    name: Option<String>,
    payload: Option<String>,
}

async fn serve() -> anyhow::Result<(Arc<ServerState>, u16, tokio::task::JoinHandle<()>)> {
    let state = Arc::new(ServerState::new());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let route = Router::new()
        .route(
            "/echo",
            get(
                |State(s): State<Arc<ServerState>>, Query(p): Query<Params>| async move {
                    let v = p.v.unwrap_or_default();
                    s.record(format!("echo:{v}"));
                    v
                },
            ),
        )
        .route(
            "/gate",
            get(
                |State(s): State<Arc<ServerState>>, Query(p): Query<Params>| async move {
                    let name = p.name.unwrap_or_default();
                    s.record(format!("gate:{name}"));
                    let _ = s.gate(&name).acquire().await.map(|p| p.forget());
                    name
                },
            ),
        )
        .route(
            "/open",
            get(
                |State(s): State<Arc<ServerState>>, Query(p): Query<Params>| async move {
                    let name = p.name.unwrap_or_default();
                    s.record(format!("open:{name}"));
                    s.gate(&name).add_permits(1);
                    "opened".to_string()
                },
            ),
        )
        .route(
            "/callback",
            get(
                |State(s): State<Arc<ServerState>>, Query(p): Query<Params>| async move {
                    let payload = p.payload.unwrap_or_default();
                    s.record(format!("callback:{payload}"));
                    match payload.as_str() {
                        "first" => s.second_arrived.notified().await,
                        "second" => {
                            s.second_arrived.notify_one();
                            let _ = s.resume.acquire().await;
                        }
                        _ => {}
                    }
                    payload
                },
            ),
        )
        .route(
            "/release",
            get(|State(s): State<Arc<ServerState>>| async move {
                s.record("release".to_string());
                s.checkpoint.notify_one();
                let _ = s.resume.acquire().await;
                "released".to_string()
            }),
        )
        .with_state(state.clone());
    let handle =
        tokio::spawn(async move { axum::serve(listener, route).await.unwrap() }.in_current_span());
    Ok((state, port, handle))
}

#[derive(Debug, Clone)]
struct Call {
    index: OplogIndex,
    function: String,
    parent: Option<OplogIndex>,
    owner: Option<OplogIndex>,
    ordinal: Option<u64>,
    request: String,
}

/// Every `Start` from the invocation of `method` up to its `AgentInvocationFinished`.
fn calls(entries: &[PublicOplogEntryWithIndex], method: &str) -> Vec<Call> {
    let mut inside = false;
    let mut out = Vec::new();
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => {
                inside = format!("{:?}", params.invocation).contains(&format!("\"{method}\""));
            }
            PublicOplogEntry::AgentInvocationFinished(_) => inside = false,
            PublicOplogEntry::Start(params) if inside => out.push(Call {
                index: entry.oplog_index,
                function: params.function_name.clone(),
                parent: params.parent_start_index,
                owner: params.observational_owner,
                ordinal: params.invocation_id.map(|id| id.ordinal),
                request: format!("{:?}", params.request),
            }),
            _ => {}
        }
    }
    out
}

fn print_calls(scenario: &str, calls: &[Call]) {
    println!("==== {scenario}: {} Starts", calls.len());
    for c in calls {
        let url = ["v=", "name=", "payload=", "release"]
            .iter()
            .find_map(|k| {
                c.request.find(k).map(|i| {
                    c.request[i..]
                        .chars()
                        .take_while(|ch| ch.is_alphanumeric() || *ch == '=' || *ch == '-')
                        .collect::<String>()
                })
            })
            .unwrap_or_default();
        println!(
            "  {:>4} {:<45} parent={:<6} owner={:<6} ordinal={:<6} {}",
            c.index.to_string(),
            c.function,
            c.parent.map(|p| p.to_string()).unwrap_or("-".into()),
            c.owner.map(|p| p.to_string()).unwrap_or("-".into()),
            c.ordinal.map(|p| p.to_string()).unwrap_or("-".into()),
            url
        );
    }
}

/// The custom-operation `Start`s and, for each, the request markers of the HTTP
/// sends it owns.
fn owned_sends(calls: &[Call], op_function: &str) -> Vec<(Call, Vec<String>)> {
    calls
        .iter()
        .filter(|c| c.function == op_function)
        .map(|op| {
            let sends = calls
                .iter()
                .filter(|c| c.function == "http::client::send" && c.owner == Some(op.index))
                .map(|c| marker(&c.request))
                .collect();
            (op.clone(), sends)
        })
        .collect()
}

fn root_sends(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .filter(|c| c.function == "http::client::send" && c.owner.is_none())
        .map(|c| marker(&c.request))
        .collect()
}

fn marker(request: &str) -> String {
    for (key, prefix) in [
        ("v=", "echo:"),
        ("gate?name=", "gate:"),
        ("open?name=", "open:"),
    ] {
        if let Some(i) = request.find(key) {
            let value: String = request[i + key.len()..]
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '-')
                .collect();
            return format!("{prefix}{value}");
        }
    }
    if let Some(i) = request.find("payload=") {
        let value: String = request[i + 8..]
            .chars()
            .take_while(|ch| ch.is_alphanumeric())
            .collect();
        return format!("callback:{value}");
    }
    if request.contains("/release") {
        return "release".to_string();
    }
    "?".to_string()
}

struct Outcome {
    live: Result<String, String>,
    calls: Vec<Call>,
    history: Vec<String>,
    replay: Result<String, String>,
    history_after_replay: Vec<String>,
    live_elapsed: Duration,
}

/// Invokes `method` once, records its oplog, restarts the executor for real and
/// invokes `echo`, which first replays the earlier invocation.
async fn run(
    deps: &WorkerExecutorTestDependencies,
    last_unique_id: &LastUniqueId,
    component: &PrecompiledComponent,
    name: &str,
    method: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<Outcome> {
    run_then(
        deps,
        last_unique_id,
        component,
        name,
        method,
        params,
        "echo",
    )
    .await
}

/// Like [`run`], with `follow_up` as the invocation after the restart.
async fn run_then(
    deps: &WorkerExecutorTestDependencies,
    last_unique_id: &LastUniqueId,
    component: &PrecompiledComponent,
    name: &str,
    method: &str,
    params: TypedSchemaValue,
    follow_up: &str,
) -> anyhow::Result<Outcome> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (state, port, server) = serve().await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("CdConcAgent", name);
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    let started = Instant::now();
    let live = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent(&component, &agent_id, method, params),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out"))
    .and_then(|r| r)
    .and_then(|r| r.into_typed::<String>())
    .map_err(|e| format!("{e:#}"));
    let live_elapsed = started.elapsed();
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let calls = calls(&entries, method);
    let history = state.history();

    drop(executor);
    let executor = start(deps, &context).await?;
    let replay = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent(&component, &agent_id, follow_up, data_value!()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out"))
    .and_then(|r| r)
    .and_then(|r| r.into_typed::<String>())
    .map_err(|e| format!("{e:#}"));
    let history_after_replay = state.history();
    drop(executor);
    server.abort();
    Ok(Outcome {
        live,
        calls,
        history,
        replay,
        history_after_replay,
        live_elapsed,
    })
}

fn report(scenario: &str, outcome: &Outcome) {
    print_calls(scenario, &outcome.calls);
    println!("  live: {:?} in {:?}", outcome.live, outcome.live_elapsed);
    println!("  server history: {:?}", outcome.history);
    println!("  replay + echo: {:?}", outcome.replay);
    println!(
        "  effects repeated on replay: {:?}",
        outcome.history_after_replay[outcome
            .history
            .len()
            .min(outcome.history_after_replay.len())..]
            .iter()
            .filter(|e| *e != "echo:echo")
            .collect::<Vec<_>>()
    );
}

fn assert_replays_cleanly(outcome: &Outcome) {
    assert_eq!(
        outcome.replay.as_deref(),
        Ok("echo"),
        "replay after restart"
    );
    let mut expected = outcome.history.clone();
    expected.push("echo:echo".to_string());
    assert_eq!(
        outcome.history_after_replay, expected,
        "completed effects are not repeated on replay"
    );
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_concurrent_sibling_custom_operations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "siblings",
        "siblings",
        data_value!(),
    )
    .await?;
    report("siblings", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("sib,b,after"));
    let ops = owned_sends(&outcome.calls, "cdconc::op");
    assert_eq!(ops.len(), 2);
    for (op, _) in &ops {
        assert_eq!(op.parent, None, "siblings are not nested");
    }
    let mut owned: Vec<_> = ops.iter().map(|(_, s)| s.clone()).collect();
    owned.sort();
    assert_eq!(
        owned,
        vec![
            vec!["echo:b".to_string(), "open:sib".to_string()],
            vec!["gate:sib".to_string()]
        ]
    );
    assert_eq!(root_sends(&outcome.calls), vec!["echo:after".to_string()]);
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_root_sdk_calls_while_custom_body_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "root",
        "root-during-body",
        data_value!(),
    )
    .await?;
    report("root-during-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("root,root"));
    let ops = owned_sends(&outcome.calls, "cdconc::op");
    assert_eq!(ops.len(), 1);
    assert_eq!(
        ops[0].1,
        vec!["open:entered-root".to_string(), "gate:root".to_string()]
    );
    assert_eq!(
        root_sends(&outcome.calls),
        vec![
            "gate:entered-root".to_string(),
            "echo:root".to_string(),
            "open:root".to_string()
        ]
    );
    let key = outcome
        .calls
        .iter()
        .find(|c| c.function == "golem::api::generate_idempotency-key")
        .expect("idempotency key call");
    assert_eq!(
        key.owner, None,
        "a root goroutine's SDK call stays root-level"
    );
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_root_stdlib_calls_while_custom_body_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "stdlib",
        "stdlib-during-body",
        data_value!(),
    )
    .await?;
    report("stdlib-during-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("stdlib"));
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_custom_body_sleeps_alone(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "sleep",
        "sleep-in-body",
        data_value!(),
    )
    .await?;
    report("sleep-in-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("slept"));
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_custom_body_sleeps_while_root_goroutine_runs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "sleep-sib",
        "sleep-with-sibling",
        data_value!(),
    )
    .await?;
    report("sleep-with-sibling", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("slept,r1,r2"));
    assert_eq!(
        root_sends(&outcome.calls),
        vec!["echo:r1".to_string(), "echo:r2".to_string()]
    );
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_custom_body_children_inherit_its_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "children",
        "children",
        data_value!(),
    )
    .await?;
    report("children", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("c0,c1"));
    let ops = owned_sends(&outcome.calls, "cdconc::parent");
    assert_eq!(ops.len(), 1);
    let mut owned = ops[0].1.clone();
    owned.sort();
    assert_eq!(owned, vec!["echo:c0".to_string(), "echo:c1".to_string()]);
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_concurrent_nested_custom_operations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "nested",
        "nested",
        data_value!(),
    )
    .await?;
    report("nested", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("nested,i1"));
    let outer: Vec<_> = outcome
        .calls
        .iter()
        .filter(|c| c.function == "cdconc::outer")
        .collect();
    assert_eq!(outer.len(), 1);
    let inner = owned_sends(&outcome.calls, "cdconc::inner");
    assert_eq!(inner.len(), 2);
    for (op, _) in &inner {
        assert_eq!(
            op.parent,
            Some(outer[0].index),
            "inner operations nest under outer"
        );
    }
    let mut owned: Vec<_> = inner.iter().map(|(_, s)| s.clone()).collect();
    owned.sort();
    assert_eq!(
        owned,
        vec![
            vec!["echo:i1".to_string(), "open:nested".to_string()],
            vec!["gate:nested".to_string()]
        ]
    );
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_goroutine_outliving_custom_body_keeps_it_open(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "escaped",
        "escaped",
        data_value!(),
    )
    .await?;
    report("escaped", &outcome);
    let live = outcome
        .live
        .expect_err("Run waits for the goroutine its body started");
    assert!(live.contains("failed to produce a result"), "{live}");
    assert!(
        outcome.history.is_empty(),
        "the goroutine never ran past the body"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_custom_durability_recovers_with_one_completed_and_one_incomplete_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (state, port, server) = serve().await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("CdConcAgent", "partial");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "echo", data_value!())
        .await?;
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &agent_id, "partial", data_value!())
                .await
        }
        .in_current_span()
    });
    tokio::time::timeout(Duration::from_secs(30), state.checkpoint.notified()).await?;
    executor.commit_oplog(&worker_id).await?;
    let before = calls(
        &executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?,
        "partial",
    );
    print_calls("partial (before crash)", &before);

    executor.simulated_crash(&worker_id).await?;
    state.resume.close();
    let result = tokio::time::timeout(Duration::from_secs(60), invocation).await;
    let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let after = calls(&entries, "partial");
    print_calls("partial (after recovery)", &after);
    println!("  result: {result:?}");
    println!("  server history: {:?}", state.history());
    let result = result???.into_typed::<String>()?;
    assert_eq!(result, "first,second");
    let history = state.history();
    assert_eq!(history.iter().filter(|e| *e == "callback:first").count(), 1);
    assert_eq!(
        history.iter().filter(|e| *e == "callback:second").count(),
        2
    );
    let ops = |calls: &[Call]| {
        let mut ops: Vec<_> = calls
            .iter()
            .filter(|c| c.function == "cdconc::op")
            .map(|c| (c.index, c.ordinal))
            .collect();
        ops.sort();
        ops
    };
    assert_eq!(
        ops(&after),
        ops(&before),
        "recovery retains both logical identities"
    );
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(&entry.entry, PublicOplogEntry::Jump(_))),
        "call-local recovery does not roll back sibling history"
    );
    drop(executor);
    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_root_timer_pending_while_custom_body_runs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "root-sleep",
        "root-sleep-during-body",
        data_value!(),
    )
    .await?;
    report("root-sleep-during-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("b1b2b3,root"));
    assert_eq!(root_sends(&outcome.calls), vec!["echo:root".to_string()]);
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_gc_triggered_inside_custom_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "gc",
        "gc-in-body",
        data_value!(),
    )
    .await?;
    report("gc-in-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("gc2016,rootroot2"));
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_gc_after_sequential_custom_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "gcseq",
        "gc-after-body",
        data_value!(),
    )
    .await?;
    report("gc-after-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("b6,r0r1r2r3r4r5r6r7"));
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_body_clock_reads_while_root_timer_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "body-clock",
        "body-clock-root-timer",
        data_value!(),
    )
    .await?;
    report("body-clock-root-timer", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("b,root"));
    assert_replays_cleanly(&outcome);
    Ok(())
}

/// The value an invocation kept in memory must be the same after the restart
/// replays that invocation.
fn assert_memory_survives_replay(outcome: &Outcome) {
    assert!(outcome.live.is_ok(), "live: {:?}", outcome.live);
    assert_eq!(outcome.replay, outcome.live, "value in memory after replay");
    assert_eq!(
        outcome.history_after_replay, outcome.history,
        "completed effects are not repeated on replay"
    );
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_runtime_random_source_after_sequential_custom_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run_then(
        deps,
        last_unique_id,
        agent_sdk_go,
        "rand-body",
        "rand-after-body",
        data_value!(),
        "remembered",
    )
    .await?;
    report("rand-after-body", &outcome);
    assert_memory_survives_replay(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_custom_body_timer_due_before_root_timer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "body-timer",
        "body-timer-first",
        data_value!(),
    )
    .await?;
    report("body-timer-first", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("b,r"));
    let ops = owned_sends(&outcome.calls, "cdconc::op");
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].1, vec!["echo:b".to_string()]);
    assert_eq!(root_sends(&outcome.calls), vec!["echo:r".to_string()]);
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_sibling_custom_bodies_sleep_on_timers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "sib-timers",
        "sibling-timers",
        data_value!(),
    )
    .await?;
    report("sibling-timers", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("s0,s1"));
    for name in ["s0", "s1"] {
        let ops = owned_sends(&outcome.calls, &format!("cdconc::{name}"));
        assert_eq!(ops.len(), 1, "{name}");
        assert_eq!(ops[0].0.parent, None, "{name} is a root operation");
        assert_eq!(ops[0].1, vec![format!("echo:{name}")]);
    }
    assert!(root_sends(&outcome.calls).is_empty());
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_after_func_callback_belongs_to_custom_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "after-func",
        "after-func-in-body",
        data_value!(),
    )
    .await?;
    report("after-func-in-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("af,r"));
    let ops = owned_sends(&outcome.calls, "cdconc::op");
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].1, vec!["echo:af".to_string()]);
    assert_eq!(root_sends(&outcome.calls), vec!["echo:r".to_string()]);
    assert_replays_cleanly(&outcome);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_timers_of_a_finished_custom_body_are_stopped(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let outcome = run(
        deps,
        last_unique_id,
        agent_sdk_go,
        "outlive",
        "timer-outlives-body",
        data_value!(),
    )
    .await?;
    report("timer-outlives-body", &outcome);
    assert_eq!(outcome.live.as_deref(), Ok("b,r"));
    assert!(!outcome.history.iter().any(|e| e == "echo:late"));
    assert_eq!(root_sends(&outcome.calls), vec!["echo:r".to_string()]);
    assert_replays_cleanly(&outcome);
    Ok(())
}
